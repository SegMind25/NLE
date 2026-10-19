//! The window system: Win32 windows, messages and GDI drawing, built on a software renderer and
//! a small display backend (X11, or a scriptable headless one).
//!
//! * `surface` and `font` draw pixels (lines, shapes, text, blits) into memory.
//! * `gdi` holds the GDI object model: device contexts, pens, brushes, fonts, bitmaps.
//! * `wm` is the window manager: classes, windows, painting, focus, hit testing.
//! * `msg` is the message pump: queues, timers, dispatch, and the default window procedure.
//! * `controls` and `msgbox` are the built-in BUTTON / STATIC / EDIT classes and MessageBox.
//! * `backend`, `x11` and `headless` get finished pixels on screen and bring input back.
//!
//! One lock (`lock()`) guards all of it. It is never held while guest code runs or while waiting
//! for input, so window procedures can call straight back into the API.

pub mod api;
pub mod backend;
pub mod controls;
pub mod font;
pub mod gdi;
pub mod headless;
pub mod msg;
pub mod msgbox;
pub mod png;
pub mod surface;
pub mod wm;
pub mod x11;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

// ---- Win32 constants used across the module ------------------------------------------------------
pub const WM_NULL: u32 = 0x0000;
pub const WM_CREATE: u32 = 0x0001;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_MOVE: u32 = 0x0003;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_ACTIVATE: u32 = 0x0006;
pub const WM_SETFOCUS: u32 = 0x0007;
pub const WM_KILLFOCUS: u32 = 0x0008;
pub const WM_ENABLE: u32 = 0x000A;
pub const WM_SETREDRAW: u32 = 0x000B;
pub const WM_SETTEXT: u32 = 0x000C;
pub const WM_GETTEXT: u32 = 0x000D;
pub const WM_GETTEXTLENGTH: u32 = 0x000E;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_QUERYENDSESSION: u32 = 0x0011;
pub const WM_QUIT: u32 = 0x0012;
pub const WM_ERASEBKGND: u32 = 0x0014;
pub const WM_SYSCOLORCHANGE: u32 = 0x0015;
pub const WM_SHOWWINDOW: u32 = 0x0018;
pub const WM_ACTIVATEAPP: u32 = 0x001C;
pub const WM_SETCURSOR: u32 = 0x0020;
pub const WM_MOUSEACTIVATE: u32 = 0x0021;
pub const WM_GETMINMAXINFO: u32 = 0x0024;
pub const WM_WINDOWPOSCHANGING: u32 = 0x0046;
pub const WM_WINDOWPOSCHANGED: u32 = 0x0047;
pub const WM_NOTIFY: u32 = 0x004E;
pub const WM_GETICON: u32 = 0x007F;
pub const WM_SETICON: u32 = 0x0080;
pub const WM_NCCREATE: u32 = 0x0081;
pub const WM_NCDESTROY: u32 = 0x0082;
pub const WM_NCCALCSIZE: u32 = 0x0083;
pub const WM_NCHITTEST: u32 = 0x0084;
pub const WM_NCPAINT: u32 = 0x0085;
pub const WM_NCACTIVATE: u32 = 0x0086;
pub const WM_GETDLGCODE: u32 = 0x0087;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_CHAR: u32 = 0x0102;
pub const WM_DEADCHAR: u32 = 0x0103;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const WM_SYSCHAR: u32 = 0x0106;
pub const WM_INITDIALOG: u32 = 0x0110;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_SYSCOMMAND: u32 = 0x0112;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_HSCROLL: u32 = 0x0114;
pub const WM_VSCROLL: u32 = 0x0115;
pub const WM_CTLCOLORMSGBOX: u32 = 0x0132;
pub const WM_CTLCOLOREDIT: u32 = 0x0133;
pub const WM_CTLCOLORBTN: u32 = 0x0135;
pub const WM_CTLCOLORDLG: u32 = 0x0136;
pub const WM_CTLCOLORSTATIC: u32 = 0x0138;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_RBUTTONDBLCLK: u32 = 0x0206;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MBUTTONDBLCLK: u32 = 0x0209;
pub const WM_MOUSEWHEEL: u32 = 0x020A;
pub const WM_CAPTURECHANGED: u32 = 0x0215;
pub const WM_USER: u32 = 0x0400;
pub const WM_APP: u32 = 0x8000;

