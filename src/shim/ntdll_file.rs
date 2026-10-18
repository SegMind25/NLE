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
const STATUS_PIPE_BROKEN: NTSTATUS = 0xc000_014bu32 as i32;
const STATUS_OBJECT_NAME_COLLISION: NTSTATUS = 0xc000_0035u32 as i32;
const STATUS_OBJECT_PATH_NOT_FOUND: NTSTATUS = 0xc000_003au32 as i32;
const STATUS_SHARING_VIOLATION: NTSTATUS = 0xc000_0043u32 as i32;
const STATUS_OBJECT_NAME_INVALID: NTSTATUS = 0xc000_0033u32 as i32;
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
    match e {
        ERROR_SUCCESS => STATUS_SUCCESS,
        ERROR_FILE_NOT_FOUND => STATUS_OBJECT_NAME_NOT_FOUND,
        ERROR_PATH_NOT_FOUND => STATUS_OBJECT_PATH_NOT_FOUND,
        ERROR_ACCESS_DENIED => STATUS_ACCESS_DENIED,
        ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS => STATUS_OBJECT_NAME_COLLISION,
        ERROR_SHARING_VIOLATION => STATUS_SHARING_VIOLATION,
        ERROR_INVALID_NAME => STATUS_OBJECT_NAME_INVALID,
        ERROR_INVALID_HANDLE => STATUS_INVALID_HANDLE,
        ERROR_BROKEN_PIPE => STATUS_PIPE_BROKEN,
        ERROR_NOT_ENOUGH_MEMORY => STATUS_NO_MEMORY,
        ERROR_NOT_SUPPORTED => STATUS_NOT_SUPPORTED,
        _ => STATUS_UNSUCCESSFUL,
    }
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
    for pre in ["\\\\.\\pipe\\", "\\??\\pipe\\", "\\device\\namedpipe\\"] {
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
    fn NtSetInformationFile(h: HANDLE, iosb: *mut IoStatusBlock, buf: *const u8, len: u32, class: u32) -> NTSTATUS {
        let win_class = match class {
            13 => 4,
            64 => 21,
            10 => 3,
            65 => 22,
            20 => 6,
            19 => 5,
            4 => 0,
            _ => return finish(iosb, STATUS_NOT_SUPPORTED, 0),
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
    fn NtQueryDirectoryFile(_h: HANDLE, _e: HANDLE, _a: usize, _c: usize, iosb: *mut IoStatusBlock, _buf: usize, _len: u32, _class: u32, _single: u8, _name: usize, _restart: u8) -> NTSTATUS {
        finish(iosb, STATUS_NO_MORE_FILES, 0)
    }
    fn NtFlushBuffersFile(h: HANDLE, iosb: *mut IoStatusBlock) -> NTSTATUS {
        k32_file::FlushFileBuffers(h);
        finish(iosb, STATUS_SUCCESS, 0)
    }
}

unsafe fn query_file_info(h: HANDLE, iosb: *mut IoStatusBlock, buf: *mut u8, len: u32, class: u32) -> NTSTATUS {
    let map = match class {
        4 => 0,
        5 => 1,
        35 => 9,
        _ => return finish(iosb, STATUS_NOT_SUPPORTED, 0),
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
                p.queue.lock().unwrap().push_back(Completion { bytes, key, overlapped: ov });
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
        let mut put = |c: Completion, n: usize| {
            let e = entries.add(n * 4);
            *e = c.key;
            *e.add(1) = c.overlapped;
            *e.add(2) = 0;
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
    fn CreateNamedPipeW(name: LPCWSTR, _open: DWORD, _mode: DWORD, _max: DWORD, _out: DWORD, _in: DWORD, _timeout: DWORD, _sa: usize) -> HANDLE {
        let path = w_ptr_to_string(name);
        let n = pipe_name(&path).unwrap_or(path);
        match create_pipe_server(&n) {
            Some(h) => h,
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
    fn ConnectNamedPipe(_h: HANDLE, _ov: usize) -> BOOL {
        TRUE
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
