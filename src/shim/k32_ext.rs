//! Smaller kernel32, advapi32, dbghelp, netapi32 and crypt32 entry points real programs import without needing much of:
//! most answer "not available" the way an unprivileged, unconfigured Windows would.

use crate::runtime::paths;
use crate::runtime::strings::*;
use crate::shim::{k32_file, k32_sync};
use crate::win::*;

const E_FAIL: i32 = 0x8000_4005u32 as i32;
const ERROR_NO_MORE_FILES: DWORD = 18;
const ERROR_CALL_NOT_IMPL: DWORD = 120;

fn hresult_from_win32(e: DWORD) -> i32 {
    if e == 0 { 0 } else { ((e & 0xffff) | 0x8007_0000) as i32 }
}

crate::shims! { "kernel32" => K32_MORE_SHIMS;
    fn ReOpenFile(h: HANDLE, _access: DWORD, _share: DWORD, _flags: DWORD) -> HANDLE {
        let mut out: HANDLE = 0;
        // same object, new handle: the descriptor behind it is shared
        if k32_sync::DuplicateHandle(CURRENT_PROCESS, h, CURRENT_PROCESS, &mut out, 0, FALSE, 2) == FALSE {
            return INVALID_HANDLE_VALUE;
        }
        out
    }
    fn ReadConsoleInputW(_h: HANDLE, _buf: usize, _len: DWORD, read: *mut DWORD) -> BOOL {
        if !read.is_null() {
            *read = 0;
        }
        set_last_error(ERROR_INVALID_HANDLE);
        FALSE
    }
    fn WriteConsoleInputW(_h: HANDLE, _buf: usize, len: DWORD, written: *mut DWORD) -> BOOL {
        if !written.is_null() {
            *written = len;
        }
        TRUE
    }
    fn GetNamedPipeHandleStateW(_h: HANDLE, state: *mut DWORD, instances: *mut DWORD, max: *mut DWORD, timeout: *mut DWORD, _user: usize, _user_len: DWORD) -> BOOL {
        for p in [state, instances, max, timeout] {
            if !p.is_null() {
                *p = 0;
            }
        }
        TRUE
    }
    fn GetDiskFreeSpaceA(root: LPCSTR, spc: *mut DWORD, bps: *mut DWORD, free: *mut DWORD, total: *mut DWORD) -> BOOL {
        if root.is_null() {
            return crate::shim::k32_pool::GetDiskFreeSpaceW(std::ptr::null(), spc, bps, free, total);
        }
        let w = to_wide_nul(&a_to_string(root));
        crate::shim::k32_pool::GetDiskFreeSpaceW(w.as_ptr(), spc, bps, free, total)
    }
    fn GetTempFileNameA(dir: LPCSTR, prefix: LPCSTR, unique: UINT, buf: LPSTR) -> UINT {
        let d = to_wide_nul(&a_to_string(dir));
        let p = to_wide_nul(&a_to_string(prefix));
        let mut out = [0u16; MAX_PATH];
        let n = k32_file::GetTempFileNameW(d.as_ptr(), p.as_ptr(), unique, out.as_mut_ptr());
        if n != 0 {
            let s = w_to_string(&out[..wlen(out.as_ptr())]);
            copy_a(buf, MAX_PATH, s.as_bytes());
        }
        n
    }
    // the value string comes back as it is: there is no locale-specific grouping to apply
    fn GetNumberFormatEx(_locale: LPCWSTR, _flags: DWORD, value: LPCWSTR, _fmt: usize, buf: LPWSTR, size: i32) -> i32 {
        let s = w_ptr_to_string(value);
        if size == 0 {
            return s.encode_utf16().count() as i32 + 1;
        }
        return_w(buf, size as u32, &s) as i32 + 1
    }
    fn GetCurrencyFormatEx(_locale: LPCWSTR, _flags: DWORD, value: LPCWSTR, _fmt: usize, buf: LPWSTR, size: i32) -> i32 {
        let s = format!("${}", w_ptr_to_string(value));
        if size == 0 {
            return s.encode_utf16().count() as i32 + 1;
        }
        return_w(buf, size as u32, &s) as i32 + 1
    }
    fn GetGeoInfoW(_geo: i32, _ty: u32, _buf: LPWSTR, _size: i32, _lang: u16) -> i32 {
        set_last_error(ERROR_INVALID_PARAMETER);
        0
    }
    fn GetUserGeoID(_class: u32) -> i32 {
        244
    }
    fn ResolveLocaleName(name: LPCWSTR, out: LPWSTR, size: i32) -> i32 {
        let n = if name.is_null() { "en-US".to_string() } else { w_ptr_to_string(name) };
        let n = if n.is_empty() { "en-US".to_string() } else { n };
        return_w(out, size as u32, &n) as i32 + 1
    }
    fn QueryThreadCycleTime(_h: HANDLE, cycles: *mut u64) -> BOOL {
        let mut ts: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts);
        *cycles = ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
        TRUE
    }
    fn RtlCaptureStackBackTrace(_skip: DWORD, _count: DWORD, _frames: usize, hash: *mut DWORD) -> u16 {
        if !hash.is_null() {
            *hash = 0;
        }
        0
    }
    fn GetLogicalDriveStringsW(size: DWORD, buf: LPWSTR) -> DWORD {
        let drives: Vec<u16> = "C:\\\0Z:\\\0".encode_utf16().collect();
        if (size as usize) < drives.len() + 1 {
            return drives.len() as DWORD + 1;
        }
        std::ptr::copy_nonoverlapping(drives.as_ptr(), buf, drives.len());
        *buf.add(drives.len()) = 0;
        drives.len() as DWORD
    }
    fn FindFirstVolumeW(_buf: LPWSTR, _len: DWORD) -> HANDLE {
        set_last_error(ERROR_NO_MORE_FILES);
        INVALID_HANDLE_VALUE
    }
    fn FindNextVolumeW(_h: HANDLE, _buf: LPWSTR, _len: DWORD) -> BOOL {
        set_last_error(ERROR_NO_MORE_FILES);
        FALSE
    }
    fn FindVolumeClose(_h: HANDLE) -> BOOL {
        TRUE
    }
    fn GetVolumePathNamesForVolumeNameW(_name: LPCWSTR, _buf: LPWSTR, _len: DWORD, _ret: *mut DWORD) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    // process snapshots (Python's getppid): not available, which the callers handle
    fn PssCaptureSnapshot(_process: HANDLE, _flags: u32, _ctx: DWORD, _out: *mut usize) -> DWORD {
        ERROR_NOT_SUPPORTED
    }
    fn PssFreeSnapshot(_process: HANDLE, _snapshot: usize) -> DWORD {
        ERROR_NOT_SUPPORTED
    }
    fn PssQuerySnapshot(_snapshot: usize, _class: u32, _buf: usize, _len: DWORD) -> DWORD {
        ERROR_NOT_SUPPORTED
    }
    fn CopyFile2(src: LPCWSTR, dst: LPCWSTR, params: *const u32) -> i32 {
        // COPY_FILE2_EXTENDED_PARAMETERS: size, copy flags (COPY_FILE_FAIL_IF_EXISTS = 1)
        let fail_if_exists = !params.is_null() && *params.add(1) & 1 != 0;
        if k32_file::CopyFileW(src, dst, fail_if_exists as BOOL) != FALSE {
            0
        } else {
            hresult_from_win32(last_error())
        }
    }
    fn PathCchCombineEx(out: LPWSTR, size: usize, a: LPCWSTR, b: LPCWSTR, _flags: DWORD) -> i32 {
        let (a, b) = (w_ptr_to_string(a), w_ptr_to_string(b));
        let joined = if paths::is_absolute_win(&b) || a.is_empty() {
            b
        } else if b.is_empty() {
            a
        } else {
            format!("{}{}{}", a.trim_end_matches('\\'), "\\", b.trim_start_matches('\\'))
        };
        if joined.encode_utf16().count() + 1 > size {
            return hresult_from_win32(ERROR_INSUFFICIENT_BUFFER);
        }
        return_w(out, size as u32, &joined);
        0
    }
    // the end of the root: "C:\", "\\server\share\" or "\"
    fn PathCchSkipRoot(path: LPCWSTR, end: *mut LPCWSTR) -> i32 {
        let w = wstr(path);
        let slash = |c: u16| c == b'\\' as u16 || c == b'/' as u16;
        let mut i;
        if w.len() >= 3 && w[1] == b':' as u16 && slash(w[2]) {
            i = 3;
        } else if w.len() >= 2 && slash(w[0]) && slash(w[1]) {
            let mut parts = 0;
            i = 2;
            while i < w.len() && parts < 2 {
                if slash(w[i]) {
                    parts += 1;
                }
                i += 1;
            }
        } else if !w.is_empty() && slash(w[0]) {
            i = 1;
        } else {
            return E_FAIL;
        }
        *end = path.add(i);
        0
    }
    fn EnumResourceLanguagesA(_module: HMODULE, _ty: LPCSTR, _name: LPCSTR, _cb: usize, _param: isize) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn EnumSystemLocalesA(_cb: usize, _flags: DWORD) -> BOOL {
        TRUE
    }
    fn GetConsoleTitleA(_buf: LPSTR, _size: DWORD) -> DWORD {
        0
    }
    fn DebugBreakProcess(_h: HANDLE) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
}

