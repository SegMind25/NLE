//! Overlapped I/O: operations that return before they finish and report to an I/O completion port (or an event)
//! when they have. Pipes and sockets get this; one thread polls the descriptors of the operations that could not
//! finish at once and completes them, writing the result into the program's `OVERLAPPED` and queueing a packet on
//! the completion port the handle is associated with, as Windows does. Event loops such as libuv's are built on it.
//!
//! The rules mirrored here: an operation that finishes during the call still posts its packet (no handle skips them,
//! `SetFileCompletionNotificationModes` says so), one that fails during the call posts nothing, and closing a handle
//! or `CancelIoEx` completes what is pending with `STATUS_CANCELLED`.

use crate::runtime::strings::*;
use crate::shim::{k32_file, k32_sync, ntdll_file, ws2_32};
use crate::win::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub const STATUS_PENDING: usize = 0x103;
const ERROR_IO_PENDING: DWORD = 997;
const ERROR_IO_INCOMPLETE: DWORD = 996;
const ERROR_PIPE_CONNECTED: DWORD = 535;
const WSA_IO_PENDING: DWORD = 997;
const SOCKET_ERROR: i32 = -1;

// ---- which handles are overlapped --------------------------------------------------------------------------

static OVERLAPPED_FILES: Mutex<Option<HashSet<HANDLE>>> = Mutex::new(None);

/// Records that a file handle was opened with `FILE_FLAG_OVERLAPPED`.
pub fn mark_overlapped(h: HANDLE) {
    OVERLAPPED_FILES.lock().unwrap().get_or_insert_with(HashSet::new).insert(h);
}

pub fn is_overlapped(h: HANDLE) -> bool {
    OVERLAPPED_FILES.lock().unwrap().as_ref().is_some_and(|s| s.contains(&h))
}

/// What closing a handle does to its overlapped state: pending operations end, the completion port forgets it.
pub unsafe fn on_close_handle(h: HANDLE) {
    cancel(h, None);
    if let Some(s) = OVERLAPPED_FILES.lock().unwrap().as_mut() {
        s.remove(&h);
    }
    ntdll_file::iocp_disassociate(h);
}

// ---- operations --------------------------------------------------------------------------------------------

pub enum Work {
    /// Receive into the buffers (pointer, length); datagrams also report where they came from (address, length pointer).
    /// `pipe`: end of input is an error (a broken pipe) rather than a zero-byte success.
    Recv { bufs: Vec<(usize, usize)>, from: Option<(usize, usize)>, pipe: bool },
    /// Send the buffers; datagrams to an address.
    Send { bufs: Vec<(usize, usize)>, to: Option<Vec<u8>> },
    /// `AcceptEx`: the connection replaces the descriptor of the socket prepared for it.
    Accept { sock: i32, buf: usize, recv_len: usize, local_len: usize, remote_len: usize },
}

struct Op {
    id: u64,
    fd: i32,
    /// what the completion port knows the descriptor by: the file handle or the socket
    handle: HANDLE,
    ov: usize,
    work: Work,
    /// bytes of a send that already went out
    done: usize,
}

pub enum Submitted {
    /// finished during the call, with this many bytes (the packet is queued)
    Done(usize),
    /// not finished: the engine completes it later
    Pending,
    /// failed during the call with this errno; nothing is queued
    Failed(i32),
}

struct Engine {
    ops: Mutex<Vec<Op>>,
    wake: i32,
}

static ENGINE: OnceLock<&'static Engine> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        let wake = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        let e: &'static Engine = Box::leak(Box::new(Engine { ops: Mutex::new(Vec::new()), wake }));
        std::thread::Builder::new().name("nle-aio".into()).spawn(move || unsafe { run(e) }).expect("aio thread");
        e
    })
}

unsafe fn nudge(e: &Engine) {
    let one = 1u64;
    libc::write(e.wake, &one as *const u64 as *const libc::c_void, 8);
}

