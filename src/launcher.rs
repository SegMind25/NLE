//! `nle gui`: a desktop window for NLE itself. It browses folders for Windows programs, shows what `inspect` says about
//! the one you pick (verdict, imports, what is missing), runs it, converts it to a Linux ELF, and checks the system.
//!
//! The window is built from NLE's own window system (the same software-rendered controls Windows programs get), so
//! it needs nothing beyond what NLE already uses: an X11 display, or the scripted headless display for tests.
//! Running, relinking and the doctor are done by starting `nle` again and showing its output.

use crate::analyze::{self, Report, Status, Verdict};
use crate::backend;
use crate::gui::font::{Family, Sized};
use crate::gui::gdi::{LogFont, Object};
use crate::gui::imagelist::ImageList;
use crate::gui::menu::{Accel, FCONTROL, FSHIFT, FVIRTKEY};
use crate::gui::msg::{self, Filter, Next};
use crate::gui::surface::{Brush, Canvas, Pen, PenStyle, Rect, Surface};
use crate::gui::wm::{Class, ClassRef, CreateArgs};
use crate::gui::{self, api, with, *};
use crate::pe::{Layout, Pe};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;

// ---- identifiers ---------------------------------------------------------------------------------------------

const ID_PATH: usize = 102;
const ID_PLACES: usize = 103;
const ID_UP: usize = 104;
const ID_TABS: usize = 105;
const ID_LIST: usize = 106;
const ID_IMPORTS: usize = 107;
const ID_MISSING: usize = 108;
const ID_LOG: usize = 109;
const ID_SYSTEM: usize = 110;
const ID_STATUS: usize = 111;

const CMD_RUN: usize = 201;
const CMD_STRICT: usize = 202;
const CMD_RELINK: usize = 203;
const CMD_STOP: usize = 204;
const CMD_DOCTOR: usize = 205;
const CMD_REFRESH: usize = 206;
const CMD_CLEAR: usize = 207;
const CMD_QUIT: usize = 301;
const CMD_ABOUT: usize = 302;
const CMD_FOCUS_PATH: usize = 303;
const CMD_PREFIX: usize = 304;
const CMD_HOME: usize = 305;

const TIMER_POLL: usize = 1;

const GREEN: u32 = 0x16A34A;
const AMBER: u32 = 0xD97706;
const RED: u32 = 0xDC2626;
const INK: u32 = 0x1F2937;
const MUTED: u32 = 0x6B7280;
const PAGE: u32 = 0xF3F4F6;

// ---- state -----------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Up,
    Dir,
    Exe,
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    path: PathBuf,
    kind: Kind,
    size: u64,
    verdict: Option<Verdict>,
}

/// What the overview card shows; computed when the selection changes so painting is cheap.
#[derive(Clone, Debug, Default)]
struct Overview {
    title: String,
    subtitle: String,
    verdict: Option<Verdict>,
    pills: Vec<String>,
    implemented: usize,
    total: usize,
    notes: Vec<String>,
    runs_with: String,
    error: Option<String>,
}

struct Job {
    label: String,
    pid: u32,
    rx: Receiver<JobMsg>,
    /// What to do when it ends: refresh the listing after a relink.
    relink: bool,
}

enum JobMsg {
    Line(String),
    Done(i32),
}

#[derive(Default, Clone, Copy)]
struct Handles {
    main: usize,
    path: usize,
    places: usize,
    up: usize,
    list: usize,
    tabs: usize,
    imports: usize,
    missing: usize,
    log: usize,
    system: usize,
    status: usize,
    buttons: [usize; 6],
}

struct App {
    h: Handles,
    dir: PathBuf,
    entries: Vec<Entry>,
    selected: Option<usize>,
    report: Option<Report>,
    view: Overview,
    job: Option<Job>,
    log_events: bool,
    accel: usize,
    wine: Option<PathBuf>,
    places: Vec<(String, PathBuf)>,
    icons: usize,
    mono: usize,
    /// Set while the listing is being rebuilt, so the selection notifications it causes are ignored.
    filling: bool,
    pending_select: Option<PathBuf>,
}

static APP: Mutex<Option<App>> = Mutex::new(None);

fn app<R>(f: impl FnOnce(&mut App) -> R) -> R {
    let mut g = APP.lock().unwrap_or_else(|e| e.into_inner());
    f(g.as_mut().expect("the launcher is not running"))
}

fn handles() -> Handles {
    app(|a| a.h)
}

// ---- small helpers ----------------------------------------------------------------------------------------------

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c == '\r' {
            continue;
        }
        if c == '\t' {
            out.push(' ');
            continue;
        }
        out.push(c);
    }
    out
}

fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn verdict_color(v: Verdict) -> u32 {
    match v {
        Verdict::Native => GREEN,
        Verdict::Partial => AMBER,
        Verdict::Unsupported => RED,
    }
}

fn verdict_icon(v: Option<Verdict>) -> i32 {
    match v {
        Some(Verdict::Native) => 2,
        Some(Verdict::Partial) => 3,
        Some(Verdict::Unsupported) => 4,
        None => 1,
    }
}

/// Shortens `s` to fit `w` pixels, ending in "..." when it had to be cut.
fn fit(font: &Sized, s: &str, w: i32) -> String {
    if font.text_width(s) <= w {
        return s.to_string();
    }
    let mut t: Vec<char> = s.chars().collect();
    while !t.is_empty() && font.text_width(&format!("{}...", t.iter().collect::<String>())) > w {
        t.pop();
    }
    format!("{}...", t.iter().collect::<String>())
}

/// Breaks text into lines of at most `w` pixels (at most `max` lines; the last one is cut with "...").
fn wrap(font: &Sized, text: &str, w: i32, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let next = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if font.text_width(&next) <= w || cur.is_empty() {
            cur = next;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > max {
        lines.truncate(max);
        if let Some(last) = lines.last_mut() {
            *last = fit(font, &format!("{last} ..."), w);
        }
    }
    lines.into_iter().map(|l| fit(font, &l, w)).collect()
}

fn wide(s: &str) -> Vec<u16> {
    crate::runtime::strings::to_wide_nul(s)
}

unsafe fn send_text(h: usize, msg: u32, wp: usize, text: &str) -> isize {
    let w = wide(text);
    api::send(h, msg, wp, w.as_ptr() as isize)
}

/// The directory listing: ".." (when there is a parent), folders, then the .exe files, each group by name.
fn list_dir(dir: &Path) -> Vec<Entry> {
    let (mut dirs, mut exes) = (Vec::new(), Vec::new());
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let Ok(md) = std::fs::metadata(&path) else { continue };
            if md.is_dir() {
                dirs.push(Entry { name, path, kind: Kind::Dir, size: 0, verdict: None });
            } else if md.is_file() && name.to_lowercase().ends_with(".exe") {
                exes.push(Entry { name, path, kind: Kind::Exe, size: md.len(), verdict: None });
            }
        }
    }
    dirs.sort_by_key(|e| e.name.to_lowercase());
    exes.sort_by_key(|e| e.name.to_lowercase());
    let mut out = Vec::new();
    if let Some(parent) = dir.parent() {
        out.push(Entry { name: "..".into(), path: parent.to_path_buf(), kind: Kind::Up, size: 0, verdict: None });
    }
    out.extend(dirs);
    out.extend(exes);
    out
}

fn analyze_file(path: &Path) -> Result<Report, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let pe = Pe::parse(&bytes, Layout::File).map_err(|e| e.to_string())?;
    Ok(analyze::analyze(&pe, Some(path)))
}

