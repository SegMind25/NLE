//! advapi32 / shlwapi: the registry.
//!
//! A small hive kept in memory and saved to `<prefix>/registry.dat`, so values a program writes
//! are still there the next time it runs and are seen by its child processes. The file is checked
//! for changes on every call, which keeps processes that share a prefix in step.
//!
//! The five root keys are separate trees (no merged view of HKEY_CLASSES_ROOT). Strings are stored
//! as UTF-16, whichever API wrote them.

use crate::runtime::paths;
use crate::runtime::strings::*;
use crate::win::*;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;
const REG_MULTI_SZ: u32 = 7;
const REG_QWORD: u32 = 11;

const ERROR_KEY_DELETED: u32 = 1018;
const ERROR_UNSUPPORTED_TYPE: u32 = 1630;
const REG_CREATED_NEW_KEY: u32 = 1;
const REG_OPENED_EXISTING_KEY: u32 = 2;

const RRF_RT_REG_SZ: u32 = 0x02;
const RRF_RT_REG_EXPAND_SZ: u32 = 0x04;
const RRF_RT_REG_BINARY: u32 = 0x08;
const RRF_RT_REG_DWORD: u32 = 0x10;
const RRF_RT_REG_MULTI_SZ: u32 = 0x20;
const RRF_RT_REG_QWORD: u32 = 0x40;
const RRF_NOEXPAND: u32 = 0x1000_0000;
const RRF_ZEROONFAILURE: u32 = 0x2000_0000;

const ROOTS: [&str; 5] = ["hkey_classes_root", "hkey_current_user", "hkey_local_machine", "hkey_users", "hkey_current_config"];
const FIRST_DYNAMIC_HANDLE: usize = 0x7000_0001;

#[derive(Clone, Debug)]
struct Value {
    name: String,
    ty: u32,
    data: Vec<u8>,
}

#[derive(Clone, Default, Debug)]
struct Node {
    name: String,
    stamp: u64,
    subkeys: BTreeMap<String, Node>,
    values: BTreeMap<String, Value>,
}

struct State {
    roots: BTreeMap<String, Node>,
    /// Open key handle -> lower-case path from the root, e.g. `hkey_local_machine\software\foo`.
    handles: HashMap<usize, String>,
    next_handle: usize,
    file_stamp: Option<SystemTime>,
}

fn now_filetime() -> u64 {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (d.as_secs() + 11_644_473_600) * 10_000_000 + d.subsec_nanos() as u64 / 100
}

fn components(path: &str) -> Vec<&str> {
    path.split('\\').filter(|c| !c.is_empty()).collect()
}

fn utf16le(s: &str, nul: bool) -> Vec<u8> {
    let mut v: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    if nul {
        v.extend([0, 0]);
    }
    v
}

