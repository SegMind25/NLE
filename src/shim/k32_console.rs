//! kernel32: console. Maps the Win32 console API onto a Linux terminal (ANSI sequences, termios,
//! SIGINT delivery to `SetConsoleCtrlHandler` handlers).

use crate::runtime::handles::{self, FileObj, Kind};
use crate::runtime::strings::*;
use crate::runtime::teb;
use crate::win::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

const ENABLE_LINE_INPUT: DWORD = 0x2;
const ENABLE_ECHO_INPUT: DWORD = 0x4;
const DEFAULT_INPUT_MODE: DWORD = 0x1 | 0x2 | 0x4 | 0x10 | 0x20 | 0x40;
const DEFAULT_OUTPUT_MODE: DWORD = 0x1 | 0x2;

type CtrlHandler = unsafe extern "win64" fn(DWORD) -> BOOL;

static ORIG_TERMIOS: Mutex<Option<libc::termios>> = Mutex::new(None);
static INPUT_PENDING: Mutex<(Vec<u8>, Vec<u16>)> = Mutex::new((Vec::new(), Vec::new()));
static HANDLERS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static CTRL_THREAD: AtomicBool = AtomicBool::new(false);
static IGNORE_CTRL_C: AtomicBool = AtomicBool::new(false);
static CTRL_PIPE: Mutex<[i32; 2]> = Mutex::new([-1, -1]);

/// Restores the terminal state changed through `SetConsoleMode`. Called on process exit.
pub fn restore_terminal() {
    if let Some(t) = ORIG_TERMIOS.lock().unwrap().take() {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) };
    }
}

fn term_size(fd: i32) -> Option<(i16, i16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0 {
        Some((ws.ws_col as i16, ws.ws_row.max(1) as i16))
    } else {
        None
    }
}

fn console_fd(h: HANDLE) -> Option<(i32, bool)> {
    match handles::get(h).as_deref() {
        Some(Kind::File(f)) => Some((f.fd(), f.is_tty)),
        _ => None,
    }
}

fn with_console<R>(h: HANDLE, f: impl FnOnce(&FileObj) -> R) -> Option<R> {
    match handles::get(h).as_deref() {
        Some(Kind::File(file)) if file.is_tty => Some(f(file)),
        _ => {
            set_last_error(ERROR_INVALID_HANDLE);
            None
        }
    }
}

unsafe fn put_bytes(fd: i32, b: &[u8]) {
    let mut done = 0;
    while done < b.len() {
        let r = libc::write(fd, b.as_ptr().add(done) as *const libc::c_void, b.len() - done);
        if r < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }
        done += r as usize;
    }
}

fn sgr_for(attr: u32) -> String {
    let ansi = |bits: u32| ((bits & 4 != 0) as u32) | (((bits & 2 != 0) as u32) << 1) | (((bits & 1 != 0) as u32) << 2);
    let fg = ansi(attr & 7);
    let bg = ansi((attr >> 4) & 7);
    let fg_code = if attr & 8 != 0 { 90 + fg } else { 30 + fg };
    let bg_code = if attr & 0x80 != 0 { 100 + bg } else { 40 + bg };
    if attr & 0xff == 7 {
        "\x1b[0m".to_string()
    } else {
        format!("\x1b[0;{fg_code};{bg_code}m")
    }
}

unsafe fn write_console_utf16(h: HANDLE, w: &[u16], written: *mut DWORD) -> BOOL {
    let Some((fd, _)) = console_fd(h) else {
        set_last_error(ERROR_INVALID_HANDLE);
        return FALSE;
    };
    let s = String::from_utf16_lossy(w);
    put_bytes(fd, s.as_bytes());
    if !written.is_null() {
        *written = w.len() as DWORD;
    }
    TRUE
}

