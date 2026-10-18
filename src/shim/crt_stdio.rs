//! msvcrt / ucrt: stdio. `FILE*` handles are host stdio streams; the three standard streams are
//! presented as entries of an `_iob` array so programs can compare and index them as on Windows.

use super::crt_printf::{self, Args};
use super::crt_startup::errno_ptr;
use crate::runtime::handles::{self, FileObj, Kind};
use crate::runtime::paths::{self, Device};
use crate::runtime::strings::*;
use crate::win::*;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::Mutex;

extern "C" {
    static mut stdin: *mut libc::FILE;
    static mut stdout: *mut libc::FILE;
    static mut stderr: *mut libc::FILE;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Iob {
    ptr: usize,
    cnt: i32,
    base: usize,
    flag: i32,
    file: i32,
    charbuf: i32,
    bufsiz: i32,
    tmpfname: usize,
}

const fn iob(fd: i32) -> Iob {
    Iob { ptr: 0, cnt: 0, base: 0, flag: 0, file: fd, charbuf: 0, bufsiz: 0, tmpfname: 0 }
}

pub static mut IOB: [Iob; 3] = [iob(0), iob(1), iob(2)];
static FD_HANDLES: Mutex<Option<HashMap<i32, HANDLE>>> = Mutex::new(None);

const EOF: i32 = -1;

fn sync_errno() {
    unsafe { *errno_ptr() = *libc::__errno_location() };
}

/// Layout of a stream handed to the program: Microsoft's `_FILEX` (48-byte `FILE` followed by a
/// `CRITICAL_SECTION` that `_lock_file` enters), then our private pointer to the host stream.
const MSFILE_LOCK_OFFSET: usize = 48;
const MSFILE_HOST_OFFSET: usize = 88;
const MSFILE_MAGIC_OFFSET: usize = 96;
const MSFILE_SIZE: usize = 104;
const MSFILE_MAGIC: u64 = 0x4558_4559_4e41_0001;

unsafe fn ms_file_new(host: *mut libc::FILE) -> usize {
    if host.is_null() {
        return 0;
    }
    let p = libc::calloc(1, MSFILE_SIZE) as *mut u8;
    if p.is_null() {
        libc::fclose(host);
        return 0;
    }
    (*(p as *mut Iob)).file = libc::fileno(host);
    *(p.add(MSFILE_HOST_OFFSET) as *mut usize) = host as usize;
    *(p.add(MSFILE_MAGIC_OFFSET) as *mut u64) = MSFILE_MAGIC;
    let _ = MSFILE_LOCK_OFFSET;
    p as usize
}

unsafe fn ms_file_free(p: usize) {
    *((p + MSFILE_MAGIC_OFFSET) as *mut u64) = 0;
    libc::free(p as *mut libc::c_void);
}

fn in_iob(p: usize) -> bool {
    let base = std::ptr::addr_of!(IOB) as usize;
    p >= base && p < base + 3 * std::mem::size_of::<Iob>()
}

pub unsafe fn host_file(p: usize) -> *mut libc::FILE {
    if in_iob(p) {
        let base = std::ptr::addr_of!(IOB) as usize;
        return match (p - base) / std::mem::size_of::<Iob>() {
            0 => stdin,
            1 => stdout,
            _ => stderr,
        };
    }
    if p == 0 || *((p + MSFILE_MAGIC_OFFSET) as *const u64) != MSFILE_MAGIC {
        libc::fflush(std::ptr::null_mut());
        eprintln!("anyexe: invalid FILE* passed to the C runtime (0x{p:x})");
        libc::_exit(3);
    }
    *((p + MSFILE_HOST_OFFSET) as *const usize) as *mut libc::FILE
}

unsafe fn iob_entry(i: usize) -> usize {
    std::ptr::addr_of!(IOB[i.min(2)]) as usize
}

fn mode_for_host(mode: &str) -> CString {
    let mode = mode.split(',').next().unwrap_or("r");
    let mut m: String = mode.chars().filter(|c| matches!(c, 'r' | 'w' | 'a' | '+' | 'b' | 'x')).collect();
    if m.is_empty() {
        m.push('r');
    }
    CString::new(m).unwrap()
}

unsafe fn open_stream(path: &str, mode: &str) -> usize {
    let host = match paths::device(path) {
        Some(Device::Null) => std::path::PathBuf::from("/dev/null"),
        Some(Device::Con) | Some(Device::ConIn) | Some(Device::ConOut) => std::path::PathBuf::from("/dev/tty"),
        None => paths::resolve(path),
    };
    let c = CString::new(host.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
    let f = libc::fopen(c.as_ptr(), mode_for_host(mode).as_ptr());
    if f.is_null() {
        sync_errno();
    }
    ms_file_new(f)
}

unsafe fn out_file(fp: usize, bytes: &[u8]) -> i32 {
    let f = host_file(fp);
    if f.is_null() {
        *errno_ptr() = 22;
        return -1;
    }
    if libc::fwrite(bytes.as_ptr() as *const libc::c_void, 1, bytes.len(), f) != bytes.len() {
        sync_errno();
        return -1;
    }
    bytes.len() as i32
}

unsafe fn write_cbuf(buf: *mut u8, bytes: &[u8]) -> i32 {
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
    *buf.add(bytes.len()) = 0;
    bytes.len() as i32
}

unsafe fn write_wbuf(buf: *mut u16, units: &[u16]) -> i32 {
    std::ptr::copy_nonoverlapping(units.as_ptr(), buf, units.len());
    *buf.add(units.len()) = 0;
    units.len() as i32
}

/// MS `_snprintf`: writes at most `n` bytes, terminates only if there is room, -1 when truncated.
unsafe fn snprintf_ms(buf: *mut u8, n: usize, bytes: &[u8]) -> i32 {
    let k = bytes.len().min(n);
    if k > 0 {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, k);
    }
    if bytes.len() < n {
        *buf.add(bytes.len()) = 0;
    }
    if bytes.len() <= n { bytes.len() as i32 } else { -1 }
}

/// C99 `snprintf`: always terminates, returns the full length.
unsafe fn snprintf_c99(buf: *mut u8, n: usize, bytes: &[u8]) -> i32 {
    if n > 0 {
        let k = bytes.len().min(n - 1);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, k);
        *buf.add(k) = 0;
    }
    bytes.len() as i32
}

