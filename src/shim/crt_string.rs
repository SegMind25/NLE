//! msvcrt / ucrt: memory, narrow and wide string functions, character classification.

use crate::runtime::strings::*;
use crate::win::*;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::OnceLock;

const EINVAL: i32 = 22;
const ERANGE: i32 = 34;

pub fn set_errno(e: i32) {
    unsafe { *super::crt_startup::errno_ptr() = e };
}

unsafe fn slen(p: *const u8) -> usize {
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    n
}

fn lower(b: u8) -> u8 {
    b.to_ascii_lowercase()
}

unsafe fn cmp_bytes(a: *const u8, b: *const u8, n: usize, fold: bool) -> i32 {
    for i in 0..n {
        let (mut x, mut y) = (*a.add(i), *b.add(i));
        if fold {
            x = lower(x);
            y = lower(y);
        }
        if x != y {
            return if x < y { -1 } else { 1 };
        }
        if x == 0 {
            return 0;
        }
    }
    0
}

unsafe fn cmp_wide(a: *const u16, b: *const u16, n: usize, fold: bool) -> i32 {
    for i in 0..n {
        let (mut x, mut y) = (*a.add(i), *b.add(i));
        if fold {
            x = lower_unit(upper_unit(x));
            y = lower_unit(upper_unit(y));
        }
        if x != y {
            return if x < y { -1 } else { 1 };
        }
        if x == 0 {
            return 0;
        }
    }
    0
}

struct CtypeTable(UnsafeCell<[u16; 257]>);
unsafe impl Sync for CtypeTable {}

fn ctype_table() -> *const u16 {
    static T: OnceLock<CtypeTable> = OnceLock::new();
    let t = T.get_or_init(|| {
        let mut tab = [0u16; 257];
        for i in 0..256usize {
            let c = i as u8;
            let mut v = 0u16;
            if c.is_ascii_uppercase() {
                v |= 0x1 | 0x100;
            }
            if c.is_ascii_lowercase() {
                v |= 0x2 | 0x100;
            }
            if c.is_ascii_digit() {
                v |= 0x4;
            }
            if c.is_ascii_whitespace() || c == 0x0b {
                v |= 0x8;
            }
            if c.is_ascii_punctuation() {
                v |= 0x10;
            }
            if c.is_ascii_control() {
                v |= 0x20;
            }
            if c == b' ' || c == b'\t' {
                v |= 0x40;
            }
            if c.is_ascii_hexdigit() {
                v |= 0x80;
            }
            tab[i + 1] = v;
        }
        CtypeTable(UnsafeCell::new(tab))
    });
    t.0.get() as *const u16
}

pub static mut PCTYPE: usize = 0;
pub static mut MB_CUR_MAX: i32 = 1;

pub fn init_data() {
    unsafe { PCTYPE = ctype_table().add(1) as usize };
}

fn cls(c: i32, mask: u16) -> i32 {
    if !(-1..=255).contains(&c) {
        return 0;
    }
    unsafe { (*ctype_table().add((c + 1) as usize) & mask) as i32 }
}

unsafe fn strtol_impl(s: *const u8, end: *mut *mut u8, base: i32, signed: bool, bits64: bool) -> u64 {
    let r = libc::strtoull(s as *const libc::c_char, end as *mut *mut libc::c_char, base);
    let _ = (signed, bits64);
    r
}

unsafe fn parse_signed(s: *const u8, end: *mut *mut u8, base: i32, min: i64, max: i64) -> i64 {
    *super::crt_startup::errno_ptr() = 0;
    let v = libc::strtoll(s as *const libc::c_char, end as *mut *mut libc::c_char, base);
    if v < min {
        set_errno(ERANGE);
        return min;
    }
    if v > max {
        set_errno(ERANGE);
        return max;
    }
    if std::io::Error::last_os_error().raw_os_error() == Some(libc::ERANGE) {
        set_errno(ERANGE);
    }
    v
}

unsafe fn parse_unsigned(s: *const u8, end: *mut *mut u8, base: i32, max: u64) -> u64 {
    *super::crt_startup::errno_ptr() = 0;
    let v = strtol_impl(s, end, base, false, true);
    if v > max {
        set_errno(ERANGE);
        return max;
    }
    v
}

fn wide_to_ansi_lossy(p: *const u16) -> Vec<u8> {
    unsafe { w_to_string(wstr(p)).into_bytes() }
}