/// The verdict shown in the listing; skips very large files and stops after a few hundred programs.
fn quick_verdict(e: &Entry) -> Option<Verdict> {
    if e.size > 64 * 1024 * 1024 {
        return None;
    }
    analyze_file(&e.path).ok().map(|r| r.verdict)
}

fn system_rows() -> Vec<(bool, String, String)> {
    let mut rows = Vec::new();
    rows.push((true, "NLE".to_string(), format!("version {} on {}", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH)));
    rows.push((cfg!(target_arch = "x86_64"), "x86-64 CPU".to_string(), "Windows x64 code runs natively, so the host must be x86-64".to_string()));
    let prefix = crate::runtime::paths::default_prefix();
    rows.push((true, "Windows prefix".to_string(), prefix.display().to_string()));
    let display = if std::env::var("NLE_GUI").is_ok_and(|v| v.eq_ignore_ascii_case("headless")) {
        (true, "headless (scripted)".to_string())
    } else {
        match std::env::var("DISPLAY") {
            Ok(d) if !d.is_empty() => (true, d),
            _ => (false, "DISPLAY is not set".to_string()),
        }
    };
    rows.push((display.0, "Display".to_string(), display.1));
    let sans = crate::gui::font::face(Family::Sans, false, false).is_some();
    rows.push((sans, "Fonts for window text".to_string(), if sans { "TrueType fonts found".to_string() } else { "none found (install fonts-liberation or fonts-dejavu-core)".to_string() }));
    match crate::cli::runtime_lib() {
        Some(p) => rows.push((true, "libnle.so (for relink)".to_string(), p.display().to_string())),
        None => rows.push((false, "libnle.so (for relink)".to_string(), "not found next to the nle binary".to_string())),
    }
    match backend::find_wine() {
        Some(w) => rows.push((true, "Wine fallback".to_string(), w.display().to_string())),
        None => rows.push((false, "Wine fallback".to_string(), "not installed; only matters for programs the native layer cannot run".to_string())),
    }
    let bf = std::fs::read_to_string("/proc/sys/fs/binfmt_misc/nle").is_ok();
    rows.push((bf, "binfmt_misc handler".to_string(), if bf { "registered: .exe files run directly".to_string() } else { "not registered (optional, see `nle binfmt`)".to_string() }));
    rows
}

// ---- icons -------------------------------------------------------------------------------------------------------

/// 16x16 icons for the listing: 0 folder, 1 program, 2 native, 3 partial, 4 unsupported, 5 up.
fn make_icons(g: &mut Gui) -> usize {
    const KEY: u32 = 0xFF00FF;
    let mut list = ImageList::new(16, 16, 0x20);
    for which in 0..6 {
        let mut s = Surface::new(16, 16);
        let b = s.bounds();
        s.fill(b, KEY);
        {
            let mut cv = Canvas::new(&mut s, 0, 0, b);
            let none = Pen { style: PenStyle::Null, width: 1, color: 0 };
            let solid = |c: u32, w: i32| Pen { style: PenStyle::Solid, width: w, color: c };
            match which {
                0 => {
                    cv.fill_rect(Rect::new(1, 3, 7, 5), 0xD9A21B);
                    cv.fill_rect(Rect::new(1, 4, 15, 14), 0xF2C14E);
                    cv.fill_rect(Rect::new(1, 6, 15, 14), 0xF7D36B);
                    cv.frame_rect(Rect::new(1, 4, 15, 14), 0xB07F10, 1);
                }
                1 => {
                    cv.fill_rect(Rect::new(1, 2, 15, 14), 0x5B6574);
                    cv.fill_rect(Rect::new(2, 3, 14, 13), 0xFFFFFF);
                    cv.fill_rect(Rect::new(2, 3, 14, 6), 0x3B82F6);
                    cv.fill_rect(Rect::new(4, 8, 12, 9), 0xC9CED6);
                    cv.fill_rect(Rect::new(4, 10, 9, 11), 0xC9CED6);
                }
                2 => {
                    cv.ellipse(Rect::new(0, 0, 16, 16), &none, &Brush::Solid(GREEN));
                    cv.line(4, 8, 7, 11, &solid(0xFFFFFF, 2));
                    cv.line(7, 11, 12, 5, &solid(0xFFFFFF, 2));
                }
                3 => {
                    cv.polygon(&[(8, 0), (15, 14), (1, 14)], &none, &Brush::Solid(AMBER), false);
                    cv.fill_rect(Rect::new(7, 5, 9, 10), 0xFFFFFF);
                    cv.fill_rect(Rect::new(7, 11, 9, 13), 0xFFFFFF);
                }
                4 => {
                    cv.ellipse(Rect::new(0, 0, 16, 16), &none, &Brush::Solid(RED));
                    cv.line(5, 5, 11, 11, &solid(0xFFFFFF, 2));
                    cv.line(11, 5, 5, 11, &solid(0xFFFFFF, 2));
                }
                _ => {
                    cv.polygon(&[(8, 2), (14, 9), (10, 9), (10, 14), (6, 14), (6, 9), (2, 9)], &none, &Brush::Solid(0x6B7280), false);
                }
            }
        }
        list.add_strip(&s, None, Some(KEY));
    }
    g.add_object(Object::ImageList(Box::new(list)))
}

// ---- controls -----------------------------------------------------------------------------------------------------

#[repr(C)]
struct LvItem {
    mask: u32,
    item: i32,
    sub: i32,
    state: u32,
    state_mask: u32,
    text: *const u16,
    cch: i32,
    image: i32,
    param: isize,
    indent: i32,
}

#[repr(C)]
struct LvColumn {
    mask: u32,
    fmt: i32,
    cx: i32,
    text: *const u16,
    cch: i32,
    sub: i32,
}

#[repr(C)]
struct TcItem {
    mask: u32,
    state: u32,
    state_mask: u32,
    text: *const u16,
    cch: i32,
    image: i32,
    param: isize,
}

unsafe fn lv_column(h: usize, i: usize, text: &str, width: i32, right: bool) {
    let w = wide(text);
    let c = LvColumn { mask: 1 | 2 | 4, fmt: right as i32, cx: width, text: w.as_ptr(), cch: 0, sub: 0 };
    api::send(h, 0x1061, i, &c as *const LvColumn as isize);
}

unsafe fn lv_row(h: usize, index: usize, image: i32, cells: &[String]) {
    let first = wide(cells.first().map_or("", |s| s.as_str()));
    let it = LvItem { mask: 1 | 2, item: index as i32, sub: 0, state: 0, state_mask: 0, text: first.as_ptr(), cch: 0, image, param: 0, indent: 0 };
    api::send(h, 0x104D, 0, &it as *const LvItem as isize);
    for (k, c) in cells.iter().enumerate().skip(1) {
        let w = wide(c);
        let sub = LvItem { mask: 1, item: index as i32, sub: k as i32, state: 0, state_mask: 0, text: w.as_ptr(), cch: 0, image: 0, param: 0, indent: 0 };
        api::send(h, 0x1074, index, &sub as *const LvItem as isize);
    }
}

unsafe fn lv_select(h: usize, index: usize) {
    let st = LvItem { mask: 8, item: 0, sub: 0, state: 3, state_mask: 3, text: std::ptr::null(), cch: 0, image: 0, param: 0, indent: 0 };
    api::send(h, 0x102B, index, &st as *const LvItem as isize);
    api::send(h, 0x1013, index, 0);
}

