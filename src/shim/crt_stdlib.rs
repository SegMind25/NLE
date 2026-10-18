//! msvcrt / ucrt: allocation, conversion helpers, sorting, random numbers, environment and time.

use super::crt_startup::errno_ptr;
use crate::runtime::strings::*;
use crate::runtime::{self, paths};
use crate::win::*;
use std::cell::Cell;
use std::ffi::{c_void, CString};

type CmpFn = unsafe extern "win64" fn(*const u8, *const u8) -> i32;
type CmpFnS = unsafe extern "win64" fn(usize, *const u8, *const u8) -> i32;

thread_local! {
    static SEED: Cell<u32> = const { Cell::new(1) };
    static TM_BUF: std::cell::UnsafeCell<[i32; 9]> = const { std::cell::UnsafeCell::new([0; 9]) };
}

struct SortCtx {
    cmp: CmpFn,
}

unsafe extern "C" fn sort_trampoline(a: *const c_void, b: *const c_void, ctx: *mut c_void) -> libc::c_int {
    let c = &*(ctx as *const SortCtx);
    (c.cmp)(a as *const u8, b as *const u8)
}

struct SortCtxS {
    cmp: CmpFnS,
    user: usize,
}

unsafe extern "C" fn sort_trampoline_s(a: *const c_void, b: *const c_void, ctx: *mut c_void) -> libc::c_int {
    let c = &*(ctx as *const SortCtxS);
    (c.cmp)(c.user, a as *const u8, b as *const u8)
}

unsafe fn tm_to_libc(t: *const i32) -> libc::tm {
    let mut tm: libc::tm = std::mem::zeroed();
    tm.tm_sec = *t;
    tm.tm_min = *t.add(1);
    tm.tm_hour = *t.add(2);
    tm.tm_mday = *t.add(3);
    tm.tm_mon = *t.add(4);
    tm.tm_year = *t.add(5);
    tm.tm_wday = *t.add(6);
    tm.tm_yday = *t.add(7);
    tm.tm_isdst = *t.add(8);
    tm
}

unsafe fn tm_from_libc(dst: *mut i32, tm: &libc::tm) {
    *dst = tm.tm_sec;
    *dst.add(1) = tm.tm_min;
    *dst.add(2) = tm.tm_hour;
    *dst.add(3) = tm.tm_mday;
    *dst.add(4) = tm.tm_mon;
    *dst.add(5) = tm.tm_year;
    *dst.add(6) = tm.tm_wday;
    *dst.add(7) = tm.tm_yday;
    *dst.add(8) = tm.tm_isdst;
}

unsafe fn tm_static() -> *mut i32 {
    TM_BUF.with(|b| b.get() as *mut i32)
}

unsafe fn do_localtime(t: *const i64, local: bool, out: *mut i32) -> *mut i32 {
    if t.is_null() || *t < 0 {
        *errno_ptr() = 22;
        return std::ptr::null_mut();
    }
    let tt = *t as libc::time_t;
    let mut tm: libc::tm = std::mem::zeroed();
    if local {
        libc::localtime_r(&tt, &mut tm);
    } else {
        libc::gmtime_r(&tt, &mut tm);
    }
    tm_from_libc(out, &tm);
    out
}

fn now_secs() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };
    ts.tv_sec as i64
}

unsafe fn put_cstr(buf: *mut u8, s: &str) {
    std::ptr::copy_nonoverlapping(s.as_ptr(), buf, s.len());
    *buf.add(s.len()) = 0;
}