unsafe fn swprintf_n(buf: *mut u16, n: usize, bytes: &[u8], c99: bool) -> i32 {
    let w = crt_printf::to_utf16(bytes);
    if n == 0 {
        return if c99 { w.len() as i32 } else { -1 };
    }
    let k = w.len().min(n - if c99 { 1 } else { 0 });
    std::ptr::copy_nonoverlapping(w.as_ptr(), buf, k);
    if w.len() < n {
        *buf.add(w.len()) = 0;
    } else if c99 {
        *buf.add(k) = 0;
    }
    if c99 || w.len() <= n { w.len() as i32 } else { -1 }
}

// ---- variadic entry points: spill the register arguments and hand over a Win64 va_list ---------------
#[macro_export]
macro_rules! va_thunk {
    ($name:ident, 1, $target:path) => {
        #[unsafe(naked)]
        pub unsafe extern "win64" fn $name() {
            std::arch::naked_asm!(
                "mov [rsp+16], rdx",
                "mov [rsp+24], r8",
                "mov [rsp+32], r9",
                "lea rdx, [rsp+16]",
                "jmp {t}",
                t = sym $target,
            );
        }
    };
    ($name:ident, 2, $target:path) => {
        #[unsafe(naked)]
        pub unsafe extern "win64" fn $name() {
            std::arch::naked_asm!(
                "mov [rsp+24], r8",
                "mov [rsp+32], r9",
                "lea r8, [rsp+24]",
                "jmp {t}",
                t = sym $target,
            );
        }
    };
    ($name:ident, 3, $target:path) => {
        #[unsafe(naked)]
        pub unsafe extern "win64" fn $name() {
            std::arch::naked_asm!(
                "mov [rsp+32], r9",
                "lea r9, [rsp+32]",
                "jmp {t}",
                t = sym $target,
            );
        }
    };
}

unsafe extern "win64" fn printf_v(fmt: *const u8, va: *const u64) -> i32 {
    let out = crt_printf::fmt_narrow(fmt, &mut Args::new(va));
    out_file(iob_entry(1), &out)
}
unsafe extern "win64" fn fprintf_v(fp: usize, fmt: *const u8, va: *const u64) -> i32 {
    let out = crt_printf::fmt_narrow(fmt, &mut Args::new(va));
    out_file(fp, &out)
}
unsafe extern "win64" fn sprintf_v(buf: *mut u8, fmt: *const u8, va: *const u64) -> i32 {
    write_cbuf(buf, &crt_printf::fmt_narrow(fmt, &mut Args::new(va)))
}
unsafe extern "win64" fn snprintf_v(buf: *mut u8, n: usize, fmt: *const u8, va: *const u64) -> i32 {
    snprintf_c99(buf, n, &crt_printf::fmt_narrow(fmt, &mut Args::new(va)))
}
unsafe extern "win64" fn snprintf_ms_v(buf: *mut u8, n: usize, fmt: *const u8, va: *const u64) -> i32 {
    snprintf_ms(buf, n, &crt_printf::fmt_narrow(fmt, &mut Args::new(va)))
}
unsafe extern "win64" fn scprintf_v(fmt: *const u8, va: *const u64) -> i32 {
    crt_printf::fmt_narrow(fmt, &mut Args::new(va)).len() as i32
}
unsafe extern "win64" fn sprintf_s_v(buf: *mut u8, size: usize, fmt: *const u8, va: *const u64) -> i32 {
    let out = crt_printf::fmt_narrow(fmt, &mut Args::new(va));
    if out.len() + 1 > size {
        if size > 0 {
            *buf = 0;
        }
        *errno_ptr() = 34;
        return -1;
    }
    write_cbuf(buf, &out)
}
unsafe extern "win64" fn wprintf_v(fmt: *const u16, va: *const u64) -> i32 {
    let out = crt_printf::fmt_wide(fmt, &mut Args::new(va));
    out_file(iob_entry(1), &out)
}
unsafe extern "win64" fn fwprintf_v(fp: usize, fmt: *const u16, va: *const u64) -> i32 {
    let out = crt_printf::fmt_wide(fmt, &mut Args::new(va));
    out_file(fp, &out)
}
unsafe extern "win64" fn swprintf_v(buf: *mut u16, fmt: *const u16, va: *const u64) -> i32 {
    write_wbuf(buf, &crt_printf::to_utf16(&crt_printf::fmt_wide(fmt, &mut Args::new(va))))
}
unsafe extern "win64" fn snwprintf_v(buf: *mut u16, n: usize, fmt: *const u16, va: *const u64) -> i32 {
    swprintf_n(buf, n, &crt_printf::fmt_wide(fmt, &mut Args::new(va)), false)
}
unsafe extern "win64" fn swprintf_s_v(buf: *mut u16, size: usize, fmt: *const u16, va: *const u64) -> i32 {
    let w = crt_printf::to_utf16(&crt_printf::fmt_wide(fmt, &mut Args::new(va)));
    if w.len() + 1 > size {
        if size > 0 {
            *buf = 0;
        }
        return -1;
    }
    write_wbuf(buf, &w)
}

va_thunk!(printf, 1, printf_v);
va_thunk!(printf_s, 1, printf_v);
va_thunk!(fprintf, 2, fprintf_v);
va_thunk!(fprintf_s, 2, fprintf_v);
va_thunk!(sprintf, 2, sprintf_v);
va_thunk!(snprintf, 3, snprintf_v);
va_thunk!(_snprintf, 3, snprintf_ms_v);
va_thunk!(_scprintf, 1, scprintf_v);
va_thunk!(sprintf_s, 3, sprintf_s_v);
va_thunk!(wprintf, 1, wprintf_v);
va_thunk!(fwprintf, 2, fwprintf_v);
va_thunk!(swprintf, 2, swprintf_v);
va_thunk!(_snwprintf, 3, snwprintf_v);
va_thunk!(swprintf_s, 3, swprintf_s_v);

// ---- scanf ----------------------------------------------------------------------------------------------
trait Src {
    fn peek(&mut self) -> Option<u8>;
    fn bump(&mut self);
    fn consumed(&self) -> usize;
}

