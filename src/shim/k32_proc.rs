//! kernel32: process, module, environment and system information.

use crate::loader;
use crate::runtime::handles::{self, Kind};
use crate::runtime::{self, paths, strings, teb};
use crate::win::*;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

fn cmdline() -> &'static (Vec<u16>, Vec<u8>) {
    static C: OnceLock<(Vec<u16>, Vec<u8>)> = OnceLock::new();
    C.get_or_init(|| {
        let s = runtime::command_line();
        let mut a = s.clone().into_bytes();
        a.push(0);
        (strings::to_wide_nul(&s), a)
    })
}

fn system32() -> &'static str {
    "C:\\windows\\system32"
}

fn env_block_w() -> Vec<u16> {
    let mut b: Vec<u16> = Vec::new();
    for (k, v) in runtime::ENV.lock().unwrap().iter() {
        b.extend(format!("{k}={v}").encode_utf16());
        b.push(0);
    }
    b.push(0);
    if b.len() == 1 {
        b.push(0);
    }
    b
}

unsafe fn alloc_copy<T: Copy>(data: &[T]) -> *mut T {
    let p = libc::malloc(std::mem::size_of_val(data).max(1)) as *mut T;
    if !p.is_null() {
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
    }
    p
}

pub(crate) fn expand_env(src: &str) -> String {
    let mut out = String::new();
    let mut rest = src;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let name = &after[..j];
                match runtime::env_get(name) {
                    Some(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('%');
                rest = after;
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

const ERROR_BAD_LENGTH: DWORD = 24;

fn new_job() -> HANDLE {
    handles::insert(Kind::Job(handles::JobObj { limits: Mutex::new([0; 144]), members: Mutex::new(Vec::new()) }))
}

fn job_of(h: HANDLE) -> Option<Arc<Kind>> {
    let k = handles::get(h).filter(|k| matches!(&**k, Kind::Job(_)));
    if k.is_none() {
        set_last_error(ERROR_INVALID_HANDLE);
    }
    k
}

fn job_ref(k: &Kind) -> &handles::JobObj {
    match k {
        Kind::Job(j) => j,
        _ => unreachable!("job_of only returns jobs"),
    }
}

/// Size of the information classes programs use: basic and extended limits, UI restrictions, end-of-job time.
fn job_info_size(class: u32) -> Option<usize> {
    match class {
        2 => Some(64),
        4 | 6 => Some(4),
        9 => Some(144),
        _ => None,
    }
}

fn module_path(h: usize) -> Option<String> {
    if h == 0 {
        return Some(runtime::cfg().exe_win.clone());
    }
    if let Some(m) = loader::module_by_base(h) {
        return Some(m.win_path());
    }
    loader::fake_module_name(h).map(|n| format!("{}\\{}", system32(), n))
}

unsafe fn load_library(name: &str) -> HMODULE {
    let h = load_library_inner(name);
    if runtime::cfg().trace.api {
        eprintln!("nle[api]: LoadLibrary({name}) -> {}", if h == 0 { "NULL".to_string() } else { format!("0x{h:x}") });
    }
    h
}

unsafe fn load_library_inner(name: &str) -> HMODULE {
    if name.is_empty() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    if let Some(m) = loader::module_by_name(name) {
        return m.base;
    }
    if crate::registry::is_system_dll(&name.rsplit(['\\', '/']).next().map(|n| if n.contains('.') { n.to_string() } else { format!("{n}.dll") }).unwrap_or_default()) {
        if let Some(h) = loader::fake_module(name) {
            return h;
        }
    }
    match loader::find_dll(name) {
        Some(p) => match loader::load_library(&p) {
            Ok(m) => m.base,
            Err(e) => {
                crate::trace_load!("LoadLibrary({name}) failed: {e}");
                set_last_error(match e {
                    loader::LoadError::Pe(_) | loader::LoadError::Unsupported(_) => ERROR_BAD_EXE_FORMAT,
                    _ => ERROR_MOD_NOT_FOUND,
                });
                0
            }
        },
        None => {
            set_last_error(ERROR_MOD_NOT_FOUND);
            0
        }
    }
}

unsafe fn fill_version(v: *mut OSVERSIONINFOEXW) -> BOOL {
    let size = (*v).size;
    if (size as usize) < 20 {
        set_last_error(ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    (*v).major = 10;
    (*v).minor = 0;
    (*v).build = 19045;
    (*v).platform = 2;
    if size as usize >= std::mem::size_of::<OSVERSIONINFOW>() {
        (*v).csd = [0; 128];
    }
    if size as usize >= std::mem::size_of::<OSVERSIONINFOEXW>() {
        (*v).sp_major = 0;
        (*v).sp_minor = 0;
        (*v).suite_mask = 0;
        (*v).product_type = 1;
    }
    TRUE
}

unsafe fn fill_sysinfo(si: *mut SYSTEM_INFO) {
    let n = runtime::cpu_count();
    (*si).processor_architecture = 9;
    (*si).reserved = 0;
    (*si).page_size = 4096;
    (*si).min_app_address = 0x10000;
    (*si).max_app_address = 0x7fff_ffff_efff;
    (*si).active_processor_mask = if n >= 64 { usize::MAX } else { (1usize << n) - 1 };
    (*si).number_of_processors = n as DWORD;
    (*si).processor_type = 8664;
    (*si).allocation_granularity = 0x10000;
    (*si).processor_level = 6;
    (*si).processor_revision = 0x5507;
}

fn host_meminfo() -> (u64, u64) {
    let mut si: libc::sysinfo = unsafe { std::mem::zeroed() };
    unsafe { libc::sysinfo(&mut si) };
    let unit = si.mem_unit.max(1) as u64;
    let total = si.totalram as u64 * unit;
    let avail = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("MemAvailable:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
        .map(|kb| kb * 1024)
        .unwrap_or(si.freeram as u64 * unit);
    (total, avail)
}

pub fn temp_dir_win() -> String {
    let t = runtime::env_get("TEMP").unwrap_or_else(|| "C:\\windows\\temp".to_string());
    if t.ends_with('\\') {
        t
    } else {
        format!("{t}\\")
    }
}

/// `VerifyVersionInfo` against Windows 10 (10.0.19045, no service pack). Major, minor and service pack are compared
/// the way Windows does: the first of them that differs decides, the later ones only matter when the earlier are equal.
unsafe fn verify_version(v: *mut u8, mask: DWORD, cond: u64, wide: bool) -> BOOL {
    const ERROR_OLD_WIN_VERSION: DWORD = 1150;
    let get32 = |off: usize| *(v.add(off) as *const u32);
    let get16 = |off: usize| *(v.add(off) as *const u16) as u32;
    let sp = if wide { 20 + 256 } else { 20 + 128 };
    let op = |idx: u32| ((cond >> (3 * idx)) & 7) as u32;
    let fail = || {
        set_last_error(ERROR_OLD_WIN_VERSION);
        FALSE
    };
    // (mask bit, condition index, wanted, have)
    let ordered: [(u32, u32, u32, u32); 4] = [(2, 1, get32(4), 10), (1, 0, get32(8), 0), (0x20, 5, get16(sp), 0), (0x10, 4, get16(sp + 2), 0)];
    let mut decided = false;
    let mut last_op = 3;
    for (bit, idx, wanted, have) in ordered {
        if mask & bit == 0 {
            continue;
        }
        last_op = op(idx);
        if have != wanted {
            let ok = match last_op {
                2 | 3 => have > wanted,
                4 | 5 => have < wanted,
                _ => false,
            };
            if !ok {
                return fail();
            }
            decided = true;
            break;
        }
    }
    // everything compared equal: only conditions that allow equality hold
    if !decided && ordered.iter().any(|f| mask & f.0 != 0) && !matches!(last_op, 1 | 3 | 5) {
        return fail();
    }
    let independent: [(u32, u32, u32, u32); 2] = [(4, 2, get32(12), 19045), (8, 3, get32(16), 2)];
    for (bit, idx, wanted, have) in independent {
        if mask & bit == 0 {
            continue;
        }
        let ok = match op(idx) {
            2 => have > wanted,
            3 => have >= wanted,
            4 => have < wanted,
            5 => have <= wanted,
            _ => have == wanted,
        };
        if !ok {
            return fail();
        }
    }
    TRUE
}

crate::shims! { "kernel32";
    fn GetLastError() -> DWORD {
        last_error()
    }
    fn SetLastError(e: DWORD) {
        set_last_error(e);
    }
    fn SetLastErrorEx(e: DWORD, _ty: DWORD) {
        set_last_error(e);
    }
    fn SetErrorMode(_mode: UINT) -> UINT {
        0
    }
    fn GetErrorMode() -> UINT {
        0
    }

    fn GetCurrentProcess() -> HANDLE {
        CURRENT_PROCESS
    }

    fn CreateJobObjectW(_attrs: usize, _name: LPCWSTR) -> HANDLE {
        new_job()
    }
    fn CreateJobObjectA(_attrs: usize, _name: LPCSTR) -> HANDLE {
        new_job()
    }
    fn SetInformationJobObject(job: HANDLE, class: u32, info: *const u8, len: u32) -> BOOL {
        let Some(k) = job_of(job) else { return FALSE };
        let j = job_ref(&k);
        let Some(size) = job_info_size(class) else {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        };
        if info.is_null() || len < size as u32 {
            set_last_error(ERROR_BAD_LENGTH);
            return FALSE;
        }
        // The basic and extended limit structures share their first 64 bytes, so one buffer serves both.
        if class == 2 || class == 9 {
            std::ptr::copy_nonoverlapping(info, j.limits.lock().unwrap().as_mut_ptr(), size);
        }
        TRUE
    }
    fn QueryInformationJobObject(job: HANDLE, class: u32, info: *mut u8, len: u32, returned: *mut u32) -> BOOL {
        let Some(k) = job_of(job) else { return FALSE };
        let j = job_ref(&k);
        let Some(size) = job_info_size(class) else {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        };
        if info.is_null() || len < size as u32 {
            set_last_error(ERROR_BAD_LENGTH);
            return FALSE;
        }
        let out = std::slice::from_raw_parts_mut(info, size);
        out.fill(0);
        if class == 2 || class == 9 {
            out.copy_from_slice(&j.limits.lock().unwrap()[..size]);
        }
        if !returned.is_null() {
            *returned = size as u32;
        }
        TRUE
    }
    fn AssignProcessToJobObject(job: HANDLE, process: HANDLE) -> BOOL {
        let Some(k) = job_of(job) else { return FALSE };
        let j = job_ref(&k);
        if process == CURRENT_PROCESS {
            return TRUE;
        }
        match handles::get(process) {
            Some(p) if matches!(&*p, Kind::Process(_)) => {
                j.members.lock().unwrap().push(p);
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }
    fn TerminateJobObject(job: HANDLE, _exit_code: u32) -> BOOL {
        let Some(k) = job_of(job) else { return FALSE };
        let j = job_ref(&k);
        j.kill_members();
        TRUE
    }
    fn IsProcessInJob(process: HANDLE, job: HANDLE, result: *mut BOOL) -> BOOL {
        let member = |j: &handles::JobObj| {
            process != CURRENT_PROCESS
                && handles::get(process).is_some_and(|p| j.members.lock().unwrap().iter().any(|m| Arc::ptr_eq(m, &p)))
        };
        let inside = if job == 0 {
            false
        } else {
            match job_of(job) {
                Some(k) => member(job_ref(&k)),
                None => return FALSE,
            }
        };
        *result = inside as BOOL;
        TRUE
    }
    fn GetCurrentProcessId() -> DWORD {
        libc::getpid() as DWORD
    }
    fn GetProcessId(h: HANDLE) -> DWORD {
        if h == CURRENT_PROCESS {
            return libc::getpid() as DWORD;
        }
        match handles::get(h).as_deref() {
            Some(Kind::Process(p)) => p.pid.load(Ordering::SeqCst),
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                0
            }
        }
    }
    fn OpenProcess(_access: DWORD, _inherit: BOOL, pid: DWORD) -> HANDLE {
        if pid == libc::getpid() as DWORD {
            return CURRENT_PROCESS;
        }
        set_last_error(ERROR_INVALID_PARAMETER);
        0
    }
    fn GetExitCodeProcess(h: HANDLE, code: *mut DWORD) -> BOOL {
        if h == CURRENT_PROCESS {
            if !code.is_null() {
                *code = 259;
            }
            return TRUE;
        }
        match handles::get(h).as_deref() {
            Some(Kind::Process(p)) => {
                if !code.is_null() {
                    *code = p.exit_code.load(Ordering::SeqCst);
                }
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }
    fn ExitProcess(code: UINT) -> ! {
        loader::exit_process(code)
    }
    fn TerminateProcess(h: HANDLE, code: UINT) -> BOOL {
        if h == CURRENT_PROCESS {
            loader::exit_process(code);
        }
        match handles::get(h).as_deref() {
            Some(Kind::Process(p)) => {
                let pid = p.pid.load(Ordering::SeqCst) as i32;
                if pid > 0 {
                    libc::kill(pid, libc::SIGKILL);
                }
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }
    fn FatalExit(code: i32) -> ! {
        loader::exit_process(code as u32)
    }
    fn FatalAppExitA(_action: UINT, msg: LPCSTR) -> ! {
        eprintln!("{}", strings::a_to_string(msg));
        loader::exit_process(1)
    }
    fn FatalAppExitW(_action: UINT, msg: LPCWSTR) -> ! {
        eprintln!("{}", strings::w_ptr_to_string(msg));
        loader::exit_process(1)
    }
    fn SetPriorityClass(_h: HANDLE, _class: DWORD) -> BOOL {
        TRUE
    }
    fn GetPriorityClass(_h: HANDLE) -> DWORD {
        0x20
    }
    fn SetProcessPriorityBoost(_h: HANDLE, _disable: BOOL) -> BOOL {
        TRUE
    }
    fn SetProcessWorkingSetSize(_h: HANDLE, _min: usize, _max: usize) -> BOOL {
        TRUE
    }
    fn SetProcessShutdownParameters(_level: DWORD, _flags: DWORD) -> BOOL {
        TRUE
    }
    fn GetProcessHandleCount(_h: HANDLE, count: *mut DWORD) -> BOOL {
        if !count.is_null() {
            *count = 64;
        }
        TRUE
    }
    fn GetProcessAffinityMask(_h: HANDLE, proc_mask: *mut usize, sys_mask: *mut usize) -> BOOL {
        let n = runtime::cpu_count();
        let m = if n >= 64 { usize::MAX } else { (1usize << n) - 1 };
        if !proc_mask.is_null() {
            *proc_mask = m;
        }
        if !sys_mask.is_null() {
            *sys_mask = m;
        }
        TRUE
    }
    fn SetProcessAffinityMask(_h: HANDLE, _mask: usize) -> BOOL {
        TRUE
    }
    fn GetProcessTimes(_h: HANDLE, creation: *mut FILETIME, exit: *mut FILETIME, kernel: *mut FILETIME, user: *mut FILETIME) -> BOOL {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        let ft = |t: libc::timeval| FILETIME::from_u64(t.tv_sec as u64 * 10_000_000 + t.tv_usec as u64 * 10);
        if !creation.is_null() {
            *creation = FILETIME::default();
        }
        if !exit.is_null() {
            *exit = FILETIME::default();
        }
        if !kernel.is_null() {
            *kernel = ft(ru.ru_stime);
        }
        if !user.is_null() {
            *user = ft(ru.ru_utime);
        }
        TRUE
    }
    fn QueryFullProcessImageNameW(_h: HANDLE, _flags: DWORD, buf: LPWSTR, size: *mut DWORD) -> BOOL {
        let p = strings::to_wide(&runtime::cfg().exe_win);
        if p.len() + 1 > *size as usize {
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            return FALSE;
        }
        strings::copy_w(buf, *size as usize, &p);
        *size = p.len() as DWORD;
        TRUE
    }
    fn ProcessIdToSessionId(_pid: DWORD, sid: *mut DWORD) -> BOOL {
        if !sid.is_null() {
            *sid = 1;
        }
        TRUE
    }
    fn WTSGetActiveConsoleSessionId() -> DWORD {
        1
    }
    fn GetNumaHighestNodeNumber(n: *mut ULONG) -> BOOL {
        if !n.is_null() {
            *n = 0;
        }
        TRUE
    }

    fn GetCommandLineW() -> *const u16 {
        cmdline().0.as_ptr()
    }
    fn GetCommandLineA() -> *const u8 {
        cmdline().1.as_ptr()
    }
    fn GetStartupInfoW(si: *mut STARTUPINFOW) {
        std::ptr::write_bytes(si as *mut u8, 0, std::mem::size_of::<STARTUPINFOW>());
        (*si).cb = std::mem::size_of::<STARTUPINFOW>() as DWORD;
    }
    fn GetStartupInfoA(si: *mut STARTUPINFOA) {
        std::ptr::write_bytes(si as *mut u8, 0, std::mem::size_of::<STARTUPINFOA>());
        (*si).cb = std::mem::size_of::<STARTUPINFOA>() as DWORD;
    }

    fn GetEnvironmentVariableW(name: LPCWSTR, buf: LPWSTR, size: DWORD) -> DWORD {
        match runtime::env_get(&strings::w_ptr_to_string(name)) {
            Some(v) => strings::return_w(buf, size, &v),
            None => {
                set_last_error(ERROR_ENVVAR_NOT_FOUND);
                0
            }
        }
    }
    fn GetEnvironmentVariableA(name: LPCSTR, buf: LPSTR, size: DWORD) -> DWORD {
        match runtime::env_get(&strings::a_to_string(name)) {
            Some(v) => strings::return_a(buf, size, &v),
            None => {
                set_last_error(ERROR_ENVVAR_NOT_FOUND);
                0
            }
        }
    }
    fn SetEnvironmentVariableW(name: LPCWSTR, value: LPCWSTR) -> BOOL {
        let n = strings::w_ptr_to_string(name);
        if n.is_empty() || n.contains('=') {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        let v = if value.is_null() { None } else { Some(strings::w_ptr_to_string(value)) };
        runtime::env_set(&n, v.as_deref());
        TRUE
    }
    fn SetEnvironmentVariableA(name: LPCSTR, value: LPCSTR) -> BOOL {
        let n = strings::a_to_string(name);
        if n.is_empty() || n.contains('=') {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        let v = if value.is_null() { None } else { Some(strings::a_to_string(value)) };
        runtime::env_set(&n, v.as_deref());
        TRUE
    }
    fn GetEnvironmentStringsW() -> *mut u16 {
        alloc_copy(&env_block_w())
    }
    fn GetEnvironmentStrings() -> *mut u8 {
        let mut b: Vec<u8> = Vec::new();
        for (k, v) in runtime::ENV.lock().unwrap().iter() {
            b.extend(format!("{k}={v}").bytes());
            b.push(0);
        }
        b.push(0);
        alloc_copy(&b)
    }
    fn GetEnvironmentStringsA() -> *mut u8 {
        GetEnvironmentStrings()
    }
    fn FreeEnvironmentStringsW(p: *mut u16) -> BOOL {
        libc::free(p as *mut libc::c_void);
        TRUE
    }
    fn FreeEnvironmentStringsA(p: *mut u8) -> BOOL {
        libc::free(p as *mut libc::c_void);
        TRUE
    }
    fn ExpandEnvironmentStringsW(src: LPCWSTR, dst: LPWSTR, size: DWORD) -> DWORD {
        let e = strings::to_wide(&expand_env(&strings::w_ptr_to_string(src)));
        if strings::copy_w(dst, size as usize, &e) {
            e.len() as DWORD + 1
        } else {
            e.len() as DWORD + 1
        }
    }
    fn ExpandEnvironmentStringsA(src: LPCSTR, dst: LPSTR, size: DWORD) -> DWORD {
        let e = expand_env(&strings::a_to_string(src));
        strings::copy_a(dst, size as usize, e.as_bytes());
        e.len() as DWORD + 1
    }

    fn GetModuleHandleW(name: LPCWSTR) -> HMODULE {
        let n = if name.is_null() { None } else { Some(strings::w_ptr_to_string(name)) };
        match loader::get_module_handle(n.as_deref()) {
            Some(h) => h,
            None => {
                set_last_error(ERROR_MOD_NOT_FOUND);
                0
            }
        }
    }
    fn GetModuleHandleA(name: LPCSTR) -> HMODULE {
        let n = if name.is_null() { None } else { Some(strings::a_to_string(name)) };
        match loader::get_module_handle(n.as_deref()) {
            Some(h) => h,
            None => {
                set_last_error(ERROR_MOD_NOT_FOUND);
                0
            }
        }
    }
    fn GetModuleHandleExW(flags: DWORD, name: LPCWSTR, out: *mut HMODULE) -> BOOL {
        let h = if flags & 4 != 0 {
            loader::module_from_addr(name as usize).map(|m| m.base)
        } else if name.is_null() {
            loader::get_module_handle(None)
        } else {
            loader::get_module_handle(Some(&strings::w_ptr_to_string(name)))
        };
        match h {
            Some(h) => {
                if !out.is_null() {
                    *out = h;
                }
                TRUE
            }
            None => {
                set_last_error(ERROR_MOD_NOT_FOUND);
                FALSE
            }
        }
    }
    fn GetModuleHandleExA(flags: DWORD, name: LPCSTR, out: *mut HMODULE) -> BOOL {
        let h = if flags & 4 != 0 {
            loader::module_from_addr(name as usize).map(|m| m.base)
        } else if name.is_null() {
            loader::get_module_handle(None)
        } else {
            loader::get_module_handle(Some(&strings::a_to_string(name)))
        };
        match h {
            Some(h) => {
                if !out.is_null() {
                    *out = h;
                }
                TRUE
            }
            None => {
                set_last_error(ERROR_MOD_NOT_FOUND);
                FALSE
            }
        }
    }
    fn GetModuleFileNameW(h: HMODULE, buf: LPWSTR, size: DWORD) -> DWORD {
        let Some(p) = module_path(h) else {
            set_last_error(ERROR_MOD_NOT_FOUND);
            return 0;
        };
        let w = strings::to_wide(&p);
        let n = w.len().min((size as usize).saturating_sub(1));
        if size == 0 {
            return 0;
        }
        std::ptr::copy_nonoverlapping(w.as_ptr(), buf, n);
        *buf.add(n) = 0;
        if n < w.len() {
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
        }
        n as DWORD
    }
    fn GetModuleFileNameA(h: HMODULE, buf: LPSTR, size: DWORD) -> DWORD {
        let Some(p) = module_path(h) else {
            set_last_error(ERROR_MOD_NOT_FOUND);
            return 0;
        };
        let b = p.as_bytes();
        let n = b.len().min((size as usize).saturating_sub(1));
        if size == 0 {
            return 0;
        }
        std::ptr::copy_nonoverlapping(b.as_ptr(), buf, n);
        *buf.add(n) = 0;
        if n < b.len() {
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
        }
        n as DWORD
    }
    fn LoadLibraryW(name: LPCWSTR) -> HMODULE {
        load_library(&strings::w_ptr_to_string(name))
    }
    fn LoadLibraryA(name: LPCSTR) -> HMODULE {
        load_library(&strings::a_to_string(name))
    }
    fn LoadLibraryExW(name: LPCWSTR, _file: HANDLE, _flags: DWORD) -> HMODULE {
        load_library(&strings::w_ptr_to_string(name))
    }
    fn LoadLibraryExA(name: LPCSTR, _file: HANDLE, _flags: DWORD) -> HMODULE {
        load_library(&strings::a_to_string(name))
    }
    fn FreeLibrary(h: HMODULE) -> BOOL {
        if let Some(m) = loader::module_by_base(h) {
            loader::free_library(&m);
            return TRUE;
        }
        if loader::fake_module_name(h).is_some() {
            return TRUE;
        }
        set_last_error(ERROR_INVALID_HANDLE);
        FALSE
    }
    fn FreeLibraryAndExitThread(h: HMODULE, code: DWORD) -> ! {
        FreeLibrary(h);
        crate::shim::k32_thread::ExitThread(code)
    }
    fn DisableThreadLibraryCalls(h: HMODULE) -> BOOL {
        match loader::module_by_base(h) {
            Some(m) => {
                m.no_thread_calls.store(true, Ordering::Relaxed);
                TRUE
            }
            None => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }
    fn GetProcAddress(h: HMODULE, name: LPCSTR) -> usize {
        let (r, label) = if (name as usize) < 0x10000 {
            (loader::get_proc_address_ordinal(h, name as usize as u32), format!("#{}", name as usize))
        } else {
            let n = strings::a_to_string(name);
            (loader::get_proc_address(h, &n), n)
        };
        if runtime::cfg().trace.api {
            let m = loader::fake_module_name(h).or_else(|| loader::module_by_base(h).map(|m| m.name.clone())).unwrap_or_else(|| format!("0x{h:x}"));
            eprintln!("nle[api]: GetProcAddress({m}, {label}) -> {}", r.map_or("NULL".to_string(), |a| format!("0x{a:x}")));
        }
        match r {
            Some(a) => a,
            None => {
                set_last_error(ERROR_PROC_NOT_FOUND);
                0
            }
        }
    }
    fn SetDllDirectoryW(p: LPCWSTR) -> BOOL {
        if !p.is_null() && *p != 0 {
            loader::add_dll_directory(paths::resolve(&strings::w_ptr_to_string(p)));
        }
        TRUE
    }
    fn SetDllDirectoryA(p: LPCSTR) -> BOOL {
        if !p.is_null() && *p != 0 {
            loader::add_dll_directory(paths::resolve(&strings::a_to_string(p)));
        }
        TRUE
    }
    fn SetDefaultDllDirectories(_flags: DWORD) -> BOOL {
        TRUE
    }
    fn AddDllDirectory(p: LPCWSTR) -> usize {
        loader::add_dll_directory(paths::resolve(&strings::w_ptr_to_string(p)))
    }
    fn RemoveDllDirectory(c: usize) -> BOOL {
        loader::remove_dll_directory(c);
        TRUE
    }

    fn GetSystemInfo(si: *mut SYSTEM_INFO) {
        fill_sysinfo(si);
    }
    fn GetNativeSystemInfo(si: *mut SYSTEM_INFO) {
        fill_sysinfo(si);
    }
    fn GetVersion() -> DWORD {
        0x4a65_000a
    }
    fn GetVersionExW(v: *mut OSVERSIONINFOEXW) -> BOOL {
        fill_version(v)
    }
    fn GetVersionExA(v: *mut OSVERSIONINFOEXW) -> BOOL {
        fill_version(v)
    }
    // compares the requested fields with Windows 10 (10.0.19045, no service pack); a field with no condition must be equal
    fn VerifyVersionInfoW(v: *mut u8, mask: DWORD, cond: u64) -> BOOL {
        verify_version(v, mask, cond, true)
    }
    fn VerifyVersionInfoA(v: *mut u8, mask: DWORD, cond: u64) -> BOOL {
        verify_version(v, mask, cond, false)
    }
    // each field type has three bits of the condition mask, indexed by the bit it has in the type mask
    fn VerSetConditionMask(mask: u64, ty: DWORD, cond: u8) -> u64 {
        let mut mask = mask;
        for i in 0..8 {
            if ty & (1 << i) != 0 {
                mask = (mask & !(7u64 << (3 * i))) | ((cond as u64 & 7) << (3 * i));
            }
        }
        mask
    }
    fn IsWow64Process(_h: HANDLE, out: *mut BOOL) -> BOOL {
        if !out.is_null() {
            *out = FALSE;
        }
        TRUE
    }
    fn IsWow64Process2(_h: HANDLE, process: *mut u16, native: *mut u16) -> BOOL {
        if !process.is_null() {
            *process = 0;
        }
        if !native.is_null() {
            *native = 0x8664;
        }
        TRUE
    }
    fn IsProcessorFeaturePresent(f: DWORD) -> BOOL {
        let t = teb::processor_features();
        (f < 64 && t[f as usize]) as BOOL
    }
    fn GetActiveProcessorCount(_group: WORD) -> DWORD {
        runtime::cpu_count() as DWORD
    }
    fn GetMaximumProcessorCount(_group: WORD) -> DWORD {
        runtime::cpu_count() as DWORD
    }
    fn GetActiveProcessorGroupCount() -> WORD {
        1
    }
    fn GetMaximumProcessorGroupCount() -> WORD {
        1
    }
    fn GlobalMemoryStatusEx(m: *mut MEMORYSTATUSEX) -> BOOL {
        let (total, avail) = host_meminfo();
        (*m).memory_load = (100 - avail * 100 / total.max(1)) as DWORD;
        (*m).total_phys = total;
        (*m).avail_phys = avail;
        (*m).total_page_file = total;
        (*m).avail_page_file = avail;
        (*m).total_virtual = 0x7fff_ffff_0000;
        (*m).avail_virtual = 0x7fff_ffff_0000 - (1 << 30);
        (*m).avail_extended_virtual = 0;
        TRUE
    }
    fn GetPhysicallyInstalledSystemMemory(kb: *mut u64) -> BOOL {
        *kb = host_meminfo().0 / 1024;
        TRUE
    }
    fn GetProductInfo(_a: DWORD, _b: DWORD, _c: DWORD, _d: DWORD, out: *mut DWORD) -> BOOL {
        *out = 0x30;
        TRUE
    }

    fn GetComputerNameW(buf: LPWSTR, size: *mut DWORD) -> BOOL {
        let n = runtime::host_name().to_ascii_uppercase();
        let w = strings::to_wide(&n);
        if strings::copy_w(buf, *size as usize, &w) {
            *size = w.len() as DWORD;
            TRUE
        } else {
            *size = w.len() as DWORD + 1;
            set_last_error(ERROR_BUFFER_OVERFLOW);
            FALSE
        }
    }
    fn GetComputerNameA(buf: LPSTR, size: *mut DWORD) -> BOOL {
        let n = runtime::host_name().to_ascii_uppercase();
        if strings::copy_a(buf, *size as usize, n.as_bytes()) {
            *size = n.len() as DWORD;
            TRUE
        } else {
            *size = n.len() as DWORD + 1;
            set_last_error(ERROR_BUFFER_OVERFLOW);
            FALSE
        }
    }
    fn GetComputerNameExW(_kind: DWORD, buf: LPWSTR, size: *mut DWORD) -> BOOL {
        GetComputerNameW(buf, size)
    }
    fn GetComputerNameExA(_kind: DWORD, buf: LPSTR, size: *mut DWORD) -> BOOL {
        GetComputerNameA(buf, size)
    }
    fn GetSystemDirectoryW(buf: LPWSTR, size: UINT) -> UINT {
        strings::return_w(buf, size, system32())
    }
    fn GetSystemDirectoryA(buf: LPSTR, size: UINT) -> UINT {
        strings::return_a(buf, size, system32())
    }
    fn GetWindowsDirectoryW(buf: LPWSTR, size: UINT) -> UINT {
        strings::return_w(buf, size, "C:\\windows")
    }
    fn GetWindowsDirectoryA(buf: LPSTR, size: UINT) -> UINT {
        strings::return_a(buf, size, "C:\\windows")
    }
    fn GetSystemWindowsDirectoryW(buf: LPWSTR, size: UINT) -> UINT {
        strings::return_w(buf, size, "C:\\windows")
    }
    fn GetSystemWindowsDirectoryA(buf: LPSTR, size: UINT) -> UINT {
        strings::return_a(buf, size, "C:\\windows")
    }
    fn GetSystemWow64DirectoryW(_buf: LPWSTR, _size: UINT) -> UINT {
        set_last_error(ERROR_CALL_NOT_IMPLEMENTED);
        0
    }
    fn GetTempPathW(size: DWORD, buf: LPWSTR) -> DWORD {
        strings::return_w(buf, size, &temp_dir_win())
    }
    fn GetTempPathA(size: DWORD, buf: LPSTR) -> DWORD {
        strings::return_a(buf, size, &temp_dir_win())
    }
    fn GetTempPath2W(size: DWORD, buf: LPWSTR) -> DWORD {
        strings::return_w(buf, size, &temp_dir_win())
    }
    fn GetLogicalDrives() -> DWORD {
        (1 << 2) | (1 << 25)
    }
    fn GetDriveTypeW(root: LPCWSTR) -> UINT {
        let r = if root.is_null() { paths::cwd_win() } else { strings::w_ptr_to_string(root) };
        match paths::has_drive(&r) {
            Some('c') | Some('z') => 3,
            _ => 1,
        }
    }
    fn GetDriveTypeA(root: LPCSTR) -> UINT {
        let r = if root.is_null() { paths::cwd_win() } else { strings::a_to_string(root) };
        match paths::has_drive(&r) {
            Some('c') | Some('z') => 3,
            _ => 1,
        }
    }

    fn OutputDebugStringA(_s: LPCSTR) {}
    fn OutputDebugStringW(_s: LPCWSTR) {}
    fn IsDebuggerPresent() -> BOOL {
        FALSE
    }
    fn CheckRemoteDebuggerPresent(_h: HANDLE, out: *mut BOOL) -> BOOL {
        if !out.is_null() {
            *out = FALSE;
        }
        TRUE
    }
    fn DebugBreak() {
        libc::raise(libc::SIGTRAP);
    }
    fn EncodePointer(p: usize) -> usize {
        p
    }
    fn DecodePointer(p: usize) -> usize {
        p
    }
    fn EncodeSystemPointer(p: usize) -> usize {
        p
    }
    fn DecodeSystemPointer(p: usize) -> usize {
        p
    }
    fn Beep(_freq: DWORD, _ms: DWORD) -> BOOL {
        TRUE
    }
    fn WerSetFlags(_flags: DWORD) -> i32 {
        0
    }
    fn WerGetFlags(_h: HANDLE, flags: *mut DWORD) -> i32 {
        if !flags.is_null() {
            *flags = 0;
        }
        0
    }
    fn GetCurrentPackageFullName(_len: *mut UINT, _name: LPWSTR) -> i32 {
        15700
    }
    fn NeedCurrentDirectoryForExePathW(_exe: LPCWSTR) -> BOOL {
        TRUE
    }
}

pub const ERROR_BUFFER_OVERFLOW: DWORD = 111;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_expansion() {
        runtime::env_set("NLE_TEST_VAR", Some("value"));
        assert_eq!(expand_env("a%NLE_TEST_VAR%b"), "avalueb");
        assert_eq!(expand_env("%UNDEFINED_X%"), "%UNDEFINED_X%");
        assert_eq!(expand_env("100%"), "100%");
    }
}
