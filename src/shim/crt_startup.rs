//! msvcrt / ucrt: program startup (`__getmainargs`, `_initterm`), exit and atexit handling, errno,
//! locks, signals, threads and the global CRT data exports.

use crate::loader;
use crate::runtime::strings::*;
use crate::runtime::{self};
use crate::shim::k32_sync;
use crate::win::*;
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::sync::Mutex;

type VoidFn = unsafe extern "win64" fn();
type SigHandler = unsafe extern "win64" fn(i32);
type ThreadFn = unsafe extern "win64" fn(usize) -> u32;

thread_local! {
    static ERRNO: UnsafeCell<i32> = const { UnsafeCell::new(0) };
    static DOSERRNO: UnsafeCell<u32> = const { UnsafeCell::new(0) };
}

pub fn errno_ptr() -> *mut i32 {
    ERRNO.with(|e| e.get())
}

pub static mut ARGC: i32 = 0;
pub static mut ARGV: usize = 0;
pub static mut WARGV: usize = 0;
pub static mut ENVIRON: usize = 0;
pub static mut WENVIRON: usize = 0;
pub static mut FMODE: i32 = 0;
pub static mut COMMODE: i32 = 0;
pub static mut PGMPTR: usize = 0;
pub static mut WPGMPTR: usize = 0;
pub static mut ACMDLN: usize = 0;
pub static mut WCMDLN: usize = 0;
pub static mut WINVER: u32 = 0x0a00;
pub static mut WINMAJOR: u32 = 10;
pub static mut WINMINOR: u32 = 0;
pub static mut OSVER: u32 = 19045;
pub static mut OSPLATFORM: u32 = 2;
pub static mut DAYLIGHT: i32 = 0;
pub static mut TIMEZONE: i32 = 0;
pub static mut TZNAME: [usize; 2] = [0; 2];

static ATEXIT: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static ONEXIT_TABLES: Mutex<Option<HashMap<usize, Vec<usize>>>> = Mutex::new(None);
static SIGNALS: Mutex<[usize; 32]> = Mutex::new([0; 32]);
static mut LOCKS: [CRITICAL_SECTION; 64] = unsafe { std::mem::zeroed() };

unsafe fn leak_cstr(s: &str) -> *mut u8 {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    Box::leak(v.into_boxed_slice()).as_mut_ptr()
}

unsafe fn leak_wstr(s: &str) -> *mut u16 {
    Box::leak(to_wide_nul(s).into_boxed_slice()).as_mut_ptr()
}

unsafe fn ensure_args() {
    if ARGV != 0 {
        return;
    }
    let c = runtime::cfg();
    let mut av: Vec<*mut u8> = vec![leak_cstr(&c.exe_win)];
    av.extend(c.args.iter().map(|a| leak_cstr(a)));
    ARGC = av.len() as i32;
    av.push(std::ptr::null_mut());
    ARGV = Box::leak(av.into_boxed_slice()).as_ptr() as usize;
    let mut wv: Vec<*mut u16> = vec![leak_wstr(&c.exe_win)];
    wv.extend(c.args.iter().map(|a| leak_wstr(a)));
    wv.push(std::ptr::null_mut());
    WARGV = Box::leak(wv.into_boxed_slice()).as_ptr() as usize;
    let env = runtime::ENV.lock().unwrap().clone();
    let mut ev: Vec<*mut u8> = env.iter().map(|(k, v)| leak_cstr(&format!("{k}={v}"))).collect();
    ev.push(std::ptr::null_mut());
    ENVIRON = Box::leak(ev.into_boxed_slice()).as_ptr() as usize;
    let mut wev: Vec<*mut u16> = env.iter().map(|(k, v)| leak_wstr(&format!("{k}={v}"))).collect();
    wev.push(std::ptr::null_mut());
    WENVIRON = Box::leak(wev.into_boxed_slice()).as_ptr() as usize;
    PGMPTR = *(ARGV as *const usize);
    WPGMPTR = *(WARGV as *const usize);
    ACMDLN = leak_cstr(&runtime::command_line()) as usize;
    WCMDLN = leak_wstr(&runtime::command_line()) as usize;
}