struct StrSrc {
    p: *const u8,
    pos: usize,
}

impl Src for StrSrc {
    fn peek(&mut self) -> Option<u8> {
        let b = unsafe { *self.p.add(self.pos) };
        (b != 0).then_some(b)
    }
    fn bump(&mut self) {
        self.pos += 1;
    }
    fn consumed(&self) -> usize {
        self.pos
    }
}

struct FileSrc {
    f: *mut libc::FILE,
    pos: usize,
}

impl Src for FileSrc {
    fn peek(&mut self) -> Option<u8> {
        unsafe {
            let c = libc::fgetc(self.f);
            if c == EOF {
                None
            } else {
                libc::ungetc(c, self.f);
                Some(c as u8)
            }
        }
    }
    fn bump(&mut self) {
        unsafe { libc::fgetc(self.f) };
        self.pos += 1;
    }
    fn consumed(&self) -> usize {
        self.pos
    }
}

unsafe fn scan<S: Src>(src: &mut S, fmt: &[u8], args: &mut Args) -> i32 {
    let mut assigned = 0;
    let mut i = 0;
    let mut any_input = false;
    let skip_ws = |s: &mut S| {
        while matches!(s.peek(), Some(b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)) {
            s.bump();
        }
    };
    while i < fmt.len() {
        let c = fmt[i];
        if c.is_ascii_whitespace() {
            skip_ws(src);
            i += 1;
            continue;
        }
        if c != b'%' {
            match src.peek() {
                Some(b) if b == c => {
                    src.bump();
                    any_input = true;
                    i += 1;
                    continue;
                }
                _ => break,
            }
        }
        i += 1;
        if i >= fmt.len() {
            break;
        }
        if fmt[i] == b'%' {
            skip_ws(src);
            if src.peek() == Some(b'%') {
                src.bump();
                i += 1;
                continue;
            }
            break;
        }
        let suppress = fmt[i] == b'*';
        if suppress {
            i += 1;
        }
        let mut width = 0usize;
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            width = width * 10 + (fmt[i] - b'0') as usize;
            i += 1;
        }
        let max = if width == 0 { usize::MAX } else { width };
        let (mut h, mut l, mut ll) = (0, false, false);
        loop {
            match fmt.get(i) {
                Some(b'h') => h += 1,
                Some(b'l') | Some(b'L') | Some(b'w') => {
                    if l {
                        ll = true
                    }
                    l = true
                }
                Some(b'z') | Some(b'j') | Some(b't') => ll = true,
                Some(b'I') => {
                    if fmt[i + 1..].starts_with(b"64") {
                        ll = true;
                        i += 2;
                    } else if fmt[i + 1..].starts_with(b"32") {
                        i += 2;
                    } else {
                        ll = true;
                    }
                }
                _ => break,
            }
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        match conv {
            b'd' | b'i' | b'u' | b'x' | b'X' | b'o' | b'p' => {
                skip_ws(src);
                let mut text = String::new();
                let mut base = match conv {
                    b'x' | b'X' | b'p' => 16,
                    b'o' => 8,
                    b'i' => 0,
                    _ => 10,
                };
                if matches!(src.peek(), Some(b'+' | b'-')) && text.len() < max {
                    text.push(src.peek().unwrap() as char);
                    src.bump();
                }
                if (base == 16 || base == 0) && src.peek() == Some(b'0') {
                    text.push('0');
                    src.bump();
                    if matches!(src.peek(), Some(b'x' | b'X')) && base != 8 {
                        src.bump();
                        base = 16;
                    } else if base == 0 {
                        base = 8;
                    }
                }
                let is_digit = |b: u8| (b as char).to_digit(if base == 0 { 10 } else { base }).is_some();
                while text.len() < max {
                    match src.peek() {
                        Some(b) if is_digit(b) => {
                            text.push(b as char);
                            src.bump();
                        }
                        _ => break,
                    }
                }
                let digits_only: String = text.trim_start_matches(['+', '-']).to_string();
                if digits_only.is_empty() {
                    break;
                }
                any_input = true;
                if suppress {
                    continue;
                }
                let radix = if base == 0 { 10 } else { base };
                let neg = text.starts_with('-');
                let mag = u64::from_str_radix(&digits_only, radix).unwrap_or(u64::MAX);
                let v = if neg { (mag as i64).wrapping_neg() as u64 } else { mag };
                let p = args.next_ptr();
                if ll || conv == b'p' {
                    *(p as *mut u64) = v;
                } else if h == 1 {
                    *(p as *mut u16) = v as u16;
                } else if h >= 2 {
                    *(p as *mut u8) = v as u8;
                } else {
                    *(p as *mut u32) = v as u32;
                }
                assigned += 1;
            }
            b'f' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                skip_ws(src);
                let mut text = String::new();
                while text.len() < max {
                    match src.peek() {
                        Some(b) if b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E' | b'x' | b'X') || b.is_ascii_alphabetic() && text.chars().all(|c| c.is_ascii_alphabetic() || c == '-' || c == '+') => {
                            text.push(b as char);
                            src.bump();
                        }
                        _ => break,
                    }
                }
                let v: Option<f64> = (1..=text.len()).rev().find_map(|n| text[..n].parse::<f64>().ok());
                let Some(v) = v else { break };
                any_input = true;
                if suppress {
                    continue;
                }
                let p = args.next_ptr();
                if l || ll {
                    *(p as *mut f64) = v;
                } else {
                    *(p as *mut f32) = v as f32;
                }
                assigned += 1;
            }
            b's' | b'S' => {
                skip_ws(src);
                let mut bytes: Vec<u8> = Vec::new();
                while bytes.len() < max {
                    match src.peek() {
                        Some(b) if !b.is_ascii_whitespace() => {
                            bytes.push(b);
                            src.bump();
                        }
                        _ => break,
                    }
                }
                if bytes.is_empty() {
                    break;
                }
                any_input = true;
                if suppress {
                    continue;
                }
                let p = args.next_ptr();
                if l {
                    let w = crt_printf::to_utf16(&bytes);
                    write_wbuf(p as *mut u16, &w);
                } else {
                    write_cbuf(p as *mut u8, &bytes);
                }
                assigned += 1;
            }
            b'c' | b'C' => {
                let n = if width == 0 { 1 } else { width };
                let mut bytes = Vec::new();
                for _ in 0..n {
                    match src.peek() {
                        Some(b) => {
                            bytes.push(b);
                            src.bump();
                        }
                        None => break,
                    }
                }
                if bytes.len() < n {
                    break;
                }
                any_input = true;
                if suppress {
                    continue;
                }
                let p = args.next_ptr();
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
                assigned += 1;
            }
            b'[' => {
                let mut negate = false;
                if fmt.get(i) == Some(&b'^') {
                    negate = true;
                    i += 1;
                }
                let mut set: Vec<u8> = Vec::new();
                if fmt.get(i) == Some(&b']') {
                    set.push(b']');
                    i += 1;
                }
                while i < fmt.len() && fmt[i] != b']' {
                    if fmt.get(i + 1) == Some(&b'-') && fmt.get(i + 2).is_some_and(|&e| e != b']') {
                        for ch in fmt[i]..=fmt[i + 2] {
                            set.push(ch);
                        }
                        i += 3;
                    } else {
                        set.push(fmt[i]);
                        i += 1;
                    }
                }
                i += 1;
                let mut bytes = Vec::new();
                while bytes.len() < max {
                    match src.peek() {
                        Some(b) if set.contains(&b) != negate => {
                            bytes.push(b);
                            src.bump();
                        }
                        _ => break,
                    }
                }
                if bytes.is_empty() {
                    break;
                }
                any_input = true;
                if suppress {
                    continue;
                }
                let p = args.next_ptr();
                write_cbuf(p as *mut u8, &bytes);
                assigned += 1;
            }
            b'n' => {
                if !suppress {
                    let p = args.next_ptr();
                    *(p as *mut i32) = src.consumed() as i32;
                }
            }
            _ => break,
        }
    }
    if assigned == 0 && !any_input && src.peek().is_none() {
        EOF
    } else {
        assigned
    }
}

