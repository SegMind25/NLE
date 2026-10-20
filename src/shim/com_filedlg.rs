//! The Vista file dialogs as COM objects: `IFileOpenDialog` / `IFileSaveDialog` (what `CoCreateInstance` hands out for
//! their classes), the `IShellItem` and `IShellItemArray` they return, and `IFileDialogCustomize`. The dialog itself is
//! the one `GetOpenFileName` shows (see `gui::filedlg`); these objects hold what the program set, run it and give the
//! answer back. Objects are plain structures that begin with a pointer to a table of `extern "win64"` functions, as COM
//! objects do, and count their references.

use crate::gui::filedlg::{self, Request};
use crate::runtime::paths;
use crate::runtime::strings::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

type Fp = *const ();
const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
const E_POINTER: i32 = 0x8000_4003u32 as i32;
const E_FAIL: i32 = 0x8000_4005u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000Eu32 as i32;
const HRESULT_CANCELLED: i32 = 0x8007_04C7u32 as i32;

#[repr(transparent)]
pub struct Vtbl<const N: usize>([Fp; N]);
unsafe impl<const N: usize> Sync for Vtbl<N> {}

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

pub const CLSID_FILE_OPEN_DIALOG: [u8; 16] = guid(0xDC1C5A9C, 0xE88A, 0x4DDE, [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7]);
pub const CLSID_FILE_SAVE_DIALOG: [u8; 16] = guid(0xC0B4E2F3, 0xBA21, 0x4773, [0x8D, 0xBA, 0x33, 0x5E, 0xC9, 0x46, 0xEB, 0x8B]);
const IID_IUNKNOWN: [u8; 16] = guid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IFILE_DIALOG: [u8; 16] = guid(0x42F85136, 0xDB7E, 0x439C, [0x85, 0xF1, 0xE4, 0x07, 0x5D, 0x13, 0x5F, 0xC8]);
const IID_IFILE_OPEN_DIALOG: [u8; 16] = guid(0xD57C7288, 0xD4AD, 0x4768, [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60]);
const IID_IFILE_SAVE_DIALOG: [u8; 16] = guid(0x84BCCD23, 0x5FDE, 0x4CDB, [0xAE, 0xA4, 0xAF, 0x64, 0xB8, 0x3D, 0x78, 0xAB]);
const IID_IMODAL_WINDOW: [u8; 16] = guid(0xB4DB1657, 0x70D7, 0x485E, [0x8E, 0x3E, 0x6F, 0xCB, 0x5A, 0x5C, 0x18, 0x02]);
const IID_ISHELL_ITEM: [u8; 16] = guid(0x43826D1E, 0xE718, 0x42EE, [0xBC, 0x55, 0xA1, 0xE2, 0x61, 0xC3, 0x7B, 0xFE]);
const IID_ISHELL_ITEM_ARRAY: [u8; 16] = guid(0xB63EA76D, 0x1F85, 0x456F, [0xA1, 0x9C, 0x48, 0x15, 0x9E, 0xFA, 0x85, 0x8B]);
const IID_IFILE_DIALOG_CUSTOMIZE: [u8; 16] = guid(0xE6FDD21A, 0x163F, 0x4975, [0x9C, 0x8C, 0xA6, 0x9F, 0x1B, 0xA3, 0x70, 0x34]);

unsafe fn iid_is(p: usize, want: &[[u8; 16]]) -> bool {
    p != 0 && {
        let got = std::slice::from_raw_parts(p as *const u8, 16);
        want.iter().any(|w| w == got)
    }
}

unsafe extern "win64" fn not_implemented() -> i32 {
    E_NOTIMPL
}

unsafe extern "win64" fn ok() -> i32 {
    S_OK
}

/// A string the caller frees with `CoTaskMemFree`.
unsafe fn task_string(s: &str) -> usize {
    let w = to_wide_nul(s);
    let p = libc::malloc(w.len() * 2) as *mut u16;
    if !p.is_null() {
        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
    }
    p as usize
}

macro_rules! unknown_methods {
    ($ty:ty, $iids:expr) => {
        unsafe extern "win64" fn query_interface(this: *mut $ty, riid: usize, out: *mut usize) -> i32 {
            if out.is_null() {
                return E_POINTER;
            }
            *out = 0;
            if iid_is(riid, &$iids) {
                *out = this as usize;
                add_ref(this);
                return S_OK;
            }
            query_other(this, riid, out)
        }
        unsafe extern "win64" fn add_ref(this: *mut $ty) -> u32 {
            (*this).refs.fetch_add(1, Ordering::SeqCst) + 1
        }
        unsafe extern "win64" fn release(this: *mut $ty) -> u32 {
            let left = (*this).refs.fetch_sub(1, Ordering::SeqCst) - 1;
            if left == 0 {
                destroy(this);
            }
            left
        }
    };
}

