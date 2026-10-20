//! ntdll file system calls (`NtCreateFile`, `NtReadFile`, ...), named pipes and I/O completion ports.

use crate::runtime::handles::{self, Completion, FileObj, IocpObj, Kind};
use crate::runtime::paths;
use crate::runtime::strings::*;
use crate::shim::k32_file;
use crate::win::*;
use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

const STATUS_END_OF_FILE: NTSTATUS = 0xc000_0011u32 as i32;
const STATUS_OBJECT_PATH_NOT_FOUND: NTSTATUS = 0xc000_003au32 as i32;
const STATUS_NO_MORE_FILES: NTSTATUS = 0x8000_0006u32 as i32;
const FILE_USE_FILE_POINTER_POSITION: i64 = -2;

#[repr(C)]
pub struct IoStatusBlock {
    status: usize,
    information: usize,
}

#[repr(C)]
pub struct ObjectAttributes {
    length: u32,
    root: HANDLE,
    name: *const UNICODE_STRING,
    attributes: u32,
}

static NAMED_PIPES: Mutex<Option<HashMap<String, i32>>> = Mutex::new(None);
static IOCP_ASSOC: Mutex<Option<HashMap<HANDLE, (HANDLE, usize)>>> = Mutex::new(None);

fn dos_to_status(e: DWORD) -> NTSTATUS {
    crate::shim::ntdll::dos_to_nt(e) as NTSTATUS
}

unsafe fn finish(iosb: *mut IoStatusBlock, status: NTSTATUS, info: usize) -> NTSTATUS {
    if !iosb.is_null() {
        (*iosb).status = status as u32 as usize;
        (*iosb).information = info;
    }
    status
}

/// Converts the `\??\C:\dir\file` style name in OBJECT_ATTRIBUTES to a Windows path.
unsafe fn object_path(oa: *const ObjectAttributes) -> Option<String> {
    if oa.is_null() || (*oa).name.is_null() {
        return None;
    }
    let us = &*(*oa).name;
    let name = w_to_string(std::slice::from_raw_parts(us.buffer, us.length as usize / 2));
    let name = name.strip_prefix("\\??\\").map(str::to_string).unwrap_or(name);
    if (*oa).root != 0 {
        if let Some(Kind::File(f)) = handles::get((*oa).root).as_deref() {
            if let Some(p) = &f.path {
                return Some(format!("{}\\{}", paths::host_to_win(p).trim_end_matches('\\'), name.trim_start_matches('\\')));
            }
        }
    }
    Some(name)
}

pub fn pipe_name(win: &str) -> Option<String> {
    let l = win.replace('/', "\\");
    let lower = l.to_ascii_lowercase();
    for pre in ["\\\\.\\pipe\\", "\\\\?\\pipe\\", "\\??\\pipe\\", "\\device\\namedpipe\\"] {
        if lower.starts_with(pre) {
            return Some(l[pre.len()..].to_string());
        }
    }
    None
}

/// Returns the client end of a named pipe created earlier in this process.
pub fn take_pipe_client(name: &str) -> Option<i32> {
    NAMED_PIPES.lock().unwrap().as_mut().and_then(|m| m.remove(name))
}

unsafe fn create_pipe_server(name: &str) -> Option<HANDLE> {
    let mut fds = [0i32; 2];
    if libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) != 0 {
        return None;
    }
    NAMED_PIPES.lock().unwrap().get_or_insert_with(HashMap::new).insert(name.to_string(), fds[1]);
    Some(handles::insert(Kind::File(FileObj::new(fds[0], true))))
}

