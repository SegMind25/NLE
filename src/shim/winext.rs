//! The libraries real-world programs touch beyond the basics: visual styles (which answer "no theme is active", so
//! programs draw with the classic controls), input methods, the desktop window manager, BSTR strings and variants,
//! drag and drop, file operations of the shell, SIDs, CryptoAPI / CNG hashes, certificate and trust functions that
//! report "nothing there", date and time formatting, thread-pool work items, and a few user32 / gdi32 odds and ends.

use crate::runtime::hash::{self, Algo, Hasher};
use crate::runtime::strings::*;
use crate::runtime::paths;
use crate::win::*;
use std::collections::HashMap;
use std::sync::Mutex;

const S_OK: i32 = 0;
const E_FAIL: i32 = 0x8000_4005u32 as i32;
const E_HANDLE: i32 = 0x8007_0006u32 as i32;
const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154u32 as i32;
const TRUST_E_NOSIGNATURE: i32 = 0x800B_0100u32 as i32;
const DRAGDROP_S_CANCEL: i32 = 0x0004_0101;

// ---- visual styles: no theme ---------------------------------------------------------------------------------

crate::shims! { "uxtheme" => UXTHEME_SHIMS;
    fn OpenThemeData(_hwnd: usize, _classes: LPCWSTR) -> usize {
        0
    }
    fn OpenThemeDataEx(_hwnd: usize, _classes: LPCWSTR, _flags: DWORD) -> usize {
        0
    }
    fn CloseThemeData(_theme: usize) -> i32 {
        S_OK
    }
    fn IsThemeActive() -> BOOL {
        FALSE
    }
    fn IsAppThemed() -> BOOL {
        FALSE
    }
    fn GetThemeAppProperties() -> DWORD {
        0
    }
    fn SetThemeAppProperties(_flags: DWORD) {}
    fn IsCompositionActive() -> BOOL {
        FALSE
    }
    fn GetCurrentThemeName(_name: LPWSTR, _n: i32, _color: LPWSTR, _nc: i32, _size: LPWSTR, _ns: i32) -> i32 {
        E_FAIL
    }
    fn SetWindowTheme(_hwnd: usize, _name: LPCWSTR, _list: LPCWSTR) -> i32 {
        S_OK
    }
    fn EnableTheming(_enable: BOOL) -> i32 {
        S_OK
    }
    fn EnableThemeDialogTexture(_hwnd: usize, _flags: DWORD) -> i32 {
        S_OK
    }
    fn IsThemeDialogTextureEnabled(_hwnd: usize) -> BOOL {
        FALSE
    }
    fn DrawThemeBackground(_t: usize, _hdc: usize, _part: i32, _state: i32, _rect: usize, _clip: usize) -> i32 {
        E_HANDLE
    }
    fn DrawThemeBackgroundEx(_t: usize, _hdc: usize, _part: i32, _state: i32, _rect: usize, _opts: usize) -> i32 {
        E_HANDLE
    }
    fn DrawThemeParentBackground(_hwnd: usize, _hdc: usize, _rect: usize) -> i32 {
        E_HANDLE
    }
    fn DrawThemeText(_t: usize, _hdc: usize, _part: i32, _state: i32, _text: LPCWSTR, _n: i32, _flags: DWORD, _flags2: DWORD, _rect: usize) -> i32 {
        E_HANDLE
    }
    fn DrawThemeTextEx(_t: usize, _hdc: usize, _part: i32, _state: i32, _text: LPCWSTR, _n: i32, _flags: DWORD, _rect: usize, _opts: usize) -> i32 {
        E_HANDLE
    }
    fn DrawThemeEdge(_t: usize, _hdc: usize, _part: i32, _state: i32, _rect: usize, _edge: u32, _flags: u32, _content: usize) -> i32 {
        E_HANDLE
    }
    fn DrawThemeIcon(_t: usize, _hdc: usize, _part: i32, _state: i32, _rect: usize, _il: usize, _i: i32) -> i32 {
        E_HANDLE
    }
    fn GetThemePartSize(_t: usize, _hdc: usize, _part: i32, _state: i32, _rect: usize, _size: i32, _out: usize) -> i32 {
        E_HANDLE
    }
    fn GetThemeBackgroundContentRect(_t: usize, _hdc: usize, _part: i32, _state: i32, _rect: usize, _out: usize) -> i32 {
        E_HANDLE
    }
    fn GetThemeBackgroundExtent(_t: usize, _hdc: usize, _part: i32, _state: i32, _rect: usize, _out: usize) -> i32 {
        E_HANDLE
    }
    fn GetThemeTextExtent(_t: usize, _hdc: usize, _part: i32, _state: i32, _text: LPCWSTR, _n: i32, _flags: DWORD, _bound: usize, _out: usize) -> i32 {
        E_HANDLE
    }
    fn GetThemeFont(_t: usize, _hdc: usize, _part: i32, _state: i32, _prop: i32, _font: usize) -> i32 {
        E_HANDLE
    }
    fn GetThemeColor(_t: usize, _part: i32, _state: i32, _prop: i32, _out: *mut u32) -> i32 {
        E_HANDLE
    }
    fn GetThemeInt(_t: usize, _part: i32, _state: i32, _prop: i32, _out: *mut i32) -> i32 {
        E_HANDLE
    }
    fn GetThemeBool(_t: usize, _part: i32, _state: i32, _prop: i32, _out: *mut BOOL) -> i32 {
        E_HANDLE
    }
    fn GetThemeMetric(_t: usize, _hdc: usize, _part: i32, _state: i32, _prop: i32, _out: *mut i32) -> i32 {
        E_HANDLE
    }
    fn GetThemeMargins(_t: usize, _hdc: usize, _part: i32, _state: i32, _prop: i32, _rect: usize, _out: usize) -> i32 {
        E_HANDLE
    }
    fn GetThemePosition(_t: usize, _part: i32, _state: i32, _prop: i32, _out: usize) -> i32 {
        E_HANDLE
    }
    fn GetThemeRect(_t: usize, _part: i32, _state: i32, _prop: i32, _out: usize) -> i32 {
        E_HANDLE
    }
    fn GetThemeEnumValue(_t: usize, _part: i32, _state: i32, _prop: i32, _out: *mut i32) -> i32 {
        E_HANDLE
    }
    fn GetThemeTransitionDuration(_t: usize, _part: i32, _from: i32, _to: i32, _prop: i32, _out: *mut u32) -> i32 {
        E_HANDLE
    }
    fn GetThemeSysColor(_t: usize, id: i32) -> u32 {
        crate::gui::gdi::sys_color(id)
    }
    fn GetThemeSysFont(_t: usize, _id: i32, _font: usize) -> i32 {
        E_HANDLE
    }
    fn GetThemeSysSize(_t: usize, id: i32) -> i32 {
        match id {
            2 | 3 => 17,
            _ => 0,
        }
    }
    fn IsThemePartDefined(_t: usize, _part: i32, _state: i32) -> BOOL {
        FALSE
    }
    fn IsThemeBackgroundPartiallyTransparent(_t: usize, _part: i32, _state: i32) -> BOOL {
        FALSE
    }
    fn HitTestThemeBackground(_t: usize, _hdc: usize, _part: i32, _state: i32, _opts: DWORD, _rect: usize, _rgn: usize, _pt: i64, _code: *mut u16) -> i32 {
        E_HANDLE
    }
    fn BufferedPaintInit() -> i32 {
        S_OK
    }
    fn BufferedPaintUnInit() -> i32 {
        S_OK
    }
    fn BeginBufferedPaint(_hdc: usize, _rect: usize, _fmt: u32, _params: usize, out: *mut usize) -> usize {
        // no buffered paint: callers draw straight to the window when they get no buffer
        if !out.is_null() {
            *out = 0;
        }
        0
    }
    fn EndBufferedPaint(_h: usize, _update: BOOL) -> i32 {
        S_OK
    }
    fn GetBufferedPaintBits(_h: usize, _bits: usize, _cx: *mut i32) -> i32 {
        E_FAIL
    }
    fn GetBufferedPaintDC(_h: usize) -> usize {
        0
    }
    fn GetBufferedPaintTargetDC(_h: usize) -> usize {
        0
    }
    fn BufferedPaintClear(_h: usize, _rect: usize) -> i32 {
        S_OK
    }
    fn BufferedPaintSetAlpha(_h: usize, _rect: usize, _alpha: u8) -> i32 {
        S_OK
    }
    fn BufferedPaintStopAllAnimations(_hwnd: usize) -> i32 {
        S_OK
    }
    fn BufferedPaintRenderAnimation(_hwnd: usize, _hdc: usize) -> BOOL {
        FALSE
    }
    fn BeginBufferedAnimation(_hwnd: usize, _hdc: usize, _rect: usize, _fmt: u32, _params: usize, _anim: usize, _from: *mut usize, _to: *mut usize) -> usize {
        0
    }
    fn EndBufferedAnimation(_h: usize, _update: BOOL) -> i32 {
        S_OK
    }
}

