//! kernel32: files, directories, pipes and path APIs.

use crate::runtime::handles::{self, FileObj, FindEntry, FindObj, Kind};
use crate::runtime::paths::{self, Device};
use crate::runtime::strings::*;
use crate::win::*;
use std::ffi::CString;
use std::fs::Metadata;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::Ordering;

const FILE_READ_DATA: DWORD = 0x1;
const FILE_WRITE_DATA: DWORD = 0x2;
const EPOCH_DIFF_SECS: i64 = 11_644_473_600;

fn cpath(p: &Path) -> CString {
    CString::new(p.as_os_str().to_string_lossy().into_owned()).unwrap_or_default()
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn err_from_io(e: &std::io::Error, parent_missing: bool) -> DWORD {
    match e.raw_os_error() {
        Some(libc::ENOENT) if parent_missing => ERROR_PATH_NOT_FOUND,
        Some(n) => errno_to_win(n),
        None => ERROR_INVALID_FUNCTION,
    }
}

fn parent_missing(p: &Path) -> bool {
    p.parent().map(|d| !d.as_os_str().is_empty() && !d.exists()).unwrap_or(false)
}

fn unix_to_filetime(sec: i64, nsec: i64) -> FILETIME {
    FILETIME::from_u64(((sec + EPOCH_DIFF_SECS) as u64) * 10_000_000 + (nsec / 100) as u64)
}

fn md_times(md: &Metadata) -> (FILETIME, FILETIME, FILETIME) {
    let created = md.created().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
    let creation = match created {
        Some(d) => unix_to_filetime(d.as_secs() as i64, d.subsec_nanos() as i64),
        None => unix_to_filetime(md.ctime(), md.ctime_nsec()),
    };
    (creation, unix_to_filetime(md.atime(), md.atime_nsec()), unix_to_filetime(md.mtime(), md.mtime_nsec()))
}

fn md_attrs(md: &Metadata) -> DWORD {
    let mut a = 0;
    if md.is_dir() {
        a |= FILE_ATTRIBUTE_DIRECTORY;
    }
    if md.file_type().is_symlink() {
        a |= FILE_ATTRIBUTE_REPARSE_POINT;
    }
    if md.permissions().mode() & 0o222 == 0 {
        a |= FILE_ATTRIBUTE_READONLY;
    }
    if a & FILE_ATTRIBUTE_DIRECTORY == 0 && a & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        a |= FILE_ATTRIBUTE_ARCHIVE;
    }
    a
}

fn with_file<R>(h: HANDLE, f: impl FnOnce(&FileObj) -> R) -> Option<R> {
    match handles::get(h).as_deref() {
        Some(Kind::File(file)) => Some(f(file)),
        _ => {
            set_last_error(ERROR_INVALID_HANDLE);
            None
        }
    }
}