unsafe fn nt_open(h: *mut HANDLE, access: u32, oa: *const ObjectAttributes, iosb: *mut IoStatusBlock, disposition: u32, options: u32) -> NTSTATUS {
    let Some(path) = object_path(oa) else { return finish(iosb, STATUS_INVALID_PARAMETER, 0) };
    if let Some(name) = pipe_name(&path) {
        return match take_pipe_client(&name) {
            Some(fd) => {
                *h = handles::insert(Kind::File(FileObj::new(fd, true)));
                finish(iosb, STATUS_SUCCESS, 1)
            }
            None => finish(iosb, STATUS_OBJECT_NAME_NOT_FOUND, 0),
        };
    }
    let (win_disp, created_info) = match disposition {
        0 => (CREATE_ALWAYS, 2),
        1 => (OPEN_EXISTING, 1),
        2 => (CREATE_NEW, 2),
        3 => (OPEN_ALWAYS, 1),
        4 => (TRUNCATE_EXISTING, 3),
        5 => (CREATE_ALWAYS, 2),
        _ => return finish(iosb, STATUS_INVALID_PARAMETER, 0),
    };
    let host = paths::resolve(&path);
    if options & 1 != 0 && !host.exists() && matches!(disposition, 2 | 3) {
        if std::fs::create_dir(&host).is_err() {
            return finish(iosb, STATUS_OBJECT_PATH_NOT_FOUND, 0);
        }
    }
    let mut flags = 0;
    if options & 0x1000 != 0 {
        flags |= FILE_FLAG_DELETE_ON_CLOSE;
    }
    let handle = k32_file::open_file(&path, access, win_disp, flags);
    if handle == INVALID_HANDLE_VALUE {
        return finish(iosb, dos_to_status(last_error()), 0);
    }
    *h = handle;
    finish(iosb, STATUS_SUCCESS, if last_error() == ERROR_ALREADY_EXISTS { 1 } else { created_info })
}

unsafe fn nt_io(read: bool, h: HANDLE, iosb: *mut IoStatusBlock, buf: *mut u8, len: u32, offset: *const i64) -> NTSTATUS {
    let mut ov = OVERLAPPED { internal: 0, internal_high: 0, offset: 0, offset_high: 0, h_event: 0 };
    let ovp = if !offset.is_null() && *offset >= 0 && *offset != FILE_USE_FILE_POINTER_POSITION {
        ov.offset = *offset as u32;
        ov.offset_high = (*offset >> 32) as u32;
        &mut ov as *mut OVERLAPPED
    } else {
        std::ptr::null_mut()
    };
    let mut done: DWORD = 0;
    let ok = if read { k32_file::read_file(h, buf, len, &mut done, ovp) } else { k32_file::write_file(h, buf, len, &mut done, ovp) };
    if ok != FALSE {
        if read && done == 0 && len > 0 {
            return finish(iosb, STATUS_END_OF_FILE, 0);
        }
        return finish(iosb, STATUS_SUCCESS, done as usize);
    }
    let e = last_error();
    if read && e == ERROR_HANDLE_EOF {
        return finish(iosb, STATUS_END_OF_FILE, 0);
    }
    finish(iosb, dos_to_status(e), 0)
}

/// The completion port and key a handle (a file or a socket) is associated with.
pub fn iocp_assoc(h: HANDLE) -> Option<(HANDLE, usize)> {
    IOCP_ASSOC.lock().unwrap().as_ref().and_then(|m| m.get(&h).copied())
}

pub fn iocp_disassociate(h: HANDLE) {
    if let Some(m) = IOCP_ASSOC.lock().unwrap().as_mut() {
        m.remove(&h);
    }
}

/// Queues a completion packet on a port.
pub fn post_to(port: HANDLE, bytes: DWORD, key: usize, ov: usize, status: u32) -> bool {
    match iocp_of(port).as_deref() {
        Some(Kind::Iocp(p)) => {
            p.queue.lock().unwrap().push_back(Completion { bytes, key, overlapped: ov, status });
            p.cv.notify_one();
            true
        }
        _ => false,
    }
}

fn iocp_of(h: HANDLE) -> Option<std::sync::Arc<Kind>> {
    handles::get(h).filter(|k| matches!(&**k, Kind::Iocp(_)))
}

unsafe fn dequeue(port: HANDLE, ms: DWORD) -> Option<Completion> {
    let k = iocp_of(port)?;
    let Kind::Iocp(p) = &*k else { return None };
    let deadline = (ms != INFINITE).then(|| Instant::now() + Duration::from_millis(ms as u64));
    let mut q = p.queue.lock().unwrap();
    loop {
        if let Some(c) = q.pop_front() {
            return Some(c);
        }
        q = match deadline {
            Some(d) => {
                let rem = d.saturating_duration_since(Instant::now());
                if rem.is_zero() {
                    return None;
                }
                p.cv.wait_timeout(q, rem).unwrap().0
            }
            None => p.cv.wait(q).unwrap(),
        };
    }
}