// ---- input methods and the desktop window manager -----------------------------------------------------------

crate::shims! { "imm32" => IMM_SHIMS;
    fn ImmGetContext(_hwnd: usize) -> usize {
        0
    }
    fn ImmReleaseContext(_hwnd: usize, _himc: usize) -> BOOL {
        TRUE
    }
    fn ImmCreateContext() -> usize {
        0
    }
    fn ImmDestroyContext(_himc: usize) -> BOOL {
        TRUE
    }
    fn ImmAssociateContext(_hwnd: usize, _himc: usize) -> usize {
        0
    }
    fn ImmAssociateContextEx(_hwnd: usize, _himc: usize, _flags: DWORD) -> BOOL {
        TRUE
    }
    fn ImmGetCompositionStringW(_himc: usize, _index: DWORD, _buf: usize, _len: DWORD) -> i32 {
        0
    }
    fn ImmGetCompositionStringA(_himc: usize, _index: DWORD, _buf: usize, _len: DWORD) -> i32 {
        0
    }
    fn ImmSetCompositionStringW(_himc: usize, _index: DWORD, _c: usize, _cl: DWORD, _r: usize, _rl: DWORD) -> BOOL {
        TRUE
    }
    fn ImmSetCompositionWindow(_himc: usize, _form: usize) -> BOOL {
        TRUE
    }
    fn ImmSetCandidateWindow(_himc: usize, _form: usize) -> BOOL {
        TRUE
    }
    fn ImmSetCompositionFontW(_himc: usize, _lf: usize) -> BOOL {
        TRUE
    }
    fn ImmSetCompositionFontA(_himc: usize, _lf: usize) -> BOOL {
        TRUE
    }
    fn ImmNotifyIME(_himc: usize, _action: DWORD, _index: DWORD, _value: DWORD) -> BOOL {
        TRUE
    }
    fn ImmEscapeW(_hkl: usize, _himc: usize, _code: u32, _data: usize) -> isize {
        0
    }
    fn ImmGetOpenStatus(_himc: usize) -> BOOL {
        FALSE
    }
    fn ImmSetOpenStatus(_himc: usize, _open: BOOL) -> BOOL {
        TRUE
    }
    fn ImmGetConversionStatus(_himc: usize, _conv: *mut DWORD, _sentence: *mut DWORD) -> BOOL {
        FALSE
    }
    fn ImmSetConversionStatus(_himc: usize, _conv: DWORD, _sentence: DWORD) -> BOOL {
        TRUE
    }
    fn ImmGetDefaultIMEWnd(_hwnd: usize) -> usize {
        0
    }
    fn ImmIsIME(_hkl: usize) -> BOOL {
        FALSE
    }
    fn ImmGetCandidateWindow(_himc: usize, _index: DWORD, _form: usize) -> BOOL {
        FALSE
    }
    fn ImmGetCompositionWindow(_himc: usize, _form: usize) -> BOOL {
        FALSE
    }
    fn ImmGetCompositionFontW(_himc: usize, _lf: usize) -> BOOL {
        FALSE
    }
    fn ImmGetVirtualKey(_hwnd: usize) -> u32 {
        0xff
    }
}

crate::shims! { "dwmapi" => DWM_SHIMS;
    fn DwmIsCompositionEnabled(out: *mut BOOL) -> i32 {
        if !out.is_null() {
            *out = FALSE;
        }
        S_OK
    }
    fn DwmSetWindowAttribute(_hwnd: usize, _attr: DWORD, _value: usize, _size: DWORD) -> i32 {
        S_OK
    }
    fn DwmGetWindowAttribute(_hwnd: usize, _attr: DWORD, _value: usize, _size: DWORD) -> i32 {
        E_FAIL
    }
    fn DwmExtendFrameIntoClientArea(_hwnd: usize, _margins: usize) -> i32 {
        S_OK
    }
    fn DwmEnableBlurBehindWindow(_hwnd: usize, _bb: usize) -> i32 {
        S_OK
    }
    fn DwmFlush() -> i32 {
        S_OK
    }
    fn DwmGetColorizationColor(color: *mut DWORD, opaque: *mut BOOL) -> i32 {
        if !color.is_null() {
            *color = 0xff00_78d7;
        }
        if !opaque.is_null() {
            *opaque = TRUE;
        }
        S_OK
    }
    fn DwmDefWindowProc(_hwnd: usize, _msg: u32, _wp: usize, _lp: isize, _result: *mut isize) -> BOOL {
        FALSE
    }
    fn DwmEnableComposition(_how: u32) -> i32 {
        S_OK
    }
}

crate::shims! { "sensapi" => SENSAPI_SHIMS;
    fn IsNetworkAlive(flags: *mut DWORD) -> BOOL {
        if !flags.is_null() {
            *flags = 1;
        }
        TRUE
    }
    fn IsDestinationReachableW(_dest: LPCWSTR, _qos: usize) -> BOOL {
        FALSE
    }
    fn IsDestinationReachableA(_dest: LPCSTR, _qos: usize) -> BOOL {
        FALSE
    }
}

// ---- BSTR strings and variants -------------------------------------------------------------------------------

unsafe fn bstr_alloc(chars: *const u16, len: usize) -> *mut u16 {
    let bytes = len * 2;
    let block = libc::malloc(4 + bytes + 2) as *mut u8;
    if block.is_null() {
        return std::ptr::null_mut();
    }
    *(block as *mut u32) = bytes as u32;
    let data = block.add(4) as *mut u16;
    if chars.is_null() {
        std::ptr::write_bytes(block.add(4), 0, bytes);
    } else {
        std::ptr::copy_nonoverlapping(chars, data, len);
    }
    *data.add(len) = 0;
    data
}

unsafe fn wide_len(p: *const u16) -> usize {
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    n
}

unsafe fn bstr_free(b: *mut u16) {
    if !b.is_null() {
        libc::free((b as *mut u8).sub(4) as *mut libc::c_void);
    }
}

unsafe fn bstr_bytes(b: *const u16) -> u32 {
    if b.is_null() {
        0
    } else {
        *((b as *const u8).sub(4) as *const u32)
    }
}