crate::shims! { "user32" => USER32_MORE_SHIMS;
    fn GetProcessWindowStation() -> HANDLE {
        0
    }
    fn GetUserObjectInformationW(_h: HANDLE, _index: i32, _info: usize, _len: DWORD, _needed: *mut DWORD) -> BOOL {
        set_last_error(ERROR_INVALID_HANDLE);
        FALSE
    }
}

crate::shims! { "advapi32" => ADVAPI32_MORE_SHIMS;
    fn GetSecurityInfo(_h: HANDLE, _ty: u32, _info: u32, _owner: usize, _group: usize, _dacl: usize, _sacl: usize, _sd: usize) -> DWORD {
        ERROR_NOT_SUPPORTED
    }
    fn SetSecurityInfo(_h: HANDLE, _ty: u32, _info: u32, _owner: usize, _group: usize, _dacl: usize, _sacl: usize) -> DWORD {
        ERROR_NOT_SUPPORTED
    }
    fn SetEntriesInAclA(_count: u32, _entries: usize, _old: usize, _new: *mut usize) -> DWORD {
        ERROR_NOT_SUPPORTED
    }
    fn SetEntriesInAclW(_count: u32, _entries: usize, _old: usize, _new: *mut usize) -> DWORD {
        ERROR_NOT_SUPPORTED
    }
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(_s: LPCWSTR, _rev: DWORD, _sd: *mut usize, _size: *mut DWORD) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn LookupPrivilegeValueA(_sys: LPCSTR, _name: LPCSTR, luid: *mut u64) -> BOOL {
        *luid = 1;
        TRUE
    }
    fn RegConnectRegistryW(_machine: LPCWSTR, _key: usize, _out: *mut usize) -> LONG {
        ERROR_CALL_NOT_IMPL as LONG
    }
    fn RegLoadKeyW(_key: usize, _sub: LPCWSTR, _file: LPCWSTR) -> LONG {
        ERROR_CALL_NOT_IMPL as LONG
    }
    fn RegSaveKeyW(_key: usize, _file: LPCWSTR, _sa: usize) -> LONG {
        ERROR_CALL_NOT_IMPL as LONG
    }
    // CryptoAPI pieces beyond hashing: nothing is available
    fn CryptDecrypt(_k: usize, _h: usize, _fin: BOOL, _flags: DWORD, _data: usize, _len: *mut DWORD) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn CryptDestroyKey(_k: usize) -> BOOL {
        TRUE
    }
    fn CryptEnumProvidersW(_idx: DWORD, _res: usize, _flags: DWORD, _ty: *mut DWORD, _name: LPWSTR, _len: *mut DWORD) -> BOOL {
        set_last_error(259);
        FALSE
    }
    fn CryptExportKey(_k: usize, _x: usize, _blob: DWORD, _flags: DWORD, _data: usize, _len: *mut DWORD) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn CryptGetProvParam(_p: usize, _param: DWORD, _data: usize, _len: *mut DWORD, _flags: DWORD) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn CryptGetUserKey(_p: usize, _spec: DWORD, _key: *mut usize) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn CryptSetHashParam(_h: usize, _param: DWORD, _data: usize, _flags: DWORD) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn CryptSignHashW(_h: usize, _spec: DWORD, _desc: LPCWSTR, _flags: DWORD, _sig: usize, _len: *mut DWORD) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
}