fn utf16_to_string(data: &[u8]) -> String {
    let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

impl State {
    fn empty() -> State {
        let mut roots = BTreeMap::new();
        for r in ROOTS {
            roots.insert(r.to_string(), Node { name: r.to_uppercase(), ..Node::default() });
        }
        State { roots, handles: HashMap::new(), next_handle: FIRST_DYNAMIC_HANDLE, file_stamp: None }
    }

    fn node(&self, path: &str) -> Option<&Node> {
        let mut parts = path.split('\\');
        let mut n = self.roots.get(parts.next()?)?;
        for p in parts {
            n = n.subkeys.get(p)?;
        }
        Some(n)
    }

    fn node_mut(&mut self, path: &str) -> Option<&mut Node> {
        let mut parts = path.split('\\');
        let mut n = self.roots.get_mut(parts.next()?)?;
        for p in parts {
            n = n.subkeys.get_mut(p)?;
        }
        Some(n)
    }

    fn path_of(&self, h: usize) -> Result<String, u32> {
        if let Some(r) = predefined(h) {
            return Ok(r.to_string());
        }
        match self.handles.get(&h) {
            Some(p) if self.node(p).is_some() => Ok(p.clone()),
            Some(_) => Err(ERROR_KEY_DELETED),
            None => Err(ERROR_INVALID_HANDLE),
        }
    }

    /// Opens (or creates) `sub` below the key `h`. Returns the new handle and whether a key was created.
    fn open(&mut self, h: usize, sub: &str, create: bool) -> Result<(usize, bool), u32> {
        let base = self.path_of(h)?;
        let mut path = base;
        let mut created = false;
        for comp in components(sub) {
            let lower = comp.to_lowercase();
            let parent = self.node_mut(&path).ok_or(ERROR_KEY_DELETED)?;
            if !parent.subkeys.contains_key(&lower) {
                if !create {
                    return Err(ERROR_FILE_NOT_FOUND);
                }
                parent.subkeys.insert(lower.clone(), Node { name: comp.to_string(), stamp: now_filetime(), ..Node::default() });
                parent.stamp = now_filetime();
                created = true;
            }
            path = format!("{path}\\{lower}");
        }
        let handle = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(handle, path);
        Ok((handle, created))
    }

    fn close(&mut self, h: usize) -> u32 {
        if predefined(h).is_some() || self.handles.remove(&h).is_some() {
            ERROR_SUCCESS
        } else {
            ERROR_INVALID_HANDLE
        }
    }

    fn set_value(&mut self, h: usize, name: &str, ty: u32, data: Vec<u8>) -> u32 {
        let path = match self.path_of(h) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let n = self.node_mut(&path).unwrap();
        n.values.insert(name.to_lowercase(), Value { name: name.to_string(), ty, data });
        n.stamp = now_filetime();
        ERROR_SUCCESS
    }

    fn get_value(&self, h: usize, name: &str) -> Result<&Value, u32> {
        let path = self.path_of(h)?;
        self.node(&path).unwrap().values.get(&name.to_lowercase()).ok_or(ERROR_FILE_NOT_FOUND)
    }

    fn delete_value(&mut self, h: usize, name: &str) -> u32 {
        let path = match self.path_of(h) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let n = self.node_mut(&path).unwrap();
        if n.values.remove(&name.to_lowercase()).is_some() {
            n.stamp = now_filetime();
            ERROR_SUCCESS
        } else {
            ERROR_FILE_NOT_FOUND
        }
    }

    /// Deletes the key `sub` below `h`. Without `recursive`, a key that has subkeys is refused (as on Windows).
    /// An empty `sub` with `recursive` clears `h` itself.
    fn delete_key(&mut self, h: usize, sub: &str, recursive: bool) -> u32 {
        let base = match self.path_of(h) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let comps = components(sub);
        let Some((last, parents)) = comps.split_last() else {
            if recursive {
                let n = self.node_mut(&base).unwrap();
                n.subkeys.clear();
                n.values.clear();
                n.stamp = now_filetime();
                return ERROR_SUCCESS;
            }
            return ERROR_ACCESS_DENIED;
        };
        let mut path = base;
        for p in parents {
            path = format!("{path}\\{}", p.to_lowercase());
        }
        let Some(parent) = self.node_mut(&path) else { return ERROR_FILE_NOT_FOUND };
        let key = last.to_lowercase();
        match parent.subkeys.get(&key) {
            None => ERROR_FILE_NOT_FOUND,
            Some(k) if !recursive && !k.subkeys.is_empty() => ERROR_ACCESS_DENIED,
            Some(_) => {
                parent.subkeys.remove(&key);
                parent.stamp = now_filetime();
                ERROR_SUCCESS
            }
        }
    }

    fn enum_key(&self, h: usize, index: usize) -> Result<(&Node, u64), u32> {
        let path = self.path_of(h)?;
        let n = self.node(&path).unwrap();
        n.subkeys.values().nth(index).map(|k| (k, k.stamp)).ok_or(ERROR_NO_MORE_ITEMS)
    }

    fn enum_value(&self, h: usize, index: usize) -> Result<&Value, u32> {
        let path = self.path_of(h)?;
        self.node(&path).unwrap().values.values().nth(index).ok_or(ERROR_NO_MORE_ITEMS)
    }

    fn node_of(&self, h: usize) -> Result<&Node, u32> {
        let path = self.path_of(h)?;
        Ok(self.node(&path).unwrap())
    }
}

fn predefined(h: usize) -> Option<&'static str> {
    let low = h as u32;
    let high = (h as u64) >> 32;
    if low & 0x8000_0000 == 0 || !(high == 0 || high == 0xffff_ffff) {
        return None;
    }
    match low {
        0x8000_0000 => Some(ROOTS[0]),
        0x8000_0001 => Some(ROOTS[1]),
        0x8000_0002 => Some(ROOTS[2]),
        0x8000_0003 => Some(ROOTS[3]),
        0x8000_0005 => Some(ROOTS[4]),
        _ => None,
    }
}

// ---- persistence -----------------------------------------------------------------------------

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            '\t' => out.push_str("%09"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

fn write_node(out: &mut String, path: &str, n: &Node) {
    out.push_str(&format!("K\t{}\t{}\n", escape(path), n.stamp));
    for v in n.values.values() {
        out.push_str(&format!("V\t{}\t{}\t{}\n", escape(&v.name), v.ty, hex(&v.data)));
    }
    for k in n.subkeys.values() {
        write_node(out, &format!("{path}\\{}", k.name), k);
    }
}

impl State {
    fn serialize(&self) -> String {
        let mut out = String::from("NLE-REGISTRY 1\n");
        for (r, n) in &self.roots {
            write_node(&mut out, r, n);
        }
        out
    }

    fn parse(text: &str) -> State {
        let mut st = State::empty();
        let mut current: Option<String> = None;
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split('\t').collect();
            match (f.first().copied(), f.len()) {
                (Some("K"), 3) => {
                    let path = unescape(f[1]);
                    let stamp = f[2].parse().unwrap_or(0);
                    let comps = components(&path);
                    let Some(root) = comps.first().map(|r| r.to_lowercase()) else { continue };
                    let Some(mut n) = st.roots.get_mut(&root) else {
                        current = None;
                        continue;
                    };
                    for c in &comps[1..] {
                        n = n.subkeys.entry(c.to_lowercase()).or_insert_with(|| Node { name: c.to_string(), ..Node::default() });
                    }
                    n.stamp = stamp;
                    current = Some(comps.iter().map(|c| c.to_lowercase()).collect::<Vec<_>>().join("\\"));
                }
                (Some("V"), 4) => {
                    let Some(p) = &current else { continue };
                    let Some(n) = st.node_mut(p) else { continue };
                    let name = unescape(f[1]);
                    n.values.insert(name.to_lowercase(), Value { name, ty: f[2].parse().unwrap_or(REG_BINARY), data: unhex(f[3]) });
                }
                _ => {}
            }
        }
        st
    }
}

fn registry_file() -> PathBuf {
    paths::prefix().join("registry.dat")
}