crate::shims! { "oleaut32" => OLEAUT_EXT_SHIMS;
    fn SysAllocStringByteLen(s: LPCSTR, len: u32) -> *mut u16 {
        let b = bstr_alloc(std::ptr::null(), (len as usize).div_ceil(2));
        if !b.is_null() {
            *((b as *mut u8).sub(4) as *mut u32) = len;
            if !s.is_null() {
                std::ptr::copy_nonoverlapping(s, b as *mut u8, len as usize);
            }
        }
        b
    }
    fn SysReAllocString(b: *mut *mut u16, s: LPCWSTR) -> BOOL {
        let n = if s.is_null() { std::ptr::null_mut() } else { bstr_alloc(s, wide_len(s)) };
        bstr_free(*b);
        *b = n;
        (!n.is_null() || s.is_null()) as BOOL
    }
    fn SysReAllocStringLen(b: *mut *mut u16, s: LPCWSTR, len: u32) -> BOOL {
        let n = bstr_alloc(s, len as usize);
        bstr_free(*b);
        *b = n;
        (!n.is_null()) as BOOL
    }
    fn VariantCopy(dst: *mut u8, src: *const u8) -> i32 {
        std::ptr::copy_nonoverlapping(src, dst, 24);
        if *(src as *const u16) == 8 {
            let s = *(src.add(8) as *const *const u16);
            *(dst.add(8) as *mut *mut u16) = if s.is_null() { std::ptr::null_mut() } else { bstr_alloc(s, (bstr_bytes(s) / 2) as usize) };
        }
        S_OK
    }
}

// ---- ole32 drag and drop, shell32 and shlwapi odds and ends --------------------------------------------------

crate::shims! { "ole32" => OLE_EXT_SHIMS;
    fn RegisterDragDrop(_hwnd: usize, _target: usize) -> i32 {
        S_OK
    }
    fn RevokeDragDrop(_hwnd: usize) -> i32 {
        S_OK
    }
    fn DoDragDrop(_source: usize, _target: usize, _effects: DWORD, effect: *mut DWORD) -> i32 {
        if !effect.is_null() {
            *effect = 0;
        }
        DRAGDROP_S_CANCEL
    }
    fn ReleaseStgMedium(_medium: usize) {}
    fn CLSIDFromProgID(_prog: LPCWSTR, _clsid: usize) -> i32 {
        REGDB_E_CLASSNOTREG
    }
}

unsafe fn double_null_list(p: *const u16) -> Vec<String> {
    let mut out = Vec::new();
    if p.is_null() {
        return out;
    }
    let mut at = 0;
    loop {
        let n = wide_len(p.add(at));
        if n == 0 {
            break;
        }
        out.push(String::from_utf16_lossy(std::slice::from_raw_parts(p.add(at), n)));
        at += n + 1;
    }
    out
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

/// The files a `pFrom` entry names: itself, or what a wildcard in its last part matches.
fn expand_source(win: &str) -> Vec<std::path::PathBuf> {
    let host = paths::resolve(win);
    let name = host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.contains('*') && !name.contains('?') {
        return vec![host];
    }
    let (pat, dir) = (name.to_lowercase().chars().collect::<Vec<char>>(), host.parent().map(|p| p.to_path_buf()).unwrap_or_default());
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n: Vec<char> = e.file_name().to_string_lossy().to_lowercase().chars().collect();
            if crate::shim::k32_file::wildcard(&pat, &n) {
                out.push(e.path());
            }
        }
    }
    out
}

crate::shims! { "shell32" => SHELL_EXT_SHIMS;
    fn DragQueryFileW(_hdrop: usize, _index: u32, _buf: LPWSTR, _cch: u32) -> u32 {
        0
    }
    fn DragQueryFileA(_hdrop: usize, _index: u32, _buf: LPSTR, _cch: u32) -> u32 {
        0
    }
    fn DragQueryPoint(_hdrop: usize, _pt: *mut i32) -> BOOL {
        FALSE
    }
    fn DragFinish(_hdrop: usize) {}
    fn Shell_NotifyIconW(_msg: u32, _data: usize) -> BOOL {
        TRUE
    }
    fn Shell_NotifyIconA(_msg: u32, _data: usize) -> BOOL {
        TRUE
    }
    fn SHCreateItemFromParsingName(path: LPCWSTR, _ctx: usize, riid: usize, out: *mut usize) -> i32 {
        super::com_filedlg::item_from_parsing_name(&w_ptr_to_string(path), riid, out)
    }
    // SHFILEOPSTRUCTW: hwnd, wFunc @8, pFrom @16, pTo @24, fFlags @32, fAnyOperationsAborted @36
    fn SHFileOperationW(op: *mut u8) -> i32 {
        let func = *(op.add(8) as *const u32);
        let from = double_null_list(*(op.add(16) as *const *const u16));
        let to = double_null_list(*(op.add(24) as *const *const u16));
        *(op.add(36) as *mut i32) = FALSE;
        let mut failed = 0;
        for (i, f) in from.iter().enumerate() {
            for src in expand_source(f) {
                let name = src.file_name().map(|n| n.to_os_string()).unwrap_or_default();
                let dest = to.get(i).or(to.first()).map(|t| paths::resolve(t));
                let r = match func {
                    // FO_MOVE
                    1 => dest.map(|d| if d.is_dir() { d.join(&name) } else { d }).map_or(Err(std::io::Error::other("no destination")), |d| std::fs::rename(&src, &d).or_else(|_| copy_tree(&src, &d).and_then(|_| if src.is_dir() { std::fs::remove_dir_all(&src) } else { std::fs::remove_file(&src) }))),
                    // FO_COPY
                    2 => dest.map(|d| if d.is_dir() || from.len() > 1 { d.join(&name) } else { d }).map_or(Err(std::io::Error::other("no destination")), |d| copy_tree(&src, &d)),
                    // FO_DELETE
                    3 => if src.is_dir() { std::fs::remove_dir_all(&src) } else { std::fs::remove_file(&src) },
                    // FO_RENAME
                    4 => dest.map_or(Err(std::io::Error::other("no destination")), |d| std::fs::rename(&src, d)),
                    _ => Err(std::io::Error::other("unknown operation")),
                };
                if r.is_err() {
                    failed = 0x402;
                }
            }
        }
        failed
    }
}

fn win_path_ext(p: &[u16]) -> Option<usize> {
    let slash = p.iter().rposition(|c| *c == b'\\' as u16 || *c == b'/' as u16).map_or(0, |i| i + 1);
    p[slash..].iter().rposition(|c| *c == b'.' as u16).map(|i| slash + i)
}

