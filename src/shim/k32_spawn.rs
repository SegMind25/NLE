//! kernel32: `CreateProcess`, `WinExec`, Toolhelp snapshots and process attribute lists.
//!
//! A Windows child (`MZ`) is started by running the `nle` launcher on it; Linux binaries and
//! scripts are started directly.

use crate::loader;
use crate::runtime::handles::{self, FindEntry, FindObj, Kind, ProcObj, ThreadObj};
use crate::runtime::strings::*;
use crate::runtime::{self, paths};
use crate::shim::k32_file::host_fd_of;
use crate::shim::misc::split_command_line;
use crate::win::*;
use std::ffi::CStr;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

const STILL_ACTIVE_CODE: u32 = 259;
const STARTF_USESTDHANDLES: DWORD = 0x100;
const CREATE_UNICODE_ENVIRONMENT: DWORD = 0x400;
const TH32CS_SNAPTHREAD: DWORD = 0x4;
const TH32CS_SNAPPROCESS: DWORD = 0x2;
const TH32CS_SNAPMODULE: DWORD = 0x8;
const TH32CS_SNAPMODULE32: DWORD = 0x10;

/// Finds the `nle` launcher used to start Windows children.
fn launcher() -> Option<PathBuf> {
    static L: OnceLock<Option<PathBuf>> = OnceLock::new();
    L.get_or_init(|| {
        if let Some(p) = std::env::var_os("NLE_BIN") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Some(p);
            }
        }
        if let Ok(me) = std::fs::read_link("/proc/self/exe") {
            if me.file_name().is_some_and(|n| n == "nle") {
                return Some(me);
            }
        }
        let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
        if unsafe { libc::dladdr(launcher as *const () as *const libc::c_void, &mut info) } != 0 && !info.dli_fname.is_null() {
            let lib = PathBuf::from(unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy().into_owned());
            if let Some(dir) = lib.parent() {
                for cand in [dir.join("nle"), dir.join("../bin/nle")] {
                    if cand.is_file() {
                        return Some(cand);
                    }
                }
            }
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path).map(|d| d.join("nle")).find(|c| c.is_file())
    })
    .clone()
}

fn has_extension(name: &str) -> bool {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    base.rfind('.').is_some_and(|i| i > 0)
}

fn existing_file(p: PathBuf) -> Option<PathBuf> {
    p.is_file().then_some(p)
}

/// Resolves a program name the way `CreateProcess` searches for it.
fn resolve_program(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let try_one = |win: &str| -> Option<PathBuf> {
        let host = if win.starts_with('/') { PathBuf::from(win) } else { paths::resolve(win) };
        existing_file(host.clone()).or_else(|| {
            if has_extension(win) {
                None
            } else {
                let mut s = host.into_os_string();
                s.push(".exe");
                let h2 = paths::ci_resolve(Path::new(&s));
                existing_file(h2)
            }
        })
    };
    if name.contains(['\\', '/']) || paths::has_drive(name).is_some() {
        return try_one(name);
    }
    let mut dirs: Vec<String> = Vec::new();
    if let Some(d) = runtime::cfg().exe_win.rsplit_once('\\').map(|(d, _)| d.to_string()) {
        dirs.push(d);
    }
    dirs.push(paths::cwd_win());
    dirs.push("C:\\windows\\system32".into());
    dirs.push("C:\\windows".into());
    if let Some(p) = runtime::env_get("PATH") {
        dirs.extend(p.split(';').filter(|s| !s.is_empty()).map(str::to_string));
    }
    dirs.iter().find_map(|d| try_one(&format!("{}\\{}", d.trim_end_matches('\\'), name)))
}

/// Works out which program a command line names (the first token may contain unquoted spaces).
fn find_target(app: Option<&str>, cmdline: &str) -> Option<PathBuf> {
    if let Some(a) = app.filter(|a| !a.is_empty()) {
        return resolve_program(a);
    }
    let c = cmdline.trim_start();
    if let Some(rest) = c.strip_prefix('"') {
        return resolve_program(rest.split('"').next().unwrap_or(""));
    }
    let mut end = 0;
    let bytes = c.as_bytes();
    loop {
        let next = bytes[end..].iter().position(|&b| b == b' ' || b == b'\t').map(|i| end + i);
        let stop = next.unwrap_or(c.len());
        if let Some(p) = resolve_program(&c[..stop]) {
            return Some(p);
        }
        match next {
            Some(i) => end = i + 1,
            None => return None,
        }
    }
}

