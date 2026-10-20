//! kernel32: string conversion, comparison, character classification, locale and FormatMessage.
//!
//! The emulated locale is en-US with UTF-8 as the ANSI and OEM code page.

use crate::runtime::strings::*;
use crate::win::*;

pub const LOCALE_EN_US: DWORD = 0x0409;
const MB_ERR_INVALID_CHARS: DWORD = 8;
const WC_ERR_INVALID_CHARS: DWORD = 0x80;
const LOCALE_RETURN_NUMBER: DWORD = 0x2000_0000;
const NORM_IGNORECASE: DWORD = 1;
const LINGUISTIC_IGNORECASE: DWORD = 0x10;
const LCMAP_LOWERCASE: DWORD = 0x100;
const LCMAP_UPPERCASE: DWORD = 0x200;

const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

fn locale_string(lctype: DWORD) -> Option<String> {
    let t = lctype & !(LOCALE_RETURN_NUMBER | 0x8000_0000 | 0x4000_0000);
    let s: String = match t {
        0x1 => "0409".into(),
        0x2 | 0x1001 => "English".into(),
        0x3 => "ENU".into(),
        0x4 => "English".into(),
        0x5 => "1".into(),
        0x6 | 0x1002 => "United States".into(),
        0x7 => "USA".into(),
        0xb | 0x1004 => "65001".into(),
        0xc => ",".into(),
        0xd => "1".into(),
        0xe => ".".into(),
        0xf => ",".into(),
        0x10 => "3;0".into(),
        0x11 => "2".into(),
        0x12 => "1".into(),
        0x13 => "0123456789".into(),
        0x14 => "$".into(),
        0x15 => "USD".into(),
        0x16 => ".".into(),
        0x17 => ",".into(),
        0x18 => "3;0".into(),
        0x19 => "2".into(),
        0x1b | 0x1c => "0".into(),
        0x1d => "/".into(),
        0x1e => ":".into(),
        0x1f => "M/d/yyyy".into(),
        0x20 => "dddd, MMMM d, yyyy".into(),
        0x1003 => "h:mm:ss tt".into(),
        0x21 => "0".into(),
        0x22 => "1".into(),
        0x23 => "0".into(),
        0x25 => "0".into(),
        0x28 => "AM".into(),
        0x29 => "PM".into(),
        0x50 => "".into(),
        0x51 => "-".into(),
        0x59 => "en".into(),
        0x5a => "US".into(),
        0x5c => "en-US".into(),
        0x72 => "English (United States)".into(),
        0x1010 => "1".into(),
        0x100c => "6".into(),
        0x100d => "0".into(),
        0x2a..=0x30 => DAYS[(t - 0x2a) as usize].into(),
        0x31..=0x37 => DAYS[(t - 0x31) as usize][..3].into(),
        0x38..=0x43 => MONTHS[(t - 0x38) as usize].into(),
        0x44..=0x4f => MONTHS[(t - 0x44) as usize][..3].into(),
        _ => return None,
    };
    Some(s)
}

