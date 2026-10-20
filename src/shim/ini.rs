//! kernel32 profile (INI) files: `GetPrivateProfileString` and its relatives, the older `win.ini` variants, and the
//! writers. A file is read and rewritten on each call, keeping the lines the program did not touch.

use crate::runtime::paths;
use crate::runtime::strings::*;
use crate::win::*;
use std::sync::Mutex;

/// Profile calls from several threads must not interleave their read-modify-write cycles.
static LOCK: Mutex<()> = Mutex::new(());

/// The text of a profile file as lines, and how to write it back.
#[derive(Default)]
pub struct Ini {
    pub lines: Vec<String>,
    utf16: bool,
}

impl Ini {
    pub fn parse(bytes: &[u8]) -> Ini {
        let (text, utf16) = if bytes.starts_with(&[0xff, 0xfe]) {
            let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            (String::from_utf16_lossy(&units), true)
        } else {
            let b = bytes.strip_prefix(&[0xef, 0xbb, 0xbf][..]).unwrap_or(bytes);
            (String::from_utf8_lossy(b).into_owned(), false)
        };
        Ini { lines: text.lines().map(|l| l.to_string()).collect(), utf16 }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut text = self.lines.join("\r\n");
        if !self.lines.is_empty() {
            text.push_str("\r\n");
        }
        if self.utf16 {
            let mut out = vec![0xff, 0xfe];
            for u in text.encode_utf16() {
                out.extend_from_slice(&u.to_le_bytes());
            }
            out
        } else {
            text.into_bytes()
        }
    }

    fn header(line: &str) -> Option<&str> {
        let t = line.trim();
        let inner = t.strip_prefix('[')?;
        let end = inner.find(']')?;
        Some(inner[..end].trim())
    }

    /// Line range of the section's header and its body: (header index, end index exclusive).
    fn section(&self, name: &str) -> Option<(usize, usize)> {
        let start = self.lines.iter().position(|l| Self::header(l).is_some_and(|h| h.eq_ignore_ascii_case(name)))?;
        let end = self.lines[start + 1..].iter().position(|l| Self::header(l).is_some()).map_or(self.lines.len(), |p| start + 1 + p);
        Some((start, end))
    }

    /// `key=value` of a body line; comments and blank lines have none.
    fn entry(line: &str) -> Option<(&str, &str)> {
        let t = line.trim_start();
        if t.is_empty() || t.starts_with(';') {
            return None;
        }
        let eq = t.find('=')?;
        Some((t[..eq].trim_end(), t[eq + 1..].trim()))
    }

    pub fn section_names(&self) -> Vec<String> {
        self.lines.iter().filter_map(|l| Self::header(l).map(str::to_string)).collect()
    }

    /// Every `(key, raw value)` of a section, or None when the section does not exist.
    pub fn entries(&self, section: &str) -> Option<Vec<(String, String)>> {
        let (s, e) = self.section(section)?;
        Some(self.lines[s + 1..e].iter().filter_map(|l| Self::entry(l)).map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    pub fn get(&self, section: &str, key: &str) -> Option<String> {
        let (s, e) = self.section(section)?;
        self.lines[s + 1..e].iter().filter_map(|l| Self::entry(l)).find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| unquote(v).to_string())
    }

    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let line = format!("{key}={value}");
        match self.section(section) {
            Some((s, e)) => {
                if let Some(i) = (s + 1..e).find(|i| Self::entry(&self.lines[*i]).is_some_and(|(k, _)| k.eq_ignore_ascii_case(key))) {
                    self.lines[i] = line;
                } else {
                    // after the last entry, before trailing blank lines
                    let mut at = e;
                    while at > s + 1 && self.lines[at - 1].trim().is_empty() {
                        at -= 1;
                    }
                    self.lines.insert(at, line);
                }
            }
            None => {
                if self.lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    self.lines.push(String::new());
                }
                self.lines.push(format!("[{section}]"));
                self.lines.push(line);
            }
        }
    }

    pub fn remove_key(&mut self, section: &str, key: &str) {
        if let Some((s, e)) = self.section(section) {
            if let Some(i) = (s + 1..e).find(|i| Self::entry(&self.lines[*i]).is_some_and(|(k, _)| k.eq_ignore_ascii_case(key))) {
                self.lines.remove(i);
            }
        }
    }

    pub fn remove_section(&mut self, section: &str) {
        if let Some((s, e)) = self.section(section) {
            self.lines.drain(s..e);
        }
    }

    pub fn replace_section(&mut self, section: &str, body: &[String]) {
        self.remove_section(section);
        if self.lines.last().is_some_and(|l| !l.trim().is_empty()) {
            self.lines.push(String::new());
        }
        self.lines.push(format!("[{section}]"));
        self.lines.extend(body.iter().cloned());
    }
}