unsafe fn wcs_num_prefix(p: *const u16, end: *mut *mut u16, f: impl FnOnce(*const u8, *mut *mut u8) -> f64) -> f64 {
    let b = wide_to_ansi_lossy(p);
    let mut c = b.clone();
    c.push(0);
    let mut e: *mut u8 = std::ptr::null_mut();
    let v = f(c.as_ptr(), &mut e);
    if !end.is_null() {
        let consumed = e as usize - c.as_ptr() as usize;
        let units = String::from_utf8_lossy(&b[..consumed.min(b.len())]).encode_utf16().count();
        *end = p.add(units) as *mut u16;
    }
    v
}

crate::shims! { "crt";
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        std::ptr::copy_nonoverlapping(src, dst, n);
        dst
    }
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        std::ptr::copy(src, dst, n);
        dst
    }
    fn memset(dst: *mut u8, c: i32, n: usize) -> *mut u8 {
        std::ptr::write_bytes(dst, c as u8, n);
        dst
    }
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
        libc::memcmp(a as *const c_void, b as *const c_void, n)
    }
    fn memchr(p: *const u8, c: i32, n: usize) -> *mut u8 {
        libc::memchr(p as *const c_void, c, n) as *mut u8
    }
    fn _memicmp(a: *const u8, b: *const u8, n: usize) -> i32 {
        for i in 0..n {
            let (x, y) = (lower(*a.add(i)), lower(*b.add(i)));
            if x != y {
                return if x < y { -1 } else { 1 };
            }
        }
        0
    }
    fn memcpy_s(dst: *mut u8, dst_size: usize, src: *const u8, n: usize) -> i32 {
        if n == 0 {
            return 0;
        }
        if dst.is_null() || src.is_null() {
            return EINVAL;
        }
        if dst_size < n {
            std::ptr::write_bytes(dst, 0, dst_size);
            return ERANGE;
        }
        std::ptr::copy(src, dst, n);
        0
    }
    fn memmove_s(dst: *mut u8, dst_size: usize, src: *const u8, n: usize) -> i32 {
        memcpy_s(dst, dst_size, src, n)
    }

    fn strlen(s: *const u8) -> usize {
        slen(s)
    }
    fn strnlen(s: *const u8, max: usize) -> usize {
        let mut n = 0;
        while n < max && *s.add(n) != 0 {
            n += 1;
        }
        n
    }
    fn strcpy(dst: *mut u8, src: *const u8) -> *mut u8 {
        std::ptr::copy_nonoverlapping(src, dst, slen(src) + 1);
        dst
    }
    fn strncpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        let l = strnlen(src, n);
        std::ptr::copy_nonoverlapping(src, dst, l);
        std::ptr::write_bytes(dst.add(l), 0, n - l);
        dst
    }
    fn strcat(dst: *mut u8, src: *const u8) -> *mut u8 {
        std::ptr::copy_nonoverlapping(src, dst.add(slen(dst)), slen(src) + 1);
        dst
    }
    fn strncat(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        let d = slen(dst);
        let l = strnlen(src, n);
        std::ptr::copy_nonoverlapping(src, dst.add(d), l);
        *dst.add(d + l) = 0;
        dst
    }
    fn strcpy_s(dst: *mut u8, size: usize, src: *const u8) -> i32 {
        if dst.is_null() || size == 0 {
            return EINVAL;
        }
        if src.is_null() {
            *dst = 0;
            return EINVAL;
        }
        let l = slen(src);
        if l + 1 > size {
            *dst = 0;
            return ERANGE;
        }
        std::ptr::copy_nonoverlapping(src, dst, l + 1);
        0
    }
    fn strncpy_s(dst: *mut u8, size: usize, src: *const u8, count: usize) -> i32 {
        if dst.is_null() || size == 0 {
            return EINVAL;
        }
        if src.is_null() {
            *dst = 0;
            return EINVAL;
        }
        let l = strnlen(src, count);
        if l + 1 > size {
            *dst = 0;
            return ERANGE;
        }
        std::ptr::copy_nonoverlapping(src, dst, l);
        *dst.add(l) = 0;
        0
    }
    fn strcat_s(dst: *mut u8, size: usize, src: *const u8) -> i32 {
        if dst.is_null() || src.is_null() {
            return EINVAL;
        }
        let d = strnlen(dst, size);
        if d == size {
            return EINVAL;
        }
        let l = slen(src);
        if d + l + 1 > size {
            *dst = 0;
            return ERANGE;
        }
        std::ptr::copy_nonoverlapping(src, dst.add(d), l + 1);
        0
    }
    fn strcmp(a: *const u8, b: *const u8) -> i32 {
        cmp_bytes(a, b, usize::MAX, false)
    }
    fn strncmp(a: *const u8, b: *const u8, n: usize) -> i32 {
        cmp_bytes(a, b, n, false)
    }
    fn _stricmp(a: *const u8, b: *const u8) -> i32 {
        cmp_bytes(a, b, usize::MAX, true)
    }
    fn _strnicmp(a: *const u8, b: *const u8, n: usize) -> i32 {
        cmp_bytes(a, b, n, true)
    }
    fn stricmp(a: *const u8, b: *const u8) -> i32 {
        cmp_bytes(a, b, usize::MAX, true)
    }
    fn strcoll(a: *const u8, b: *const u8) -> i32 {
        cmp_bytes(a, b, usize::MAX, false)
    }
    fn strchr(s: *const u8, c: i32) -> *mut u8 {
        libc::strchr(s as *const libc::c_char, c) as *mut u8
    }
    fn strrchr(s: *const u8, c: i32) -> *mut u8 {
        libc::strrchr(s as *const libc::c_char, c) as *mut u8
    }
    fn strstr(h: *const u8, n: *const u8) -> *mut u8 {
        libc::strstr(h as *const libc::c_char, n as *const libc::c_char) as *mut u8
    }
    fn strpbrk(s: *const u8, set: *const u8) -> *mut u8 {
        libc::strpbrk(s as *const libc::c_char, set as *const libc::c_char) as *mut u8
    }
    fn strspn(s: *const u8, set: *const u8) -> usize {
        libc::strspn(s as *const libc::c_char, set as *const libc::c_char)
    }
    fn strcspn(s: *const u8, set: *const u8) -> usize {
        libc::strcspn(s as *const libc::c_char, set as *const libc::c_char)
    }
    fn strdup(s: *const u8) -> *mut u8 {
        libc::strdup(s as *const libc::c_char) as *mut u8
    }
    fn _strdup(s: *const u8) -> *mut u8 {
        libc::strdup(s as *const libc::c_char) as *mut u8
    }
    fn strtok(s: *mut u8, delim: *const u8) -> *mut u8 {
        libc::strtok(s as *mut libc::c_char, delim as *const libc::c_char) as *mut u8
    }
    fn strtok_s(s: *mut u8, delim: *const u8, ctx: *mut *mut u8) -> *mut u8 {
        libc::strtok_r(s as *mut libc::c_char, delim as *const libc::c_char, ctx as *mut *mut libc::c_char) as *mut u8
    }
    fn _strlwr(s: *mut u8) -> *mut u8 {
        let mut i = 0;
        while *s.add(i) != 0 {
            *s.add(i) = lower(*s.add(i));
            i += 1;
        }
        s
    }
    fn _strupr(s: *mut u8) -> *mut u8 {
        let mut i = 0;
        while *s.add(i) != 0 {
            *s.add(i) = (*s.add(i)).to_ascii_uppercase();
            i += 1;
        }
        s
    }
    fn _strrev(s: *mut u8) -> *mut u8 {
        std::slice::from_raw_parts_mut(s, slen(s)).reverse();
        s
    }
    fn _strset(s: *mut u8, c: i32) -> *mut u8 {
        let n = slen(s);
        std::ptr::write_bytes(s, c as u8, n);
        s
    }

    fn wcslen(s: *const u16) -> usize {
        wlen(s)
    }
    fn wcsnlen(s: *const u16, max: usize) -> usize {
        let mut n = 0;
        while n < max && *s.add(n) != 0 {
            n += 1;
        }
        n
    }
    fn wcscpy(dst: *mut u16, src: *const u16) -> *mut u16 {
        std::ptr::copy_nonoverlapping(src, dst, wlen(src) + 1);
        dst
    }
    fn wcsncpy(dst: *mut u16, src: *const u16, n: usize) -> *mut u16 {
        let l = wcsnlen(src, n);
        std::ptr::copy_nonoverlapping(src, dst, l);
        for i in l..n {
            *dst.add(i) = 0;
        }
        dst
    }
    fn wcscat(dst: *mut u16, src: *const u16) -> *mut u16 {
        std::ptr::copy_nonoverlapping(src, dst.add(wlen(dst)), wlen(src) + 1);
        dst
    }
    fn wcsncat(dst: *mut u16, src: *const u16, n: usize) -> *mut u16 {
        let d = wlen(dst);
        let l = wcsnlen(src, n);
        std::ptr::copy_nonoverlapping(src, dst.add(d), l);
        *dst.add(d + l) = 0;
        dst
    }
    fn wcscpy_s(dst: *mut u16, size: usize, src: *const u16) -> i32 {
        if dst.is_null() || size == 0 {
            return EINVAL;
        }
        if src.is_null() {
            *dst = 0;
            return EINVAL;
        }
        let l = wlen(src);
        if l + 1 > size {
            *dst = 0;
            return ERANGE;
        }
        std::ptr::copy_nonoverlapping(src, dst, l + 1);
        0
    }
    fn wcscat_s(dst: *mut u16, size: usize, src: *const u16) -> i32 {
        if dst.is_null() || src.is_null() {
            return EINVAL;
        }
        let d = wcsnlen(dst, size);
        if d == size {
            return EINVAL;
        }
        let l = wlen(src);
        if d + l + 1 > size {
            *dst = 0;
            return ERANGE;
        }
        std::ptr::copy_nonoverlapping(src, dst.add(d), l + 1);
        0
    }
    fn wcscmp(a: *const u16, b: *const u16) -> i32 {
        cmp_wide(a, b, usize::MAX, false)
    }
    fn wcsncmp(a: *const u16, b: *const u16, n: usize) -> i32 {
        cmp_wide(a, b, n, false)
    }
    fn _wcsicmp(a: *const u16, b: *const u16) -> i32 {
        cmp_wide(a, b, usize::MAX, true)
    }
    fn _wcsnicmp(a: *const u16, b: *const u16, n: usize) -> i32 {
        cmp_wide(a, b, n, true)
    }
    fn wcscoll(a: *const u16, b: *const u16) -> i32 {
        cmp_wide(a, b, usize::MAX, false)
    }
    fn wcschr(s: *const u16, c: u16) -> *mut u16 {
        let mut p = s;
        loop {
            if *p == c {
                return p as *mut u16;
            }
            if *p == 0 {
                return std::ptr::null_mut();
            }
            p = p.add(1);
        }
    }
    fn wcsrchr(s: *const u16, c: u16) -> *mut u16 {
        let mut p = s;
        let mut last = std::ptr::null_mut();
        loop {
            if *p == c {
                last = p as *mut u16;
            }
            if *p == 0 {
                return last;
            }
            p = p.add(1);
        }
    }
    fn wcsstr(h: *const u16, n: *const u16) -> *mut u16 {
        let nl = wlen(n);
        if nl == 0 {
            return h as *mut u16;
        }
        let hl = wlen(h);
        if nl > hl {
            return std::ptr::null_mut();
        }
        for i in 0..=(hl - nl) {
            if std::slice::from_raw_parts(h.add(i), nl) == std::slice::from_raw_parts(n, nl) {
                return h.add(i) as *mut u16;
            }
        }
        std::ptr::null_mut()
    }
    fn wcsspn(s: *const u16, set: *const u16) -> usize {
        let set = wstr(set);
        let mut n = 0;
        while *s.add(n) != 0 && set.contains(&*s.add(n)) {
            n += 1;
        }
        n
    }
    fn wcscspn(s: *const u16, set: *const u16) -> usize {
        let set = wstr(set);
        let mut n = 0;
        while *s.add(n) != 0 && !set.contains(&*s.add(n)) {
            n += 1;
        }
        n
    }
    fn wcspbrk(s: *const u16, set: *const u16) -> *mut u16 {
        let n = wcscspn(s, set);
        if *s.add(n) == 0 { std::ptr::null_mut() } else { s.add(n) as *mut u16 }
    }
    fn _wcsdup(s: *const u16) -> *mut u16 {
        let l = wlen(s) + 1;
        let p = libc::malloc(l * 2) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(s, p, l);
        }
        p
    }
    fn wcstok(s: *mut u16, delim: *const u16, ctx: *mut *mut u16) -> *mut u16 {
        let mut p = if s.is_null() { *ctx } else { s };
        if p.is_null() {
            return std::ptr::null_mut();
        }
        let d = wstr(delim);
        while *p != 0 && d.contains(&*p) {
            p = p.add(1);
        }
        if *p == 0 {
            *ctx = std::ptr::null_mut();
            return std::ptr::null_mut();
        }
        let start = p;
        while *p != 0 && !d.contains(&*p) {
            p = p.add(1);
        }
        if *p != 0 {
            *p = 0;
            *ctx = p.add(1);
        } else {
            *ctx = std::ptr::null_mut();
        }
        start
    }
    fn _wcslwr(s: *mut u16) -> *mut u16 {
        let mut i = 0;
        while *s.add(i) != 0 {
            *s.add(i) = lower_unit(*s.add(i));
            i += 1;
        }
        s
    }
    fn _wcsupr(s: *mut u16) -> *mut u16 {
        let mut i = 0;
        while *s.add(i) != 0 {
            *s.add(i) = upper_unit(*s.add(i));
            i += 1;
        }
        s
    }
    fn wmemcpy(dst: *mut u16, src: *const u16, n: usize) -> *mut u16 {
        std::ptr::copy_nonoverlapping(src, dst, n);
        dst
    }
    fn wmemmove(dst: *mut u16, src: *const u16, n: usize) -> *mut u16 {
        std::ptr::copy(src, dst, n);
        dst
    }
    fn wmemset(dst: *mut u16, c: u16, n: usize) -> *mut u16 {
        for i in 0..n {
            *dst.add(i) = c;
        }
        dst
    }
    fn wmemcmp(a: *const u16, b: *const u16, n: usize) -> i32 {
        for i in 0..n {
            let (x, y) = (*a.add(i), *b.add(i));
            if x != y {
                return if x < y { -1 } else { 1 };
            }
        }
        0
    }
    fn wmemchr(s: *const u16, c: u16, n: usize) -> *mut u16 {
        for i in 0..n {
            if *s.add(i) == c {
                return s.add(i) as *mut u16;
            }
        }
        std::ptr::null_mut()
    }
    fn towlower(c: u16) -> u16 {
        lower_unit(c)
    }
    fn towupper(c: u16) -> u16 {
        upper_unit(c)
    }
    fn iswalpha(c: u16) -> i32 {
        char::from_u32(c as u32).is_some_and(|c| c.is_alphabetic()) as i32
    }
    fn iswdigit(c: u16) -> i32 {
        (c >= b'0' as u16 && c <= b'9' as u16) as i32
    }
    fn iswalnum(c: u16) -> i32 {
        char::from_u32(c as u32).is_some_and(|c| c.is_alphanumeric()) as i32
    }
    fn iswspace(c: u16) -> i32 {
        char::from_u32(c as u32).is_some_and(|c| c.is_whitespace()) as i32
    }
    fn iswupper(c: u16) -> i32 {
        char::from_u32(c as u32).is_some_and(|c| c.is_uppercase()) as i32
    }
    fn iswlower(c: u16) -> i32 {
        char::from_u32(c as u32).is_some_and(|c| c.is_lowercase()) as i32
    }
    fn iswpunct(c: u16) -> i32 {
        char::from_u32(c as u32).is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control()) as i32
    }
    fn iswxdigit(c: u16) -> i32 {
        char::from_u32(c as u32).is_some_and(|c| c.is_ascii_hexdigit()) as i32
    }
    fn iswcntrl(c: u16) -> i32 {
        char::from_u32(c as u32).is_some_and(|c| c.is_control()) as i32
    }
    fn iswprint(c: u16) -> i32 {
        char::from_u32(c as u32).is_some_and(|c| !c.is_control()) as i32
    }
    fn iswgraph(c: u16) -> i32 {
        char::from_u32(c as u32).is_some_and(|c| !c.is_control() && !c.is_whitespace()) as i32
    }
    fn iswctype(c: u16, mask: u16) -> i32 {
        ((ctype1_mask(c) & mask) != 0) as i32
    }
    fn is_wctype(c: u16, mask: u16) -> i32 {
        ((ctype1_mask(c) & mask) != 0) as i32
    }

    fn isalpha(c: i32) -> i32 {
        cls(c, 0x103)
    }
    fn isupper(c: i32) -> i32 {
        cls(c, 0x1)
    }
    fn islower(c: i32) -> i32 {
        cls(c, 0x2)
    }
    fn isdigit(c: i32) -> i32 {
        cls(c, 0x4)
    }
    fn isxdigit(c: i32) -> i32 {
        cls(c, 0x80)
    }
    fn isspace(c: i32) -> i32 {
        cls(c, 0x8)
    }
    fn ispunct(c: i32) -> i32 {
        cls(c, 0x10)
    }
    fn isalnum(c: i32) -> i32 {
        cls(c, 0x107)
    }
    fn isprint(c: i32) -> i32 {
        cls(c, 0x157)
    }
    fn isgraph(c: i32) -> i32 {
        cls(c, 0x117)
    }
    fn iscntrl(c: i32) -> i32 {
        cls(c, 0x20)
    }
    fn isblank(c: i32) -> i32 {
        cls(c, 0x40)
    }
    fn isascii(c: i32) -> i32 {
        ((c as u32) < 0x80) as i32
    }
    fn __isascii(c: i32) -> i32 {
        ((c as u32) < 0x80) as i32
    }
    fn toupper(c: i32) -> i32 {
        if (b'a' as i32..=b'z' as i32).contains(&c) { c - 32 } else { c }
    }
    fn tolower(c: i32) -> i32 {
        if (b'A' as i32..=b'Z' as i32).contains(&c) { c + 32 } else { c }
    }
    fn _toupper(c: i32) -> i32 {
        c - 32
    }
    fn _tolower(c: i32) -> i32 {
        c + 32
    }
    fn _isctype(c: i32, mask: i32) -> i32 {
        cls(c, mask as u16)
    }
    fn __pctype_func() -> *const u16 {
        ctype_table().add(1)
    }
    fn ___mb_cur_max_func() -> i32 {
        1
    }
    fn ___lc_codepage_func() -> u32 {
        0
    }
    fn ___lc_locale_name_func() -> *const u8 {
        std::ptr::null()
    }
    fn __mb_cur_max_dummy() -> i32 {
        1
    }
    fn _isleadbyte_l_dummy() -> i32 {
        0
    }
    fn isleadbyte(_c: i32) -> i32 {
        0
    }
    fn _ismbblead(_c: u32) -> i32 {
        0
    }
    fn _ismbbtrail(_c: u32) -> i32 {
        0
    }
    fn _mbstrlen(s: *const u8) -> usize {
        a_to_string(s).chars().count()
    }
    fn _mbslen(s: *const u8) -> usize {
        a_to_string(s).chars().count()
    }
    fn mblen(s: *const u8, n: usize) -> i32 {
        if s.is_null() {
            return 0;
        }
        if n == 0 || *s == 0 {
            return 0;
        }
        let b = std::slice::from_raw_parts(s, n.min(4));
        let need = match b[0] {
            0..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => return 1,
        };
        if b.len() >= need && std::str::from_utf8(&b[..need]).is_ok() { need as i32 } else { 1 }
    }

    fn mbstowcs(dst: *mut u16, src: *const u8, n: usize) -> usize {
        let w = mb_to_wide(CP_UTF8, astr(src), false).unwrap_or_default();
        if dst.is_null() {
            return w.len();
        }
        let k = w.len().min(n);
        std::ptr::copy_nonoverlapping(w.as_ptr(), dst, k);
        if k < n {
            *dst.add(k) = 0;
        }
        k
    }
    fn wcstombs(dst: *mut u8, src: *const u16, n: usize) -> usize {
        let mut d = false;
        let b = wide_to_mb(CP_UTF8, wstr(src), false, &mut d).unwrap_or_default();
        if dst.is_null() {
            return b.len();
        }
        let k = b.len().min(n);
        std::ptr::copy_nonoverlapping(b.as_ptr(), dst, k);
        if k < n {
            *dst.add(k) = 0;
        }
        k
    }
    fn mbtowc(dst: *mut u16, src: *const u8, n: usize) -> i32 {
        if src.is_null() {
            return 0;
        }
        let l = mblen(src, n);
        if l <= 0 {
            if !dst.is_null() {
                *dst = 0;
            }
            return 0;
        }
        let w = mb_to_wide(CP_UTF8, std::slice::from_raw_parts(src, l as usize), false).unwrap_or_default();
        if !dst.is_null() && !w.is_empty() {
            *dst = w[0];
        }
        l
    }
    fn wctomb(dst: *mut u8, c: u16) -> i32 {
        if dst.is_null() {
            return 0;
        }
        let mut d = false;
        let b = wide_to_mb(CP_UTF8, &[c], false, &mut d).unwrap_or_default();
        std::ptr::copy_nonoverlapping(b.as_ptr(), dst, b.len());
        b.len() as i32
    }
    fn wctomb_s(ret: *mut i32, dst: *mut u8, size: usize, c: u16) -> i32 {
        let mut d = false;
        let b = wide_to_mb(CP_UTF8, &[c], false, &mut d).unwrap_or_default();
        if dst.is_null() || size < b.len() {
            return ERANGE;
        }
        std::ptr::copy_nonoverlapping(b.as_ptr(), dst, b.len());
        if !ret.is_null() {
            *ret = b.len() as i32;
        }
        0
    }
    fn btowc(c: i32) -> u16 {
        if c == -1 { 0xffff } else { (c as u8) as u16 }
    }
    fn wctob(c: u16) -> i32 {
        if c < 0x80 { c as i32 } else { -1 }
    }

    fn atoi(s: *const u8) -> i32 {
        libc::strtol(s as *const libc::c_char, std::ptr::null_mut(), 10) as i32
    }
    fn atol(s: *const u8) -> i32 {
        libc::strtol(s as *const libc::c_char, std::ptr::null_mut(), 10) as i32
    }
    fn atoll(s: *const u8) -> i64 {
        libc::strtoll(s as *const libc::c_char, std::ptr::null_mut(), 10)
    }
    fn _atoi64(s: *const u8) -> i64 {
        libc::strtoll(s as *const libc::c_char, std::ptr::null_mut(), 10)
    }
    fn atof(s: *const u8) -> f64 {
        libc::strtod(s as *const libc::c_char, std::ptr::null_mut())
    }
    fn strtol(s: *const u8, end: *mut *mut u8, base: i32) -> i32 {
        parse_signed(s, end, base, i32::MIN as i64, i32::MAX as i64) as i32
    }
    fn strtoul(s: *const u8, end: *mut *mut u8, base: i32) -> u32 {
        parse_unsigned(s, end, base, u32::MAX as u64) as u32
    }
    fn strtoll(s: *const u8, end: *mut *mut u8, base: i32) -> i64 {
        parse_signed(s, end, base, i64::MIN, i64::MAX)
    }
    fn strtoull(s: *const u8, end: *mut *mut u8, base: i32) -> u64 {
        parse_unsigned(s, end, base, u64::MAX)
    }
    fn _strtoi64(s: *const u8, end: *mut *mut u8, base: i32) -> i64 {
        parse_signed(s, end, base, i64::MIN, i64::MAX)
    }
    fn _strtoui64(s: *const u8, end: *mut *mut u8, base: i32) -> u64 {
        parse_unsigned(s, end, base, u64::MAX)
    }
    fn strtod(s: *const u8, end: *mut *mut u8) -> f64 {
        libc::strtod(s as *const libc::c_char, end as *mut *mut libc::c_char)
    }
    fn strtof(s: *const u8, end: *mut *mut u8) -> f32 {
        libc::strtof(s as *const libc::c_char, end as *mut *mut libc::c_char)
    }
    fn wcstol(s: *const u16, end: *mut *mut u16, base: i32) -> i32 {
        wcs_num_prefix(s, end, |c, e| parse_signed(c, e, base, i32::MIN as i64, i32::MAX as i64) as f64) as i32
    }
    fn wcstoul(s: *const u16, end: *mut *mut u16, base: i32) -> u32 {
        wcs_num_prefix(s, end, |c, e| parse_unsigned(c, e, base, u32::MAX as u64) as f64) as u32
    }
    fn wcstoll(s: *const u16, end: *mut *mut u16, base: i32) -> i64 {
        let b = wide_to_ansi_lossy(s);
        let mut c = b;
        c.push(0);
        let mut e: *mut u8 = std::ptr::null_mut();
        let v = parse_signed(c.as_ptr(), &mut e, base, i64::MIN, i64::MAX);
        if !end.is_null() {
            *end = s.add(e as usize - c.as_ptr() as usize) as *mut u16;
        }
        v
    }
    fn wcstod(s: *const u16, end: *mut *mut u16) -> f64 {
        wcs_num_prefix(s, end, |c, e| libc::strtod(c as *const libc::c_char, e as *mut *mut libc::c_char))
    }
    fn _wtoi(s: *const u16) -> i32 {
        wcstol(s, std::ptr::null_mut(), 10)
    }
    fn _wtol(s: *const u16) -> i32 {
        wcstol(s, std::ptr::null_mut(), 10)
    }
    fn _wtoi64(s: *const u16) -> i64 {
        wcstoll(s, std::ptr::null_mut(), 10)
    }
    fn _wtof(s: *const u16) -> f64 {
        wcstod(s, std::ptr::null_mut())
    }
    fn _itoa(v: i32, buf: *mut u8, radix: i32) -> *mut u8 {
        put_radix(buf, v as i64, radix, true, false)
    }
    fn _ltoa(v: i32, buf: *mut u8, radix: i32) -> *mut u8 {
        put_radix(buf, v as i64, radix, true, false)
    }
    fn _ultoa(v: u32, buf: *mut u8, radix: i32) -> *mut u8 {
        put_radix(buf, v as i64, radix, false, false)
    }
    fn _i64toa(v: i64, buf: *mut u8, radix: i32) -> *mut u8 {
        put_radix(buf, v, radix, true, true)
    }
    fn _ui64toa(v: u64, buf: *mut u8, radix: i32) -> *mut u8 {
        put_radix(buf, v as i64, radix, false, true)
    }
    fn _itoa_s(v: i32, buf: *mut u8, size: usize, radix: i32) -> i32 {
        let s = radix_string(v as i64, radix, true, false);
        if s.len() + 1 > size {
            return ERANGE;
        }
        std::ptr::copy_nonoverlapping(s.as_ptr(), buf, s.len());
        *buf.add(s.len()) = 0;
        0
    }
    fn _itow(v: i32, buf: *mut u16, radix: i32) -> *mut u16 {
        let s = radix_string(v as i64, radix, true, false);
        for (i, b) in s.bytes().enumerate() {
            *buf.add(i) = b as u16;
        }
        *buf.add(s.len()) = 0;
        buf
    }
    fn _i64tow(v: i64, buf: *mut u16, radix: i32) -> *mut u16 {
        let s = radix_string(v, radix, true, true);
        for (i, b) in s.bytes().enumerate() {
            *buf.add(i) = b as u16;
        }
        *buf.add(s.len()) = 0;
        buf
    }
}