crate::shims! { "ntdll" => NTFILE_SHIMS;
    fn NtCreateFile(h: *mut HANDLE, access: u32, oa: *const ObjectAttributes, iosb: *mut IoStatusBlock, _alloc: *const i64, _attrs: u32, _share: u32, disposition: u32, options: u32, _ea: usize, _ea_len: u32) -> NTSTATUS {
        nt_open(h, access, oa, iosb, disposition, options)
    }
    fn NtOpenFile(h: *mut HANDLE, access: u32, oa: *const ObjectAttributes, iosb: *mut IoStatusBlock, _share: u32, options: u32) -> NTSTATUS {
        nt_open(h, access, oa, iosb, 1, options)
    }
    fn NtReadFile(h: HANDLE, _event: HANDLE, _apc: usize, _ctx: usize, iosb: *mut IoStatusBlock, buf: *mut u8, len: u32, offset: *const i64, _key: *const u32) -> NTSTATUS {
        nt_io(true, h, iosb, buf, len, offset)
    }
    fn NtWriteFile(h: HANDLE, _event: HANDLE, _apc: usize, _ctx: usize, iosb: *mut IoStatusBlock, buf: *const u8, len: u32, offset: *const i64, _key: *const u32) -> NTSTATUS {
        nt_io(false, h, iosb, buf as *mut u8, len, offset)
    }
    fn NtCreateNamedPipeFile(h: *mut HANDLE, _access: u32, oa: *const ObjectAttributes, iosb: *mut IoStatusBlock, _share: u32, _disp: u32, _opts: u32, _ty: u32, _rd: u32, _comp: u32, _max: u32, _inq: u32, _outq: u32, _timeout: *const i64) -> NTSTATUS {
        let Some(path) = object_path(oa) else { return finish(iosb, STATUS_INVALID_PARAMETER, 0) };
        let name = pipe_name(&path).unwrap_or(path);
        match create_pipe_server(&name) {
            Some(handle) => {
                *h = handle;
                finish(iosb, STATUS_SUCCESS, 1)
            }
            None => finish(iosb, STATUS_UNSUCCESSFUL, 0),
        }
    }
    fn NtQueryInformationFile(h: HANDLE, iosb: *mut IoStatusBlock, buf: *mut u8, len: u32, class: u32) -> NTSTATUS {
        query_file_info(h, iosb, buf, len, class)
    }
    fn NtQueryVolumeInformationFile(h: HANDLE, iosb: *mut IoStatusBlock, buf: *mut u8, len: u32, class: u32) -> NTSTATUS {
        query_volume_info(h, iosb, buf, len, class)
    }
    fn NtSetInformationFile(h: HANDLE, iosb: *mut IoStatusBlock, buf: *const u8, len: u32, class: u32) -> NTSTATUS {
        let win_class = match class {
            13 => 4,
            64 => 21,
            10 => 3,
            65 => 22,
            20 => 6,
            19 => 5,
            4 => 0,
            _ => {
                if crate::runtime::TRACE_ERR.load(std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("nle[err]: NtSetInformationFile class {class} is not supported");
                }
                return finish(iosb, STATUS_NOT_SUPPORTED, 0);
            }
        };
        if class == 14 {
            return finish(iosb, STATUS_SUCCESS, 0);
        }
        if k32_file::SetFileInformationByHandle(h, win_class, buf, len) != FALSE {
            finish(iosb, STATUS_SUCCESS, 0)
        } else {
            finish(iosb, dos_to_status(last_error()), 0)
        }
    }
    fn NtDeviceIoControlFile(_h: HANDLE, _e: HANDLE, _a: usize, _c: usize, iosb: *mut IoStatusBlock, _code: u32, _in: usize, _inl: u32, _out: usize, _outl: u32) -> NTSTATUS {
        finish(iosb, STATUS_NOT_SUPPORTED, 0)
    }
    fn NtFsControlFile(_h: HANDLE, _e: HANDLE, _a: usize, _c: usize, iosb: *mut IoStatusBlock, _code: u32, _in: usize, _inl: u32, _out: usize, _outl: u32) -> NTSTATUS {
        finish(iosb, STATUS_NOT_SUPPORTED, 0)
    }
    fn NtQueryDirectoryFile(h: HANDLE, _e: HANDLE, _a: usize, _c: usize, iosb: *mut IoStatusBlock, buf: *mut u8, len: u32, class: u32, single: u8, name: *const UNICODE_STRING, restart: u8) -> NTSTATUS {
        query_directory(h, iosb, buf, len, class, single != 0, name, restart != 0)
    }
    fn NtFlushBuffersFile(h: HANDLE, iosb: *mut IoStatusBlock) -> NTSTATUS {
        k32_file::FlushFileBuffers(h);
        finish(iosb, STATUS_SUCCESS, 0)
    }
}

