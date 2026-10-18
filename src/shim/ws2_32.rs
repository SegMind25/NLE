//! ws2_32: Winsock over BSD sockets.
//!
//! Sockets are host file descriptors, so a `SOCKET` is just the fd. The translation work is in the
//! constants and structure layouts that differ between Winsock and Linux (address families, socket
//! options, `fd_set`, `addrinfo`, error codes). Operations are synchronous; overlapped I/O is not
//! implemented.

use crate::runtime::strings::*;
use crate::win::*;
use std::ffi::{c_void, CStr, CString};

type SOCKET = usize;
const INVALID_SOCKET: SOCKET = !0;
const SOCKET_ERROR: i32 = -1;

const WIN_AF_INET6: i32 = 23;
const WIN_SOL_SOCKET: i32 = 0xffff;

mod cc {
    use std::ffi::c_void;
    extern "C" {
        pub fn inet_pton(af: i32, src: *const libc::c_char, dst: *mut c_void) -> i32;
        pub fn inet_ntop(af: i32, src: *const c_void, dst: *mut libc::c_char, size: u32) -> *const libc::c_char;
    }
}

const FIONBIO: u32 = 0x8004_667e;
const FIONREAD: u32 = 0x4004_667f;

fn wsa_error(errno: i32) -> DWORD {
    match errno {
        libc::EINTR => 10004,
        libc::EBADF => 10009,
        libc::EACCES => 10013,
        libc::EFAULT => 10014,
        libc::EINVAL => 10022,
        libc::EMFILE => 10024,
        libc::EWOULDBLOCK => 10035,
        libc::EINPROGRESS => 10035,
        libc::EALREADY => 10037,
        libc::ENOTSOCK => 10038,
        libc::EDESTADDRREQ => 10039,
        libc::EMSGSIZE => 10040,
        libc::EPROTOTYPE => 10041,
        libc::ENOPROTOOPT => 10042,
        libc::EPROTONOSUPPORT => 10043,
        libc::EOPNOTSUPP => 10045,
        libc::EAFNOSUPPORT => 10047,
        libc::EADDRINUSE => 10048,
        libc::EADDRNOTAVAIL => 10049,
        libc::ENETDOWN => 10050,
        libc::ENETUNREACH => 10051,
        libc::ECONNABORTED => 10053,
        libc::ECONNRESET => 10054,
        libc::ENOBUFS => 10055,
        libc::EISCONN => 10056,
        libc::ENOTCONN => 10057,
        libc::ESHUTDOWN => 10058,
        libc::ETIMEDOUT => 10060,
        libc::ECONNREFUSED => 10061,
        libc::EHOSTUNREACH => 10065,
        libc::EPIPE => 10053,
        _ => 10022,
    }
}

fn fail_errno() -> i32 {
    let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    set_last_error(wsa_error(e));
    SOCKET_ERROR
}

fn family_to_host(af: i32) -> i32 {
    if af == WIN_AF_INET6 { libc::AF_INET6 } else { af }
}

fn family_to_win(af: i32) -> i32 {
    if af == libc::AF_INET6 { WIN_AF_INET6 } else { af }
}

/// Converts a Windows `sockaddr` into a host one (only the family differs).
unsafe fn sockaddr_to_host(addr: *const u8, len: i32) -> Vec<u8> {
    let mut v = std::slice::from_raw_parts(addr, len.max(0) as usize).to_vec();
    if v.len() >= 2 {
        let fam = u16::from_le_bytes([v[0], v[1]]) as i32;
        v[0..2].copy_from_slice(&(family_to_host(fam) as u16).to_le_bytes());
    }
    v
}

unsafe fn sockaddr_to_win(src: &[u8], dst: *mut u8, len: *mut i32) {
    if dst.is_null() || len.is_null() {
        return;
    }
    let n = src.len().min(*len as usize);
    std::ptr::copy_nonoverlapping(src.as_ptr(), dst, n);
    if n >= 2 {
        let fam = u16::from_le_bytes([src[0], src[1]]) as i32;
        let w = (family_to_win(fam) as u16).to_le_bytes();
        *dst = w[0];
        *dst.add(1) = w[1];
    }
    *len = src.len() as i32;
}