fn file_stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn save(st: &mut State, path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, st.serialize()).is_ok() && std::fs::rename(&tmp, path).is_ok() {
        st.file_stamp = file_stamp(path);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn machine_guid() -> String {
    use std::hash::{Hash, Hasher};
    let id = std::fs::read_to_string("/etc/machine-id").unwrap_or_else(|_| crate::runtime::host_name());
    let mut a = std::collections::hash_map::DefaultHasher::new();
    id.trim().hash(&mut a);
    let mut b = std::collections::hash_map::DefaultHasher::new();
    (id.trim(), 1u8).hash(&mut b);
    let (x, y) = (a.finish(), b.finish());
    format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", (x >> 32) as u32, (x >> 16) as u16, x as u16, (y >> 48) as u16, y & 0xffff_ffff_ffff)
}

fn cpu_info() -> (String, String, u32) {
    let text = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let field = |key: &str| text.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_once(':')).map(|(_, v)| v.trim().to_string());
    let mhz = field("cpu MHz").and_then(|m| m.parse::<f64>().ok()).unwrap_or(2000.0) as u32;
    (field("model name").unwrap_or_else(|| "Intel(R) Core(TM) Processor".into()), field("vendor_id").unwrap_or_else(|| "GenuineIntel".into()), mhz)
}

/// What a fresh Windows 10 install would answer to the usual "which Windows is this" questions.
fn seed(st: &mut State) {
    let user = crate::runtime::user_name();
    fn put(st: &mut State, key: &str, name: &str, ty: u32, data: Vec<u8>) {
        let (root, sub) = key.split_once('\\').unwrap_or((key, ""));
        let rootkey = if root == "HKLM" { 0x8000_0002usize } else { 0x8000_0001usize };
        let (h, _) = st.open(rootkey, sub, true).unwrap();
        st.set_value(h, name, ty, data);
        st.close(h);
    }
    let sz = |s: &str| utf16le(s, true);
    let dw = |v: u32| v.to_le_bytes().to_vec();
    let nt = "HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    put(st, nt, "ProductName", REG_SZ, sz("Windows 10 Pro"));
    put(st, nt, "CurrentVersion", REG_SZ, sz("6.3"));
    put(st, nt, "CurrentBuild", REG_SZ, sz("19045"));
    put(st, nt, "CurrentBuildNumber", REG_SZ, sz("19045"));
    put(st, nt, "CurrentMajorVersionNumber", REG_DWORD, dw(10));
    put(st, nt, "CurrentMinorVersionNumber", REG_DWORD, dw(0));
    put(st, nt, "EditionID", REG_SZ, sz("Professional"));
    put(st, nt, "InstallationType", REG_SZ, sz("Client"));
    put(st, nt, "DisplayVersion", REG_SZ, sz("22H2"));
    put(st, nt, "ReleaseId", REG_SZ, sz("2009"));
    put(st, nt, "CSDVersion", REG_SZ, sz(""));
    put(st, nt, "SystemRoot", REG_SZ, sz("C:\\windows"));
    put(st, nt, "PathName", REG_SZ, sz("C:\\windows"));
    let cv = "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion";
    put(st, cv, "ProgramFilesDir", REG_SZ, sz("C:\\Program Files"));
    put(st, cv, "ProgramFilesDir (x86)", REG_SZ, sz("C:\\Program Files (x86)"));
    put(st, cv, "ProgramW6432Dir", REG_SZ, sz("C:\\Program Files"));
    put(st, cv, "CommonFilesDir", REG_SZ, sz("C:\\Program Files\\Common Files"));
    put(st, cv, "CommonFilesDir (x86)", REG_SZ, sz("C:\\Program Files (x86)\\Common Files"));
    put(st, cv, "ProgramFilesPath", REG_EXPAND_SZ, sz("%ProgramFiles%"));
    let cp = "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Nls\\CodePage";
    put(st, cp, "ACP", REG_SZ, sz("65001"));
    put(st, cp, "OEMCP", REG_SZ, sz("65001"));
    put(st, cp, "MACCP", REG_SZ, sz("10000"));
    put(st, "HKLM\\SOFTWARE\\Microsoft\\Cryptography", "MachineGuid", REG_SZ, sz(&machine_guid()));
    let (name, vendor, mhz) = cpu_info();
    let cpu = "HKLM\\HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0";
    put(st, cpu, "ProcessorNameString", REG_SZ, sz(&name));
    put(st, cpu, "VendorIdentifier", REG_SZ, sz(&vendor));
    put(st, cpu, "~MHz", REG_DWORD, dw(mhz));
    put(st, cpu, "Identifier", REG_SZ, sz("Intel64 Family 6 Model 85 Stepping 7"));
    let env = "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment";
    put(st, env, "OS", REG_SZ, sz("Windows_NT"));
    put(st, env, "PROCESSOR_ARCHITECTURE", REG_SZ, sz("AMD64"));
    let intl = "HKCU\\Control Panel\\International";
    put(st, intl, "LocaleName", REG_SZ, sz("en-US"));
    put(st, intl, "sDecimal", REG_SZ, sz("."));
    put(st, intl, "sThousand", REG_SZ, sz(","));
    put(st, intl, "iCountry", REG_SZ, sz("1"));
    put(st, "HKCU\\Environment", "USERNAME", REG_SZ, sz(&user));
    let _ = st.open(0x8000_0001, "Software", true);
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Runs `f` on the registry, first picking up changes another process saved. `f` returns the
/// result and whether it modified anything (the hive is then saved).
fn with_state<R>(f: impl FnOnce(&mut State) -> (R, bool)) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let path = registry_file();
    let stamp = file_stamp(&path);
    let reload = match guard.as_ref() {
        None => true,
        Some(s) => s.file_stamp != stamp,
    };
    if reload {
        let handles = guard.as_ref().map(|s| (s.handles.clone(), s.next_handle));
        let mut st = match std::fs::read_to_string(&path) {
            Ok(t) => State::parse(&t),
            Err(_) => {
                let mut s = State::empty();
                seed(&mut s);
                s.handles.clear();
                s.next_handle = FIRST_DYNAMIC_HANDLE;
                save(&mut s, &path);
                s
            }
        };
        if let Some((h, n)) = handles {
            st.handles = h;
            st.next_handle = n;
        }
        st.file_stamp = file_stamp(&path);
        *guard = Some(st);
    }
    let st = guard.as_mut().unwrap();
    let (r, dirty) = f(st);
    if dirty {
        save(st, &path);
    }
    r
}

