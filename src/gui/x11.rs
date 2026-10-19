//! The X11 display backend. libX11 is loaded at run time (`dlopen`), so NLE has no build- or
//! start-up dependency on X; a machine without it simply cannot open windows.
//!
//! Only top-level windows exist on the X side. Everything inside them is drawn by this process and
//! sent as finished pixels with `XPutImage`; keyboard and mouse events come back through `poll`.

use super::backend::*;
use super::surface::{Rect, Surface};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CString};
use std::sync::Mutex;

type Display = c_void;
type Xid = c_ulong;

macro_rules! lib_fns {
    ($( $name:ident : $ty:ty ),* $(,)?) => {
        #[allow(non_snake_case)]
        struct Lib { $( $name: $ty ),* }
        impl Lib {
            unsafe fn load(h: *mut c_void) -> Result<Lib, String> {
                Ok(Lib { $( $name: {
                    let p = libc::dlsym(h, concat!(stringify!($name), "\0").as_ptr() as *const c_char);
                    if p.is_null() { return Err(format!("libX11 lacks {}", stringify!($name))); }
                    std::mem::transmute::<*mut c_void, $ty>(p)
                } ),* })
            }
        }
    };
}

lib_fns! {
    XOpenDisplay: unsafe extern "C" fn(*const c_char) -> *mut Display,
    XInitThreads: unsafe extern "C" fn() -> c_int,
    XSetErrorHandler: unsafe extern "C" fn(unsafe extern "C" fn(*mut Display, *mut c_void) -> c_int) -> *mut c_void,
    XDefaultScreen: unsafe extern "C" fn(*mut Display) -> c_int,
    XRootWindow: unsafe extern "C" fn(*mut Display, c_int) -> Xid,
    XDisplayWidth: unsafe extern "C" fn(*mut Display, c_int) -> c_int,
    XDisplayHeight: unsafe extern "C" fn(*mut Display, c_int) -> c_int,
    XDefaultDepth: unsafe extern "C" fn(*mut Display, c_int) -> c_int,
    XDefaultVisual: unsafe extern "C" fn(*mut Display, c_int) -> *mut c_void,
    XDefaultGC: unsafe extern "C" fn(*mut Display, c_int) -> *mut c_void,
    XCreateSimpleWindow: unsafe extern "C" fn(*mut Display, Xid, c_int, c_int, c_uint, c_uint, c_uint, c_ulong, c_ulong) -> Xid,
    XDestroyWindow: unsafe extern "C" fn(*mut Display, Xid) -> c_int,
    XMapWindow: unsafe extern "C" fn(*mut Display, Xid) -> c_int,
    XUnmapWindow: unsafe extern "C" fn(*mut Display, Xid) -> c_int,
    XRaiseWindow: unsafe extern "C" fn(*mut Display, Xid) -> c_int,
    XSelectInput: unsafe extern "C" fn(*mut Display, Xid, c_long) -> c_int,
    XStoreName: unsafe extern "C" fn(*mut Display, Xid, *const c_char) -> c_int,
    XInternAtom: unsafe extern "C" fn(*mut Display, *const c_char, c_int) -> Xid,
    XSetWMProtocols: unsafe extern "C" fn(*mut Display, Xid, *mut Xid, c_int) -> c_int,
    XPending: unsafe extern "C" fn(*mut Display) -> c_int,
    XNextEvent: unsafe extern "C" fn(*mut Display, *mut u64) -> c_int,
    XConnectionNumber: unsafe extern "C" fn(*mut Display) -> c_int,
    XFlush: unsafe extern "C" fn(*mut Display) -> c_int,
    XCreateImage: unsafe extern "C" fn(*mut Display, *mut c_void, c_uint, c_int, c_int, *mut c_char, c_uint, c_uint, c_int, c_int) -> *mut c_void,
    XPutImage: unsafe extern "C" fn(*mut Display, Xid, *mut c_void, *mut c_void, c_int, c_int, c_int, c_int, c_uint, c_uint) -> c_int,
    XFree: unsafe extern "C" fn(*mut c_void) -> c_int,
    XMoveResizeWindow: unsafe extern "C" fn(*mut Display, Xid, c_int, c_int, c_uint, c_uint) -> c_int,
    XChangeProperty: unsafe extern "C" fn(*mut Display, Xid, Xid, Xid, c_int, c_int, *const u8, c_int) -> c_int,
    XSetWMNormalHints: unsafe extern "C" fn(*mut Display, Xid, *mut c_void),
    XQueryPointer: unsafe extern "C" fn(*mut Display, Xid, *mut Xid, *mut Xid, *mut c_int, *mut c_int, *mut c_int, *mut c_int, *mut c_uint) -> c_int,
    XLookupString: unsafe extern "C" fn(*mut c_void, *mut c_char, c_int, *mut c_ulong, *mut c_void) -> c_int,
    XLookupKeysym: unsafe extern "C" fn(*mut c_void, c_int) -> c_ulong,
    XSetInputFocus: unsafe extern "C" fn(*mut Display, Xid, c_int, c_ulong) -> c_int,
    XGrabPointer: unsafe extern "C" fn(*mut Display, Xid, c_int, c_uint, c_int, c_int, Xid, Xid, c_ulong) -> c_int,
    XUngrabPointer: unsafe extern "C" fn(*mut Display, c_ulong) -> c_int,
    XCreateFontCursor: unsafe extern "C" fn(*mut Display, c_uint) -> Xid,
    XDefineCursor: unsafe extern "C" fn(*mut Display, Xid, Xid) -> c_int,
    XBell: unsafe extern "C" fn(*mut Display, c_int) -> c_int,
    XTranslateCoordinates: unsafe extern "C" fn(*mut Display, Xid, Xid, c_int, c_int, *mut c_int, *mut c_int, *mut Xid) -> c_int,
    XSetWindowBackgroundPixmap: unsafe extern "C" fn(*mut Display, Xid, Xid) -> c_int,
    XSetTransientForHint: unsafe extern "C" fn(*mut Display, Xid, Xid) -> c_int,
    XChangeWindowAttributes: unsafe extern "C" fn(*mut Display, Xid, c_ulong, *mut u64) -> c_int,
}