impl Args {
    unsafe fn next_ptr(&mut self) -> usize {
        self.next_raw() as usize
    }
}

unsafe extern "win64" fn sscanf_v(s: *const u8, fmt: *const u8, va: *const u64) -> i32 {
    scan(&mut StrSrc { p: s, pos: 0 }, astr(fmt), &mut Args::new(va))
}
unsafe extern "win64" fn fscanf_v(fp: usize, fmt: *const u8, va: *const u64) -> i32 {
    scan(&mut FileSrc { f: host_file(fp), pos: 0 }, astr(fmt), &mut Args::new(va))
}
unsafe extern "win64" fn scanf_v(fmt: *const u8, va: *const u64) -> i32 {
    scan(&mut FileSrc { f: stdin, pos: 0 }, astr(fmt), &mut Args::new(va))
}
unsafe extern "win64" fn swscanf_v(s: *const u16, fmt: *const u16, va: *const u64) -> i32 {
    let mut text = w_to_string(wstr(s)).into_bytes();
    text.push(0);
    scan(&mut StrSrc { p: text.as_ptr(), pos: 0 }, w_to_string(wstr(fmt)).as_bytes(), &mut Args::new(va))
}

va_thunk!(sscanf, 2, sscanf_v);
va_thunk!(fscanf, 2, fscanf_v);
va_thunk!(scanf, 1, scanf_v);
va_thunk!(swscanf, 2, swscanf_v);

// ---- file system helpers -------------------------------------------------------------------------------
unsafe fn fill_stat64(out: *mut u8, st: &libc::stat, size32: bool) {
    let put = |off: usize, v: i64, n: usize| {
        let b = v.to_le_bytes();
        std::ptr::copy_nonoverlapping(b.as_ptr(), out.add(off), n);
    };
    std::ptr::write_bytes(out, 0, if size32 { 48 } else { 56 });
    put(0, st.st_dev as i64, 4);
    put(4, (st.st_ino & 0xffff) as i64, 2);
    put(6, (st.st_mode & 0xffff) as i64, 2);
    put(8, st.st_nlink as i64, 2);
    put(16, st.st_rdev as i64, 4);
    if size32 {
        put(20, st.st_size, 4);
        put(24, st.st_atime, 8);
        put(32, st.st_mtime, 8);
        put(40, st.st_ctime, 8);
    } else {
        put(24, st.st_size, 8);
        put(32, st.st_atime, 8);
        put(40, st.st_mtime, 8);
        put(48, st.st_ctime, 8);
    }
}