/// Values wrapped in one pair of matching quotes lose them.
fn unquote(v: &str) -> &str {
    let b = v.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

/// The file a profile call names: a bare name lives in the Windows directory, anything with a path is that path.
fn file_path(name: &str) -> std::path::PathBuf {
    if name.contains('\\') || name.contains('/') || name.contains(':') {
        paths::resolve(&paths::full_path(name))
    } else {
        paths::resolve(&format!("C:\\Windows\\{name}"))
    }
}

fn load(name: &str) -> Ini {
    std::fs::read(file_path(name)).map(|b| Ini::parse(&b)).unwrap_or_default()
}

fn store(name: &str, ini: &Ini) -> bool {
    let path = file_path(name);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&path, ini.to_bytes()) {
        Ok(()) => true,
        Err(_) => {
            set_last_error_from_errno();
            false
        }
    }
}

/// UTF-16 units of `items` as the double-NUL terminated list profile calls return, cut to `cap` units.
fn multi(items: &[String], cap: usize) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for i in items {
        out.extend(i.encode_utf16());
        out.push(0);
    }
    out.push(0);
    if out.len() > cap {
        out.truncate(cap.saturating_sub(2));
        out.push(0);
        out.push(0);
    }
    out
}

/// What `GetPrivateProfileString` produces as UTF-16 units: a value, the sections, or a section's keys.
fn lookup(ini: &Ini, app: Option<&str>, key: Option<&str>, default: &str) -> (Vec<u16>, bool) {
    match (app, key) {
        (None, _) => (multi(&ini.section_names(), usize::MAX), true),
        (Some(a), None) => (multi(&ini.entries(a).unwrap_or_default().into_iter().map(|e| e.0).collect::<Vec<_>>(), usize::MAX), true),
        (Some(a), Some(k)) => (ini.get(a, k).unwrap_or_else(|| default.to_string()).encode_utf16().collect(), false),
    }
}

unsafe fn opt_w(p: LPCWSTR) -> Option<String> {
    if p.is_null() { None } else { Some(w_ptr_to_string(p)) }
}

unsafe fn opt_a(p: LPCSTR) -> Option<String> {
    if p.is_null() { None } else { Some(a_to_string(p)) }
}

/// Copies `units` into the caller's buffer the way the profile calls do; the result is the character count.
unsafe fn put_units(units: &[u16], is_list: bool, out: LPWSTR, size: DWORD) -> DWORD {
    if size == 0 {
        return 0;
    }
    let cap = size as usize;
    let body: &[u16] = if is_list { &units[..units.len() - 1] } else { units };
    let tail = if is_list { 2 } else { 1 };
    if body.len() + tail <= cap {
        std::ptr::copy_nonoverlapping(body.as_ptr(), out, body.len());
        *out.add(body.len()) = 0;
        if is_list {
            *out.add(body.len() + 1) = 0;
        }
        return (body.len() - usize::from(is_list)) as DWORD;
    }
    // truncated: as much as fits, still terminated
    let keep = cap - tail;
    std::ptr::copy_nonoverlapping(body.as_ptr(), out, keep);
    *out.add(keep) = 0;
    if is_list {
        *out.add(keep + 1) = 0;
    }
    (cap - tail) as DWORD
}