// ---- IShellItem ---------------------------------------------------------------------------------------------

#[repr(C)]
pub struct ShellItem {
    vtbl: *const Vtbl<8>,
    refs: AtomicU32,
    path: PathBuf,
}

mod shell_item {
    use super::*;

    unknown_methods!(ShellItem, [IID_IUNKNOWN, IID_ISHELL_ITEM]);

    unsafe fn query_other(_this: *mut ShellItem, _riid: usize, _out: *mut usize) -> i32 {
        E_NOINTERFACE
    }

    unsafe fn destroy(this: *mut ShellItem) {
        drop(Box::from_raw(this));
    }

    // SIGDN_NORMALDISPLAY 0, PARENTRELATIVEPARSING 0x80018001, DESKTOPABSOLUTEPARSING 0x80028000, PARENTRELATIVEEDITING
    // 0x80031001, DESKTOPABSOLUTEEDITING 0x8004c000, FILESYSPATH 0x80058000, URL 0x80068000, PARENTRELATIVE 0x80080001
    unsafe extern "win64" fn get_display_name(this: *mut ShellItem, sigdn: u32, out: *mut usize) -> i32 {
        if out.is_null() {
            return E_POINTER;
        }
        let path = &(*this).path;
        let full = paths::host_to_win(path);
        let name = path.file_name().map_or(full.clone(), |n| n.to_string_lossy().into_owned());
        let text = match sigdn {
            0 | 0x8001_8001 | 0x8003_1001 | 0x8008_0001 => name,
            0x8006_8000 => format!("file:///{}", full.replace('\\', "/")),
            _ => full,
        };
        *out = task_string(&text);
        if *out == 0 { E_OUTOFMEMORY } else { S_OK }
    }

    unsafe extern "win64" fn get_parent(this: *mut ShellItem, out: *mut usize) -> i32 {
        match (&(*this).path).parent() {
            Some(p) if !out.is_null() => {
                *out = new_item(p.to_path_buf());
                S_OK
            }
            _ => E_FAIL,
        }
    }

    // SFGAO_FOLDER 0x20000000, SFGAO_FILESYSTEM 0x40000000, SFGAO_STREAM 0x400000
    unsafe extern "win64" fn get_attributes(this: *mut ShellItem, mask: u32, out: *mut u32) -> i32 {
        if out.is_null() {
            return E_POINTER;
        }
        let mut attrs = 0x4000_0000u32;
        attrs |= if (&(*this).path).is_dir() { 0x2000_0000 } else { 0x0040_0000 };
        *out = attrs & mask;
        if attrs & mask == mask { S_OK } else { S_FALSE }
    }

    unsafe extern "win64" fn compare(this: *mut ShellItem, other: usize, _hint: u32, order: *mut i32) -> i32 {
        if other == 0 || order.is_null() {
            return E_INVALIDARG;
        }
        let theirs = item_path(other).unwrap_or_default();
        *order = match (&(*this).path).cmp(&theirs) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        };
        if *order == 0 { S_OK } else { S_FALSE }
    }

    pub static VTBL: Vtbl<8> = Vtbl([query_interface as Fp, add_ref as Fp, release as Fp, not_implemented as Fp, get_parent as Fp, get_display_name as Fp, get_attributes as Fp, compare as Fp]);
}

/// A new shell item for `path`, with one reference (the caller's).
pub unsafe fn new_item(path: PathBuf) -> usize {
    Box::into_raw(Box::new(ShellItem { vtbl: &shell_item::VTBL, refs: AtomicU32::new(1), path })) as usize
}

/// The path of any `IShellItem`, ours or the program's, by asking it for its file system path.
pub unsafe fn item_path(item: usize) -> Option<PathBuf> {
    if item == 0 {
        return None;
    }
    let vt = *(item as *const *const usize);
    type GetDisplayName = unsafe extern "win64" fn(usize, u32, *mut usize) -> i32;
    let f: GetDisplayName = std::mem::transmute(*vt.add(5));
    let mut name = 0usize;
    if f(item, 0x8005_8000, &mut name) < 0 || name == 0 {
        return None;
    }
    let text = w_ptr_to_string(name as *const u16);
    libc::free(name as *mut libc::c_void);
    Some(paths::resolve(&text))
}

