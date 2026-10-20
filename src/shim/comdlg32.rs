//! comdlg32: the common dialogs. The file dialogs are real (see `gui::filedlg`); colour, font and printing report that
//! the dialog could not be made, which programs treat as the user cancelling.

use crate::gui::filedlg::{self, Request};
use crate::runtime::paths;
use crate::runtime::strings::*;
use crate::win::*;
use std::path::PathBuf;

const CDERR_INITIALIZATION: u32 = 0x0002;
const PDERR_NODEFAULTPRN: u32 = 0x1008;

unsafe fn read_str(p: usize, wide: bool) -> String {
    if p == 0 {
        String::new()
    } else if wide {
        w_ptr_to_string(p as *const u16)
    } else {
        a_to_string(p as *const u8)
    }
}

/// A list of strings ended by an empty one ("desc\0pattern\0desc\0pattern\0\0").
unsafe fn read_multi(p: usize, wide: bool) -> Vec<String> {
    let mut out = Vec::new();
    if p == 0 {
        return out;
    }
    let mut at = p;
    loop {
        let s = read_str(at, wide);
        if s.is_empty() {
            break;
        }
        at += if wide { (s.encode_utf16().count() + 1) * 2 } else { s.len() + 1 };
        out.push(s);
    }
    out
}

unsafe fn request(ofn: *const u8, save: bool, wide: bool) -> Request {
    let ptr = |o: usize| *(ofn.add(o) as *const usize);
    let u32_at = |o: usize| *(ofn.add(o) as *const u32);
    let flags = u32_at(96);
    let title = read_str(ptr(88), wide);
    Request {
        save,
        flags,
        title: if title.is_empty() { if save { "Save As".into() } else { "Open".into() } } else { title },
        initial_dir: read_str(ptr(80), wide),
        initial_file: read_str(ptr(48), wide),
        def_ext: read_str(ptr(104), wide),
        filters: filedlg::parse_filters(&read_multi(ptr(24), wide)),
        filter_index: u32_at(44) as usize,
    }
}

/// Puts a string (or several, NUL separated) into the program's buffer; the number of units it needs when it does not fit.
unsafe fn store(dst: usize, cap: usize, text: &[String], wide: bool) -> Result<(), usize> {
    let mut units: Vec<u16> = Vec::new();
    for t in text {
        units.extend(t.encode_utf16());
        units.push(0);
    }
    if text.len() > 1 {
        units.push(0);
    }
    if wide {
        if units.len() > cap {
            return Err(units.len());
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), dst as *mut u16, units.len());
        Ok(())
    } else {
        let bytes: Vec<u8> = String::from_utf16_lossy(&units).into_bytes();
        if bytes.len() > cap {
            return Err(bytes.len());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst as *mut u8, bytes.len());
        Ok(())
    }
}

unsafe fn finish(ofn: *mut u8, files: &[PathBuf], filter: usize, wide: bool) -> BOOL {
    let ptr = |o: usize| *(ofn.add(o) as *const usize);
    let flags = *(ofn.add(96) as *const u32);
    let wins: Vec<String> = files.iter().map(|p| paths::host_to_win(p)).collect();
    let multi = wins.len() > 1 && flags & filedlg::OFN_ALLOWMULTISELECT != 0;
    let (strings, offset, ext) = if multi {
        let dir = wins[0].rsplit_once('\\').map_or(wins[0].clone(), |(d, _)| if d.ends_with(':') { format!("{d}\\") } else { d.to_string() });
        let mut v = vec![dir.clone()];
        v.extend(wins.iter().map(|w| w.rsplit('\\').next().unwrap_or(w).to_string()));
        (v, dir.encode_utf16().count() + 1, 0)
    } else {
        let (n, e) = filedlg::offsets(&wins[0]);
        (vec![wins[0].clone()], n, e)
    };
    let (file_ptr, cap) = (ptr(48), *(ofn.add(56) as *const u32) as usize);
    if file_ptr != 0 {
        if let Err(need) = store(file_ptr, cap, &strings, wide) {
            filedlg::set_extended_error(filedlg::FNERR_BUFFERTOOSMALL);
            if cap >= 1 {
                if wide {
                    *(file_ptr as *mut u16) = need.min(0xffff) as u16;
                } else if cap >= 2 {
                    *(file_ptr as *mut u16) = need.min(0xffff) as u16;
                }
            }
            return FALSE;
        }
    }
    let title_ptr = ptr(64);
    if title_ptr != 0 {
        let name = wins[0].rsplit('\\').next().unwrap_or(&wins[0]).to_string();
        let _ = store(title_ptr, *(ofn.add(72) as *const u32) as usize, &[name], wide);
    }
    *(ofn.add(44) as *mut u32) = filter as u32;
    *(ofn.add(100) as *mut u16) = offset as u16;
    *(ofn.add(102) as *mut u16) = ext as u16;
    filedlg::set_extended_error(0);
    TRUE
}

unsafe fn file_dialog(ofn: *mut u8, save: bool, wide: bool) -> BOOL {
    if ofn.is_null() || *(ofn as *const u32) < 76 {
        filedlg::set_extended_error(0x0001); // CDERR_STRUCTSIZE
        return FALSE;
    }
    let owner = *(ofn.add(8) as *const usize);
    filedlg::set_extended_error(0);
    let req = request(ofn, save, wide);
    if crate::runtime::cfg().trace.api {
        eprintln!("nle[gui]: {} dialog owner={owner:#x} {req:?}", if save { "save" } else { "open" });
    }
    match filedlg::run(owner, req) {
        Some((files, filter)) => finish(ofn, &files, filter, wide),
        None => FALSE,
    }
}

crate::shims! { "comdlg32";
    fn GetOpenFileNameW(ofn: *mut u8) -> BOOL {
        file_dialog(ofn, false, true)
    }
    fn GetOpenFileNameA(ofn: *mut u8) -> BOOL {
        file_dialog(ofn, false, false)
    }
    fn GetSaveFileNameW(ofn: *mut u8) -> BOOL {
        file_dialog(ofn, true, true)
    }
    fn GetSaveFileNameA(ofn: *mut u8) -> BOOL {
        file_dialog(ofn, true, false)
    }
    fn CommDlgExtendedError() -> DWORD {
        filedlg::extended_error()
    }
    fn ChooseColorW(_cc: *mut u8) -> BOOL {
        filedlg::set_extended_error(CDERR_INITIALIZATION);
        FALSE
    }
    fn ChooseColorA(_cc: *mut u8) -> BOOL {
        filedlg::set_extended_error(CDERR_INITIALIZATION);
        FALSE
    }
    fn ChooseFontW(_cf: *mut u8) -> BOOL {
        filedlg::set_extended_error(CDERR_INITIALIZATION);
        FALSE
    }
    fn ChooseFontA(_cf: *mut u8) -> BOOL {
        filedlg::set_extended_error(CDERR_INITIALIZATION);
        FALSE
    }
    fn PrintDlgW(_pd: *mut u8) -> BOOL {
        filedlg::set_extended_error(PDERR_NODEFAULTPRN);
        FALSE
    }
    fn PrintDlgA(_pd: *mut u8) -> BOOL {
        filedlg::set_extended_error(PDERR_NODEFAULTPRN);
        FALSE
    }
    fn PageSetupDlgW(_pd: *mut u8) -> BOOL {
        filedlg::set_extended_error(PDERR_NODEFAULTPRN);
        FALSE
    }
    fn PageSetupDlgA(_pd: *mut u8) -> BOOL {
        filedlg::set_extended_error(PDERR_NODEFAULTPRN);
        FALSE
    }
}