unsafe fn put_units_a(units: &[u16], is_list: bool, out: LPSTR, size: DWORD) -> DWORD {
    if size == 0 {
        return 0;
    }
    let mut tmp = vec![0u16; size as usize];
    let n = put_units(units, is_list, tmp.as_mut_ptr(), size) as usize;
    let total = if is_list { n + 2 } else { n + 1 };
    let mut at = 0;
    let mut used = 0usize;
    // convert unit by unit so the result stays inside the caller's buffer
    for u in tmp.iter().take(total.min(size as usize)) {
        let mut b = [0u8; 4];
        let s = char::from_u32(*u as u32).unwrap_or('?').encode_utf8(&mut b);
        let bytes = if *u == 0 { &[0u8][..] } else { s.as_bytes() };
        if at + bytes.len() > size as usize {
            break;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.add(at), bytes.len());
        at += bytes.len();
        used += 1;
    }
    let _ = used;
    if at < size as usize {
        *out.add(at) = 0;
    }
    if is_list { at.saturating_sub(2) as DWORD } else { at.saturating_sub(1) as DWORD }
}

fn parse_uint(s: &str, default: u32) -> u32 {
    let t = s.trim();
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() { default } else { digits.parse::<u64>().map_or(default, |v| v as u32) }
}

crate::shims! { "kernel32" => INI_SHIMS;
    fn GetPrivateProfileStringW(app: LPCWSTR, key: LPCWSTR, default: LPCWSTR, out: LPWSTR, size: DWORD, file: LPCWSTR) -> DWORD {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ini = load(&w_ptr_to_string(file));
        let (units, list) = lookup(&ini, opt_w(app).as_deref(), opt_w(key).as_deref(), &opt_w(default).unwrap_or_default());
        put_units(&units, list, out, size)
    }
    fn GetPrivateProfileStringA(app: LPCSTR, key: LPCSTR, default: LPCSTR, out: LPSTR, size: DWORD, file: LPCSTR) -> DWORD {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ini = load(&a_to_string(file));
        let (units, list) = lookup(&ini, opt_a(app).as_deref(), opt_a(key).as_deref(), &opt_a(default).unwrap_or_default());
        put_units_a(&units, list, out, size)
    }
    fn GetPrivateProfileIntW(app: LPCWSTR, key: LPCWSTR, default: i32, file: LPCWSTR) -> u32 {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ini = load(&w_ptr_to_string(file));
        match ini.get(&w_ptr_to_string(app), &w_ptr_to_string(key)) {
            Some(v) => parse_uint(&v, default as u32),
            None => default as u32,
        }
    }
    fn GetPrivateProfileIntA(app: LPCSTR, key: LPCSTR, default: i32, file: LPCSTR) -> u32 {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ini = load(&a_to_string(file));
        match ini.get(&a_to_string(app), &a_to_string(key)) {
            Some(v) => parse_uint(&v, default as u32),
            None => default as u32,
        }
    }
    fn GetPrivateProfileSectionNamesW(out: LPWSTR, size: DWORD, file: LPCWSTR) -> DWORD {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ini = load(&w_ptr_to_string(file));
        put_units(&multi(&ini.section_names(), usize::MAX), true, out, size)
    }
    fn GetPrivateProfileSectionNamesA(out: LPSTR, size: DWORD, file: LPCSTR) -> DWORD {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ini = load(&a_to_string(file));
        put_units_a(&multi(&ini.section_names(), usize::MAX), true, out, size)
    }
    fn GetPrivateProfileSectionW(app: LPCWSTR, out: LPWSTR, size: DWORD, file: LPCWSTR) -> DWORD {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ini = load(&w_ptr_to_string(file));
        let items: Vec<String> = ini.entries(&w_ptr_to_string(app)).unwrap_or_default().into_iter().map(|(k, v)| format!("{k}={v}")).collect();
        put_units(&multi(&items, usize::MAX), true, out, size)
    }
    fn GetPrivateProfileSectionA(app: LPCSTR, out: LPSTR, size: DWORD, file: LPCSTR) -> DWORD {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ini = load(&a_to_string(file));
        let items: Vec<String> = ini.entries(&a_to_string(app)).unwrap_or_default().into_iter().map(|(k, v)| format!("{k}={v}")).collect();
        put_units_a(&multi(&items, usize::MAX), true, out, size)
    }
    fn WritePrivateProfileStringW(app: LPCWSTR, key: LPCWSTR, value: LPCWSTR, file: LPCWSTR) -> BOOL {
        write_string(opt_w(app), opt_w(key), opt_w(value), &w_ptr_to_string(file))
    }
    fn WritePrivateProfileStringA(app: LPCSTR, key: LPCSTR, value: LPCSTR, file: LPCSTR) -> BOOL {
        write_string(opt_a(app), opt_a(key), opt_a(value), &a_to_string(file))
    }
    fn WritePrivateProfileSectionW(app: LPCWSTR, data: LPCWSTR, file: LPCWSTR) -> BOOL {
        let body = if data.is_null() { Vec::new() } else { double_nul(data) };
        write_section(&w_ptr_to_string(app), &body, &w_ptr_to_string(file))
    }
    fn WritePrivateProfileSectionA(app: LPCSTR, data: LPCSTR, file: LPCSTR) -> BOOL {
        let mut body = Vec::new();
        if !data.is_null() {
            let mut p = data;
            loop {
                let s = a_to_string(p);
                if s.is_empty() {
                    break;
                }
                p = p.add(s.len() + 1);
                body.push(s);
            }
        }
        write_section(&a_to_string(app), &body, &a_to_string(file))
    }
    fn GetProfileStringW(app: LPCWSTR, key: LPCWSTR, default: LPCWSTR, out: LPWSTR, size: DWORD) -> DWORD {
        GetPrivateProfileStringW(app, key, default, out, size, to_wide_nul("win.ini").as_ptr())
    }
    fn GetProfileStringA(app: LPCSTR, key: LPCSTR, default: LPCSTR, out: LPSTR, size: DWORD) -> DWORD {
        GetPrivateProfileStringA(app, key, default, out, size, b"win.ini\0".as_ptr())
    }
    fn GetProfileIntW(app: LPCWSTR, key: LPCWSTR, default: i32) -> u32 {
        GetPrivateProfileIntW(app, key, default, to_wide_nul("win.ini").as_ptr())
    }
    fn GetProfileIntA(app: LPCSTR, key: LPCSTR, default: i32) -> u32 {
        GetPrivateProfileIntA(app, key, default, b"win.ini\0".as_ptr())
    }
    fn WriteProfileStringW(app: LPCWSTR, key: LPCWSTR, value: LPCWSTR) -> BOOL {
        WritePrivateProfileStringW(app, key, value, to_wide_nul("win.ini").as_ptr())
    }
    fn WriteProfileStringA(app: LPCSTR, key: LPCSTR, value: LPCSTR) -> BOOL {
        WritePrivateProfileStringA(app, key, value, b"win.ini\0".as_ptr())
    }
}