/// `SHCreateItemFromParsingName`: an item for a path, as the interface asked for.
pub unsafe fn item_from_parsing_name(path: &str, riid: usize, out: *mut usize) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    *out = 0;
    let item = new_item(paths::resolve(path));
    let vt = *(item as *const *const usize);
    type Qi = unsafe extern "win64" fn(usize, usize, *mut usize) -> i32;
    let qi: Qi = std::mem::transmute(*vt);
    let hr = qi(item, riid, out);
    type Rel = unsafe extern "win64" fn(usize) -> u32;
    let rel: Rel = std::mem::transmute(*vt.add(2));
    rel(item);
    hr
}

// ---- IShellItemArray ----------------------------------------------------------------------------------------

#[repr(C)]
pub struct ShellItemArray {
    vtbl: *const Vtbl<10>,
    refs: AtomicU32,
    paths: Vec<PathBuf>,
}

mod item_array {
    use super::*;

    unknown_methods!(ShellItemArray, [IID_IUNKNOWN, IID_ISHELL_ITEM_ARRAY]);

    unsafe fn query_other(_this: *mut ShellItemArray, _riid: usize, _out: *mut usize) -> i32 {
        E_NOINTERFACE
    }

    unsafe fn destroy(this: *mut ShellItemArray) {
        drop(Box::from_raw(this));
    }

    unsafe extern "win64" fn get_count(this: *mut ShellItemArray, out: *mut u32) -> i32 {
        if out.is_null() {
            return E_POINTER;
        }
        *out = (&(*this).paths).len() as u32;
        S_OK
    }

    unsafe extern "win64" fn get_item_at(this: *mut ShellItemArray, index: u32, out: *mut usize) -> i32 {
        match (&(*this).paths).get(index as usize) {
            Some(p) if !out.is_null() => {
                *out = new_item(p.clone());
                S_OK
            }
            _ => E_INVALIDARG,
        }
    }

    pub static VTBL: Vtbl<10> = Vtbl([query_interface as Fp, add_ref as Fp, release as Fp, not_implemented as Fp, not_implemented as Fp, not_implemented as Fp, not_implemented as Fp, get_count as Fp, get_item_at as Fp, not_implemented as Fp]);
}

unsafe fn new_array(paths: Vec<PathBuf>) -> usize {
    Box::into_raw(Box::new(ShellItemArray { vtbl: &item_array::VTBL, refs: AtomicU32::new(1), paths })) as usize
}

// ---- IFileDialogCustomize ------------------------------------------------------------------------------------

#[repr(C)]
pub struct Customize {
    vtbl: *const Vtbl<30>,
    refs: AtomicU32,
    /// (control id, state) of the check buttons the program added
    checks: Vec<(u32, bool)>,
}

mod customize {
    use super::*;

    unknown_methods!(Customize, [IID_IUNKNOWN, IID_IFILE_DIALOG_CUSTOMIZE]);

    unsafe fn query_other(_this: *mut Customize, _riid: usize, _out: *mut usize) -> i32 {
        E_NOINTERFACE
    }

    unsafe fn destroy(this: *mut Customize) {
        drop(Box::from_raw(this));
    }

    unsafe extern "win64" fn add_check_button(this: *mut Customize, id: u32, _label: usize, checked: i32) -> i32 {
        (&mut (*this).checks).push((id, checked != 0));
        S_OK
    }

    unsafe extern "win64" fn get_check_button_state(this: *mut Customize, id: u32, out: *mut i32) -> i32 {
        match (&(*this).checks).iter().find(|c| c.0 == id) {
            Some(c) if !out.is_null() => {
                *out = c.1 as i32;
                S_OK
            }
            _ => E_INVALIDARG,
        }
    }

    unsafe extern "win64" fn set_check_button_state(this: *mut Customize, id: u32, checked: i32) -> i32 {
        match (&mut (*this).checks).iter_mut().find(|c| c.0 == id) {
            Some(c) => {
                c.1 = checked != 0;
                S_OK
            }
            None => E_INVALIDARG,
        }
    }