// ---- argument conversion ---------------------------------------------------------------------

unsafe fn str_arg(p: *const u8, wide: bool) -> Option<String> {
    if p.is_null() {
        None
    } else if wide {
        Some(w_ptr_to_string(p as *const u16))
    } else {
        Some(a_to_string(p))
    }
}

fn is_text(ty: u32) -> bool {
    matches!(ty, REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ)
}

/// What the ANSI APIs hand us is UTF-8; the hive stores UTF-16.
fn to_stored(ty: u32, bytes: &[u8], wide: bool) -> Vec<u8> {
    if wide || !is_text(ty) {
        bytes.to_vec()
    } else {
        String::from_utf8_lossy(bytes).encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }
}

fn from_stored(ty: u32, data: &[u8], wide: bool) -> Vec<u8> {
    if wide || !is_text(ty) {
        data.to_vec()
    } else {
        let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units).into_bytes()
    }
}

fn unit_len(s: &str, wide: bool) -> usize {
    if wide {
        s.encode_utf16().count()
    } else {
        s.len()
    }
}

/// Writes `s` and a terminator into a caller buffer whose capacity is in characters.
unsafe fn put_text(dst: *mut u8, cap: usize, s: &str, wide: bool) {
    if wide {
        let w: Vec<u16> = s.encode_utf16().collect();
        std::ptr::copy_nonoverlapping(w.as_ptr(), dst as *mut u16, w.len().min(cap.saturating_sub(1)));
        *(dst as *mut u16).add(w.len().min(cap.saturating_sub(1))) = 0;
    } else {
        let n = s.len().min(cap.saturating_sub(1));
        std::ptr::copy_nonoverlapping(s.as_ptr(), dst, n);
        *dst.add(n) = 0;
    }
}

/// Name out-parameters of the enumeration calls: capacity in `*len` (with terminator), result length without.
unsafe fn put_name(dst: *mut u8, len: *mut DWORD, s: &str, wide: bool) -> u32 {
    let need = unit_len(s, wide);
    let cap = if len.is_null() { 0 } else { *len as usize };
    if !len.is_null() {
        *len = need as DWORD;
    }
    if dst.is_null() || cap < need + 1 {
        return ERROR_MORE_DATA;
    }
    put_text(dst, cap, s, wide);
    ERROR_SUCCESS
}

unsafe fn query_value(h: usize, name: Option<String>, ty_out: *mut DWORD, data: *mut u8, len: *mut DWORD, wide: bool) -> u32 {
    let name = name.unwrap_or_default();
    with_state(|st| {
        let r = match st.get_value(h, &name) {
            Ok(v) => {
                let bytes = from_stored(v.ty, &v.data, wide);
                if !ty_out.is_null() {
                    *ty_out = v.ty;
                }
                if len.is_null() {
                    if data.is_null() { ERROR_SUCCESS } else { ERROR_INVALID_PARAMETER }
                } else if data.is_null() {
                    *len = bytes.len() as DWORD;
                    ERROR_SUCCESS
                } else if (*len as usize) < bytes.len() {
                    *len = bytes.len() as DWORD;
                    ERROR_MORE_DATA
                } else {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
                    *len = bytes.len() as DWORD;
                    ERROR_SUCCESS
                }
            }
            Err(e) => e,
        };
        (r, false)
    })
}

unsafe fn set_value(h: usize, name: Option<String>, ty: u32, data: *const u8, len: DWORD, wide: bool) -> u32 {
    let bytes: &[u8] = if data.is_null() || len == 0 { &[] } else { std::slice::from_raw_parts(data, len as usize) };
    let stored = to_stored(ty, bytes, wide);
    with_state(|st| {
        let r = st.set_value(h, &name.unwrap_or_default(), ty, stored);
        (r, r == ERROR_SUCCESS)
    })
}

unsafe fn open_key(h: usize, sub: Option<String>, create: bool, out: *mut usize, disposition: *mut DWORD) -> u32 {
    if out.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    with_state(|st| match st.open(h, sub.as_deref().unwrap_or(""), create) {
        Ok((handle, created)) => {
            *out = handle;
            if !disposition.is_null() {
                *disposition = if created { REG_CREATED_NEW_KEY } else { REG_OPENED_EXISTING_KEY };
            }
            (ERROR_SUCCESS, created)
        }
        Err(e) => (e, false),
    })
}

unsafe fn enum_key(h: usize, index: DWORD, name: *mut u8, len: *mut DWORD, last: *mut FILETIME, wide: bool) -> u32 {
    with_state(|st| {
        let r = match st.enum_key(h, index as usize) {
            Ok((n, stamp)) => {
                let r = put_name(name, len, &n.name, wide);
                if r == ERROR_SUCCESS && !last.is_null() {
                    *last = FILETIME { low: stamp as u32, high: (stamp >> 32) as u32 };
                }
                r
            }
            Err(e) => e,
        };
        (r, false)
    })
}