/// A relinked program that opens its own `.exe` for reading gets the original file (an appended archive and
/// all) when the relinker kept it, as a memory file opened through `/proc/self/fd`; anything else gets the ELF.
fn embedded_original(host: &Path, write: bool) -> Option<PathBuf> {
    use std::io::Write;
    use std::os::fd::{FromRawFd, IntoRawFd};
    static FD: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();
    if write || crate::runtime::cfg().elf_alias.as_deref() != Some(host) {
        return None;
    }
    let fd = (*FD.get_or_init(|| {
        let bytes = crate::relink::read_embedded(host)?;
        let fd = unsafe { libc::memfd_create(c"nle-original".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
        f.write_all(&bytes).ok()?;
        Some(f.into_raw_fd())
    }))?;
    Some(PathBuf::from(format!("/proc/self/fd/{fd}")))
}

pub(crate) unsafe fn open_file(path: &str, access: DWORD, disposition: DWORD, attrs_flags: DWORD) -> HANDLE {
    let want_read = access & (GENERIC_READ | FILE_READ_DATA | GENERIC_ALL) != 0;
    let want_write = access & (GENERIC_WRITE | FILE_WRITE_DATA | FILE_APPEND_DATA | GENERIC_ALL) != 0;

    if let Some(dev) = paths::device(path) {
        let fd = match dev {
            Device::Null => libc::open(b"/dev/null\0".as_ptr() as *const libc::c_char, libc::O_RDWR),
            Device::ConIn => libc::dup(0),
            Device::ConOut => libc::dup(1),
            Device::Con => libc::dup(if want_write { 1 } else { 0 }),
        };
        if fd < 0 {
            set_last_error(errno_to_win(errno()));
            return INVALID_HANDLE_VALUE;
        }
        return handles::insert(Kind::File(FileObj::new(fd, true)));
    }

    if let Some(name) = crate::shim::ntdll_file::pipe_name(path) {
        return match crate::shim::ntdll_file::take_pipe_client(&name) {
            Some(fd) => handles::insert(Kind::File(FileObj::new(fd, true))),
            None => {
                set_last_error(ERROR_FILE_NOT_FOUND);
                INVALID_HANDLE_VALUE
            }
        };
    }

    let host = paths::resolve(path);
    let host = embedded_original(&host, want_write).unwrap_or(host);
    let mut flags = libc::O_CLOEXEC;
    flags |= match (want_read || !want_write, want_write) {
        (true, true) => libc::O_RDWR,
        (false, true) => libc::O_WRONLY,
        _ => libc::O_RDONLY,
    };
    let existed = host.exists();
    match disposition {
        CREATE_NEW => flags |= libc::O_CREAT | libc::O_EXCL,
        CREATE_ALWAYS => flags |= libc::O_CREAT | libc::O_TRUNC,
        OPEN_EXISTING => {}
        OPEN_ALWAYS => flags |= libc::O_CREAT,
        TRUNCATE_EXISTING => flags |= libc::O_TRUNC,
        _ => {
            set_last_error(ERROR_INVALID_PARAMETER);
            return INVALID_HANDLE_VALUE;
        }
    }
    let append = access & FILE_APPEND_DATA != 0 && access & (FILE_WRITE_DATA | GENERIC_WRITE | GENERIC_ALL) == 0;
    if append {
        flags |= libc::O_APPEND;
    }
    if host.is_dir() {
        flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
        if want_write || disposition == CREATE_NEW || disposition == CREATE_ALWAYS {
            set_last_error(if disposition == CREATE_NEW { ERROR_FILE_EXISTS } else { ERROR_ACCESS_DENIED });
            return INVALID_HANDLE_VALUE;
        }
    }
    let mode: libc::mode_t = if attrs_flags & FILE_ATTRIBUTE_READONLY != 0 { 0o444 } else { 0o666 };
    let c = cpath(&host);
    let fd = libc::open(c.as_ptr(), flags, mode as libc::c_uint);
    if fd < 0 {
        let e = errno();
        set_last_error(if e == libc::EEXIST {
            ERROR_FILE_EXISTS
        } else if e == libc::ENOENT {
            if parent_missing(&host) { ERROR_PATH_NOT_FOUND } else { ERROR_FILE_NOT_FOUND }
        } else {
            errno_to_win(e)
        });
        return INVALID_HANDLE_VALUE;
    }
    let mut obj = FileObj::new(fd, true);
    obj.append = append;
    obj.path = Some(host.clone());
    if attrs_flags & FILE_FLAG_DELETE_ON_CLOSE != 0 {
        obj.delete_on_close = Some(host);
    }
    set_last_error(if existed && matches!(disposition, CREATE_ALWAYS | OPEN_ALWAYS) { ERROR_ALREADY_EXISTS } else { ERROR_SUCCESS });
    handles::insert(Kind::File(obj))
}

unsafe fn finish_overlapped(ov: *mut OVERLAPPED, n: usize) {
    if ov.is_null() {
        return;
    }
    (*ov).internal = 0;
    (*ov).internal_high = n;
    if (*ov).h_event != 0 {
        crate::shim::k32_sync::SetEvent((*ov).h_event & !1);
    }
}

pub(crate) unsafe fn read_file(h: HANDLE, buf: *mut u8, n: DWORD, read: *mut DWORD, ov: *mut OVERLAPPED) -> BOOL {
    let r = with_file(h, |f| {
        let fd = f.fd();
        loop {
            let r = if !ov.is_null() && !f.is_pipe && !f.is_tty {
                let off = ((*ov).offset_high as u64) << 32 | (*ov).offset as u64;
                libc::pread(fd, buf as *mut libc::c_void, n as usize, off as libc::off_t)
            } else {
                libc::read(fd, buf as *mut libc::c_void, n as usize)
            };
            if r < 0 && errno() == libc::EINTR {
                continue;
            }
            return (r, f.is_pipe);
        }
    });
    let Some((r, is_pipe)) = r else { return FALSE };
    if r < 0 {
        set_last_error(errno_to_win(errno()));
        return FALSE;
    }
    if !read.is_null() {
        *read = r as DWORD;
    }
    if r == 0 && n > 0 {
        if is_pipe {
            set_last_error(ERROR_BROKEN_PIPE);
            return FALSE;
        }
        if !ov.is_null() {
            set_last_error(ERROR_HANDLE_EOF);
            return FALSE;
        }
    }
    finish_overlapped(ov, r as usize);
    TRUE
}

pub(crate) unsafe fn write_file(h: HANDLE, buf: *const u8, n: DWORD, written: *mut DWORD, ov: *mut OVERLAPPED) -> BOOL {
    let r = with_file(h, |f| {
        let fd = f.fd();
        let mut done = 0usize;
        let use_off = !ov.is_null() && !f.is_pipe && !f.is_tty && !f.append;
        let base = if use_off { ((*ov).offset_high as u64) << 32 | (*ov).offset as u64 } else { 0 };
        while done < n as usize {
            let p = buf.add(done) as *const libc::c_void;
            let left = n as usize - done;
            let r = if use_off { libc::pwrite(fd, p, left, (base + done as u64) as libc::off_t) } else { libc::write(fd, p, left) };
            if r < 0 {
                if errno() == libc::EINTR {
                    continue;
                }
                return Err(errno());
            }
            done += r as usize;
        }
        Ok(done)
    });
    match r {
        None => FALSE,
        Some(Err(e)) => {
            set_last_error(errno_to_win(e));
            FALSE
        }
        Some(Ok(done)) => {
            if !written.is_null() {
                *written = done as DWORD;
            }
            finish_overlapped(ov, done);
            TRUE
        }
    }
}

fn file_pos_op(h: HANDLE, off: i64, method: DWORD) -> Option<i64> {
    let whence = match method {
        FILE_BEGIN => libc::SEEK_SET,
        FILE_CURRENT => libc::SEEK_CUR,
        FILE_END => libc::SEEK_END,
        _ => {
            set_last_error(ERROR_INVALID_PARAMETER);
            return None;
        }
    };
    with_file(h, |f| {
        let r = unsafe { libc::lseek(f.fd(), off as libc::off_t, whence) };
        if r < 0 {
            set_last_error(errno_to_win(errno()));
            None
        } else {
            Some(r as i64)
        }
    })
    .flatten()
}

fn file_size(h: HANDLE) -> Option<u64> {
    with_file(h, |f| {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(f.fd(), &mut st) } != 0 {
            set_last_error(errno_to_win(errno()));
            None
        } else {
            Some(st.st_size as u64)
        }
    })
    .flatten()
}

fn stat_path(win: &str) -> Result<(PathBuf, Metadata), DWORD> {
    let host = paths::resolve(win);
    match std::fs::symlink_metadata(&host) {
        Ok(md) => Ok((host, md)),
        Err(e) => Err(err_from_io(&e, parent_missing(&host))),
    }
}

unsafe fn attr_data(win: &str, out: *mut WIN32_FILE_ATTRIBUTE_DATA) -> BOOL {
    match stat_path(win) {
        Ok((host, mut md)) => {
            if md.file_type().is_symlink() {
                if let Ok(real) = std::fs::metadata(&host) {
                    md = real;
                }
            }
            let (c, a, w) = md_times(&md);
            *out = WIN32_FILE_ATTRIBUTE_DATA {
                attributes: md_attrs(&md),
                creation_time: c,
                last_access_time: a,
                last_write_time: w,
                size_high: (md.len() >> 32) as DWORD,
                size_low: md.len() as DWORD,
            };
            TRUE
        }
        Err(e) => {
            set_last_error(e);
            FALSE
        }
    }
}

fn wildcard(pat: &[char], name: &[char]) -> bool {
    match (pat.first(), name.first()) {
        (None, None) => true,
        (Some('*'), _) => wildcard(&pat[1..], name) || (!name.is_empty() && wildcard(pat, &name[1..])),
        (Some('?'), Some(_)) => wildcard(&pat[1..], &name[1..]),
        (Some(p), Some(n)) => p.to_lowercase().eq(n.to_lowercase()) && wildcard(&pat[1..], &name[1..]),
        _ => false,
    }
}

fn find_entries(win_pattern: &str) -> Result<Vec<FindEntry>, DWORD> {
    let (dir_part, pat) = match win_pattern.rfind(['\\', '/']) {
        Some(i) => (&win_pattern[..i + 1], &win_pattern[i + 1..]),
        None => ("", win_pattern),
    };
    let dir_host = if dir_part.is_empty() { PathBuf::from(".") } else { paths::resolve(dir_part) };
    let has_wild = pat.contains(['*', '?']);
    if !has_wild {
        let host = dir_host.join(pat);
        let host = paths::ci_resolve(&host);
        return match std::fs::symlink_metadata(&host) {
            Ok(_) => Ok(vec![FindEntry { name: host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), path: host }]),
            Err(_) if parent_missing(&host) => Err(ERROR_PATH_NOT_FOUND),
            Err(_) => Err(ERROR_FILE_NOT_FOUND),
        };
    }
    let rd = match std::fs::read_dir(&dir_host) {
        Ok(r) => r,
        Err(e) => return Err(if e.raw_os_error() == Some(libc::ENOENT) { ERROR_PATH_NOT_FOUND } else { err_from_io(&e, false) }),
    };
    let pat = if pat == "*.*" { "*" } else { pat };
    let pchars: Vec<char> = pat.chars().collect();
    let mut out: Vec<FindEntry> = Vec::new();
    let is_root = dir_host.parent().is_none();
    if !is_root {
        for dots in [".", ".."] {
            let chars: Vec<char> = dots.chars().collect();
            if wildcard(&pchars, &chars) {
                out.push(FindEntry { name: dots.to_string(), path: dir_host.join(dots) });
            }
        }
    }
    let mut entries: Vec<FindEntry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let chars: Vec<char> = name.chars().collect();
            wildcard(&pchars, &chars).then(|| FindEntry { path: e.path(), name })
        })
        .collect();
    entries.sort_by_key(|e| e.name.to_uppercase());
    out.extend(entries);
    if out.is_empty() {
        return Err(ERROR_FILE_NOT_FOUND);
    }
    Ok(out)
}