fn rgb_to_hls(r: u8, g: u8, b: u8) -> (u16, u16, u16) {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-9 {
        return (0, (l * 240.0).round() as u16, 0);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    ((h * 240.0).round() as u16 % 240, (l * 240.0).round() as u16, (s * 240.0).round() as u16)
}

fn hls_to_rgb(h: u16, l: u16, s: u16) -> u32 {
    let (h, l, s) = (h as f64 / 240.0, l as f64 / 240.0, s as f64 / 240.0);
    let to = |c: f64| (c * 255.0).round().clamp(0.0, 255.0) as u32;
    if s == 0.0 {
        return to(l) * 0x0001_0101;
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    to(f(h + 1.0 / 3.0)) | to(f(h)) << 8 | to(f(h - 1.0 / 3.0)) << 16
}

crate::shims! { "shlwapi" => SHLWAPI_EXT_SHIMS;
    fn PathAddExtensionW(path: LPWSTR, ext: LPCWSTR) -> BOOL {
        let p = w_ptr_to_string(path);
        let pw: Vec<u16> = p.encode_utf16().collect();
        if win_path_ext(&pw).is_some() {
            return FALSE;
        }
        let e = if ext.is_null() { ".exe".to_string() } else { w_ptr_to_string(ext) };
        let joined = to_wide_nul(&format!("{p}{e}"));
        std::ptr::copy_nonoverlapping(joined.as_ptr(), path, joined.len());
        TRUE
    }
    fn PathRemoveExtensionW(path: LPWSTR) {
        let p = w_ptr_to_string(path);
        let pw: Vec<u16> = p.encode_utf16().collect();
        if let Some(i) = win_path_ext(&pw) {
            *path.add(i) = 0;
        }
    }
    fn PathGetDriveNumberW(path: LPCWSTR) -> i32 {
        let p = w_ptr_to_string(path);
        let b = p.as_bytes();
        if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
            (b[0].to_ascii_lowercase() - b'a') as i32
        } else {
            -1
        }
    }
    fn PathIsNetworkPathW(path: LPCWSTR) -> BOOL {
        w_ptr_to_string(path).starts_with("\\\\") as BOOL
    }
    fn PathMatchSpecW(path: LPCWSTR, spec: LPCWSTR) -> BOOL {
        let name: Vec<char> = w_ptr_to_string(path).to_lowercase().chars().collect();
        // a spec can hold several patterns separated by semicolons
        w_ptr_to_string(spec).to_lowercase().split(';').any(|p| crate::shim::k32_file::wildcard(&p.trim().chars().collect::<Vec<_>>(), &name)) as BOOL
    }
    fn PathCompactPathExW(out: LPWSTR, src: LPCWSTR, max: u32, _flags: DWORD) -> BOOL {
        let s: Vec<u16> = w_ptr_to_string(src).encode_utf16().collect();
        let max = max as usize;
        if max == 0 {
            return FALSE;
        }
        let keep = max.saturating_sub(1);
        let mut v: Vec<u16> = if s.len() <= keep {
            s
        } else if keep > 3 {
            let tail = (keep - 3) / 2;
            let head = keep - 3 - tail;
            let mut t = s[..head].to_vec();
            t.extend("...".encode_utf16());
            t.extend_from_slice(&s[s.len() - tail..]);
            t
        } else {
            s[..keep].to_vec()
        };
        v.push(0);
        std::ptr::copy_nonoverlapping(v.as_ptr(), out, v.len());
        TRUE
    }
    fn AssocQueryStringW(_flags: DWORD, _str: u32, _assoc: LPCWSTR, _extra: LPCWSTR, _out: LPWSTR, _cch: *mut DWORD) -> i32 {
        // no association is registered
        0x8000_0000u32 as i32 | 0x0007_0483
    }
    fn ColorRGBToHLS(rgb: u32, h: *mut u16, l: *mut u16, s: *mut u16) {
        let (hh, ll, ss) = rgb_to_hls(rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8);
        *h = hh;
        *l = ll;
        *s = ss;
    }
    fn ColorHLSToRGB(h: u16, l: u16, s: u16) -> u32 {
        hls_to_rgb(h, l, s)
    }
    fn ColorAdjustLuma(rgb: u32, n: i32, scale: BOOL) -> u32 {
        let (h, l, s) = rgb_to_hls(rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8);
        let l2 = if scale != 0 { (l as i32 + (l as i32 * n) / 1000).clamp(0, 240) } else { (l as i32 + n).clamp(0, 240) };
        hls_to_rgb(h, l2 as u16, s)
    }
}

// ---- advapi32: SIDs, text encoding, hashes, security descriptors ---------------------------------------------

unsafe fn sid_len(sid: *const u8) -> usize {
    8 + 4 * *sid.add(1) as usize
}

/// A running hash: plain, or keyed (HMAC) which keeps its input until it is finished.
enum HashObj {
    Plain(Hasher),
    Keyed { algo: Algo, key: Vec<u8>, data: Vec<u8> },
}

impl HashObj {
    fn algo(&self) -> Algo {
        match self {
            HashObj::Plain(h) => h.algo(),
            HashObj::Keyed { algo, .. } => *algo,
        }
    }
    fn update(&mut self, bytes: &[u8]) {
        match self {
            HashObj::Plain(h) => h.update(bytes),
            HashObj::Keyed { data, .. } => data.extend_from_slice(bytes),
        }
    }
    fn finish(&self) -> Vec<u8> {
        match self {
            HashObj::Plain(h) => h.finish(),
            HashObj::Keyed { algo, key, data } => hash::hmac(*algo, key, data),
        }
    }
}

static HASH_OBJECTS: Mutex<Option<HashMap<usize, HashObj>>> = Mutex::new(None);
static NEXT_HANDLE: Mutex<usize> = Mutex::new(0x7000);

fn next_handle() -> usize {
    let mut n = NEXT_HANDLE.lock().unwrap_or_else(|e| e.into_inner());
    *n += 4;
    *n
}

fn hash_new(h: HashObj) -> usize {
    let id = next_handle();
    HASH_OBJECTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id, h);
    id
}

fn with_hash<R>(id: usize, f: impl FnOnce(&mut HashObj) -> R) -> Option<R> {
    HASH_OBJECTS.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.get_mut(&id)).map(f)
}