fn ctype1_mask(c: u16) -> u16 {
    if c < 128 {
        unsafe { *ctype_table().add(c as usize + 1) }
    } else {
        char::from_u32(c as u32).map_or(0, |ch| {
            let mut v = 0;
            if ch.is_alphabetic() {
                v |= 0x100;
            }
            if ch.is_uppercase() {
                v |= 0x1;
            }
            if ch.is_lowercase() {
                v |= 0x2;
            }
            v
        })
    }
}

fn radix_string(v: i64, radix: i32, signed: bool, bits64: bool) -> String {
    let radix = if (2..=36).contains(&radix) { radix as u32 } else { 10 };
    let (neg, mut mag): (bool, u64) = if signed && radix == 10 {
        (v < 0, v.unsigned_abs())
    } else if bits64 {
        (false, v as u64)
    } else {
        (false, v as u32 as u64)
    };
    if mag == 0 {
        return "0".into();
    }
    let mut digits = Vec::new();
    while mag > 0 {
        digits.push(std::char::from_digit((mag % radix as u64) as u32, radix).unwrap());
        mag /= radix as u64;
    }
    if neg {
        digits.push('-');
    }
    digits.iter().rev().collect()
}

unsafe fn put_radix(buf: *mut u8, v: i64, radix: i32, signed: bool, bits64: bool) -> *mut u8 {
    let s = radix_string(v, radix, signed, bits64);
    std::ptr::copy_nonoverlapping(s.as_ptr(), buf, s.len());
    *buf.add(s.len()) = 0;
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radix_conversion() {
        assert_eq!(radix_string(255, 16, true, false), "ff");
        assert_eq!(radix_string(-42, 10, true, false), "-42");
        assert_eq!(radix_string(0, 2, true, false), "0");
        assert_eq!(radix_string(5, 2, false, true), "101");
        assert_eq!(radix_string(-1, 16, true, false), "ffffffff");
    }

    #[test]
    fn ctype_table_values() {
        init_data();
        assert_ne!(cls(b'a' as i32, 0x103), 0);
        assert_eq!(cls(b'a' as i32, 0x1), 0);
        assert_ne!(cls(b'7' as i32, 0x4), 0);
        assert_ne!(cls(b' ' as i32, 0x8), 0);
        assert_eq!(cls(-1, 0xff), 0);
    }
}