crate::shims! { "crypt32" => CRYPT32_MORE_SHIMS;
    fn CertEnumCertificatesInStore(_store: usize, _prev: usize) -> usize {
        0
    }
    fn CertDuplicateCertificateContext(ctx: usize) -> usize {
        ctx
    }
}

// a process with no symbols and no way to write a dump: the calls say so
crate::shims! { "dbghelp" => DBGHELP_MORE_SHIMS;
    fn SymInitialize(_process: HANDLE, _path: LPCSTR, _invade: BOOL) -> BOOL {
        FALSE
    }
    fn SymCleanup(_process: HANDLE) -> BOOL {
        TRUE
    }
    fn SymGetOptions() -> DWORD {
        0
    }
    fn SymSetOptions(options: DWORD) -> DWORD {
        options
    }
    fn SymFromAddr(_process: HANDLE, _addr: u64, _disp: *mut u64, _sym: usize) -> BOOL {
        FALSE
    }
    fn SymGetLineFromAddr64(_process: HANDLE, _addr: u64, _disp: *mut DWORD, _line: usize) -> BOOL {
        FALSE
    }
    fn SymGetModuleBase64(_process: HANDLE, _addr: u64) -> u64 {
        0
    }
    fn SymFunctionTableAccess64(_process: HANDLE, _addr: u64) -> usize {
        0
    }
    fn SymGetSearchPathW(_process: HANDLE, _buf: LPWSTR, _len: DWORD) -> BOOL {
        FALSE
    }
    fn SymSetSearchPathW(_process: HANDLE, _path: LPCWSTR) -> BOOL {
        FALSE
    }
    fn StackWalk64(_machine: DWORD, _process: HANDLE, _thread: HANDLE, _frame: usize, _ctx: usize, _read: usize, _access: usize, _base: usize, _translate: usize) -> BOOL {
        FALSE
    }
    fn UnDecorateSymbolName(_name: LPCSTR, _out: LPSTR, _len: DWORD, _flags: DWORD) -> DWORD {
        0
    }
    fn MiniDumpWriteDump(_process: HANDLE, _pid: DWORD, _file: HANDLE, _ty: u32, _ex: usize, _user: usize, _cb: usize) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
}

crate::shims! { "netapi32" => NETAPI32_SHIMS;
    fn NetApiBufferFree(buf: usize) -> DWORD {
        if buf != 0 {
            libc::free(buf as *mut libc::c_void);
        }
        0
    }
    fn NetUserEnum(_server: LPCWSTR, _level: DWORD, _filter: DWORD, _buf: *mut usize, _max: DWORD, _read: *mut DWORD, _total: *mut DWORD, _resume: *mut DWORD) -> DWORD {
        ERROR_ACCESS_DENIED
    }
}