unsafe fn double_nul(p: *const u16) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        let n = wlen(p.add(at));
        if n == 0 {
            break;
        }
        out.push(String::from_utf16_lossy(std::slice::from_raw_parts(p.add(at), n)));
        at += n + 1;
    }
    out
}

fn write_string(app: Option<String>, key: Option<String>, value: Option<String>, file: &str) -> BOOL {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // a call without a section only asks for the file to be flushed
    let Some(app) = app else { return TRUE };
    let mut ini = load(file);
    match (key, value) {
        (None, _) => ini.remove_section(&app),
        (Some(k), None) => ini.remove_key(&app, &k),
        (Some(k), Some(v)) => ini.set(&app, &k, &v),
    }
    store(file, &ini) as BOOL
}

fn write_section(app: &str, body: &[String], file: &str) -> BOOL {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ini = load(file);
    ini.replace_section(app, body);
    store(file, &ini) as BOOL
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "; settings\r\n[General]\r\nName = \"Ada\"\r\nCount=3\r\n\r\n[Other]\r\nKey=value ; not a comment here\r\n";

    #[test]
    fn values_are_found_trimmed_and_unquoted() {
        let ini = Ini::parse(SAMPLE.as_bytes());
        assert_eq!(ini.get("general", "NAME").as_deref(), Some("Ada"));
        assert_eq!(ini.get("General", "Count").as_deref(), Some("3"));
        assert_eq!(ini.get("Other", "key").as_deref(), Some("value ; not a comment here"));
        assert_eq!(ini.get("General", "Missing"), None);
        assert_eq!(ini.get("Nope", "Name"), None);
    }

    #[test]
    fn sections_and_keys_are_listed_in_file_order() {
        let ini = Ini::parse(SAMPLE.as_bytes());
        assert_eq!(ini.section_names(), vec!["General", "Other"]);
        let keys: Vec<String> = ini.entries("General").unwrap().into_iter().map(|e| e.0).collect();
        assert_eq!(keys, vec!["Name", "Count"]);
    }

    #[test]
    fn writing_keeps_everything_else() {
        let mut ini = Ini::parse(SAMPLE.as_bytes());
        ini.set("General", "Count", "4");
        ini.set("General", "Added", "yes");
        ini.set("Fresh", "A", "1");
        ini.remove_key("Other", "Key");
        let text = String::from_utf8(ini.to_bytes()).unwrap();
        assert!(text.starts_with("; settings\r\n[General]\r\n"));
        assert!(text.contains("Count=4\r\nAdded=yes\r\n\r\n[Other]"));
        assert!(text.ends_with("[Fresh]\r\nA=1\r\n"));
        assert!(!text.contains("Key="));
    }

    #[test]
    fn a_section_can_be_replaced_or_dropped() {
        let mut ini = Ini::parse(SAMPLE.as_bytes());
        ini.replace_section("General", &["x=1".to_string(), "y=2".to_string()]);
        assert_eq!(ini.get("General", "x").as_deref(), Some("1"));
        assert_eq!(ini.get("General", "Name"), None);
        ini.remove_section("Other");
        assert_eq!(ini.section_names(), vec!["General"]);
    }

    #[test]
    fn utf16_files_stay_utf16() {
        let mut bytes = vec![0xff, 0xfe];
        for u in "[S]\r\nk=\u{e9}\r\n".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let mut ini = Ini::parse(&bytes);
        assert_eq!(ini.get("S", "k").as_deref(), Some("\u{e9}"));
        ini.set("S", "n", "1");
        assert!(ini.to_bytes().starts_with(&[0xff, 0xfe]));
    }

    #[test]
    fn lists_end_with_two_nuls_and_truncate_cleanly() {
        let list = multi(&["ab".to_string(), "c".to_string()], usize::MAX);
        assert_eq!(list, vec![b'a' as u16, b'b' as u16, 0, b'c' as u16, 0, 0]);
        let mut buf = [0xffffu16; 8];
        let n = unsafe { put_units(&list, true, buf.as_mut_ptr(), 8) };
        assert_eq!(n, 4);
        assert_eq!(&buf[..6], &list[..]);
        let mut small = [0xffffu16; 4];
        let n = unsafe { put_units(&list, true, small.as_mut_ptr(), 4) };
        assert_eq!(n, 2);
        assert_eq!(small, [b'a' as u16, b'b' as u16, 0, 0]);
    }

    #[test]
    fn integers_parse_like_the_profile_call() {
        assert_eq!(parse_uint("42", 7), 42);
        assert_eq!(parse_uint(" 12abc", 7), 12);
        assert_eq!(parse_uint("abc", 7), 7);
    }
}