enum ProgramKind {
    Windows,
    Native,
}

fn program_kind(path: &Path) -> Option<ProgramKind> {
    use std::io::Read;
    let mut head = [0u8; 4];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).ok()?;
    if n >= 2 && &head[..2] == b"MZ" {
        Some(ProgramKind::Windows)
    } else if (n >= 4 && &head == b"\x7fELF") || (n >= 2 && &head[..2] == b"#!") {
        Some(ProgramKind::Native)
    } else {
        None
    }
}

fn stdio_for(h: HANDLE, fallback_fd: i32) -> Stdio {
    let fd = host_fd_of(h).filter(|fd| *fd >= 0).unwrap_or(fallback_fd);
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if dup < 0 {
        Stdio::inherit()
    } else {
        Stdio::from(unsafe { OwnedFd::from_raw_fd(dup) })
    }
}

fn exit_code_of(st: std::process::ExitStatus) -> u32 {
    use std::os::unix::process::ExitStatusExt;
    match (st.code(), st.signal()) {
        (Some(c), _) => c as u32,
        (None, Some(libc::SIGSEGV)) => 0xC000_0005,
        (None, Some(libc::SIGINT)) => 0xC000_013A,
        (None, Some(s)) => 128 + s as u32,
        _ => 1,
    }
}

unsafe fn parse_env_block(block: *const u8, unicode: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut p = block;
    loop {
        let entry = if unicode {
            let w = wstr(p as *const u16);
            if w.is_empty() {
                break;
            }
            p = p.add((w.len() + 1) * 2);
            w_to_string(w)
        } else {
            let a = astr(p);
            if a.is_empty() {
                break;
            }
            p = p.add(a.len() + 1);
            String::from_utf8_lossy(a).into_owned()
        };
        if entry.starts_with('=') {
            continue;
        }
        if let Some((k, v)) = entry.split_once('=') {
            out.push((k.to_string(), v.to_string()));
        }
    }
    out
}

struct Request<'a> {
    app: Option<String>,
    cmdline: String,
    env: Option<Vec<(String, String)>>,
    cwd: Option<String>,
    startf: DWORD,
    si: Option<&'a STARTUPINFOW>,
    pi: *mut PROCESS_INFORMATION,
}

/// A Linux program cannot open `Z:\\tmp\\x`, so an argument that is a whole drive path (a launcher passing the script it
/// was started from, say) is handed over as the host path. Anything else is passed unchanged.
fn native_arg(arg: &str) -> String {
    let b = arg.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') {
        paths::resolve(arg).to_string_lossy().into_owned()
    } else {
        arg.to_string()
    }
}

