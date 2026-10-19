//! Smaller Windows libraries: bcrypt, advapi32, shell32, ole32/oleaut32, user32, shlwapi, psapi, winmm,
//! version and userenv. They cover what console and service-style programs call; GUI-heavy API
//! surface (window creation, GDI, Direct3D) is deliberately absent and reported as unimplemented.

use crate::runtime::strings::*;
use crate::runtime::{self, paths};
use crate::shim::crt_printf::{self, Args};
use crate::win::*;
use std::ffi::c_void;

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_FAIL: i32 = 0x8000_4005u32 as i32;
pub const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
pub const E_OUTOFMEMORY: i32 = 0x8007_000eu32 as i32;
pub const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154u32 as i32;
pub const CO_E_NOTINITIALIZED: i32 = 0x8004_01f0u32 as i32;

fn user_profile() -> String {
    format!("C:\\users\\{}", runtime::user_name())
}

unsafe fn random_fill(buf: *mut u8, len: usize) {
    let mut done = 0;
    while done < len {
        let r = libc::getrandom(buf.add(done) as *mut c_void, len - done, 0);
        if r <= 0 {
            break;
        }
        done += r as usize;
    }
}

/// Splits a command line into arguments with the rules of `CommandLineToArgvW`.
pub fn split_command_line(cmd: &str) -> Vec<String> {
    let chars: Vec<char> = cmd.chars().collect();
    let mut args = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let mut arg = String::new();
        let mut in_quotes = false;
        loop {
            let mut backslashes = 0;
            while i < chars.len() && chars[i] == '\\' {
                backslashes += 1;
                i += 1;
            }
            if i < chars.len() && chars[i] == '"' {
                arg.push_str(&"\\".repeat(backslashes / 2));
                if backslashes % 2 == 1 {
                    arg.push('"');
                } else if in_quotes && chars.get(i + 1) == Some(&'"') {
                    arg.push('"');
                    i += 1;
                } else {
                    in_quotes = !in_quotes;
                }
                i += 1;
            } else {
                arg.push_str(&"\\".repeat(backslashes));
                if i >= chars.len() || (!in_quotes && (chars[i] == ' ' || chars[i] == '\t')) {
                    break;
                }
                arg.push(chars[i]);
                i += 1;
            }
        }
        args.push(arg);
    }
    args
}

fn csidl_path(csidl: i32) -> Option<String> {
    let p = user_profile();
    Some(match csidl & 0xff {
        0x00 | 0x10 | 0x19 => format!("{p}\\Desktop"),
        0x05 | 0x0c => format!("{p}\\Documents"),
        0x1a => format!("{p}\\AppData\\Roaming"),
        0x1c => format!("{p}\\AppData\\Local"),
        0x23 => "C:\\ProgramData".into(),
        0x24 => "C:\\windows".into(),
        0x25 | 0x29 => "C:\\windows\\system32".into(),
        0x26 | 0x2a => "C:\\Program Files".into(),
        0x28 => p,
        0x2b | 0x2c => "C:\\Program Files\\Common Files".into(),
        0x20 => format!("{p}\\AppData\\Local\\Microsoft\\Windows\\INetCache"),
        _ => return None,
    })
}

fn known_folder(guid: &[u8; 16]) -> Option<String> {
    let d1 = u32::from_le_bytes(guid[0..4].try_into().unwrap());
    let p = user_profile();
    Some(match d1 {
        0x3eb685db => format!("{p}\\AppData\\Roaming"),
        0xf1b32785 => format!("{p}\\AppData\\Local"),
        0xfdd39ad0 => format!("{p}\\Documents"),
        0x5e6c858f => p,
        0x62ab5d82 => "C:\\ProgramData".into(),
        0xb4bfcc3a => format!("{p}\\Desktop"),
        0x374de290 => format!("{p}\\Downloads"),
        0x905e63b6 => format!("{p}\\AppData\\Local"),
        0x905e63b7 => format!("{p}\\AppData\\LocalLow"),
        0x905e63b8 => format!("{p}\\AppData\\Local"),
        0x7b0db17d => "C:\\Program Files".into(),
        0xf38bf404 => "C:\\Program Files (x86)".into(),
        0xf38bf405 => "C:\\Windows".into(),
        _ => return None,
    })
}