unsafe extern "C" fn ignore_x_errors(_: *mut Display, _: *mut c_void) -> c_int {
    0
}

struct WinInfo {
    cursor: Option<Cursor>,
}

pub struct X11 {
    lib: Lib,
    dpy: *mut Display,
    screen: c_int,
    root: Xid,
    depth: c_int,
    visual: *mut c_void,
    gc: *mut c_void,
    fd: c_int,
    wm_delete: Xid,
    wm_protocols: Xid,
    net_wm_name: Xid,
    utf8: Xid,
    motif_hints: Xid,
    net_wm_state: Xid,
    net_wm_above: Xid,
    windows: Mutex<HashMap<u64, WinInfo>>,
    cursors: Mutex<HashMap<u32, Xid>>,
    read_lock: Mutex<()>,
    clipboard: Mutex<String>,
}

// Xlib is initialised with XInitThreads, so its calls may come from any thread.
unsafe impl Send for X11 {}
unsafe impl Sync for X11 {}

const KEY_PRESS: u32 = 2;
const KEY_RELEASE: u32 = 3;
const BUTTON_PRESS: u32 = 4;
const BUTTON_RELEASE: u32 = 5;
const MOTION_NOTIFY: u32 = 6;
const FOCUS_IN: u32 = 9;
const FOCUS_OUT: u32 = 10;
const EXPOSE: u32 = 12;
const CONFIGURE_NOTIFY: u32 = 22;
const CLIENT_MESSAGE: u32 = 33;

const EVENT_MASK: c_long = 1 /*KeyPress*/ | 2 /*KeyRelease*/ | 4 /*ButtonPress*/ | 8 /*ButtonRelease*/ | 1 << 6 /*PointerMotion*/ | 1 << 15 /*Exposure*/ | 1 << 17 /*StructureNotify*/ | 1 << 21 /*FocusChange*/;