unsafe fn run(e: &'static Engine) {
    loop {
        let wanted: Vec<(u64, i32, i16)> = e.ops.lock().unwrap().iter().map(|o| (o.id, o.fd, if matches!(o.work, Work::Send { .. }) { libc::POLLOUT } else { libc::POLLIN })).collect();
        let mut fds: Vec<libc::pollfd> = vec![libc::pollfd { fd: e.wake, events: libc::POLLIN, revents: 0 }];
        fds.extend(wanted.iter().map(|w| libc::pollfd { fd: w.1, events: w.2, revents: 0 }));
        libc::poll(fds.as_mut_ptr(), fds.len() as u64, -1);
        if fds[0].revents != 0 {
            let mut v = 0u64;
            libc::read(e.wake, &mut v as *mut u64 as *mut libc::c_void, 8);
        }
        for (i, (id, _, _)) in wanted.iter().enumerate() {
            if fds[i + 1].revents == 0 {
                continue;
            }
            let op = {
                let mut ops = e.ops.lock().unwrap();
                match ops.iter().position(|o| o.id == *id) {
                    Some(p) => ops.remove(p),
                    None => continue,
                }
            };
            let mut op = op;
            match attempt(&mut op) {
                Err(libc::EAGAIN) => e.ops.lock().unwrap().push(op),
                r => complete(&op, r),
            }
        }
    }
}

fn is_socket(fd: i32) -> bool {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    unsafe { libc::fstat(fd, &mut st) == 0 && st.st_mode & libc::S_IFMT == libc::S_IFSOCK }
}

fn ready(fd: i32, events: i16) -> bool {
    let mut p = libc::pollfd { fd, events, revents: 0 };
    unsafe { libc::poll(&mut p, 1, 0) > 0 }
}

fn iovecs(bufs: &[(usize, usize)], skip: usize) -> Vec<libc::iovec> {
    let mut skip = skip;
    let mut v = Vec::new();
    for &(p, l) in bufs {
        if skip >= l {
            skip -= l;
            continue;
        }
        v.push(libc::iovec { iov_base: (p + skip) as *mut libc::c_void, iov_len: l - skip });
        skip = 0;
    }
    v
}

/// Tries the operation without blocking: the bytes moved, or the errno (`EAGAIN`: not yet).
unsafe fn attempt(op: &mut Op) -> Result<usize, i32> {
    let fd = op.fd;
    let errno = || *libc::__errno_location();
    match &op.work {
        Work::Recv { bufs, from, pipe } => {
            let total: usize = bufs.iter().map(|b| b.1).sum();
            if total == 0 {
                // a zero-byte read finishes when there is something to read (or the other end is gone)
                return if ready(fd, libc::POLLIN) { Ok(0) } else { Err(libc::EAGAIN) };
            }
            let mut iov = iovecs(bufs, 0);
            let n = if is_socket(fd) {
                let mut name = [0u8; 128];
                let mut msg: libc::msghdr = std::mem::zeroed();
                msg.msg_iov = iov.as_mut_ptr();
                msg.msg_iovlen = iov.len();
                if from.is_some() {
                    msg.msg_name = name.as_mut_ptr() as *mut libc::c_void;
                    msg.msg_namelen = name.len() as u32;
                }
                let n = libc::recvmsg(fd, &mut msg, libc::MSG_DONTWAIT);
                if n >= 0 {
                    if let Some((addr, len)) = from {
                        ws2_32::sockaddr_to_win(&name[..msg.msg_namelen as usize], *addr as *mut u8, *len as *mut i32);
                    }
                }
                n
            } else {
                if !ready(fd, libc::POLLIN) {
                    return Err(libc::EAGAIN);
                }
                libc::readv(fd, iov.as_ptr(), iov.len() as i32)
            };
            if n < 0 {
                return Err(errno());
            }
            if n == 0 && *pipe {
                return Err(libc::EPIPE);
            }
            Ok(n as usize)
        }
        Work::Send { bufs, to } => {
            let total: usize = bufs.iter().map(|b| b.1).sum();
            let dgram = to.is_some() || {
                let mut ty: i32 = 0;
                let mut l = 4u32;
                is_socket(fd) && libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE, &mut ty as *mut i32 as *mut libc::c_void, &mut l) == 0 && ty == libc::SOCK_DGRAM
            };
            loop {
                if op.done >= total && !(total == 0 && op.done == 0 && dgram) {
                    return Ok(op.done);
                }
                let mut iov = iovecs(bufs, op.done);
                let n = if is_socket(fd) {
                    let mut msg: libc::msghdr = std::mem::zeroed();
                    msg.msg_iov = iov.as_mut_ptr();
                    msg.msg_iovlen = iov.len();
                    let host = to.as_ref().map(|a| a.as_ptr());
                    if let (Some(p), Some(a)) = (host, to) {
                        msg.msg_name = p as *mut libc::c_void;
                        msg.msg_namelen = a.len() as u32;
                    }
                    libc::sendmsg(fd, &msg, libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL)
                } else {
                    if !ready(fd, libc::POLLOUT) {
                        return Err(libc::EAGAIN);
                    }
                    libc::writev(fd, iov.as_ptr(), iov.len() as i32)
                };
                if n < 0 {
                    return Err(errno());
                }
                op.done += n as usize;
                if dgram {
                    return Ok(op.done);
                }
            }
        }
        Work::Accept { sock, buf, recv_len, local_len, remote_len } => {
            if !ready(fd, libc::POLLIN) {
                return Err(libc::EAGAIN);
            }
            let mut remote = [0u8; 128];
            let mut rl = remote.len() as u32;
            let accepted = libc::accept4(fd, remote.as_mut_ptr() as *mut libc::sockaddr, &mut rl, libc::SOCK_CLOEXEC);
            if accepted < 0 {
                return Err(errno());
            }
            // the connection takes the place of the socket the program made for it
            libc::dup2(accepted, *sock);
            libc::close(accepted);
            let mut local = [0u8; 128];
            let mut ll = local.len() as u32;
            libc::getsockname(*sock, local.as_mut_ptr() as *mut libc::sockaddr, &mut ll);
            let mut l = *local_len as i32;
            ws2_32::sockaddr_to_win(&local[..ll as usize], (*buf + *recv_len) as *mut u8, &mut l);
            let mut r = *remote_len as i32;
            ws2_32::sockaddr_to_win(&remote[..rl as usize], (*buf + *recv_len + *local_len) as *mut u8, &mut r);
            Ok(0)
        }
    }
}