unsafe fn read_console_utf16(h: HANDLE, out: *mut u16, n: DWORD, read: *mut DWORD) -> BOOL {
    let Some((fd, _)) = console_fd(h) else {
        set_last_error(ERROR_INVALID_HANDLE);
        return FALSE;
    };
    let mut guard = INPUT_PENDING.lock().unwrap();
    if guard.1.is_empty() {
        loop {
            let mut tmp = [0u8; 4096];
            let r = libc::read(fd, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len());
            if r < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            if r <= 0 {
                if !read.is_null() {
                    *read = 0;
                }
                return TRUE;
            }
            guard.0.extend_from_slice(&tmp[..r as usize]);
            let valid = match std::str::from_utf8(&guard.0) {
                Ok(_) => guard.0.len(),
                Err(e) => e.valid_up_to(),
            };
            if valid > 0 {
                let text: String = String::from_utf8_lossy(&guard.0[..valid]).into_owned();
                guard.0.drain(..valid);
                guard.1 = text.encode_utf16().collect();
                break;
            }
            if guard.0.len() >= 4 {
                guard.0.clear();
            }
        }
    }
    let take = guard.1.len().min(n as usize);
    std::ptr::copy_nonoverlapping(guard.1.as_ptr(), out, take);
    guard.1.drain(..take);
    if !read.is_null() {
        *read = take as DWORD;
    }
    TRUE
}

extern "C" fn on_sigint(_sig: libc::c_int) {
    let fd = CTRL_PIPE.try_lock().map(|p| p[1]).unwrap_or(-1);
    if fd >= 0 {
        unsafe { libc::write(fd, [0u8].as_ptr() as *const libc::c_void, 1) };
    }
}

fn ensure_ctrl_thread() {
    if CTRL_THREAD.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return;
    }
    *CTRL_PIPE.lock().unwrap() = fds;
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as *const () as usize);
    }
    let rfd = fds[0];
    let _ = std::thread::Builder::new().name("nle-ctrl".into()).spawn(move || unsafe {
        let teb_ptr = teb::setup_thread();
        loop {
            let mut b = [0u8; 1];
            if libc::read(rfd, b.as_mut_ptr() as *mut libc::c_void, 1) <= 0 {
                break;
            }
            if IGNORE_CTRL_C.load(Ordering::SeqCst) {
                continue;
            }
            let handlers: Vec<usize> = HANDLERS.lock().unwrap().iter().rev().copied().collect();
            let mut handled = false;
            for h in handlers {
                let f: CtrlHandler = std::mem::transmute(h);
                if f(0) != 0 {
                    handled = true;
                    break;
                }
            }
            if !handled {
                crate::loader::exit_process(0xc000_013a);
            }
        }
        teb::teardown_thread(teb_ptr);
    });
}

fn apply_input_mode(mode: DWORD) {
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) != 0 {
            return;
        }
        {
            let mut orig = ORIG_TERMIOS.lock().unwrap();
            if orig.is_none() {
                *orig = Some(t);
            }
        }
        if mode & ENABLE_LINE_INPUT != 0 {
            t.c_lflag |= libc::ICANON;
        } else {
            t.c_lflag &= !libc::ICANON;
            t.c_cc[libc::VMIN] = 1;
            t.c_cc[libc::VTIME] = 0;
        }
        if mode & ENABLE_ECHO_INPUT != 0 {
            t.c_lflag |= libc::ECHO;
        } else {
            t.c_lflag &= !libc::ECHO;
        }
        libc::tcsetattr(0, libc::TCSANOW, &t);
    }
}