unsafe fn child(class: &str, text: &str, style: u32, ex: u32, id: usize, parent: usize, font: usize) -> usize {
    let h = api::create_window_ex(CreateArgs { ex_style: ex, class: ClassRef::Name(class.into()), title: text.to_string(), style: WS_CHILD | style, x: 0, y: 0, w: 10, h: 10, parent, menu: id, hinstance: 0, param: 0, ansi: false });
    if h != 0 && font != 0 {
        api::send(h, WM_SETFONT, font, 0);
    }
    h
}

fn set_enabled(h: usize, on: bool) {
    with(|g| {
        if let Some(w) = g.windows.get_mut(&h) {
            if on {
                w.style &= !WS_DISABLED;
            } else {
                w.style |= WS_DISABLED;
            }
        }
        g.invalidate(h, None, true);
    });
}

unsafe fn status(part: usize, text: &str) {
    let h = handles().status;
    if h != 0 {
        send_text(h, 0x40B, part, text);
    }
}

// ---- events (for tests and scripts) ---------------------------------------------------------------------------------

fn event(line: String) {
    if app(|a| a.log_events) {
        println!("{line}");
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
}

// ---- the log -----------------------------------------------------------------------------------------------------

unsafe fn log_line(text: &str, echo: bool) {
    let log = handles().log;
    if log == 0 {
        return;
    }
    send_text(log, 0x180, 0, text);
    let n = api::send(log, 0x18B, 0, 0);
    if n > 5000 {
        api::send(log, 0x182, 0, 0);
    }
    let n = api::send(log, 0x18B, 0, 0);
    api::send(log, 0x197, (n.max(1) - 1) as usize, 0);
    if echo {
        event(format!("OUT {text}"));
    }
}

unsafe fn clear_log() {
    let log = handles().log;
    if log != 0 {
        api::send(log, 0x184, 0, 0);
    }
}

unsafe fn show_page(i: usize) {
    let h = handles();
    for (k, w) in [h.imports, h.missing, h.log, h.system].into_iter().enumerate() {
        api::show_window(w, if k == i { SW_SHOW } else { SW_HIDE });
    }
}

unsafe fn select_tab(i: usize) {
    let h = handles();
    api::send(h.tabs, 0x130C, i, 0);
    show_page(i);
}

// ---- jobs ---------------------------------------------------------------------------------------------------------

fn spawn_job(label: &str, args: Vec<String>, relink: bool) -> Result<Job, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(exe).args(&args).env_remove("NLE_GUI_SCRIPT").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(JobMsg::Line(strip_ansi(&line)));
            }
        }));
    }
    if let Some(err) = child.stderr.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = tx.send(JobMsg::Line(strip_ansi(&line)));
            }
        }));
    }
    std::thread::spawn(move || {
        use std::os::unix::process::ExitStatusExt;
        let status = child.wait();
        for r in readers {
            let _ = r.join();
        }
        let code = match status {
            Ok(s) => s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)),
            Err(_) => -1,
        };
        let _ = tx.send(JobMsg::Done(code));
    });
    Ok(Job { label: label.to_string(), pid, rx, relink })
}

unsafe fn start_job(label: &str, args: Vec<String>, relink: bool) {
    if app(|a| a.job.is_some()) {
        return;
    }
    match spawn_job(label, args, relink) {
        Ok(job) => {
            let main = handles().main;
            app(|a| a.job = Some(job));
            select_tab(2);
            log_line(&format!("$ nle {label}"), false);
            status(0, &format!("running {label} ..."));
            update_buttons();
            msg::set_timer(main, TIMER_POLL, 40, 0);
        }
        Err(e) => {
            log_line(&format!("cannot start nle: {e}"), true);
        }
    }
}

/// Moves what the running job printed into the log; handles its end.
unsafe fn poll_job() {
    let mut lines = Vec::new();
    let mut done = None;
    app(|a| {
        if let Some(job) = &a.job {
            while let Ok(m) = job.rx.try_recv() {
                match m {
                    JobMsg::Line(l) => lines.push(l),
                    JobMsg::Done(c) => {
                        done = Some((c, job.label.clone(), job.relink));
                        break;
                    }
                }
            }
        }
        if done.is_some() {
            a.job = None;
        }
    });
    for l in lines {
        log_line(&l, true);
    }
    if let Some((code, label, relink)) = done {
        let main = handles().main;
        msg::kill_timer(main, TIMER_POLL);
        log_line(&format!("[exit code {code}]"), false);
        event(format!("EXIT {label} {code}"));
        status(0, &format!("{label} finished with exit code {code}"));
        if relink {
            event(format!("RELINK {}", if code == 0 { "ok" } else { "failed" }));
            refresh();
        }
        update_buttons();
    }
}