fn status_for_errno(e: i32) -> usize {
    (match e {
        libc::ECANCELED => 0xC000_0120u32,
        libc::EPIPE => 0xC000_014B,
        libc::ECONNRESET => 0xC000_020D,
        libc::ECONNABORTED => 0xC000_0241,
        libc::ECONNREFUSED => 0xC000_0236,
        libc::ETIMEDOUT => 0xC000_00B5,
        libc::EHOSTUNREACH => 0xC000_023D,
        libc::ENETUNREACH => 0xC000_023C,
        libc::ENOTCONN => 0xC000_0128,
        libc::EBADF => 0xC000_0008,
        libc::EADDRINUSE => 0xC000_0020,
        _ => 0xC000_0001,
    }) as usize
}

/// Writes the outcome into the program's `OVERLAPPED`, signals its event and queues the packet.
unsafe fn complete(op: &Op, res: Result<usize, i32>) {
    let ov = op.ov as *mut OVERLAPPED;
    let (status, bytes) = match res {
        Ok(n) => (0usize, n),
        Err(e) => (status_for_errno(e), 0),
    };
    (*ov).internal_high = bytes;
    (*ov).internal = status;
    signal_and_post(op.handle, ov, bytes);
}

/// The event of an `OVERLAPPED` is set, and a packet is queued on the handle's completion port unless the low bit of
/// the event says not to.
pub unsafe fn signal_and_post(handle: HANDLE, ov: *mut OVERLAPPED, bytes: usize) {
    let ev = (*ov).h_event;
    if ev & !1 != 0 {
        k32_sync::SetEvent(ev & !1);
    }
    if ev & 1 == 0 {
        if let Some((port, key)) = ntdll_file::iocp_assoc(handle) {
            ntdll_file::post_to(port, bytes as u32, key, ov as usize, (*ov).internal as u32);
        }
    }
}

/// Starts an operation: it finishes now, goes pending, or fails.
pub unsafe fn submit(handle: HANDLE, fd: i32, ov: *mut OVERLAPPED, work: Work) -> Submitted {
    (*ov).internal = STATUS_PENDING;
    (*ov).internal_high = 0;
    let mut op = Op { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), fd, handle, ov: ov as usize, work, done: 0 };
    match attempt(&mut op) {
        Ok(n) => {
            complete(&op, Ok(n));
            Submitted::Done(n)
        }
        Err(libc::EAGAIN) => {
            let e = engine();
            e.ops.lock().unwrap().push(op);
            nudge(e);
            Submitted::Pending
        }
        Err(e) => {
            (*ov).internal = status_for_errno(e);
            Submitted::Failed(e)
        }
    }
}