unsafe fn spawn(req: Request) -> BOOL {
    let Some(target) = find_target(req.app.as_deref(), &req.cmdline) else {
        set_last_error(ERROR_FILE_NOT_FOUND);
        return FALSE;
    };
    let Some(kind) = program_kind(&target) else {
        set_last_error(ERROR_BAD_EXE_FORMAT);
        return FALSE;
    };
    let argv = split_command_line(&req.cmdline);
    let args: Vec<String> = argv.into_iter().skip(1).collect();

    let mut cmd = match kind {
        ProgramKind::Windows => {
            let Some(bin) = launcher() else {
                eprintln!("nle: cannot start {}: the `nle` launcher was not found (set NLE_BIN)", target.display());
                set_last_error(ERROR_FILE_NOT_FOUND);
                return FALSE;
            };
            let mut c = Command::new(bin);
            c.arg("run").arg(&target).args(&args);
            c
        }
        ProgramKind::Native => {
            let mut c = Command::new(&target);
            c.args(args.iter().map(|a| native_arg(a)));
            c
        }
    };

    // The child sees the command line verbatim and the Windows-view environment: the supplied
    // block, or ours.
    let win_env: Vec<(String, String)> = req.env.clone().unwrap_or_else(|| runtime::ENV.lock().unwrap().clone());
    cmd.env_clear();
    cmd.env("NLE_CMDLINE", &req.cmdline);
    if let Some(p) = std::env::var_os("PATH") {
        cmd.env("PATH", p);
    }
    for (k, v) in &win_env {
        if k.eq_ignore_ascii_case("PATH") {
            cmd.env("NLE_WINPATH", v);
        } else {
            cmd.env(k, v);
        }
    }
    for (k, v) in std::env::vars() {
        if k.starts_with("NLE_") && k != "NLE_CMDLINE" && k != "NLE_WINPATH" {
            cmd.env(k, v);
        }
    }
    cmd.env("NLE_PREFIX", paths::prefix());
    if let Some(dir) = &req.cwd {
        cmd.current_dir(paths::resolve(dir));
    }

    let use_std = req.startf & STARTF_USESTDHANDLES != 0;
    let (hin, hout, herr) = match req.si {
        Some(si) if use_std => (si.std_input, si.std_output, si.std_error),
        _ => (handles::std_handle(STD_INPUT_HANDLE), handles::std_handle(STD_OUTPUT_HANDLE), handles::std_handle(STD_ERROR_HANDLE)),
    };
    cmd.stdin(stdio_for(hin, 0)).stdout(stdio_for(hout, 1)).stderr(stdio_for(herr, 2));

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            set_last_error(match e.kind() {
                std::io::ErrorKind::PermissionDenied => ERROR_ACCESS_DENIED,
                std::io::ErrorKind::NotFound => ERROR_FILE_NOT_FOUND,
                _ => ERROR_BAD_EXE_FORMAT,
            });
            return FALSE;
        }
    };
    let pid = child.id();
    let proc_obj = Arc::new(Kind::Process(ProcObj {
        pid: AtomicU32::new(pid),
        done: AtomicBool::new(false),
        exit_code: AtomicU32::new(STILL_ACTIVE_CODE),
    }));
    let h_proc = handles::insert_arc(proc_obj.clone());
    let h_thread = handles::insert(Kind::Thread(ThreadObj {
        tid: AtomicU32::new(pid),
        done: AtomicBool::new(false),
        exit_code: AtomicU32::new(STILL_ACTIVE_CODE),
        suspended: Mutex::new(false),
        resume: Condvar::new(),
        pthread: AtomicU64::new(0),
    }));
    std::thread::spawn(move || {
        let mut child = child;
        let code = child.wait().map(exit_code_of).unwrap_or(1);
        if let Kind::Process(p) = &*proc_obj {
            handles::locked_change(|| {
                p.exit_code.store(code, Ordering::SeqCst);
                p.done.store(true, Ordering::SeqCst);
            });
        }
    });
    if !req.pi.is_null() {
        *req.pi = PROCESS_INFORMATION { process: h_proc, thread: h_thread, process_id: pid, thread_id: pid };
    } else {
        handles::close(h_proc);
        handles::close(h_thread);
    }
    TRUE
}