pub unsafe fn run_atexit() {
    loop {
        let f = ATEXIT.lock().unwrap().pop();
        match f {
            Some(f) => {
                let f: VoidFn = std::mem::transmute(f);
                f();
            }
            None => break,
        }
    }
}

unsafe fn do_exit(code: i32) -> ! {
    run_atexit();
    loader::exit_process(code as u32)
}

unsafe fn raise_signal(sig: i32) -> i32 {
    if !(1..32).contains(&sig) {
        return -1;
    }
    let h = SIGNALS.lock().unwrap()[sig as usize];
    match h {
        1 => 0,
        0 => {
            match sig {
                6 => {
                    eprintln!("\nThis application has requested the Runtime to terminate it in an unusual way.");
                    loader::exit_process(3)
                }
                2 => loader::exit_process(0xc000_013a),
                _ => loader::exit_process(3),
            }
        }
        h => {
            SIGNALS.lock().unwrap()[sig as usize] = 0;
            let f: SigHandler = std::mem::transmute(h);
            f(sig);
            0
        }
    }
}

extern "C" fn host_signal(sig: libc::c_int) {
    unsafe {
        let h = SIGNALS.lock().map(|s| s[sig as usize]).unwrap_or(0);
        if h > 1 {
            let f: SigHandler = std::mem::transmute(h);
            f(sig);
        }
    }
}