unsafe fn stat_path_into(path: &str, out: *mut u8, size32: bool) -> i32 {
    let host = paths::resolve(path);
    let c = CString::new(host.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
    let mut st: libc::stat = std::mem::zeroed();
    if libc::stat(c.as_ptr(), &mut st) != 0 {
        sync_errno();
        return -1;
    }
    fill_stat64(out, &st, size32);
    0
}

unsafe fn host_path_c(p: *const u8) -> CString {
    let host = paths::resolve(&a_to_string(p));
    CString::new(host.as_os_str().to_string_lossy().into_owned()).unwrap_or_default()
}

fn oflag_to_host(o: i32) -> i32 {
    let mut f = match o & 3 {
        1 => libc::O_WRONLY,
        2 => libc::O_RDWR,
        _ => libc::O_RDONLY,
    };
    if o & 0x8 != 0 {
        f |= libc::O_APPEND;
    }
    if o & 0x100 != 0 {
        f |= libc::O_CREAT;
    }
    if o & 0x200 != 0 {
        f |= libc::O_TRUNC;
    }
    if o & 0x400 != 0 {
        f |= libc::O_EXCL;
    }
    f | libc::O_CLOEXEC
}

pub fn fd_to_handle(fd: i32) -> HANDLE {
    if (0..3).contains(&fd) {
        return handles::std_handle(match fd {
            0 => STD_INPUT_HANDLE,
            1 => STD_OUTPUT_HANDLE,
            _ => STD_ERROR_HANDLE,
        });
    }
    let mut g = FD_HANDLES.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    if let Some(h) = m.get(&fd) {
        return *h;
    }
    let h = handles::insert(Kind::File(FileObj::new(fd, false)));
    m.insert(fd, h);
    h
}

crate::shims! { "crt";
    fn __iob_func() -> usize {
        iob_entry(0)
    }
    fn __acrt_iob_func(i: u32) -> usize {
        iob_entry(i as usize)
    }
    fn fopen(path: LPCSTR, mode: LPCSTR) -> usize {
        open_stream(&a_to_string(path), &a_to_string(mode))
    }
    fn _fsopen(path: LPCSTR, mode: LPCSTR, _share: i32) -> usize {
        open_stream(&a_to_string(path), &a_to_string(mode))
    }
    fn _wfopen(path: LPCWSTR, mode: LPCWSTR) -> usize {
        open_stream(&w_ptr_to_string(path), &w_ptr_to_string(mode))
    }
    fn fopen_s(out: *mut usize, path: LPCSTR, mode: LPCSTR) -> i32 {
        let f = open_stream(&a_to_string(path), &a_to_string(mode));
        *out = f;
        if f == 0 { *errno_ptr() } else { 0 }
    }
    fn _wfopen_s(out: *mut usize, path: LPCWSTR, mode: LPCWSTR) -> i32 {
        let f = open_stream(&w_ptr_to_string(path), &w_ptr_to_string(mode));
        *out = f;
        if f == 0 { *errno_ptr() } else { 0 }
    }
    fn freopen(path: LPCSTR, mode: LPCSTR, fp: usize) -> usize {
        let host = paths::resolve(&a_to_string(path));
        let c = CString::new(host.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
        let r = libc::freopen(c.as_ptr(), mode_for_host(&a_to_string(mode)).as_ptr(), host_file(fp));
        if r.is_null() {
            sync_errno();
            return 0;
        }
        if !in_iob(fp) {
            (*(fp as *mut Iob)).file = libc::fileno(r);
        }
        fp
    }
    fn tmpfile() -> usize {
        ms_file_new(libc::tmpfile())
    }
    fn fclose(fp: usize) -> i32 {
        let r = libc::fclose(host_file(fp));
        if !in_iob(fp) {
            ms_file_free(fp);
        }
        r
    }
    fn _fcloseall() -> i32 {
        0
    }
    fn fflush(fp: usize) -> i32 {
        libc::fflush(if fp == 0 { std::ptr::null_mut() } else { host_file(fp) })
    }
    fn _flushall() -> i32 {
        libc::fflush(std::ptr::null_mut());
        0
    }
    fn fread(buf: *mut u8, size: usize, count: usize, fp: usize) -> usize {
        libc::fread(buf as *mut libc::c_void, size, count, host_file(fp))
    }
    fn fwrite(buf: *const u8, size: usize, count: usize, fp: usize) -> usize {
        libc::fwrite(buf as *const libc::c_void, size, count, host_file(fp))
    }
    fn fgets(buf: *mut u8, n: i32, fp: usize) -> *mut u8 {
        libc::fgets(buf as *mut libc::c_char, n, host_file(fp)) as *mut u8
    }
    fn fputs(s: *const u8, fp: usize) -> i32 {
        libc::fputs(s as *const libc::c_char, host_file(fp))
    }
    fn fgetc(fp: usize) -> i32 {
        libc::fgetc(host_file(fp))
    }
    fn fputc(c: i32, fp: usize) -> i32 {
        libc::fputc(c, host_file(fp))
    }
    fn getc(fp: usize) -> i32 {
        libc::fgetc(host_file(fp))
    }
    fn putc(c: i32, fp: usize) -> i32 {
        libc::fputc(c, host_file(fp))
    }
    fn getchar() -> i32 {
        libc::fgetc(stdin)
    }
    fn putchar(c: i32) -> i32 {
        libc::fputc(c, stdout)
    }
    fn puts(s: *const u8) -> i32 {
        if libc::fputs(s as *const libc::c_char, stdout) < 0 {
            return EOF;
        }
        libc::fputc(b'\n' as i32, stdout)
    }
    fn ungetc(c: i32, fp: usize) -> i32 {
        libc::ungetc(c, host_file(fp))
    }
    fn fseek(fp: usize, off: i32, whence: i32) -> i32 {
        libc::fseek(host_file(fp), off as libc::c_long, whence)
    }
    fn _fseeki64(fp: usize, off: i64, whence: i32) -> i32 {
        libc::fseeko(host_file(fp), off as libc::off_t, whence)
    }
    fn ftell(fp: usize) -> i32 {
        let p = libc::ftello(host_file(fp));
        if p > i32::MAX as libc::off_t { -1 } else { p as i32 }
    }
    fn _ftelli64(fp: usize) -> i64 {
        libc::ftello(host_file(fp)) as i64
    }
    fn fgetpos(fp: usize, pos: *mut i64) -> i32 {
        let p = libc::ftello(host_file(fp));
        if p < 0 {
            return -1;
        }
        *pos = p as i64;
        0
    }
    fn fsetpos(fp: usize, pos: *const i64) -> i32 {
        libc::fseeko(host_file(fp), *pos as libc::off_t, libc::SEEK_SET)
    }
    fn rewind(fp: usize) {
        libc::rewind(host_file(fp));
    }
    fn feof(fp: usize) -> i32 {
        libc::feof(host_file(fp))
    }
    fn ferror(fp: usize) -> i32 {
        libc::ferror(host_file(fp))
    }
    fn clearerr(fp: usize) {
        libc::clearerr(host_file(fp));
    }
    fn setvbuf(fp: usize, buf: *mut u8, mode: i32, size: usize) -> i32 {
        libc::setvbuf(host_file(fp), buf as *mut libc::c_char, mode, size)
    }
    fn setbuf(fp: usize, buf: *mut u8) {
        libc::setbuf(host_file(fp), buf as *mut libc::c_char);
    }
    fn _fileno(fp: usize) -> i32 {
        libc::fileno(host_file(fp))
    }
    fn fileno(fp: usize) -> i32 {
        libc::fileno(host_file(fp))
    }
    fn _fdopen(fd: i32, mode: LPCSTR) -> usize {
        ms_file_new(libc::fdopen(fd, mode_for_host(&a_to_string(mode)).as_ptr()))
    }
    fn fdopen(fd: i32, mode: LPCSTR) -> usize {
        ms_file_new(libc::fdopen(fd, mode_for_host(&a_to_string(mode)).as_ptr()))
    }
    fn _lock_file(_fp: usize) {}
    fn _unlock_file(_fp: usize) {}
    fn fputwc(c: u16, fp: usize) -> u16 {
        let s = String::from_utf16_lossy(&[c]);
        if out_file(fp, s.as_bytes()) < 0 { 0xffff } else { c }
    }
    fn putwc(c: u16, fp: usize) -> u16 {
        fputwc(c, fp)
    }
    fn putwchar(c: u16) -> u16 {
        fputwc(c, iob_entry(1))
    }
    fn fputws(s: LPCWSTR, fp: usize) -> i32 {
        if out_file(fp, w_ptr_to_string(s).as_bytes()) < 0 { -1 } else { 0 }
    }
    fn fgetwc(fp: usize) -> u16 {
        let f = host_file(fp);
        let first = libc::fgetc(f);
        if first == EOF {
            return 0xffff;
        }
        let need = match first as u8 {
            0..=0x7f => 0,
            0xc0..=0xdf => 1,
            0xe0..=0xef => 2,
            _ => 3,
        };
        let mut bytes = vec![first as u8];
        for _ in 0..need {
            let c = libc::fgetc(f);
            if c == EOF {
                break;
            }
            bytes.push(c as u8);
        }
        String::from_utf8_lossy(&bytes).encode_utf16().next().unwrap_or(0xfffd)
    }
    fn getwc(fp: usize) -> u16 {
        fgetwc(fp)
    }
    fn getwchar() -> u16 {
        fgetwc(iob_entry(0))
    }

    fn vprintf(fmt: LPCSTR, va: *const u64) -> i32 {
        printf_v(fmt, va)
    }
    fn vfprintf(fp: usize, fmt: LPCSTR, va: *const u64) -> i32 {
        fprintf_v(fp, fmt, va)
    }
    fn vsprintf(buf: LPSTR, fmt: LPCSTR, va: *const u64) -> i32 {
        sprintf_v(buf, fmt, va)
    }
    fn vsnprintf(buf: LPSTR, n: usize, fmt: LPCSTR, va: *const u64) -> i32 {
        snprintf_v(buf, n, fmt, va)
    }
    fn _vsnprintf(buf: LPSTR, n: usize, fmt: LPCSTR, va: *const u64) -> i32 {
        snprintf_ms_v(buf, n, fmt, va)
    }
    fn _vscprintf(fmt: LPCSTR, va: *const u64) -> i32 {
        scprintf_v(fmt, va)
    }
    fn vsprintf_s(buf: LPSTR, size: usize, fmt: LPCSTR, va: *const u64) -> i32 {
        sprintf_s_v(buf, size, fmt, va)
    }
    fn vwprintf(fmt: LPCWSTR, va: *const u64) -> i32 {
        wprintf_v(fmt, va)
    }
    fn vfwprintf(fp: usize, fmt: LPCWSTR, va: *const u64) -> i32 {
        fwprintf_v(fp, fmt, va)
    }
    fn vswprintf(buf: LPWSTR, fmt: LPCWSTR, va: *const u64) -> i32 {
        swprintf_v(buf, fmt, va)
    }
    fn _vsnwprintf(buf: LPWSTR, n: usize, fmt: LPCWSTR, va: *const u64) -> i32 {
        snwprintf_v(buf, n, fmt, va)
    }
    fn _vscwprintf(fmt: LPCWSTR, va: *const u64) -> i32 {
        crt_printf::fmt_wide(fmt, &mut Args::new(va)).len() as i32
    }
    fn __stdio_common_vfprintf(_opts: u64, fp: usize, fmt: LPCSTR, _loc: usize, va: *const u64) -> i32 {
        fprintf_v(fp, fmt, va)
    }
    fn __stdio_common_vfwprintf(_opts: u64, fp: usize, fmt: LPCWSTR, _loc: usize, va: *const u64) -> i32 {
        fwprintf_v(fp, fmt, va)
    }
    fn __stdio_common_vsprintf(opts: u64, buf: LPSTR, n: usize, fmt: LPCSTR, _loc: usize, va: *const u64) -> i32 {
        let out = crt_printf::fmt_narrow(fmt, &mut Args::new(va));
        if opts & 2 != 0 { snprintf_c99(buf, n, &out) } else { snprintf_ms(buf, n, &out) }
    }
    fn __stdio_common_vsprintf_s(_opts: u64, buf: LPSTR, size: usize, fmt: LPCSTR, _loc: usize, va: *const u64) -> i32 {
        sprintf_s_v(buf, size, fmt, va)
    }
    fn __stdio_common_vsnprintf_s(_opts: u64, buf: LPSTR, size: usize, max: usize, fmt: LPCSTR, _loc: usize, va: *const u64) -> i32 {
        let out = crt_printf::fmt_narrow(fmt, &mut Args::new(va));
        let n = out.len().min(max);
        if n + 1 > size {
            if size > 0 {
                *buf = 0;
            }
            return -1;
        }
        write_cbuf(buf, &out[..n])
    }
    fn __stdio_common_vswprintf(opts: u64, buf: LPWSTR, n: usize, fmt: LPCWSTR, _loc: usize, va: *const u64) -> i32 {
        swprintf_n(buf, n, &crt_printf::fmt_wide(fmt, &mut Args::new(va)), opts & 2 != 0)
    }
    fn __stdio_common_vsscanf(_opts: u64, s: LPCSTR, _len: usize, fmt: LPCSTR, _loc: usize, va: *const u64) -> i32 {
        sscanf_v(s, fmt, va)
    }
    fn __stdio_common_vfscanf(_opts: u64, fp: usize, fmt: LPCSTR, _loc: usize, va: *const u64) -> i32 {
        fscanf_v(fp, fmt, va)
    }
    fn vsscanf(s: LPCSTR, fmt: LPCSTR, va: *const u64) -> i32 {
        sscanf_v(s, fmt, va)
    }

    fn _open(path: LPCSTR, oflag: i32, mode: i32) -> i32 {
        let fd = libc::open(host_path_c(path).as_ptr(), oflag_to_host(oflag), if mode == 0 { 0o666 } else { mode } as libc::c_uint);
        if fd < 0 {
            sync_errno();
        }
        fd
    }
    fn _wopen(path: LPCWSTR, oflag: i32, mode: i32) -> i32 {
        let p = to_wide_nul(&w_ptr_to_string(path));
        let _ = p;
        let host = paths::resolve(&w_ptr_to_string(path));
        let c = CString::new(host.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
        let fd = libc::open(c.as_ptr(), oflag_to_host(oflag), if mode == 0 { 0o666 } else { mode } as libc::c_uint);
        if fd < 0 {
            sync_errno();
        }
        fd
    }
    fn _sopen(path: LPCSTR, oflag: i32, _share: i32, mode: i32) -> i32 {
        _open(path, oflag, mode)
    }
    fn _creat(path: LPCSTR, mode: i32) -> i32 {
        _open(path, 0x100 | 0x200 | 1, mode)
    }
    fn _close(fd: i32) -> i32 {
        libc::close(fd)
    }
    fn _read(fd: i32, buf: *mut u8, n: u32) -> i32 {
        let r = libc::read(fd, buf as *mut libc::c_void, n as usize);
        if r < 0 {
            sync_errno();
        }
        r as i32
    }
    fn _write(fd: i32, buf: *const u8, n: u32) -> i32 {
        let r = libc::write(fd, buf as *const libc::c_void, n as usize);
        if r < 0 {
            sync_errno();
        }
        r as i32
    }
    fn _lseek(fd: i32, off: i32, whence: i32) -> i32 {
        libc::lseek(fd, off as libc::off_t, whence) as i32
    }
    fn _lseeki64(fd: i32, off: i64, whence: i32) -> i64 {
        libc::lseek(fd, off as libc::off_t, whence) as i64
    }
    fn _tell(fd: i32) -> i32 {
        libc::lseek(fd, 0, libc::SEEK_CUR) as i32
    }
    fn _telli64(fd: i32) -> i64 {
        libc::lseek(fd, 0, libc::SEEK_CUR) as i64
    }
    fn _eof(fd: i32) -> i32 {
        let cur = libc::lseek(fd, 0, libc::SEEK_CUR);
        let end = libc::lseek(fd, 0, libc::SEEK_END);
        libc::lseek(fd, cur, libc::SEEK_SET);
        (cur >= end) as i32
    }
    fn _dup(fd: i32) -> i32 {
        libc::dup(fd)
    }
    fn _dup2(a: i32, b: i32) -> i32 {
        if libc::dup2(a, b) < 0 { -1 } else { 0 }
    }
    fn _isatty(fd: i32) -> i32 {
        libc::isatty(fd)
    }
    fn _setmode(_fd: i32, _mode: i32) -> i32 {
        0x8000
    }
    fn _commit(fd: i32) -> i32 {
        libc::fsync(fd)
    }
    fn _chsize(fd: i32, size: i32) -> i32 {
        libc::ftruncate(fd, size as libc::off_t)
    }
    fn _chsize_s(fd: i32, size: i64) -> i32 {
        libc::ftruncate(fd, size as libc::off_t)
    }
    fn _filelength(fd: i32) -> i32 {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 { -1 } else { st.st_size as i32 }
    }
    fn _filelengthi64(fd: i32) -> i64 {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 { -1 } else { st.st_size as i64 }
    }
    fn _pipe(fds: *mut i32, _size: u32, _mode: i32) -> i32 {
        libc::pipe(fds)
    }
    fn _get_osfhandle(fd: i32) -> isize {
        if libc::fcntl(fd, libc::F_GETFD) < 0 {
            *errno_ptr() = 9;
            return -1;
        }
        fd_to_handle(fd) as isize
    }
    fn _open_osfhandle(h: isize, _flags: i32) -> i32 {
        match crate::shim::k32_file::host_fd_of(h as HANDLE) {
            Some(fd) => libc::dup(fd),
            None => {
                *errno_ptr() = 9;
                -1
            }
        }
    }
    fn _access(path: LPCSTR, mode: i32) -> i32 {
        let r = libc::access(host_path_c(path).as_ptr(), mode & 6);
        if r != 0 {
            sync_errno();
        }
        r
    }
    fn _access_s(path: LPCSTR, mode: i32) -> i32 {
        if _access(path, mode) == 0 { 0 } else { *errno_ptr() }
    }
    fn _unlink(path: LPCSTR) -> i32 {
        let r = libc::unlink(host_path_c(path).as_ptr());
        if r != 0 {
            sync_errno();
        }
        r
    }
    fn unlink(path: LPCSTR) -> i32 {
        _unlink(path)
    }
    fn remove(path: LPCSTR) -> i32 {
        let c = host_path_c(path);
        let mut r = libc::unlink(c.as_ptr());
        if r != 0 && *libc::__errno_location() == libc::EISDIR {
            r = libc::rmdir(c.as_ptr());
        }
        if r != 0 {
            sync_errno();
        }
        r
    }
    fn _wremove(path: LPCWSTR) -> i32 {
        let host = paths::resolve(&w_ptr_to_string(path));
        match std::fs::remove_file(&host) {
            Ok(()) => 0,
            Err(e) => {
                *errno_ptr() = e.raw_os_error().unwrap_or(2);
                -1
            }
        }
    }
    fn rename(old: LPCSTR, new: LPCSTR) -> i32 {
        let r = libc::rename(host_path_c(old).as_ptr(), host_path_c(new).as_ptr());
        if r != 0 {
            sync_errno();
        }
        r
    }
    fn _mkdir(path: LPCSTR) -> i32 {
        let r = libc::mkdir(host_path_c(path).as_ptr(), 0o777);
        if r != 0 {
            sync_errno();
        }
        r
    }
    fn _rmdir(path: LPCSTR) -> i32 {
        let r = libc::rmdir(host_path_c(path).as_ptr());
        if r != 0 {
            sync_errno();
        }
        r
    }
    fn _chdir(path: LPCSTR) -> i32 {
        let r = libc::chdir(host_path_c(path).as_ptr());
        if r != 0 {
            sync_errno();
        }
        r
    }
    fn _getcwd(buf: LPSTR, n: i32) -> LPSTR {
        let cwd = paths::cwd_win();
        if buf.is_null() {
            let p = libc::malloc(cwd.len() + 1) as *mut u8;
            write_cbuf(p, cwd.as_bytes());
            return p;
        }
        if cwd.len() + 1 > n as usize {
            *errno_ptr() = 34;
            return std::ptr::null_mut();
        }
        write_cbuf(buf, cwd.as_bytes());
        buf
    }
    fn _stat64(path: LPCSTR, out: *mut u8) -> i32 {
        stat_path_into(&a_to_string(path), out, false)
    }
    fn _stat64i32(path: LPCSTR, out: *mut u8) -> i32 {
        stat_path_into(&a_to_string(path), out, true)
    }
    fn _wstat64(path: LPCWSTR, out: *mut u8) -> i32 {
        stat_path_into(&w_ptr_to_string(path), out, false)
    }
    fn _wstat64i32(path: LPCWSTR, out: *mut u8) -> i32 {
        stat_path_into(&w_ptr_to_string(path), out, true)
    }
    fn _fstat64(fd: i32, out: *mut u8) -> i32 {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 {
            sync_errno();
            return -1;
        }
        fill_stat64(out, &st, false);
        0
    }
    fn _fstat64i32(fd: i32, out: *mut u8) -> i32 {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 {
            sync_errno();
            return -1;
        }
        fill_stat64(out, &st, true);
        0
    }
    fn _fullpath(buf: LPSTR, path: LPCSTR, size: usize) -> LPSTR {
        let full = paths::full_path(&a_to_string(path));
        let dst = if buf.is_null() { libc::malloc(full.len() + 1) as *mut u8 } else { buf };
        if buf.is_null() || full.len() < size {
            write_cbuf(dst, full.as_bytes());
            dst
        } else {
            *errno_ptr() = 34;
            std::ptr::null_mut()
        }
    }
    fn _splitpath(path: LPCSTR, drive: LPSTR, dir: LPSTR, fname: LPSTR, ext: LPSTR) {
        let p = a_to_string(path);
        let (d, rest) = match paths::has_drive(&p) {
            Some(_) => p.split_at(2),
            None => ("", p.as_str()),
        };
        let slash = rest.rfind(['\\', '/']).map(|i| i + 1).unwrap_or(0);
        let (dirp, file) = rest.split_at(slash);
        let dot = file.rfind('.').unwrap_or(file.len());
        let (name, e) = file.split_at(dot);
        for (dst, src) in [(drive, d), (dir, dirp), (fname, name), (ext, e)] {
            if !dst.is_null() {
                write_cbuf(dst, src.as_bytes());
            }
        }
    }
    fn _makepath(buf: LPSTR, drive: LPCSTR, dir: LPCSTR, fname: LPCSTR, ext: LPCSTR) {
        let s = |p: LPCSTR| if p.is_null() { String::new() } else { a_to_string(p) };
        let (d, di, f, e) = (s(drive), s(dir), s(fname), s(ext));
        let mut out = String::new();
        if !d.is_empty() {
            out.push_str(&d);
            if !d.ends_with(':') {
                out.push(':');
            }
        }
        out.push_str(&di);
        if !di.is_empty() && !di.ends_with(['\\', '/']) {
            out.push('\\');
        }
        out.push_str(&f);
        if !e.is_empty() && !e.starts_with('.') {
            out.push('.');
        }
        out.push_str(&e);
        write_cbuf(buf, out.as_bytes());
    }
}

crate::shim_extra!("crt";
    "printf" => printf,
    "printf_s" => printf_s,
    "fprintf" => fprintf,
    "fprintf_s" => fprintf_s,
    "sprintf" => sprintf,
    "snprintf" => snprintf,
    "_snprintf" => _snprintf,
    "_scprintf" => _scprintf,
    "sprintf_s" => sprintf_s,
    "wprintf" => wprintf,
    "fwprintf" => fwprintf,
    "swprintf" => swprintf,
    "_snwprintf" => _snwprintf,
    "swprintf_s" => swprintf_s,
    "sscanf" => sscanf,
    "fscanf" => fscanf,
    "scanf" => scanf,
    "swscanf" => swscanf,
    "_iob" => std::ptr::addr_of!(IOB),
);

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn run(fmt: &str, input: &str, slots: &[u64]) -> i32 {
        let mut i = input.as_bytes().to_vec();
        i.push(0);
        scan(&mut StrSrc { p: i.as_ptr(), pos: 0 }, fmt.as_bytes(), &mut Args::new(slots.as_ptr()))
    }

    #[test]
    fn scanf_basic() {
        unsafe {
            let (mut a, mut b) = (0i32, 0f64);
            let mut s = [0u8; 16];
            let slots = [&mut a as *mut i32 as u64, s.as_mut_ptr() as u64, &mut b as *mut f64 as u64];
            let n = run("%d %s %lf", "42 hello 2.5", &slots);
            assert_eq!(n, 3);
            assert_eq!(a, 42);
            assert_eq!(&s[..6], b"hello\0");
            assert_eq!(b, 2.5);
        }
    }

    #[test]
    fn scanf_hex_and_literals() {
        unsafe {
            let (mut a, mut b) = (0u32, 0i32);
            let slots = [&mut a as *mut u32 as u64, &mut b as *mut i32 as u64];
            assert_eq!(run("0x%x,%d", "0xff,-3", &slots), 2);
            assert_eq!((a, b), (255, -3));
        }
    }

    #[test]
    fn scanf_failure_and_eof() {
        unsafe {
            let mut a = 0i32;
            let slots = [&mut a as *mut i32 as u64];
            assert_eq!(run("%d", "abc", &slots), 0);
            assert_eq!(run("%d", "", &slots), EOF);
        }
    }

    #[test]
    fn stat_layout() {
        unsafe {
            let mut st: libc::stat = std::mem::zeroed();
            st.st_size = 1234;
            st.st_mode = 0o100644;
            let mut buf = [0u8; 56];
            fill_stat64(buf.as_mut_ptr(), &st, false);
            assert_eq!(u16::from_le_bytes([buf[6], buf[7]]), 0o100644);
            assert_eq!(i64::from_le_bytes(buf[24..32].try_into().unwrap()), 1234);
        }
    }
}