fn cursor_shape(c: Cursor) -> c_uint {
    match c {
        Cursor::Arrow => 68,
        Cursor::IBeam => 152,
        Cursor::Wait => 150,
        Cursor::Cross => 34,
        Cursor::Hand => 60,
        Cursor::SizeAll => 52,
        Cursor::SizeWE => 108,
        Cursor::SizeNS => 116,
        Cursor::SizeNWSE => 14,
        Cursor::SizeNESW => 12,
        Cursor::No => 0,
    }
}

/// X keysym to Windows virtual-key code and whether the key is an "extended" one.
fn vk_for_keysym(sym: c_ulong) -> (u32, bool) {
    match sym {
        0x61..=0x7a => (sym as u32 - 0x20, false),
        0x41..=0x5a => (sym as u32, false),
        0x30..=0x39 => (sym as u32, false),
        0x20 => (0x20, false),
        0xff0d => (0x0D, false),
        0xff8d => (0x0D, true),
        0xff1b => (0x1B, false),
        0xff08 => (0x08, false),
        0xff09 | 0xfe20 => (0x09, false),
        0xff51 => (0x25, true),
        0xff52 => (0x26, true),
        0xff53 => (0x27, true),
        0xff54 => (0x28, true),
        0xff50 => (0x24, true),
        0xff57 => (0x23, true),
        0xff55 => (0x21, true),
        0xff56 => (0x22, true),
        0xff63 => (0x2D, true),
        0xffff => (0x2E, true),
        0xffe1 | 0xffe2 => (0x10, false),
        0xffe3 => (0x11, false),
        0xffe4 => (0x11, true),
        0xffe9 | 0xffe7 => (0x12, false),
        0xffea | 0xffe8 | 0xfe03 => (0x12, true),
        0xffe5 => (0x14, false),
        0xff7f => (0x90, false),
        0xff14 => (0x91, false),
        0xff13 => (0x13, false),
        0xff61 => (0x2C, true),
        0xffeb | 0xffec => (0x5B, true),
        0xff67 => (0x5D, true),
        0xffbe..=0xffc9 => (0x70 + (sym as u32 - 0xffbe), false),
        0xffb0..=0xffb9 => (0x60 + (sym as u32 - 0xffb0), false),
        0xffaa => (0x6A, false),
        0xffab => (0x6B, false),
        0xffad => (0x6D, false),
        0xffae => (0x6E, false),
        0xffaf => (0x6F, true),
        0x2d => (0xBD, false),
        0x3d => (0xBB, false),
        0x2c => (0xBC, false),
        0x2e => (0xBE, false),
        0x2f => (0xBF, false),
        0x3b => (0xBA, false),
        0x27 => (0xDE, false),
        0x5b => (0xDB, false),
        0x5d => (0xDD, false),
        0x5c => (0xDC, false),
        0x60 => (0xC0, false),
        _ => (0, false),
    }
}