crate::shims! { "crt";
    fn __getmainargs(argc: *mut i32, argv: *mut usize, env: *mut usize, _wild: i32, _info: usize) -> i32 {
        ensure_args();
        *argc = ARGC;
        *argv = ARGV;
        *env = ENVIRON;
        0
    }
    fn __wgetmainargs(argc: *mut i32, argv: *mut usize, env: *mut usize, _wild: i32, _info: usize) -> i32 {
        ensure_args();
        *argc = ARGC;
        *argv = WARGV;
        *env = WENVIRON;
        0
    }
    fn __p___argc() -> *mut i32 {
        ensure_args();
        std::ptr::addr_of_mut!(ARGC)
    }
    fn __p___argv() -> *mut usize {
        ensure_args();
        std::ptr::addr_of_mut!(ARGV)
    }
    fn __p___wargv() -> *mut usize {
        ensure_args();
        std::ptr::addr_of_mut!(WARGV)
    }
    fn __p__environ() -> *mut usize {
        ensure_args();
        std::ptr::addr_of_mut!(ENVIRON)
    }
    fn __p__wenviron() -> *mut usize {
        ensure_args();
        std::ptr::addr_of_mut!(WENVIRON)
    }
    fn __p__pgmptr() -> *mut usize {
        ensure_args();
        std::ptr::addr_of_mut!(PGMPTR)
    }
    fn __p__wpgmptr() -> *mut usize {
        ensure_args();
        std::ptr::addr_of_mut!(WPGMPTR)
    }
    fn __p__acmdln() -> *mut usize {
        ensure_args();
        std::ptr::addr_of_mut!(ACMDLN)
    }
    fn __p__wcmdln() -> *mut usize {
        ensure_args();
        std::ptr::addr_of_mut!(WCMDLN)
    }
    fn __p__fmode() -> *mut i32 {
        std::ptr::addr_of_mut!(FMODE)
    }
    fn __p__commode() -> *mut i32 {
        std::ptr::addr_of_mut!(COMMODE)
    }
    fn _get_pgmptr(out: *mut usize) -> i32 {
        ensure_args();
        *out = PGMPTR;
        0
    }
    fn _get_wpgmptr(out: *mut usize) -> i32 {
        ensure_args();
        *out = WPGMPTR;
        0
    }
    fn _configure_narrow_argv(_mode: i32) -> i32 {
        ensure_args();
        0
    }
    fn _configure_wide_argv(_mode: i32) -> i32 {
        ensure_args();
        0
    }
    fn _initialize_narrow_environment() -> i32 {
        ensure_args();
        0
    }
    fn _initialize_wide_environment() -> i32 {
        ensure_args();
        0
    }
    fn _get_initial_narrow_environment() -> usize {
        ensure_args();
        ENVIRON
    }
    fn _get_initial_wide_environment() -> usize {
        ensure_args();
        WENVIRON
    }
    fn _get_narrow_winmain_command_line() -> usize {
        ensure_args();
        ACMDLN
    }
    fn _get_wide_winmain_command_line() -> usize {
        ensure_args();
        WCMDLN
    }
    fn _set_fmode(mode: i32) -> i32 {
        FMODE = mode;
        0
    }
    fn _get_fmode(out: *mut i32) -> i32 {
        *out = FMODE;
        0
    }
    fn __set_app_type(_t: i32) {}
    fn _set_app_type(_t: i32) {}
    fn __setusermatherr(_h: usize) {}
    fn _set_new_mode(_m: i32) -> i32 {
        0
    }
    fn _configthreadlocale(_m: i32) -> i32 {
        0
    }
    fn _set_invalid_parameter_handler(_h: usize) -> usize {
        0
    }
    fn _set_purecall_handler(_h: usize) -> usize {
        0
    }
    fn _set_abort_behavior(_flags: u32, _mask: u32) -> u32 {
        0
    }
    fn _set_error_mode(_m: i32) -> i32 {
        0
    }
    fn _seh_filter_exe(_code: u32, _info: usize) -> i32 {
        0
    }
    fn _controlfp(new: u32, mask: u32) -> u32 {
        let _ = (new, mask);
        0x9001f
    }
    fn _controlfp_s(cur: *mut u32, _new: u32, _mask: u32) -> i32 {
        if !cur.is_null() {
            *cur = 0x9001f;
        }
        0
    }
    fn _clearfp() -> u32 {
        0
    }
    fn _statusfp() -> u32 {
        0
    }
    fn _fpreset() {}
    fn __fpecode() -> *mut i32 {
        errno_ptr()
    }
    fn _initterm(start: *const usize, end: *const usize) {
        let mut p = start;
        while p < end {
            if *p != 0 {
                let f: VoidFn = std::mem::transmute(*p);
                f();
            }
            p = p.add(1);
        }
    }
    fn _initterm_e(start: *const usize, end: *const usize) -> i32 {
        let mut p = start;
        while p < end {
            if *p != 0 {
                let f: unsafe extern "win64" fn() -> i32 = std::mem::transmute(*p);
                let r = f();
                if r != 0 {
                    return r;
                }
            }
            p = p.add(1);
        }
        0
    }
    fn atexit(f: usize) -> i32 {
        ATEXIT.lock().unwrap().push(f);
        0
    }
    fn _crt_atexit(f: usize) -> i32 {
        ATEXIT.lock().unwrap().push(f);
        0
    }
    fn _onexit(f: usize) -> usize {
        ATEXIT.lock().unwrap().push(f);
        f
    }
    fn _initialize_onexit_table(table: *mut usize) -> i32 {
        for i in 0..3 {
            *table.add(i) = 0;
        }
        ONEXIT_TABLES.lock().unwrap().get_or_insert_with(HashMap::new).insert(table as usize, Vec::new());
        0
    }
    fn _register_onexit_function(table: *mut usize, f: usize) -> i32 {
        ONEXIT_TABLES.lock().unwrap().get_or_insert_with(HashMap::new).entry(table as usize).or_default().push(f);
        0
    }
    fn _execute_onexit_table(table: *mut usize) -> i32 {
        loop {
            let f = ONEXIT_TABLES.lock().unwrap().as_mut().and_then(|m| m.get_mut(&(table as usize))).and_then(|v| v.pop());
            match f {
                Some(f) => {
                    let f: VoidFn = std::mem::transmute(f);
                    f();
                }
                None => break,
            }
        }
        0
    }
    fn exit(code: i32) -> ! {
        do_exit(code)
    }
    fn _exit(code: i32) -> ! {
        loader::exit_process(code as u32)
    }
    fn _Exit(code: i32) -> ! {
        loader::exit_process(code as u32)
    }
    fn quick_exit(code: i32) -> ! {
        loader::exit_process(code as u32)
    }
    fn _cexit() {
        run_atexit();
        libc::fflush(std::ptr::null_mut());
    }
    fn _c_exit() {}
    fn abort() -> ! {
        raise_signal(6);
        loader::exit_process(3)
    }
    fn _amsg_exit(n: i32) -> ! {
        eprintln!("runtime error R{n:04}");
        loader::exit_process(255)
    }
    fn _lock(n: i32) {
        k32_sync::cs_enter(std::ptr::addr_of_mut!(LOCKS[(n as usize) & 63]));
    }
    fn _unlock(n: i32) {
        k32_sync::cs_leave(std::ptr::addr_of_mut!(LOCKS[(n as usize) & 63]));
    }
    fn __crtSetUnhandledExceptionFilter(f: usize) -> usize {
        f
    }

    fn _errno() -> *mut i32 {
        errno_ptr()
    }
    // libuv and friends read `_doserrno` right after a failed CRT call: it follows the errno the call left
    fn __doserrno() -> *mut u32 {
        DOSERRNO.with(|e| {
            *e.get() = errno_to_win(*errno_ptr());
            e.get()
        })
    }
    fn _get_errno(out: *mut i32) -> i32 {
        *out = *errno_ptr();
        0
    }
    fn _set_errno(v: i32) -> i32 {
        *errno_ptr() = v;
        0
    }
    fn strerror(n: i32) -> *const u8 {
        libc::strerror(n) as *const u8
    }
    fn _strerror(msg: *const u8) -> *const u8 {
        let e = *errno_ptr();
        let text = format!("{}{}\n", if msg.is_null() { String::new() } else { format!("{}: ", a_to_string(msg)) }, a_to_string(libc::strerror(e) as *const u8));
        leak_cstr(&text)
    }
    fn perror(msg: *const u8) {
        let e = *errno_ptr();
        let m = a_to_string(libc::strerror(e) as *const u8);
        if msg.is_null() || *msg == 0 {
            eprintln!("{m}");
        } else {
            eprintln!("{}: {m}", a_to_string(msg));
        }
    }

    fn signal(sig: i32, handler: usize) -> usize {
        if !(1..32).contains(&sig) || sig == 9 || sig == 19 {
            *errno_ptr() = 22;
            return usize::MAX;
        }
        let prev = std::mem::replace(&mut SIGNALS.lock().unwrap()[sig as usize], handler);
        if matches!(sig, 2 | 15) {
            let host = if handler == 0 {
                libc::SIG_DFL
            } else if handler == 1 {
                libc::SIG_IGN
            } else {
                host_signal as *const () as usize
            };
            libc::signal(sig, host);
        }
        prev
    }
    fn raise(sig: i32) -> i32 {
        raise_signal(sig)
    }

    fn _beginthreadex(_sec: usize, stack: u32, start: ThreadFn, arg: usize, flags: u32, tid: *mut u32) -> usize {
        crate::shim::k32_thread::CreateThread(0, stack as usize, start as usize, arg, flags, tid)
    }
    fn _endthreadex(code: u32) -> ! {
        crate::shim::k32_thread::ExitThread(code)
    }
    fn _beginthread(start: ThreadFn, stack: u32, arg: usize) -> usize {
        crate::shim::k32_thread::CreateThread(0, stack as usize, start as usize, arg, 0, std::ptr::null_mut())
    }
    fn _endthread() -> ! {
        crate::shim::k32_thread::ExitThread(0)
    }
    fn _getpid() -> i32 {
        libc::getpid()
    }
    fn getpid() -> i32 {
        libc::getpid()
    }
    fn _cwait(_status: *mut i32, _pid: usize, _action: i32) -> usize {
        usize::MAX
    }

    fn setlocale(_cat: i32, _loc: *const u8) -> *const u8 {
        b"C\0".as_ptr()
    }
    fn _wsetlocale(_cat: i32, _loc: *const u16) -> *const u16 {
        static C: [u16; 2] = [b'C' as u16, 0];
        C.as_ptr()
    }
    fn localeconv() -> *mut u8 {
        static mut LCONV: [usize; 24] = [0; 24];
        LCONV[0] = b".\0".as_ptr() as usize;
        for i in 1..10 {
            LCONV[i] = b"\0".as_ptr() as usize;
        }
        std::ptr::addr_of_mut!(LCONV) as *mut u8
    }
    fn _create_locale(_cat: i32, _loc: *const u8) -> usize {
        1
    }
    fn _free_locale(_l: usize) {}
    fn __crt_dummy() {}
}