fn entry_metadata(e: &FindEntry) -> Option<Metadata> {
    std::fs::symlink_metadata(&e.path).ok()
}

unsafe fn fill_find_w(out: *mut WIN32_FIND_DATAW, e: &FindEntry) {
    std::ptr::write_bytes(out as *mut u8, 0, std::mem::size_of::<WIN32_FIND_DATAW>());
    if let Some(md) = entry_metadata(e) {
        let (c, a, w) = md_times(&md);
        (*out).attributes = md_attrs(&md);
        (*out).creation_time = c;
        (*out).last_access_time = a;
        (*out).last_write_time = w;
        (*out).size_high = (md.len() >> 32) as DWORD;
        (*out).size_low = md.len() as DWORD;
    }
    let w = to_wide(&e.name);
    let n = w.len().min(MAX_PATH - 1);
    std::ptr::copy_nonoverlapping(w.as_ptr(), std::ptr::addr_of_mut!((*out).file_name) as *mut u16, n);
}

unsafe fn fill_find_a(out: *mut WIN32_FIND_DATAA, e: &FindEntry) {
    let mut tmp: WIN32_FIND_DATAW = std::mem::zeroed();
    fill_find_w(&mut tmp, e);
    std::ptr::write_bytes(out as *mut u8, 0, std::mem::size_of::<WIN32_FIND_DATAA>());
    (*out).attributes = tmp.attributes;
    (*out).creation_time = tmp.creation_time;
    (*out).last_access_time = tmp.last_access_time;
    (*out).last_write_time = tmp.last_write_time;
    (*out).size_high = tmp.size_high;
    (*out).size_low = tmp.size_low;
    let b = e.name.as_bytes();
    let n = b.len().min(MAX_PATH - 1);
    std::ptr::copy_nonoverlapping(b.as_ptr(), std::ptr::addr_of_mut!((*out).file_name) as *mut u8, n);
}

unsafe fn find_first(win_pattern: &str, fill: impl FnOnce(&FindEntry)) -> HANDLE {
    match find_entries(win_pattern) {
        Ok(entries) => {
            fill(&entries[0]);
            handles::insert(Kind::Find(FindObj { entries: Mutex::new((entries, 1)) }))
        }
        Err(e) => {
            set_last_error(e);
            INVALID_HANDLE_VALUE
        }
    }
}

unsafe fn find_next(h: HANDLE, fill: impl FnOnce(&FindEntry)) -> BOOL {
    match handles::get(h).as_deref() {
        Some(Kind::Find(f)) => {
            let mut g = f.entries.lock().unwrap();
            if g.1 >= g.0.len() {
                set_last_error(ERROR_NO_MORE_FILES);
                return FALSE;
            }
            fill(&g.0[g.1]);
            g.1 += 1;
            TRUE
        }
        _ => {
            set_last_error(ERROR_INVALID_HANDLE);
            FALSE
        }
    }
}

fn status(r: std::io::Result<()>, host: &Path) -> BOOL {
    match r {
        Ok(()) => TRUE,
        Err(e) => {
            set_last_error(err_from_io(&e, parent_missing(host)));
            FALSE
        }
    }
}

fn move_file(old: &str, new: &str, replace: bool) -> BOOL {
    let (o, n) = (paths::resolve(old), paths::resolve(new));
    if !replace && std::fs::symlink_metadata(&n).is_ok() {
        set_last_error(ERROR_ALREADY_EXISTS);
        return FALSE;
    }
    match std::fs::rename(&o, &n) {
        Ok(()) => TRUE,
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => match std::fs::copy(&o, &n).and_then(|_| std::fs::remove_file(&o)) {
            Ok(()) => TRUE,
            Err(e2) => {
                set_last_error(err_from_io(&e2, false));
                FALSE
            }
        },
        Err(e) => {
            set_last_error(err_from_io(&e, parent_missing(&n) || !o.exists() && parent_missing(&o)));
            FALSE
        }
    }
}