/// Ends the pending operations of a handle (or one of them): each completes as cancelled. Returns how many there were.
pub unsafe fn cancel(handle: HANDLE, ov: Option<usize>) -> usize {
    let Some(e) = ENGINE.get().copied() else { return 0 };
    let removed: Vec<Op> = {
        let mut ops = e.ops.lock().unwrap();
        let (gone, keep): (Vec<Op>, Vec<Op>) = std::mem::take(&mut *ops).into_iter().partition(|o| o.handle == handle && ov.is_none_or(|v| v == o.ov));
        *ops = keep;
        gone
    };
    let n = removed.len();
    for op in removed {
        complete(&op, Err(libc::ECANCELED));
    }
    n
}

/// A connection attempt runs on a thread of its own (it may block) and completes like any other operation.
pub unsafe fn connect(handle: HANDLE, fd: i32, ov: *mut OVERLAPPED, addr: Vec<u8>, send: (usize, usize)) {
    (*ov).internal = STATUS_PENDING;
    (*ov).internal_high = 0;
    let ov_addr = ov as usize;
    std::thread::spawn(move || unsafe {
        let mut r = libc::connect(fd, addr.as_ptr() as *const libc::sockaddr, addr.len() as u32);
        if r < 0 && *libc::__errno_location() == libc::EINPROGRESS {
            let mut p = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
            libc::poll(&mut p, 1, -1);
            let mut err: i32 = 0;
            let mut l = 4u32;
            libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ERROR, &mut err as *mut i32 as *mut libc::c_void, &mut l);
            if err != 0 {
                *libc::__errno_location() = err;
            } else {
                r = 0;
            }
        }
        let res = if r < 0 {
            Err(*libc::__errno_location())
        } else if send.1 > 0 {
            let n = libc::send(fd, send.0 as *const libc::c_void, send.1, libc::MSG_NOSIGNAL);
            if n < 0 { Err(*libc::__errno_location()) } else { Ok(n as usize) }
        } else {
            Ok(0)
        };
        let op = Op { id: 0, fd, handle, ov: ov_addr, work: Work::Send { bufs: Vec::new(), to: None }, done: 0 };
        complete(&op, res);
    });
}

// ---- results -----------------------------------------------------------------------------------------------