// window styles
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_MINIMIZE: u32 = 0x2000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WS_DISABLED: u32 = 0x0800_0000;
pub const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
pub const WS_MAXIMIZE: u32 = 0x0100_0000;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_BORDER: u32 = 0x0080_0000;
pub const WS_DLGFRAME: u32 = 0x0040_0000;
pub const WS_VSCROLL: u32 = 0x0020_0000;
pub const WS_HSCROLL: u32 = 0x0010_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_THICKFRAME: u32 = 0x0004_0000;
pub const WS_GROUP: u32 = 0x0002_0000;
pub const WS_TABSTOP: u32 = 0x0001_0000;
pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
pub const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
pub const WS_EX_DLGMODALFRAME: u32 = 0x0000_0001;
pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const WS_EX_WINDOWEDGE: u32 = 0x0000_0100;
pub const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;
pub const WS_EX_STATICEDGE: u32 = 0x0002_0000;
pub const CW_USEDEFAULT: i32 = i32::MIN;

pub const SW_HIDE: i32 = 0;
pub const SW_SHOWNORMAL: i32 = 1;
pub const SW_SHOWMINIMIZED: i32 = 2;
pub const SW_SHOWMAXIMIZED: i32 = 3;
pub const SW_SHOWNOACTIVATE: i32 = 4;
pub const SW_SHOW: i32 = 5;
pub const SW_MINIMIZE: i32 = 6;
pub const SW_SHOWMINNOACTIVE: i32 = 7;
pub const SW_SHOWNA: i32 = 8;
pub const SW_RESTORE: i32 = 9;
pub const SW_SHOWDEFAULT: i32 = 10;

// handles live in their own number space, well below 2^31 so 32-bit truncation is harmless
const FIRST_HANDLE: usize = 0x000A_0004;

/// A message as it travels through a queue.
#[derive(Clone, Copy, Debug, Default)]
pub struct Msg {
    pub hwnd: usize,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
    pub time: u32,
    pub pt: (i32, i32),
}

pub struct Gui {
    pub backend: Option<Arc<dyn backend::Backend>>,
    pub backend_failed: bool,
    pub objects: HashMap<usize, gdi::Object>,
    pub windows: HashMap<usize, wm::Wnd>,
    pub classes: HashMap<String, Arc<wm::Class>>,
    pub atoms: HashMap<u16, String>,
    pub next_handle: usize,
    pub next_atom: u16,
    pub threads: HashMap<u32, msg::ThreadState>,
    pub native_to_hwnd: HashMap<u64, usize>,
    pub focus: usize,
    pub active: usize,
    pub capture: usize,
    pub keys: [u8; 256],
    pub toggles: [bool; 256],
    pub pointer: (i32, i32),
    pub buttons: u32,
    pub last_click: Option<(u32, usize, u32, (i32, i32))>,
    pub key_chars: VecDeque<(u32, u16)>,
    pub stock: HashMap<i32, usize>,
    pub started: std::time::Instant,
    pub desktop: usize,
}

impl Gui {
    fn new() -> Gui {
        let mut g = Gui {
            backend: None,
            backend_failed: false,
            objects: HashMap::new(),
            windows: HashMap::new(),
            classes: HashMap::new(),
            atoms: HashMap::new(),
            next_handle: FIRST_HANDLE,
            next_atom: 0xC001,
            threads: HashMap::new(),
            native_to_hwnd: HashMap::new(),
            focus: 0,
            active: 0,
            capture: 0,
            keys: [0; 256],
            toggles: [false; 256],
            pointer: (0, 0),
            buttons: 0,
            last_click: None,
            key_chars: VecDeque::new(),
            stock: HashMap::new(),
            started: std::time::Instant::now(),
            desktop: 0,
        };
        g.desktop = g.alloc_handle();
        gdi::init_stock(&mut g);
        controls::register_builtin_classes(&mut g);
        g
    }

    pub fn alloc_handle(&mut self) -> usize {
        let h = self.next_handle;
        self.next_handle += 4;
        h
    }

    /// Milliseconds since the window system started (what `GetTickCount`-style message times use).
    pub fn tick(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }
}

pub fn lock() -> MutexGuard<'static, Gui> {
    static G: OnceLock<Mutex<Gui>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(Gui::new())).lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs `f` with the window system locked.
pub fn with<R>(f: impl FnOnce(&mut Gui) -> R) -> R {
    f(&mut lock())
}

pub fn make_lparam(lo: i32, hi: i32) -> isize {
    ((lo as u16 as u32) | ((hi as u16 as u32) << 16)) as i32 as isize
}

pub fn loword(v: usize) -> i32 {
    (v & 0xffff) as i32
}

pub fn hiword(v: usize) -> i32 {
    ((v >> 16) & 0xffff) as i32
}

pub fn lo_signed(v: isize) -> i32 {
    (v & 0xffff) as u16 as i16 as i32
}

pub fn hi_signed(v: isize) -> i32 {
    ((v >> 16) & 0xffff) as u16 as i16 as i32
}