/// `NtQueryVolumeInformationFile`: the volume of a handle, answered from `fstatvfs` (classes 1, 3, 4, 5 and 7).
unsafe fn query_volume_info(h: HANDLE, iosb: *mut IoStatusBlock, buf: *mut u8, len: u32, class: u32) -> NTSTATUS {
    let Some((fd, tty, pipe)) = k32_file::with_file(h, |f| (f.fd.load(std::sync::atomic::Ordering::Relaxed), f.is_tty, f.is_pipe)) else {
        return finish(iosb, STATUS_INVALID_HANDLE, 0);
    };
    let mut vfs: libc::statvfs = std::mem::zeroed();
    let have = fd >= 0 && libc::fstatvfs(fd, &mut vfs) == 0;
    let unit = if have { vfs.f_frsize.max(512) as u32 } else { 4096 };
    let (total, free, avail) = if have { (vfs.f_blocks as i64, vfs.f_bfree as i64, vfs.f_bavail as i64) } else { (0, 0, 0) };
    let put = |bytes: &[u8]| -> NTSTATUS {
        if (len as usize) < bytes.len() {
            return finish(iosb, STATUS_INFO_LENGTH_MISMATCH, 0);
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        finish(iosb, STATUS_SUCCESS, bytes.len())
    };
    match class {
        // volume: creation time, serial number, label length, supports objects
        1 => {
            let mut b = vec![0u8; 20];
            b[8..12].copy_from_slice(&0x4e4c_4501u32.to_le_bytes());
            put(&b)
        }
        // size: total and available units, sectors per unit, bytes per sector
        3 => {
            let mut b = Vec::new();
            b.extend_from_slice(&total.to_le_bytes());
            b.extend_from_slice(&avail.to_le_bytes());
            b.extend_from_slice(&(unit / 512).to_le_bytes());
            b.extend_from_slice(&512u32.to_le_bytes());
            put(&b)
        }
        // device: FILE_DEVICE_DISK / NAMED_PIPE / CONSOLE and the characteristics
        4 => {
            let ty: u32 = if tty { 0x50 } else if pipe { 0x11 } else { 7 };
            let mut b = Vec::new();
            b.extend_from_slice(&ty.to_le_bytes());
            b.extend_from_slice(&0u32.to_le_bytes());
            put(&b)
        }
        // attributes: case preserving + unicode on disk, 255 characters per name, "NTFS"
        5 => {
            let name = to_wide("NTFS");
            let mut b = Vec::new();
            b.extend_from_slice(&0x0000_0007u32.to_le_bytes());
            b.extend_from_slice(&255i32.to_le_bytes());
            b.extend_from_slice(&((name.len() * 2) as u32).to_le_bytes());
            for u in name {
                b.extend_from_slice(&u.to_le_bytes());
            }
            put(&b)
        }
        7 => {
            let mut b = Vec::new();
            b.extend_from_slice(&total.to_le_bytes());
            b.extend_from_slice(&avail.to_le_bytes());
            b.extend_from_slice(&free.to_le_bytes());
            b.extend_from_slice(&(unit / 512).to_le_bytes());
            b.extend_from_slice(&512u32.to_le_bytes());
            put(&b)
        }
        _ => finish(iosb, STATUS_NOT_SUPPORTED, 0),
    }
}

/// The entries of a directory handle that `NtQueryDirectoryFile` has not handed out yet.
struct DirScan {
    entries: Vec<(String, std::path::PathBuf)>,
    next: usize,
}

static DIR_SCANS: Mutex<Option<HashMap<HANDLE, DirScan>>> = Mutex::new(None);

/// `NtQueryDirectoryFile`: the directory information classes 1, 2, 3, 12, 37 and 38, a pattern on the first call,
/// "." and ".." included as Windows lists them.
unsafe fn query_directory(h: HANDLE, iosb: *mut IoStatusBlock, buf: *mut u8, len: u32, class: u32, single: bool, name: *const UNICODE_STRING, restart: bool) -> NTSTATUS {
    use std::os::unix::fs::MetadataExt;
    // (offset of the name, fixed header before it) per class; EaSize at 64 in the classes that have one
    let (name_off, id_off) = match class {
        1 => (64usize, None),
        2 => (68, None),
        3 => (94, None),
        12 => (12, None),
        37 => (104, Some(96usize)),
        38 => (80, Some(72)),
        _ => return finish(iosb, STATUS_NOT_SUPPORTED, 0),
    };
    let Some(dir) = k32_file::with_file(h, |f| f.path.clone()) else { return finish(iosb, STATUS_INVALID_HANDLE, 0) };
    let Some(dir) = dir else { return finish(iosb, STATUS_INVALID_HANDLE, 0) };
    let mut scans = DIR_SCANS.lock().unwrap();
    let scans = scans.get_or_insert_with(HashMap::new);
    if restart || !scans.contains_key(&h) {
        let pattern: Vec<char> = if name.is_null() || (*name).length == 0 {
            vec!['*']
        } else {
            String::from_utf16_lossy(std::slice::from_raw_parts((*name).buffer, (*name).length as usize / 2)).chars().collect()
        };
        let mut entries: Vec<(String, std::path::PathBuf)> = vec![(".".into(), dir.clone()), ("..".into(), dir.join(".."))];
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut names: Vec<_> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            entries.extend(names.into_iter().map(|n| { let p = dir.join(&n); (n, p) }));
        }
        entries.retain(|(n, _)| k32_file::wildcard(&pattern, &n.chars().collect::<Vec<char>>()));
        scans.insert(h, DirScan { entries, next: 0 });
    }
    let scan = scans.get_mut(&h).unwrap();
    if scan.next >= scan.entries.len() {
        // the first call that finds nothing says there is no such file, later ones that there are no more
        scans.remove(&h);
        return finish(iosb, STATUS_NO_MORE_FILES, 0);
    }
    let mut at = 0usize;
    let mut records: Vec<(usize, usize)> = Vec::new();
    while scan.next < scan.entries.len() {
        let (n, p) = &scan.entries[scan.next];
        let wide = to_wide(n);
        let size = (name_off + wide.len() * 2 + 7) & !7;
        if at + size > len as usize {
            break;
        }
        let md = std::fs::metadata(p).or_else(|_| std::fs::symlink_metadata(p));
        let rec = std::slice::from_raw_parts_mut(buf.add(at), size);
        rec.fill(0);
        let put32 = |rec: &mut [u8], off: usize, v: u32| rec[off..off + 4].copy_from_slice(&v.to_le_bytes());
        let put64 = |rec: &mut [u8], off: usize, v: u64| rec[off..off + 8].copy_from_slice(&v.to_le_bytes());
        if class != 12 {
            if let Ok(md) = &md {
                let (c, a, w) = k32_file::md_times(md);
                put64(rec, 8, c.to_u64());
                put64(rec, 16, a.to_u64());
                put64(rec, 24, w.to_u64());
                put64(rec, 32, w.to_u64());
                put64(rec, 40, md.len());
                put64(rec, 48, (md.len() + 4095) & !4095);
                put32(rec, 56, k32_file::md_attrs(md));
                if let Some(off) = id_off {
                    put64(rec, off, md.ino());
                }
            }
            put32(rec, 60, (wide.len() * 2) as u32);
        } else {
            put32(rec, 8, (wide.len() * 2) as u32);
        }
        for (i, u) in wide.iter().enumerate() {
            rec[name_off + i * 2..name_off + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        records.push((at, size));
        at += size;
        scan.next += 1;
        if single {
            break;
        }
    }
    if records.is_empty() {
        return finish(iosb, STATUS_BUFFER_OVERFLOW, 0);
    }
    // each record says how far the next one is; the last one says 0
    for (at, size) in &records[..records.len() - 1] {
        std::ptr::copy_nonoverlapping((*size as u32).to_le_bytes().as_ptr(), buf.add(*at), 4);
    }
    finish(iosb, STATUS_SUCCESS, at)
}

unsafe fn query_file_info(h: HANDLE, iosb: *mut IoStatusBlock, buf: *mut u8, len: u32, class: u32) -> NTSTATUS {
    let map = match class {
        4 => 0,
        5 => 1,
        35 => 9,
        6 | 8 | 9 | 14 | 16 | 18 | 24 => return query_file_info_composite(h, iosb, buf, len, class),
        _ => {
            if crate::runtime::TRACE_ERR.load(std::sync::atomic::Ordering::Relaxed) {
                eprintln!("nle[err]: NtQueryInformationFile class {class} is not supported");
            }
            return finish(iosb, STATUS_NOT_SUPPORTED, 0);
        }
    };
    if k32_file::GetFileInformationByHandleEx(h, map, buf, len) != FALSE {
        let n = match class {
            4 => 40,
            5 => 24,
            _ => 8,
        };
        finish(iosb, STATUS_SUCCESS, n)
    } else {
        finish(iosb, dos_to_status(last_error()), 0)
    }
}

const STATUS_BUFFER_OVERFLOW: NTSTATUS = 0x8000_0005u32 as i32;

/// What the file information classes that are built from several parts say: internal (6), access (8), name (9),
/// position (14), mode (16) and all of them at once (18).
unsafe fn query_file_info_composite(h: HANDLE, iosb: *mut IoStatusBlock, buf: *mut u8, len: u32, class: u32) -> NTSTATUS {
    let Some((fd, path, pipe)) = k32_file::with_file(h, |f| (f.fd.load(std::sync::atomic::Ordering::Relaxed), f.path.clone(), f.is_pipe)) else {
        return finish(iosb, STATUS_INVALID_HANDLE, 0);
    };
    let mut st: libc::stat = std::mem::zeroed();
    let have_stat = fd >= 0 && libc::fstat(fd, &mut st) == 0;
    let index = if have_stat { st.st_ino as u64 } else { 0 };
    // FILE_READ_DATA / FILE_WRITE_DATA with the usual companions, from how the descriptor was opened
    let flags = if fd >= 0 { libc::fcntl(fd, libc::F_GETFL) } else { 0 };
    let access: u32 = match flags & libc::O_ACCMODE {
        libc::O_WRONLY => 0x0012_0116,
        libc::O_RDWR => 0x0012_019f,
        _ => 0x0012_0089,
    };
    // the descriptors NLE hands out are synchronous: FILE_SYNCHRONOUS_IO_NONALERT
    let mode: u32 = if pipe || have_stat { 0x20 } else { 0 };
    let position = if fd >= 0 { libc::lseek(fd, 0, libc::SEEK_CUR).max(0) as i64 } else { 0 };
    let name: Vec<u16> = path.as_deref().map(|p| paths::host_to_win(p)).map(|w| to_wide(w.get(2..).unwrap_or(&w))).unwrap_or_default();
    let mut out: Vec<u8> = Vec::new();
    let name_info = |out: &mut Vec<u8>| {
        out.extend_from_slice(&((name.len() * 2) as u32).to_le_bytes());
        for u in &name {
            out.extend_from_slice(&u.to_le_bytes());
        }
    };
    match class {
        6 => out.extend_from_slice(&index.to_le_bytes()),
        8 => out.extend_from_slice(&access.to_le_bytes()),
        9 => name_info(&mut out),
        14 => out.extend_from_slice(&position.to_le_bytes()),
        16 => out.extend_from_slice(&mode.to_le_bytes()),
        // FilePipeLocalInformation: byte stream, full duplex, quotas, what can be read, connected, client end
        24 => {
            let mut unread: libc::c_int = 0;
            if fd >= 0 {
                libc::ioctl(fd, libc::FIONREAD, &mut unread);
            }
            for v in [0u32, 2, 255, 1, 65536, unread.max(0) as u32, 65536, 65536, 3, 0] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        _ => {
            // basic and standard information first, as the single classes give them
            let mut part = [0u8; 64];
            if k32_file::GetFileInformationByHandleEx(h, 0, part.as_mut_ptr(), 40) == FALSE {
                return finish(iosb, dos_to_status(last_error()), 0);
            }
            if k32_file::GetFileInformationByHandleEx(h, 1, part.as_mut_ptr().add(40), 24) == FALSE {
                return finish(iosb, dos_to_status(last_error()), 0);
            }
            out.extend_from_slice(&part);
            out.extend_from_slice(&index.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // EaSize
            out.extend_from_slice(&access.to_le_bytes());
            out.extend_from_slice(&position.to_le_bytes());
            out.extend_from_slice(&mode.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes()); // AlignmentRequirement
            name_info(&mut out);
        }
    }
    let n = out.len().min(len as usize);
    std::ptr::copy_nonoverlapping(out.as_ptr(), buf, n);
    // a name that does not fit is cut off and said so; every other class has to fit
    let fixed = match class { 9 => 4, 18 => 100, _ => out.len() };
    if (len as usize) < fixed {
        return finish(iosb, STATUS_INFO_LENGTH_MISMATCH, 0);
    }
    finish(iosb, if n < out.len() { STATUS_BUFFER_OVERFLOW } else { STATUS_SUCCESS }, n)
}

crate::shims! { "kernel32" => IOCP_SHIMS;
    fn CreateIoCompletionPort(file: HANDLE, existing: HANDLE, key: usize, _threads: DWORD) -> HANDLE {
        let port = if existing != 0 {
            existing
        } else {
            handles::insert(Kind::Iocp(IocpObj { queue: Mutex::new(VecDeque::new()), cv: Condvar::new() }))
        };
        if file != INVALID_HANDLE_VALUE {
            IOCP_ASSOC.lock().unwrap().get_or_insert_with(HashMap::new).insert(file, (port, key));
        }
        port
    }
    fn PostQueuedCompletionStatus(port: HANDLE, bytes: DWORD, key: usize, ov: usize) -> BOOL {
        match iocp_of(port).as_deref() {
            Some(Kind::Iocp(p)) => {
                p.queue.lock().unwrap().push_back(Completion { bytes, key, overlapped: ov, status: 0 });
                p.cv.notify_one();
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }
    fn GetQueuedCompletionStatus(port: HANDLE, bytes: *mut DWORD, key: *mut usize, ov: *mut usize, ms: DWORD) -> BOOL {
        match dequeue(port, ms) {
            Some(c) => {
                *bytes = c.bytes;
                *key = c.key;
                *ov = c.overlapped;
                // an operation that failed is reported as a failed call that still hands over its OVERLAPPED
                if c.status != 0 {
                    set_last_error(crate::shim::ntdll::nt_to_dos(c.status));
                    return FALSE;
                }
                TRUE
            }
            None => {
                *ov = 0;
                set_last_error(if iocp_of(port).is_some() { WAIT_TIMEOUT } else { ERROR_INVALID_HANDLE });
                FALSE
            }
        }
    }
    fn GetQueuedCompletionStatusEx(port: HANDLE, entries: *mut usize, count: ULONG, removed: *mut ULONG, ms: DWORD, _alertable: BOOL) -> BOOL {
        let Some(first) = dequeue(port, ms) else {
            set_last_error(if iocp_of(port).is_some() { WAIT_TIMEOUT } else { ERROR_INVALID_HANDLE });
            return FALSE;
        };
        let mut n = 0usize;
        let put = |c: Completion, n: usize| {
            let e = entries.add(n * 4);
            *e = c.key;
            *e.add(1) = c.overlapped;
            *e.add(2) = c.status as usize;
            *(e.add(3) as *mut u32) = c.bytes;
        };
        put(first, n);
        n += 1;
        while (n as u32) < count {
            match dequeue(port, 0) {
                Some(c) => {
                    put(c, n);
                    n += 1;
                }
                None => break,
            }
        }
        *removed = n as ULONG;
        TRUE
    }
    fn CreateNamedPipeW(name: LPCWSTR, open: DWORD, _mode: DWORD, _max: DWORD, _out: DWORD, _in: DWORD, _timeout: DWORD, _sa: usize) -> HANDLE {
        let path = w_ptr_to_string(name);
        let n = pipe_name(&path).unwrap_or(path);
        match create_pipe_server(&n) {
            Some(h) => {
                if open & FILE_FLAG_OVERLAPPED != 0 {
                    crate::shim::aio::mark_overlapped(h);
                }
                h
            }
            None => {
                set_last_error(ERROR_NOT_ENOUGH_MEMORY);
                INVALID_HANDLE_VALUE
            }
        }
    }
    fn CreateNamedPipeA(name: LPCSTR, open: DWORD, mode: DWORD, max: DWORD, out: DWORD, inb: DWORD, timeout: DWORD, sa: usize) -> HANDLE {
        let w = to_wide_nul(&a_to_string(name));
        CreateNamedPipeW(w.as_ptr(), open, mode, max, out, inb, timeout, sa)
    }
    fn DisconnectNamedPipe(_h: HANDLE) -> BOOL {
        TRUE
    }
    fn WaitNamedPipeW(_name: LPCWSTR, _ms: DWORD) -> BOOL {
        TRUE
    }
    fn WriteFileEx(_h: HANDLE, _buf: *const u8, _n: DWORD, _ov: usize, _cb: usize) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
}