fn sockopt_to_host(level: i32, name: i32) -> Option<(i32, i32)> {
    if level == WIN_SOL_SOCKET {
        let n = match name {
            0x0004 => libc::SO_REUSEADDR,
            0x0008 => libc::SO_KEEPALIVE,
            0x0020 => libc::SO_BROADCAST,
            0x0080 => libc::SO_LINGER,
            0x0100 => libc::SO_OOBINLINE,
            0x1001 => libc::SO_SNDBUF,
            0x1002 => libc::SO_RCVBUF,
            0x1005 => libc::SO_SNDTIMEO,
            0x1006 => libc::SO_RCVTIMEO,
            0x1007 => libc::SO_ERROR,
            0x1008 => libc::SO_TYPE,
            _ => return None,
        };
        return Some((libc::SOL_SOCKET, n));
    }
    if level == libc::IPPROTO_IPV6 {
        let n = match name {
            27 => libc::IPV6_V6ONLY,
            _ => name,
        };
        return Some((level, n));
    }
    if level == libc::IPPROTO_IP {
        let n = match name {
            4 => libc::IP_TTL,
            _ => name,
        };
        return Some((level, n));
    }
    Some((level, name))
}

unsafe fn fd_set_to_vec(set: *const u8) -> Vec<i32> {
    if set.is_null() {
        return Vec::new();
    }
    let count = *(set as *const u32) as usize;
    (0..count.min(64)).map(|i| *(set.add(8 + i * 8) as *const usize) as i32).collect()
}

unsafe fn fd_set_write_back(set: *mut u8, ready: &[i32]) {
    if set.is_null() {
        return;
    }
    *(set as *mut u32) = ready.len() as u32;
    for (i, fd) in ready.iter().enumerate() {
        *(set.add(8 + i * 8) as *mut usize) = *fd as usize;
    }
}

fn win_flags_to_host(flags: i32) -> i32 {
    let mut f = flags & 0x7;
    if flags & 0x8 != 0 {
        f |= libc::MSG_WAITALL;
    }
    f
}

unsafe fn do_getaddrinfo(node: Option<&str>, service: Option<&str>, hints: *const u8) -> Result<*mut u8, i32> {
    let mut h: libc::addrinfo = std::mem::zeroed();
    if !hints.is_null() {
        h.ai_flags = *(hints as *const i32);
        h.ai_family = family_to_host(*(hints.add(4) as *const i32));
        h.ai_socktype = *(hints.add(8) as *const i32);
        h.ai_protocol = *(hints.add(12) as *const i32);
    }
    let cn = node.map(|n| CString::new(n).unwrap_or_default());
    let cs = service.map(|n| CString::new(n).unwrap_or_default());
    let mut res: *mut libc::addrinfo = std::ptr::null_mut();
    let rc = libc::getaddrinfo(
        cn.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
        cs.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
        &h,
        &mut res,
    );
    if rc != 0 {
        return Err(match rc {
            libc::EAI_NONAME => 11001,
            libc::EAI_AGAIN => 11002,
            libc::EAI_FAIL => 11003,
            libc::EAI_SERVICE => 10109,
            libc::EAI_FAMILY => 10047,
            libc::EAI_SOCKTYPE => 10044,
            libc::EAI_MEMORY => 8,
            _ => 11001,
        });
    }
    let mut head: *mut u8 = std::ptr::null_mut();
    let mut tail: *mut u8 = std::ptr::null_mut();
    let mut cur = res;
    while !cur.is_null() {
        let ai = &*cur;
        let blk = libc::calloc(1, 48) as *mut u8;
        *(blk as *mut i32) = ai.ai_flags;
        *(blk.add(4) as *mut i32) = family_to_win(ai.ai_family);
        *(blk.add(8) as *mut i32) = ai.ai_socktype;
        *(blk.add(12) as *mut i32) = ai.ai_protocol;
        *(blk.add(16) as *mut usize) = ai.ai_addrlen as usize;
        if !ai.ai_canonname.is_null() {
            *(blk.add(24) as *mut usize) = libc::strdup(ai.ai_canonname) as usize;
        }
        let sa = libc::malloc(ai.ai_addrlen as usize) as *mut u8;
        std::ptr::copy_nonoverlapping(ai.ai_addr as *const u8, sa, ai.ai_addrlen as usize);
        let fam = (family_to_win(ai.ai_family) as u16).to_le_bytes();
        *sa = fam[0];
        *sa.add(1) = fam[1];
        *(blk.add(32) as *mut usize) = sa as usize;
        if head.is_null() {
            head = blk;
        } else {
            *(tail.add(40) as *mut usize) = blk as usize;
        }
        tail = blk;
        cur = ai.ai_next;
    }
    libc::freeaddrinfo(res);
    Ok(head)
}