unsafe fn co_task_wide(s: &str) -> *mut u16 {
    let w = to_wide_nul(s);
    let p = libc::malloc(w.len() * 2) as *mut u16;
    std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
    p
}

unsafe fn proc_mem(counters: *mut u8, cb: u32) -> BOOL {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let mut it = statm.split_whitespace().map(|v| v.parse::<usize>().unwrap_or(0));
    let (size, rss) = (it.next().unwrap_or(0) * 4096, it.next().unwrap_or(0) * 4096);
    if (cb as usize) < 72 {
        return FALSE;
    }
    std::ptr::write_bytes(counters, 0, cb as usize);
    *(counters as *mut u32) = cb;
    let w = |off: usize, v: usize| *(counters.add(off) as *mut usize) = v;
    w(16, rss);
    w(24, rss);
    w(48, size);
    w(56, size);
    w(64, size);
    TRUE
}

unsafe fn wsprintf_impl(buf: *mut u8, fmt: *const u8, va: *const u64) -> i32 {
    let out = crt_printf::fmt_narrow(fmt, &mut Args::new(va));
    std::ptr::copy_nonoverlapping(out.as_ptr(), buf, out.len());
    *buf.add(out.len()) = 0;
    out.len() as i32
}

unsafe fn wsprintfw_impl(buf: *mut u16, fmt: *const u16, va: *const u64) -> i32 {
    let w = crt_printf::to_utf16(&crt_printf::fmt_wide(fmt, &mut Args::new(va)));
    std::ptr::copy_nonoverlapping(w.as_ptr(), buf, w.len());
    *buf.add(w.len()) = 0;
    w.len() as i32
}

unsafe extern "win64" fn wsprintf_a_v(buf: *mut u8, fmt: *const u8, va: *const u64) -> i32 {
    wsprintf_impl(buf, fmt, va)
}

unsafe extern "win64" fn wsprintf_w_v(buf: *mut u16, fmt: *const u16, va: *const u64) -> i32 {
    wsprintfw_impl(buf, fmt, va)
}

crate::va_thunk!(wsprintf_a, 2, wsprintf_a_v);
crate::va_thunk!(wsprintf_w, 2, wsprintf_w_v);

crate::shim_extra!("user32" => USER_EXTRA;
    "wsprintfA" => wsprintf_a,
    "wsprintfW" => wsprintf_w,
);

crate::shims! { "bcrypt" => BCRYPT_SHIMS;
    fn ProcessPrng(buf: *mut u8, len: usize) -> BOOL {
        random_fill(buf, len);
        TRUE
    }
    fn BCryptGenRandom(_alg: usize, buf: *mut u8, len: ULONG, _flags: ULONG) -> NTSTATUS {
        random_fill(buf, len as usize);
        STATUS_SUCCESS
    }
    fn BCryptOpenAlgorithmProvider(out: *mut usize, _id: LPCWSTR, _impl: LPCWSTR, _flags: ULONG) -> NTSTATUS {
        *out = 1;
        STATUS_SUCCESS
    }
    fn BCryptCloseAlgorithmProvider(_h: usize, _flags: ULONG) -> NTSTATUS {
        STATUS_SUCCESS
    }
}

