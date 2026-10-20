//! The common file dialogs (`GetOpenFileName`, `GetSaveFileName`), built from NLE's own controls as a dialog template
//! with a native dialog procedure: a path box and an Up button, a list of the folder's entries, the file name, the
//! file type filter and the buttons. What the program asked for (`OPENFILENAME`) is read once, the answer is written
//! back the way Windows does: the full path, the offset of the name and of the extension, the chosen filter.

use super::api;
use super::dialog;
use super::gdi::Object;
use super::imagelist::ImageList;
use super::msgbox;
use super::res::IconImage;
use super::*;
use crate::runtime::paths;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

pub const OFN_OVERWRITEPROMPT: u32 = 0x2;
pub const OFN_NOCHANGEDIR: u32 = 0x8;
pub const OFN_ALLOWMULTISELECT: u32 = 0x200;
pub const OFN_PATHMUSTEXIST: u32 = 0x800;
pub const OFN_FILEMUSTEXIST: u32 = 0x1000;
pub const OFN_NOVALIDATE: u32 = 0x100;
pub const OFN_PICKFOLDERS: u32 = 0x2000_0000;

pub const FNERR_BUFFERTOOSMALL: u32 = 0x3003;

const ID_PATH: usize = 1001;
const ID_UP: usize = 1002;
const ID_LIST: usize = 1003;
const ID_NAME: usize = 1004;
const ID_TYPE: usize = 1005;

/// The last error a common dialog reported (`CommDlgExtendedError`).
static LAST_ERROR: AtomicU32 = AtomicU32::new(0);

pub fn extended_error() -> u32 {
    LAST_ERROR.load(Ordering::Relaxed)
}

pub fn set_extended_error(code: u32) {
    LAST_ERROR.store(code, Ordering::Relaxed);
}

/// What the program asked for.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub save: bool,
    pub flags: u32,
    pub title: String,
    pub initial_dir: String,
    pub initial_file: String,
    pub def_ext: String,
    /// (description, patterns)
    pub filters: Vec<(String, Vec<String>)>,
    pub filter_index: usize,
}

/// One row of the list.
#[derive(Clone, Debug)]
struct Entry {
    name: String,
    dir: bool,
    size: u64,
    modified: i64,
}

struct State {
    req: Request,
    dir: PathBuf,
    entries: Vec<Entry>,
    filter: usize,
    /// a pattern typed into the file name box ("*.txt") overrides the filter
    typed: Option<String>,
    chosen: Vec<PathBuf>,
}

static STATES: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);

fn state_of(dlg: usize) -> Option<&'static mut State> {
    let p = STATES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&dlg).copied())?;
    // the state lives on the stack of `run`, which outlives its dialog
    Some(unsafe { &mut *(p as *mut State) })
}

// ---- pure helpers ---------------------------------------------------------------------------------------------

/// Splits "Description\0*.a;*.b\0..." (already decoded into separate strings) into (description, patterns).
pub fn parse_filters(parts: &[String]) -> Vec<(String, Vec<String>)> {
    parts
        .chunks(2)
        .filter(|c| c.len() == 2 && !c[0].is_empty())
        .map(|c| (c[0].clone(), c[1].split(';').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()))
        .collect()
}

pub fn pattern_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    crate::shim::k32_file::wildcard(&p, &n)
}

fn matches_any(patterns: &[String], name: &str) -> bool {
    patterns.is_empty() || patterns.iter().any(|p| p == "*.*" || p == "*" || pattern_matches(p, name))
}