crate::shims! { "kernel32";
    fn WriteConsoleW(h: HANDLE, buf: *const u16, n: DWORD, written: *mut DWORD, _reserved: usize) -> BOOL {
        write_console_utf16(h, std::slice::from_raw_parts(buf, n as usize), written)
    }
    fn WriteConsoleA(h: HANDLE, buf: *const u8, n: DWORD, written: *mut DWORD, _reserved: usize) -> BOOL {
        let Some((fd, _)) = console_fd(h) else {
            set_last_error(ERROR_INVALID_HANDLE);
            return FALSE;
        };
        put_bytes(fd, std::slice::from_raw_parts(buf, n as usize));
        if !written.is_null() {
            *written = n;
        }
        TRUE
    }
    fn ReadConsoleW(h: HANDLE, buf: *mut u16, n: DWORD, read: *mut DWORD, _ctl: usize) -> BOOL {
        read_console_utf16(h, buf, n, read)
    }
    fn ReadConsoleA(h: HANDLE, buf: *mut u8, n: DWORD, read: *mut DWORD, _ctl: usize) -> BOOL {
        let Some((fd, _)) = console_fd(h) else {
            set_last_error(ERROR_INVALID_HANDLE);
            return FALSE;
        };
        let r = libc::read(fd, buf as *mut libc::c_void, n as usize);
        if r < 0 {
            set_last_error_from_errno();
            return FALSE;
        }
        if !read.is_null() {
            *read = r as DWORD;
        }
        TRUE
    }
    fn GetConsoleMode(h: HANDLE, mode: *mut DWORD) -> BOOL {
        match with_console(h, |f| {
            let m = f.console_mode.load(Ordering::SeqCst);
            if m != 0 {
                m
            } else if f.fd() == 0 {
                DEFAULT_INPUT_MODE
            } else {
                DEFAULT_OUTPUT_MODE
            }
        }) {
            Some(m) => {
                if !mode.is_null() {
                    *mode = m;
                }
                TRUE
            }
            None => FALSE,
        }
    }
    fn SetConsoleMode(h: HANDLE, mode: DWORD) -> BOOL {
        with_console(h, |f| {
            f.console_mode.store(mode | 0x8000_0000, Ordering::SeqCst);
            if f.fd() == 0 {
                apply_input_mode(mode);
            }
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn GetConsoleScreenBufferInfo(h: HANDLE, info: *mut CONSOLE_SCREEN_BUFFER_INFO) -> BOOL {
        with_console(h, |f| {
            let (cols, rows) = term_size(f.fd()).unwrap_or((80, 25));
            *info = CONSOLE_SCREEN_BUFFER_INFO {
                size: COORD { x: cols, y: rows },
                cursor: COORD { x: 0, y: 0 },
                attributes: f.console_attr.load(Ordering::SeqCst) as WORD,
                window: SMALL_RECT { left: 0, top: 0, right: cols - 1, bottom: rows - 1 },
                max_window_size: COORD { x: cols, y: rows },
            };
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn GetLargestConsoleWindowSize(h: HANDLE) -> u32 {
        let (cols, rows) = console_fd(h).and_then(|(fd, _)| term_size(fd)).unwrap_or((80, 25));
        (rows as u32) << 16 | cols as u32
    }
    fn SetConsoleTextAttribute(h: HANDLE, attr: WORD) -> BOOL {
        with_console(h, |f| {
            f.console_attr.store(attr as u32, Ordering::SeqCst);
            put_bytes(f.fd(), sgr_for(attr as u32).as_bytes());
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn SetConsoleCursorPosition(h: HANDLE, pos: u32) -> BOOL {
        with_console(h, |f| {
            let (x, y) = ((pos & 0xffff) as i16, (pos >> 16) as i16);
            put_bytes(f.fd(), format!("\x1b[{};{}H", y + 1, x + 1).as_bytes());
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn GetConsoleCursorInfo(h: HANDLE, info: *mut u32) -> BOOL {
        with_console(h, |_| {
            *info = 25;
            *info.add(1) = 1;
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn SetConsoleCursorInfo(h: HANDLE, info: *const u32) -> BOOL {
        with_console(h, |f| {
            put_bytes(f.fd(), if *info.add(1) != 0 { b"\x1b[?25h" } else { b"\x1b[?25l" });
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn FillConsoleOutputCharacterW(h: HANDLE, ch: WCHAR, len: DWORD, pos: u32, written: *mut DWORD) -> BOOL {
        with_console(h, |f| {
            let (cols, rows) = term_size(f.fd()).unwrap_or((80, 25));
            if ch == b' ' as u16 && pos == 0 && len as i64 >= cols as i64 * rows as i64 {
                put_bytes(f.fd(), b"\x1b[2J\x1b[H");
            }
            if !written.is_null() {
                *written = len;
            }
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn FillConsoleOutputAttribute(h: HANDLE, _attr: WORD, len: DWORD, _pos: u32, written: *mut DWORD) -> BOOL {
        with_console(h, |_| {
            if !written.is_null() {
                *written = len;
            }
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn SetConsoleScreenBufferSize(_h: HANDLE, _size: u32) -> BOOL {
        TRUE
    }
    fn SetConsoleWindowInfo(_h: HANDLE, _abs: BOOL, _rect: *const SMALL_RECT) -> BOOL {
        TRUE
    }
    fn SetConsoleTitleW(title: LPCWSTR) -> BOOL {
        if libc::isatty(1) == 1 {
            put_bytes(1, format!("\x1b]0;{}\x07", w_ptr_to_string(title)).as_bytes());
        }
        TRUE
    }
    fn SetConsoleTitleA(title: LPCSTR) -> BOOL {
        if libc::isatty(1) == 1 {
            put_bytes(1, format!("\x1b]0;{}\x07", a_to_string(title)).as_bytes());
        }
        TRUE
    }
    fn GetConsoleTitleW(buf: LPWSTR, size: DWORD) -> DWORD {
        if size > 0 {
            *buf = 0;
        }
        0
    }
    fn GetConsoleCP() -> UINT {
        65001
    }
    fn GetConsoleOutputCP() -> UINT {
        65001
    }
    fn SetConsoleCP(_cp: UINT) -> BOOL {
        TRUE
    }
    fn SetConsoleOutputCP(_cp: UINT) -> BOOL {
        TRUE
    }
    fn GetConsoleWindow() -> HANDLE {
        0
    }
    fn AllocConsole() -> BOOL {
        TRUE
    }
    fn FreeConsole() -> BOOL {
        TRUE
    }
    fn AttachConsole(_pid: DWORD) -> BOOL {
        TRUE
    }
    fn GetConsoleProcessList(list: *mut DWORD, n: DWORD) -> DWORD {
        if n >= 1 && !list.is_null() {
            *list = libc::getpid() as DWORD;
        }
        1
    }
    fn FlushConsoleInputBuffer(h: HANDLE) -> BOOL {
        with_console(h, |f| {
            libc::tcflush(f.fd(), libc::TCIFLUSH);
            INPUT_PENDING.lock().unwrap().1.clear();
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn GetNumberOfConsoleInputEvents(h: HANDLE, n: *mut DWORD) -> BOOL {
        with_console(h, |f| {
            let mut avail: libc::c_int = 0;
            libc::ioctl(f.fd(), libc::FIONREAD, &mut avail);
            *n = avail as DWORD;
            TRUE
        })
        .unwrap_or(FALSE)
    }
    fn SetConsoleCtrlHandler(handler: usize, add: BOOL) -> BOOL {
        ensure_ctrl_thread();
        if handler == 0 {
            IGNORE_CTRL_C.store(add != 0, Ordering::SeqCst);
            return TRUE;
        }
        let mut h = HANDLERS.lock().unwrap();
        if add != 0 {
            h.push(handler);
            TRUE
        } else if let Some(i) = h.iter().position(|&x| x == handler) {
            h.remove(i);
            TRUE
        } else {
            set_last_error(ERROR_INVALID_PARAMETER);
            FALSE
        }
    }
    fn GenerateConsoleCtrlEvent(event: DWORD, _group: DWORD) -> BOOL {
        match event {
            0 | 1 => {
                libc::raise(libc::SIGINT);
                TRUE
            }
            _ => FALSE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_mapping() {
        assert_eq!(sgr_for(7), "\x1b[0m");
        // bright red on black
        assert_eq!(sgr_for(0x0c), "\x1b[0;91;40m");
        // blue on white background
        assert_eq!(sgr_for(0x71), "\x1b[0;34;47m");
    }
}