fn copy_file(old: &str, new: &str, fail_if_exists: bool) -> BOOL {
    let (o, n) = (paths::resolve(old), paths::resolve(new));
    if fail_if_exists && n.exists() {
        set_last_error(ERROR_FILE_EXISTS);
        return FALSE;
    }
    match std::fs::copy(&o, &n) {
        Ok(_) => TRUE,
        Err(e) => {
            set_last_error(err_from_io(&e, parent_missing(&n)));
            FALSE
        }
    }
}

unsafe fn get_full_path_w(path: &str, size: DWORD, buf: LPWSTR, filepart: *mut LPWSTR) -> DWORD {
    let full = paths::full_path(path);
    let w = to_wide(&full);
    if w.len() + 1 > size as usize {
        return w.len() as DWORD + 1;
    }
    copy_w(buf, size as usize, &w);
    if !filepart.is_null() {
        *filepart = match full.rfind('\\') {
            Some(i) if i + 1 < full.len() => buf.add(full[..i + 1].encode_utf16().count()),
            _ => std::ptr::null_mut(),
        };
    }
    w.len() as DWORD
}

unsafe fn get_cwd_w(size: DWORD, buf: LPWSTR) -> DWORD {
    return_w(buf, size, &paths::cwd_win())
}

unsafe fn set_cwd(path: &str) -> BOOL {
    let host = paths::resolve(path);
    match std::env::set_current_dir(&host) {
        Ok(()) => TRUE,
        Err(e) => {
            set_last_error(err_from_io(&e, false));
            FALSE
        }
    }
}

unsafe fn fd_path(f: &FileObj) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{}", f.fd())).ok()
}

fn set_times(h: HANDLE, c: *const FILETIME, a: *const FILETIME, w: *const FILETIME) -> BOOL {
    let conv = |p: *const FILETIME| -> libc::timespec {
        if p.is_null() {
            return libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_OMIT };
        }
        let v = unsafe { (*p).to_u64() };
        if v == 0 {
            return libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_OMIT };
        }
        let secs = (v / 10_000_000) as i64 - EPOCH_DIFF_SECS;
        libc::timespec { tv_sec: secs as libc::time_t, tv_nsec: ((v % 10_000_000) * 100) as i64 }
    };
    let _ = c;
    let times = [conv(a), conv(w)];
    with_file(h, |f| {
        if unsafe { libc::futimens(f.fd(), times.as_ptr()) } == 0 {
            TRUE
        } else {
            set_last_error(errno_to_win(errno()));
            FALSE
        }
    })
    .unwrap_or(FALSE)
}

unsafe fn set_info_by_handle(h: HANDLE, class: i32, info: *const u8, size: DWORD) -> BOOL {
    let path = with_file(h, |f| f.path.clone().or_else(|| fd_path(f))).flatten();
    match class {
        4 | 21 => {
            let delete = if class == 4 { *info != 0 } else { *(info as *const u32) & 1 != 0 };
            if !delete {
                return TRUE;
            }
            let Some(p) = path else {
                set_last_error(ERROR_INVALID_HANDLE);
                return FALSE;
            };
            let r = if p.is_dir() { std::fs::remove_dir(&p) } else { std::fs::remove_file(&p) };
            status(r, &p)
        }
        3 | 22 => {
            let Some(from) = path else {
                set_last_error(ERROR_INVALID_HANDLE);
                return FALSE;
            };
            let flags = *(info as *const u32);
            let replace = if class == 3 { *info != 0 } else { flags & 1 != 0 };
            let len = (info.add(16) as *const u32).read_unaligned() as usize / 2;
            let name = std::slice::from_raw_parts(info.add(20) as *const u16, len.min(size as usize / 2));
            let to = w_to_string(name);
            move_file(&paths::host_to_win(&from), &to, replace)
        }
        6 => {
            let len = *(info as *const i64);
            with_file(h, |f| if libc::ftruncate(f.fd(), len as libc::off_t) == 0 { TRUE } else {
                set_last_error(errno_to_win(errno()));
                FALSE
            })
            .unwrap_or(FALSE)
        }
        5 => TRUE,
        0 => {
            let ft = |o: usize| info.add(o) as *const FILETIME;
            set_times(h, ft(0), ft(8), ft(16))
        }
        _ => {
            set_last_error(ERROR_INVALID_PARAMETER);
            FALSE
        }
    }
}

fn win_name(a: LPCSTR) -> String {
    unsafe { a_to_string(a) }
}

fn win_namew(w: LPCWSTR) -> String {
    unsafe { w_ptr_to_string(w) }
}

/// `SearchPath`: look for `file` in `dir`, or in the usual places (program directory, current directory, the
/// system directories, then `PATH`), adding `ext` when the name has no extension.
fn search_path(dir: Option<String>, file: &str, ext: Option<String>) -> Option<String> {
    let base = file.rsplit(['\\', '/']).next().unwrap_or(file);
    let name = match &ext {
        Some(e) if !base.contains('.') => format!("{file}{e}"),
        _ => file.to_string(),
    };
    let is_file = |p: &str| stat_path(p).is_ok_and(|(_, md)| !md.is_dir());
    if name.contains(':') || name.starts_with('\\') {
        return is_file(&name).then(|| paths::full_path(&name));
    }
    let mut dirs: Vec<String> = Vec::new();
    match dir {
        Some(d) => dirs.push(d),
        None => {
            if let Some((d, _)) = crate::runtime::cfg().exe_win.rsplit_once('\\') {
                dirs.push(d.to_string());
            }
            dirs.push(paths::cwd_win());
            dirs.push("C:\\windows\\system32".into());
            dirs.push("C:\\windows".into());
            if let Some(p) = crate::runtime::env_get("PATH") {
                dirs.extend(p.split(';').filter(|d| !d.is_empty()).map(String::from));
            }
        }
    }
    dirs.iter()
        .map(|d| format!("{}\\{name}", d.trim_end_matches('\\')))
        .find(|c| is_file(c))
        .map(|c| paths::full_path(&c))
}