impl X11 {
    pub fn open() -> Result<X11, String> {
        unsafe {
            let mut handle = std::ptr::null_mut();
            for name in ["libX11.so.6", "libX11.so"] {
                let c = CString::new(name).unwrap();
                handle = libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
                if !handle.is_null() {
                    break;
                }
            }
            if handle.is_null() {
                return Err("libX11 is not installed (install libx11-6) or use NLE_GUI=headless".into());
            }
            let lib = Lib::load(handle)?;
            (lib.XInitThreads)();
            (lib.XSetErrorHandler)(ignore_x_errors);
            // Auto-repeat should arrive as repeated presses, not press/release pairs.
            let detectable = libc::dlsym(handle, c"XkbSetDetectableAutoRepeat".as_ptr());
            let dpy = (lib.XOpenDisplay)(std::ptr::null());
            if dpy.is_null() {
                let d = std::env::var("DISPLAY").unwrap_or_default();
                return Err(format!("cannot connect to the X display `{d}`"));
            }
            if !detectable.is_null() {
                let f: unsafe extern "C" fn(*mut Display, c_int, *mut c_int) -> c_int = std::mem::transmute(detectable);
                f(dpy, 1, std::ptr::null_mut());
            }
            let screen = (lib.XDefaultScreen)(dpy);
            let intern = |name: &str| {
                let c = CString::new(name).unwrap();
                (lib.XInternAtom)(dpy, c.as_ptr(), 0)
            };
            Ok(X11 {
                root: (lib.XRootWindow)(dpy, screen),
                depth: (lib.XDefaultDepth)(dpy, screen),
                visual: (lib.XDefaultVisual)(dpy, screen),
                gc: (lib.XDefaultGC)(dpy, screen),
                fd: (lib.XConnectionNumber)(dpy),
                wm_delete: intern("WM_DELETE_WINDOW"),
                wm_protocols: intern("WM_PROTOCOLS"),
                net_wm_name: intern("_NET_WM_NAME"),
                utf8: intern("UTF8_STRING"),
                motif_hints: intern("_MOTIF_WM_HINTS"),
                net_wm_state: intern("_NET_WM_STATE"),
                net_wm_above: intern("_NET_WM_STATE_ABOVE"),
                dpy,
                screen,
                lib,
                windows: Mutex::new(HashMap::new()),
                cursors: Mutex::new(HashMap::new()),
                read_lock: Mutex::new(()),
                clipboard: Mutex::new(String::new()),
            })
        }
    }

    unsafe fn size_hints(&self, win: Xid, pos: Option<(i32, i32)>, fixed: Option<(i32, i32)>) {
        // XSizeHints: flags(long) x y width height min_w min_h max_w max_h inc... (80 bytes)
        let mut h = [0i64; 10];
        let mut flags: i64 = 0;
        let ints = h.as_mut_ptr() as *mut i32;
        if let Some((x, y)) = pos {
            flags |= 1 | 4; // USPosition | PPosition
            *ints.add(2) = x;
            *ints.add(3) = y;
        }
        if let Some((w, hh)) = fixed {
            flags |= 16 | 32; // PMinSize | PMaxSize
            *ints.add(6) = w;
            *ints.add(7) = hh;
            *ints.add(8) = w;
            *ints.add(9) = hh;
        }
        h[0] = flags;
        (self.lib.XSetWMNormalHints)(self.dpy, win, h.as_mut_ptr() as *mut c_void);
    }

    unsafe fn set_title_prop(&self, win: Xid, title: &str) {
        let c = CString::new(title.replace('\0', "")).unwrap_or_default();
        (self.lib.XStoreName)(self.dpy, win, c.as_ptr());
        (self.lib.XChangeProperty)(self.dpy, win, self.net_wm_name, self.utf8, 8, 0, title.as_ptr(), title.len() as c_int);
    }

    unsafe fn translate_key(&self, ev: *mut u64, down: bool, win: u64) -> Option<Event> {
        let evp = ev as *mut c_void;
        let keycode = *(ev as *const u8).add(84).cast::<u32>();
        let sym0 = (self.lib.XLookupKeysym)(evp, 0);
        let (vk, ext) = vk_for_keysym(sym0);
        let mut ch = None;
        if down {
            let mut buf = [0 as c_char; 16];
            let mut sym: c_ulong = 0;
            let n = (self.lib.XLookupString)(evp, buf.as_mut_ptr(), 16, &mut sym, std::ptr::null_mut());
            if n > 0 && sym != 0xffff {
                let bytes: Vec<u8> = buf[..n as usize].iter().map(|b| *b as u8).collect();
                ch = match std::str::from_utf8(&bytes) {
                    Ok(s) => s.chars().next(),
                    Err(_) => Some(bytes[0] as char),
                };
            } else if sym >= 0x01000000 {
                ch = char::from_u32((sym - 0x01000000) as u32);
            }
        }
        if vk == 0 && ch.is_none() {
            return None;
        }
        Some(Event::Key { win, down, vk, scan: keycode.saturating_sub(8), ch, ext, repeat: false })
    }