/// `GetOverlappedResult`: the bytes moved, or the error the operation ended with; waits for a pending one when asked.
pub unsafe fn get_result(ov: *const OVERLAPPED, bytes: *mut DWORD, wait: bool) -> BOOL {
    if ov.is_null() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    loop {
        let status = std::ptr::read_volatile(&(*ov).internal);
        if status == STATUS_PENDING {
            if !wait {
                set_last_error(ERROR_IO_INCOMPLETE);
                return FALSE;
            }
            let ev = (*ov).h_event & !1;
            if ev != 0 {
                k32_sync::WaitForSingleObject(ev, 20);
            } else {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            continue;
        }
        if !bytes.is_null() {
            *bytes = (*ov).internal_high as DWORD;
        }
        if status == 0 {
            return TRUE;
        }
        set_last_error(crate::shim::ntdll::nt_to_dos(status as u32));
        return FALSE;
    }
}

/// The Winsock number of the Win32 error an operation ended with.
fn wsa_of_dos(e: DWORD) -> DWORD {
    match e {
        64 => 10054,
        1236 => 10053,
        1225 => 10061,
        121 => 10060,
        1231 => 10051,
        1232 => 10065,
        e => e,
    }
}

// ---- files ---------------------------------------------------------------------------------------------------

/// `ReadFile` / `WriteFile` with an `OVERLAPPED` on an overlapped pipe: `None` when the handle is not one (the
/// caller does the plain operation).
pub unsafe fn file_io(write: bool, h: HANDLE, buf: usize, n: DWORD, transferred: *mut DWORD, ov: *mut OVERLAPPED) -> Option<BOOL> {
    if ov.is_null() || !is_overlapped(h) {
        return None;
    }
    let (fd, pipe, regular) = k32_file::with_file(h, |f| {
        let mut st: libc::stat = std::mem::zeroed();
        (f.fd(), f.is_pipe, libc::fstat(f.fd(), &mut st) == 0 && st.st_mode & libc::S_IFMT == libc::S_IFREG)
    })?;
    if regular {
        // files answer at once (the plain path does the I/O at the given offset); the caller posts the packet
        return None;
    }
    let bufs = vec![(buf, n as usize)];
    let work = if write { Work::Send { bufs, to: None } } else { Work::Recv { bufs, from: None, pipe: pipe || !is_socket(fd) } };
    match submit(h, fd, ov, work) {
        Submitted::Done(done) => {
            if !transferred.is_null() {
                *transferred = done as DWORD;
            }
            Some(TRUE)
        }
        Submitted::Pending => {
            set_last_error(ERROR_IO_PENDING);
            Some(FALSE)
        }
        Submitted::Failed(e) => {
            set_last_error(if e == libc::EPIPE && write { 232 } else { errno_to_win(e) });
            Some(FALSE)
        }
    }
}

// ---- sockets -------------------------------------------------------------------------------------------------

unsafe fn wsabufs(bufs: *const u8, count: u32) -> Vec<(usize, usize)> {
    (0..count as usize).map(|i| (*(bufs.add(i * 16 + 8) as *const usize), *(bufs.add(i * 16) as *const u32) as usize)).collect()
}

unsafe fn socket_result(r: Submitted, counter: *mut u32) -> i32 {
    match r {
        Submitted::Done(n) => {
            if !counter.is_null() {
                *counter = n as u32;
            }
            0
        }
        Submitted::Pending => {
            set_last_error(WSA_IO_PENDING);
            SOCKET_ERROR
        }
        Submitted::Failed(e) => {
            set_last_error(ws2_32::wsa_error(e));
            SOCKET_ERROR
        }
    }
}

pub unsafe fn wsa_recv(s: usize, bufs: *const u8, count: u32, received: *mut u32, from: Option<(usize, usize)>, ov: usize) -> i32 {
    let r = submit(s as HANDLE, s as i32, ov as *mut OVERLAPPED, Work::Recv { bufs: wsabufs(bufs, count), from, pipe: false });
    socket_result(r, received)
}

pub unsafe fn wsa_send(s: usize, bufs: *const u8, count: u32, sent: *mut u32, to: Option<Vec<u8>>, ov: usize) -> i32 {
    let r = submit(s as HANDLE, s as i32, ov as *mut OVERLAPPED, Work::Send { bufs: wsabufs(bufs, count), to });
    socket_result(r, sent)
}

// the GUIDs of the extension functions `WSAIoctl(SIO_GET_EXTENSION_FUNCTION_POINTER)` hands out
const GUID_ACCEPTEX: [u8; 16] = [0xf1, 0x7d, 0x36, 0xb5, 0xac, 0xcb, 0xcf, 0x11, 0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92];
const GUID_GETACCEPTEXSOCKADDRS: [u8; 16] = [0xf2, 0x7d, 0x36, 0xb5, 0xac, 0xcb, 0xcf, 0x11, 0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92];
const GUID_CONNECTEX: [u8; 16] = [0xb9, 0x07, 0xa2, 0x25, 0xf3, 0xdd, 0x60, 0x46, 0x8e, 0xe9, 0x76, 0xe5, 0x8c, 0x74, 0x06, 0x3e];
const GUID_TRANSMITFILE: [u8; 16] = [0xf0, 0x7d, 0x36, 0xb5, 0xac, 0xcb, 0xcf, 0x11, 0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92];
const GUID_DISCONNECTEX: [u8; 16] = [0x11, 0x2e, 0xda, 0x7f, 0x30, 0x86, 0x6f, 0x43, 0xa0, 0x31, 0xf5, 0x36, 0xa6, 0xee, 0xc1, 0x57];

unsafe extern "win64" fn accept_ex(listen: usize, accept: usize, buf: usize, recv_len: u32, local_len: u32, remote_len: u32, bytes: *mut u32, ov: usize) -> BOOL {
    if ov == 0 {
        set_last_error(ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    let work = Work::Accept { sock: accept as i32, buf, recv_len: recv_len as usize, local_len: local_len as usize, remote_len: remote_len as usize };
    match submit(listen as HANDLE, listen as i32, ov as *mut OVERLAPPED, work) {
        Submitted::Done(_) => {
            if !bytes.is_null() {
                *bytes = 0;
            }
            TRUE
        }
        Submitted::Pending => {
            set_last_error(ERROR_IO_PENDING);
            FALSE
        }
        Submitted::Failed(e) => {
            set_last_error(ws2_32::wsa_error(e));
            FALSE
        }
    }
}

unsafe extern "win64" fn get_accept_ex_sockaddrs(buf: usize, recv_len: u32, local_len: u32, _remote_len: u32, local: *mut usize, local_size: *mut i32, remote: *mut usize, remote_size: *mut i32) {
    let size_of = |p: usize| match u16::from_le_bytes([*(p as *const u8), *(p as *const u8).add(1)]) {
        23 => 28,
        _ => 16,
    };
    let l = buf + recv_len as usize;
    let r = l + local_len as usize;
    *local = l;
    *local_size = size_of(l);
    *remote = r;
    *remote_size = size_of(r);
}

unsafe extern "win64" fn connect_ex(s: usize, name: *const u8, name_len: i32, send: usize, send_len: u32, _sent: *mut u32, ov: usize) -> BOOL {
    if ov == 0 {
        set_last_error(ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    let addr = ws2_32::sockaddr_to_host(name, name_len);
    connect(s as HANDLE, s as i32, ov as *mut OVERLAPPED, addr, (send, send_len as usize));
    set_last_error(ERROR_IO_PENDING);
    FALSE
}

/// `TransmitFile`: the bytes of an open file sent down the socket, all before it returns (the packet is queued as for
/// any operation that finished at once).
unsafe extern "win64" fn transmit_file(s: usize, file: HANDLE, count: u32, _per_send: u32, ov: usize, buffers: usize, _flags: u32) -> BOOL {
    // TRANSMIT_FILE_BUFFERS: head pointer and length, tail pointer and length
    let send_all = |p: usize, n: usize| -> bool {
        let mut done = 0;
        while done < n {
            let r = libc::send(s as i32, (p + done) as *const libc::c_void, n - done, libc::MSG_NOSIGNAL);
            if r <= 0 {
                return false;
            }
            done += r as usize;
        }
        true
    };
    let mut total = 0usize;
    let mut ok = true;
    if buffers != 0 {
        let b = buffers as *const usize;
        let (head, head_len) = (*b, *b.add(1) as u32 as usize);
        if head != 0 && head_len > 0 {
            ok = send_all(head, head_len);
            total += head_len;
        }
    }
    if ok && file != 0 {
        let fd = k32_file::with_file(file, |f| f.fd()).unwrap_or(-1);
        let mut left = if count == 0 { usize::MAX } else { count as usize };
        let mut buf = vec![0u8; 65536];
        while ok && left > 0 && fd >= 0 {
            let want = buf.len().min(left);
            let n = libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, want);
            if n <= 0 {
                break;
            }
            ok = send_all(buf.as_ptr() as usize, n as usize);
            total += n as usize;
            left -= n as usize;
        }
    }
    if ok && buffers != 0 {
        let b = buffers as *const usize;
        let (tail, tail_len) = (*b.add(2), *b.add(3) as u32 as usize);
        if tail != 0 && tail_len > 0 {
            ok = send_all(tail, tail_len);
            total += tail_len;
        }
    }
    if !ok {
        set_last_error(ws2_32::wsa_error(*libc::__errno_location()));
        return FALSE;
    }
    if ov != 0 {
        let ov = ov as *mut OVERLAPPED;
        (*ov).internal = 0;
        (*ov).internal_high = total;
        signal_and_post(s as HANDLE, ov, total);
    }
    TRUE
}

unsafe extern "win64" fn disconnect_ex(s: usize, ov: usize, _flags: u32, _reserved: u32) -> BOOL {
    libc::shutdown(s as i32, libc::SHUT_RDWR);
    if ov != 0 {
        let ov = ov as *mut OVERLAPPED;
        (*ov).internal = 0;
        (*ov).internal_high = 0;
        signal_and_post(s as HANDLE, ov, 0);
    }
    TRUE
}

crate::shims! { "ws2_32" => AIO_WS2_SHIMS;
    fn WSARecvFrom(s: usize, bufs: *const u8, count: u32, received: *mut u32, _flags: *mut u32, from: usize, from_len: usize, ov: usize, _cb: usize) -> i32 {
        let from = (from != 0).then_some((from, from_len));
        if ov != 0 {
            return wsa_recv(s, bufs, count, received, from, ov);
        }
        // not overlapped: a plain blocking receive
        let mut iov: Vec<libc::iovec> = wsabufs(bufs, count).into_iter().map(|(p, l)| libc::iovec { iov_base: p as *mut libc::c_void, iov_len: l }).collect();
        let mut name = [0u8; 128];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = iov.as_mut_ptr();
        msg.msg_iovlen = iov.len();
        msg.msg_name = name.as_mut_ptr() as *mut libc::c_void;
        msg.msg_namelen = name.len() as u32;
        let n = libc::recvmsg(s as i32, &mut msg, 0);
        if n < 0 {
            set_last_error(ws2_32::wsa_error(*libc::__errno_location()));
            return SOCKET_ERROR;
        }
        if let Some((a, l)) = from {
            ws2_32::sockaddr_to_win(&name[..msg.msg_namelen as usize], a as *mut u8, l as *mut i32);
        }
        if !received.is_null() {
            *received = n as u32;
        }
        0
    }
    fn WSASendTo(s: usize, bufs: *const u8, count: u32, sent: *mut u32, _flags: u32, to: *const u8, to_len: i32, ov: usize, _cb: usize) -> i32 {
        let addr = (!to.is_null()).then(|| ws2_32::sockaddr_to_host(to, to_len));
        if ov != 0 {
            return wsa_send(s, bufs, count, sent, addr, ov);
        }
        let mut iov: Vec<libc::iovec> = wsabufs(bufs, count).into_iter().map(|(p, l)| libc::iovec { iov_base: p as *mut libc::c_void, iov_len: l }).collect();
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = iov.as_mut_ptr();
        msg.msg_iovlen = iov.len();
        if let Some(a) = &addr {
            msg.msg_name = a.as_ptr() as *mut libc::c_void;
            msg.msg_namelen = a.len() as u32;
        }
        let n = libc::sendmsg(s as i32, &msg, libc::MSG_NOSIGNAL);
        if n < 0 {
            set_last_error(ws2_32::wsa_error(*libc::__errno_location()));
            return SOCKET_ERROR;
        }
        if !sent.is_null() {
            *sent = n as u32;
        }
        0
    }
    fn WSAGetOverlappedResult(_s: usize, ov: *const OVERLAPPED, transferred: *mut u32, wait: BOOL, flags: *mut u32) -> BOOL {
        if !flags.is_null() {
            *flags = 0;
        }
        let r = get_result(ov, transferred, wait != FALSE);
        if r == FALSE {
            let e = last_error();
            set_last_error(wsa_of_dos(e));
        }
        r
    }
    fn WSAIoctl(s: usize, code: u32, inb: usize, inl: u32, outb: usize, outl: u32, ret: *mut u32, _ov: usize, _cb: usize) -> i32 {
        let give = |v: usize| -> i32 {
            if outb == 0 || (outl as usize) < 8 {
                set_last_error(10014);
                return SOCKET_ERROR;
            }
            *(outb as *mut usize) = v;
            if !ret.is_null() {
                *ret = 8;
            }
            0
        };
        match code {
            // SIO_GET_EXTENSION_FUNCTION_POINTER
            0xC800_0006 => {
                if inb == 0 || inl < 16 {
                    set_last_error(10014);
                    return SOCKET_ERROR;
                }
                let guid = std::slice::from_raw_parts(inb as *const u8, 16);
                if guid == GUID_ACCEPTEX {
                    give(accept_ex as *const () as usize)
                } else if guid == GUID_GETACCEPTEXSOCKADDRS {
                    give(get_accept_ex_sockaddrs as *const () as usize)
                } else if guid == GUID_CONNECTEX {
                    give(connect_ex as *const () as usize)
                } else if guid == GUID_TRANSMITFILE {
                    give(transmit_file as *const () as usize)
                } else if guid == GUID_DISCONNECTEX {
                    give(disconnect_ex as *const () as usize)
                } else {
                    set_last_error(10022);
                    SOCKET_ERROR
                }
            }
            // SIO_BASE_HANDLE
            0x4800_0022 => give(s),
            // SIO_KEEPALIVE_VALS: on/off, idle and interval in milliseconds
            0x9800_0004 if inb != 0 && inl >= 12 => {
                let v = std::slice::from_raw_parts(inb as *const u32, 3);
                let on: i32 = (v[0] != 0) as i32;
                libc::setsockopt(s as i32, libc::SOL_SOCKET, libc::SO_KEEPALIVE, &on as *const i32 as *const libc::c_void, 4);
                if on != 0 {
                    let idle = (v[1] / 1000).max(1) as i32;
                    let intvl = (v[2] / 1000).max(1) as i32;
                    libc::setsockopt(s as i32, libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, &idle as *const i32 as *const libc::c_void, 4);
                    libc::setsockopt(s as i32, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, &intvl as *const i32 as *const libc::c_void, 4);
                }
                if !ret.is_null() {
                    *ret = 0;
                }
                0
            }
            // FIONBIO and FIONREAD: the ioctlsocket ones
            0x8004_667E | 0x4004_667F if inb != 0 => ws2_32::ioctlsocket(s, code, inb as *mut u32),
            // everything else (SIO_UDP_CONNRESET, SIO_ADDRESS_LIST_QUERY, ...) is accepted and does nothing
            _ => {
                if !ret.is_null() {
                    *ret = 0;
                }
                0
            }
        }
    }
    // two providers, TCP and UDP over IPv4, both installable-file-system handles: no layered service provider
    fn WSAEnumProtocolsW(_protocols: *const i32, buf: *mut u8, len: *mut u32) -> i32 {
        const ENTRY: usize = 628;
        if buf.is_null() || (*len as usize) < ENTRY * 2 {
            *len = (ENTRY * 2) as u32;
            set_last_error(10055);
            return SOCKET_ERROR;
        }
        std::ptr::write_bytes(buf, 0, ENTRY * 2);
        for (i, (ty, proto, name)) in [(1i32, 6i32, "MSAFD Tcpip [TCP/IP]"), (2, 17, "MSAFD Tcpip [UDP/IP]")].into_iter().enumerate() {
            let e = buf.add(i * ENTRY);
            *(e as *mut u32) = 0x0002_0066; // XP1_IFS_HANDLES | connection oriented / message oriented flags
            *(e.add(40) as *mut u32) = 1001 + i as u32; // catalog entry id
            *(e.add(72) as *mut i32) = 2; // iVersion
            *(e.add(76) as *mut i32) = 2; // AF_INET
            *(e.add(80) as *mut i32) = 16;
            *(e.add(84) as *mut i32) = 16;
            *(e.add(88) as *mut i32) = ty;
            *(e.add(92) as *mut i32) = proto;
            for (j, u) in name.encode_utf16().enumerate() {
                *(e.add(112 + j * 2) as *mut u16) = u;
            }
        }
        2
    }
}

crate::shims! { "kernel32" => AIO_K32_SHIMS;
    fn GetOverlappedResult(_h: HANDLE, ov: *const OVERLAPPED, bytes: *mut DWORD, wait: BOOL) -> BOOL {
        get_result(ov, bytes, wait != FALSE)
    }
    fn GetOverlappedResultEx(_h: HANDLE, ov: *const OVERLAPPED, bytes: *mut DWORD, ms: DWORD, _alertable: BOOL) -> BOOL {
        get_result(ov, bytes, ms != 0)
    }
    fn CancelIo(h: HANDLE) -> BOOL {
        cancel(h, None);
        TRUE
    }
    fn CancelIoEx(h: HANDLE, ov: *const OVERLAPPED) -> BOOL {
        let n = cancel(h, (!ov.is_null()).then_some(ov as usize));
        if n == 0 && !ov.is_null() {
            set_last_error(1168);
            return FALSE;
        }
        TRUE
    }
    // a pipe instance made by NLE is connected from the start: overlapped callers are told so
    fn ConnectNamedPipe(_h: HANDLE, ov: usize) -> BOOL {
        if ov == 0 {
            return TRUE;
        }
        set_last_error(ERROR_PIPE_CONNECTED);
        FALSE
    }
    // completion packets are never skipped: every overlapped operation posts its own
    fn SetFileCompletionNotificationModes(_h: HANDLE, _flags: u8) -> BOOL {
        set_last_error(ERROR_INVALID_PARAMETER);
        FALSE
    }
}