unsafe fn locale_info_w(lctype: DWORD, buf: LPWSTR, cch: i32) -> i32 {
    let Some(s) = locale_string(lctype) else {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    };
    if lctype & LOCALE_RETURN_NUMBER != 0 {
        if cch >= 2 && !buf.is_null() {
            *(buf as *mut u32) = s.parse::<u32>().unwrap_or(0);
        }
        return 2;
    }
    let w = to_wide_nul(&s);
    if cch == 0 {
        return w.len() as i32;
    }
    if (cch as usize) < w.len() {
        set_last_error(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    std::ptr::copy_nonoverlapping(w.as_ptr(), buf, w.len());
    w.len() as i32
}

unsafe fn locale_info_a(lctype: DWORD, buf: LPSTR, cch: i32) -> i32 {
    let Some(s) = locale_string(lctype) else {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    };
    if lctype & LOCALE_RETURN_NUMBER != 0 {
        if cch >= 4 && !buf.is_null() {
            *(buf as *mut u32) = s.parse::<u32>().unwrap_or(0);
        }
        return 4;
    }
    let n = s.len() as i32 + 1;
    if cch == 0 {
        return n;
    }
    if cch < n {
        set_last_error(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    copy_a(buf, cch as usize, s.as_bytes());
    n
}

fn cmp_units(a: &[u16], b: &[u16], ignore_case: bool) -> i32 {
    let norm = |u: u16| if ignore_case { lower_unit(upper_unit(u)) } else { u };
    for (x, y) in a.iter().zip(b.iter()) {
        let (x, y) = (norm(*x), norm(*y));
        if x != y {
            return if x < y { 1 } else { 3 };
        }
    }
    match a.len().cmp(&b.len()) {
        std::cmp::Ordering::Less => 1,
        std::cmp::Ordering::Equal => 2,
        std::cmp::Ordering::Greater => 3,
    }
}

unsafe fn compare_w(flags: DWORD, a: LPCWSTR, an: i32, b: LPCWSTR, bn: i32) -> i32 {
    cmp_units(wstr_n(a, an), wstr_n(b, bn), flags & (NORM_IGNORECASE | LINGUISTIC_IGNORECASE) != 0)
}

unsafe fn compare_a(flags: DWORD, a: LPCSTR, an: i32, b: LPCSTR, bn: i32) -> i32 {
    let wa = mb_to_wide(CP_UTF8, astr_n(a, an), false).unwrap_or_default();
    let wb = mb_to_wide(CP_UTF8, astr_n(b, bn), false).unwrap_or_default();
    cmp_units(&wa, &wb, flags & (NORM_IGNORECASE | LINGUISTIC_IGNORECASE) != 0)
}

unsafe fn lc_map(flags: DWORD, src: &[u16], dst: LPWSTR, dstlen: i32, include_nul: bool) -> i32 {
    let mut out: Vec<u16> = if flags & LCMAP_LOWERCASE != 0 {
        src.iter().map(|&u| lower_unit(u)).collect()
    } else if flags & LCMAP_UPPERCASE != 0 {
        src.iter().map(|&u| upper_unit(u)).collect()
    } else {
        src.to_vec()
    };
    if include_nul {
        out.push(0);
    }
    if dstlen == 0 {
        return out.len() as i32;
    }
    if (dstlen as usize) < out.len() {
        set_last_error(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    std::ptr::copy_nonoverlapping(out.as_ptr(), dst, out.len());
    out.len() as i32
}

fn ctype1(u: u16) -> u16 {
    let Some(c) = char::from_u32(u as u32) else { return 0 };
    let mut r = 0u16;
    if c.is_uppercase() {
        r |= 0x1;
    }
    if c.is_lowercase() {
        r |= 0x2;
    }
    if c.is_ascii_digit() || c.is_numeric() && c.is_ascii() {
        r |= 0x4;
    }
    if c.is_whitespace() {
        r |= 0x8;
    }
    if c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace() && !c.is_control() && u > 0x7f) {
        r |= 0x10;
    }
    if c.is_control() {
        r |= 0x20;
    }
    if c == ' ' || c == '\t' {
        r |= 0x40;
    }
    if c.is_ascii_hexdigit() {
        r |= 0x80;
    }
    if c.is_alphabetic() {
        r |= 0x100;
    }
    r | 0x200
}

unsafe fn string_type(ty: DWORD, src: &[u16], out: *mut u16) -> BOOL {
    if out.is_null() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    for (i, &u) in src.iter().enumerate() {
        *out.add(i) = if ty == 1 { ctype1(u) } else { 0 };
    }
    TRUE
}

static MESSAGES: &[(DWORD, &str)] = &[
    (0, "The operation completed successfully."),
    (1, "Incorrect function."),
    (2, "The system cannot find the file specified."),
    (3, "The system cannot find the path specified."),
    (4, "The system cannot open the file."),
    (5, "Access is denied."),
    (6, "The handle is invalid."),
    (8, "Not enough memory resources are available to process this command."),
    (13, "The data is invalid."),
    (14, "Not enough memory resources are available to complete this operation."),
    (17, "The system cannot move the file to a different disk drive."),
    (18, "There are no more files."),
    (32, "The process cannot access the file because it is being used by another process."),
    (38, "Reached the end of the file."),
    (50, "The request is not supported."),
    (80, "The file exists."),
    (87, "The parameter is incorrect."),
    (109, "The pipe has been ended."),
    (112, "There is not enough space on the disk."),
    (120, "This function is not supported on this system."),
    (122, "The data area passed to a system call is too small."),
    (123, "The filename, directory name, or volume label syntax is incorrect."),
    (126, "The specified module could not be found."),
    (127, "The specified procedure could not be found."),
    (145, "The directory is not empty."),
    (183, "Cannot create a file when that file already exists."),
    (193, "%1 is not a valid Win32 application."),
    (203, "The system could not find the environment option that was entered."),
    (234, "More data is available."),
    (258, "The wait operation timed out."),
    (259, "No more data is available."),
    (267, "The directory name is invalid."),
    (997, "Overlapped I/O operation is in progress."),
    (998, "Invalid access to memory location."),
    (1168, "Element not found."),
    (1314, "A required privilege is not held by the client."),
    (1460, "This operation returned because the timeout period expired."),
    (0x8000_4001, "Not implemented"),
    (0x8000_4002, "No such interface supported"),
    (0x8000_4003, "Invalid pointer"),
    (0x8000_4005, "Unspecified error"),
    (0x8000_FFFF, "Catastrophic failure"),
    (0x8004_0154, "Class not registered"),
    (0x8007_000E, "Not enough memory resources are available to complete this operation."),
    (0x8009_000F, "Object already exists."),
    (0x8009_0008, "Invalid algorithm specified."),
    (0x8009_2004, "Cannot find object or property."),
    (0x800B_0100, "No signature was present in the subject."),
    (0x800B_0109, "A certificate chain processed, but terminated in a root certificate which is not trusted by the trust provider."),
    (0x800B_010A, "A certificate chain could not be built to a trusted root authority."),
    (0x2EE7, "The server name or address could not be resolved"),
    (0x2EFD, "A connection with the server could not be established"),
];

pub fn system_message(id: DWORD) -> Option<&'static str> {
    if let Some((_, m)) = MESSAGES.iter().find(|(c, _)| *c == id) {
        return Some(m);
    }
    // HRESULT_FROM_WIN32: the low word is a system error code
    if id >> 16 == 0x8007 {
        return MESSAGES.iter().find(|(c, _)| *c == id & 0xffff).map(|(_, m)| *m);
    }
    None
}

unsafe fn slot_args(flags: DWORD, args: *const usize) -> *const usize {
    if args.is_null() {
        return std::ptr::null();
    }
    if flags & 0x2000 != 0 {
        args
    } else {
        *(args as *const *const usize)
    }
}

/// Expands `%1`, `%2!s!`, `%n`, `%%` ... in a FormatMessage template.
unsafe fn expand_template(t: &[u16], slots: *const usize, wide: bool) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    let mut i = 0;
    let c = |ch: char| ch as u16;
    while i < t.len() {
        if t[i] != c('%') {
            out.push(t[i]);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&n) = t.get(i) else { break };
        match char::from_u32(n as u32).unwrap_or(' ') {
            '%' => out.push(c('%')),
            'n' => out.extend("\r\n".encode_utf16()),
            'r' => out.push(13),
            't' => out.push(9),
            '.' => out.push(c('.')),
            '!' => out.push(c('!')),
            '0' => break,
            d if d.is_ascii_digit() => {
                let mut num = d.to_digit(10).unwrap() as usize;
                i += 1;
                while i < t.len() && (t[i] as u8 as char).is_ascii_digit() && t[i] < 128 {
                    num = num * 10 + (t[i] as u8 - b'0') as usize;
                    i += 1;
                }
                let mut spec = String::from("s");
                if t.get(i) == Some(&c('!')) {
                    let end = t[i + 1..].iter().position(|&x| x == c('!')).map(|p| i + 1 + p);
                    if let Some(e) = end {
                        spec = w_to_string(&t[i + 1..e]);
                        i = e + 1;
                    }
                }
                let arg = if slots.is_null() { 0 } else { *slots.add(num - 1) };
                let text = match spec.chars().last().unwrap_or('s') {
                    's' | 'S' => {
                        if arg == 0 {
                            "(null)".to_string()
                        } else if wide {
                            w_ptr_to_string(arg as *const u16)
                        } else {
                            a_to_string(arg as *const u8)
                        }
                    }
                    'd' | 'i' => (arg as i32).to_string(),
                    'u' => (arg as u32).to_string(),
                    'x' => format!("{:x}", arg as u32),
                    'X' => format!("{:X}", arg as u32),
                    'c' => char::from_u32(arg as u32).map(String::from).unwrap_or_default(),
                    'p' => format!("{arg:x}"),
                    _ => arg.to_string(),
                };
                out.extend(text.encode_utf16());
                continue;
            }
            other => {
                out.push(c('%'));
                let mut b = [0u16; 2];
                out.extend_from_slice(other.encode_utf16(&mut b));
            }
        }
        i += 1;
    }
    out
}

unsafe fn format_message(flags: DWORD, source: usize, id: DWORD, args: *const usize, wide: bool) -> Option<Vec<u16>> {
    let slots = slot_args(flags, args);
    let mut text: Vec<u16> = if flags & 0x400 != 0 {
        if wide {
            wstr(source as *const u16).to_vec()
        } else {
            mb_to_wide(CP_UTF8, astr(source as *const u8), false)?
        }
    } else if flags & 0x1000 != 0 {
        let m = system_message(id)?;
        let mut v = to_wide(m);
        if flags & 0xff == 0 {
            v.extend("\r\n".encode_utf16());
        }
        v
    } else {
        set_last_error(ERROR_INVALID_PARAMETER);
        return None;
    };
    if flags & 0x200 == 0 {
        text = expand_template(&text, slots, wide);
    }
    Some(text)
}

crate::shims! { "kernel32";
    fn MultiByteToWideChar(cp: UINT, flags: DWORD, src: LPCSTR, srclen: i32, dst: LPWSTR, dstlen: i32) -> i32 {
        if src.is_null() || srclen == 0 {
            set_last_error(ERROR_INVALID_PARAMETER);
            return 0;
        }
        let bytes = astr_n(src, srclen);
        let Some(mut w) = mb_to_wide(cp, bytes, flags & MB_ERR_INVALID_CHARS != 0) else {
            set_last_error(ERROR_NO_UNICODE_TRANSLATION);
            return 0;
        };
        if srclen < 0 {
            w.push(0);
        }
        if dstlen == 0 {
            return w.len() as i32;
        }
        if (dstlen as usize) < w.len() || dst.is_null() {
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            return 0;
        }
        std::ptr::copy_nonoverlapping(w.as_ptr(), dst, w.len());
        w.len() as i32
    }
    fn WideCharToMultiByte(cp: UINT, flags: DWORD, src: LPCWSTR, srclen: i32, dst: LPSTR, dstlen: i32, _def: LPCSTR, used: *mut BOOL) -> i32 {
        if src.is_null() || srclen == 0 {
            set_last_error(ERROR_INVALID_PARAMETER);
            return 0;
        }
        let mut w = wstr_n(src, srclen).to_vec();
        if srclen < 0 {
            w.push(0);
        }
        let mut used_default = false;
        let Some(b) = wide_to_mb(cp, &w, flags & WC_ERR_INVALID_CHARS != 0, &mut used_default) else {
            set_last_error(ERROR_NO_UNICODE_TRANSLATION);
            return 0;
        };
        if !used.is_null() {
            *used = used_default as BOOL;
        }
        if dstlen == 0 {
            return b.len() as i32;
        }
        if (dstlen as usize) < b.len() || dst.is_null() {
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            return 0;
        }
        std::ptr::copy_nonoverlapping(b.as_ptr(), dst, b.len());
        b.len() as i32
    }

    fn lstrlenA(s: LPCSTR) -> i32 {
        astr(s).len() as i32
    }
    fn lstrlenW(s: LPCWSTR) -> i32 {
        wlen(s) as i32
    }
    fn lstrcpyA(dst: LPSTR, src: LPCSTR) -> LPSTR {
        let b = astr(src);
        std::ptr::copy_nonoverlapping(b.as_ptr(), dst, b.len());
        *dst.add(b.len()) = 0;
        dst
    }
    fn lstrcpyW(dst: LPWSTR, src: LPCWSTR) -> LPWSTR {
        let w = wstr(src);
        std::ptr::copy_nonoverlapping(w.as_ptr(), dst, w.len());
        *dst.add(w.len()) = 0;
        dst
    }
    fn lstrcpynA(dst: LPSTR, src: LPCSTR, max: i32) -> LPSTR {
        if max <= 0 {
            return dst;
        }
        let b = astr(src);
        let n = b.len().min(max as usize - 1);
        std::ptr::copy_nonoverlapping(b.as_ptr(), dst, n);
        *dst.add(n) = 0;
        dst
    }
    fn lstrcpynW(dst: LPWSTR, src: LPCWSTR, max: i32) -> LPWSTR {
        if max <= 0 {
            return dst;
        }
        let w = wstr(src);
        let n = w.len().min(max as usize - 1);
        std::ptr::copy_nonoverlapping(w.as_ptr(), dst, n);
        *dst.add(n) = 0;
        dst
    }
    fn lstrcatA(dst: LPSTR, src: LPCSTR) -> LPSTR {
        let at = astr(dst).len();
        lstrcpyA(dst.add(at), src);
        dst
    }
    fn lstrcatW(dst: LPWSTR, src: LPCWSTR) -> LPWSTR {
        let at = wlen(dst);
        lstrcpyW(dst.add(at), src);
        dst
    }
    fn lstrcmpA(a: LPCSTR, b: LPCSTR) -> i32 {
        compare_a(0, a, -1, b, -1) - 2
    }
    fn lstrcmpW(a: LPCWSTR, b: LPCWSTR) -> i32 {
        compare_w(0, a, -1, b, -1) - 2
    }
    fn lstrcmpiA(a: LPCSTR, b: LPCSTR) -> i32 {
        compare_a(NORM_IGNORECASE, a, -1, b, -1) - 2
    }
    fn lstrcmpiW(a: LPCWSTR, b: LPCWSTR) -> i32 {
        compare_w(NORM_IGNORECASE, a, -1, b, -1) - 2
    }
    fn CompareStringW(_locale: DWORD, flags: DWORD, a: LPCWSTR, an: i32, b: LPCWSTR, bn: i32) -> i32 {
        compare_w(flags, a, an, b, bn)
    }
    fn CompareStringA(_locale: DWORD, flags: DWORD, a: LPCSTR, an: i32, b: LPCSTR, bn: i32) -> i32 {
        compare_a(flags, a, an, b, bn)
    }
    fn CompareStringEx(_name: LPCWSTR, flags: DWORD, a: LPCWSTR, an: i32, b: LPCWSTR, bn: i32, _v: usize, _r: usize, _l: usize) -> i32 {
        compare_w(flags, a, an, b, bn)
    }
    fn CompareStringOrdinal(a: LPCWSTR, an: i32, b: LPCWSTR, bn: i32, ignore_case: BOOL) -> i32 {
        cmp_units(wstr_n(a, an), wstr_n(b, bn), ignore_case != 0)
    }
    fn IsDBCSLeadByte(_b: u8) -> BOOL {
        FALSE
    }
    fn IsDBCSLeadByteEx(_cp: UINT, _b: u8) -> BOOL {
        FALSE
    }

    fn GetStringTypeW(ty: DWORD, src: LPCWSTR, cnt: i32, out: *mut u16) -> BOOL {
        string_type(ty, wstr_n(src, cnt), out)
    }
    fn GetStringTypeExW(_locale: DWORD, ty: DWORD, src: LPCWSTR, cnt: i32, out: *mut u16) -> BOOL {
        string_type(ty, wstr_n(src, cnt), out)
    }
    fn GetStringTypeA(_locale: DWORD, ty: DWORD, src: LPCSTR, cnt: i32, out: *mut u16) -> BOOL {
        let w = mb_to_wide(CP_UTF8, astr_n(src, cnt), false).unwrap_or_default();
        string_type(ty, &w, out)
    }
    fn GetStringTypeExA(locale: DWORD, ty: DWORD, src: LPCSTR, cnt: i32, out: *mut u16) -> BOOL {
        GetStringTypeA(locale, ty, src, cnt, out)
    }
    fn LCMapStringW(_locale: DWORD, flags: DWORD, src: LPCWSTR, n: i32, dst: LPWSTR, dstlen: i32) -> i32 {
        lc_map(flags, wstr_n(src, n), dst, dstlen, n < 0)
    }
    fn LCMapStringEx(_name: LPCWSTR, flags: DWORD, src: LPCWSTR, n: i32, dst: LPWSTR, dstlen: i32, _v: usize, _r: usize, _l: usize) -> i32 {
        lc_map(flags, wstr_n(src, n), dst, dstlen, n < 0)
    }
    fn LCMapStringA(_locale: DWORD, flags: DWORD, src: LPCSTR, n: i32, dst: LPSTR, dstlen: i32) -> i32 {
        let w = mb_to_wide(CP_UTF8, astr_n(src, n), false).unwrap_or_default();
        let mut tmp = vec![0u16; w.len() + 2];
        let r = lc_map(flags, &w, tmp.as_mut_ptr(), tmp.len() as i32, n < 0);
        if r == 0 {
            return 0;
        }
        let mut d = false;
        let b = wide_to_mb(CP_UTF8, &tmp[..r as usize], false, &mut d).unwrap_or_default();
        if dstlen == 0 {
            return b.len() as i32;
        }
        if (dstlen as usize) < b.len() {
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            return 0;
        }
        std::ptr::copy_nonoverlapping(b.as_ptr(), dst, b.len());
        b.len() as i32
    }
    fn FoldStringW(_flags: DWORD, src: LPCWSTR, n: i32, dst: LPWSTR, dstlen: i32) -> i32 {
        lc_map(0, wstr_n(src, n), dst, dstlen, n < 0)
    }

    fn GetACP() -> UINT {
        65001
    }
    fn GetOEMCP() -> UINT {
        65001
    }
    fn IsValidCodePage(cp: UINT) -> BOOL {
        matches!(cp, 0 | 1 | 437 | 850 | 852 | 866 | 932 | 936 | 949 | 950 | 1250..=1258 | 28591 | 28592 | 28605 | 65000 | 65001 | 20127) as BOOL
    }
    fn GetCPInfo(cp: UINT, info: *mut u32) -> BOOL {
        if info.is_null() {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        std::ptr::write_bytes(info as *mut u8, 0, 18);
        *info = if is_utf8_cp(cp) { 4 } else { 1 };
        *(info.add(1) as *mut u8) = b'?';
        TRUE
    }

    fn GetLocaleInfoW(_locale: DWORD, lctype: DWORD, buf: LPWSTR, cch: i32) -> i32 {
        locale_info_w(lctype, buf, cch)
    }
    fn GetLocaleInfoEx(_name: LPCWSTR, lctype: DWORD, buf: LPWSTR, cch: i32) -> i32 {
        locale_info_w(lctype, buf, cch)
    }
    fn GetLocaleInfoA(_locale: DWORD, lctype: DWORD, buf: LPSTR, cch: i32) -> i32 {
        locale_info_a(lctype, buf, cch)
    }
    fn GetUserDefaultLCID() -> DWORD {
        LOCALE_EN_US
    }
    fn GetSystemDefaultLCID() -> DWORD {
        LOCALE_EN_US
    }
    fn GetThreadLocale() -> DWORD {
        LOCALE_EN_US
    }
    fn SetThreadLocale(_l: DWORD) -> BOOL {
        TRUE
    }
    fn GetUserDefaultLangID() -> WORD {
        0x409
    }
    fn GetSystemDefaultLangID() -> WORD {
        0x409
    }
    fn GetUserDefaultUILanguage() -> WORD {
        0x409
    }
    fn GetSystemDefaultUILanguage() -> WORD {
        0x409
    }
    fn GetUserDefaultLocaleName(buf: LPWSTR, cch: i32) -> i32 {
        locale_info_w(0x5c, buf, cch)
    }
    fn GetSystemDefaultLocaleName(buf: LPWSTR, cch: i32) -> i32 {
        locale_info_w(0x5c, buf, cch)
    }
    fn LCIDToLocaleName(_lcid: DWORD, buf: LPWSTR, cch: i32, _flags: DWORD) -> i32 {
        locale_info_w(0x5c, buf, cch)
    }
    fn LocaleNameToLCID(_name: LPCWSTR, _flags: DWORD) -> DWORD {
        LOCALE_EN_US
    }
    fn IsValidLocale(_l: DWORD, _flags: DWORD) -> BOOL {
        TRUE
    }
    fn IsValidLocaleName(_n: LPCWSTR) -> BOOL {
        TRUE
    }
    fn IsValidLanguageGroup(_g: DWORD, _f: DWORD) -> BOOL {
        TRUE
    }
    fn GetUserPreferredUILanguages(_flags: DWORD, num: *mut ULONG, buf: LPWSTR, len: *mut ULONG) -> BOOL {
        let w: Vec<u16> = "en-US\0\0".encode_utf16().collect();
        if !num.is_null() {
            *num = 1;
        }
        if buf.is_null() || (*len as usize) < w.len() {
            if !len.is_null() {
                *len = w.len() as ULONG;
            }
            return buf.is_null() as BOOL;
        }
        std::ptr::copy_nonoverlapping(w.as_ptr(), buf, w.len());
        *len = w.len() as ULONG;
        TRUE
    }

    fn FormatMessageW(flags: DWORD, source: usize, id: DWORD, _lang: DWORD, buf: LPWSTR, size: DWORD, args: *const usize) -> DWORD {
        let Some(text) = format_message(flags, source, id, args, true) else {
            if last_error() == ERROR_SUCCESS {
                set_last_error(317);
            }
            return 0;
        };
        if flags & 0x100 != 0 {
            let p = libc::malloc((text.len() + 1) * 2) as *mut u16;
            std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
            *p.add(text.len()) = 0;
            *(buf as *mut *mut u16) = p;
        } else if !copy_w(buf, size as usize, &text) {
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            return 0;
        }
        text.len() as DWORD
    }
    fn FormatMessageA(flags: DWORD, source: usize, id: DWORD, _lang: DWORD, buf: LPSTR, size: DWORD, args: *const usize) -> DWORD {
        let Some(text) = format_message(flags, source, id, args, false) else {
            if last_error() == ERROR_SUCCESS {
                set_last_error(317);
            }
            return 0;
        };
        let mut d = false;
        let b = wide_to_mb(CP_UTF8, &text, false, &mut d).unwrap_or_default();
        if flags & 0x100 != 0 {
            let p = libc::malloc(b.len() + 1) as *mut u8;
            std::ptr::copy_nonoverlapping(b.as_ptr(), p, b.len());
            *p.add(b.len()) = 0;
            *(buf as *mut *mut u8) = p;
        } else if !copy_a(buf, size as usize, &b) {
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            return 0;
        }
        b.len() as DWORD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_orders() {
        let a = to_wide("abc");
        let b = to_wide("abd");
        assert_eq!(cmp_units(&a, &b, false), 1);
        assert_eq!(cmp_units(&b, &a, false), 3);
        assert_eq!(cmp_units(&a, &to_wide("ABC"), true), 2);
        assert_eq!(cmp_units(&a, &to_wide("ABC"), false), 3);
    }

    #[test]
    fn template_expansion() {
        let t = to_wide("x%1!d! %%");
        let args = [42usize];
        let out = unsafe { expand_template(&t, args.as_ptr(), true) };
        assert_eq!(w_to_string(&out), "x42 %");
    }

    #[test]
    fn locale_defaults() {
        assert_eq!(locale_string(0xe).as_deref(), Some("."));
        assert_eq!(locale_string(0x38).as_deref(), Some("January"));
        assert_eq!(locale_string(0x44).as_deref(), Some("Jan"));
    }
}