    fn convert(&self, ev: *mut u64, out: &mut Vec<Event>) {
        unsafe {
            let b = ev as *const u8;
            let ty = *(b as *const c_int) as u32;
            let rd_i32 = |o: usize| *(b.add(o) as *const i32);
            let rd_u64 = |o: usize| *(b.add(o) as *const u64);
            match ty {
                KEY_PRESS | KEY_RELEASE => {
                    if let Some(e) = self.translate_key(ev, ty == KEY_PRESS, rd_u64(32)) {
                        out.push(e);
                    }
                }
                BUTTON_PRESS | BUTTON_RELEASE => {
                    let button = *(b.add(84) as *const u32);
                    let (win, x, y) = (rd_u64(32), rd_i32(64), rd_i32(68));
                    let down = ty == BUTTON_PRESS;
                    match button {
                        1..=3 => out.push(Event::Button { win, button, down, x, y }),
                        4 if down => out.push(Event::Wheel { win, delta: 120, x, y }),
                        5 if down => out.push(Event::Wheel { win, delta: -120, x, y }),
                        _ => {}
                    }
                }
                MOTION_NOTIFY => out.push(Event::Motion { win: rd_u64(32), x: rd_i32(64), y: rd_i32(68) }),
                EXPOSE => {
                    let (x, y, w, h) = (rd_i32(40), rd_i32(44), rd_i32(48), rd_i32(52));
                    out.push(Event::Expose { win: rd_u64(32), rect: Rect::new(x, y, x + w, y + h) });
                }
                CONFIGURE_NOTIFY => {
                    let win = rd_u64(40);
                    let (w, h) = (rd_i32(56), rd_i32(60));
                    let (mut rx, mut ry) = (0, 0);
                    let mut child: Xid = 0;
                    (self.lib.XTranslateCoordinates)(self.dpy, win, self.root, 0, 0, &mut rx, &mut ry, &mut child);
                    out.push(Event::Configure { win, x: rx, y: ry, w, h });
                }
                CLIENT_MESSAGE => {
                    if rd_u64(40) == self.wm_protocols && rd_u64(56) == self.wm_delete {
                        out.push(Event::Close { win: rd_u64(32) });
                    }
                }
                FOCUS_IN | FOCUS_OUT => {
                    let (mode, detail) = (rd_i32(40), rd_i32(44));
                    if mode != 1 && mode != 2 && detail != 5 {
                        out.push(Event::Focus { win: rd_u64(32), gained: ty == FOCUS_IN });
                    }
                }
                _ => {}
            }
        }
    }

    fn drain(&self) -> Vec<Event> {
        let _g = self.read_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        unsafe {
            while (self.lib.XPending)(self.dpy) > 0 {
                let mut ev = [0u64; 24];
                (self.lib.XNextEvent)(self.dpy, ev.as_mut_ptr());
                self.convert(ev.as_mut_ptr(), &mut out);
            }
        }
        out
    }
}

