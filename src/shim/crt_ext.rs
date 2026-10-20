//! More of the C runtime that real programs import: the secure / positional printf entry points, `strtoimax`,
//! wide string odds and ends, conio, the time zone variables, `_dclass`, and the VC++ runtime's exception state.

use super::crt_startup::errno_ptr;
use crate::runtime::strings::*;
use crate::runtime::{self, paths};
use crate::win::*;
use std::cell::UnsafeCell;
use std::ffi::CString;

thread_local! {
    /// What `__current_exception` and `__current_exception_context` hand out: the exception being handled, if any
    static CURRENT_EXCEPTION: UnsafeCell<usize> = const { UnsafeCell::new(0) };
    static CURRENT_CONTEXT: UnsafeCell<usize> = const { UnsafeCell::new(0) };
    static UNGOT: UnsafeCell<i32> = const { UnsafeCell::new(-1) };
}

static mut SYS_NERR: i32 = 43;
static SYS_ERRLIST: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

static mut DAYLIGHT: i32 = 0;
static mut TIMEZONE: i32 = 0;
static mut DSTBIAS: i32 = 0;

/// A byte from the terminal (or a pipe) without line buffering; -1 at the end of the input.
unsafe fn read_key(echo: bool) -> i32 {
    let ungot = UNGOT.with(|u| std::mem::replace(&mut *u.get(), -1));
    if ungot >= 0 {
        return ungot;
    }
    let mut tio: libc::termios = std::mem::zeroed();
    let tty = libc::isatty(0) == 1 && libc::tcgetattr(0, &mut tio) == 0;
    let saved = tio;
    if tty {
        tio.c_lflag &= !(libc::ICANON | libc::ECHO);
        libc::tcsetattr(0, libc::TCSANOW, &tio);
    }
    let mut b = 0u8;
    let n = libc::read(0, &mut b as *mut u8 as *mut libc::c_void, 1);
    if tty {
        libc::tcsetattr(0, libc::TCSANOW, &saved);
    }
    if n != 1 {
        return -1;
    }
    if echo {
        libc::write(1, &b as *const u8 as *const libc::c_void, 1);
    }
    b as i32
}

unsafe fn classify(x: f64) -> i16 {
    match x.classify() {
        std::num::FpCategory::Nan => 2,
        std::num::FpCategory::Infinite => 1,
        std::num::FpCategory::Zero => 0,
        std::num::FpCategory::Subnormal => -2,
        std::num::FpCategory::Normal => -1,
    }
}