/// Registers data exports (variables the program reads through its import table).
pub static DATA: &[crate::registry::Shim] = &[
    crate::registry::Shim { family: "crt", name: "_fmode", addr: std::ptr::addr_of!(FMODE) as *const () },
    crate::registry::Shim { family: "crt", name: "_commode", addr: std::ptr::addr_of!(COMMODE) as *const () },
    crate::registry::Shim { family: "crt", name: "__argc", addr: std::ptr::addr_of!(ARGC) as *const () },
    crate::registry::Shim { family: "crt", name: "__argv", addr: std::ptr::addr_of!(ARGV) as *const () },
    crate::registry::Shim { family: "crt", name: "__wargv", addr: std::ptr::addr_of!(WARGV) as *const () },
    crate::registry::Shim { family: "crt", name: "_environ", addr: std::ptr::addr_of!(ENVIRON) as *const () },
    crate::registry::Shim { family: "crt", name: "_wenviron", addr: std::ptr::addr_of!(WENVIRON) as *const () },
    crate::registry::Shim { family: "crt", name: "__initenv", addr: std::ptr::addr_of!(ENVIRON) as *const () },
    crate::registry::Shim { family: "crt", name: "__winitenv", addr: std::ptr::addr_of!(WENVIRON) as *const () },
    crate::registry::Shim { family: "crt", name: "_pgmptr", addr: std::ptr::addr_of!(PGMPTR) as *const () },
    crate::registry::Shim { family: "crt", name: "_wpgmptr", addr: std::ptr::addr_of!(WPGMPTR) as *const () },
    crate::registry::Shim { family: "crt", name: "_acmdln", addr: std::ptr::addr_of!(ACMDLN) as *const () },
    crate::registry::Shim { family: "crt", name: "_wcmdln", addr: std::ptr::addr_of!(WCMDLN) as *const () },
    crate::registry::Shim { family: "crt", name: "_winver", addr: std::ptr::addr_of!(WINVER) as *const () },
    crate::registry::Shim { family: "crt", name: "_winmajor", addr: std::ptr::addr_of!(WINMAJOR) as *const () },
    crate::registry::Shim { family: "crt", name: "_winminor", addr: std::ptr::addr_of!(WINMINOR) as *const () },
    crate::registry::Shim { family: "crt", name: "_osver", addr: std::ptr::addr_of!(OSVER) as *const () },
    crate::registry::Shim { family: "crt", name: "_osplatform", addr: std::ptr::addr_of!(OSPLATFORM) as *const () },
    crate::registry::Shim { family: "crt", name: "_daylight", addr: std::ptr::addr_of!(DAYLIGHT) as *const () },
    crate::registry::Shim { family: "crt", name: "_timezone", addr: std::ptr::addr_of!(TIMEZONE) as *const () },
    crate::registry::Shim { family: "crt", name: "_tzname", addr: std::ptr::addr_of!(TZNAME) as *const () },
    crate::registry::Shim { family: "crt", name: "_pctype", addr: std::ptr::addr_of!(super::crt_string::PCTYPE) as *const () },
    crate::registry::Shim { family: "crt", name: "__mb_cur_max", addr: std::ptr::addr_of!(super::crt_string::MB_CUR_MAX) as *const () },
];

/// Called once by the loader before guest code runs: makes the data exports valid.
pub fn init_data() {
    super::crt_string::init_data();
    unsafe {
        ensure_args();
        TZNAME = [b"UTC\0".as_ptr() as usize, b"UTC\0".as_ptr() as usize];
    }
}

pub unsafe fn thread_errno_reset() {
    *errno_ptr() = 0;
}