crate::shims! { "advapi32" => ADVAPI_SHIMS;
    fn SystemFunction036(buf: *mut u8, len: ULONG) -> u8 {
        random_fill(buf, len as usize);
        1
    }
    fn CryptAcquireContextW(prov: *mut usize, _c: LPCWSTR, _p: LPCWSTR, _t: DWORD, _f: DWORD) -> BOOL {
        *prov = 1;
        TRUE
    }
    fn CryptAcquireContextA(prov: *mut usize, _c: LPCSTR, _p: LPCSTR, _t: DWORD, _f: DWORD) -> BOOL {
        *prov = 1;
        TRUE
    }
    fn CryptReleaseContext(_p: usize, _f: DWORD) -> BOOL {
        TRUE
    }
    fn CryptGenRandom(_p: usize, len: DWORD, buf: *mut u8) -> BOOL {
        random_fill(buf, len as usize);
        TRUE
    }
    fn GetUserNameW(buf: LPWSTR, size: *mut DWORD) -> BOOL {
        let w = to_wide(&runtime::user_name());
        if copy_w(buf, *size as usize, &w) {
            *size = w.len() as DWORD + 1;
            TRUE
        } else {
            *size = w.len() as DWORD + 1;
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            FALSE
        }
    }
    fn GetUserNameA(buf: LPSTR, size: *mut DWORD) -> BOOL {
        let n = runtime::user_name();
        if copy_a(buf, *size as usize, n.as_bytes()) {
            *size = n.len() as DWORD + 1;
            TRUE
        } else {
            *size = n.len() as DWORD + 1;
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            FALSE
        }
    }
    fn OpenProcessToken(_p: HANDLE, _access: DWORD, token: *mut HANDLE) -> BOOL {
        *token = crate::runtime::handles::insert(crate::runtime::handles::Kind::Event(crate::runtime::handles::EventObj {
            manual: true,
            signaled: std::sync::atomic::AtomicBool::new(true),
        }));
        TRUE
    }
    fn OpenThreadToken(_t: HANDLE, _access: DWORD, _self: BOOL, token: *mut HANDLE) -> BOOL {
        OpenProcessToken(0, 0, token)
    }
    fn GetTokenInformation(_t: HANDLE, _class: u32, _buf: *mut u8, _len: DWORD, ret: *mut DWORD) -> BOOL {
        if !ret.is_null() {
            *ret = 0;
        }
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn LookupPrivilegeValueW(_sys: LPCWSTR, _name: LPCWSTR, luid: *mut u64) -> BOOL {
        *luid = 1;
        TRUE
    }
    fn AdjustTokenPrivileges(_t: HANDLE, _all: BOOL, _new: usize, _len: DWORD, _prev: usize, _ret: usize) -> BOOL {
        TRUE
    }
    fn RegisterEventSourceW(_m: LPCWSTR, _s: LPCWSTR) -> HANDLE {
        0
    }
    fn DeregisterEventSource(_h: HANDLE) -> BOOL {
        TRUE
    }
    fn ReportEventW(_h: HANDLE, _t: WORD, _c: WORD, _id: DWORD, _sid: usize, _n: WORD, _d: DWORD, _s: usize, _raw: usize) -> BOOL {
        TRUE
    }
    fn EventRegister(_p: usize, _cb: usize, _ctx: usize, h: *mut u64) -> DWORD {
        *h = 1;
        0
    }
    fn EventUnregister(_h: u64) -> DWORD {
        0
    }
    fn EventWriteTransfer(_h: u64, _d: usize, _a: usize, _r: usize, _n: DWORD, _p: usize) -> DWORD {
        0
    }
    fn EventSetInformation(_h: u64, _c: u32, _i: usize, _l: ULONG) -> DWORD {
        0
    }
}

crate::shims! { "userenv" => USERENV_SHIMS;
    fn GetUserProfileDirectoryW(_token: HANDLE, buf: LPWSTR, size: *mut DWORD) -> BOOL {
        let w = to_wide(&user_profile());
        if copy_w(buf, *size as usize, &w) {
            *size = w.len() as DWORD + 1;
            TRUE
        } else {
            *size = w.len() as DWORD + 1;
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            FALSE
        }
    }
    fn GetUserProfileDirectoryA(_token: HANDLE, buf: LPSTR, size: *mut DWORD) -> BOOL {
        let p = user_profile();
        if copy_a(buf, *size as usize, p.as_bytes()) {
            *size = p.len() as DWORD + 1;
            TRUE
        } else {
            *size = p.len() as DWORD + 1;
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            FALSE
        }
    }
}

crate::shims! { "shell32" => SHELL_SHIMS;
    fn CommandLineToArgvW(cmd: LPCWSTR, argc: *mut i32) -> *mut *mut u16 {
        let text = if cmd.is_null() || *cmd == 0 { runtime::cfg().exe_win.clone() } else { w_ptr_to_string(cmd) };
        let args = split_command_line(&text);
        let ptrs = args.len() * 8 + 8;
        let total: usize = ptrs + args.iter().map(|a| (a.encode_utf16().count() + 1) * 2).sum::<usize>();
        let block = libc::malloc(total) as *mut u8;
        let mut data = block.add(ptrs) as *mut u16;
        for (i, a) in args.iter().enumerate() {
            *(block as *mut *mut u16).add(i) = data;
            let w = to_wide_nul(a);
            std::ptr::copy_nonoverlapping(w.as_ptr(), data, w.len());
            data = data.add(w.len());
        }
        *(block as *mut *mut u16).add(args.len()) = std::ptr::null_mut();
        if !argc.is_null() {
            *argc = args.len() as i32;
        }
        block as *mut *mut u16
    }
    fn SHGetFolderPathW(_hwnd: HANDLE, csidl: i32, _token: HANDLE, _flags: DWORD, buf: LPWSTR) -> i32 {
        match csidl_path(csidl) {
            Some(p) => {
                copy_w(buf, MAX_PATH, &to_wide(&p));
                S_OK
            }
            None => E_FAIL,
        }
    }
    fn SHGetFolderPathA(_hwnd: HANDLE, csidl: i32, _token: HANDLE, _flags: DWORD, buf: LPSTR) -> i32 {
        match csidl_path(csidl) {
            Some(p) => {
                copy_a(buf, MAX_PATH, p.as_bytes());
                S_OK
            }
            None => E_FAIL,
        }
    }
    fn SHGetKnownFolderPath(guid: *const [u8; 16], _flags: DWORD, _token: HANDLE, out: *mut *mut u16) -> i32 {
        match known_folder(&*guid) {
            Some(p) => {
                *out = co_task_wide(&p);
                S_OK
            }
            None => {
                *out = std::ptr::null_mut();
                E_FAIL
            }
        }
    }
    fn SHCreateDirectoryExW(_hwnd: HANDLE, path: LPCWSTR, _sa: usize) -> i32 {
        match std::fs::create_dir_all(paths::resolve(&w_ptr_to_string(path))) {
            Ok(()) => 0,
            Err(_) => ERROR_ALREADY_EXISTS as i32,
        }
    }
    fn ShellExecuteW(_hwnd: HANDLE, _verb: LPCWSTR, file: LPCWSTR, params: LPCWSTR, _dir: LPCWSTR, _show: i32) -> usize {
        let target = w_ptr_to_string(file);
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(&target);
        let _ = params;
        match cmd.spawn() {
            Ok(_) => 33,
            Err(_) => 31,
        }
    }
    fn IsUserAnAdmin() -> BOOL {
        (libc::geteuid() == 0) as BOOL
    }
    fn SHGetSpecialFolderPathW(_hwnd: HANDLE, buf: LPWSTR, csidl: i32, _create: BOOL) -> BOOL {
        match csidl_path(csidl) {
            Some(p) => {
                copy_w(buf, MAX_PATH, &to_wide(&p));
                TRUE
            }
            None => FALSE,
        }
    }
}

crate::shims! { "ole32" => OLE_SHIMS;
    fn CoInitialize(_p: usize) -> i32 {
        S_OK
    }
    fn CoInitializeEx(_p: usize, _flags: DWORD) -> i32 {
        S_OK
    }
    fn CoUninitialize() {}
    fn OleInitialize(_p: usize) -> i32 {
        S_OK
    }
    fn OleUninitialize() {}
    fn CoInitializeSecurity(_a: usize, _b: i32, _c: usize, _d: usize, _e: DWORD, _f: DWORD, _g: usize, _h: DWORD, _i: usize) -> i32 {
        S_OK
    }
    fn CoCreateInstance(_clsid: usize, _outer: usize, _ctx: DWORD, _iid: usize, out: *mut usize) -> i32 {
        if !out.is_null() {
            *out = 0;
        }
        REGDB_E_CLASSNOTREG
    }
    fn CoGetClassObject(_clsid: usize, _ctx: DWORD, _server: usize, _iid: usize, out: *mut usize) -> i32 {
        if !out.is_null() {
            *out = 0;
        }
        REGDB_E_CLASSNOTREG
    }
    fn CoTaskMemAlloc(n: usize) -> *mut u8 {
        libc::malloc(n.max(1)) as *mut u8
    }
    fn CoTaskMemRealloc(p: *mut u8, n: usize) -> *mut u8 {
        libc::realloc(p as *mut c_void, n.max(1)) as *mut u8
    }
    fn CoTaskMemFree(p: *mut u8) {
        libc::free(p as *mut c_void);
    }
    fn CoCreateGuid(guid: *mut u8) -> i32 {
        random_fill(guid, 16);
        *guid.add(7) = (*guid.add(7) & 0x0f) | 0x40;
        *guid.add(8) = (*guid.add(8) & 0x3f) | 0x80;
        S_OK
    }
    fn StringFromGUID2(guid: *const [u8; 16], buf: LPWSTR, cch: i32) -> i32 {
        let g = &*guid;
        let s = format!(
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            u32::from_le_bytes(g[0..4].try_into().unwrap()),
            u16::from_le_bytes(g[4..6].try_into().unwrap()),
            u16::from_le_bytes(g[6..8].try_into().unwrap()),
            g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
        );
        if copy_w(buf, cch as usize, &to_wide(&s)) { 39 } else { 0 }
    }
    fn CoGetMalloc(_ctx: DWORD, out: *mut usize) -> i32 {
        *out = 0;
        E_NOTIMPL
    }
    fn CoSetProxyBlanket(_a: usize, _b: DWORD, _c: DWORD, _d: usize, _e: DWORD, _f: DWORD, _g: usize, _h: DWORD) -> i32 {
        S_OK
    }
}

crate::shims! { "oleaut32" => OLEAUT_SHIMS;
    fn SysAllocString(s: LPCWSTR) -> *mut u16 {
        SysAllocStringLen(s, wlen(s) as u32)
    }
    fn SysAllocStringLen(s: LPCWSTR, n: u32) -> *mut u16 {
        let bytes = n as usize * 2;
        let p = libc::malloc(bytes + 6) as *mut u8;
        *(p as *mut u32) = bytes as u32;
        let data = p.add(4) as *mut u16;
        if !s.is_null() {
            std::ptr::copy_nonoverlapping(s, data, n as usize);
        } else {
            std::ptr::write_bytes(data as *mut u8, 0, bytes);
        }
        *data.add(n as usize) = 0;
        data
    }
    fn SysFreeString(b: *mut u16) {
        if !b.is_null() {
            libc::free((b as *mut u8).sub(4) as *mut c_void);
        }
    }
    fn SysStringLen(b: *const u16) -> u32 {
        if b.is_null() { 0 } else { *((b as *const u8).sub(4) as *const u32) / 2 }
    }
    fn SysStringByteLen(b: *const u16) -> u32 {
        if b.is_null() { 0 } else { *((b as *const u8).sub(4) as *const u32) }
    }
    fn VariantInit(v: *mut u8) {
        std::ptr::write_bytes(v, 0, 24);
    }
    fn VariantClear(v: *mut u8) -> i32 {
        std::ptr::write_bytes(v, 0, 24);
        S_OK
    }
}

crate::shims! { "user32" => USER_SHIMS;
    fn SetProcessDPIAware() -> BOOL {
        TRUE
    }
    fn SetProcessDpiAwarenessContext(_c: usize) -> BOOL {
        TRUE
    }
    fn CharUpperW(s: *mut u16) -> *mut u16 {
        if (s as usize) < 0x10000 {
            return upper_unit(s as usize as u16) as usize as *mut u16;
        }
        let mut i = 0;
        while *s.add(i) != 0 {
            *s.add(i) = upper_unit(*s.add(i));
            i += 1;
        }
        s
    }
    fn CharLowerW(s: *mut u16) -> *mut u16 {
        if (s as usize) < 0x10000 {
            return lower_unit(s as usize as u16) as usize as *mut u16;
        }
        let mut i = 0;
        while *s.add(i) != 0 {
            *s.add(i) = lower_unit(*s.add(i));
            i += 1;
        }
        s
    }
    fn CharUpperA(s: *mut u8) -> *mut u8 {
        if (s as usize) < 0x10000 {
            return (s as usize as u8).to_ascii_uppercase() as usize as *mut u8;
        }
        let mut i = 0;
        while *s.add(i) != 0 {
            *s.add(i) = (*s.add(i)).to_ascii_uppercase();
            i += 1;
        }
        s
    }
    fn CharLowerA(s: *mut u8) -> *mut u8 {
        if (s as usize) < 0x10000 {
            return (s as usize as u8).to_ascii_lowercase() as usize as *mut u8;
        }
        let mut i = 0;
        while *s.add(i) != 0 {
            *s.add(i) = (*s.add(i)).to_ascii_lowercase();
            i += 1;
        }
        s
    }
    fn CharUpperBuffW(s: *mut u16, n: DWORD) -> DWORD {
        for i in 0..n as usize {
            *s.add(i) = upper_unit(*s.add(i));
        }
        n
    }
    fn CharLowerBuffW(s: *mut u16, n: DWORD) -> DWORD {
        for i in 0..n as usize {
            *s.add(i) = lower_unit(*s.add(i));
        }
        n
    }
    fn CharNextA(s: *const u8) -> *const u8 {
        if *s == 0 { s } else { s.add(1) }
    }
    fn CharNextW(s: *const u16) -> *const u16 {
        if *s == 0 { s } else { s.add(1) }
    }
    fn CharPrevA(start: *const u8, cur: *const u8) -> *const u8 {
        if cur > start { cur.sub(1) } else { start }
    }
    fn IsCharAlphaW(c: u16) -> BOOL {
        char::from_u32(c as u32).is_some_and(|c| c.is_alphabetic()) as BOOL
    }
    fn IsCharAlphaNumericW(c: u16) -> BOOL {
        char::from_u32(c as u32).is_some_and(|c| c.is_alphanumeric()) as BOOL
    }
    fn IsCharUpperW(c: u16) -> BOOL {
        char::from_u32(c as u32).is_some_and(|c| c.is_uppercase()) as BOOL
    }
    fn IsCharLowerW(c: u16) -> BOOL {
        char::from_u32(c as u32).is_some_and(|c| c.is_lowercase()) as BOOL
    }
    fn OemToCharA(src: LPCSTR, dst: LPSTR) -> BOOL {
        lstrcpy_bytes(dst, src);
        TRUE
    }
    fn CharToOemA(src: LPCSTR, dst: LPSTR) -> BOOL {
        lstrcpy_bytes(dst, src);
        TRUE
    }
    fn LoadStringW(_inst: HMODULE, _id: UINT, _buf: LPWSTR, _max: i32) -> i32 {
        0
    }
    fn wvsprintfA(buf: LPSTR, fmt: LPCSTR, va: *const u64) -> i32 {
        wsprintf_impl(buf, fmt, va)
    }
    fn wvsprintfW(buf: LPWSTR, fmt: LPCWSTR, va: *const u64) -> i32 {
        wsprintfw_impl(buf, fmt, va)
    }
}

unsafe fn lstrcpy_bytes(dst: *mut u8, src: *const u8) {
    let b = astr(src);
    std::ptr::copy_nonoverlapping(b.as_ptr(), dst, b.len());
    *dst.add(b.len()) = 0;
}

crate::shims! { "shlwapi" => SHLWAPI_SHIMS;
    fn PathFileExistsW(p: LPCWSTR) -> BOOL {
        paths::resolve(&w_ptr_to_string(p)).exists() as BOOL
    }
    fn PathFileExistsA(p: LPCSTR) -> BOOL {
        paths::resolve(&a_to_string(p)).exists() as BOOL
    }
    fn PathIsDirectoryW(p: LPCWSTR) -> BOOL {
        paths::resolve(&w_ptr_to_string(p)).is_dir() as BOOL
    }
    fn PathIsRelativeW(p: LPCWSTR) -> BOOL {
        (!paths::is_absolute_win(&w_ptr_to_string(p))) as BOOL
    }
    fn PathFindFileNameW(p: LPCWSTR) -> *const u16 {
        let s = wstr(p);
        match s.iter().rposition(|&c| c == b'\\' as u16 || c == b'/' as u16) {
            Some(i) => p.add(i + 1),
            None => p,
        }
    }
    fn PathFindExtensionW(p: LPCWSTR) -> *const u16 {
        let s = wstr(p);
        let start = s.iter().rposition(|&c| c == b'\\' as u16 || c == b'/' as u16).map(|i| i + 1).unwrap_or(0);
        match s[start..].iter().rposition(|&c| c == b'.' as u16) {
            Some(i) => p.add(start + i),
            None => p.add(s.len()),
        }
    }
    fn PathRemoveFileSpecW(p: LPWSTR) -> BOOL {
        let s = wstr(p);
        match s.iter().rposition(|&c| c == b'\\' as u16 || c == b'/' as u16) {
            Some(i) => {
                *p.add(i) = 0;
                TRUE
            }
            None => FALSE,
        }
    }
    fn PathAppendW(p: LPWSTR, more: LPCWSTR) -> BOOL {
        let mut s = w_ptr_to_string(p);
        if !s.is_empty() && !s.ends_with('\\') {
            s.push('\\');
        }
        s.push_str(w_ptr_to_string(more).trim_start_matches('\\'));
        copy_w(p, MAX_PATH, &to_wide(&s)) as BOOL
    }
    fn PathCombineW(dst: LPWSTR, a: LPCWSTR, b: LPCWSTR) -> LPWSTR {
        let (a, b) = (w_ptr_to_string(a), w_ptr_to_string(b));
        let s = if paths::is_absolute_win(&b) || a.is_empty() {
            b
        } else {
            format!("{}\\{}", a.trim_end_matches('\\'), b.trim_start_matches('\\'))
        };
        copy_w(dst, MAX_PATH, &to_wide(&paths::full_path(&s).trim_end_matches('\\').to_string()));
        dst
    }
    fn PathCanonicalizeW(dst: LPWSTR, src: LPCWSTR) -> BOOL {
        copy_w(dst, MAX_PATH, &to_wide(&paths::full_path(&w_ptr_to_string(src)))) as BOOL
    }
    fn PathStripPathW(p: LPWSTR) {
        let s = wstr(p);
        if let Some(i) = s.iter().rposition(|&c| c == b'\\' as u16 || c == b'/' as u16) {
            let rest: Vec<u16> = s[i + 1..].to_vec();
            std::ptr::copy_nonoverlapping(rest.as_ptr(), p, rest.len());
            *p.add(rest.len()) = 0;
        }
    }
    fn StrToIntW(s: LPCWSTR) -> i32 {
        w_ptr_to_string(s).trim().parse::<i32>().unwrap_or(0)
    }
    fn StrStrIW(h: LPCWSTR, n: LPCWSTR) -> *const u16 {
        let hay = w_ptr_to_string(h).to_lowercase();
        let needle = w_ptr_to_string(n).to_lowercase();
        match hay.find(&needle) {
            Some(i) => h.add(hay[..i].encode_utf16().count()),
            None => std::ptr::null(),
        }
    }
}

crate::shims! { "version" => VERSION_SHIMS;
    fn GetFileVersionInfoSizeW(_f: LPCWSTR, handle: *mut DWORD) -> DWORD {
        if !handle.is_null() {
            *handle = 0;
        }
        set_last_error(1813);
        0
    }
    fn GetFileVersionInfoSizeA(_f: LPCSTR, handle: *mut DWORD) -> DWORD {
        if !handle.is_null() {
            *handle = 0;
        }
        set_last_error(1813);
        0
    }
    fn GetFileVersionInfoW(_f: LPCWSTR, _h: DWORD, _len: DWORD, _data: *mut u8) -> BOOL {
        set_last_error(1813);
        FALSE
    }
    fn VerQueryValueW(_block: *const u8, _sub: LPCWSTR, _buf: *mut usize, _len: *mut UINT) -> BOOL {
        FALSE
    }
}

crate::shims! { "psapi" => PSAPI_SHIMS;
    fn GetModuleFileNameExW(_p: HANDLE, m: HMODULE, buf: LPWSTR, size: DWORD) -> DWORD {
        crate::shim::k32_proc::GetModuleFileNameW(m, buf, size)
    }
    fn EnumProcessModules(_p: HANDLE, mods: *mut HMODULE, cb: DWORD, needed: *mut DWORD) -> BOOL {
        let all = crate::loader::all_modules();
        let max = cb as usize / 8;
        for (i, m) in all.iter().take(max).enumerate() {
            *mods.add(i) = m.base;
        }
        if !needed.is_null() {
            *needed = (all.len() * 8) as DWORD;
        }
        TRUE
    }
    fn GetModuleInformation(_p: HANDLE, m: HMODULE, info: *mut usize, _cb: DWORD) -> BOOL {
        match crate::loader::module_by_base(m) {
            Some(md) => {
                *info = md.base;
                *info.add(1) = md.size;
                *info.add(2) = md.entry;
                TRUE
            }
            None => FALSE,
        }
    }
    fn GetProcessMemoryInfo(_p: HANDLE, counters: *mut u8, cb: DWORD) -> BOOL {
        proc_mem(counters, cb)
    }
    fn K32GetProcessMemoryInfo(p: HANDLE, counters: *mut u8, cb: DWORD) -> BOOL {
        GetProcessMemoryInfo(p, counters, cb)
    }
    fn K32GetModuleFileNameExW(p: HANDLE, m: HMODULE, buf: LPWSTR, size: DWORD) -> DWORD {
        GetModuleFileNameExW(p, m, buf, size)
    }
    fn K32EnumProcessModules(p: HANDLE, mods: *mut HMODULE, cb: DWORD, needed: *mut DWORD) -> BOOL {
        EnumProcessModules(p, mods, cb, needed)
    }
}

crate::shims! { "winmm" => WINMM_SHIMS;
    fn timeGetTime() -> DWORD {
        crate::shim::k32_time::GetTickCount()
    }
    fn timeBeginPeriod(_ms: UINT) -> UINT {
        0
    }
    fn timeEndPeriod(_ms: UINT) -> UINT {
        0
    }
    fn timeGetDevCaps(caps: *mut u32, cb: UINT) -> UINT {
        if cb >= 8 {
            *caps = 1;
            *caps.add(1) = 1_000_000;
        }
        0
    }
    fn PlaySoundW(_s: LPCWSTR, _m: HMODULE, _f: DWORD) -> BOOL {
        TRUE
    }
}

crate::shim_extra!("kernel32" => PSAPI_K32;
    "K32GetProcessMemoryInfo" => GetProcessMemoryInfo,
    "K32EnumProcessModules" => EnumProcessModules,
    "K32GetModuleFileNameExW" => GetModuleFileNameExW,
    "K32GetModuleInformation" => GetModuleInformation,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_splitting() {
        assert_eq!(split_command_line("a b  c"), vec!["a", "b", "c"]);
        assert_eq!(split_command_line("\"a b\" c"), vec!["a b", "c"]);
        assert_eq!(split_command_line("a\\\\\"b c\" d"), vec!["a\\b c", "d"]);
        assert_eq!(split_command_line("\"C:\\Program Files\\x.exe\" /s"), vec!["C:\\Program Files\\x.exe", "/s"]);
        assert_eq!(split_command_line("a\\\"b"), vec!["a\"b"]);
        assert!(split_command_line("   ").is_empty());
    }

    fn split_ext(p: &str) -> (&str, &str) {
        let name_start = p.rfind(['\\', '/']).map(|i| i + 1).unwrap_or(0);
        match p[name_start..].rfind('.') {
            Some(i) => p.split_at(name_start + i),
            None => (p, ""),
        }
    }

    #[test]
    fn extension_split() {
        assert_eq!(split_ext("C:\\a\\b.txt"), ("C:\\a\\b", ".txt"));
        assert_eq!(split_ext("C:\\a.b\\c"), ("C:\\a.b\\c", ""));
    }
}
