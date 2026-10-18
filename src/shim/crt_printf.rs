//! printf-family formatting engine with Microsoft C runtime semantics.
//!
//! Arguments come from a Win64 `va_list`: an array of 8-byte slots (doubles are passed as their bit
//! pattern). Output is UTF-8 bytes; wide callers convert afterwards.

use crate::runtime::strings;

pub struct Args {
    slots: *const u64,
}

impl Args {
    pub fn new(slots: *const u64) -> Args {
        Args { slots }
    }

    pub unsafe fn next_raw(&mut self) -> u64 {
        self.next_u64()
    }

    unsafe fn next_u64(&mut self) -> u64 {
        if self.slots.is_null() {
            return 0;
        }
        let v = self.slots.read();
        self.slots = self.slots.add(1);
        v
    }

    unsafe fn next_i32(&mut self) -> i32 {
        self.next_u64() as i32
    }

    unsafe fn next_f64(&mut self) -> f64 {
        f64::from_bits(self.next_u64())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Len {
    None,
    HH,
    H,
    L,
    LL,
    Ptr,
    W,
}

struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: usize,
    prec: Option<usize>,
}

fn pad(out: &mut Vec<u8>, body: &[u8], width: usize, left: bool, ch: u8) {
    let n = String::from_utf8_lossy(body).chars().count().max(body.len().min(width));
    let visible = if body.iter().all(|b| *b < 0x80) { body.len() } else { n };
    if visible >= width {
        out.extend_from_slice(body);
    } else if left {
        out.extend_from_slice(body);
        out.extend(std::iter::repeat(b' ').take(width - visible));
    } else {
        out.extend(std::iter::repeat(ch).take(width - visible));
        out.extend_from_slice(body);
    }
}

fn emit_int(out: &mut Vec<u8>, s: &Spec, negative: bool, mut digits: String, prefix: &str) {
    if let Some(p) = s.prec {
        if digits == "0" && p == 0 {
            digits.clear();
        }
        while digits.len() < p {
            digits.insert(0, '0');
        }
    }
    let sign = if negative {
        "-"
    } else if s.plus {
        "+"
    } else if s.space {
        " "
    } else {
        ""
    };
    let head = format!("{sign}{prefix}");
    let total = head.len() + digits.len();
    if total >= s.width {
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(digits.as_bytes());
    } else if s.left {
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(digits.as_bytes());
        out.extend(std::iter::repeat(b' ').take(s.width - total));
    } else if s.zero && s.prec.is_none() {
        out.extend_from_slice(head.as_bytes());
        out.extend(std::iter::repeat(b'0').take(s.width - total));
        out.extend_from_slice(digits.as_bytes());
    } else {
        out.extend(std::iter::repeat(b' ').take(s.width - total));
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(digits.as_bytes());
    }
}

fn c_float(s: &Spec, conv: u8, v: f64) -> Vec<u8> {
    let mut spec = String::from("%");
    if s.left {
        spec.push('-');
    }
    if s.plus {
        spec.push('+');
    }
    if s.space {
        spec.push(' ');
    }
    if s.alt {
        spec.push('#');
    }
    if s.zero {
        spec.push('0');
    }
    if s.width > 0 {
        spec.push_str(&s.width.to_string());
    }
    if let Some(p) = s.prec {
        spec.push_str(&format!(".{p}"));
    }
    spec.push(conv as char);
    spec.push('\0');
    let mut buf = vec![0u8; 512];
    loop {
        let n = unsafe { libc::snprintf(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), spec.as_ptr() as *const libc::c_char, v) };
        if n < 0 {
            return Vec::new();
        }
        if (n as usize) < buf.len() {
            buf.truncate(n as usize);
            return buf;
        }
        buf.resize(n as usize + 1, 0);
    }
}

unsafe fn read_narrow<'a>(p: *const u8, prec: Option<usize>) -> &'a [u8] {
    let mut n = 0;
    while prec.map_or(true, |m| n < m) && *p.add(n) != 0 {
        n += 1;
    }
    std::slice::from_raw_parts(p, n)
}

unsafe fn read_wide(p: *const u16, prec: Option<usize>) -> Vec<u8> {
    let mut n = 0;
    while prec.map_or(true, |m| n < m) && *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n)).into_bytes()
}