/// The names in a quoted list: `"a b.txt" "c.txt"`, or a single name without quotes.
pub fn split_names(text: &str) -> Vec<String> {
    let t = text.trim();
    if !t.contains('"') {
        return if t.is_empty() { Vec::new() } else { vec![t.to_string()] };
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in t.chars() {
        match c {
            '"' => {
                if quoted && !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                quoted = !quoted;
            }
            c if quoted => cur.push(c),
            _ => {}
        }
    }
    out
}

fn human_size(bytes: u64) -> String {
    if bytes == 0 {
        return "0 KB".into();
    }
    format!("{} KB", bytes.div_ceil(1024))
}

fn file_type(e: &Entry) -> String {
    if e.dir {
        return "File folder".into();
    }
    match Path::new(&e.name).extension() {
        Some(x) => format!("{} File", x.to_string_lossy().to_uppercase()),
        None => "File".into(),
    }
}

fn format_time(secs: i64) -> String {
    unsafe {
        let t = secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let h12 = if tm.tm_hour % 12 == 0 { 12 } else { tm.tm_hour % 12 };
        format!("{}/{}/{} {}:{:02} {}", tm.tm_mon + 1, tm.tm_mday, tm.tm_year + 1900, h12, tm.tm_min, if tm.tm_hour < 12 { "AM" } else { "PM" })
    }
}

/// The entries of a folder that the filter lets through: folders first, then files, each by name.
fn read_entries(dir: &Path, patterns: &[String], folders_only: bool) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            let is_dir = md.is_dir();
            if !is_dir && (folders_only || !matches_any(patterns, &name)) {
                continue;
            }
            let modified = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
            out.push(Entry { name, dir: is_dir, size: if is_dir { 0 } else { md.len() }, modified });
        }
    }
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// Where a typed name points: an absolute Windows path as it is, anything else inside `dir`.
fn resolve_typed(dir: &Path, text: &str) -> PathBuf {
    let t = text.trim();
    let absolute = paths::is_absolute_win(t) || t.starts_with('/') || t.starts_with('\\');
    if absolute {
        paths::resolve(t)
    } else {
        dir.join(t.replace('\\', "/"))
    }
}

/// Offsets into the finished path: where the name starts and where its extension starts (0 when there is none).
pub fn offsets(win_path: &str) -> (usize, usize) {
    let u: Vec<u16> = win_path.encode_utf16().collect();
    let name = u.iter().rposition(|c| *c == b'\\' as u16 || *c == b'/' as u16).map_or(0, |i| i + 1);
    let ext = u[name..].iter().rposition(|c| *c == b'.' as u16).map_or(0, |i| name + i + 1);
    (name, ext)
}

// ---- the dialog template ---------------------------------------------------------------------------------------

struct Template(Vec<u8>);