unsafe fn enum_value(h: usize, index: DWORD, name: *mut u8, len: *mut DWORD, ty: *mut DWORD, data: *mut u8, data_len: *mut DWORD, wide: bool) -> u32 {
    with_state(|st| {
        let r = match st.enum_value(h, index as usize) {
            Ok(v) => {
                let mut r = put_name(name, len, &v.name, wide);
                if !ty.is_null() {
                    *ty = v.ty;
                }
                if !data_len.is_null() {
                    let bytes = from_stored(v.ty, &v.data, wide);
                    if !data.is_null() && (*data_len as usize) >= bytes.len() {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
                    } else if !data.is_null() && r == ERROR_SUCCESS {
                        r = ERROR_MORE_DATA;
                    }
                    *data_len = bytes.len() as DWORD;
                }
                r
            }
            Err(e) => e,
        };
        (r, false)
    })
}

#[allow(clippy::too_many_arguments)]
unsafe fn query_info(h: usize, subkeys: *mut DWORD, max_sub: *mut DWORD, values: *mut DWORD, max_name: *mut DWORD, max_data: *mut DWORD, last: *mut FILETIME, wide: bool) -> u32 {
    with_state(|st| {
        let r = match st.node_of(h) {
            Ok(n) => {
                let set = |p: *mut DWORD, v: usize| {
                    if !p.is_null() {
                        *p = v as DWORD;
                    }
                };
                set(subkeys, n.subkeys.len());
                set(max_sub, n.subkeys.values().map(|k| unit_len(&k.name, wide)).max().unwrap_or(0));
                set(values, n.values.len());
                set(max_name, n.values.values().map(|v| unit_len(&v.name, wide)).max().unwrap_or(0));
                set(max_data, n.values.values().map(|v| from_stored(v.ty, &v.data, wide).len()).max().unwrap_or(0));
                if !last.is_null() {
                    *last = FILETIME { low: n.stamp as u32, high: (n.stamp >> 32) as u32 };
                }
                ERROR_SUCCESS
            }
            Err(e) => e,
        };
        (r, false)
    })
}

unsafe fn delete_key(h: usize, sub: Option<String>, recursive: bool) -> u32 {
    with_state(|st| {
        let r = st.delete_key(h, sub.as_deref().unwrap_or(""), recursive);
        (r, r == ERROR_SUCCESS)
    })
}

unsafe fn delete_value(h: usize, name: Option<String>) -> u32 {
    with_state(|st| {
        let r = st.delete_value(h, &name.unwrap_or_default());
        (r, r == ERROR_SUCCESS)
    })
}