/// Formats `fmt` (UTF-8 bytes). `wide_ctx` is true for the `w*printf` functions, where a plain `%s`/`%c`
/// means a wide string/character.
pub unsafe fn format(fmt: &[u8], wide_ctx: bool, a: &mut Args) -> Vec<u8> {
    let mut out = Vec::with_capacity(fmt.len() + 32);
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= fmt.len() {
            break;
        }
        let mut s = Spec { left: false, plus: false, space: false, alt: false, zero: false, width: 0, prec: None };
        while i < fmt.len() {
            match fmt[i] {
                b'-' => s.left = true,
                b'+' => s.plus = true,
                b' ' => s.space = true,
                b'#' => s.alt = true,
                b'0' => s.zero = true,
                _ => break,
            }
            i += 1;
        }
        if i < fmt.len() && fmt[i] == b'*' {
            let w = a.next_i32();
            if w < 0 {
                s.left = true;
            }
            s.width = w.unsigned_abs() as usize;
            i += 1;
        } else {
            while i < fmt.len() && fmt[i].is_ascii_digit() {
                s.width = s.width * 10 + (fmt[i] - b'0') as usize;
                i += 1;
            }
        }
        if i < fmt.len() && fmt[i] == b'.' {
            i += 1;
            if i < fmt.len() && fmt[i] == b'*' {
                let p = a.next_i32();
                s.prec = if p >= 0 { Some(p as usize) } else { None };
                i += 1;
            } else {
                let mut p = 0usize;
                while i < fmt.len() && fmt[i].is_ascii_digit() {
                    p = p * 10 + (fmt[i] - b'0') as usize;
                    i += 1;
                }
                s.prec = Some(p);
            }
        }
        let mut len = Len::None;
        if i < fmt.len() {
            match fmt[i] {
                b'h' => {
                    i += 1;
                    if i < fmt.len() && fmt[i] == b'h' {
                        len = Len::HH;
                        i += 1;
                    } else {
                        len = Len::H;
                    }
                }
                b'l' => {
                    i += 1;
                    if i < fmt.len() && fmt[i] == b'l' {
                        len = Len::LL;
                        i += 1;
                    } else {
                        len = Len::L;
                    }
                }
                b'L' => {
                    len = Len::L;
                    i += 1;
                }
                b'w' => {
                    len = Len::W;
                    i += 1;
                }
                b'z' | b'j' | b't' => {
                    len = Len::Ptr;
                    i += 1;
                }
                b'I' => {
                    i += 1;
                    if fmt[i..].starts_with(b"64") {
                        len = Len::LL;
                        i += 2;
                    } else if fmt[i..].starts_with(b"32") {
                        i += 2;
                    } else {
                        len = Len::Ptr;
                    }
                }
                _ => {}
            }
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        match conv {
            b'%' => out.push(b'%'),
            b'd' | b'i' => {
                let raw = a.next_u64();
                let v: i64 = match len {
                    Len::HH => raw as i8 as i64,
                    Len::H => raw as i16 as i64,
                    Len::LL | Len::Ptr => raw as i64,
                    _ => raw as i32 as i64,
                };
                emit_int(&mut out, &s, v < 0, v.unsigned_abs().to_string(), "");
            }
            b'u' | b'o' | b'x' | b'X' => {
                let raw = a.next_u64();
                let v: u64 = match len {
                    Len::HH => raw as u8 as u64,
                    Len::H => raw as u16 as u64,
                    Len::LL | Len::Ptr => raw,
                    _ => raw as u32 as u64,
                };
                let (digits, prefix) = match conv {
                    b'u' => (v.to_string(), ""),
                    b'o' => (format!("{v:o}"), if s.alt && v != 0 { "0" } else { "" }),
                    b'x' => (format!("{v:x}"), if s.alt && v != 0 { "0x" } else { "" }),
                    _ => (format!("{v:X}"), if s.alt && v != 0 { "0X" } else { "" }),
                };
                emit_int(&mut out, &Spec { plus: false, space: false, ..s }, false, digits, prefix);
            }
            b'p' => {
                let v = a.next_u64();
                pad(&mut out, format!("{v:016X}").as_bytes(), s.width, s.left, b' ');
            }
            b'c' | b'C' => {
                let raw = a.next_u64();
                let narrow = match len {
                    Len::H => true,
                    Len::L | Len::W => false,
                    _ => (conv == b'c') != wide_ctx,
                };
                let body: Vec<u8> = if narrow {
                    vec![raw as u8]
                } else {
                    char::from_u32(raw as u16 as u32).map(|c| c.to_string().into_bytes()).unwrap_or_else(|| vec![b'?'])
                };
                pad(&mut out, &body, s.width, s.left, b' ');
            }
            b's' | b'S' => {
                let p = a.next_u64() as usize;
                let narrow = match len {
                    Len::H => true,
                    Len::L | Len::W => false,
                    _ => (conv == b's') != wide_ctx,
                };
                let body: Vec<u8> = if p == 0 {
                    b"(null)".to_vec()
                } else if narrow {
                    read_narrow(p as *const u8, s.prec).to_vec()
                } else {
                    read_wide(p as *const u16, s.prec)
                };
                pad(&mut out, &body, s.width, s.left, b' ');
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                let v = a.next_f64();
                out.extend(c_float(&s, conv, v));
            }
            b'n' => {
                a.next_u64();
            }
            other => {
                out.push(b'%');
                out.push(other);
            }
        }
    }
    out
}