    pub static VTBL: Vtbl<30> = Vtbl([
        query_interface as Fp, add_ref as Fp, release as Fp,
        ok as Fp,        // EnableOpenDropDown
        ok as Fp,        // AddMenu
        ok as Fp,        // AddPushButton
        ok as Fp,        // AddComboBox
        ok as Fp,        // AddRadioButtonList
        add_check_button as Fp,
        ok as Fp,        // AddEditBox
        ok as Fp,        // AddSeparator
        ok as Fp,        // AddText
        ok as Fp,        // SetControlLabel
        ok as Fp,        // GetControlState
        ok as Fp,        // SetControlState
        ok as Fp,        // GetEditBoxText
        ok as Fp,        // SetEditBoxText
        get_check_button_state as Fp,
        set_check_button_state as Fp,
        ok as Fp,        // AddControlItem
        ok as Fp,        // RemoveControlItem
        ok as Fp,        // RemoveAllControlItems
        ok as Fp,        // GetControlItemState
        ok as Fp,        // SetControlItemState
        ok as Fp,        // GetSelectedControlItem
        ok as Fp,        // SetSelectedControlItem
        ok as Fp,        // StartVisualGroup
        ok as Fp,        // EndVisualGroup
        ok as Fp,        // MakeProminent
        ok as Fp,        // SetControlItemText
    ]);
}

// ---- IFileOpenDialog / IFileSaveDialog -----------------------------------------------------------------------------

#[repr(C)]
pub struct FileDialog {
    vtbl: *const Fp,
    refs: AtomicU32,
    save: bool,
    options: u32,
    title: String,
    def_ext: String,
    file_name: String,
    /// (name, spec) of the file types
    types: Vec<(String, String)>,
    type_index: u32,
    folder: Option<PathBuf>,
    default_folder: Option<PathBuf>,
    events: usize,
    customize: usize,
    results: Vec<PathBuf>,
}

mod dialog {
    use super::*;

    unknown_methods!(FileDialog, [IID_IUNKNOWN, IID_IFILE_DIALOG, IID_IMODAL_WINDOW, IID_IFILE_OPEN_DIALOG, IID_IFILE_SAVE_DIALOG]);

    unsafe fn query_other(this: *mut FileDialog, riid: usize, out: *mut usize) -> i32 {
        if iid_is(riid, &[IID_IFILE_DIALOG_CUSTOMIZE]) {
            if (*this).customize == 0 {
                (*this).customize = Box::into_raw(Box::new(Customize { vtbl: &customize::VTBL, refs: AtomicU32::new(1), checks: Vec::new() })) as usize;
            }
            let c = (*this).customize as *mut Customize;
            (*c).refs.fetch_add(1, Ordering::SeqCst);
            *out = c as usize;
            return S_OK;
        }
        E_NOINTERFACE
    }

    unsafe fn destroy(this: *mut FileDialog) {
        let d = Box::from_raw(this);
        if d.events != 0 {
            release_com(d.events);
        }
        if d.customize != 0 {
            release_com(d.customize);
        }
    }

    unsafe fn release_com(p: usize) {
        let vt = *(p as *const *const usize);
        type Rel = unsafe extern "win64" fn(usize) -> u32;
        let rel: Rel = std::mem::transmute(*vt.add(2));
        rel(p);
    }

    unsafe fn wstr_arg(p: usize) -> String {
        if p == 0 { String::new() } else { w_ptr_to_string(p as *const u16) }
    }

    /// The request the program's settings amount to.
    unsafe fn request(d: &FileDialog) -> Request {
        // FOS_PICKFOLDERS is a different bit in the old flags
        let mut flags = d.options & !0x20;
        if d.options & 0x20 != 0 {
            flags |= filedlg::OFN_PICKFOLDERS;
        }
        let filters: Vec<(String, Vec<String>)> = d.types.iter().map(|(n, s)| (n.clone(), s.split(';').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())).collect();
        let dir = d.folder.clone().or_else(|| d.default_folder.clone()).map(|p| paths::host_to_win(&p)).unwrap_or_default();
        Request {
            save: d.save,
            flags,
            title: if d.title.is_empty() { if d.save { "Save As".into() } else { "Open".into() } } else { d.title.clone() },
            initial_dir: dir,
            initial_file: d.file_name.clone(),
            def_ext: d.def_ext.clone(),
            filters,
            filter_index: d.type_index.max(1) as usize,
        }
    }