fn stop_job() {
    let pid = app(|a| a.job.as_ref().map(|j| j.pid));
    if let Some(pid) = pid {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
}

fn update_buttons() {
    let (h, running, exe, partial_ok) = app(|a| (a.h, a.job.is_some(), a.selected.and_then(|i| a.entries.get(i)).is_some_and(|e| e.kind == Kind::Exe) && a.report.is_some(), true));
    let _ = partial_ok;
    let idle_exe = exe && !running;
    set_enabled(h.buttons[0], idle_exe);
    set_enabled(h.buttons[1], idle_exe);
    set_enabled(h.buttons[2], idle_exe);
    set_enabled(h.buttons[3], running);
    set_enabled(h.buttons[4], !running);
}

// ---- navigation and selection ------------------------------------------------------------------------------------------

/// Reads the current folder again and rebuilds the list.
fn refresh() {
    let dir = app(|a| a.dir.clone());
    navigate_to(dir, app(|a| a.selected.and_then(|i| a.entries.get(i)).map(|e| e.path.clone())));
}

fn navigate_to(dir: PathBuf, select: Option<PathBuf>) {
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let mut entries = list_dir(&dir);
    let mut analysed = 0;
    for e in entries.iter_mut().filter(|e| e.kind == Kind::Exe) {
        if analysed >= 300 {
            break;
        }
        e.verdict = quick_verdict(e);
        analysed += 1;
    }
    let (native, partial, unsupported) = entries.iter().fold((0, 0, 0), |(n, p, u), e| match e.verdict {
        Some(Verdict::Native) => (n + 1, p, u),
        Some(Verdict::Partial) => (n, p + 1, u),
        Some(Verdict::Unsupported) => (n, p, u + 1),
        None => (n, p, u),
    });
    let programs = entries.iter().filter(|e| e.kind == Kind::Exe).count();
    let h = handles();
    unsafe {
        app(|a| {
            a.filling = true;
            a.dir = dir.clone();
            a.entries = entries.clone();
            a.selected = None;
            a.report = None;
        });
        api::send(h.list, 0x1009, 0, 0);
        for (i, e) in entries.iter().enumerate() {
            let (image, size, verdict) = match e.kind {
                Kind::Up => (5, String::new(), String::new()),
                Kind::Dir => (0, String::new(), String::new()),
                Kind::Exe => (verdict_icon(e.verdict), human_size(e.size), e.verdict.map_or("-".to_string(), |v| v.as_str().to_string())),
            };
            lv_row(h.list, i, image, &[e.name.clone(), size, verdict]);
        }
        send_text(h.path, WM_SETTEXT, 0, &dir.display().to_string());
        app(|a| a.filling = false);
        let subtitle = dir.display().to_string();
        let view = Overview {
            title: dir.file_name().map_or("/".to_string(), |n| n.to_string_lossy().into_owned()),
            subtitle,
            pills: vec![format!("{programs} programs")],
            notes: if programs == 0 {
                vec!["No Windows programs in this folder. Open a folder that has .exe files, or pick one of the places.".to_string()]
            } else {
                vec![format!("{native} run natively, {partial} partly, {unsupported} not at all. Pick a program to see its imports and run it.")]
            },
            ..Default::default()
        };
        app(|a| a.view = view);
        fill_details();
        update_buttons();
        status(0, &format!("{programs} programs in {}", dir.display()));
        status(1, "");
        invalidate_card();
        event(format!("DIR {}", dir.file_name().map_or("/".to_string(), |n| n.to_string_lossy().into_owned())));
        if let Some(want) = select.or_else(|| app(|a| a.pending_select.take())) {
            if let Some(i) = entries.iter().position(|e| e.path == want && e.kind == Kind::Exe) {
                lv_select(h.list, i);
            }
        }
    }
}

fn invalidate_card() {
    let main = handles().main;
    with(|g| {
        let (cw, _) = g.windows.get(&main).map_or((0, 0), |w| w.client_size());
        let r = card_rect(cw);
        g.invalidate(main, Some(Rect::new(r.l - 2, r.t - 2, r.r + 2, r.b + 2)), true);
    });
}

fn go_up() {
    let up = app(|a| a.dir.parent().map(|p| p.to_path_buf()));
    if let Some(p) = up {
        let from = app(|a| a.dir.clone());
        navigate_to(p, Some(from));
    }
}

/// A row of the list was selected: look at the program.
unsafe fn select_entry(i: usize) {
    let Some(entry) = app(|a| a.entries.get(i).cloned()) else { return };
    app(|a| {
        a.selected = Some(i);
        a.report = None;
    });
    match entry.kind {
        Kind::Up | Kind::Dir => {
            // a folder shows the summary that was made when the listing was built
            let programs = app(|a| a.entries.len());
            let _ = programs;
            let view = Overview { title: entry.name.clone(), subtitle: entry.path.display().to_string(), pills: vec!["folder".to_string()], notes: vec!["Press Enter or double-click to open it.".to_string()], ..Default::default() };
            app(|a| a.view = view);
            event(format!("SELECT {} folder", entry.name));
        }
        Kind::Exe => {
            let wine = app(|a| a.wine.clone());
            match analyze_file(&entry.path) {
                Ok(report) => {
                    let verdict = report.verdict;
                    let total = report.imports.len();
                    let implemented = report.imports.iter().filter(|i| matches!(i.status, Status::Resolved | Status::Bundled)).count();
                    let mut pills = vec![report.subsystem.to_string(), report.arch.to_string(), human_size(entry.size)];
                    if report.is_dll {
                        pills.push("DLL".to_string());
                    }
                    if report.has_tls {
                        pills.push("TLS".to_string());
                    }
                    let runs_with = match backend::choose(&report, wine.is_some()) {
                        backend::Choice::Native => "Runs on the native layer; no Wine needed.".to_string(),
                        backend::Choice::NativeWithWarning(w) => format!("Native layer, with a warning: {w}."),
                        backend::Choice::Wine(w) => format!("Wine takes it: {w}."),
                        backend::Choice::Refuse(w) => format!("Cannot run: {w}."),
                    };
                    let view = Overview { title: entry.name.clone(), subtitle: entry.path.display().to_string(), verdict: Some(verdict), pills, implemented, total, notes: report.notes.clone(), runs_with, error: None };
                    app(|a| {
                        a.view = view;
                        a.report = Some(report);
                    });
                    event(format!("SELECT {} {}", entry.name, verdict.as_str()));
                }
                Err(e) => {
                    let view = Overview { title: entry.name.clone(), subtitle: entry.path.display().to_string(), error: Some(format!("Not a Windows program NLE can read: {e}")), ..Default::default() };
                    app(|a| a.view = view);
                    event(format!("SELECT {} error", entry.name));
                }
            }
        }
    }
    fill_details();
    update_buttons();
    status(1, &entry.path.display().to_string());
    invalidate_card();
}

/// Fills the imports and missing-API tabs from the report of the selected program.
unsafe fn fill_details() {
    let h = handles();
    api::send(h.imports, 0x1009, 0, 0);
    api::send(h.missing, 0x1009, 0, 0);
    let (libs, missing, bundled) = app(|a| match &a.report {
        Some(r) => {
            let libs: Vec<Vec<String>> = r
                .libs
                .iter()
                .map(|(dll, l)| {
                    let total = l.resolved + l.unresolved;
                    vec![dll.clone(), total.to_string(), l.resolved.to_string(), l.unresolved.to_string(), if l.unresolved == 0 { "ok".into() } else { "partial".into() }]
                })
                .collect();
            let missing: Vec<Vec<String>> = r.unresolved().iter().map(|u| vec![u.dll.clone(), u.name.clone(), if u.delay { "delay-load".into() } else { String::new() }]).collect();
            let bundled: Vec<String> = r.bundled.iter().filter_map(|b| b.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
            (libs, missing, bundled)
        }
        None => (Vec::new(), Vec::new(), Vec::new()),
    });
    let has_report = app(|a| a.report.is_some());
    for (i, row) in libs.iter().enumerate() {
        let image = if row[3] == "0" { 2 } else { 3 };
        lv_row(h.imports, i, image, row);
    }
    for (k, b) in bundled.iter().enumerate() {
        lv_row(h.imports, libs.len() + k, 2, &[b.clone(), String::new(), String::new(), String::new(), "bundled".to_string()]);
    }
    if has_report && missing.is_empty() {
        lv_row(h.missing, 0, 2, &["Nothing is missing".to_string(), "every import of this program is implemented".to_string(), String::new()]);
    }
    for (i, row) in missing.iter().enumerate() {
        lv_row(h.missing, i, 3, row);
    }
}

unsafe fn fill_system() {
    let h = handles();
    api::send(h.system, 0x1009, 0, 0);
    for (i, (ok, what, detail)) in system_rows().into_iter().enumerate() {
        lv_row(h.system, i, if ok { 2 } else { 3 }, &[what, detail]);
    }
}

// ---- actions --------------------------------------------------------------------------------------------------------------

fn selected_exe() -> Option<Entry> {
    app(|a| a.selected.and_then(|i| a.entries.get(i)).filter(|e| e.kind == Kind::Exe && a.report.is_some()).cloned())
}

unsafe fn action_run(strict: bool) {
    let Some(e) = selected_exe() else { return };
    let mut args = vec!["run".to_string()];
    if strict {
        args.push("--strict".to_string());
    }
    args.push(e.path.display().to_string());
    event(format!("RUN {}", e.name));
    start_job(&format!("run {}", e.name), args, false);
}

unsafe fn action_relink() {
    let Some(e) = selected_exe() else { return };
    let stem = e.path.file_stem().map_or("app".to_string(), |s| s.to_string_lossy().into_owned());
    let out = e.path.parent().unwrap_or(Path::new(".")).join(format!("{stem}-linux"));
    event(format!("RELINK {}", e.name));
    app(|a| a.pending_select = Some(e.path.clone()));
    start_job(&format!("relink {}", e.name), vec!["relink".into(), e.path.display().to_string(), "-o".into(), out.display().to_string()], true);
}

unsafe fn action_doctor() {
    event("DOCTOR".to_string());
    start_job("doctor", vec!["doctor".to_string()], false);
}

unsafe fn about() {
    let h = handles().main;
    let text = format!("NLE {}\nNative Linux Exe: runs Windows x64 programs as native Linux processes, with no emulation.\n\nMIT licensed. https://github.com/SegMind25/NLE", env!("CARGO_PKG_VERSION"));
    crate::gui::msgbox::message_box(h, &text, "About NLE", 0x40);
}

unsafe fn command(id: usize) {
    match id {
        CMD_RUN => action_run(false),
        CMD_STRICT => action_run(true),
        CMD_RELINK => action_relink(),
        CMD_STOP => stop_job(),
        CMD_DOCTOR => action_doctor(),
        CMD_REFRESH => refresh(),
        CMD_CLEAR => clear_log(),
        CMD_QUIT => {
            api::send(handles().main, WM_CLOSE, 0, 0);
        }
        CMD_ABOUT => about(),
        CMD_FOCUS_PATH => {
            api::set_focus(handles().path);
        }
        CMD_PREFIX => {
            let prefix = crate::runtime::paths::default_prefix();
            let drive = prefix.join("drive_c");
            navigate_to(if drive.is_dir() { drive } else { prefix }, None);
        }
        CMD_HOME => {
            if let Some(home) = std::env::var_os("HOME") {
                navigate_to(PathBuf::from(home), None);
            }
        }
        ID_UP => go_up(),
        _ => {}
    }
}

// ---- window procedure ---------------------------------------------------------------------------------------------------------

const HEADER_H: i32 = 60;
const BAR_TOP: i32 = 68;
const CONTENT_TOP: i32 = 110;
const MARGIN: i32 = 12;
const CARD_H: i32 = 150;

fn left_width(cw: i32) -> i32 {
    ((cw as f32 * 0.38) as i32).clamp(300, 440)
}

fn card_rect(cw: i32) -> Rect {
    let l = MARGIN + left_width(cw) + MARGIN;
    Rect::new(l, CONTENT_TOP, (cw - MARGIN).max(l + 100), CONTENT_TOP + CARD_H)
}

unsafe fn layout() {
    let h = handles();
    if h.list == 0 {
        return;
    }
    let (cw, ch) = with(|g| g.windows.get(&h.main).map_or((0, 0), |w| w.client_size()));
    let pos = |w: usize, x: i32, y: i32, cx: i32, cy: i32| {
        api::set_window_pos(w, x, y, cx.max(1), cy.max(1), 0x4 | 0x10);
    };
    // the status bar lays itself out at the bottom
    api::send(h.status, WM_SIZE, 0, 0);
    let status_h = with(|g| g.windows.get(&h.status).map_or(22, |w| w.rect.height()));
    let bottom = ch - status_h - MARGIN;
    // command bar
    let mut x = MARGIN;
    for (i, (w, width)) in [(h.buttons[0], 84), (h.buttons[1], 108), (h.buttons[2], 92), (h.buttons[3], 70), (h.buttons[4], 84)].into_iter().enumerate() {
        pos(w, x, BAR_TOP, width, 30);
        x += width + if i == 1 { 18 } else { 8 };
    }
    pos(h.buttons[5], cw - MARGIN - 90, BAR_TOP, 90, 30);
    // left column: places, path, up, list
    let lw = left_width(cw);
    pos(h.places, MARGIN, CONTENT_TOP, 96, 24);
    pos(h.path, MARGIN + 102, CONTENT_TOP, lw - 102 - 32, 24);
    pos(h.up, MARGIN + lw - 28, CONTENT_TOP, 28, 24);
    pos(h.list, MARGIN, CONTENT_TOP + 30, lw, bottom - CONTENT_TOP - 30);
    // right column: tabs under the overview card
    let rx = MARGIN + lw + MARGIN;
    let tab_top = CONTENT_TOP + CARD_H + 10;
    pos(h.tabs, rx, tab_top, cw - rx - MARGIN, bottom - tab_top);
    let mut page = [0i32, 0, cw - rx - MARGIN, bottom - tab_top];
    api::send(h.tabs, 0x1328, 0, page.as_mut_ptr() as isize);
    for w in [h.imports, h.missing, h.log, h.system] {
        pos(w, rx + page[0], tab_top + page[1], page[2] - page[0], page[3] - page[1]);
    }
    with(|g| g.invalidate(h.main, None, true));
}

unsafe fn paint_main() {
    let h = handles().main;
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    let (view, wine, log_events) = app(|a| (a.view.clone(), a.wine.clone(), a.log_events));
    let _ = log_events;
    let running = app(|a| a.job.as_ref().map(|j| j.label.clone()));
    let display = if std::env::var("NLE_GUI").is_ok_and(|v| v.eq_ignore_ascii_case("headless")) {
        "display: headless".to_string()
    } else {
        match std::env::var("DISPLAY") {
            Ok(d) if !d.is_empty() => format!("display: X11 {d}"),
            _ => "display: none".to_string(),
        }
    };
    let wine_text = match &wine {
        Some(w) => format!("Wine: {}", w.file_name().map_or("found".to_string(), |n| n.to_string_lossy().into_owned())),
        None => "Wine: not installed".to_string(),
    };
    with(|g| {
        let (cw, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
        let big = Sized::new(Family::Sans, true, false, 22.0);
        let title_font = Sized::new(Family::Sans, true, false, 17.0);
        let body = Sized::new(Family::Sans, false, false, 12.0);
        let bold = Sized::new(Family::Sans, true, false, 12.0);
        let small = Sized::new(Family::Sans, false, false, 11.0);
        let card = card_rect(cw);
        g.draw(hdc, |cv, _| {
            let none = Pen { style: PenStyle::Null, width: 1, color: 0 };
            cv.fill_rect(Rect::new(0, 0, cw, ch), PAGE);
            // header: a dark band with a gradient and the logo
            for y in 0..HEADER_H {
                let t = y as f32 / HEADER_H as f32;
                let mix = |a: u32, b: u32| (a as f32 + (b as f32 - a as f32) * t) as u32;
                let (r, gr, b) = (mix(0x0A, 0x14), mix(0x10, 0x2A), mix(0x20, 0x4A));
                cv.fill_rect(Rect::new(0, y, cw, y + 1), r << 16 | gr << 8 | b);
            }
            cv.fill_rect(Rect::new(0, HEADER_H, cw, HEADER_H + 1), 0x0EA5E9);
            // the brand mark: an N and a block cursor on a dark tile
            cv.round_rect(Rect::new(16, 12, 52, 48), 10, 10, &none, &Brush::Solid(0x35497F));
            cv.round_rect(Rect::new(17, 13, 51, 47), 9, 9, &none, &Brush::Solid(0x101B36));
            let nw = big.text_width("N");
            big.draw(cv, 30 - nw / 2, 14, "N", 0x38BDF8, None);
            cv.fill_rect(Rect::new(41, 31, 46, 37), 0x34D399);
            title_font.draw(cv, 64, 11, "NLE", 0xFFFFFF, None);
            small.draw(cv, 64, 33, "Native Linux Exe: Windows programs, run natively", 0x9DB4D0, None);
            // status pills on the right
            let mut right = cw - 16;
            let busy = running.as_ref().map(|r| format!("running: {r}"));
            let pills: Vec<(String, u32)> = [Some((wine_text.clone(), if wine.is_some() { 0x2D6A4F } else { 0x3B4658 })), Some((display.clone(), 0x2B4C7E)), busy.map(|b| (b, 0x8A5A00))].into_iter().flatten().collect();
            for (text, color) in pills.iter().rev() {
                let w = small.text_width(text) + 22;
                cv.round_rect(Rect::new(right - w, 20, right, 42), 22, 22, &none, &Brush::Solid(*color));
                small.draw(cv, right - w + 11, 24, text, 0xE8EEF7, None);
                right -= w + 8;
            }
            // the overview card
            cv.round_rect(Rect::new(card.l - 1, card.t - 1, card.r + 1, card.b + 1), 10, 10, &none, &Brush::Solid(0xD1D5DB));
            cv.round_rect(card, 10, 10, &none, &Brush::Solid(0xFFFFFF));
            let inner = card.r - card.l - 36;
            let (x0, mut y) = (card.l + 18, card.t + 14);
            title_font.draw(cv, x0, y, &fit(&title_font, &view.title, inner), INK, None);
            y += 24;
            body.draw(cv, x0, y, &fit(&body, &view.subtitle, inner), MUTED, None);
            y += 24;
            if let Some(err) = &view.error {
                for line in wrap(&body, err, inner, 3) {
                    body.draw(cv, x0, y, &line, RED, None);
                    y += 18;
                }
                return;
            }
            let mut x = x0;
            if let Some(v) = view.verdict {
                let label = v.as_str().to_uppercase();
                let w = bold.text_width(&label) + 22;
                cv.round_rect(Rect::new(x, y, x + w, y + 22), 22, 22, &none, &Brush::Solid(verdict_color(v)));
                bold.draw(cv, x + 11, y + 3, &label, 0xFFFFFF, None);
                x += w + 8;
            }
            for p in &view.pills {
                let w = small.text_width(p) + 18;
                if x + w > card.r - 14 {
                    break;
                }
                cv.round_rect(Rect::new(x, y, x + w, y + 22), 22, 22, &none, &Brush::Solid(0xE5E7EB));
                small.draw(cv, x + 9, y + 4, p, 0x374151, None);
                x += w + 6;
            }
            y += 32;
            if view.total > 0 {
                let frac = view.implemented as f32 / view.total as f32;
                let bar_w = (inner - 190).clamp(60, 260);
                cv.round_rect(Rect::new(x0, y + 3, x0 + bar_w, y + 13), 10, 10, &none, &Brush::Solid(0xE5E7EB));
                if view.implemented > 0 {
                    cv.round_rect(Rect::new(x0, y + 3, x0 + (bar_w as f32 * frac).max(10.0) as i32, y + 13), 10, 10, &none, &Brush::Solid(view.verdict.map_or(GREEN, verdict_color)));
                }
                body.draw(cv, x0 + bar_w + 10, y, &format!("{} of {} imports implemented", view.implemented, view.total), INK, None);
                y += 22;
            }
            if !view.runs_with.is_empty() {
                for line in wrap(&body, &view.runs_with, inner, 2) {
                    body.draw(cv, x0, y, &line, 0x374151, None);
                    y += 17;
                }
            }
            for n in &view.notes {
                if y + 16 > card.b - 6 {
                    break;
                }
                for line in wrap(&small, n, inner, 2) {
                    if y + 15 > card.b - 4 {
                        break;
                    }
                    small.draw(cv, x0, y, &line, MUTED, None);
                    y += 15;
                }
            }
        });
    });
    api::end_paint(h, ps.as_ptr());
}

/// Owner-drawn push buttons: a green primary button, white secondary ones.
unsafe fn draw_button(lp: isize) {
    let p = lp as *const u8;
    let id = *(p.add(4) as *const u32) as usize;
    let state = *(p.add(16) as *const u32);
    let hdc = *(p.add(32) as *const u64) as usize;
    let rc = Rect::new(*(p.add(40) as *const i32), *(p.add(44) as *const i32), *(p.add(48) as *const i32), *(p.add(52) as *const i32));
    let (label, kind) = match id {
        CMD_RUN => ("Run", 1),
        CMD_STRICT => ("Run --strict", 0),
        CMD_RELINK => ("Relink", 0),
        CMD_STOP => ("Stop", 2),
        CMD_DOCTOR => ("Doctor", 0),
        CMD_REFRESH => ("Refresh", 0),
        ID_UP => ("", 3),
        _ => ("", 0),
    };
    let disabled = state & 4 != 0;
    let pressed = state & 1 != 0;
    with(|g| {
        let bold = Sized::new(Family::Sans, true, false, 12.0);
        g.draw(hdc, |cv, _| {
            let none = Pen { style: PenStyle::Null, width: 1, color: 0 };
            let (fill, border, ink) = if disabled {
                (0xECEEF1, 0xD5D9DF, 0x9CA3AF)
            } else if kind == 1 {
                (if pressed { 0x0F7A39 } else { GREEN }, 0x12873F, 0xFFFFFF)
            } else if kind == 2 {
                (if pressed { 0xFEE2E2 } else { 0xFFFFFF }, 0xE5A3A3, RED)
            } else {
                (if pressed { 0xE5E7EB } else { 0xFFFFFF }, 0xC7CCD4, INK)
            };
            cv.fill_rect(rc, PAGE);
            cv.round_rect(rc, 8, 8, &none, &Brush::Solid(border));
            cv.round_rect(Rect::new(rc.l + 1, rc.t + 1, rc.r - 1, rc.b - 1), 7, 7, &none, &Brush::Solid(fill));
            let tw = bold.text_width(label);
            let (cx, cy) = ((rc.l + rc.r) / 2, (rc.t + rc.b) / 2);
            let icon_w = if kind == 1 || kind == 2 { 14 } else { 0 };
            let tx = cx - (tw + icon_w) / 2 + icon_w;
            bold.draw(cv, tx, cy - bold.height() / 2, label, ink, None);
            if kind == 1 {
                cv.polygon(&[(tx - 14, cy - 5), (tx - 14, cy + 5), (tx - 5, cy)], &none, &Brush::Solid(ink), false);
            } else if kind == 2 {
                cv.fill_rect(Rect::new(tx - 14, cy - 4, tx - 6, cy + 4), ink);
            } else if kind == 3 {
                cv.polygon(&[(cx, cy - 7), (cx + 6, cy), (cx + 2, cy), (cx + 2, cy + 6), (cx - 2, cy + 6), (cx - 2, cy), (cx - 6, cy)], &none, &Brush::Solid(ink), false);
            }
        });
    });
}

unsafe fn notify(lp: isize) -> isize {
    let nm = lp as *const u8;
    let from = *(nm as *const u64) as usize;
    let code = *(nm.add(16) as *const i32);
    let h = handles();
    if from == h.tabs && code == -551 {
        let sel = api::send(h.tabs, 0x130B, 0, 0);
        show_page(sel.max(0) as usize);
    } else if from == h.list {
        let filling = app(|a| a.filling);
        match code {
            // LVN_ITEMCHANGED: iItem@24, uNewState@32, uOldState@36, uChanged@40
            -101 if !filling => {
                let (item, new, old, changed) = (*(nm.add(24) as *const i32), *(nm.add(32) as *const u32), *(nm.add(36) as *const u32), *(nm.add(40) as *const u32));
                if changed & 8 != 0 && new & 2 != 0 && old & 2 == 0 && item >= 0 {
                    select_entry(item as usize);
                }
            }
            // LVN_ITEMACTIVATE: open a folder, run a program
            -114 => {
                let item = *(nm.add(24) as *const i32);
                if let Some(e) = app(|a| a.entries.get(item.max(0) as usize).cloned()) {
                    match e.kind {
                        Kind::Up => go_up(),
                        Kind::Dir => navigate_to(e.path, None),
                        Kind::Exe => {
                            select_entry(item as usize);
                            action_run(false);
                        }
                    }
                }
            }
            // LVN_KEYDOWN: Backspace goes up a folder
            -155 => {
                if *(nm.add(24) as *const u16) == 0x08 {
                    go_up();
                }
            }
            _ => {}
        }
    }
    0
}

unsafe extern "win64" fn launcher_proc(h: usize, m: u32, wp: usize, lp: isize) -> isize {
    match m {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint_main();
            0
        }
        WM_SIZE => {
            layout();
            0
        }
        WM_DRAWITEM => {
            draw_button(lp);
            1
        }
        WM_NOTIFY => notify(lp),
        WM_TIMER => {
            if wp == TIMER_POLL {
                poll_job();
            }
            0
        }
        WM_COMMAND => {
            let (id, code) = (loword(wp) as usize, hiword(wp));
            let from = lp as usize;
            if id == ID_PLACES && code == 1 {
                // CBN_SELCHANGE
                let sel = api::send(from, 0x147, 0, 0);
                if sel >= 1 {
                    if let Some((_, dir)) = app(|a| a.places.get(sel as usize - 1).cloned()) {
                        navigate_to(dir, None);
                    }
                    api::send(from, 0x14E, 0, 0);
                }
            } else if id != ID_PLACES && id != ID_PATH && id != ID_LIST {
                command(id);
            }
            0
        }
        WM_DESTROY => {
            stop_job();
            let tid = crate::runtime::handles::current_tid();
            with(|g| {
                g.thread_state(tid).quit = Some(0);
                g.wake(tid);
            });
            0
        }
        _ => msg::def_window_proc(h, m, wp, lp, false),
    }
}

// ---- startup --------------------------------------------------------------------------------------------------------------------

unsafe fn build_menu(main: usize) -> usize {
    with(|g| {
        let bar = g.menu_create(false);
        let popup = |g: &mut Gui, title: &str, items: &[(&str, usize)]| {
            let m = g.menu_create(true);
            for (text, id) in items {
                if text.is_empty() {
                    g.menu_insert(m, None, 0x800, 0, String::new(), 0);
                } else {
                    g.menu_insert(m, None, 0, *id, text.to_string(), 0);
                }
            }
            g.menu_insert(bar, None, 0x10, m, title.to_string(), 0);
        };
        popup(g, "&File", &[("&Open path\tCtrl+L", CMD_FOCUS_PATH), ("&Home folder", CMD_HOME), ("Windows &prefix (C:)", CMD_PREFIX), ("&Refresh\tF5", CMD_REFRESH), ("", 0), ("&Quit\tCtrl+Q", CMD_QUIT)]);
        popup(g, "&Program", &[("&Run\tCtrl+R", CMD_RUN), ("Run with --&strict\tCtrl+Shift+R", CMD_STRICT), ("Re&link to a Linux ELF\tCtrl+E", CMD_RELINK), ("", 0), ("S&top\tCtrl+.", CMD_STOP)]);
        popup(g, "&Tools", &[("&Doctor\tF9", CMD_DOCTOR), ("&Clear output", CMD_CLEAR)]);
        popup(g, "&Help", &[("&About NLE", CMD_ABOUT)]);
        let accel = g.accel_create(vec![
            Accel { flags: FVIRTKEY | FCONTROL, key: b'L' as u16, cmd: CMD_FOCUS_PATH as u16 },
            Accel { flags: FVIRTKEY, key: 0x74, cmd: CMD_REFRESH as u16 },
            Accel { flags: FVIRTKEY | FCONTROL, key: b'R' as u16, cmd: CMD_RUN as u16 },
            Accel { flags: FVIRTKEY | FCONTROL | FSHIFT, key: b'R' as u16, cmd: CMD_STRICT as u16 },
            Accel { flags: FVIRTKEY | FCONTROL, key: b'E' as u16, cmd: CMD_RELINK as u16 },
            Accel { flags: FVIRTKEY | FCONTROL, key: 0xBE, cmd: CMD_STOP as u16 },
            Accel { flags: FVIRTKEY, key: 0x78, cmd: CMD_DOCTOR as u16 },
            Accel { flags: FVIRTKEY | FCONTROL, key: b'Q' as u16, cmd: CMD_QUIT as u16 },
        ]);
        app(|a| a.accel = accel);
        let _ = main;
        bar
    })
}

fn places() -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        v.push(("Home".to_string(), home.clone()));
        for (name, sub) in [("Downloads", "Downloads"), ("Desktop", "Desktop")] {
            let p = home.join(sub);
            if p.is_dir() {
                v.push((name.to_string(), p));
            }
        }
    }
    let prefix = crate::runtime::paths::default_prefix();
    let drive = prefix.join("drive_c");
    if drive.is_dir() {
        v.push(("C: drive".to_string(), drive));
    }
    if let Ok(cwd) = std::env::current_dir() {
        v.push(("Current".to_string(), cwd));
    }
    v
}