impl Backend for X11 {
    fn name(&self) -> &'static str {
        "x11"
    }

    fn screen_size(&self) -> (i32, i32) {
        unsafe { ((self.lib.XDisplayWidth)(self.dpy, self.screen), (self.lib.XDisplayHeight)(self.dpy, self.screen)) }
    }

    fn create_window(&self, spec: &WinSpec) -> u64 {
        unsafe {
            let (x, y) = (spec.x.unwrap_or(0), spec.y.unwrap_or(0));
            let win = (self.lib.XCreateSimpleWindow)(self.dpy, self.root, x, y, spec.w.max(1) as c_uint, spec.h.max(1) as c_uint, 0, 0, 0);
            (self.lib.XSelectInput)(self.dpy, win, EVENT_MASK);
            (self.lib.XSetWindowBackgroundPixmap)(self.dpy, win, 0);
            if spec.popup {
                // XSetWindowAttributes with only override_redirect set (CWOverrideRedirect = 1 << 9)
                let mut attrs = [0u64; 14];
                attrs[11] = 1;
                (self.lib.XChangeWindowAttributes)(self.dpy, win, 1 << 9, attrs.as_mut_ptr());
            }
            let mut proto = self.wm_delete;
            (self.lib.XSetWMProtocols)(self.dpy, win, &mut proto, 1);
            self.set_title_prop(win, &spec.title);
            self.size_hints(win, spec.x.zip(spec.y), if spec.resizable { None } else { Some((spec.w, spec.h)) });
            if !spec.decorated {
                // _MOTIF_WM_HINTS: flags = MWM_HINTS_DECORATIONS, decorations = none
                let hints: [c_long; 5] = [2, 0, 0, 0, 0];
                (self.lib.XChangeProperty)(self.dpy, win, self.motif_hints, self.motif_hints, 32, 0, hints.as_ptr() as *const u8, 5);
            }
            if spec.topmost {
                let atom = self.net_wm_above;
                let state: [c_ulong; 1] = [atom];
                (self.lib.XChangeProperty)(self.dpy, win, self.net_wm_state, 4 /*XA_ATOM*/, 32, 0, state.as_ptr() as *const u8, 1);
            }
            if let Some(owner) = spec.transient_for {
                (self.lib.XSetTransientForHint)(self.dpy, win, owner);
            }
            self.windows.lock().unwrap_or_else(|e| e.into_inner()).insert(win, WinInfo { cursor: None });
            (self.lib.XFlush)(self.dpy);
            win
        }
    }

    fn destroy_window(&self, native: u64) {
        unsafe {
            (self.lib.XDestroyWindow)(self.dpy, native);
            (self.lib.XFlush)(self.dpy);
        }
        self.windows.lock().unwrap_or_else(|e| e.into_inner()).remove(&native);
    }

    fn show(&self, native: u64, visible: bool) {
        unsafe {
            if visible {
                (self.lib.XMapWindow)(self.dpy, native);
            } else {
                (self.lib.XUnmapWindow)(self.dpy, native);
            }
            (self.lib.XFlush)(self.dpy);
        }
    }

    fn set_title(&self, native: u64, title: &str) {
        unsafe {
            self.set_title_prop(native, title);
            (self.lib.XFlush)(self.dpy);
        }
    }

    fn move_resize(&self, native: u64, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            (self.lib.XMoveResizeWindow)(self.dpy, native, x, y, w.max(1) as c_uint, h.max(1) as c_uint);
            (self.lib.XFlush)(self.dpy);
        }
    }

    fn set_resizable(&self, native: u64, resizable: bool, w: i32, h: i32) {
        unsafe {
            self.size_hints(native, None, if resizable { None } else { Some((w, h)) });
        }
    }

    fn present(&self, native: u64, surf: &Surface, rect: Rect) {
        let r = rect.intersect(&surf.bounds());
        if r.is_empty() {
            return;
        }
        unsafe {
            let data = surf.bytes().as_ptr() as *mut c_char;
            let img = (self.lib.XCreateImage)(self.dpy, self.visual, self.depth as c_uint, 2 /*ZPixmap*/, 0, data, surf.w as c_uint, surf.h as c_uint, 32, surf.stride as c_int);
            if img.is_null() {
                return;
            }
            (self.lib.XPutImage)(self.dpy, native, self.gc, img, r.l, r.t, r.l, r.t, r.width() as c_uint, r.height() as c_uint);
            // the pixels belong to the surface: detach them before the image is freed
            *((img as *mut u8).add(16) as *mut *mut c_char) = std::ptr::null_mut();
            (self.lib.XFree)(img);
            (self.lib.XFlush)(self.dpy);
        }
    }

    fn set_cursor(&self, native: u64, cursor: Cursor) {
        unsafe {
            let shape = cursor_shape(cursor);
            let id = {
                let mut m = self.cursors.lock().unwrap_or_else(|e| e.into_inner());
                *m.entry(shape).or_insert_with(|| (self.lib.XCreateFontCursor)(self.dpy, shape))
            };
            (self.lib.XDefineCursor)(self.dpy, native, id);
            (self.lib.XFlush)(self.dpy);
        }
        if let Some(w) = self.windows.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&native) {
            w.cursor = Some(cursor);
        }
    }

    fn focus(&self, native: u64) {
        unsafe {
            (self.lib.XSetInputFocus)(self.dpy, native, 2 /*RevertToParent*/, 0);
            (self.lib.XFlush)(self.dpy);
        }
    }

    fn raise(&self, native: u64) {
        unsafe {
            (self.lib.XRaiseWindow)(self.dpy, native);
            (self.lib.XFlush)(self.dpy);
        }
    }

    fn grab_pointer(&self, native: u64, grab: bool) {
        unsafe {
            if grab {
                (self.lib.XGrabPointer)(self.dpy, native, 0, 0x4C, 1, 1, 0, 0, 0);
            } else {
                (self.lib.XUngrabPointer)(self.dpy, 0);
            }
            (self.lib.XFlush)(self.dpy);
        }
    }

    fn pointer(&self) -> (i32, i32, u32) {
        unsafe {
            let (mut root, mut child): (Xid, Xid) = (0, 0);
            let (mut rx, mut ry, mut wx, mut wy) = (0, 0, 0, 0);
            let mut mask: c_uint = 0;
            (self.lib.XQueryPointer)(self.dpy, self.root, &mut root, &mut child, &mut rx, &mut ry, &mut wx, &mut wy, &mut mask);
            let mut buttons = 0;
            if mask & 0x100 != 0 {
                buttons |= 1;
            }
            if mask & 0x400 != 0 {
                buttons |= 2;
            }
            if mask & 0x200 != 0 {
                buttons |= 4;
            }
            (rx, ry, buttons)
        }
    }

    fn bell(&self) {
        unsafe {
            (self.lib.XBell)(self.dpy, 0);
            (self.lib.XFlush)(self.dpy);
        }
    }

    fn poll(&self, wake_fd: i32, timeout_ms: i32) -> Vec<Event> {
        let events = self.drain();
        if !events.is_empty() || timeout_ms == 0 {
            return events;
        }
        let mut fds = [libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: wake_fd, events: libc::POLLIN, revents: 0 }];
        let n = unsafe { libc::poll(fds.as_mut_ptr(), if wake_fd >= 0 { 2 } else { 1 }, timeout_ms) };
        if n > 0 && fds[0].revents & libc::POLLIN != 0 {
            return self.drain();
        }
        Vec::new()
    }

    fn set_clipboard(&self, text: &str) {
        *self.clipboard.lock().unwrap_or_else(|e| e.into_inner()) = text.to_string();
    }

    fn get_clipboard(&self) -> String {
        self.clipboard.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms_map_to_virtual_keys() {
        assert_eq!(vk_for_keysym(0x61), (0x41, false));
        assert_eq!(vk_for_keysym(0x39), (0x39, false));
        assert_eq!(vk_for_keysym(0xff0d), (0x0D, false));
        assert_eq!(vk_for_keysym(0xff51), (0x25, true));
        assert_eq!(vk_for_keysym(0xffbe), (0x70, false));
        assert_eq!(vk_for_keysym(0xffe1), (0x10, false));
        assert_eq!(vk_for_keysym(0xdead), (0, false));
    }

    #[test]
    fn opening_without_a_display_fails_with_a_reason() {
        // only meaningful where there is no X server; elsewhere it either works or names the display
        let saved = std::env::var_os("DISPLAY");
        std::env::set_var("DISPLAY", ":99999");
        let r = X11::open();
        match saved {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        if let Err(e) = r {
            assert!(e.contains("display") || e.contains("libX11"), "{e}");
        }
    }
}