unsafe fn run_system(cmd: &str) -> i32 {
    let t = cmd.trim();
    let lower = t.to_ascii_lowercase();
    if lower == "pause" {
        print!("Press any key to continue . . . ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let mut b = [0u8; 1];
        libc::read(0, b.as_mut_ptr() as *mut c_void, 1);
        println!();
        return 0;
    }
    if lower == "cls" {
        print!("\x1b[2J\x1b[H");
        return 0;
    }
    let c = CString::new(t.replace('\\', "/")).unwrap_or_default();
    let r = libc::system(c.as_ptr());
    if r == -1 {
        -1
    } else {
        libc::WEXITSTATUS(r)
    }
}

crate::shims! { "crt";
    fn malloc(n: usize) -> *mut u8 {
        libc::malloc(n) as *mut u8
    }
    fn calloc(n: usize, size: usize) -> *mut u8 {
        libc::calloc(n, size) as *mut u8
    }
    fn realloc(p: *mut u8, n: usize) -> *mut u8 {
        libc::realloc(p as *mut c_void, n) as *mut u8
    }
    fn free(p: *mut u8) {
        libc::free(p as *mut c_void);
    }
    fn _msize(p: *mut u8) -> usize {
        libc::malloc_usable_size(p as *mut c_void)
    }
    fn _expand(_p: *mut u8, _n: usize) -> *mut u8 {
        std::ptr::null_mut()
    }
    fn _aligned_malloc(size: usize, align: usize) -> *mut u8 {
        let mut p: *mut c_void = std::ptr::null_mut();
        if libc::posix_memalign(&mut p, align.max(std::mem::size_of::<usize>()), size) != 0 {
            return std::ptr::null_mut();
        }
        p as *mut u8
    }
    fn _aligned_free(p: *mut u8) {
        libc::free(p as *mut c_void);
    }
    fn _aligned_realloc(p: *mut u8, size: usize, align: usize) -> *mut u8 {
        let n = _aligned_malloc(size, align);
        if !n.is_null() && !p.is_null() {
            let old = libc::malloc_usable_size(p as *mut c_void);
            std::ptr::copy_nonoverlapping(p, n, old.min(size));
            libc::free(p as *mut c_void);
        }
        n
    }
    fn _recalloc(p: *mut u8, n: usize, size: usize) -> *mut u8 {
        let old = if p.is_null() { 0 } else { libc::malloc_usable_size(p as *mut c_void) };
        let r = libc::realloc(p as *mut c_void, n * size) as *mut u8;
        if !r.is_null() && n * size > old {
            std::ptr::write_bytes(r.add(old), 0, n * size - old);
        }
        r
    }
    fn _callnewh(_n: usize) -> i32 {
        0
    }
    fn _get_heap_handle() -> usize {
        crate::runtime::teb::process_heap()
    }

    fn abs(v: i32) -> i32 {
        v.wrapping_abs()
    }
    fn labs(v: i32) -> i32 {
        v.wrapping_abs()
    }
    fn llabs(v: i64) -> i64 {
        v.wrapping_abs()
    }
    fn _abs64(v: i64) -> i64 {
        v.wrapping_abs()
    }
    fn rand() -> i32 {
        SEED.with(|s| {
            let n = s.get().wrapping_mul(214013).wrapping_add(2531011);
            s.set(n);
            ((n >> 16) & 0x7fff) as i32
        })
    }
    fn srand(seed: u32) {
        SEED.with(|s| s.set(seed));
    }
    fn rand_s(out: *mut u32) -> i32 {
        let mut v = 0u32;
        libc::getrandom(&mut v as *mut u32 as *mut c_void, 4, 0);
        *out = v;
        0
    }
    fn qsort(base: *mut u8, n: usize, size: usize, cmp: CmpFn) {
        let ctx = SortCtx { cmp };
        libc::qsort_r(base as *mut c_void, n, size, Some(sort_trampoline), &ctx as *const SortCtx as *mut c_void);
    }
    fn qsort_s(base: *mut u8, n: usize, size: usize, cmp: CmpFnS, user: usize) {
        let ctx = SortCtxS { cmp, user };
        libc::qsort_r(base as *mut c_void, n, size, Some(sort_trampoline_s), &ctx as *const SortCtxS as *mut c_void);
    }
    fn bsearch(key: *const u8, base: *const u8, n: usize, size: usize, cmp: CmpFn) -> *mut u8 {
        let (mut lo, mut hi) = (0usize, n);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let p = base.add(mid * size);
            let r = cmp(key, p);
            if r == 0 {
                return p as *mut u8;
            }
            if r < 0 {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        std::ptr::null_mut()
    }

    fn getenv(name: LPCSTR) -> *mut u8 {
        match runtime::env_get(&a_to_string(name)) {
            Some(v) => {
                let c = CString::new(v).unwrap_or_default();
                libc::strdup(c.as_ptr()) as *mut u8
            }
            None => std::ptr::null_mut(),
        }
    }
    fn _wgetenv(name: LPCWSTR) -> *mut u16 {
        match runtime::env_get(&w_ptr_to_string(name)) {
            Some(v) => {
                let w = to_wide_nul(&v);
                let p = libc::malloc(w.len() * 2) as *mut u16;
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                p
            }
            None => std::ptr::null_mut(),
        }
    }
    fn getenv_s(len: *mut usize, buf: LPSTR, size: usize, name: LPCSTR) -> i32 {
        match runtime::env_get(&a_to_string(name)) {
            Some(v) => {
                if !len.is_null() {
                    *len = v.len() + 1;
                }
                if buf.is_null() || size < v.len() + 1 {
                    return 34;
                }
                put_cstr(buf, &v);
                0
            }
            None => {
                if !len.is_null() {
                    *len = 0;
                }
                0
            }
        }
    }
    fn _dupenv_s(buf: *mut *mut u8, len: *mut usize, name: LPCSTR) -> i32 {
        match runtime::env_get(&a_to_string(name)) {
            Some(v) => {
                let c = CString::new(v).unwrap_or_default();
                *buf = libc::strdup(c.as_ptr()) as *mut u8;
                if !len.is_null() {
                    *len = c.as_bytes().len() + 1;
                }
            }
            None => {
                *buf = std::ptr::null_mut();
                if !len.is_null() {
                    *len = 0;
                }
            }
        }
        0
    }
    fn _putenv(s: LPCSTR) -> i32 {
        let t = a_to_string(s);
        match t.split_once('=') {
            Some((k, v)) if !k.is_empty() => {
                runtime::env_set(k, if v.is_empty() { None } else { Some(v) });
                0
            }
            _ => -1,
        }
    }
    fn _wputenv(s: LPCWSTR) -> i32 {
        let t = w_ptr_to_string(s);
        match t.split_once('=') {
            Some((k, v)) if !k.is_empty() => {
                runtime::env_set(k, if v.is_empty() { None } else { Some(v) });
                0
            }
            _ => -1,
        }
    }
    fn _putenv_s(name: LPCSTR, value: LPCSTR) -> i32 {
        runtime::env_set(&a_to_string(name), Some(&a_to_string(value)));
        0
    }
    fn system(cmd: LPCSTR) -> i32 {
        if cmd.is_null() {
            return 1;
        }
        run_system(&a_to_string(cmd))
    }
    fn _wsystem(cmd: LPCWSTR) -> i32 {
        run_system(&w_ptr_to_string(cmd))
    }
    fn _sleep(ms: u32) {
        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
    }
    fn _swab(src: *const u8, dst: *mut u8, n: i32) {
        let mut i = 0;
        while i + 1 < n as usize {
            *dst.add(i) = *src.add(i + 1);
            *dst.add(i + 1) = *src.add(i);
            i += 2;
        }
    }
    fn _rotl(v: u32, s: i32) -> u32 {
        v.rotate_left(s as u32 & 31)
    }
    fn _rotr(v: u32, s: i32) -> u32 {
        v.rotate_right(s as u32 & 31)
    }
    fn _rotl64(v: u64, s: i32) -> u64 {
        v.rotate_left(s as u32 & 63)
    }
    fn _byteswap_ulong(v: u32) -> u32 {
        v.swap_bytes()
    }
    fn _byteswap_uint64(v: u64) -> u64 {
        v.swap_bytes()
    }
    fn _byteswap_ushort(v: u16) -> u16 {
        v.swap_bytes()
    }

    fn time(out: *mut i64) -> i64 {
        let t = now_secs();
        if !out.is_null() {
            *out = t;
        }
        t
    }
    fn _time64(out: *mut i64) -> i64 {
        time(out)
    }
    fn clock() -> i32 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts);
        (ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000) as i32
    }
    fn difftime(a: i64, b: i64) -> f64 {
        (a - b) as f64
    }
    fn _difftime64(a: i64, b: i64) -> f64 {
        (a - b) as f64
    }
    fn localtime(t: *const i64) -> *mut i32 {
        do_localtime(t, true, tm_static())
    }
    fn _localtime64(t: *const i64) -> *mut i32 {
        do_localtime(t, true, tm_static())
    }
    fn gmtime(t: *const i64) -> *mut i32 {
        do_localtime(t, false, tm_static())
    }
    fn _gmtime64(t: *const i64) -> *mut i32 {
        do_localtime(t, false, tm_static())
    }
    fn _localtime64_s(out: *mut i32, t: *const i64) -> i32 {
        if do_localtime(t, true, out).is_null() { 22 } else { 0 }
    }
    fn _gmtime64_s(out: *mut i32, t: *const i64) -> i32 {
        if do_localtime(t, false, out).is_null() { 22 } else { 0 }
    }
    fn localtime_s(out: *mut i32, t: *const i64) -> i32 {
        _localtime64_s(out, t)
    }
    fn gmtime_s(out: *mut i32, t: *const i64) -> i32 {
        _gmtime64_s(out, t)
    }
    fn mktime(tm: *mut i32) -> i64 {
        let mut t = tm_to_libc(tm);
        let r = libc::mktime(&mut t);
        tm_from_libc(tm, &t);
        r as i64
    }
    fn _mktime64(tm: *mut i32) -> i64 {
        mktime(tm)
    }
    fn _mkgmtime64(tm: *mut i32) -> i64 {
        let mut t = tm_to_libc(tm);
        libc::timegm(&mut t) as i64
    }
    fn strftime(buf: *mut u8, max: usize, fmt: LPCSTR, tm: *const i32) -> usize {
        let t = tm_to_libc(tm);
        libc::strftime(buf as *mut libc::c_char, max, fmt as *const libc::c_char, &t)
    }
    fn wcsftime(buf: *mut u16, max: usize, fmt: LPCWSTR, tm: *const i32) -> usize {
        let t = tm_to_libc(tm);
        let f = CString::new(w_ptr_to_string(fmt)).unwrap_or_default();
        let mut tmp = vec![0u8; max * 4 + 8];
        let n = libc::strftime(tmp.as_mut_ptr() as *mut libc::c_char, tmp.len(), f.as_ptr(), &t);
        let w: Vec<u16> = String::from_utf8_lossy(&tmp[..n]).encode_utf16().collect();
        if w.len() + 1 > max {
            return 0;
        }
        std::ptr::copy_nonoverlapping(w.as_ptr(), buf, w.len());
        *buf.add(w.len()) = 0;
        w.len()
    }
    fn asctime(tm: *const i32) -> *const u8 {
        static mut BUF: [u8; 32] = [0; 32];
        let t = tm_to_libc(tm);
        libc::asctime_r(&t, std::ptr::addr_of_mut!(BUF) as *mut libc::c_char);
        std::ptr::addr_of!(BUF) as *const u8
    }
    fn ctime(t: *const i64) -> *const u8 {
        static mut BUF: [u8; 32] = [0; 32];
        let tt = *t as libc::time_t;
        libc::ctime_r(&tt, std::ptr::addr_of_mut!(BUF) as *mut libc::c_char);
        std::ptr::addr_of!(BUF) as *const u8
    }
    fn _ctime64(t: *const i64) -> *const u8 {
        ctime(t)
    }
    fn _tzset() {}
    fn _ftime64(out: *mut u8) {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
        *(out as *mut i64) = ts.tv_sec as i64;
        *(out.add(8) as *mut u16) = (ts.tv_nsec / 1_000_000) as u16;
        *(out.add(10) as *mut i16) = 0;
        *(out.add(12) as *mut i16) = 0;
    }
    fn _get_timezone(out: *mut i32) -> i32 {
        *out = 0;
        0
    }
    fn _get_daylight(out: *mut i32) -> i32 {
        *out = 0;
        0
    }
    fn __p__daylight() -> *mut i32 {
        std::ptr::addr_of_mut!(super::crt_startup::DAYLIGHT)
    }
    fn __p__timezone() -> *mut i32 {
        std::ptr::addr_of_mut!(super::crt_startup::TIMEZONE)
    }
    fn __p__tzname() -> *mut usize {
        std::ptr::addr_of_mut!(super::crt_startup::TZNAME) as *mut usize
    }
    fn _strdate(buf: *mut u8) -> *mut u8 {
        let t = now_secs() as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        put_cstr(buf, &format!("{:02}/{:02}/{:02}", tm.tm_mon + 1, tm.tm_mday, tm.tm_year % 100));
        buf
    }
    fn _strtime(buf: *mut u8) -> *mut u8 {
        let t = now_secs() as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        put_cstr(buf, &format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec));
        buf
    }
    fn _getcwd_dummy() {}
    fn _wgetcwd(buf: LPWSTR, n: i32) -> LPWSTR {
        let cwd = to_wide_nul(&paths::cwd_win());
        if cwd.len() > n as usize {
            return std::ptr::null_mut();
        }
        std::ptr::copy_nonoverlapping(cwd.as_ptr(), buf, cwd.len());
        buf
    }
    fn div(a: i32, b: i32) -> u64 {
        let q = a.wrapping_div(b);
        let r = a.wrapping_rem(b);
        (q as u32 as u64) | ((r as u32 as u64) << 32)
    }
    fn _purecall() {
        eprintln!("R6025\n- pure virtual function call");
        crate::loader::exit_process(255);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msvc_rand_sequence() {
        SEED.with(|s| s.set(1));
        let seq: Vec<i32> = (0..5).map(|_| unsafe { rand() }).collect();
        assert_eq!(seq, vec![41, 18467, 6334, 26500, 19169]);
    }

    #[test]
    fn div_packs_quotient_and_remainder() {
        let v = unsafe { div(17, 5) };
        assert_eq!(v as u32, 3);
        assert_eq!((v >> 32) as u32, 2);
    }
}