crate::shims! { "advapi32" => ADVAPI_EXT_SHIMS;
    fn AllocateAndInitializeSid(authority: *const u8, count: u8, a0: DWORD, a1: DWORD, a2: DWORD, a3: DWORD, a4: DWORD, a5: DWORD, a6: DWORD, a7: DWORD, out: *mut *mut u8) -> BOOL {
        if count == 0 || count > 8 {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        let sid = libc::malloc(8 + 4 * count as usize) as *mut u8;
        *sid = 1;
        *sid.add(1) = count;
        std::ptr::copy_nonoverlapping(authority, sid.add(2), 6);
        for (i, v) in [a0, a1, a2, a3, a4, a5, a6, a7].iter().take(count as usize).enumerate() {
            *(sid.add(8 + 4 * i) as *mut DWORD) = *v;
        }
        *out = sid;
        TRUE
    }
    fn FreeSid(sid: *mut u8) -> *mut u8 {
        libc::free(sid as *mut libc::c_void);
        std::ptr::null_mut()
    }
    fn GetLengthSid(sid: *const u8) -> DWORD {
        sid_len(sid) as DWORD
    }
    fn IsValidSid(sid: *const u8) -> BOOL {
        (!sid.is_null() && *sid == 1 && *sid.add(1) <= 15) as BOOL
    }
    fn EqualSid(a: *const u8, b: *const u8) -> BOOL {
        (sid_len(a) == sid_len(b) && std::slice::from_raw_parts(a, sid_len(a)) == std::slice::from_raw_parts(b, sid_len(b))) as BOOL
    }
    fn CopySid(len: DWORD, dst: *mut u8, src: *const u8) -> BOOL {
        let n = sid_len(src);
        if (len as usize) < n {
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            return FALSE;
        }
        std::ptr::copy_nonoverlapping(src, dst, n);
        TRUE
    }
    fn CheckTokenMembership(_token: HANDLE, _sid: *const u8, member: *mut BOOL) -> BOOL {
        // the process runs with the rights of an ordinary user: never an administrator
        if !member.is_null() {
            *member = FALSE;
        }
        TRUE
    }
    fn IsTextUnicode(buf: *const u8, size: i32, result: *mut i32) -> BOOL {
        if buf.is_null() || size < 2 {
            return FALSE;
        }
        let b = std::slice::from_raw_parts(buf, size as usize);
        if b[0] == 0xff && b[1] == 0xfe {
            if !result.is_null() {
                *result = 8; // IS_TEXT_UNICODE_SIGNATURE
            }
            return TRUE;
        }
        if b[0] == 0xfe && b[1] == 0xff {
            if !result.is_null() {
                *result = 0x80; // IS_TEXT_UNICODE_REVERSE_SIGNATURE
            }
            return FALSE;
        }
        // UTF-16 text of mostly ASCII has a zero in every second byte
        let pairs = b.len() / 2;
        let zeros = (0..pairs).filter(|i| b[i * 2 + 1] == 0 && b[i * 2] != 0).count();
        let likely = pairs > 0 && zeros * 10 >= pairs * 6 && b.len() % 2 == 0;
        if likely && !result.is_null() {
            *result = 1; // IS_TEXT_UNICODE_ASCII16
        }
        likely as BOOL
    }
    fn GetNamedSecurityInfoW(_name: LPCWSTR, _ty: u32, _info: DWORD, _owner: usize, _group: usize, _dacl: usize, _sacl: usize, _sd: usize) -> DWORD {
        ERROR_NOT_SUPPORTED
    }
    fn GetFileSecurityW(_name: LPCWSTR, _info: DWORD, _sd: usize, _len: DWORD, _needed: *mut DWORD) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn SetFileSecurityW(_name: LPCWSTR, _info: DWORD, _sd: usize) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn CryptCreateHash(_prov: usize, alg: u32, _key: usize, _flags: DWORD, out: *mut usize) -> BOOL {
        match Algo::from_calg(alg) {
            Some(a) => {
                *out = hash_new(HashObj::Plain(Hasher::new(a)));
                TRUE
            }
            None => {
                set_last_error(0x8009_0008); // NTE_BAD_ALGID
                FALSE
            }
        }
    }
    fn CryptHashData(h: usize, data: *const u8, len: DWORD, _flags: DWORD) -> BOOL {
        let bytes = if len == 0 { &[][..] } else { std::slice::from_raw_parts(data, len as usize) };
        with_hash(h, |x| x.update(bytes)).is_some() as BOOL
    }
    // HP_ALGID 1, HP_HASHVAL 2, HP_HASHSIZE 4
    fn CryptGetHashParam(h: usize, param: DWORD, out: *mut u8, len: *mut DWORD, _flags: DWORD) -> BOOL {
        let Some(value) = with_hash(h, |x| match param {
            2 => x.finish(),
            4 => (x.algo().digest_len() as u32).to_le_bytes().to_vec(),
            _ => Vec::new(),
        }) else {
            set_last_error(ERROR_INVALID_HANDLE);
            return FALSE;
        };
        if value.is_empty() {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        let have = *len as usize;
        *len = value.len() as DWORD;
        if out.is_null() {
            return TRUE;
        }
        if have < value.len() {
            set_last_error(ERROR_MORE_DATA);
            return FALSE;
        }
        std::ptr::copy_nonoverlapping(value.as_ptr(), out, value.len());
        TRUE
    }
    fn CryptDestroyHash(h: usize) -> BOOL {
        HASH_OBJECTS.lock().unwrap_or_else(|e| e.into_inner()).as_mut().is_some_and(|m| m.remove(&h).is_some()) as BOOL
    }
}

// CNG: `BCryptOpenAlgorithmProvider` (misc.rs) calls `new_provider`; a provider handle remembers which hash it is for
static PROVIDERS: Mutex<Option<HashMap<usize, (Option<Algo>, bool)>>> = Mutex::new(None);

/// Makes the handle for an opened algorithm provider; `hmac` is set by BCRYPT_ALG_HANDLE_HMAC_FLAG.
pub fn new_provider(name: &str, hmac: bool) -> usize {
    let id = next_handle();
    PROVIDERS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id, (Algo::from_name(name), hmac));
    id
}

fn provider(handle: usize) -> Option<(Option<Algo>, bool)> {
    PROVIDERS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&handle).copied())
}

const STATUS_INVALID_HANDLE: NTSTATUS = 0xC000_0008u32 as i32;
const STATUS_BUFFER_TOO_SMALL: NTSTATUS = 0xC000_0023u32 as i32;
const STATUS_NOT_SUPPORTED: NTSTATUS = 0xC000_00BBu32 as i32;

unsafe fn keyed_object(alg: usize, secret: *const u8, secret_len: ULONG) -> Option<HashObj> {
    let (Some(a), hmac) = provider(alg)? else { return None };
    Some(if hmac || (!secret.is_null() && secret_len > 0) {
        let key = if secret.is_null() { Vec::new() } else { std::slice::from_raw_parts(secret, secret_len as usize).to_vec() };
        HashObj::Keyed { algo: a, key, data: Vec::new() }
    } else {
        HashObj::Plain(Hasher::new(a))
    })
}

crate::shims! { "bcrypt" => BCRYPT_HASH_SHIMS;
    fn BCryptCreateHash(alg: usize, out: *mut usize, _object: *mut u8, _object_len: ULONG, secret: *const u8, secret_len: ULONG, _flags: ULONG) -> NTSTATUS {
        match keyed_object(alg, secret, secret_len) {
            Some(o) => {
                *out = hash_new(o);
                0
            }
            None => STATUS_NOT_SUPPORTED,
        }
    }
    fn BCryptHashData(h: usize, data: *const u8, len: ULONG, _flags: ULONG) -> NTSTATUS {
        let bytes = if len == 0 { &[][..] } else { std::slice::from_raw_parts(data, len as usize) };
        if with_hash(h, |x| x.update(bytes)).is_some() { 0 } else { STATUS_INVALID_HANDLE }
    }
    fn BCryptFinishHash(h: usize, out: *mut u8, len: ULONG, _flags: ULONG) -> NTSTATUS {
        match with_hash(h, |x| x.finish()) {
            Some(d) if len as usize >= d.len() => {
                std::ptr::copy_nonoverlapping(d.as_ptr(), out, d.len());
                0
            }
            Some(_) => STATUS_BUFFER_TOO_SMALL,
            None => STATUS_INVALID_HANDLE,
        }
    }
    fn BCryptDestroyHash(h: usize) -> NTSTATUS {
        HASH_OBJECTS.lock().unwrap_or_else(|e| e.into_inner()).as_mut().map(|m| m.remove(&h));
        0
    }
    // one shot: algorithm, optional key, input, output
    fn BCryptHash(alg: usize, secret: *const u8, secret_len: ULONG, data: *const u8, len: ULONG, out: *mut u8, out_len: ULONG) -> NTSTATUS {
        let Some(mut o) = keyed_object(alg, secret, secret_len) else { return STATUS_NOT_SUPPORTED };
        if len > 0 {
            o.update(std::slice::from_raw_parts(data, len as usize));
        }
        let d = o.finish();
        if (out_len as usize) < d.len() {
            return STATUS_BUFFER_TOO_SMALL;
        }
        std::ptr::copy_nonoverlapping(d.as_ptr(), out, d.len());
        0
    }
    fn BCryptGetProperty(h: usize, prop: LPCWSTR, out: *mut u8, len: ULONG, result: *mut ULONG, _flags: ULONG) -> NTSTATUS {
        let name = w_ptr_to_string(prop);
        let algo = provider(h).and_then(|p| p.0).or_else(|| with_hash(h, |x| x.algo()));
        let value: u32 = match name.as_str() {
            "HashDigestLength" => match algo {
                Some(a) => a.digest_len() as u32,
                None => return STATUS_NOT_SUPPORTED,
            },
            "HashBlockLength" => algo.map_or(64, |a| a.block_len() as u32),
            "ObjectLength" => 16,
            _ => return STATUS_NOT_SUPPORTED,
        };
        if !result.is_null() {
            *result = 4;
        }
        if len < 4 || out.is_null() {
            return STATUS_BUFFER_TOO_SMALL;
        }
        *(out as *mut u32) = value;
        0
    }
    fn BCryptDuplicateHash(h: usize, out: *mut usize, _object: *mut u8, _len: ULONG, _flags: ULONG) -> NTSTATUS {
        let copy = with_hash(h, |x| match x {
            HashObj::Plain(p) => HashObj::Plain(p.clone()),
            HashObj::Keyed { algo, key, data } => HashObj::Keyed { algo: *algo, key: key.clone(), data: data.clone() },
        });
        match copy {
            Some(c) => {
                *out = hash_new(c);
                0
            }
            None => STATUS_INVALID_HANDLE,
        }
    }
}