crate::shims! { "crt";
    // ---- stdio -----------------------------------------------------------------------------------------------
    fn tmpnam(buf: LPSTR) -> LPSTR {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        static mut SHARED: [u8; 32] = [0; 32];
        let name = format!("\\s{:x}.{:x}", libc::getpid(), N.fetch_add(1, Ordering::SeqCst) + 1);
        let dst = if buf.is_null() { std::ptr::addr_of_mut!(SHARED) as *mut u8 } else { buf };
        std::ptr::copy_nonoverlapping(name.as_ptr(), dst, name.len());
        *dst.add(name.len()) = 0;
        dst
    }
    fn _locking(_fd: i32, _mode: i32, _bytes: i32) -> i32 {
        0
    }
    fn _popen(_cmd: LPCSTR, _mode: LPCSTR) -> usize {
        *errno_ptr() = libc::ENOSYS;
        0
    }
    fn _pclose(_fp: usize) -> i32 {
        -1
    }
    fn _kbhit() -> i32 {
        let mut p = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        (libc::poll(&mut p, 1, 0) > 0 && p.revents & libc::POLLIN != 0) as i32
    }
    // ---- conio -----------------------------------------------------------------------------------------------
    fn _getch() -> i32 {
        read_key(false)
    }
    fn _getche() -> i32 {
        read_key(true)
    }
    fn _getwch() -> u32 {
        read_key(false) as u32
    }
    fn _getwche() -> u32 {
        read_key(true) as u32
    }
    fn _putch(c: i32) -> i32 {
        let b = c as u8;
        libc::write(1, &b as *const u8 as *const libc::c_void, 1);
        c
    }
    fn _putwch(c: u16) -> u16 {
        let s = String::from_utf16_lossy(&[c]);
        libc::write(1, s.as_ptr() as *const libc::c_void, s.len());
        c
    }
    fn _ungetch(c: i32) -> i32 {
        UNGOT.with(|u| *u.get() = c);
        c
    }
    fn _ungetwch(c: u32) -> u32 {
        UNGOT.with(|u| *u.get() = c as i32);
        c
    }
    // ---- strings ---------------------------------------------------------------------------------------------
    fn _wcsrev(s: *mut u16) -> *mut u16 {
        let n = wlen(s);
        std::slice::from_raw_parts_mut(s, n).reverse();
        s
    }
    fn wcsncpy_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32 {
        if dst.is_null() || size == 0 {
            return libc::EINVAL;
        }
        if src.is_null() {
            *dst = 0;
            return libc::EINVAL;
        }
        let n = if count == usize::MAX { wlen(src).min(size - 1) } else { wlen(src).min(count) };
        if n + 1 > size {
            *dst = 0;
            return libc::ERANGE;
        }
        std::ptr::copy_nonoverlapping(src, dst, n);
        *dst.add(n) = 0;
        0
    }
    fn wcsxfrm(dst: *mut u16, src: *const u16, n: usize) -> usize {
        let l = wlen(src);
        if !dst.is_null() && n > l {
            std::ptr::copy_nonoverlapping(src, dst, l + 1);
        }
        l
    }
    fn wcstok_s(s: *mut u16, delim: *const u16, ctx: *mut *mut u16) -> *mut u16 {
        super::crt_string::wcstok(s, delim, ctx)
    }
    fn strtoimax(s: *const u8, end: *mut *mut u8, base: i32) -> i64 {
        super::crt_string::strtoll(s, end, base)
    }
    fn strtoumax(s: *const u8, end: *mut *mut u8, base: i32) -> u64 {
        super::crt_string::strtoull(s, end, base)
    }
    // ---- environment, files ----------------------------------------------------------------------------------
    fn _wputenv_s(name: LPCWSTR, value: LPCWSTR) -> i32 {
        runtime::env_set(&w_ptr_to_string(name), Some(&w_ptr_to_string(value)));
        0
    }
    fn _umask(mask: i32) -> i32 {
        libc::umask((mask & 0o777) as libc::mode_t) as i32
    }
    fn _wchmod(path: LPCWSTR, mode: i32) -> i32 {
        let host = paths::resolve(&w_ptr_to_string(path));
        let c = CString::new(host.to_string_lossy().as_bytes()).unwrap_or_default();
        let m = std::fs::metadata(&host).map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions())).unwrap_or(0o644);
        // _S_IWRITE (0x80) decides whether anyone may write
        let new = if mode & 0x80 != 0 { m | 0o200 } else { m & !0o222 };
        let r = libc::chmod(c.as_ptr(), new as libc::mode_t);
        if r != 0 {
            *errno_ptr() = *libc::__errno_location();
        }
        r
    }
    fn _wrmdir(path: LPCWSTR) -> i32 {
        let host = paths::resolve(&w_ptr_to_string(path));
        match std::fs::remove_dir(&host) {
            Ok(()) => 0,
            Err(e) => {
                *errno_ptr() = e.raw_os_error().unwrap_or(libc::EACCES);
                -1
            }
        }
    }
    fn _heapmin() -> i32 {
        0
    }
    // ---- the error message table (`_sys_errlist` / `_sys_nerr`) --------------------------------------------------
    fn __sys_nerr() -> *mut i32 {
        std::ptr::addr_of_mut!(SYS_NERR)
    }
    fn __sys_errlist() -> *const usize {
        *SYS_ERRLIST.get_or_init(|| {
            let list: Vec<usize> = (0..43).map(|e| CString::new(std::ffi::CStr::from_ptr(libc::strerror(e)).to_bytes()).unwrap().into_raw() as usize).collect();
            Box::leak(list.into_boxed_slice()).as_ptr() as usize
        }) as *const usize
    }
    // ---- time zone variables (UTC) -------------------------------------------------------------------------------
    fn __daylight() -> *mut i32 {
        std::ptr::addr_of_mut!(DAYLIGHT)
    }
    fn __timezone() -> *mut i32 {
        std::ptr::addr_of_mut!(TIMEZONE)
    }
    fn __dstbias() -> *mut i32 {
        std::ptr::addr_of_mut!(DSTBIAS)
    }
    // ---- math, runtime ---------------------------------------------------------------------------------------
    fn _dclass(x: f64) -> i16 {
        classify(x)
    }
    fn _fdclass(x: f32) -> i16 {
        classify(x as f64)
    }
    fn __fpe_flt_rounds() -> i32 {
        1
    }
    fn _set_thread_local_invalid_parameter_handler(_h: usize) -> usize {
        0
    }
    fn _register_thread_local_exe_atexit_callback(_cb: usize) {}
    fn _seh_filter_dll(_code: u32, _info: usize) -> i32 {
        0
    }
    fn terminate() {
        libc::fflush(std::ptr::null_mut());
        eprintln!("nle: terminate() called");
        libc::_exit(3)
    }
    // ---- the VC++ runtime's exception state ----------------------------------------------------------------------
    fn __current_exception() -> *mut usize {
        CURRENT_EXCEPTION.with(|c| c.get())
    }
    fn __current_exception_context() -> *mut usize {
        CURRENT_CONTEXT.with(|c| c.get())
    }
    fn __std_type_info_destroy_list(_root: usize) {}
    fn __DestructExceptionObject(_rec: usize) {}
}