/// `RegGetValue`: open the subkey, read the value, check its type against the allowed set and
/// expand environment references unless told not to.
unsafe fn get_value_filtered(h: usize, sub: Option<String>, name: Option<String>, flags: DWORD, ty_out: *mut DWORD, data: *mut u8, len: *mut DWORD, wide: bool) -> u32 {
    let name = name.unwrap_or_default();
    let r = with_state(|st| {
        let base = match st.path_of(h) {
            Ok(p) => p,
            Err(e) => return (Err(e), false),
        };
        let mut path = base;
        for c in components(sub.as_deref().unwrap_or("")) {
            path = format!("{path}\\{}", c.to_lowercase());
        }
        let Some(n) = st.node(&path) else { return (Err(ERROR_FILE_NOT_FOUND), false) };
        (n.values.get(&name.to_lowercase()).cloned().ok_or(ERROR_FILE_NOT_FOUND), false)
    });
    let mut v = match r {
        Ok(v) => v,
        Err(e) => {
            if flags & RRF_ZEROONFAILURE != 0 && !data.is_null() && !len.is_null() {
                std::ptr::write_bytes(data, 0, *len as usize);
            }
            return e;
        }
    };
    let allowed = match v.ty {
        REG_SZ => RRF_RT_REG_SZ,
        REG_EXPAND_SZ => RRF_RT_REG_EXPAND_SZ | if flags & RRF_NOEXPAND == 0 { RRF_RT_REG_SZ } else { 0 },
        REG_BINARY => RRF_RT_REG_BINARY,
        REG_DWORD => RRF_RT_REG_DWORD,
        REG_MULTI_SZ => RRF_RT_REG_MULTI_SZ,
        REG_QWORD => RRF_RT_REG_QWORD,
        _ => 0,
    };
    if flags & allowed == 0 {
        return ERROR_UNSUPPORTED_TYPE;
    }
    let mut reported = v.ty;
    if v.ty == REG_EXPAND_SZ && flags & RRF_NOEXPAND == 0 {
        let expanded = crate::shim::k32_proc::expand_env(&utf16_to_string(&v.data));
        v.data = utf16le(&expanded, true);
        reported = REG_SZ;
    }
    if !ty_out.is_null() {
        *ty_out = reported;
    }
    let bytes = from_stored(v.ty, &v.data, wide);
    if len.is_null() {
        return if data.is_null() { ERROR_SUCCESS } else { ERROR_INVALID_PARAMETER };
    }
    if data.is_null() {
        *len = bytes.len() as DWORD;
        return ERROR_SUCCESS;
    }
    if (*len as usize) < bytes.len() {
        *len = bytes.len() as DWORD;
        return ERROR_MORE_DATA;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
    *len = bytes.len() as DWORD;
    ERROR_SUCCESS
}

crate::shims! { "advapi32" => REG_SHIMS;
    fn RegOpenKeyExW(k: usize, sub: LPCWSTR, _opt: DWORD, _sam: DWORD, out: *mut usize) -> i32 {
        open_key(k, str_arg(sub as *const u8, true), false, out, std::ptr::null_mut()) as i32
    }
    fn RegOpenKeyExA(k: usize, sub: LPCSTR, _opt: DWORD, _sam: DWORD, out: *mut usize) -> i32 {
        open_key(k, str_arg(sub, false), false, out, std::ptr::null_mut()) as i32
    }
    fn RegOpenKeyW(k: usize, sub: LPCWSTR, out: *mut usize) -> i32 {
        open_key(k, str_arg(sub as *const u8, true), false, out, std::ptr::null_mut()) as i32
    }
    fn RegOpenKeyA(k: usize, sub: LPCSTR, out: *mut usize) -> i32 {
        open_key(k, str_arg(sub, false), false, out, std::ptr::null_mut()) as i32
    }
    fn RegOpenCurrentUser(_sam: DWORD, out: *mut usize) -> i32 {
        open_key(0x8000_0001, None, false, out, std::ptr::null_mut()) as i32
    }
    fn RegCreateKeyExW(k: usize, sub: LPCWSTR, _r: DWORD, _class: LPCWSTR, _opt: DWORD, _sam: DWORD, _sa: usize, out: *mut usize, disp: *mut DWORD) -> i32 {
        open_key(k, str_arg(sub as *const u8, true), true, out, disp) as i32
    }
    fn RegCreateKeyExA(k: usize, sub: LPCSTR, _r: DWORD, _class: LPCSTR, _opt: DWORD, _sam: DWORD, _sa: usize, out: *mut usize, disp: *mut DWORD) -> i32 {
        open_key(k, str_arg(sub, false), true, out, disp) as i32
    }
    fn RegCreateKeyW(k: usize, sub: LPCWSTR, out: *mut usize) -> i32 {
        open_key(k, str_arg(sub as *const u8, true), true, out, std::ptr::null_mut()) as i32
    }
    fn RegCreateKeyA(k: usize, sub: LPCSTR, out: *mut usize) -> i32 {
        open_key(k, str_arg(sub, false), true, out, std::ptr::null_mut()) as i32
    }
    fn RegCloseKey(k: usize) -> i32 {
        with_state(|st| (st.close(k), false)) as i32
    }
    fn RegFlushKey(_k: usize) -> i32 {
        0
    }
    fn RegQueryValueExW(k: usize, name: LPCWSTR, _r: usize, ty: *mut DWORD, data: *mut u8, len: *mut DWORD) -> i32 {
        query_value(k, str_arg(name as *const u8, true), ty, data, len, true) as i32
    }
    fn RegQueryValueExA(k: usize, name: LPCSTR, _r: usize, ty: *mut DWORD, data: *mut u8, len: *mut DWORD) -> i32 {
        query_value(k, str_arg(name, false), ty, data, len, false) as i32
    }
    fn RegSetValueExW(k: usize, name: LPCWSTR, _r: DWORD, ty: DWORD, data: *const u8, len: DWORD) -> i32 {
        set_value(k, str_arg(name as *const u8, true), ty, data, len, true) as i32
    }
    fn RegSetValueExA(k: usize, name: LPCSTR, _r: DWORD, ty: DWORD, data: *const u8, len: DWORD) -> i32 {
        set_value(k, str_arg(name, false), ty, data, len, false) as i32
    }
    fn RegSetKeyValueW(k: usize, sub: LPCWSTR, name: LPCWSTR, ty: DWORD, data: *const u8, len: DWORD) -> i32 {
        let mut h = 0usize;
        let r = open_key(k, str_arg(sub as *const u8, true), true, &mut h, std::ptr::null_mut());
        if r != ERROR_SUCCESS {
            return r as i32;
        }
        let r = set_value(h, str_arg(name as *const u8, true), ty, data, len, true);
        with_state(|st| (st.close(h), false));
        r as i32
    }
    fn RegGetValueW(k: usize, sub: LPCWSTR, name: LPCWSTR, flags: DWORD, ty: *mut DWORD, data: *mut u8, len: *mut DWORD) -> i32 {
        get_value_filtered(k, str_arg(sub as *const u8, true), str_arg(name as *const u8, true), flags, ty, data, len, true) as i32
    }
    fn RegGetValueA(k: usize, sub: LPCSTR, name: LPCSTR, flags: DWORD, ty: *mut DWORD, data: *mut u8, len: *mut DWORD) -> i32 {
        get_value_filtered(k, str_arg(sub, false), str_arg(name, false), flags, ty, data, len, false) as i32
    }
    fn RegDeleteValueW(k: usize, name: LPCWSTR) -> i32 {
        delete_value(k, str_arg(name as *const u8, true)) as i32
    }
    fn RegDeleteValueA(k: usize, name: LPCSTR) -> i32 {
        delete_value(k, str_arg(name, false)) as i32
    }
    fn RegDeleteKeyW(k: usize, sub: LPCWSTR) -> i32 {
        delete_key(k, str_arg(sub as *const u8, true), false) as i32
    }
    fn RegDeleteKeyA(k: usize, sub: LPCSTR) -> i32 {
        delete_key(k, str_arg(sub, false), false) as i32
    }
    fn RegDeleteKeyExW(k: usize, sub: LPCWSTR, _sam: DWORD, _r: DWORD) -> i32 {
        delete_key(k, str_arg(sub as *const u8, true), false) as i32
    }
    fn RegDeleteKeyExA(k: usize, sub: LPCSTR, _sam: DWORD, _r: DWORD) -> i32 {
        delete_key(k, str_arg(sub, false), false) as i32
    }
    fn RegDeleteTreeW(k: usize, sub: LPCWSTR) -> i32 {
        delete_key(k, str_arg(sub as *const u8, true), true) as i32
    }
    fn RegDeleteTreeA(k: usize, sub: LPCSTR) -> i32 {
        delete_key(k, str_arg(sub, false), true) as i32
    }
    fn RegEnumKeyExW(k: usize, i: DWORD, name: LPWSTR, len: *mut DWORD, _r: usize, _class: LPWSTR, _cl: *mut DWORD, last: *mut FILETIME) -> i32 {
        enum_key(k, i, name as *mut u8, len, last, true) as i32
    }
    fn RegEnumKeyExA(k: usize, i: DWORD, name: LPSTR, len: *mut DWORD, _r: usize, _class: LPSTR, _cl: *mut DWORD, last: *mut FILETIME) -> i32 {
        enum_key(k, i, name as *mut u8, len, last, false) as i32
    }
    fn RegEnumKeyW(k: usize, i: DWORD, name: LPWSTR, cap: DWORD) -> i32 {
        let mut len = cap;
        enum_key(k, i, name as *mut u8, &mut len, std::ptr::null_mut(), true) as i32
    }
    fn RegEnumKeyA(k: usize, i: DWORD, name: LPSTR, cap: DWORD) -> i32 {
        let mut len = cap;
        enum_key(k, i, name as *mut u8, &mut len, std::ptr::null_mut(), false) as i32
    }
    fn RegEnumValueW(k: usize, i: DWORD, name: LPWSTR, len: *mut DWORD, _r: usize, ty: *mut DWORD, data: *mut u8, data_len: *mut DWORD) -> i32 {
        enum_value(k, i, name as *mut u8, len, ty, data, data_len, true) as i32
    }
    fn RegEnumValueA(k: usize, i: DWORD, name: LPSTR, len: *mut DWORD, _r: usize, ty: *mut DWORD, data: *mut u8, data_len: *mut DWORD) -> i32 {
        enum_value(k, i, name as *mut u8, len, ty, data, data_len, false) as i32
    }
    fn RegQueryInfoKeyW(k: usize, _class: LPWSTR, _cl: *mut DWORD, _r: usize, subkeys: *mut DWORD, max_sub: *mut DWORD, _max_class: *mut DWORD, values: *mut DWORD, max_name: *mut DWORD, max_data: *mut DWORD, _sd: *mut DWORD, last: *mut FILETIME) -> i32 {
        query_info(k, subkeys, max_sub, values, max_name, max_data, last, true) as i32
    }
    fn RegQueryInfoKeyA(k: usize, _class: LPSTR, _cl: *mut DWORD, _r: usize, subkeys: *mut DWORD, max_sub: *mut DWORD, _max_class: *mut DWORD, values: *mut DWORD, max_name: *mut DWORD, max_data: *mut DWORD, _sd: *mut DWORD, last: *mut FILETIME) -> i32 {
        query_info(k, subkeys, max_sub, values, max_name, max_data, last, false) as i32
    }
    fn RegNotifyChangeKeyValue(_k: usize, _subtree: BOOL, _filter: DWORD, _event: HANDLE, _async: BOOL) -> i32 {
        0
    }
}

crate::shims! { "shlwapi" => SHREG_SHIMS;
    fn SHGetValueW(k: usize, sub: LPCWSTR, name: LPCWSTR, ty: *mut DWORD, data: *mut u8, len: *mut DWORD) -> i32 {
        get_value_filtered(k, str_arg(sub as *const u8, true), str_arg(name as *const u8, true), 0xffff | RRF_NOEXPAND, ty, data, len, true) as i32
    }
    fn SHGetValueA(k: usize, sub: LPCSTR, name: LPCSTR, ty: *mut DWORD, data: *mut u8, len: *mut DWORD) -> i32 {
        get_value_filtered(k, str_arg(sub, false), str_arg(name, false), 0xffff | RRF_NOEXPAND, ty, data, len, false) as i32
    }
    fn SHSetValueW(k: usize, sub: LPCWSTR, name: LPCWSTR, ty: DWORD, data: *const u8, len: DWORD) -> i32 {
        let mut h = 0usize;
        let r = open_key(k, str_arg(sub as *const u8, true), true, &mut h, std::ptr::null_mut());
        if r != ERROR_SUCCESS {
            return r as i32;
        }
        let r = set_value(h, str_arg(name as *const u8, true), ty, data, len, true);
        with_state(|st| (st.close(h), false));
        r as i32
    }
    fn SHDeleteKeyW(k: usize, sub: LPCWSTR) -> i32 {
        delete_key(k, str_arg(sub as *const u8, true), true) as i32
    }
    fn SHDeleteValueW(k: usize, sub: LPCWSTR, name: LPCWSTR) -> i32 {
        let mut h = 0usize;
        let r = open_key(k, str_arg(sub as *const u8, true), false, &mut h, std::ptr::null_mut());
        if r != ERROR_SUCCESS {
            return r as i32;
        }
        let r = delete_value(h, str_arg(name as *const u8, true));
        with_state(|st| (st.close(h), false));
        r as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HKLM: usize = 0x8000_0002;
    const HKCU: usize = 0xffff_ffff_8000_0001;

    fn sz(s: &str) -> Vec<u8> {
        utf16le(s, true)
    }

    #[test]
    fn keys_are_case_insensitive_and_created_on_demand() {
        let mut st = State::empty();
        assert_eq!(st.open(HKLM, "Software\\Acme", false).unwrap_err(), ERROR_FILE_NOT_FOUND);
        let (h, created) = st.open(HKLM, "Software\\Acme\\Tool", true).unwrap();
        assert!(created);
        let (h2, created) = st.open(HKLM, "SOFTWARE\\acme\\TOOL", true).unwrap();
        assert!(!created);
        assert_eq!(st.handles[&h], st.handles[&h2]);
        assert_eq!(st.node("hkey_local_machine\\software\\acme").unwrap().name, "Acme");
    }

    #[test]
    fn sign_extended_predefined_handles_work() {
        let mut st = State::empty();
        assert!(st.open(HKCU, "Software\\X", true).is_ok());
        assert!(st.open(0x8000_0001, "Software\\X", false).is_ok());
        assert_eq!(st.open(0x1234, "", false).unwrap_err(), ERROR_INVALID_HANDLE);
    }

    #[test]
    fn values_roundtrip_and_overwrite() {
        let mut st = State::empty();
        let (h, _) = st.open(HKCU, "Software\\App", true).unwrap();
        assert_eq!(st.set_value(h, "Name", REG_SZ, sz("one")), 0);
        assert_eq!(st.set_value(h, "name", REG_SZ, sz("two")), 0);
        let v = st.get_value(h, "NAME").unwrap();
        assert_eq!(utf16_to_string(&v.data), "two");
        assert_eq!(v.name, "name");
        assert_eq!(st.get_value(h, "missing").unwrap_err(), ERROR_FILE_NOT_FOUND);
        assert_eq!(st.delete_value(h, "Name"), 0);
        assert_eq!(st.delete_value(h, "Name"), ERROR_FILE_NOT_FOUND);
    }

    #[test]
    fn deleting_keys_follows_windows_rules() {
        let mut st = State::empty();
        let (h, _) = st.open(HKCU, "Software\\App\\Inner", true).unwrap();
        st.close(h);
        assert_eq!(st.delete_key(HKCU, "Software\\App", false), ERROR_ACCESS_DENIED);
        assert_eq!(st.delete_key(HKCU, "Software\\App\\Inner", false), 0);
        assert_eq!(st.delete_key(HKCU, "Software\\App", false), 0);
        assert_eq!(st.delete_key(HKCU, "Software\\App", false), ERROR_FILE_NOT_FOUND);
        st.open(HKCU, "Software\\Tree\\A\\B", true).unwrap();
        assert_eq!(st.delete_key(HKCU, "Software\\Tree", true), 0);
        assert!(st.open(HKCU, "Software\\Tree", false).is_err());
    }

    #[test]
    fn open_handle_to_a_deleted_key_reports_it() {
        let mut st = State::empty();
        let (h, _) = st.open(HKCU, "Software\\Gone", true).unwrap();
        st.delete_key(HKCU, "Software\\Gone", false);
        assert_eq!(st.set_value(h, "x", REG_DWORD, vec![1, 0, 0, 0]), ERROR_KEY_DELETED);
    }

    #[test]
    fn enumeration_is_stable_and_ends() {
        let mut st = State::empty();
        let (h, _) = st.open(HKCU, "Software\\E", true).unwrap();
        for n in ["b", "A", "c"] {
            st.open(h, n, true).unwrap();
            st.set_value(h, &format!("v{n}"), REG_DWORD, vec![0; 4]);
        }
        let names: Vec<String> = (0..3).map(|i| st.enum_key(h, i).unwrap().0.name.clone()).collect();
        assert_eq!(names, ["A", "b", "c"]);
        assert_eq!(st.enum_key(h, 3).unwrap_err(), ERROR_NO_MORE_ITEMS);
        assert_eq!(st.enum_value(h, 2).unwrap().name, "vc");
        assert_eq!(st.enum_value(h, 3).unwrap_err(), ERROR_NO_MORE_ITEMS);
    }

    #[test]
    fn hive_survives_a_save_and_load() {
        let mut st = State::empty();
        let (h, _) = st.open(HKLM, "Software\\Weird name\twith tab", true).unwrap();
        st.set_value(h, "Zero\0Byte%", REG_BINARY, vec![0, 1, 2, 255]);
        st.set_value(h, "", REG_SZ, sz("default ü€"));
        st.set_value(h, "Count", REG_DWORD, 7u32.to_le_bytes().to_vec());
        let text = st.serialize();
        let back = State::parse(&text);
        let n = back.node("hkey_local_machine\\software\\weird name\twith tab").expect("key restored");
        assert_eq!(n.name, "Weird name\twith tab");
        assert_eq!(n.values["zero\0byte%"].data, vec![0, 1, 2, 255]);
        assert_eq!(utf16_to_string(&n.values[""].data), "default ü€");
        assert_eq!(n.values["count"].ty, REG_DWORD);
        assert_eq!(back.serialize(), text);
    }

    #[test]
    fn ansi_strings_are_stored_as_utf16() {
        let stored = to_stored(REG_SZ, "héllo\0".as_bytes(), false);
        assert_eq!(utf16_to_string(&stored), "héllo");
        assert_eq!(from_stored(REG_SZ, &stored, false), "héllo\0".as_bytes());
        assert_eq!(to_stored(REG_BINARY, &[1, 2, 3], false), vec![1, 2, 3]);
        assert_eq!(from_stored(REG_SZ, &stored, true), stored);
    }

    #[test]
    fn seed_answers_the_usual_questions() {
        let mut st = State::empty();
        seed(&mut st);
        let (h, _) = st.open(HKLM, "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", false).unwrap();
        assert_eq!(utf16_to_string(&st.get_value(h, "CurrentBuildNumber").unwrap().data), "19045");
        let (h, _) = st.open(HKLM, "SYSTEM\\CurrentControlSet\\Control\\Nls\\CodePage", false).unwrap();
        assert_eq!(utf16_to_string(&st.get_value(h, "ACP").unwrap().data), "65001");
    }

    #[test]
    fn escaping_roundtrips() {
        for s in ["plain", "tab\there", "line\nbreak", "100%", "%41", "é€"] {
            assert_eq!(unescape(&escape(s)), s);
        }
    }
}