// ---- certificates, trust, internet, debug help: nothing there -----------------------------------------------

crate::shims! { "crypt32" => CRYPT32_SHIMS;
    fn CertCloseStore(_store: usize, _flags: DWORD) -> BOOL {
        TRUE
    }
    fn CertOpenSystemStoreW(_prov: usize, _name: LPCWSTR) -> usize {
        0
    }
    fn CertOpenStore(_ty: usize, _enc: DWORD, _prov: usize, _flags: DWORD, _para: usize) -> usize {
        0
    }
    fn CertFindCertificateInStore(_store: usize, _enc: DWORD, _flags: DWORD, _ty: DWORD, _para: usize, _prev: usize) -> usize {
        0
    }
    fn CertFreeCertificateContext(_ctx: usize) -> BOOL {
        TRUE
    }
    fn CertGetCertificateContextProperty(_ctx: usize, _id: DWORD, _data: usize, _len: *mut DWORD) -> BOOL {
        set_last_error(0x8009_2004); // CRYPT_E_NOT_FOUND
        FALSE
    }
    fn CertGetNameStringW(_ctx: usize, _ty: DWORD, _flags: DWORD, _para: usize, buf: LPWSTR, cch: DWORD) -> DWORD {
        if !buf.is_null() && cch > 0 {
            *buf = 0;
        }
        1
    }
    fn CertNameToStrW(_enc: DWORD, _name: usize, _ty: DWORD, buf: LPWSTR, cch: DWORD) -> DWORD {
        if !buf.is_null() && cch > 0 {
            *buf = 0;
        }
        1
    }
    fn CryptQueryObject(_ty: DWORD, _obj: usize, _content: DWORD, _format: DWORD, _flags: DWORD, _enc: usize, _ct: usize, _ft: usize, _store: usize, _msg: usize, _ctx: usize) -> BOOL {
        set_last_error(0x8009_2004);
        FALSE
    }
    fn CryptMsgGetParam(_msg: usize, _ty: DWORD, _index: DWORD, _data: usize, _len: *mut DWORD) -> BOOL {
        set_last_error(0x8009_2004);
        FALSE
    }
    fn CryptMsgClose(_msg: usize) -> BOOL {
        TRUE
    }
    fn CryptDecodeObjectEx(_enc: DWORD, _ty: usize, _data: usize, _len: DWORD, _flags: DWORD, _para: usize, _out: usize, _outlen: *mut DWORD) -> BOOL {
        set_last_error(0x8009_2004);
        FALSE
    }
}

crate::shims! { "wintrust" => WINTRUST_SHIMS;
    fn WinVerifyTrust(_hwnd: usize, _action: usize, _data: usize) -> i32 {
        // nothing is signed as far as this layer can tell
        TRUST_E_NOSIGNATURE
    }
    fn WinVerifyTrustEx(_hwnd: usize, _action: usize, _data: usize) -> i32 {
        TRUST_E_NOSIGNATURE
    }
}

crate::shims! { "wininet" => WININET_SHIMS;
    fn InternetCrackUrlW(_url: LPCWSTR, _len: DWORD, _flags: DWORD, _parts: usize) -> BOOL {
        set_last_error(12005); // ERROR_INTERNET_INVALID_URL
        FALSE
    }
    fn InternetOpenW(_agent: LPCWSTR, _access: DWORD, _proxy: LPCWSTR, _bypass: LPCWSTR, _flags: DWORD) -> usize {
        set_last_error(12029); // ERROR_INTERNET_CANNOT_CONNECT
        0
    }
    fn InternetOpenA(_agent: LPCSTR, _access: DWORD, _proxy: LPCSTR, _bypass: LPCSTR, _flags: DWORD) -> usize {
        set_last_error(12029);
        0
    }
    fn InternetOpenUrlW(_net: usize, _url: LPCWSTR, _headers: LPCWSTR, _hl: DWORD, _flags: DWORD, _ctx: usize) -> usize {
        set_last_error(12029);
        0
    }
    fn InternetConnectW(_net: usize, _host: LPCWSTR, _port: u16, _user: LPCWSTR, _pass: LPCWSTR, _service: DWORD, _flags: DWORD, _ctx: usize) -> usize {
        set_last_error(12029);
        0
    }
    fn InternetReadFile(_h: usize, _buf: usize, _n: DWORD, _read: *mut DWORD) -> BOOL {
        set_last_error(ERROR_INVALID_HANDLE);
        FALSE
    }
    fn InternetCloseHandle(_h: usize) -> BOOL {
        TRUE
    }
    fn InternetGetConnectedState(flags: *mut DWORD, _reserved: DWORD) -> BOOL {
        if !flags.is_null() {
            *flags = 0x12; // LAN, configured
        }
        TRUE
    }
}

crate::shims! { "dbghelp" => DBGHELP_SHIMS;
    fn ImageNtHeader(base: *const u8) -> *const u8 {
        if base.is_null() || *(base as *const u16) != 0x5a4d {
            return std::ptr::null();
        }
        let off = *(base.add(0x3c) as *const u32) as usize;
        if *(base.add(off) as *const u32) != 0x4550 {
            return std::ptr::null();
        }
        base.add(off)
    }
}