    unsafe extern "win64" fn show(this: *mut FileDialog, owner: usize) -> i32 {
        let d = &mut *this;
        let req = request(d);
        match filedlg::run(owner, req) {
            Some((files, filter)) => {
                d.type_index = filter as u32;
                d.results = files;
                if let Some(first) = d.results.first() {
                    d.file_name = first.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    d.folder = first.parent().map(Path::to_path_buf);
                }
                // IFileDialogEvents::OnFileOk: S_FALSE would refuse the choice
                if d.events != 0 {
                    let vt = *(d.events as *const *const usize);
                    type OnFileOk = unsafe extern "win64" fn(usize, usize) -> i32;
                    let f: OnFileOk = std::mem::transmute(*vt.add(3));
                    let _ = f(d.events, this as usize);
                }
                S_OK
            }
            None => HRESULT_CANCELLED,
        }
    }

    unsafe extern "win64" fn set_file_types(this: *mut FileDialog, count: u32, specs: usize) -> i32 {
        let d = &mut *this;
        d.types.clear();
        for i in 0..count as usize {
            // COMDLG_FILTERSPEC: pszName, pszSpec
            let name = wstr_arg(*((specs + i * 16) as *const usize));
            let spec = wstr_arg(*((specs + i * 16 + 8) as *const usize));
            d.types.push((name, spec));
        }
        S_OK
    }

    unsafe extern "win64" fn set_file_type_index(this: *mut FileDialog, index: u32) -> i32 {
        (*this).type_index = index;
        S_OK
    }

    unsafe extern "win64" fn get_file_type_index(this: *mut FileDialog, out: *mut u32) -> i32 {
        if out.is_null() {
            return E_POINTER;
        }
        *out = (*this).type_index.max(1);
        S_OK
    }

    unsafe extern "win64" fn advise(this: *mut FileDialog, events: usize, cookie: *mut u32) -> i32 {
        let d = &mut *this;
        if events != 0 {
            let vt = *(events as *const *const usize);
            type AddRef = unsafe extern "win64" fn(usize) -> u32;
            let f: AddRef = std::mem::transmute(*vt.add(1));
            f(events);
        }
        d.events = events;
        if !cookie.is_null() {
            *cookie = 1;
        }
        S_OK
    }

    unsafe extern "win64" fn unadvise(this: *mut FileDialog, _cookie: u32) -> i32 {
        let d = &mut *this;
        if d.events != 0 {
            release_com(d.events);
            d.events = 0;
        }
        S_OK
    }

    unsafe extern "win64" fn set_options(this: *mut FileDialog, options: u32) -> i32 {
        (*this).options = options;
        S_OK
    }

    unsafe extern "win64" fn get_options(this: *mut FileDialog, out: *mut u32) -> i32 {
        if out.is_null() {
            return E_POINTER;
        }
        *out = (*this).options;
        S_OK
    }

    unsafe extern "win64" fn set_default_folder(this: *mut FileDialog, item: usize) -> i32 {
        (*this).default_folder = item_path(item);
        S_OK
    }

    unsafe extern "win64" fn set_folder(this: *mut FileDialog, item: usize) -> i32 {
        (*this).folder = item_path(item);
        S_OK
    }

    unsafe extern "win64" fn get_folder(this: *mut FileDialog, out: *mut usize) -> i32 {
        let d = &*this;
        match d.folder.clone().or_else(|| d.default_folder.clone()).or_else(|| Some(paths::cwd())) {
            Some(p) if !out.is_null() => {
                *out = new_item(p);
                S_OK
            }
            _ => E_FAIL,
        }
    }

    unsafe extern "win64" fn get_current_selection(this: *mut FileDialog, out: *mut usize) -> i32 {
        match (&(*this).results).first() {
            Some(p) if !out.is_null() => {
                *out = new_item(p.clone());
                S_OK
            }
            _ => E_FAIL,
        }
    }

    unsafe extern "win64" fn set_file_name(this: *mut FileDialog, name: usize) -> i32 {
        (*this).file_name = wstr_arg(name);
        S_OK
    }

    unsafe extern "win64" fn get_file_name(this: *mut FileDialog, out: *mut usize) -> i32 {
        if out.is_null() {
            return E_POINTER;
        }
        *out = task_string(&(&(*this)).file_name);
        S_OK
    }

    unsafe extern "win64" fn set_title(this: *mut FileDialog, title: usize) -> i32 {
        (*this).title = wstr_arg(title);
        S_OK
    }

    unsafe extern "win64" fn get_result(this: *mut FileDialog, out: *mut usize) -> i32 {
        match (&(*this).results).first() {
            Some(p) if !out.is_null() => {
                *out = new_item(p.clone());
                S_OK
            }
            _ => E_FAIL,
        }
    }