crate::shims! { "kernel32" => SPAWN_SHIMS;
    fn CreateProcessW(app: LPCWSTR, cmd: LPWSTR, _pa: usize, _ta: usize, _inherit: BOOL, flags: DWORD, env: *const u8, cwd: LPCWSTR, si: *const STARTUPINFOW, pi: *mut PROCESS_INFORMATION) -> BOOL {
        let si = if si.is_null() { None } else { Some(&*si) };
        spawn(Request {
            app: (!app.is_null()).then(|| w_ptr_to_string(app)),
            cmdline: if cmd.is_null() { String::new() } else { w_ptr_to_string(cmd) },
            env: (!env.is_null()).then(|| parse_env_block(env, flags & CREATE_UNICODE_ENVIRONMENT != 0)),
            cwd: (!cwd.is_null()).then(|| w_ptr_to_string(cwd)),
            startf: si.map_or(0, |s| s.flags),
            si,
            pi,
        })
    }
    fn CreateProcessA(app: LPCSTR, cmd: LPSTR, _pa: usize, _ta: usize, _inherit: BOOL, flags: DWORD, env: *const u8, cwd: LPCSTR, si: *const STARTUPINFOW, pi: *mut PROCESS_INFORMATION) -> BOOL {
        let si = if si.is_null() { None } else { Some(&*si) };
        spawn(Request {
            app: (!app.is_null()).then(|| a_to_string(app)),
            cmdline: if cmd.is_null() { String::new() } else { a_to_string(cmd) },
            env: (!env.is_null()).then(|| parse_env_block(env, flags & CREATE_UNICODE_ENVIRONMENT != 0)),
            cwd: (!cwd.is_null()).then(|| a_to_string(cwd)),
            startf: si.map_or(0, |s| s.flags),
            si,
            pi,
        })
    }
    fn WinExec(cmd: LPCSTR, _show: UINT) -> UINT {
        let mut pi = PROCESS_INFORMATION { process: 0, thread: 0, process_id: 0, thread_id: 0 };
        let ok = spawn(Request { app: None, cmdline: a_to_string(cmd), env: None, cwd: None, startf: 0, si: None, pi: &mut pi });
        if ok == TRUE {
            handles::close(pi.process);
            handles::close(pi.thread);
            33
        } else {
            ERROR_FILE_NOT_FOUND
        }
    }
    fn InitializeProcThreadAttributeList(list: *mut u8, _count: DWORD, _flags: DWORD, size: *mut usize) -> BOOL {
        const NEEDED: usize = 64;
        if list.is_null() || *size < NEEDED {
            *size = NEEDED;
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            return FALSE;
        }
        std::ptr::write_bytes(list, 0, NEEDED);
        TRUE
    }
    fn UpdateProcThreadAttribute(_list: *mut u8, _flags: DWORD, _attr: usize, _value: *const u8, _size: usize, _prev: *mut u8, _ret: *mut usize) -> BOOL {
        TRUE
    }
    fn DeleteProcThreadAttributeList(_list: *mut u8) {}
    fn CreateToolhelp32Snapshot(flags: DWORD, pid: DWORD) -> HANDLE {
        let mut entries: Vec<FindEntry> = Vec::new();
        let mut add = |name: String, info: String| entries.push(FindEntry { name, path: PathBuf::from(info) });
        if flags & TH32CS_SNAPPROCESS != 0 {
            if let Ok(rd) = std::fs::read_dir("/proc") {
                for e in rd.flatten() {
                    let Some(p) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
                    let Ok(stat) = std::fs::read_to_string(format!("/proc/{p}/stat")) else { continue };
                    let (Some(l), Some(r)) = (stat.find('('), stat.rfind(')')) else { continue };
                    let comm = &stat[l + 1..r];
                    let fields: Vec<&str> = stat[r + 2..].split(' ').collect();
                    let ppid = fields.get(1).copied().unwrap_or("0");
                    let threads = fields.get(17).copied().unwrap_or("1");
                    add(comm.to_string(), format!("p:{p} {ppid} {threads}"));
                }
            }
        }
        if flags & (TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32) != 0 && (pid == 0 || pid == libc::getpid() as DWORD) {
            for m in loader::all_modules() {
                let win = m.win_path();
                let base = win.rsplit('\\').next().unwrap_or(&win).to_string();
                add(base, format!("m:{:x} {:x} {}", m.base, m.size, win));
            }
        }
        if flags & TH32CS_SNAPTHREAD != 0 {
            if let Ok(rd) = std::fs::read_dir("/proc/self/task") {
                for e in rd.flatten() {
                    if let Some(t) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) {
                        add(String::new(), format!("t:{t} {}", libc::getpid()));
                    }
                }
            }
        }
        handles::insert(Kind::Find(FindObj { entries: Mutex::new((entries, 0)) }))
    }
    fn Process32FirstW(snap: HANDLE, out: *mut u8) -> BOOL {
        snap_entry(snap, out, true, b'p', true, fill_process)
    }
    fn Process32NextW(snap: HANDLE, out: *mut u8) -> BOOL {
        snap_entry(snap, out, false, b'p', true, fill_process)
    }
    fn Module32FirstW(snap: HANDLE, out: *mut u8) -> BOOL {
        snap_entry(snap, out, true, b'm', true, fill_module)
    }
    fn Module32NextW(snap: HANDLE, out: *mut u8) -> BOOL {
        snap_entry(snap, out, false, b'm', true, fill_module)
    }
    fn Process32First(snap: HANDLE, out: *mut u8) -> BOOL {
        snap_entry(snap, out, true, b'p', false, fill_process)
    }
    fn Process32Next(snap: HANDLE, out: *mut u8) -> BOOL {
        snap_entry(snap, out, false, b'p', false, fill_process)
    }
    fn Module32First(snap: HANDLE, out: *mut u8) -> BOOL {
        snap_entry(snap, out, true, b'm', false, fill_module)
    }
    fn Module32Next(snap: HANDLE, out: *mut u8) -> BOOL {
        snap_entry(snap, out, false, b'm', false, fill_module)
    }
    fn Thread32First(snap: HANDLE, out: *mut u8) -> BOOL {
        snap_entry(snap, out, true, b't', true, fill_thread)
    }
    fn Thread32Next(snap: HANDLE, out: *mut u8) -> BOOL {
        snap_entry(snap, out, false, b't', true, fill_thread)
    }
}