crate::shims! { "kernel32";
    fn CreateFileW(name: LPCWSTR, access: DWORD, _share: DWORD, _sec: usize, disposition: DWORD, flags: DWORD, _template: HANDLE) -> HANDLE {
        open_file(&win_namew(name), access, disposition, flags)
    }
    fn CreateFileA(name: LPCSTR, access: DWORD, _share: DWORD, _sec: usize, disposition: DWORD, flags: DWORD, _template: HANDLE) -> HANDLE {
        open_file(&win_name(name), access, disposition, flags)
    }
    fn CreateFile2(name: LPCWSTR, access: DWORD, _share: DWORD, disposition: DWORD, _params: usize) -> HANDLE {
        open_file(&win_namew(name), access, disposition, 0)
    }
    fn ReadFile(h: HANDLE, buf: *mut u8, n: DWORD, read: *mut DWORD, ov: *mut OVERLAPPED) -> BOOL {
        read_file(h, buf, n, read, ov)
    }
    fn WriteFile(h: HANDLE, buf: *const u8, n: DWORD, written: *mut DWORD, ov: *mut OVERLAPPED) -> BOOL {
        write_file(h, buf, n, written, ov)
    }
    fn ReadFileEx(_h: HANDLE, _buf: *mut u8, _n: DWORD, _ov: *mut OVERLAPPED, _cb: usize) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn GetOverlappedResult(_h: HANDLE, ov: *const OVERLAPPED, bytes: *mut DWORD, _wait: BOOL) -> BOOL {
        if !bytes.is_null() && !ov.is_null() {
            *bytes = (*ov).internal_high as DWORD;
        }
        TRUE
    }
    fn GetOverlappedResultEx(h: HANDLE, ov: *const OVERLAPPED, bytes: *mut DWORD, _ms: DWORD, _alertable: BOOL) -> BOOL {
        GetOverlappedResult(h, ov, bytes, TRUE)
    }
    fn CancelIo(_h: HANDLE) -> BOOL {
        TRUE
    }
    fn CancelIoEx(_h: HANDLE, _ov: *const OVERLAPPED) -> BOOL {
        TRUE
    }
    fn SetFilePointer(h: HANDLE, lo: i32, hi: *mut i32, method: DWORD) -> DWORD {
        let off = if hi.is_null() { lo as i64 } else { ((*hi as i64) << 32) | (lo as u32 as i64) };
        match file_pos_op(h, off, method) {
            Some(p) => {
                if !hi.is_null() {
                    *hi = (p >> 32) as i32;
                }
                set_last_error(ERROR_SUCCESS);
                p as DWORD
            }
            None => 0xffff_ffff,
        }
    }
    fn SetFilePointerEx(h: HANDLE, dist: i64, newpos: *mut i64, method: DWORD) -> BOOL {
        match file_pos_op(h, dist, method) {
            Some(p) => {
                if !newpos.is_null() {
                    *newpos = p;
                }
                TRUE
            }
            None => FALSE,
        }
    }
    fn GetFileSizeEx(h: HANDLE, out: *mut i64) -> BOOL {
        match file_size(h) {
            Some(s) => {
                *out = s as i64;
                TRUE
            }
            None => FALSE,
        }
    }
    fn GetFileSize(h: HANDLE, high: *mut DWORD) -> DWORD {
        match file_size(h) {
            Some(s) => {
                if !high.is_null() {
                    *high = (s >> 32) as DWORD;
                }
                set_last_error(ERROR_SUCCESS);
                s as DWORD
            }
            None => 0xffff_ffff,
        }
    }
    fn GetFileType(h: HANDLE) -> DWORD {
        match handles::get(h).as_deref() {
            Some(Kind::File(f)) => {
                if f.is_tty {
                    FILE_TYPE_CHAR
                } else if f.is_pipe {
                    FILE_TYPE_PIPE
                } else {
                    FILE_TYPE_DISK
                }
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FILE_TYPE_UNKNOWN
            }
        }
    }
    fn SetEndOfFile(h: HANDLE) -> BOOL {
        with_file(h, |f| {
            let pos = libc::lseek(f.fd(), 0, libc::SEEK_CUR);
            if pos >= 0 && libc::ftruncate(f.fd(), pos) == 0 {
                TRUE
            } else {
                set_last_error(errno_to_win(errno()));
                FALSE
            }
        })
        .unwrap_or(FALSE)
    }
    fn SetFileValidData(_h: HANDLE, _len: i64) -> BOOL {
        TRUE
    }
    fn FlushFileBuffers(h: HANDLE) -> BOOL {
        with_file(h, |f| {
            if !f.is_tty && !f.is_pipe {
                libc::fsync(f.fd());
            }
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn LockFile(_h: HANDLE, _a: DWORD, _b: DWORD, _c: DWORD, _d: DWORD) -> BOOL {
        TRUE
    }
    fn LockFileEx(_h: HANDLE, _flags: DWORD, _r: DWORD, _lo: DWORD, _hi: DWORD, _ov: *mut OVERLAPPED) -> BOOL {
        TRUE
    }
    fn UnlockFile(_h: HANDLE, _a: DWORD, _b: DWORD, _c: DWORD, _d: DWORD) -> BOOL {
        TRUE
    }
    fn UnlockFileEx(_h: HANDLE, _r: DWORD, _lo: DWORD, _hi: DWORD, _ov: *mut OVERLAPPED) -> BOOL {
        TRUE
    }
    fn GetFileInformationByHandle(h: HANDLE, out: *mut BY_HANDLE_FILE_INFORMATION) -> BOOL {
        with_file(h, |f| {
            let mut st: libc::stat = std::mem::zeroed();
            if libc::fstat(f.fd(), &mut st) != 0 {
                set_last_error(errno_to_win(errno()));
                return FALSE;
            }
            let attrs = if st.st_mode & libc::S_IFMT == libc::S_IFDIR { FILE_ATTRIBUTE_DIRECTORY } else if st.st_mode & 0o222 == 0 { FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_ARCHIVE } else { FILE_ATTRIBUTE_ARCHIVE };
            *out = BY_HANDLE_FILE_INFORMATION {
                attributes: attrs,
                creation_time: unix_to_filetime(st.st_ctime, st.st_ctime_nsec),
                last_access_time: unix_to_filetime(st.st_atime, st.st_atime_nsec),
                last_write_time: unix_to_filetime(st.st_mtime, st.st_mtime_nsec),
                volume_serial: st.st_dev as DWORD,
                size_high: (st.st_size >> 32) as DWORD,
                size_low: st.st_size as DWORD,
                nlinks: st.st_nlink as DWORD,
                index_high: (st.st_ino >> 32) as DWORD,
                index_low: st.st_ino as DWORD,
            };
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn GetFileInformationByHandleEx(h: HANDLE, class: i32, buf: *mut u8, size: DWORD) -> BOOL {
        with_file(h, |f| {
            let mut st: libc::stat = std::mem::zeroed();
            if libc::fstat(f.fd(), &mut st) != 0 {
                set_last_error(errno_to_win(errno()));
                return FALSE;
            }
            let is_dir = st.st_mode & libc::S_IFMT == libc::S_IFDIR;
            let attrs = if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_ARCHIVE };
            match class {
                0 if size >= 40 => {
                    let t = |s: i64, n: i64| unix_to_filetime(s, n).to_u64() as i64;
                    let p = buf as *mut i64;
                    *p = t(st.st_ctime, st.st_ctime_nsec);
                    *p.add(1) = t(st.st_atime, st.st_atime_nsec);
                    *p.add(2) = t(st.st_mtime, st.st_mtime_nsec);
                    *p.add(3) = t(st.st_ctime, st.st_ctime_nsec);
                    *(buf.add(32) as *mut u32) = attrs;
                    *(buf.add(36) as *mut u32) = 0;
                    TRUE
                }
                1 if size >= 24 => {
                    let p = buf as *mut i64;
                    *p = st.st_blocks as i64 * 512;
                    *p.add(1) = st.st_size as i64;
                    *(buf.add(16) as *mut u32) = st.st_nlink as u32;
                    *buf.add(20) = 0;
                    *buf.add(21) = is_dir as u8;
                    TRUE
                }
                9 if size >= 8 => {
                    *(buf as *mut u32) = attrs;
                    *(buf.add(4) as *mut u32) = 0;
                    TRUE
                }
                _ => {
                    set_last_error(ERROR_INVALID_PARAMETER);
                    FALSE
                }
            }
        })
        .unwrap_or(FALSE)
    }
    fn SetFileInformationByHandle(h: HANDLE, class: i32, info: *const u8, size: DWORD) -> BOOL {
        set_info_by_handle(h, class, info, size)
    }
    fn GetFileTime(h: HANDLE, c: *mut FILETIME, a: *mut FILETIME, w: *mut FILETIME) -> BOOL {
        with_file(h, |f| {
            let mut st: libc::stat = std::mem::zeroed();
            if libc::fstat(f.fd(), &mut st) != 0 {
                set_last_error(errno_to_win(errno()));
                return FALSE;
            }
            if !c.is_null() {
                *c = unix_to_filetime(st.st_ctime, st.st_ctime_nsec);
            }
            if !a.is_null() {
                *a = unix_to_filetime(st.st_atime, st.st_atime_nsec);
            }
            if !w.is_null() {
                *w = unix_to_filetime(st.st_mtime, st.st_mtime_nsec);
            }
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn SetFileTime(h: HANDLE, c: *const FILETIME, a: *const FILETIME, w: *const FILETIME) -> BOOL {
        set_times(h, c, a, w)
    }
    fn GetFinalPathNameByHandleW(h: HANDLE, buf: LPWSTR, size: DWORD, _flags: DWORD) -> DWORD {
        let p = with_file(h, |f| fd_path(f)).flatten();
        match p {
            Some(p) => return_w(buf, size, &format!("\\\\?\\{}", paths::host_to_win(&p))),
            None => {
                set_last_error(ERROR_INVALID_HANDLE);
                0
            }
        }
    }
    fn DeviceIoControl(_h: HANDLE, _code: DWORD, _inb: usize, _inn: DWORD, _outb: usize, _outn: DWORD, _ret: *mut DWORD, _ov: usize) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn GetStdHandle(which: DWORD) -> HANDLE {
        handles::std_handle(which)
    }
    fn SetStdHandle(which: DWORD, h: HANDLE) -> BOOL {
        handles::set_std_handle(which, h) as BOOL
    }
    fn SetHandleCount(n: UINT) -> UINT {
        n
    }

    fn CreateDirectoryW(name: LPCWSTR, _sec: usize) -> BOOL {
        let h = paths::resolve(&win_namew(name));
        status(std::fs::create_dir(&h), &h)
    }
    fn CreateDirectoryA(name: LPCSTR, _sec: usize) -> BOOL {
        let h = paths::resolve(&win_name(name));
        status(std::fs::create_dir(&h), &h)
    }
    fn CreateDirectoryExW(_template: LPCWSTR, name: LPCWSTR, _sec: usize) -> BOOL {
        CreateDirectoryW(name, 0)
    }
    fn RemoveDirectoryW(name: LPCWSTR) -> BOOL {
        let h = paths::resolve(&win_namew(name));
        status(std::fs::remove_dir(&h), &h)
    }
    fn RemoveDirectoryA(name: LPCSTR) -> BOOL {
        let h = paths::resolve(&win_name(name));
        status(std::fs::remove_dir(&h), &h)
    }
    fn DeleteFileW(name: LPCWSTR) -> BOOL {
        let h = paths::resolve(&win_namew(name));
        if h.is_dir() {
            set_last_error(ERROR_ACCESS_DENIED);
            return FALSE;
        }
        status(std::fs::remove_file(&h), &h)
    }
    fn DeleteFileA(name: LPCSTR) -> BOOL {
        let h = paths::resolve(&win_name(name));
        if h.is_dir() {
            set_last_error(ERROR_ACCESS_DENIED);
            return FALSE;
        }
        status(std::fs::remove_file(&h), &h)
    }
    fn MoveFileW(old: LPCWSTR, new: LPCWSTR) -> BOOL {
        move_file(&win_namew(old), &win_namew(new), false)
    }
    fn MoveFileA(old: LPCSTR, new: LPCSTR) -> BOOL {
        move_file(&win_name(old), &win_name(new), false)
    }
    fn MoveFileExW(old: LPCWSTR, new: LPCWSTR, flags: DWORD) -> BOOL {
        move_file(&win_namew(old), &win_namew(new), flags & 1 != 0)
    }
    fn MoveFileExA(old: LPCSTR, new: LPCSTR, flags: DWORD) -> BOOL {
        move_file(&win_name(old), &win_name(new), flags & 1 != 0)
    }
    fn MoveFileWithProgressW(old: LPCWSTR, new: LPCWSTR, _cb: usize, _data: usize, flags: DWORD) -> BOOL {
        move_file(&win_namew(old), &win_namew(new), flags & 1 != 0)
    }
    fn CopyFileW(old: LPCWSTR, new: LPCWSTR, fail_if_exists: BOOL) -> BOOL {
        copy_file(&win_namew(old), &win_namew(new), fail_if_exists != 0)
    }
    fn CopyFileA(old: LPCSTR, new: LPCSTR, fail_if_exists: BOOL) -> BOOL {
        copy_file(&win_name(old), &win_name(new), fail_if_exists != 0)
    }
    fn CopyFileExW(old: LPCWSTR, new: LPCWSTR, _cb: usize, _data: usize, _cancel: *mut BOOL, flags: DWORD) -> BOOL {
        copy_file(&win_namew(old), &win_namew(new), flags & 1 != 0)
    }
    fn CreateHardLinkW(new: LPCWSTR, existing: LPCWSTR, _sec: usize) -> BOOL {
        let (n, e) = (paths::resolve(&win_namew(new)), paths::resolve(&win_namew(existing)));
        status(std::fs::hard_link(&e, &n), &n)
    }
    fn CreateSymbolicLinkW(link: LPCWSTR, target: LPCWSTR, _flags: DWORD) -> u8 {
        let l = paths::resolve(&win_namew(link));
        let t = paths::win_to_host_str(&win_namew(target));
        (status(std::os::unix::fs::symlink(&t, &l), &l) != FALSE) as u8
    }

    fn GetFileAttributesW(name: LPCWSTR) -> DWORD {
        match stat_path(&win_namew(name)) {
            Ok((_, md)) => md_attrs(&md),
            Err(e) => {
                set_last_error(e);
                INVALID_FILE_ATTRIBUTES
            }
        }
    }
    fn GetFileAttributesA(name: LPCSTR) -> DWORD {
        match stat_path(&win_name(name)) {
            Ok((_, md)) => md_attrs(&md),
            Err(e) => {
                set_last_error(e);
                INVALID_FILE_ATTRIBUTES
            }
        }
    }
    fn GetFileAttributesExW(name: LPCWSTR, _level: i32, out: *mut WIN32_FILE_ATTRIBUTE_DATA) -> BOOL {
        attr_data(&win_namew(name), out)
    }
    fn GetFileAttributesExA(name: LPCSTR, _level: i32, out: *mut WIN32_FILE_ATTRIBUTE_DATA) -> BOOL {
        attr_data(&win_name(name), out)
    }
    fn SetFileAttributesW(name: LPCWSTR, attrs: DWORD) -> BOOL {
        let h = paths::resolve(&win_namew(name));
        match std::fs::metadata(&h) {
            Ok(md) => {
                let mut perm = md.permissions();
                let mode = perm.mode();
                perm.set_mode(if attrs & FILE_ATTRIBUTE_READONLY != 0 { mode & !0o222 } else { mode | 0o200 });
                status(std::fs::set_permissions(&h, perm), &h)
            }
            Err(e) => {
                set_last_error(err_from_io(&e, parent_missing(&h)));
                FALSE
            }
        }
    }
    fn SetFileAttributesA(name: LPCSTR, attrs: DWORD) -> BOOL {
        let w = to_wide_nul(&win_name(name));
        SetFileAttributesW(w.as_ptr(), attrs)
    }

    fn FindFirstFileW(pattern: LPCWSTR, out: *mut WIN32_FIND_DATAW) -> HANDLE {
        find_first(&win_namew(pattern), |e| fill_find_w(out, e))
    }
    fn FindFirstFileA(pattern: LPCSTR, out: *mut WIN32_FIND_DATAA) -> HANDLE {
        find_first(&win_name(pattern), |e| fill_find_a(out, e))
    }
    fn FindFirstFileExW(pattern: LPCWSTR, _level: i32, out: *mut WIN32_FIND_DATAW, _op: i32, _filter: usize, _flags: DWORD) -> HANDLE {
        find_first(&win_namew(pattern), |e| fill_find_w(out, e))
    }
    fn FindFirstFileExA(pattern: LPCSTR, _level: i32, out: *mut WIN32_FIND_DATAA, _op: i32, _filter: usize, _flags: DWORD) -> HANDLE {
        find_first(&win_name(pattern), |e| fill_find_a(out, e))
    }
    fn FindNextFileW(h: HANDLE, out: *mut WIN32_FIND_DATAW) -> BOOL {
        find_next(h, |e| fill_find_w(out, e))
    }
    fn FindNextFileA(h: HANDLE, out: *mut WIN32_FIND_DATAA) -> BOOL {
        find_next(h, |e| fill_find_a(out, e))
    }
    fn FindClose(h: HANDLE) -> BOOL {
        match handles::get(h).as_deref() {
            Some(Kind::Find(_)) => {
                handles::close(h);
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }

    fn SearchPathW(path: LPCWSTR, file: LPCWSTR, ext: LPCWSTR, size: DWORD, buf: LPWSTR, filepart: *mut LPWSTR) -> DWORD {
        if file.is_null() {
            set_last_error(ERROR_INVALID_PARAMETER);
            return 0;
        }
        let dir = (!path.is_null()).then(|| win_namew(path));
        let ext = (!ext.is_null()).then(|| win_namew(ext));
        match search_path(dir, &win_namew(file), ext) {
            Some(found) => get_full_path_w(&found, size, buf, filepart),
            None => {
                set_last_error(ERROR_FILE_NOT_FOUND);
                0
            }
        }
    }
    fn SearchPathA(path: LPCSTR, file: LPCSTR, ext: LPCSTR, size: DWORD, buf: LPSTR, filepart: *mut LPSTR) -> DWORD {
        if file.is_null() {
            set_last_error(ERROR_INVALID_PARAMETER);
            return 0;
        }
        let dir = (!path.is_null()).then(|| win_name(path));
        let ext = (!ext.is_null()).then(|| win_name(ext));
        let Some(found) = search_path(dir, &win_name(file), ext) else {
            set_last_error(ERROR_FILE_NOT_FOUND);
            return 0;
        };
        let r = return_a(buf, size, &found);
        if !filepart.is_null() {
            *filepart = match found.rfind('\\') {
                Some(i) if (r as usize) <= found.len() => buf.add(i + 1),
                _ => std::ptr::null_mut(),
            };
        }
        r
    }
    fn GetFullPathNameW(path: LPCWSTR, size: DWORD, buf: LPWSTR, filepart: *mut LPWSTR) -> DWORD {
        get_full_path_w(&win_namew(path), size, buf, filepart)
    }
    fn GetFullPathNameA(path: LPCSTR, size: DWORD, buf: LPSTR, filepart: *mut LPSTR) -> DWORD {
        let full = paths::full_path(&win_name(path));
        let r = return_a(buf, size, &full);
        if !filepart.is_null() {
            *filepart = match full.rfind('\\') {
                Some(i) if i + 1 < full.len() && (r as usize) <= full.len() => buf.add(i + 1),
                _ => std::ptr::null_mut(),
            };
        }
        r
    }
    fn GetLongPathNameW(path: LPCWSTR, buf: LPWSTR, size: DWORD) -> DWORD {
        return_w(buf, size, &win_namew(path))
    }
    fn GetShortPathNameW(path: LPCWSTR, buf: LPWSTR, size: DWORD) -> DWORD {
        return_w(buf, size, &win_namew(path))
    }
    fn GetCurrentDirectoryW(size: DWORD, buf: LPWSTR) -> DWORD {
        get_cwd_w(size, buf)
    }
    fn GetCurrentDirectoryA(size: DWORD, buf: LPSTR) -> DWORD {
        return_a(buf, size, &paths::cwd_win())
    }
    fn SetCurrentDirectoryW(path: LPCWSTR) -> BOOL {
        set_cwd(&win_namew(path))
    }
    fn SetCurrentDirectoryA(path: LPCSTR) -> BOOL {
        set_cwd(&win_name(path))
    }
    fn GetTempFileNameW(dir: LPCWSTR, prefix: LPCWSTR, unique: UINT, buf: LPWSTR) -> UINT {
        let d = win_namew(dir);
        let p = win_namew(prefix);
        let pfx: String = p.chars().take(3).collect();
        let mut n = if unique != 0 { unique } else { (libc::getpid() as u32) & 0xffff | 1 };
        loop {
            let name = format!("{}{}{}{:X}.tmp", d, if d.ends_with('\\') { "" } else { "\\" }, pfx, n);
            if unique != 0 {
                copy_w(buf, MAX_PATH, &to_wide(&name));
                return unique;
            }
            let host = paths::resolve(&name);
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&host) {
                Ok(_) => {
                    copy_w(buf, MAX_PATH, &to_wide(&name));
                    return n;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n = n.wrapping_add(1) & 0xffff | 1,
                Err(e) => {
                    set_last_error(err_from_io(&e, parent_missing(&host)));
                    return 0;
                }
            }
        }
    }
    fn GetVolumePathNameW(path: LPCWSTR, buf: LPWSTR, size: DWORD) -> BOOL {
        let full = paths::full_path(&win_namew(path));
        let root = format!("{}\\", &full[..2.min(full.len())]);
        (return_w(buf, size, &root) <= size) as BOOL
    }
    fn GetVolumeInformationW(_root: LPCWSTR, vol: LPWSTR, vol_size: DWORD, serial: *mut DWORD, max_comp: *mut DWORD, flags: *mut DWORD, fs: LPWSTR, fs_size: DWORD) -> BOOL {
        if !vol.is_null() {
            copy_w(vol, vol_size as usize, &to_wide("nle"));
        }
        if !serial.is_null() {
            *serial = 0x1234_5678;
        }
        if !max_comp.is_null() {
            *max_comp = 255;
        }
        if !flags.is_null() {
            *flags = 0x0000_00ff;
        }
        if !fs.is_null() {
            copy_w(fs, fs_size as usize, &to_wide("NTFS"));
        }
        TRUE
    }
    fn GetDiskFreeSpaceExW(dir: LPCWSTR, avail: *mut u64, total: *mut u64, free: *mut u64) -> BOOL {
        let host = if dir.is_null() { PathBuf::from(".") } else { paths::resolve(&win_namew(dir)) };
        let c = cpath(&host);
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) != 0 {
            set_last_error(errno_to_win(errno()));
            return FALSE;
        }
        let bs = s.f_frsize as u64;
        if !avail.is_null() {
            *avail = s.f_bavail as u64 * bs;
        }
        if !total.is_null() {
            *total = s.f_blocks as u64 * bs;
        }
        if !free.is_null() {
            *free = s.f_bfree as u64 * bs;
        }
        TRUE
    }

    fn CreatePipe(read: *mut HANDLE, write: *mut HANDLE, _attrs: usize, _size: DWORD) -> BOOL {
        let mut fds = [0i32; 2];
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            set_last_error(errno_to_win(errno()));
            return FALSE;
        }
        *read = handles::insert(Kind::File(FileObj::new(fds[0], true)));
        *write = handles::insert(Kind::File(FileObj::new(fds[1], true)));
        TRUE
    }
    fn PeekNamedPipe(h: HANDLE, _buf: *mut u8, _n: DWORD, read: *mut DWORD, avail: *mut DWORD, left: *mut DWORD) -> BOOL {
        with_file(h, |f| {
            let mut n: libc::c_int = 0;
            libc::ioctl(f.fd(), libc::FIONREAD, &mut n);
            if !read.is_null() {
                *read = 0;
            }
            if !avail.is_null() {
                *avail = n as DWORD;
            }
            if !left.is_null() {
                *left = 0;
            }
            TRUE
        })
        .unwrap_or(FALSE)
    }
}

pub fn host_fd_of(h: HANDLE) -> Option<i32> {
    match handles::get(h).as_deref() {
        Some(Kind::File(f)) => Some(f.fd.load(Ordering::Relaxed)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn wildcards() {
        assert!(wildcard(&w("*.txt"), &w("File.TXT")));
        assert!(wildcard(&w("a?c"), &w("abc")));
        assert!(!wildcard(&w("a?c"), &w("abbc")));
        assert!(wildcard(&w("*"), &w("")));
        assert!(!wildcard(&w("*.txt"), &w("file.bin")));
    }

    #[test]
    fn filetime_epoch() {
        assert_eq!(unix_to_filetime(0, 0).to_u64(), 116_444_736_000_000_000);
    }
}