unsafe fn do_freeaddrinfo(mut p: *mut u8) {
    while !p.is_null() {
        let next = *(p.add(40) as *const usize) as *mut u8;
        libc::free(*(p.add(24) as *const usize) as *mut c_void);
        libc::free(*(p.add(32) as *const usize) as *mut c_void);
        libc::free(p as *mut c_void);
        p = next;
    }
}

unsafe fn scatter_io(s: SOCKET, bufs: *const u8, count: u32, send: bool, flags: i32, addr: Option<(*const u8, i32)>) -> Result<isize, ()> {
    let mut iov: Vec<libc::iovec> = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let len = *(bufs.add(i * 16) as *const u32) as usize;
        let ptr = *(bufs.add(i * 16 + 8) as *const usize) as *mut c_void;
        iov.push(libc::iovec { iov_base: ptr, iov_len: len });
    }
    let mut msg: libc::msghdr = std::mem::zeroed();
    msg.msg_iov = iov.as_mut_ptr();
    msg.msg_iovlen = iov.len();
    let host_addr;
    if let Some((a, l)) = addr {
        host_addr = sockaddr_to_host(a, l);
        msg.msg_name = host_addr.as_ptr() as *mut c_void;
        msg.msg_namelen = host_addr.len() as u32;
    }
    let r = if send {
        libc::sendmsg(s as i32, &msg, win_flags_to_host(flags) | libc::MSG_NOSIGNAL)
    } else {
        libc::recvmsg(s as i32, &mut msg, win_flags_to_host(flags))
    };
    if r < 0 { Err(()) } else { Ok(r) }
}

