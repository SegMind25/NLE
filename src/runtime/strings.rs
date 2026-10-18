//! String helpers for the narrow (UTF-8 "ANSI") and wide (UTF-16) Windows APIs.
//!
//! The emulated ANSI code page is UTF-8, which matches Linux file names and terminals.

use crate::win::*;

pub unsafe fn wlen(p: *const u16) -> usize {
    if p.is_null() {
        return 0;
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    n
}

pub unsafe fn wstr<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    std::slice::from_raw_parts(p, wlen(p))
}

/// `n` units, or up to the NUL when `n` is -1.
pub unsafe fn wstr_n<'a>(p: *const u16, n: i32) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    if n < 0 {
        wstr(p)
    } else {
        std::slice::from_raw_parts(p, n as usize)
    }
}

pub unsafe fn astr<'a>(p: *const u8) -> &'a [u8] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    std::slice::from_raw_parts(p, n)
}

pub unsafe fn astr_n<'a>(p: *const u8, n: i32) -> &'a [u8] {
    if p.is_null() {
        return &[];
    }
    if n < 0 {
        astr(p)
    } else {
        std::slice::from_raw_parts(p, n as usize)
    }
}

pub fn w_to_string(w: &[u16]) -> String {
    String::from_utf16_lossy(w)
}

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub fn to_wide_nul(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

pub unsafe fn a_to_string(p: *const u8) -> String {
    String::from_utf8_lossy(astr(p)).into_owned()
}

pub unsafe fn w_ptr_to_string(p: *const u16) -> String {
    w_to_string(wstr(p))
}

/// Copies `src` into `dst` with a terminating NUL. Returns false (writing nothing) if it does not fit.
pub unsafe fn copy_w(dst: *mut u16, cap: usize, src: &[u16]) -> bool {
    if dst.is_null() || src.len() + 1 > cap {
        return false;
    }
    std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
    *dst.add(src.len()) = 0;
    true
}

pub unsafe fn copy_a(dst: *mut u8, cap: usize, src: &[u8]) -> bool {
    if dst.is_null() || src.len() + 1 > cap {
        return false;
    }
    std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
    *dst.add(src.len()) = 0;
    true
}

/// Windows "buffer too small" convention for `GetXxxW(buf, size)` style APIs:
/// returns the length (without NUL) on success, or the required size including the NUL.
pub unsafe fn return_w(dst: *mut u16, cap: u32, s: &str) -> u32 {
    let w = to_wide(s);
    if copy_w(dst, cap as usize, &w) {
        w.len() as u32
    } else {
        w.len() as u32 + 1
    }
}

pub unsafe fn return_a(dst: *mut u8, cap: u32, s: &str) -> u32 {
    let b = s.as_bytes();
    if copy_a(dst, cap as usize, b) {
        b.len() as u32
    } else {
        b.len() as u32 + 1
    }
}

const CP1252_HIGH: [u16; 32] = [
    0x20ac, 0x0081, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008d, 0x017d, 0x008f, 0x0090,
    0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x02dc, 0x2122, 0x0161, 0x203a, 0x0153, 0x009d, 0x017e, 0x0178,
];

pub fn is_utf8_cp(cp: u32) -> bool {
    cp == CP_UTF8 || cp == CP_ACP || cp == CP_OEMCP || cp == 65000 || cp == 0 || cp == 1 || cp == 2 || cp == 3
}

/// Decodes `bytes` in code page `cp`. Returns `None` if `strict` and the input is invalid.
pub fn mb_to_wide(cp: u32, bytes: &[u8], strict: bool) -> Option<Vec<u16>> {
    if is_utf8_cp(cp) {
        if strict {
            return std::str::from_utf8(bytes).ok().map(|s| s.encode_utf16().collect());
        }
        return Some(String::from_utf8_lossy(bytes).encode_utf16().collect());
    }
    Some(
        bytes
            .iter()
            .map(|&b| if cp == 1252 && (0x80..0xa0).contains(&b) { CP1252_HIGH[(b - 0x80) as usize] } else { b as u16 })
            .collect(),
    )
}

/// Encodes UTF-16 to code page `cp`. `used_default` is set when a character had no mapping.
pub fn wide_to_mb(cp: u32, w: &[u16], strict: bool, used_default: &mut bool) -> Option<Vec<u8>> {
    if is_utf8_cp(cp) {
        let mut out = Vec::with_capacity(w.len());
        for r in char::decode_utf16(w.iter().copied()) {
            match r {
                Ok(c) => {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                Err(_) => {
                    if strict {
                        return None;
                    }
                    *used_default = true;
                    out.extend_from_slice("\u{fffd}".as_bytes());
                }
            }
        }
        return Some(out);
    }
    let mut out = Vec::with_capacity(w.len());
    for &u in w {
        if u < 0x80 || (u < 0x100 && !(cp == 1252 && (0x80..0xa0).contains(&u))) {
            out.push(u as u8);
        } else if let Some(i) = (cp == 1252).then(|| CP1252_HIGH.iter().position(|&x| x == u)).flatten() {
            out.push(0x80 + i as u8);
        } else {
            *used_default = true;
            out.push(b'?');
        }
    }
    Some(out)
}

/// Lowercase/uppercase mapping used by `CharUpperW`, `LCMapStringW` and friends.
pub fn upper_unit(u: u16) -> u16 {
    match char::from_u32(u as u32) {
        Some(c) => {
            let mut it = c.to_uppercase();
            match (it.next(), it.next()) {
                (Some(r), None) if (r as u32) < 0x10000 => r as u32 as u16,
                _ => u,
            }
        }
        None => u,
    }
}

pub fn lower_unit(u: u16) -> u16 {
    match char::from_u32(u as u32) {
        Some(c) => {
            let mut it = c.to_lowercase();
            match (it.next(), it.next()) {
                (Some(r), None) if (r as u32) < 0x10000 => r as u32 as u16,
                _ => u,
            }
        }
        None => u,
    }
}

/// Maps a host `errno` to the closest Win32 error code.
pub fn errno_to_win(e: i32) -> DWORD {
    match e {
        0 => ERROR_SUCCESS,
        libc::ENOENT => ERROR_FILE_NOT_FOUND,
        libc::ENOTDIR => ERROR_PATH_NOT_FOUND,
        libc::EACCES | libc::EPERM | libc::EROFS => ERROR_ACCESS_DENIED,
        libc::EEXIST => ERROR_ALREADY_EXISTS,
        libc::EBADF => ERROR_INVALID_HANDLE,
        libc::ENOMEM => ERROR_NOT_ENOUGH_MEMORY,
        libc::EINVAL => ERROR_INVALID_PARAMETER,
        libc::ENOSPC => ERROR_DISK_FULL,
        libc::EMFILE | libc::ENFILE => ERROR_TOO_MANY_OPEN_FILES,
        libc::ENOTEMPTY => ERROR_DIR_NOT_EMPTY,
        libc::EPIPE => ERROR_BROKEN_PIPE,
        libc::EXDEV => ERROR_NOT_SAME_DEVICE,
        libc::EISDIR => ERROR_ACCESS_DENIED,
        libc::ENAMETOOLONG => ERROR_INVALID_NAME,
        libc::EBUSY | libc::ETXTBSY => ERROR_SHARING_VIOLATION,
        libc::ENOSYS | libc::EOPNOTSUPP => ERROR_NOT_SUPPORTED,
        libc::ETIMEDOUT => ERROR_TIMEOUT,
        libc::EFAULT => ERROR_NOACCESS,
        _ => ERROR_INVALID_FUNCTION,
    }
}

pub fn set_last_error_from_errno() {
    let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    set_last_error(errno_to_win(e));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_roundtrip() {
        let w = mb_to_wide(CP_UTF8, "héllo ✓".as_bytes(), true).unwrap();
        let mut d = false;
        let b = wide_to_mb(CP_UTF8, &w, true, &mut d).unwrap();
        assert_eq!(b, "héllo ✓".as_bytes());
        assert!(!d);
    }

    #[test]
    fn strict_rejects_invalid_utf8() {
        assert!(mb_to_wide(CP_UTF8, &[0xff, 0xfe], true).is_none());
        assert!(mb_to_wide(CP_UTF8, &[0xff, 0xfe], false).is_some());
    }

    #[test]
    fn cp1252_high_range() {
        let w = mb_to_wide(1252, &[0x80, 0x41], false).unwrap();
        assert_eq!(w, vec![0x20ac, 0x41]);
        let mut d = false;
        assert_eq!(wide_to_mb(1252, &[0x20ac], false, &mut d).unwrap(), vec![0x80]);
    }
}