pub fn main(args: &[String]) -> ExitCode {
    let mut log_events = false;
    let mut start: Option<PathBuf> = None;
    for a in args {
        match a.as_str() {
            "--log" => log_events = true,
            "-h" | "--help" => {
                println!("usage: nle gui [--log] [FOLDER|PROGRAM.exe]\n\nopens NLE's window: browse for Windows programs, inspect, run, relink.\n--log prints what happens (SELECT, RUN, OUT, EXIT) to stdout.");
                return ExitCode::SUCCESS;
            }
            p if !p.starts_with('-') => start = Some(PathBuf::from(p)),
            other => {
                eprintln!("nle gui: unknown option `{other}`");
                return ExitCode::from(2);
            }
        }
    }
    if with(|g| g.ensure_backend().is_none()) {
        eprintln!("nle: the launcher needs a window: run it where DISPLAY is set, or use NLE_GUI=headless");
        return ExitCode::from(70);
    }
    let (dir, select) = match start {
        Some(p) if p.is_file() => (p.parent().map(|d| if d.as_os_str().is_empty() { PathBuf::from(".") } else { d.to_path_buf() }).unwrap_or_else(|| PathBuf::from(".")), Some(p)),
        Some(p) => (p, None),
        None => (std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")), None),
    };
    let select = select.and_then(|p| std::fs::canonicalize(p).ok());
    unsafe { run(dir, select, log_events) }
}

unsafe fn run(dir: PathBuf, select: Option<PathBuf>, log_events: bool) -> ExitCode {
    {
        let mut g = APP.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(App {
            h: Handles::default(),
            dir: dir.clone(),
            entries: Vec::new(),
            selected: None,
            report: None,
            view: Overview::default(),
            job: None,
            log_events,
            accel: 0,
            wine: backend::find_wine(),
            places: places(),
            icons: 0,
            mono: 0,
            filling: false,
            pending_select: select,
        });
    }
    // class, icons and fonts
    let (font, mono, icons) = with(|g| {
        g.register_class(Class { name: "NleLauncher".into(), atom: 0, style: 0, wndproc: launcher_proc as *const () as usize, cls_extra: 0, wnd_extra: 0, hinstance: 0, icon: 0, cursor: 0, background: 0, menu_name: None, unicode: true, native: true });
        let font = g.stock_object(17);
        let mono = g.add_object(Object::Font(LogFont { height: -12, width: 0, weight: 400, italic: false, underline: false, strike: false, pitch_family: 0x31, face: "Courier New".into() }));
        (font, mono, make_icons(g))
    });
    app(|a| {
        a.mono = mono;
        a.icons = icons;
    });
    let style = WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
    let (client_w, client_h) = (1060, 690);
    let inset = gui::wm::insets(style, 0, true, true);
    let (sw, sh) = with(|g| g.screen_size());
    let (ow, oh) = ((client_w + inset.l + inset.r).min(sw), (client_h + inset.t + inset.b).min(sh));
    let main = api::create_window_ex(CreateArgs { ex_style: 0, class: ClassRef::Name("NleLauncher".into()), title: "NLE".to_string(), style, x: ((sw - ow) / 2).max(0), y: ((sh - oh) / 2).max(0), w: ow, h: oh, parent: 0, menu: 0, hinstance: 0, param: 0, ansi: false });
    if main == 0 {
        eprintln!("nle: cannot create the launcher window");
        return ExitCode::from(70);
    }
    let bar = build_menu(main);
    api::set_menu(main, bar);
    let mut h = Handles { main, ..Default::default() };
    let tabstop = WS_VISIBLE | WS_TABSTOP;
    h.places = child("COMBOBOX", "", tabstop | WS_VSCROLL | 3, 0, ID_PLACES, main, font);
    h.path = child("EDIT", "", tabstop | 0x80, WS_EX_CLIENTEDGE, ID_PATH, main, font);
    h.up = child("BUTTON", "", tabstop | 0xB, 0, ID_UP, main, 0);
    h.list = child("SysListView32", "", tabstop | 1 | 4 | 8, WS_EX_CLIENTEDGE, ID_LIST, main, font);
    h.tabs = child("SysTabControl32", "", tabstop | WS_CLIPSIBLINGS, 0, ID_TABS, main, font);
    h.imports = child("SysListView32", "", WS_VISIBLE | 1 | 4 | 8, WS_EX_CLIENTEDGE, ID_IMPORTS, main, font);
    h.missing = child("SysListView32", "", 1 | 4 | 8, WS_EX_CLIENTEDGE, ID_MISSING, main, font);
    h.log = child("LISTBOX", "", WS_VSCROLL | WS_HSCROLL | 0x100 | 0x40, WS_EX_CLIENTEDGE, ID_LOG, main, mono);
    h.system = child("SysListView32", "", 1 | 4 | 8, WS_EX_CLIENTEDGE, ID_SYSTEM, main, font);
    h.status = child("msctls_statusbar32", "", WS_VISIBLE | 0x100, 0, ID_STATUS, main, font);
    for (i, id) in [CMD_RUN, CMD_STRICT, CMD_RELINK, CMD_STOP, CMD_DOCTOR, CMD_REFRESH].into_iter().enumerate() {
        h.buttons[i] = child("BUTTON", "", tabstop | 0xB, 0, id, main, 0);
    }
    app(|a| a.h = h);
    // content of the controls
    for w in [h.list, h.imports, h.missing, h.system] {
        api::send(w, 0x1036, 0x20, 0x20);
        api::send(w, 0x1003, 1, icons as isize);
    }
    api::send(h.list, 0x1001, 0, 0xFFFFFF);
    lv_column(h.list, 0, "Name", 190, false);
    lv_column(h.list, 1, "Size", 70, true);
    lv_column(h.list, 2, "Verdict", 80, false);
    lv_column(h.imports, 0, "Library", 230, false);
    lv_column(h.imports, 1, "Functions", 80, true);
    lv_column(h.imports, 2, "Implemented", 90, true);
    lv_column(h.imports, 3, "Missing", 70, true);
    lv_column(h.imports, 4, "Status", 80, false);
    lv_column(h.missing, 0, "Library", 200, false);
    lv_column(h.missing, 1, "Function", 260, false);
    lv_column(h.missing, 2, "Note", 90, false);
    lv_column(h.system, 0, "Check", 190, false);
    lv_column(h.system, 1, "Detail", 520, false);
    for (i, name) in ["Imports", "Not implemented", "Output", "System"].into_iter().enumerate() {
        let w = wide(name);
        let item = TcItem { mask: 1, state: 0, state_mask: 0, text: w.as_ptr(), cch: 0, image: 0, param: 0 };
        api::send(h.tabs, 0x133E, i, &item as *const TcItem as isize);
    }
    send_text(h.places, 0x143, 0, "Go to...");
    for (name, _) in app(|a| a.places.clone()) {
        send_text(h.places, 0x143, 0, &name);
    }
    api::send(h.places, 0x14E, 0, 0);
    let parts = [380i32, 900, -1];
    api::send(h.status, 0x404, 3, parts.as_ptr() as isize);
    status(2, "NLE");
    fill_system();
    select_tab(0);
    log_line("NLE launcher: pick a program on the left; Run starts it, Relink writes a native Linux ELF.", false);
    api::show_window(main, SW_SHOWNORMAL);
    layout();
    navigate_to(dir, None);
    update_buttons();
    api::set_focus(h.list);

    let accel = app(|a| a.accel);
    loop {
        match msg::next_message(&Filter { hwnd: 0, min: 0, max: 0 }, true, true) {
            Next::Msg(m) => {
                // Enter in the path box goes to that folder
                if m.message == WM_KEYDOWN && m.wparam == 0x0D && m.hwnd == h.path {
                    let text = with(|g| g.windows.get(&h.path).map(|w| w.text.clone())).unwrap_or_default();
                    let p = PathBuf::from(text.trim());
                    if p.is_dir() {
                        navigate_to(p, None);
                    } else {
                        status(0, &format!("{}: not a folder", text.trim()));
                    }
                    continue;
                }
                if (m.message == WM_KEYDOWN || m.message == WM_SYSKEYDOWN) && with(|g| g.is_descendant(m.hwnd, main) || m.hwnd == main) {
                    if m.message == WM_SYSKEYDOWN && m.wparam == 0x26 {
                        // Alt+Up
                        go_up();
                        continue;
                    }
                    if let Some(cmd) = with(|g| g.accel_lookup(accel, m.message, m.wparam, m.lparam)) {
                        api::send(main, WM_COMMAND, cmd as usize | (1 << 16), 0);
                        continue;
                    }
                }
                msg::translate(&m);
                msg::dispatch(&m);
            }
            Next::Quit(_) => break,
            Next::None => {}
        }
    }
    stop_job();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_colours_are_removed_from_output() {
        assert_eq!(strip_ansi("\u{1b}[32mok\u{1b}[0m done"), "ok done");
        assert_eq!(strip_ansi("a\tb\r"), "a b");
    }

    #[test]
    fn sizes_are_human_sized() {
        assert_eq!(human_size(10), "10 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(3 * 1024 * 1024 + 512 * 1024), "3.5 MB");
    }

    #[test]
    fn long_text_is_cut_to_fit() {
        let f = Sized::new(Family::Sans, false, false, 12.0);
        let s = fit(&f, "a-very-long-file-name-that-does-not-fit.exe", 80);
        assert!(s.ends_with("...") && f.text_width(&s) <= 80, "{s}");
        assert_eq!(fit(&f, "short", 200), "short");
        let lines = wrap(&f, "one two three four five six seven eight nine ten eleven twelve", 90, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| f.text_width(l) <= 90));
    }

    #[test]
    fn listing_puts_folders_before_programs_and_skips_other_files() {
        let dir = std::env::temp_dir().join(format!("nle-launcher-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Zeta")).unwrap();
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("b.EXE"), b"MZ").unwrap();
        std::fs::write(dir.join("a.exe"), b"MZ").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let names: Vec<_> = list_dir(&dir).iter().map(|e| (e.name.clone(), e.kind)).collect();
        assert_eq!(names, vec![("..".to_string(), Kind::Up), ("alpha".to_string(), Kind::Dir), ("Zeta".to_string(), Kind::Dir), ("a.exe".to_string(), Kind::Exe), ("b.EXE".to_string(), Kind::Exe)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_a_program_is_an_error_not_a_crash() {
        let p = std::env::temp_dir().join(format!("nle-launcher-bad-{}.exe", std::process::id()));
        std::fs::write(&p, b"this is not a PE file").unwrap();
        assert!(analyze_file(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