crate::shims! { "ws2_32" => WS2_SHIMS;
    fn WSAStartup(version: WORD, data: *mut u8) -> i32 {
        if !data.is_null() {
            std::ptr::write_bytes(data, 0, 408);
            *(data as *mut u16) = version;
            *(data.add(2) as *mut u16) = 0x0202;
            *(data.add(4) as *mut u16) = 32767;
            *(data.add(6) as *mut u16) = 65467;
            let d = b"AnyEXE Winsock 2.2";
            std::ptr::copy_nonoverlapping(d.as_ptr(), data.add(16), d.len());
            let st = b"Running";
            std::ptr::copy_nonoverlapping(st.as_ptr(), data.add(273), st.len());
        }
        0
    }
    fn WSACleanup() -> i32 {
        0
    }
    fn WSAGetLastError() -> i32 {
        last_error() as i32
    }
    fn WSASetLastError(e: i32) {
        set_last_error(e as DWORD);
    }
    fn socket(af: i32, ty: i32, proto: i32) -> SOCKET {
        let fd = libc::socket(family_to_host(af), ty | libc::SOCK_CLOEXEC, proto);
        if fd < 0 {
            fail_errno();
            return INVALID_SOCKET;
        }
        fd as SOCKET
    }
    fn WSASocketW(af: i32, ty: i32, proto: i32, _info: usize, _group: u32, _flags: DWORD) -> SOCKET {
        socket(af, ty, proto)
    }
    fn WSASocketA(af: i32, ty: i32, proto: i32, _info: usize, _group: u32, _flags: DWORD) -> SOCKET {
        socket(af, ty, proto)
    }
    fn closesocket(s: SOCKET) -> i32 {
        if libc::close(s as i32) != 0 { fail_errno() } else { 0 }
    }
    fn shutdown(s: SOCKET, how: i32) -> i32 {
        if libc::shutdown(s as i32, how) != 0 { fail_errno() } else { 0 }
    }
    fn bind(s: SOCKET, addr: *const u8, len: i32) -> i32 {
        let a = sockaddr_to_host(addr, len);
        if libc::bind(s as i32, a.as_ptr() as *const libc::sockaddr, a.len() as u32) != 0 { fail_errno() } else { 0 }
    }
    fn listen(s: SOCKET, backlog: i32) -> i32 {
        if libc::listen(s as i32, backlog) != 0 { fail_errno() } else { 0 }
    }
    fn accept(s: SOCKET, addr: *mut u8, len: *mut i32) -> SOCKET {
        let mut storage = [0u8; 128];
        let mut l: libc::socklen_t = storage.len() as u32;
        let fd = libc::accept4(s as i32, storage.as_mut_ptr() as *mut libc::sockaddr, &mut l, libc::SOCK_CLOEXEC);
        if fd < 0 {
            fail_errno();
            return INVALID_SOCKET;
        }
        sockaddr_to_win(&storage[..l as usize], addr, len);
        fd as SOCKET
    }
    fn connect(s: SOCKET, addr: *const u8, len: i32) -> i32 {
        let a = sockaddr_to_host(addr, len);
        if libc::connect(s as i32, a.as_ptr() as *const libc::sockaddr, a.len() as u32) != 0 { fail_errno() } else { 0 }
    }
    fn send(s: SOCKET, buf: *const u8, len: i32, flags: i32) -> i32 {
        let r = libc::send(s as i32, buf as *const c_void, len as usize, win_flags_to_host(flags) | libc::MSG_NOSIGNAL);
        if r < 0 { fail_errno() } else { r as i32 }
    }
    fn recv(s: SOCKET, buf: *mut u8, len: i32, flags: i32) -> i32 {
        let r = libc::recv(s as i32, buf as *mut c_void, len as usize, win_flags_to_host(flags));
        if r < 0 { fail_errno() } else { r as i32 }
    }
    fn sendto(s: SOCKET, buf: *const u8, len: i32, flags: i32, addr: *const u8, alen: i32) -> i32 {
        let a = sockaddr_to_host(addr, alen);
        let r = libc::sendto(s as i32, buf as *const c_void, len as usize, win_flags_to_host(flags) | libc::MSG_NOSIGNAL, a.as_ptr() as *const libc::sockaddr, a.len() as u32);
        if r < 0 { fail_errno() } else { r as i32 }
    }
    fn recvfrom(s: SOCKET, buf: *mut u8, len: i32, flags: i32, addr: *mut u8, alen: *mut i32) -> i32 {
        let mut storage = [0u8; 128];
        let mut l: libc::socklen_t = storage.len() as u32;
        let r = libc::recvfrom(s as i32, buf as *mut c_void, len as usize, win_flags_to_host(flags), storage.as_mut_ptr() as *mut libc::sockaddr, &mut l);
        if r < 0 {
            return fail_errno();
        }
        sockaddr_to_win(&storage[..l as usize], addr, alen);
        r as i32
    }
    fn WSASend(s: SOCKET, bufs: *const u8, count: u32, sent: *mut u32, flags: u32, ov: usize, _cb: usize) -> i32 {
        if ov != 0 {
            set_last_error(ERROR_NOT_SUPPORTED);
            return SOCKET_ERROR;
        }
        match scatter_io(s, bufs, count, true, flags as i32, None) {
            Ok(n) => {
                if !sent.is_null() {
                    *sent = n as u32;
                }
                0
            }
            Err(()) => fail_errno(),
        }
    }
    fn WSARecv(s: SOCKET, bufs: *const u8, count: u32, received: *mut u32, flags: *mut u32, ov: usize, _cb: usize) -> i32 {
        if ov != 0 {
            set_last_error(ERROR_NOT_SUPPORTED);
            return SOCKET_ERROR;
        }
        let f = if flags.is_null() { 0 } else { *flags as i32 };
        match scatter_io(s, bufs, count, false, f, None) {
            Ok(n) => {
                if !received.is_null() {
                    *received = n as u32;
                }
                0
            }
            Err(()) => fail_errno(),
        }
    }
    fn select(_nfds: i32, rd: *mut u8, wr: *mut u8, ex: *mut u8, timeout: *const i32) -> i32 {
        let (r, w, e) = (fd_set_to_vec(rd), fd_set_to_vec(wr), fd_set_to_vec(ex));
        let mut fds: Vec<libc::pollfd> = Vec::new();
        for (list, ev) in [(&r, libc::POLLIN), (&w, libc::POLLOUT), (&e, libc::POLLPRI)] {
            for fd in list.iter() {
                match fds.iter_mut().find(|p| p.fd == *fd) {
                    Some(p) => p.events |= ev,
                    None => fds.push(libc::pollfd { fd: *fd, events: ev, revents: 0 }),
                }
            }
        }
        let ms = if timeout.is_null() { -1 } else { *timeout * 1000 + *timeout.add(1) / 1000 };
        let rc = libc::poll(fds.as_mut_ptr(), fds.len() as u64, ms);
        if rc < 0 {
            return fail_errno();
        }
        let ready = |list: &Vec<i32>, bits: i16| -> Vec<i32> {
            list.iter().copied().filter(|fd| fds.iter().any(|p| p.fd == *fd && p.revents & (bits | libc::POLLERR | libc::POLLHUP) != 0)).collect()
        };
        let (rr, wr_ready, er) = (ready(&r, libc::POLLIN), ready(&w, libc::POLLOUT), ready(&e, libc::POLLPRI));
        fd_set_write_back(rd, &rr);
        fd_set_write_back(wr, &wr_ready);
        fd_set_write_back(ex, &er);
        (rr.len() + wr_ready.len() + er.len()) as i32
    }
    fn __WSAFDIsSet(s: SOCKET, set: *const u8) -> i32 {
        fd_set_to_vec(set).contains(&(s as i32)) as i32
    }
    fn WSAPoll(fds: *mut u8, n: u32, ms: i32) -> i32 {
        let mut host: Vec<libc::pollfd> = (0..n as usize)
            .map(|i| {
                let p = fds.add(i * 16);
                let ev = *(p.add(8) as *const i16);
                let mut h = 0i16;
                if ev & 0x300 != 0 {
                    h |= libc::POLLIN;
                }
                if ev & 0x10 != 0 {
                    h |= libc::POLLOUT;
                }
                libc::pollfd { fd: *(p as *const usize) as i32, events: h, revents: 0 }
            })
            .collect();
        let rc = libc::poll(host.as_mut_ptr(), n as u64, ms);
        if rc < 0 {
            return fail_errno();
        }
        for (i, h) in host.iter().enumerate() {
            let mut r = 0i16;
            if h.revents & libc::POLLIN != 0 {
                r |= 0x100;
            }
            if h.revents & libc::POLLOUT != 0 {
                r |= 0x10;
            }
            if h.revents & libc::POLLERR != 0 {
                r |= 1;
            }
            if h.revents & libc::POLLHUP != 0 {
                r |= 2;
            }
            if h.revents & libc::POLLNVAL != 0 {
                r |= 4;
            }
            *(fds.add(i * 16 + 10) as *mut i16) = r;
        }
        rc
    }
    fn ioctlsocket(s: SOCKET, cmd: u32, arg: *mut u32) -> i32 {
        match cmd {
            FIONBIO => {
                let fl = libc::fcntl(s as i32, libc::F_GETFL);
                let nf = if *arg != 0 { fl | libc::O_NONBLOCK } else { fl & !libc::O_NONBLOCK };
                if libc::fcntl(s as i32, libc::F_SETFL, nf) < 0 { fail_errno() } else { 0 }
            }
            FIONREAD => {
                let mut n: libc::c_int = 0;
                if libc::ioctl(s as i32, libc::FIONREAD, &mut n) < 0 {
                    return fail_errno();
                }
                *arg = n as u32;
                0
            }
            _ => {
                set_last_error(10022);
                SOCKET_ERROR
            }
        }
    }
    fn WSAIoctl(_s: SOCKET, _code: u32, _inb: usize, _inl: u32, _outb: usize, _outl: u32, _ret: *mut u32, _ov: usize, _cb: usize) -> i32 {
        set_last_error(10022);
        SOCKET_ERROR
    }
    fn getsockopt(s: SOCKET, level: i32, name: i32, val: *mut u8, len: *mut i32) -> i32 {
        let Some((l, n)) = sockopt_to_host(level, name) else {
            set_last_error(10042);
            return SOCKET_ERROR;
        };
        if level == WIN_SOL_SOCKET && (name == 0x1005 || name == 0x1006) {
            let mut tv: libc::timeval = std::mem::zeroed();
            let mut tl = std::mem::size_of::<libc::timeval>() as u32;
            if libc::getsockopt(s as i32, l, n, &mut tv as *mut _ as *mut c_void, &mut tl) != 0 {
                return fail_errno();
            }
            *(val as *mut u32) = (tv.tv_sec * 1000 + tv.tv_usec / 1000) as u32;
            *len = 4;
            return 0;
        }
        if level == WIN_SOL_SOCKET && name == 0x1007 {
            let mut v: i32 = 0;
            let mut vl = 4u32;
            if libc::getsockopt(s as i32, l, n, &mut v as *mut _ as *mut c_void, &mut vl) != 0 {
                return fail_errno();
            }
            *(val as *mut i32) = if v == 0 { 0 } else { wsa_error(v) as i32 };
            *len = 4;
            return 0;
        }
        let mut l2 = *len as u32;
        if libc::getsockopt(s as i32, l, n, val as *mut c_void, &mut l2) != 0 {
            return fail_errno();
        }
        *len = l2 as i32;
        0
    }
    fn setsockopt(s: SOCKET, level: i32, name: i32, val: *const u8, len: i32) -> i32 {
        let Some((l, n)) = sockopt_to_host(level, name) else {
            return 0;
        };
        if level == WIN_SOL_SOCKET && (name == 0x1005 || name == 0x1006) {
            let ms = *(val as *const u32);
            let tv = libc::timeval { tv_sec: (ms / 1000) as libc::time_t, tv_usec: ((ms % 1000) * 1000) as libc::suseconds_t };
            return if libc::setsockopt(s as i32, l, n, &tv as *const _ as *const c_void, std::mem::size_of::<libc::timeval>() as u32) != 0 { fail_errno() } else { 0 };
        }
        if level == WIN_SOL_SOCKET && name == 0x0080 {
            let lg = libc::linger { l_onoff: *(val as *const u16) as i32, l_linger: *(val.add(2) as *const u16) as i32 };
            return if libc::setsockopt(s as i32, l, n, &lg as *const _ as *const c_void, std::mem::size_of::<libc::linger>() as u32) != 0 { fail_errno() } else { 0 };
        }
        if libc::setsockopt(s as i32, l, n, val as *const c_void, len as u32) != 0 { fail_errno() } else { 0 }
    }
    fn getsockname(s: SOCKET, addr: *mut u8, len: *mut i32) -> i32 {
        let mut storage = [0u8; 128];
        let mut l: libc::socklen_t = storage.len() as u32;
        if libc::getsockname(s as i32, storage.as_mut_ptr() as *mut libc::sockaddr, &mut l) != 0 {
            return fail_errno();
        }
        sockaddr_to_win(&storage[..l as usize], addr, len);
        0
    }
    fn getpeername(s: SOCKET, addr: *mut u8, len: *mut i32) -> i32 {
        let mut storage = [0u8; 128];
        let mut l: libc::socklen_t = storage.len() as u32;
        if libc::getpeername(s as i32, storage.as_mut_ptr() as *mut libc::sockaddr, &mut l) != 0 {
            return fail_errno();
        }
        sockaddr_to_win(&storage[..l as usize], addr, len);
        0
    }
    fn gethostname(buf: LPSTR, len: i32) -> i32 {
        let h = crate::runtime::host_name();
        if copy_a(buf, len as usize, h.as_bytes()) { 0 } else {
            set_last_error(10014);
            SOCKET_ERROR
        }
    }
    fn GetHostNameW(buf: LPWSTR, len: i32) -> i32 {
        let h = crate::runtime::host_name();
        if copy_w(buf, len as usize, &to_wide(&h)) { 0 } else {
            set_last_error(10014);
            SOCKET_ERROR
        }
    }
    fn getaddrinfo(node: LPCSTR, service: LPCSTR, hints: *const u8, res: *mut *mut u8) -> i32 {
        let n = if node.is_null() { None } else { Some(a_to_string(node)) };
        let sv = if service.is_null() { None } else { Some(a_to_string(service)) };
        match do_getaddrinfo(n.as_deref(), sv.as_deref(), hints) {
            Ok(p) => {
                *res = p;
                0
            }
            Err(e) => {
                set_last_error(e as DWORD);
                e
            }
        }
    }
    fn GetAddrInfoW(node: LPCWSTR, service: LPCWSTR, hints: *const u8, res: *mut *mut u8) -> i32 {
        let n = if node.is_null() { None } else { Some(w_ptr_to_string(node)) };
        let sv = if service.is_null() { None } else { Some(w_ptr_to_string(service)) };
        match do_getaddrinfo(n.as_deref(), sv.as_deref(), hints) {
            Ok(p) => {
                *res = p;
                0
            }
            Err(e) => {
                set_last_error(e as DWORD);
                e
            }
        }
    }
    fn freeaddrinfo(p: *mut u8) {
        do_freeaddrinfo(p);
    }
    fn FreeAddrInfoW(p: *mut u8) {
        do_freeaddrinfo(p);
    }
    fn getnameinfo(addr: *const u8, alen: i32, host: LPSTR, hlen: u32, serv: LPSTR, slen: u32, flags: i32) -> i32 {
        let a = sockaddr_to_host(addr, alen);
        let rc = libc::getnameinfo(a.as_ptr() as *const libc::sockaddr, a.len() as u32, host as *mut libc::c_char, hlen, serv as *mut libc::c_char, slen, flags);
        if rc != 0 { 11001 } else { 0 }
    }
    fn inet_addr(s: LPCSTR) -> u32 {
        let c = CStr::from_ptr(s as *const libc::c_char);
        let mut a: libc::in_addr = std::mem::zeroed();
        if cc::inet_pton(libc::AF_INET, c.as_ptr(), &mut a as *mut _ as *mut c_void) == 1 { a.s_addr } else { u32::MAX }
    }
    fn inet_ntoa(a: u32) -> LPSTR {
        thread_local! { static BUF: std::cell::UnsafeCell<[u8; 20]> = const { std::cell::UnsafeCell::new([0; 20]) }; }
        let b = a.to_le_bytes();
        let s = format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]);
        BUF.with(|buf| {
            let p = buf.get() as *mut u8;
            copy_a(p, 20, s.as_bytes());
            p
        })
    }
    fn inet_pton(af: i32, src: LPCSTR, dst: *mut u8) -> i32 {
        cc::inet_pton(family_to_host(af), src as *const libc::c_char, dst as *mut c_void)
    }
    fn InetPtonW(af: i32, src: LPCWSTR, dst: *mut u8) -> i32 {
        let s = CString::new(w_ptr_to_string(src)).unwrap_or_default();
        cc::inet_pton(family_to_host(af), s.as_ptr(), dst as *mut c_void)
    }
    fn inet_ntop(af: i32, src: *const u8, dst: LPSTR, size: usize) -> LPCSTR {
        cc::inet_ntop(family_to_host(af), src as *const c_void, dst as *mut libc::c_char, size as u32) as LPCSTR
    }
    fn htons(v: u16) -> u16 {
        v.to_be()
    }
    fn ntohs(v: u16) -> u16 {
        u16::from_be(v)
    }
    fn htonl(v: u32) -> u32 {
        v.to_be()
    }
    fn ntohl(v: u32) -> u32 {
        u32::from_be(v)
    }
    fn WSACreateEvent() -> HANDLE {
        crate::shim::k32_sync::CreateEventW(0, TRUE, FALSE, std::ptr::null())
    }
    fn WSACloseEvent(h: HANDLE) -> BOOL {
        crate::shim::k32_sync::CloseHandle(h)
    }
    fn WSASetEvent(h: HANDLE) -> BOOL {
        crate::shim::k32_sync::SetEvent(h)
    }
    fn WSAResetEvent(h: HANDLE) -> BOOL {
        crate::shim::k32_sync::ResetEvent(h)
    }
    fn WSAWaitForMultipleEvents(n: u32, hs: *const HANDLE, all: BOOL, ms: u32, alertable: BOOL) -> u32 {
        crate::shim::k32_sync::WaitForMultipleObjectsEx(n, hs, all, ms, alertable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes() {
        assert_eq!(wsa_error(libc::ECONNREFUSED), 10061);
        assert_eq!(wsa_error(libc::EWOULDBLOCK), 10035);
        assert_eq!(wsa_error(libc::EADDRINUSE), 10048);
    }

    #[test]
    fn family_translation() {
        assert_eq!(family_to_host(23), libc::AF_INET6);
        assert_eq!(family_to_win(libc::AF_INET6), 23);
        assert_eq!(family_to_host(2), 2);
    }
}