// ---- kernel32 extras ------------------------------------------------------------------------------------------

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Formats a date with a Windows picture ("dddd, MMMM d, yyyy").
pub fn format_date(picture: &str, st: &SYSTEMTIME) -> String {
    let chars: Vec<char> = picture.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if matches!(c, 'd' | 'M' | 'y' | 'g') {
            let n = chars[i..].iter().take_while(|x| **x == c).count();
            let day = DAYS[(st.day_of_week as usize) % 7];
            let month = MONTHS[(st.month as usize).clamp(1, 12) - 1];
            match (c, n) {
                ('d', 1) => out.push_str(&st.day.to_string()),
                ('d', 2) => out.push_str(&format!("{:02}", st.day)),
                ('d', 3) => out.push_str(&day[..3]),
                ('d', _) => out.push_str(day),
                ('M', 1) => out.push_str(&st.month.to_string()),
                ('M', 2) => out.push_str(&format!("{:02}", st.month)),
                ('M', 3) => out.push_str(&month[..3]),
                ('M', _) => out.push_str(month),
                ('y', 1) => out.push_str(&(st.year % 100).to_string()),
                ('y', 2) => out.push_str(&format!("{:02}", st.year % 100)),
                ('y', _) => out.push_str(&format!("{:04}", st.year)),
                _ => out.push_str("A.D."),
            }
            i += n;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Formats a time with a Windows picture ("h:mm:ss tt").
pub fn format_time(picture: &str, st: &SYSTEMTIME) -> String {
    let chars: Vec<char> = picture.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                out.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if matches!(c, 'h' | 'H' | 'm' | 's' | 't') {
            let n = chars[i..].iter().take_while(|x| **x == c).count();
            let h12 = if st.hour % 12 == 0 { 12 } else { st.hour % 12 };
            match (c, n) {
                ('h', 1) => out.push_str(&h12.to_string()),
                ('h', _) => out.push_str(&format!("{h12:02}")),
                ('H', 1) => out.push_str(&st.hour.to_string()),
                ('H', _) => out.push_str(&format!("{:02}", st.hour)),
                ('m', 1) => out.push_str(&st.minute.to_string()),
                ('m', _) => out.push_str(&format!("{:02}", st.minute)),
                ('s', 1) => out.push_str(&st.second.to_string()),
                ('s', _) => out.push_str(&format!("{:02}", st.second)),
                ('t', 1) => out.push(if st.hour < 12 { 'A' } else { 'P' }),
                _ => out.push_str(if st.hour < 12 { "AM" } else { "PM" }),
            }
            i += n;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

unsafe fn now_or(st: *const SYSTEMTIME) -> SYSTEMTIME {
    if st.is_null() {
        let mut t = SYSTEMTIME::default();
        crate::shim::k32_time::GetLocalTime(&mut t);
        t
    } else {
        *st
    }
}

unsafe fn put_wide(text: &str, out: LPWSTR, cch: i32) -> i32 {
    let w = to_wide_nul(text);
    if cch == 0 {
        return w.len() as i32;
    }
    if (cch as usize) < w.len() {
        set_last_error(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    std::ptr::copy_nonoverlapping(w.as_ptr(), out, w.len());
    w.len() as i32
}

unsafe fn put_ansi(text: &str, out: LPSTR, cch: i32) -> i32 {
    let n = text.len() + 1;
    if cch == 0 {
        return n as i32;
    }
    if (cch as usize) < n {
        set_last_error(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    std::ptr::copy_nonoverlapping(text.as_ptr(), out, text.len());
    *out.add(text.len()) = 0;
    n as i32
}

fn date_picture(flags: DWORD, picture: String) -> String {
    if !picture.is_empty() {
        picture
    } else if flags & 2 != 0 {
        "dddd, MMMM d, yyyy".to_string()
    } else {
        "M/d/yyyy".to_string()
    }
}

fn time_picture(flags: DWORD, picture: String) -> String {
    if !picture.is_empty() {
        return picture;
    }
    let seconds = flags & 2 == 0;
    let h24 = flags & 8 != 0;
    let base = match (h24, seconds) {
        (true, true) => "HH:mm:ss",
        (true, false) => "HH:mm",
        (false, true) => "h:mm:ss tt",
        (false, false) => "h:mm tt",
    };
    if flags & 4 != 0 { base.trim_end_matches(" tt").to_string() } else { base.to_string() }
}

/// An open thread-pool work item: the callback and its context, and the threads started for it.
struct WorkItem {
    callback: usize,
    context: usize,
    threads: Vec<HANDLE>,
}

unsafe extern "win64" fn work_thread(param: usize) -> DWORD {
    let (cb, ctx, work) = *(param as *const (usize, usize, usize));
    libc::free(param as *mut libc::c_void);
    if cb != 0 {
        type Cb = unsafe extern "win64" fn(usize, usize, usize);
        let f: Cb = std::mem::transmute(cb);
        f(0, ctx, work);
    }
    0
}

crate::shims! { "kernel32" => K32_EXT_SHIMS;
    fn MulDiv(a: i32, b: i32, c: i32) -> i32 {
        if c == 0 {
            return -1;
        }
        let n = a as i64 * b as i64;
        let d = c as i64;
        let sign = if (n < 0) != (d < 0) { -1 } else { 1 };
        let r = sign * ((n.abs() + d.abs() / 2) / d.abs());
        if r > i32::MAX as i64 || r < i32::MIN as i64 { -1 } else { r as i32 }
    }
    fn GetDateFormatW(_locale: u32, flags: DWORD, st: *const SYSTEMTIME, picture: LPCWSTR, out: LPWSTR, cch: i32) -> i32 {
        let pic = date_picture(flags, if picture.is_null() { String::new() } else { w_ptr_to_string(picture) });
        put_wide(&format_date(&pic, &now_or(st)), out, cch)
    }
    fn GetDateFormatEx(_locale: LPCWSTR, flags: DWORD, st: *const SYSTEMTIME, picture: LPCWSTR, out: LPWSTR, cch: i32, _cal: LPCWSTR) -> i32 {
        let pic = date_picture(flags, if picture.is_null() { String::new() } else { w_ptr_to_string(picture) });
        put_wide(&format_date(&pic, &now_or(st)), out, cch)
    }
    fn GetDateFormatA(_locale: u32, flags: DWORD, st: *const SYSTEMTIME, picture: LPCSTR, out: LPSTR, cch: i32) -> i32 {
        let pic = date_picture(flags, if picture.is_null() { String::new() } else { a_to_string(picture) });
        put_ansi(&format_date(&pic, &now_or(st)), out, cch)
    }
    fn GetTimeFormatW(_locale: u32, flags: DWORD, st: *const SYSTEMTIME, picture: LPCWSTR, out: LPWSTR, cch: i32) -> i32 {
        let pic = time_picture(flags, if picture.is_null() { String::new() } else { w_ptr_to_string(picture) });
        put_wide(&format_time(&pic, &now_or(st)), out, cch)
    }
    fn GetTimeFormatEx(_locale: LPCWSTR, flags: DWORD, st: *const SYSTEMTIME, picture: LPCWSTR, out: LPWSTR, cch: i32) -> i32 {
        let pic = time_picture(flags, if picture.is_null() { String::new() } else { w_ptr_to_string(picture) });
        put_wide(&format_time(&pic, &now_or(st)), out, cch)
    }
    fn GetTimeFormatA(_locale: u32, flags: DWORD, st: *const SYSTEMTIME, picture: LPCSTR, out: LPSTR, cch: i32) -> i32 {
        let pic = time_picture(flags, if picture.is_null() { String::new() } else { a_to_string(picture) });
        put_ansi(&format_time(&pic, &now_or(st)), out, cch)
    }
    fn EnumSystemLocalesW(cb: usize, _flags: DWORD) -> BOOL {
        type Cb = unsafe extern "win64" fn(*const u16) -> BOOL;
        let f: Cb = std::mem::transmute(cb);
        f(to_wide_nul("00000409").as_ptr());
        TRUE
    }
    fn ReplaceFileW(replaced: LPCWSTR, replacement: LPCWSTR, backup: LPCWSTR, _flags: DWORD, _a: usize, _b: usize) -> BOOL {
        let (old, new) = (paths::resolve(&w_ptr_to_string(replaced)), paths::resolve(&w_ptr_to_string(replacement)));
        if !old.exists() {
            set_last_error(ERROR_FILE_NOT_FOUND);
            return FALSE;
        }
        if !backup.is_null() {
            let _ = std::fs::copy(&old, paths::resolve(&w_ptr_to_string(backup)));
        }
        match std::fs::rename(&new, &old) {
            Ok(()) => TRUE,
            Err(_) => {
                set_last_error_from_errno();
                FALSE
            }
        }
    }
    fn FindFirstStreamW(_path: LPCWSTR, _level: u32, _data: usize, _flags: DWORD) -> HANDLE {
        // no alternate data streams on this file system
        set_last_error(ERROR_HANDLE_EOF);
        INVALID_HANDLE_VALUE
    }
    fn RegisterApplicationRestart(_cmd: LPCWSTR, _flags: DWORD) -> i32 {
        S_OK
    }
    fn UnregisterApplicationRestart() -> i32 {
        S_OK
    }
    fn GetApplicationRestartSettings(_process: HANDLE, _cmd: LPWSTR, _size: *mut DWORD, _flags: *mut DWORD) -> i32 {
        E_FAIL
    }
    fn ReadDirectoryChangesW(_dir: HANDLE, _buf: usize, _len: DWORD, _subtree: BOOL, _filter: DWORD, _ret: *mut DWORD, _ov: usize, _cb: usize) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn CreateThreadpoolWork(callback: usize, context: usize, _env: usize) -> usize {
        Box::into_raw(Box::new(WorkItem { callback, context, threads: Vec::new() })) as usize
    }
    fn SubmitThreadpoolWork(work: usize) {
        let w = &mut *(work as *mut WorkItem);
        let arg = libc::malloc(24) as *mut (usize, usize, usize);
        *arg = (w.callback, w.context, work);
        let h = crate::shim::k32_thread::create_thread(0, work_thread as *const () as usize, arg as usize, 0, std::ptr::null_mut());
        if h != 0 {
            w.threads.push(h);
        }
    }
    fn WaitForThreadpoolWorkCallbacks(work: usize, _cancel: BOOL) {
        let w = &mut *(work as *mut WorkItem);
        for h in w.threads.drain(..) {
            crate::shim::k32_sync::WaitForSingleObject(h, INFINITE);
        }
    }
    fn CloseThreadpoolWork(work: usize) {
        WaitForThreadpoolWorkCallbacks(work, FALSE);
        drop(Box::from_raw(work as *mut WorkItem));
    }
    fn FreeLibraryWhenCallbackReturns(_instance: usize, _module: HMODULE) {}
}

crate::shims! { "comctl32" => COMCTL_EXT_SHIMS;
    // IMAGEINFO: hbmImage @0, hbmMask @8, two unused ints, rcImage @24. The image comes back as a bitmap of its own
    fn ImageList_GetImageInfo(himl: usize, index: i32, info: *mut u8) -> BOOL {
        use crate::gui::gdi::{Bitmap, Object};
        use crate::gui::surface::Surface;
        if info.is_null() {
            return FALSE;
        }
        crate::gui::with(|g| {
            let Some(l) = g.imagelist(himl) else { return FALSE };
            let Some((rgb, alpha)) = l.images.get(index.max(0) as usize).cloned() else { return FALSE };
            let (w, h) = (l.w, l.h);
            let mut color = Surface::new(w, h);
            let mut mask = Surface::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) as usize;
                    color.put(x, y, rgb[i]);
                    mask.put(x, y, if alpha[i] > 0x7f { 0 } else { 0xffffff });
                }
            }
            let hc = g.add_object(Object::Bitmap(Bitmap { surf: color, bpp: 32, bits: 0 }));
            let hm = g.add_object(Object::Bitmap(Bitmap { surf: mask, bpp: 1, bits: 0 }));
            *(info as *mut usize) = hc;
            *(info.add(8) as *mut usize) = hm;
            let r = info.add(24) as *mut i32;
            *r = 0;
            *r.add(1) = 0;
            *r.add(2) = w;
            *r.add(3) = h;
            TRUE
        })
    }
    // the icon in the size asked for, or a failure code that makes the caller fall back to LoadImage
    fn LoadIconWithScaleDown(inst: usize, name: usize, cx: i32, cy: i32, out: *mut usize) -> i32 {
        let h = crate::shim::user32::LoadImageW(inst, name, 1, cx, cy, 0);
        if h == 0 {
            if !out.is_null() {
                *out = 0;
            }
            return E_FAIL;
        }
        *out = h;
        S_OK
    }
    // which: 1 = small icon, 2 = large icon
    fn LoadIconMetric(inst: usize, name: usize, which: i32, out: *mut usize) -> i32 {
        let (cx, cy) = if which == 1 { (16, 16) } else { (32, 32) };
        LoadIconWithScaleDown(inst, name, cx, cy, out)
    }
}

crate::shims! { "shell32" => SHELL_DIR_SHIMS;
    fn SHCreateDirectory(_hwnd: HANDLE, path: LPCWSTR) -> i32 {
        crate::shim::misc::SHCreateDirectoryExW(_hwnd, path, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(h: u16) -> SYSTEMTIME {
        SYSTEMTIME { year: 2026, month: 10, day_of_week: 1, day: 5, hour: h, minute: 7, second: 9, milliseconds: 0 }
    }

    #[test]
    fn dates_follow_the_picture() {
        assert_eq!(format_date("M/d/yyyy", &st(1)), "10/5/2026");
        assert_eq!(format_date("dddd, MMMM d, yyyy", &st(1)), "Monday, October 5, 2026");
        assert_eq!(format_date("ddd dd MMM yy", &st(1)), "Mon 05 Oct 26");
        assert_eq!(format_date("yyyy'-'MM'-'dd 'day'", &st(1)), "2026-10-05 day");
    }

    #[test]
    fn times_follow_the_picture() {
        assert_eq!(format_time("h:mm:ss tt", &st(0)), "12:07:09 AM");
        assert_eq!(format_time("h:mm tt", &st(15)), "3:07 PM");
        assert_eq!(format_time("HH:mm:ss", &st(15)), "15:07:09");
        assert_eq!(time_picture(2 | 4, String::new()), "h:mm");
    }

    #[test]
    fn mul_div_rounds_like_windows() {
        unsafe {
            assert_eq!(MulDiv(10, 3, 4), 8);
            assert_eq!(MulDiv(-10, 3, 4), -8);
            assert_eq!(MulDiv(1, 1, 0), -1);
            assert_eq!(MulDiv(i32::MAX, 2, 1), -1);
        }
    }

    #[test]
    fn hls_round_trips_a_colour() {
        let (h, l, s) = rgb_to_hls(0x33, 0x99, 0xcc);
        let back = hls_to_rgb(h, l, s);
        let (r, g, b) = (back & 0xff, (back >> 8) & 0xff, (back >> 16) & 0xff);
        assert!((r as i32 - 0x33).abs() <= 3 && (g as i32 - 0x99).abs() <= 3 && (b as i32 - 0xcc).abs() <= 3, "{r:x} {g:x} {b:x}");
    }

    #[test]
    fn bstr_keeps_its_length_prefix() {
        unsafe {
            let src = to_wide_nul("hello");
            let b = bstr_alloc(src.as_ptr(), 5);
            assert_eq!(bstr_bytes(b), 10);
            assert_eq!(w_ptr_to_string(b), "hello");
            bstr_free(b);
        }
    }

    #[test]
    fn unicode_text_is_told_from_ascii() {
        unsafe {
            let utf16: Vec<u8> = "plain text here".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
            assert_eq!(IsTextUnicode(utf16.as_ptr(), utf16.len() as i32, std::ptr::null_mut()), TRUE);
            assert_eq!(IsTextUnicode(b"plain text here".as_ptr(), 15, std::ptr::null_mut()), FALSE);
        }
    }
}