crate::shims! { "ws2_32" => WS2_SPAWN_SHIMS;
    fn WSADuplicateSocketW(_s: usize, _pid: DWORD, _info: *mut u8) -> i32 {
        set_last_error(10045);
        -1
    }
}

type Filler = unsafe fn(&FindEntry, *mut u8, bool);

/// A snapshot holds rows tagged `p:` (process), `m:` (module) or `t:` (thread); First restarts the
/// walk and Next advances it, both skipping rows of the other kinds.
unsafe fn snap_entry(snap: HANDLE, out: *mut u8, first: bool, kind: u8, wide: bool, fill: Filler) -> BOOL {
    let Some(obj) = handles::get(snap) else {
        set_last_error(ERROR_INVALID_HANDLE);
        return FALSE;
    };
    let Kind::Find(f) = &*obj else {
        set_last_error(ERROR_INVALID_HANDLE);
        return FALSE;
    };
    let mut g = f.entries.lock().unwrap();
    if first {
        g.1 = 0;
    }
    let (list, idx) = &mut *g;
    while *idx < list.len() {
        let e = &list[*idx];
        *idx += 1;
        if e.path.to_string_lossy().as_bytes().first() == Some(&kind) {
            fill(e, out, wide);
            return TRUE;
        }
    }
    set_last_error(ERROR_NO_MORE_FILES);
    FALSE
}

fn row_info(e: &FindEntry) -> String {
    e.path.to_string_lossy().get(2..).unwrap_or("").to_string()
}

unsafe fn put_name(dst: *mut u8, cap: usize, s: &str, wide: bool) {
    if wide {
        copy_w(dst as *mut u16, cap, &to_wide(s));
    } else {
        copy_a(dst, cap, s.as_bytes());
    }
}

unsafe fn fill_process(e: &FindEntry, out: *mut u8, wide: bool) {
    let info = row_info(e);
    let mut it = info.split(' ').map(|s| s.parse::<u32>().unwrap_or(0));
    let (pid, ppid, threads) = (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(1));
    std::ptr::write_bytes(out.add(4), 0, if wide { 564 } else { 292 });
    *(out.add(8) as *mut u32) = pid;
    *(out.add(28) as *mut u32) = threads;
    *(out.add(32) as *mut u32) = ppid;
    *(out.add(36) as *mut i32) = 8;
    put_name(out.add(44), 260, &e.name, wide);
}

unsafe fn fill_module(e: &FindEntry, out: *mut u8, wide: bool) {
    let info = row_info(e);
    let mut it = info.splitn(3, ' ');
    let base = usize::from_str_radix(it.next().unwrap_or("0"), 16).unwrap_or(0);
    let size = u32::from_str_radix(it.next().unwrap_or("0"), 16).unwrap_or(0);
    let path = it.next().unwrap_or("");
    std::ptr::write_bytes(out.add(4), 0, if wide { 1076 } else { 560 });
    *(out.add(8) as *mut u32) = libc::getpid() as u32;
    *(out.add(12) as *mut u32) = 1;
    *(out.add(16) as *mut u32) = 1;
    *(out.add(24) as *mut usize) = base;
    *(out.add(32) as *mut u32) = size;
    *(out.add(40) as *mut usize) = base;
    put_name(out.add(48), 256, &e.name, wide);
    put_name(out.add(if wide { 560 } else { 304 }), 260, path, wide);
}

unsafe fn fill_thread(e: &FindEntry, out: *mut u8, _wide: bool) {
    let info = row_info(e);
    let mut it = info.split(' ').map(|s| s.parse::<u32>().unwrap_or(0));
    let (tid, owner) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    std::ptr::write_bytes(out.add(4), 0, 24);
    *(out.add(8) as *mut u32) = tid;
    *(out.add(12) as *mut u32) = owner;
    *(out.add(16) as *mut i32) = 8;
}

#[cfg(test)]
mod tests {
    use super::native_arg;

    #[test]
    fn only_whole_drive_paths_are_translated() {
        assert_eq!(native_arg("Z:\\usr\\bin\\env"), "/usr/bin/env");
        assert_eq!(native_arg("z:/usr/bin/env"), "/usr/bin/env");
        assert_eq!(native_arg("--flag"), "--flag");
        assert_eq!(native_arg("a:b"), "a:b");
        assert_eq!(native_arg("--path=Z:\\tmp"), "--path=Z:\\tmp");
        assert_eq!(native_arg("http://x"), "http://x");
    }
}