    unsafe extern "win64" fn set_default_extension(this: *mut FileDialog, ext: usize) -> i32 {
        (*this).def_ext = wstr_arg(ext).trim_start_matches('.').to_string();
        S_OK
    }

    unsafe extern "win64" fn get_results(this: *mut FileDialog, out: *mut usize) -> i32 {
        if out.is_null() {
            return E_POINTER;
        }
        if (&(*this).results).is_empty() {
            return E_FAIL;
        }
        *out = new_array((&(*this).results).clone());
        S_OK
    }

    pub static OPEN_VTBL: Vtbl<29> = Vtbl([
        query_interface as Fp, add_ref as Fp, release as Fp,
        show as Fp,                 // 3 Show
        set_file_types as Fp,       // 4
        set_file_type_index as Fp,  // 5
        get_file_type_index as Fp,  // 6
        advise as Fp,               // 7
        unadvise as Fp,             // 8
        set_options as Fp,          // 9
        get_options as Fp,          // 10
        set_default_folder as Fp,   // 11
        set_folder as Fp,           // 12
        get_folder as Fp,           // 13
        get_current_selection as Fp, // 14
        set_file_name as Fp,        // 15
        get_file_name as Fp,        // 16
        set_title as Fp,            // 17
        ok as Fp,                   // 18 SetOkButtonLabel
        ok as Fp,                   // 19 SetFileNameLabel
        get_result as Fp,           // 20
        ok as Fp,                   // 21 AddPlace
        set_default_extension as Fp, // 22
        ok as Fp,                   // 23 Close
        ok as Fp,                   // 24 SetClientGuid
        ok as Fp,                   // 25 ClearClientData
        ok as Fp,                   // 26 SetFilter
        get_results as Fp,          // 27 GetResults (IFileOpenDialog)
        not_implemented as Fp,      // 28 GetSelectedItems
    ]);

    // IFileSaveDialog has no GetResults: its slots 27 on are SetSaveAsItem, SetProperties, ...
    pub static SAVE_VTBL: Vtbl<32> = Vtbl([
        query_interface as Fp, add_ref as Fp, release as Fp,
        show as Fp, set_file_types as Fp, set_file_type_index as Fp, get_file_type_index as Fp, advise as Fp, unadvise as Fp,
        set_options as Fp, get_options as Fp, set_default_folder as Fp, set_folder as Fp, get_folder as Fp, get_current_selection as Fp,
        set_file_name as Fp, get_file_name as Fp, set_title as Fp, ok as Fp, ok as Fp, get_result as Fp, ok as Fp,
        set_default_extension as Fp, ok as Fp, ok as Fp, ok as Fp, ok as Fp,
        not_implemented as Fp, not_implemented as Fp, not_implemented as Fp, not_implemented as Fp, not_implemented as Fp,
    ]);
}

/// A new file dialog object of the open or save kind, with one reference.
pub unsafe fn new_dialog(save: bool) -> usize {
    // what Windows reports before the program changes anything
    let options = if save { 0x2 | 0x8 | 0x800 | 0x8000 } else { 0x8 | 0x800 | 0x1000 };
    let vtbl = if save { &dialog::SAVE_VTBL as *const _ as *const Fp } else { &dialog::OPEN_VTBL as *const _ as *const Fp };
    Box::into_raw(Box::new(FileDialog { vtbl, refs: AtomicU32::new(1), save, options, title: String::new(), def_ext: String::new(), file_name: String::new(), types: Vec::new(), type_index: 1, folder: None, default_folder: None, events: 0, customize: 0, results: Vec::new() })) as usize
}

/// `CoCreateInstance` for the classes made here; `None` when `clsid` is not one of them.
pub unsafe fn create_instance(clsid: usize, riid: usize, out: *mut usize) -> Option<i32> {
    if out.is_null() || clsid == 0 {
        return None;
    }
    let c = std::slice::from_raw_parts(clsid as *const u8, 16);
    let save = if c == CLSID_FILE_OPEN_DIALOG {
        false
    } else if c == CLSID_FILE_SAVE_DIALOG {
        true
    } else {
        return None;
    };
    *out = 0;
    let obj = new_dialog(save);
    let vt = *(obj as *const *const usize);
    type Qi = unsafe extern "win64" fn(usize, usize, *mut usize) -> i32;
    let qi: Qi = std::mem::transmute(*vt);
    let hr = qi(obj, riid, out);
    type Rel = unsafe extern "win64" fn(usize) -> u32;
    let rel: Rel = std::mem::transmute(*vt.add(2));
    rel(obj);
    Some(hr)
}