impl Template {
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn wstr(&mut self, s: &str) {
        for u in s.encode_utf16() {
            self.u16(u);
        }
        self.u16(0);
    }
    fn align(&mut self) {
        while self.0.len() % 4 != 0 {
            self.0.push(0);
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn item(&mut self, style: u32, x: i16, y: i16, cx: i16, cy: i16, id: u16, class: &str, title: &str) {
        self.align();
        self.u32(style | WS_CHILD | WS_VISIBLE);
        self.u32(0);
        for v in [x, y, cx, cy] {
            self.u16(v as u16);
        }
        self.u16(id);
        self.wstr(class);
        self.wstr(title);
        self.u16(0);
    }
}

fn build_template(req: &Request) -> Vec<u8> {
    const DS_MODALFRAME: u32 = 0x80;
    const DS_SETFONT: u32 = 0x40;
    const WS_BORDER: u32 = 0x0080_0000;
    const WS_VSCROLL: u32 = 0x0020_0000;
    const WS_TABSTOP: u32 = 0x0001_0000;
    const WS_GROUP: u32 = 0x0002_0000;
    const ES_AUTOHSCROLL: u32 = 0x80;
    const CBS_DROPDOWNLIST: u32 = 3;
    const LVS_REPORT: u32 = 1;
    const LVS_SINGLESEL: u32 = 4;
    const LVS_SHOWSELALWAYS: u32 = 8;
    const LVS_SHAREIMAGELISTS: u32 = 0x40;
    let multi = req.flags & OFN_ALLOWMULTISELECT != 0;
    let mut t = Template(Vec::new());
    t.u32(WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME | DS_SETFONT | WS_VISIBLE);
    t.u32(0);
    t.u16(10);
    for v in [0i16, 0, 330, 192] {
        t.u16(v as u16);
    }
    t.u16(0); // menu
    t.u16(0); // class
    t.wstr(&req.title);
    t.u16(9);
    t.wstr("Segoe UI");
    let label = if req.flags & OFN_PICKFOLDERS != 0 { "Folder:" } else { "File &name:" };
    t.item(0, 7, 9, 36, 8, 0xffff, "STATIC", "Look &in:");
    t.item(WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL, 45, 7, 214, 14, ID_PATH as u16, "EDIT", "");
    t.item(WS_TABSTOP, 265, 6, 58, 15, ID_UP as u16, "BUTTON", "&Up");
    t.item(WS_BORDER | WS_TABSTOP | LVS_REPORT | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS | if multi { 0 } else { LVS_SINGLESEL }, 7, 26, 316, 118, ID_LIST as u16, "SysListView32", "");
    t.item(0, 7, 153, 46, 8, 0xffff, "STATIC", label);
    t.item(WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL, 55, 150, 204, 14, ID_NAME as u16, "EDIT", "");
    t.item(0, 7, 171, 46, 8, 0xffff, "STATIC", "Files of &type:");
    t.item(WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST, 55, 168, 204, 80, ID_TYPE as u16, "COMBOBOX", "");
    t.item(WS_TABSTOP | WS_GROUP | 1, 265, 150, 58, 15, 1, "BUTTON", if req.save { "&Save" } else { "&Open" });
    t.item(WS_TABSTOP, 265, 168, 58, 15, 2, "BUTTON", "Cancel");
    // the item count sits after style, extended style
    let count = 10u16;
    t.0[8..10].copy_from_slice(&count.to_le_bytes());
    t.0
}

// ---- talking to the controls ---------------------------------------------------------------------------------

fn item(dlg: usize, id: usize) -> usize {
    with(|g| g.windows.get(&dlg).and_then(|w| w.children.iter().copied().find(|c| g.windows.get(c).is_some_and(|cw| cw.id == id)))).unwrap_or(0)
}

unsafe fn get_text(h: usize) -> String {
    let mut buf = vec![0u16; 4096];
    let n = api::send(h, 0x000D, buf.len(), buf.as_mut_ptr() as isize) as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

unsafe fn set_text(h: usize, text: &str) {
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    api::send(h, 0x000C, 0, w.as_ptr() as isize);
}

fn folder_icon() -> IconImage {
    let (w, h) = (16, 16);
    let mut rgb = vec![0u32; 256];
    let mut alpha = vec![0u8; 256];
    for y in 3..14 {
        for x in 1..15 {
            let tab = y < 5 && x > 7;
            if tab {
                continue;
            }
            let edge = y == 3 || y == 13 || x == 1 || x == 14 || (y == 5 && x > 7);
            rgb[y * 16 + x] = if edge { 0xB58A1B } else if y < 6 { 0xF3D675 } else { 0xF7DE8D };
            alpha[y * 16 + x] = 255;
        }
    }
    for x in 1..8 {
        rgb[2 * 16 + x] = 0xB58A1B;
        alpha[2 * 16 + x] = 255;
    }
    IconImage { w, h, rgb, alpha, hotspot: (0, 0) }
}

fn file_icon() -> IconImage {
    let (w, h) = (16, 16);
    let mut rgb = vec![0u32; 256];
    let mut alpha = vec![0u8; 256];
    for y in 1..15 {
        for x in 3..13 {
            let corner = x > 9 && y < 5 && (x - 9) >= (5 - y);
            if corner && x > 9 && y < 4 {
                continue;
            }
            let edge = y == 1 || y == 14 || x == 3 || x == 12;
            rgb[y * 16 + x] = if edge { 0x7F7F7F } else { 0xFFFFFF };
            alpha[y * 16 + x] = 255;
            if !edge && y > 5 && y % 2 == 0 && x > 4 && x < 11 {
                rgb[y * 16 + x] = 0x6C9BD2;
            }
        }
    }
    IconImage { w, h, rgb, alpha, hotspot: (0, 0) }
}

unsafe fn setup_columns(list: usize) {
    let cols = [("Name", 210, 0u32), ("Size", 62, 1), ("Type", 90, 0), ("Date modified", 120, 0)];
    for (i, (name, width, fmt)) in cols.iter().enumerate() {
        let text: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut col = [0u8; 40];
        col[0..4].copy_from_slice(&0xFu32.to_le_bytes());
        col[4..8].copy_from_slice(&fmt.to_le_bytes());
        col[8..12].copy_from_slice(&(*width as u32).to_le_bytes());
        col[16..24].copy_from_slice(&(text.as_ptr() as u64).to_le_bytes());
        col[28..32].copy_from_slice(&(i as u32).to_le_bytes());
        api::send(list, 0x1061, i, col.as_ptr() as isize);
    }
    let il = with(|g| {
        let mut l = ImageList::new(16, 16, 0x20);
        l.add_icon(&folder_icon(), None);
        l.add_icon(&file_icon(), None);
        g.add_object(Object::ImageList(Box::new(l)))
    });
    api::send(list, 0x1003, 1, il as isize);
    // full row select
    api::send(list, 0x1036, 0x20, 0x20);
}

unsafe fn fill_list(dlg: usize, st: &State) {
    let list = item(dlg, ID_LIST);
    api::send(list, 0x1009, 0, 0);
    for (i, e) in st.entries.iter().enumerate() {
        let name: Vec<u16> = e.name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut it = [0u8; 88];
        it[0..4].copy_from_slice(&0x3u32.to_le_bytes()); // LVIF_TEXT | LVIF_IMAGE
        it[4..8].copy_from_slice(&(i as u32).to_le_bytes());
        it[24..32].copy_from_slice(&(name.as_ptr() as u64).to_le_bytes());
        it[36..40].copy_from_slice(&(if e.dir { 0u32 } else { 1u32 }).to_le_bytes());
        api::send(list, 0x104D, 0, it.as_ptr() as isize);
        let subs = [if e.dir { String::new() } else { human_size(e.size) }, file_type(e), format_time(e.modified)];
        for (k, s) in subs.iter().enumerate() {
            let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
            let mut sub = [0u8; 88];
            sub[8..12].copy_from_slice(&((k + 1) as u32).to_le_bytes());
            sub[24..32].copy_from_slice(&(w.as_ptr() as u64).to_le_bytes());
            api::send(list, 0x1074, i, sub.as_ptr() as isize);
        }
    }
}

fn active_patterns(st: &State) -> Vec<String> {
    match &st.typed {
        Some(p) => vec![p.clone()],
        None => st.req.filters.get(st.filter).map(|f| f.1.clone()).unwrap_or_default(),
    }
}

unsafe fn refresh(dlg: usize, st: &mut State) {
    let patterns = active_patterns(st);
    st.entries = read_entries(&st.dir, &patterns, st.req.flags & OFN_PICKFOLDERS != 0);
    set_text(item(dlg, ID_PATH), &paths::host_to_win(&st.dir));
    fill_list(dlg, st);
}

unsafe fn navigate(dlg: usize, st: &mut State, to: PathBuf) {
    if to.is_dir() {
        st.dir = to.canonicalize().unwrap_or(to);
        st.typed = None;
        refresh(dlg, st);
    }
}

unsafe fn selected(dlg: usize, st: &State) -> Vec<usize> {
    let list = item(dlg, ID_LIST);
    let mut out = Vec::new();
    let mut i = -1isize;
    loop {
        i = api::send(list, 0x100C, i as usize, 2); // LVM_GETNEXTITEM, LVNI_SELECTED
        if i < 0 || i as usize >= st.entries.len() {
            break;
        }
        out.push(i as usize);
    }
    out
}

/// Finishes the dialog with `files`, after the checks the flags ask for. False when the user has to try again.
unsafe fn accept(dlg: usize, st: &mut State, names: Vec<String>) -> bool {
    let flags = st.req.flags;
    let mut chosen: Vec<PathBuf> = Vec::new();
    for n in &names {
        let mut p = resolve_typed(&st.dir, n);
        if st.req.save && p.extension().is_none() && !st.req.def_ext.is_empty() && !p.is_dir() {
            let mut s = p.into_os_string();
            s.push(".");
            s.push(st.req.def_ext.trim_start_matches('.'));
            p = PathBuf::from(s);
        }
        chosen.push(p);
    }
    for p in &chosen {
        let shown = paths::host_to_win(p);
        if flags & OFN_PICKFOLDERS != 0 {
            if !p.is_dir() {
                msgbox::message_box(dlg, &format!("{shown}\nThe folder does not exist."), &st.req.title, 0x10);
                return false;
            }
            continue;
        }
        if flags & OFN_NOVALIDATE == 0 {
            if flags & (OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST) != 0 && p.parent().is_some_and(|d| !d.is_dir()) {
                msgbox::message_box(dlg, &format!("{shown}\nThe path does not exist.\nCheck the path and try again."), &st.req.title, 0x30);
                return false;
            }
            if !st.req.save && flags & OFN_FILEMUSTEXIST != 0 && !p.is_file() {
                msgbox::message_box(dlg, &format!("{shown}\nFile not found.\nCheck the file name and try again."), &st.req.title, 0x30);
                return false;
            }
        }
        if st.req.save && flags & OFN_OVERWRITEPROMPT != 0 && p.is_file() {
            let answer = msgbox::message_box(dlg, &format!("{shown} already exists.\nDo you want to replace it?"), &st.req.title, 0x34);
            if answer != 6 {
                return false;
            }
        }
    }
    st.chosen = chosen;
    dialog::end_dialog(dlg, 1);
    true
}

unsafe fn on_ok(dlg: usize, st: &mut State) {
    // Enter in the path box means "go there"
    let focus = with(|g| g.focus);
    if focus == item(dlg, ID_PATH) {
        let typed = get_text(focus);
        let p = resolve_typed(&st.dir, &typed);
        navigate(dlg, st, p);
        return;
    }
    let name_box = item(dlg, ID_NAME);
    let text = get_text(name_box);
    let names = split_names(&text);
    if names.is_empty() {
        // nothing typed: a selected folder is opened, selected files are taken
        let sel = selected(dlg, st);
        if let Some(e) = sel.iter().filter_map(|i| st.entries.get(*i)).find(|e| e.dir) {
            let p = st.dir.join(&e.name);
            if st.req.flags & OFN_PICKFOLDERS != 0 {
                accept(dlg, st, vec![p.to_string_lossy().into_owned()]);
            } else {
                navigate(dlg, st, p);
            }
            return;
        }
        if st.req.flags & OFN_PICKFOLDERS != 0 {
            let cur = st.dir.to_string_lossy().into_owned();
            accept(dlg, st, vec![cur]);
        }
        return;
    }
    if names.len() == 1 && (names[0].contains('*') || names[0].contains('?')) {
        st.typed = Some(names[0].clone());
        refresh(dlg, st);
        return;
    }
    if names.len() == 1 {
        let p = resolve_typed(&st.dir, &names[0]);
        if p.is_dir() && st.req.flags & OFN_PICKFOLDERS == 0 {
            navigate(dlg, st, p);
            set_text(name_box, "");
            return;
        }
    }
    accept(dlg, st, names);
}

unsafe fn on_list_selection(dlg: usize, st: &State) {
    let sel = selected(dlg, st);
    let names: Vec<&str> = sel.iter().filter_map(|i| st.entries.get(*i)).filter(|e| !e.dir || st.req.flags & OFN_PICKFOLDERS != 0).map(|e| e.name.as_str()).collect();
    let text = match names.len() {
        0 => return,
        1 => names[0].to_string(),
        _ => names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(" "),
    };
    set_text(item(dlg, ID_NAME), &text);
}

unsafe fn on_activate(dlg: usize, st: &mut State, index: usize) {
    let Some(e) = st.entries.get(index).cloned() else { return };
    if e.dir {
        let p = st.dir.join(&e.name);
        navigate(dlg, st, p);
    } else {
        accept(dlg, st, vec![e.name]);
    }
}

pub unsafe extern "win64" fn dialog_proc(dlg: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_INITDIALOG => {
            STATES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(dlg, lp as usize);
            let Some(st) = state_of(dlg) else { return 0 };
            let font = with(|g| g.stock_object(17));
            let _ = font;
            setup_columns(item(dlg, ID_LIST));
            let combo = item(dlg, ID_TYPE);
            for (desc, _) in &st.req.filters {
                let w: Vec<u16> = desc.encode_utf16().chain(std::iter::once(0)).collect();
                api::send(combo, 0x143, 0, w.as_ptr() as isize);
            }
            api::send(combo, 0x14E, st.filter, 0);
            if !st.req.initial_file.is_empty() {
                set_text(item(dlg, ID_NAME), &st.req.initial_file);
            }
            refresh(dlg, st);
            0
        }
        // the focus goes to the file name once the dialog is on screen (it cannot before)
        WM_SHOWWINDOW => {
            if wp != 0 {
                let name = item(dlg, ID_NAME);
                api::set_focus(name);
                // a name filled in by the program is selected, so typing replaces it
                api::send(name, 0xB1, 0, -1);
            }
            0
        }
        WM_COMMAND => {
            let (id, code) = (loword(wp), (wp >> 16) & 0xffff);
            let Some(st) = state_of(dlg) else { return 0 };
            match id as usize {
                1 => on_ok(dlg, st),
                2 => {
                    dialog::end_dialog(dlg, 0);
                }
                ID_UP => {
                    if let Some(parent) = st.dir.parent().map(Path::to_path_buf) {
                        navigate(dlg, st, parent);
                    }
                }
                ID_TYPE if code == 1 => {
                    let i = api::send(item(dlg, ID_TYPE), 0x147, 0, 0);
                    if i >= 0 {
                        st.filter = i as usize;
                        st.typed = None;
                        refresh(dlg, st);
                    }
                }
                _ => {}
            }
            1
        }
        WM_NOTIFY => {
            let hdr = lp as *const u8;
            let (from, code) = (*(hdr as *const usize), *(hdr.add(16) as *const i32));
            let Some(st) = state_of(dlg) else { return 0 };
            if from != item(dlg, ID_LIST) {
                return 0;
            }
            match code {
                // LVN_ITEMCHANGED: the selection moved
                -101 => {
                    let new_state = *(hdr.add(32) as *const u32);
                    if new_state & 2 != 0 {
                        on_list_selection(dlg, st);
                    }
                }
                // LVN_ITEMACTIVATE and NM_DBLCLK
                -114 | -3 => {
                    let index = *(hdr.add(24) as *const i32);
                    if index >= 0 {
                        on_activate(dlg, st, index as usize);
                    }
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            dialog::end_dialog(dlg, 0);
            1
        }
        WM_DESTROY => {
            if let Some(m) = STATES.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                m.remove(&dlg);
            }
            0
        }
        _ => 0,
    }
}

/// Runs the dialog. The chosen paths, and the filter in use, on success.
pub unsafe fn run(owner: usize, req: Request) -> Option<(Vec<PathBuf>, usize)> {
    let has_display = with(|g| g.ensure_backend().is_some());
    if !has_display {
        return None;
    }
    let start = {
        let dir = if !req.initial_dir.is_empty() {
            paths::resolve(&req.initial_dir)
        } else if let Some(d) = Path::new(&req.initial_file.replace('\\', "/")).parent().filter(|d| !d.as_os_str().is_empty() && req.initial_file.contains(['\\', '/'])) {
            paths::resolve(&paths::host_to_win(d))
        } else {
            paths::cwd()
        };
        if dir.is_dir() { dir } else { paths::cwd() }
    };
    let initial_file = Path::new(&req.initial_file.replace('\\', "/")).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut req = req;
    req.initial_file = initial_file;
    let filter = req.filter_index.saturating_sub(1).min(req.filters.len().saturating_sub(1));
    let mut st = State { dir: start, entries: Vec::new(), filter, typed: None, chosen: Vec::new(), req };
    let template = build_template(&st.req);
    let r = dialog::dialog_box_param(0, Some(&template), owner, dialog_proc as *const () as usize, &mut st as *mut State as isize, false);
    if r == 1 && !st.chosen.is_empty() { Some((std::mem::take(&mut st.chosen), st.filter + 1)) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_pair_a_description_with_patterns() {
        let parts: Vec<String> = ["Text (*.txt)", "*.txt", "All files", "*.*"].iter().map(|s| s.to_string()).collect();
        let f = parse_filters(&parts);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].1, vec!["*.txt".to_string()]);
        let multi = parse_filters(&["Source".into(), "*.c; *.h".into()]);
        assert_eq!(multi[0].1, vec!["*.c".to_string(), "*.h".to_string()]);
    }

    #[test]
    fn patterns_ignore_case() {
        assert!(pattern_matches("*.TXT", "notes.txt"));
        assert!(pattern_matches("a?c.*", "abc.dat"));
        assert!(!pattern_matches("*.txt", "notes.txt.bak"));
        assert!(matches_any(&[], "anything"));
        assert!(matches_any(&["*.*".into()], "noext"));
    }

    #[test]
    fn quoted_names_are_split() {
        assert_eq!(split_names("one.txt"), vec!["one.txt"]);
        assert_eq!(split_names("\"a b.txt\" \"c.txt\""), vec!["a b.txt", "c.txt"]);
        assert!(split_names("  ").is_empty());
    }

    #[test]
    fn offsets_point_at_the_name_and_the_extension() {
        assert_eq!(offsets("C:\\dir\\file.txt"), (7, 12));
        assert_eq!(offsets("C:\\dir\\noext"), (7, 0));
        assert_eq!(offsets("C:\\a.b\\file"), (7, 0), "a dot in a folder name is not an extension");
    }

    #[test]
    fn entries_list_folders_first_and_apply_the_filter() {
        let dir = std::env::temp_dir().join(format!("nle-filedlg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zdir")).unwrap();
        std::fs::write(dir.join("b.txt"), b"x").unwrap();
        std::fs::write(dir.join("a.TXT"), b"x").unwrap();
        std::fs::write(dir.join("c.dat"), b"x").unwrap();
        std::fs::write(dir.join(".hidden"), b"x").unwrap();
        let names: Vec<String> = read_entries(&dir, &["*.txt".to_string()], false).into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["zdir", "a.TXT", "b.txt"]);
        let folders: Vec<String> = read_entries(&dir, &[], true).into_iter().map(|e| e.name).collect();
        assert_eq!(folders, vec!["zdir"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_and_types_read_like_explorer() {
        assert_eq!(human_size(1), "1 KB");
        assert_eq!(human_size(2048), "2 KB");
        let f = Entry { name: "photo.png".into(), dir: false, size: 10, modified: 0 };
        assert_eq!(file_type(&f), "PNG File");
        assert_eq!(file_type(&Entry { dir: true, ..f }), "File folder");
    }

    #[test]
    fn the_template_parses_back() {
        let req = Request { title: "Open".into(), filters: vec![("All".into(), vec!["*.*".into()])], ..Default::default() };
        let t = dialog::parse_template(&build_template(&req)).expect("a valid template");
        assert_eq!(t.items.len(), 10);
        assert_eq!(t.title, "Open");
        assert!(t.items.iter().any(|i| i.id == ID_LIST as u32 && dialog::class_name(&i.class).as_deref() == Some("SysListView32")));
    }
}