pub fn to_utf16(bytes: &[u8]) -> Vec<u16> {
    String::from_utf8_lossy(bytes).encode_utf16().collect()
}

pub unsafe fn fmt_narrow(fmt: *const u8, a: &mut Args) -> Vec<u8> {
    format(strings::astr(fmt), false, a)
}

pub unsafe fn fmt_wide(fmt: *const u16, a: &mut Args) -> Vec<u8> {
    format(strings::w_to_string(strings::wstr(fmt)).as_bytes(), true, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(fmt: &str, slots: &[u64]) -> String {
        let mut a = Args::new(slots.as_ptr());
        String::from_utf8(unsafe { format(fmt.as_bytes(), false, &mut a) }).unwrap()
    }

    #[test]
    fn integers() {
        assert_eq!(f("%d|%5d|%-5d|%05d", &[42, 42, 42, 42]), "42|   42|42   |00042");
        assert_eq!(f("%x %X %#x %o", &[255, 255, 255, 8]), "ff FF 0xff 10");
        assert_eq!(f("%d", &[(-7i32) as u32 as u64]), "-7");
        assert_eq!(f("%u", &[(-1i32) as u32 as u64]), "4294967295");
        assert_eq!(f("%ld", &[(-1i32) as u32 as u64]), "-1");
        assert_eq!(f("%I64d", &[1u64 << 40]), "1099511627776");
        assert_eq!(f("%lld %zu %Iu", &[5, 6, 7]), "5 6 7");
        assert_eq!(f("%hhd", &[300]), "44");
        assert_eq!(f("%+d % d", &[5, 5]), "+5  5");
        assert_eq!(f("%.3d", &[5]), "005");
        assert_eq!(f("%*d", &[4, 9]), "   9");
    }

    #[test]
    fn floats() {
        assert_eq!(f("%.2f", &[3.14159f64.to_bits()]), "3.14");
        assert_eq!(f("%8.3f|", &[2.5f64.to_bits()]), "   2.500|");
        assert_eq!(f("%g", &[0.0001f64.to_bits()]), "0.0001");
        assert_eq!(f("%e", &[12345.678f64.to_bits()]), "1.234568e+04");
    }

    #[test]
    fn strings_and_chars() {
        let s = b"hello\0";
        assert_eq!(f("[%s]", &[s.as_ptr() as u64]), "[hello]");
        assert_eq!(f("[%8s]", &[s.as_ptr() as u64]), "[   hello]");
        assert_eq!(f("[%-8s]", &[s.as_ptr() as u64]), "[hello   ]");
        assert_eq!(f("[%.2s]", &[s.as_ptr() as u64]), "[he]");
        assert_eq!(f("%c%c", &[b'o' as u64, b'k' as u64]), "ok");
        assert_eq!(f("%s", &[0]), "(null)");
        let w: Vec<u16> = "wide\0".encode_utf16().collect();
        assert_eq!(f("%ls", &[w.as_ptr() as u64]), "wide");
        assert_eq!(f("%S", &[w.as_ptr() as u64]), "wide");
    }

    #[test]
    fn pointer_is_msvc_style() {
        assert_eq!(f("%p", &[0x1234]), "0000000000001234");
    }

    #[test]
    fn wide_context_swaps_string_meaning() {
        let w: Vec<u16> = "ab\0".encode_utf16().collect();
        let slots = [w.as_ptr() as u64];
        let mut a = Args::new(slots.as_ptr());
        let out = unsafe { format(b"%s", true, &mut a) };
        assert_eq!(out, b"ab");
    }

    #[test]
    fn percent_and_literals() {
        assert_eq!(f("100%% done", &[]), "100% done");
        assert_eq!(f("plain", &[]), "plain");
    }
}
