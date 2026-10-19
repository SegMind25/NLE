//! user32: window classes, windows, messages, input, painting helpers, rectangles, clipboard.
//! The behaviour lives in `crate::gui`; these are the Win32 entry points over it.

use crate::gui::api::{self, send_ansi};
use crate::gui::backend::Cursor;
use crate::gui::gdi::{Object, Target};
use crate::gui::msg::{self, call_proc, current_ansi, with_ansi, Filter, Next};
use crate::gui::surface::Rect;
use crate::gui::wm::{Class, ClassRef, CreateArgs};
use crate::gui::{self, with, Msg, WS_CHILD, WS_DISABLED, WS_VISIBLE};
use crate::runtime::handles::{self, current_tid};
use crate::runtime::strings::*;
use crate::win::*;
use std::sync::atomic::{AtomicI32, Ordering};

pub unsafe fn text_arg(p: usize, wide: bool) -> String {
    if p == 0 {
        String::new()
    } else if wide {
        w_ptr_to_string(p as *const u16)
    } else {
        a_to_string(p as *const u8)
    }
}

pub unsafe fn rect_from(p: *const i32) -> Rect {
    Rect::new(*p, *p.add(1), *p.add(2), *p.add(3))
}

pub unsafe fn rect_to(p: *mut i32, r: Rect) {
    *p = r.l;
    *p.add(1) = r.t;
    *p.add(2) = r.r;
    *p.add(3) = r.b;
}

fn class_ref(p: usize, wide: bool) -> ClassRef {
    if p < 0x10000 {
        ClassRef::Atom(p as u16)
    } else {
        ClassRef::Name(unsafe { text_arg(p, wide) })
    }
}

fn point_of(v: i64) -> (i32, i32) {
    (v as i32, (v >> 32) as i32)
}

// ---- classes ----------------------------------------------------------------------------------------

unsafe fn register_class(p: *const u8, ex: bool, wide: bool) -> u32 {
    let base = if ex { 4 } else { 0 };
    let rd32 = |o: usize| *(p.add(o) as *const u32);
    let rd64 = |o: usize| *(p.add(o) as *const usize);
    let name = text_arg(rd64(64), wide);
    let class = Class {
        name,
        atom: 0,
        style: rd32(base),
        wndproc: rd64(8),
        cls_extra: rd32(16) as i32,
        wnd_extra: rd32(20) as i32,
        hinstance: rd64(24),
        icon: rd64(32),
        cursor: rd64(40),
        background: rd64(48),
        menu_name: if rd64(56) == 0 { None } else if rd64(56) < 0x10000 { None } else { Some(text_arg(rd64(56), wide)) },
        unicode: wide,
        native: false,
    };
    if class.name.is_empty() {
        set_last_error(87);
        return 0;
    }
    match with(|g| g.register_class(class)) {
        Some(a) => a as u32,
        None => {
            set_last_error(gui::wm::ERROR_CLASS_ALREADY_EXISTS);
            0
        }
    }
}

unsafe fn get_class_info(name: usize, out: *mut u8, ex: bool, wide: bool) -> BOOL {
    let Some(c) = with(|g| g.find_class(&class_ref(name, wide))) else {
        set_last_error(gui::wm::ERROR_CANNOT_FIND_WND_CLASS);
        return FALSE;
    };
    let base = if ex { 4 } else { 0 };
    std::ptr::write_bytes(out.add(base), 0, if ex { 76 } else { 72 });
    if ex {
        *(out as *mut u32) = 80;
    }
    *(out.add(base) as *mut u32) = c.style;
    *(out.add(8) as *mut usize) = c.wndproc;
    *(out.add(16) as *mut i32) = c.cls_extra;
    *(out.add(20) as *mut i32) = c.wnd_extra;
    *(out.add(24) as *mut usize) = c.hinstance;
    *(out.add(32) as *mut usize) = c.icon;
    *(out.add(40) as *mut usize) = c.cursor;
    *(out.add(48) as *mut usize) = c.background;
    TRUE
}

unsafe fn class_name_of(h: usize, buf: usize, cap: i32, wide: bool) -> i32 {
    let Some(name) = with(|g| g.windows.get(&h).map(|w| w.class.name.clone())) else {
        set_last_error(gui::wm::ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    };
    if cap <= 0 {
        return 0;
    }
    if wide {
        return_w(buf as *mut u16, cap as u32, &name) as i32
    } else {
        return_a(buf as *mut u8, cap as u32, &name) as i32
    }
}

// ---- windows ----------------------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
unsafe fn create_window(ex: u32, class: usize, title: usize, style: u32, x: i32, y: i32, w: i32, h: i32, parent: usize, menu: usize, inst: usize, param: usize, wide: bool) -> usize {
    api::create_window_ex(CreateArgs { ex_style: ex, class: class_ref(class, wide), title: text_arg(title, wide), style, x, y, w, h, parent, menu, hinstance: inst, param, ansi: !wide })
}

unsafe fn window_long(h: usize, index: i32, ptr: bool) -> isize {
    let v = with(|g| {
        let w = g.windows.get(&h)?;
        Some(match index {
            -16 => w.style as isize,
            -20 => w.ex_style as isize,
            -4 => w.wndproc as isize,
            -6 => w.class.hinstance as isize,
            -8 => if w.parent != 0 { w.parent as isize } else { w.owner as isize },
            -21 => w.userdata,
            -12 => w.id as isize,
            i if i >= 0 => {
                let i = i as usize;
                let n = if ptr { 8 } else { 4 };
                if i + n > w.extra.len() {
                    return None;
                }
                let mut b = [0u8; 8];
                b[..n].copy_from_slice(&w.extra[i..i + n]);
                u64::from_le_bytes(b) as isize
            }
            _ => 0,
        })
    });
    match v {
        Some(v) => v,
        None => {
            set_last_error(87);
            0
        }
    }
}

unsafe fn set_window_long(h: usize, index: i32, value: isize, ptr: bool) -> isize {
    let old = with(|g| {
        let w = g.windows.get_mut(&h)?;
        Some(match index {
            -16 => {
                let visible = w.style & WS_VISIBLE;
                std::mem::replace(&mut w.style, (value as u32 & !WS_VISIBLE) | visible) as isize
            }
            -20 => std::mem::replace(&mut w.ex_style, value as u32) as isize,
            -4 => std::mem::replace(&mut w.wndproc, value as usize) as isize,
            -21 => std::mem::replace(&mut w.userdata, value),
            -12 => std::mem::replace(&mut w.id, value as usize) as isize,
            -8 => std::mem::replace(&mut w.owner, value as usize) as isize,
            i if i >= 0 => {
                let i = i as usize;
                let n = if ptr { 8 } else { 4 };
                if i + n > w.extra.len() {
                    return None;
                }
                let mut b = [0u8; 8];
                b[..n].copy_from_slice(&w.extra[i..i + n]);
                w.extra[i..i + n].copy_from_slice(&(value as u64).to_le_bytes()[..n]);
                u64::from_le_bytes(b) as isize
            }
            _ => 0,
        })
    });
    match old {
        Some(v) => v,
        None => {
            set_last_error(87);
            0
        }
    }
}

fn set_text_ptr(h: usize, text: usize, wide: bool) -> BOOL {
    unsafe { send_ansi(h, gui::WM_SETTEXT, 0, text as isize, !wide) as BOOL }
}

unsafe fn get_text(h: usize, buf: usize, n: i32, wide: bool) -> i32 {
    if n <= 0 || buf == 0 {
        return 0;
    }
    send_ansi(h, gui::WM_GETTEXT, n as usize, buf as isize, !wide) as i32
}

unsafe fn find_window_impl(class: usize, title: usize, wide: bool, parent: usize, after: usize) -> usize {
    let class = if class == 0 { None } else { Some(text_arg(class, wide).to_lowercase()) };
    let title = if title == 0 { None } else { Some(text_arg(title, wide).to_lowercase()) };
    with(|g| {
        let mut hs: Vec<usize> = if parent == 0 { g.windows.values().filter(|w| w.parent == 0).map(|w| w.hwnd).collect() } else { g.windows.get(&parent).map(|w| w.children.clone()).unwrap_or_default() };
        hs.sort();
        let mut skipping = after != 0;
        for h in hs {
            if skipping {
                if h == after {
                    skipping = false;
                }
                continue;
            }
            let w = &g.windows[&h];
            if class.as_ref().is_none_or(|c| w.class.name.to_lowercase() == *c) && title.as_ref().is_none_or(|t| w.text.to_lowercase() == *t) {
                return h;
            }
        }
        0
    })
}

// ---- messages ---------------------------------------------------------------------------------------

unsafe fn write_msg(out: *mut u8, m: &Msg) {
    *(out as *mut u64) = m.hwnd as u64;
    *(out.add(8) as *mut u32) = m.message;
    *(out.add(16) as *mut u64) = m.wparam as u64;
    *(out.add(24) as *mut i64) = m.lparam as i64;
    *(out.add(32) as *mut u32) = m.time;
    *(out.add(36) as *mut i32) = m.pt.0;
    *(out.add(40) as *mut i32) = m.pt.1;
}

pub unsafe fn read_msg(p: *const u8) -> Msg {
    Msg { hwnd: *(p as *const u64) as usize, message: *(p.add(8) as *const u32), wparam: *(p.add(16) as *const u64) as usize, lparam: *(p.add(24) as *const i64) as isize, time: *(p.add(32) as *const u32), pt: (*(p.add(36) as *const i32), *(p.add(40) as *const i32)) }
}

static LAST_MSG: (AtomicI32, AtomicI32, AtomicI32) = (AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0));

unsafe fn get_message(out: *mut u8, hwnd: usize, min: u32, max: u32) -> i32 {
    // a window handle that is no longer valid makes GetMessage fail
    if hwnd != 0 && hwnd != usize::MAX && !with(|g| g.windows.contains_key(&hwnd)) {
        set_last_error(gui::wm::ERROR_INVALID_WINDOW_HANDLE);
        return -1;
    }
    match msg::next_message(&Filter { hwnd, min, max }, true, true) {
        Next::Msg(m) => {
            LAST_MSG.0.store(m.time as i32, Ordering::Relaxed);
            LAST_MSG.1.store(m.pt.0, Ordering::Relaxed);
            LAST_MSG.2.store(m.pt.1, Ordering::Relaxed);
            write_msg(out, &m);
            1
        }
        Next::Quit(code) => {
            write_msg(out, &Msg { hwnd: 0, message: gui::WM_QUIT, wparam: code as usize, lparam: 0, time: 0, pt: (0, 0) });
            0
        }
        Next::None => 0,
    }
}

unsafe fn peek_message(out: *mut u8, hwnd: usize, min: u32, max: u32, flags: u32) -> BOOL {
    match msg::next_message(&Filter { hwnd, min, max }, flags & 1 != 0, false) {
        Next::Msg(m) => {
            write_msg(out, &m);
            TRUE
        }
        Next::Quit(code) => {
            write_msg(out, &Msg { hwnd: 0, message: gui::WM_QUIT, wparam: code as usize, lparam: 0, time: 0, pt: (0, 0) });
            TRUE
        }
        Next::None => FALSE,
    }
}

unsafe fn post_message(hwnd: usize, message: u32, wp: usize, lp: isize) -> BOOL {
    if hwnd == 0xFFFF || hwnd == usize::MAX {
        with(|g| {
            let tops: Vec<usize> = g.windows.values().filter(|w| w.parent == 0).map(|w| w.hwnd).collect();
            for t in tops {
                g.post_message(t, message, wp, lp);
            }
        });
        return TRUE;
    }
    if with(|g| g.post_message(hwnd, message, wp, lp)) {
        TRUE
    } else {
        set_last_error(gui::wm::ERROR_INVALID_WINDOW_HANDLE);
        FALSE
    }
}

unsafe fn send_message(hwnd: usize, message: u32, wp: usize, lp: isize, wide: bool) -> isize {
    if hwnd == 0xFFFF || hwnd == usize::MAX {
        let tops: Vec<usize> = with(|g| g.windows.values().filter(|w| w.parent == 0).map(|w| w.hwnd).collect());
        for t in tops {
            send_ansi(t, message, wp, lp, !wide);
        }
        return 1;
    }
    send_ansi(hwnd, message, wp, lp, !wide)
}

fn peek_pending() -> bool {
    unsafe { !matches!(msg::next_message(&Filter { hwnd: 0, min: 0, max: 0 }, false, false), Next::None) }
}

unsafe fn msg_wait(count: u32, handles_p: *const usize, wait_all: BOOL, ms: u32) -> u32 {
    let start = std::time::Instant::now();
    let objs: Vec<_> = (0..count as usize).filter_map(|i| handles::get(*handles_p.add(i))).collect();
    loop {
        if !objs.is_empty() {
            let r = handles::wait_objects(&objs, wait_all != 0, 0);
            if r != 258 {
                return r;
            }
        }
        if peek_pending() {
            return count;
        }
        if ms != 0xFFFF_FFFF && start.elapsed().as_millis() as u32 >= ms {
            return 258;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

// ---- cursors and icons ----------------------------------------------------------------------------

fn stub_handle(kind: u32, id: u32) -> usize {
    with(|g| {
        let key = (kind * 100_000 + id) as i32 + 5000;
        if let Some(h) = g.stock.get(&key) {
            return *h;
        }
        let h = g.add_object(Object::Stub(id));
        g.stock.insert(key, h);
        h
    })
}

unsafe fn load_cursor(inst: usize, name: usize) -> usize {
    if inst == 0 && name < 0x10000 {
        stub_handle(1, name as u32)
    } else {
        // a cursor from the program's resources: the pointer shape is the arrow
        stub_handle(1, 32512)
    }
}

fn icon_object(img: Option<crate::gui::res::IconImage>, stock: u32) -> usize {
    match img {
        Some(i) => with(|g| g.add_object(Object::Icon(std::sync::Arc::new(i)))),
        None => stub_handle(2, stock),
    }
}

/// `LoadIcon`: a stock icon for module 0, otherwise the group icon from the module's resources.
unsafe fn load_icon(inst: usize, name: usize, wide: bool) -> usize {
    if inst == 0 {
        return stub_handle(2, if name < 0x10000 { name as u32 } else { 32512 });
    }
    icon_object(crate::gui::res::load_icon_image(inst, &crate::gui::res::Key::from_arg(name, wide), 32), 32512)
}

fn bitmap_object(s: Option<crate::gui::surface::Surface>) -> usize {
    match s {
        Some(surf) => with(|g| g.add_object(Object::Bitmap(crate::gui::gdi::Bitmap { surf, bpp: 32, bits: 0 }))),
        None => 0,
    }
}

unsafe fn load_bitmap(inst: usize, name: usize, wide: bool) -> usize {
    use crate::gui::res::{self, Key};
    if inst == 0 {
        // the built-in OEM bitmaps (check marks, arrows) are not drawn; a small blank one stands in
        return bitmap_object(Some(crate::gui::surface::Surface::new(16, 16)));
    }
    bitmap_object(res::load(inst, res::RT_BITMAP, &Key::from_arg(name, wide)).and_then(res::bitmap_from_dib))
}

/// `LoadImage` for bitmaps and icons, from a module's resources or (with `LR_LOADFROMFILE`) from a file.
unsafe fn load_image(inst: usize, name: usize, kind: u32, cx: i32, _cy: i32, flags: u32, wide: bool) -> usize {
    use crate::gui::res::{self, Key};
    const LR_LOADFROMFILE: u32 = 0x10;
    const LR_DEFAULTSIZE: u32 = 0x40;
    let from_file = flags & LR_LOADFROMFILE != 0;
    let file = || -> Option<Vec<u8>> {
        let path = text_arg(name, wide);
        std::fs::read(crate::runtime::paths::resolve(&path)).ok()
    };
    let want = if cx > 0 { cx } else if flags & LR_DEFAULTSIZE != 0 || kind != 0 { 32 } else { 0 };
    match kind {
        0 => {
            if from_file {
                bitmap_object(file().and_then(|d| res::bitmap_from_file(&d)))
            } else {
                load_bitmap(inst, name, wide)
            }
        }
        1 => {
            let img = if from_file { file().and_then(|d| res::icon_from_file(&d, want)) } else if inst != 0 { res::load_icon_image(inst, &Key::from_arg(name, wide), want) } else { None };
            icon_object(img, 32512)
        }
        2 => stub_handle(1, if inst == 0 && name < 0x10000 { name as u32 } else { 32512 }),
        _ => 0,
    }
}

unsafe fn load_string_w(inst: usize, id: u32, buf: *mut u16, max: i32) -> i32 {
    let Some(units) = crate::gui::res::load_string(inst, id) else { return 0 };
    if max == 0 {
        // a zero-sized buffer asks for a pointer to the string itself
        let leaked: &'static mut [u16] = Box::leak(units.into_boxed_slice());
        *(buf as *mut usize) = leaked.as_ptr() as usize;
        return leaked.len() as i32;
    }
    if buf.is_null() || max < 0 {
        return 0;
    }
    let n = units.len().min(max as usize - 1);
    std::ptr::copy_nonoverlapping(units.as_ptr(), buf, n);
    *buf.add(n) = 0;
    n as i32
}

unsafe fn load_string_a(inst: usize, id: u32, buf: *mut u8, max: i32) -> i32 {
    let Some(units) = crate::gui::res::load_string(inst, id) else { return 0 };
    let text = String::from_utf16_lossy(&units).into_bytes();
    if max == 0 {
        let leaked: &'static mut [u8] = Box::leak(text.into_boxed_slice());
        *(buf as *mut usize) = leaked.as_ptr() as usize;
        return leaked.len() as i32;
    }
    if buf.is_null() || max < 0 {
        return 0;
    }
    let mut n = text.len().min(max as usize - 1);
    while n > 0 && n < text.len() && text[n] & 0xC0 == 0x80 {
        n -= 1;
    }
    std::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
    *buf.add(n) = 0;
    n as i32
}

// ---- metrics ------------------------------------------------------------------------------------------

fn system_metric(index: i32) -> i32 {
    let (sw, sh) = with(|g| g.screen_size());
    match index {
        0 | 16 | 78 => sw,
        1 | 79 => sh,
        17 => sh - 40,
        2 | 3 | 20 | 21 => 17,
        4 => crate::gui::wm::CAPTION_H,
        5 | 6 => 1,
        7 | 8 => 3,
        9 | 10 => 17,
        11 | 12 | 13 | 14 => 32,
        15 => crate::gui::wm::MENU_H,
        19 => 1,
        22 | 23 => 0,
        28 => 136,
        29 => 39,
        30 | 31 => 22,
        32 | 33 => 4,
        34 => 136,
        35 => 39,
        36 | 37 => 4,
        38 | 39 => 75,
        43 => 3,
        45 | 46 => 2,
        49 | 50 => 16,
        51 => 15,
        52 | 53 => 16,
        59 => sw,
        60 => sh,
        80 => 1,
        92 => 4,
        _ => 0,
    }
}

// ---- rectangles --------------------------------------------------------------------------------------

fn get_pointer() -> (i32, i32) {
    with(|g| match g.ensure_backend() {
        Some(b) => {
            let (x, y, _) = b.pointer();
            g.pointer = (x, y);
            (x, y)
        }
        None => g.pointer,
    })
}

crate::shims! { "user32" => USER32_SHIMS;
    fn WaitForInputIdle(_process: usize, _ms: u32) -> u32 {
        0
    }
    fn RegisterClassA(p: *const u8) -> u32 {
        register_class(p, false, false)
    }
    fn RegisterClassW(p: *const u8) -> u32 {
        register_class(p, false, true)
    }
    fn RegisterClassExA(p: *const u8) -> u32 {
        register_class(p, true, false)
    }
    fn RegisterClassExW(p: *const u8) -> u32 {
        register_class(p, true, true)
    }
    fn UnregisterClassA(name: usize, _inst: usize) -> BOOL {
        let r = class_ref(name, false);
        with(|g| match g.find_class(&r) {
            Some(c) if !g.windows.values().any(|w| w.class.atom == c.atom) => {
                g.classes.remove(&c.name.to_lowercase());
                TRUE
            }
            _ => FALSE,
        })
    }
    fn UnregisterClassW(name: usize, _inst: usize) -> BOOL {
        let r = class_ref(name, true);
        with(|g| match g.find_class(&r) {
            Some(c) if !g.windows.values().any(|w| w.class.atom == c.atom) => {
                g.classes.remove(&c.name.to_lowercase());
                TRUE
            }
            _ => FALSE,
        })
    }
    fn GetClassInfoA(_inst: usize, name: usize, out: *mut u8) -> BOOL {
        get_class_info(name, out, false, false)
    }
    fn GetClassInfoW(_inst: usize, name: usize, out: *mut u8) -> BOOL {
        get_class_info(name, out, false, true)
    }
    fn GetClassInfoExA(_inst: usize, name: usize, out: *mut u8) -> BOOL {
        get_class_info(name, out, true, false)
    }
    fn GetClassInfoExW(_inst: usize, name: usize, out: *mut u8) -> BOOL {
        get_class_info(name, out, true, true)
    }
    fn GetClassNameA(h: usize, buf: usize, n: i32) -> i32 {
        class_name_of(h, buf, n, false)
    }
    fn GetClassNameW(h: usize, buf: usize, n: i32) -> i32 {
        class_name_of(h, buf, n, true)
    }
    fn GetClassLongPtrA(h: usize, index: i32) -> usize {
        class_long(h, index)
    }
    fn GetClassLongPtrW(h: usize, index: i32) -> usize {
        class_long(h, index)
    }
    fn GetClassLongA(h: usize, index: i32) -> u32 {
        class_long(h, index) as u32
    }
    fn GetClassLongW(h: usize, index: i32) -> u32 {
        class_long(h, index) as u32
    }
    fn SetClassLongPtrA(_h: usize, _index: i32, _v: usize) -> usize {
        0
    }
    fn SetClassLongPtrW(_h: usize, _index: i32, _v: usize) -> usize {
        0
    }
    fn SetClassLongA(_h: usize, _index: i32, _v: u32) -> u32 {
        0
    }
    fn SetClassLongW(_h: usize, _index: i32, _v: u32) -> u32 {
        0
    }

    fn CreateWindowExA(ex: u32, class: usize, title: usize, style: u32, x: i32, y: i32, w: i32, h: i32, parent: usize, menu: usize, inst: usize, param: usize) -> usize {
        create_window(ex, class, title, style, x, y, w, h, parent, menu, inst, param, false)
    }
    fn CreateWindowExW(ex: u32, class: usize, title: usize, style: u32, x: i32, y: i32, w: i32, h: i32, parent: usize, menu: usize, inst: usize, param: usize) -> usize {
        create_window(ex, class, title, style, x, y, w, h, parent, menu, inst, param, true)
    }
    fn DestroyWindow(h: usize) -> BOOL {
        api::destroy_window(h) as BOOL
    }
    fn ShowWindow(h: usize, cmd: i32) -> BOOL {
        api::show_window(h, cmd) as BOOL
    }
    fn ShowWindowAsync(h: usize, cmd: i32) -> BOOL {
        api::show_window(h, cmd) as BOOL
    }
    fn UpdateWindow(h: usize) -> BOOL {
        api::update_window(h) as BOOL
    }
    fn IsWindow(h: usize) -> BOOL {
        with(|g| g.windows.contains_key(&h)) as BOOL
    }
    fn IsWindowVisible(h: usize) -> BOOL {
        with(|g| g.is_visible(h)) as BOOL
    }
    fn IsWindowEnabled(h: usize) -> BOOL {
        with(|g| g.windows.contains_key(&h) && g.is_enabled(h)) as BOOL
    }
    fn IsWindowUnicode(h: usize) -> BOOL {
        with(|g| g.windows.get(&h).is_some_and(|w| w.unicode)) as BOOL
    }
    fn EnableWindow(h: usize, enable: BOOL) -> BOOL {
        let was_disabled = with(|g| {
            let w = g.windows.get_mut(&h)?;
            let d = w.style & WS_DISABLED != 0;
            if enable != 0 {
                w.style &= !WS_DISABLED;
            } else {
                w.style |= WS_DISABLED;
            }
            g.invalidate(h, None, true);
            Some(d)
        });
        if was_disabled.is_some() {
            api::send(h, gui::WM_ENABLE, (enable != 0) as usize, 0);
        }
        was_disabled.unwrap_or(false) as BOOL
    }
    fn IsIconic(h: usize) -> BOOL {
        with(|g| g.windows.get(&h).is_some_and(|w| w.minimized)) as BOOL
    }
    fn IsZoomed(h: usize) -> BOOL {
        with(|g| g.windows.get(&h).is_some_and(|w| w.maximized)) as BOOL
    }
    fn IsChild(parent: usize, h: usize) -> BOOL {
        with(|g| h != parent && g.is_descendant(h, parent)) as BOOL
    }
    fn GetParent(h: usize) -> usize {
        with(|g| g.windows.get(&h).map_or(0, |w| if w.parent != 0 { w.parent } else { w.owner }))
    }
    fn SetParent(h: usize, parent: usize) -> usize {
        with(|g| {
            let old = g.windows.get(&h)?.parent;
            if let Some(p) = g.windows.get_mut(&old) {
                p.children.retain(|c| *c != h);
            }
            if let Some(w) = g.windows.get_mut(&h) {
                w.parent = parent;
            }
            if let Some(p) = g.windows.get_mut(&parent) {
                p.children.push(h);
            }
            Some(old)
        })
        .unwrap_or(0)
    }
    fn GetAncestor(h: usize, flags: u32) -> usize {
        with(|g| match flags {
            1 => g.windows.get(&h).map_or(0, |w| w.parent),
            _ => g.top_level_of(h),
        })
    }
    fn GetWindow(h: usize, cmd: u32) -> usize {
        with(|g| {
            let Some(w) = g.windows.get(&h) else { return 0 };
            let siblings: Vec<usize> = if w.parent != 0 {
                g.windows.get(&w.parent).map(|p| p.children.clone()).unwrap_or_default()
            } else {
                let mut v: Vec<usize> = g.windows.values().filter(|x| x.parent == 0).map(|x| x.hwnd).collect();
                v.sort();
                v
            };
            let pos = siblings.iter().position(|s| *s == h).unwrap_or(0);
            match cmd {
                0 => siblings.last().copied().unwrap_or(0),
                1 => siblings.first().copied().unwrap_or(0),
                2 => pos.checked_sub(1).and_then(|i| siblings.get(i)).copied().unwrap_or(0),
                3 => siblings.get(pos + 1).copied().unwrap_or(0),
                4 => w.owner,
                5 => w.children.last().copied().unwrap_or(0),
                _ => 0,
            }
        })
    }
    fn GetTopWindow(h: usize) -> usize {
        with(|g| if h == 0 { g.main_window() } else { g.windows.get(&h).and_then(|w| w.children.last().copied()).unwrap_or(0) })
    }
    fn GetDesktopWindow() -> usize {
        with(|g| g.desktop)
    }
    fn GetShellWindow() -> usize {
        0
    }
    fn FindWindowA(class: usize, title: usize) -> usize {
        find_window_impl(class, title, false, 0, 0)
    }
    fn FindWindowW(class: usize, title: usize) -> usize {
        find_window_impl(class, title, true, 0, 0)
    }
    fn FindWindowExA(parent: usize, after: usize, class: usize, title: usize) -> usize {
        find_window_impl(class, title, false, parent, after)
    }
    fn FindWindowExW(parent: usize, after: usize, class: usize, title: usize) -> usize {
        find_window_impl(class, title, true, parent, after)
    }
    fn EnumWindows(cb: usize, lparam: isize) -> BOOL {
        let tops: Vec<usize> = with(|g| {
            let mut v: Vec<usize> = g.windows.values().filter(|w| w.parent == 0).map(|w| w.hwnd).collect();
            v.sort();
            v.reverse();
            v
        });
        type Cb = unsafe extern "win64" fn(usize, isize) -> BOOL;
        let f: Cb = std::mem::transmute(cb);
        for h in tops {
            if f(h, lparam) == 0 {
                break;
            }
        }
        TRUE
    }
    fn EnumChildWindows(parent: usize, cb: usize, lparam: isize) -> BOOL {
        let kids: Vec<usize> = with(|g| g.subtree(parent).into_iter().skip(1).collect());
        type Cb = unsafe extern "win64" fn(usize, isize) -> BOOL;
        let f: Cb = std::mem::transmute(cb);
        for h in kids {
            if f(h, lparam) == 0 {
                break;
            }
        }
        TRUE
    }
    fn EnumThreadWindows(tid: u32, cb: usize, lparam: isize) -> BOOL {
        let tops: Vec<usize> = with(|g| g.windows.values().filter(|w| w.thread == tid).map(|w| w.hwnd).collect());
        type Cb = unsafe extern "win64" fn(usize, isize) -> BOOL;
        let f: Cb = std::mem::transmute(cb);
        for h in tops {
            if f(h, lparam) == 0 {
                break;
            }
        }
        TRUE
    }
    fn GetWindowThreadProcessId(h: usize, pid: *mut u32) -> u32 {
        if !pid.is_null() {
            *pid = libc::getpid() as u32;
        }
        with(|g| g.windows.get(&h).map_or(0, |w| w.thread))
    }
    fn GetWindowTextA(h: usize, buf: usize, n: i32) -> i32 {
        get_text(h, buf, n, false)
    }
    fn GetWindowTextW(h: usize, buf: usize, n: i32) -> i32 {
        get_text(h, buf, n, true)
    }
    fn GetWindowTextLengthA(h: usize) -> i32 {
        send_ansi(h, gui::WM_GETTEXTLENGTH, 0, 0, true) as i32
    }
    fn GetWindowTextLengthW(h: usize) -> i32 {
        send_ansi(h, gui::WM_GETTEXTLENGTH, 0, 0, false) as i32
    }
    fn SetWindowTextA(h: usize, text: usize) -> BOOL {
        set_text_ptr(h, text, false)
    }
    fn SetWindowTextW(h: usize, text: usize) -> BOOL {
        set_text_ptr(h, text, true)
    }
    fn GetWindowRect(h: usize, out: *mut i32) -> BOOL {
        match with(|g| g.windows.contains_key(&h).then(|| g.window_rect_screen(h))) {
            Some(r) => {
                rect_to(out, r);
                TRUE
            }
            None => {
                set_last_error(gui::wm::ERROR_INVALID_WINDOW_HANDLE);
                FALSE
            }
        }
    }
    fn GetClientRect(h: usize, out: *mut i32) -> BOOL {
        match with(|g| g.windows.get(&h).map(|w| w.client_size())) {
            Some((w, hh)) => {
                rect_to(out, Rect::new(0, 0, w, hh));
                TRUE
            }
            None => {
                set_last_error(gui::wm::ERROR_INVALID_WINDOW_HANDLE);
                FALSE
            }
        }
    }
    fn ClientToScreen(h: usize, p: *mut i32) -> BOOL {
        match with(|g| g.windows.contains_key(&h).then(|| g.client_to_screen(h, *p, *p.add(1)))) {
            Some((x, y)) => {
                *p = x;
                *p.add(1) = y;
                TRUE
            }
            None => FALSE,
        }
    }
    fn ScreenToClient(h: usize, p: *mut i32) -> BOOL {
        match with(|g| g.windows.contains_key(&h).then(|| g.screen_to_client(h, *p, *p.add(1)))) {
            Some((x, y)) => {
                *p = x;
                *p.add(1) = y;
                TRUE
            }
            None => FALSE,
        }
    }
    fn MapWindowPoints(from: usize, to: usize, pts: *mut i32, n: u32) -> i32 {
        let (dx, dy) = with(|g| {
            let (fx, fy) = if from == 0 { (0, 0) } else { g.client_to_screen(from, 0, 0) };
            let (tx, ty) = if to == 0 { (0, 0) } else { g.client_to_screen(to, 0, 0) };
            (fx - tx, fy - ty)
        });
        for i in 0..n as usize {
            *pts.add(i * 2) += dx;
            *pts.add(i * 2 + 1) += dy;
        }
        ((dx as u16 as u32) | ((dy as u16 as u32) << 16)) as i32
    }
    fn MoveWindow(h: usize, x: i32, y: i32, w: i32, hh: i32, _repaint: BOOL) -> BOOL {
        api::set_window_pos(h, x, y, w, hh, 4) as BOOL
    }
    fn SetWindowPos(h: usize, _after: usize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> BOOL {
        api::set_window_pos(h, x, y, cx, cy, flags) as BOOL
    }
    fn BringWindowToTop(h: usize) -> BOOL {
        with(|g| g.bring_to_top(h));
        TRUE
    }
    fn GetWindowPlacement(h: usize, wp: *mut u8) -> BOOL {
        let Some((r, max, min)) = with(|g| g.windows.get(&h).map(|w| (w.restore_rect.unwrap_or(w.rect), w.maximized, w.minimized))) else { return FALSE };
        *(wp as *mut u32) = 44;
        *(wp.add(4) as *mut u32) = 0;
        *(wp.add(8) as *mut u32) = if max { 3 } else if min { 2 } else { 1 };
        *(wp.add(12) as *mut i32) = -1;
        *(wp.add(16) as *mut i32) = -1;
        *(wp.add(20) as *mut i32) = -1;
        *(wp.add(24) as *mut i32) = -1;
        rect_to(wp.add(28) as *mut i32, r);
        TRUE
    }
    fn SetWindowPlacement(h: usize, wp: *const u8) -> BOOL {
        let r = rect_from(wp.add(28) as *const i32);
        api::set_window_pos(h, r.l, r.t, r.width(), r.height(), 4);
        api::show_window(h, *(wp.add(8) as *const u32) as i32);
        TRUE
    }
    fn AdjustWindowRect(r: *mut i32, style: u32, menu: BOOL) -> BOOL {
        adjust_rect(r, style, menu, 0)
    }
    fn AdjustWindowRectEx(r: *mut i32, style: u32, menu: BOOL, ex: u32) -> BOOL {
        adjust_rect(r, style, menu, ex)
    }
    fn AdjustWindowRectExForDpi(r: *mut i32, style: u32, menu: BOOL, ex: u32, _dpi: u32) -> BOOL {
        adjust_rect(r, style, menu, ex)
    }
    fn GetWindowLongA(h: usize, index: i32) -> i32 {
        window_long(h, index, false) as i32
    }
    fn GetWindowLongW(h: usize, index: i32) -> i32 {
        window_long(h, index, false) as i32
    }
    fn GetWindowLongPtrA(h: usize, index: i32) -> isize {
        window_long(h, index, true)
    }
    fn GetWindowLongPtrW(h: usize, index: i32) -> isize {
        window_long(h, index, true)
    }
    fn SetWindowLongA(h: usize, index: i32, v: i32) -> i32 {
        set_window_long(h, index, v as isize, false) as i32
    }
    fn SetWindowLongW(h: usize, index: i32, v: i32) -> i32 {
        set_window_long(h, index, v as isize, false) as i32
    }
    fn SetWindowLongPtrA(h: usize, index: i32, v: isize) -> isize {
        set_window_long(h, index, v, true)
    }
    fn SetWindowLongPtrW(h: usize, index: i32, v: isize) -> isize {
        set_window_long(h, index, v, true)
    }
    fn SetForegroundWindow(h: usize) -> BOOL {
        api::activate_window(with(|g| g.top_level_of(h)));
        TRUE
    }
    fn GetForegroundWindow() -> usize {
        with(|g| g.active)
    }
    fn SetActiveWindow(h: usize) -> usize {
        let prev = with(|g| g.active);
        api::activate_window(with(|g| g.top_level_of(h)));
        prev
    }
    fn GetActiveWindow() -> usize {
        with(|g| g.active)
    }
    fn SetFocus(h: usize) -> usize {
        api::set_focus(h)
    }
    fn GetFocus() -> usize {
        with(|g| g.focus)
    }
    fn SetCapture(h: usize) -> usize {
        with(|g| {
            let prev = std::mem::replace(&mut g.capture, h);
            let top = g.top_level_of(h);
            if let (Some(b), Some(w)) = (g.backend.clone(), g.windows.get(&top)) {
                b.grab_pointer(w.native, true);
            }
            prev
        })
    }
    fn ReleaseCapture() -> BOOL {
        with(|g| {
            let prev = std::mem::take(&mut g.capture);
            if prev != 0 {
                let top = g.top_level_of(prev);
                if let (Some(b), Some(w)) = (g.backend.clone(), g.windows.get(&top)) {
                    b.grab_pointer(w.native, false);
                }
            }
        });
        TRUE
    }
    fn GetCapture() -> usize {
        with(|g| g.capture)
    }
    fn WindowFromPoint(pt: i64) -> usize {
        let (x, y) = point_of(pt);
        with(|g| {
            let mut tops: Vec<usize> = g.windows.values().filter(|w| w.parent == 0 && w.visible_flag()).map(|w| w.hwnd).collect();
            tops.sort();
            for t in tops.into_iter().rev() {
                let (cx, cy) = g.screen_to_client(t, x, y);
                let (w, h) = g.windows[&t].client_size();
                if Rect::new(0, 0, w, h).contains(cx, cy) {
                    return g.hit_test(t, cx, cy).0;
                }
            }
            0
        })
    }
    fn ChildWindowFromPoint(parent: usize, pt: i64) -> usize {
        let (x, y) = point_of(pt);
        with(|g| g.windows.get(&parent).map_or(0, |_| {
            let (h, _, _) = g.hit_test(parent, x, y);
            h
        }))
    }
    fn GetDlgItem(dlg: usize, id: i32) -> usize {
        with(|g| g.subtree(dlg).into_iter().skip(1).find(|h| g.windows.get(h).is_some_and(|w| w.id == id as usize && w.parent != 0)).unwrap_or(0))
    }
    fn GetDlgCtrlID(h: usize) -> i32 {
        with(|g| g.windows.get(&h).map_or(0, |w| w.id as i32))
    }
    fn SetDlgItemTextA(dlg: usize, id: i32, text: usize) -> BOOL {
        let h = GetDlgItem(dlg, id);
        if h == 0 { FALSE } else { set_text_ptr(h, text, false) }
    }
    fn SetDlgItemTextW(dlg: usize, id: i32, text: usize) -> BOOL {
        let h = GetDlgItem(dlg, id);
        if h == 0 { FALSE } else { set_text_ptr(h, text, true) }
    }
    fn GetDlgItemTextA(dlg: usize, id: i32, buf: usize, n: i32) -> u32 {
        let h = GetDlgItem(dlg, id);
        if h == 0 { 0 } else { get_text(h, buf, n, false) as u32 }
    }
    fn GetDlgItemTextW(dlg: usize, id: i32, buf: usize, n: i32) -> u32 {
        let h = GetDlgItem(dlg, id);
        if h == 0 { 0 } else { get_text(h, buf, n, true) as u32 }
    }
    fn SendDlgItemMessageA(dlg: usize, id: i32, msg: u32, wp: usize, lp: isize) -> isize {
        let h = GetDlgItem(dlg, id);
        if h == 0 { 0 } else { send_ansi(h, msg, wp, lp, true) }
    }
    fn SendDlgItemMessageW(dlg: usize, id: i32, msg: u32, wp: usize, lp: isize) -> isize {
        let h = GetDlgItem(dlg, id);
        if h == 0 { 0 } else { send_ansi(h, msg, wp, lp, false) }
    }
    fn CheckDlgButton(dlg: usize, id: i32, check: u32) -> BOOL {
        let h = GetDlgItem(dlg, id);
        if h == 0 { return FALSE; }
        api::send(h, 0x00F1, check as usize, 0);
        TRUE
    }
    fn IsDlgButtonChecked(dlg: usize, id: i32) -> u32 {
        let h = GetDlgItem(dlg, id);
        if h == 0 { 0 } else { api::send(h, 0x00F0, 0, 0) as u32 }
    }
    fn SetPropA(h: usize, name: usize, data: usize) -> BOOL {
        set_prop(h, prop_key(name, false), data)
    }
    fn SetPropW(h: usize, name: usize, data: usize) -> BOOL {
        set_prop(h, prop_key(name, true), data)
    }
    fn GetPropA(h: usize, name: usize) -> usize {
        get_prop(h, prop_key(name, false))
    }
    fn GetPropW(h: usize, name: usize) -> usize {
        get_prop(h, prop_key(name, true))
    }
    fn RemovePropA(h: usize, name: usize) -> usize {
        remove_prop(h, prop_key(name, false))
    }
    fn RemovePropW(h: usize, name: usize) -> usize {
        remove_prop(h, prop_key(name, true))
    }
    fn FlashWindow(_h: usize, _invert: BOOL) -> BOOL {
        FALSE
    }
    fn FlashWindowEx(_info: usize) -> BOOL {
        FALSE
    }
    fn SetLayeredWindowAttributes(_h: usize, _key: u32, _alpha: u8, _flags: u32) -> BOOL {
        TRUE
    }
    fn SetWindowRgn(_h: usize, _rgn: usize, _redraw: BOOL) -> i32 {
        1
    }
    fn DragAcceptFiles(_h: usize, _accept: BOOL) {}
    fn GetDpiForWindow(_h: usize) -> u32 {
        96
    }
    fn GetDpiForSystem() -> u32 {
        96
    }
    fn SetThreadDpiAwarenessContext(_c: usize) -> usize {
        0
    }
    fn EnableMenuItem(_m: usize, _id: u32, _flags: u32) -> BOOL {
        0
    }

    // ---- messages ----
    fn GetMessageA(out: *mut u8, hwnd: usize, min: u32, max: u32) -> i32 {
        get_message(out, hwnd, min, max)
    }
    fn GetMessageW(out: *mut u8, hwnd: usize, min: u32, max: u32) -> i32 {
        get_message(out, hwnd, min, max)
    }
    fn PeekMessageA(out: *mut u8, hwnd: usize, min: u32, max: u32, flags: u32) -> BOOL {
        peek_message(out, hwnd, min, max, flags)
    }
    fn PeekMessageW(out: *mut u8, hwnd: usize, min: u32, max: u32, flags: u32) -> BOOL {
        peek_message(out, hwnd, min, max, flags)
    }
    fn TranslateMessage(m: *const u8) -> BOOL {
        msg::translate(&read_msg(m)) as BOOL
    }
    fn DispatchMessageA(m: *const u8) -> isize {
        msg::dispatch(&read_msg(m))
    }
    fn DispatchMessageW(m: *const u8) -> isize {
        msg::dispatch(&read_msg(m))
    }
    fn PostMessageA(h: usize, m: u32, wp: usize, lp: isize) -> BOOL {
        post_message(h, m, wp, lp)
    }
    fn PostMessageW(h: usize, m: u32, wp: usize, lp: isize) -> BOOL {
        post_message(h, m, wp, lp)
    }
    fn PostThreadMessageA(tid: u32, m: u32, wp: usize, lp: isize) -> BOOL {
        with(|g| g.post_to_thread(tid, Msg { hwnd: 0, message: m, wparam: wp, lparam: lp, time: 0, pt: (0, 0) }));
        TRUE
    }
    fn PostThreadMessageW(tid: u32, m: u32, wp: usize, lp: isize) -> BOOL {
        with(|g| g.post_to_thread(tid, Msg { hwnd: 0, message: m, wparam: wp, lparam: lp, time: 0, pt: (0, 0) }));
        TRUE
    }
    fn PostQuitMessage(code: i32) {
        let tid = current_tid();
        with(|g| {
            g.thread_state(tid).quit = Some(code);
            g.wake(tid);
        });
    }
    fn SendMessageA(h: usize, m: u32, wp: usize, lp: isize) -> isize {
        send_message(h, m, wp, lp, false)
    }
    fn SendMessageW(h: usize, m: u32, wp: usize, lp: isize) -> isize {
        send_message(h, m, wp, lp, true)
    }
    fn SendMessageTimeoutA(h: usize, m: u32, wp: usize, lp: isize, _flags: u32, _ms: u32, result: *mut usize) -> isize {
        let r = send_message(h, m, wp, lp, false);
        if !result.is_null() {
            *result = r as usize;
        }
        1
    }
    fn SendMessageTimeoutW(h: usize, m: u32, wp: usize, lp: isize, _flags: u32, _ms: u32, result: *mut usize) -> isize {
        let r = send_message(h, m, wp, lp, true);
        if !result.is_null() {
            *result = r as usize;
        }
        1
    }
    fn SendNotifyMessageA(h: usize, m: u32, wp: usize, lp: isize) -> BOOL {
        post_message(h, m, wp, lp)
    }
    fn SendNotifyMessageW(h: usize, m: u32, wp: usize, lp: isize) -> BOOL {
        post_message(h, m, wp, lp)
    }
    fn ReplyMessage(_r: isize) -> BOOL {
        TRUE
    }
    fn InSendMessage() -> BOOL {
        FALSE
    }
    fn WaitMessage() -> BOOL {
        msg::next_message(&Filter { hwnd: 0, min: 0, max: 0 }, false, true);
        TRUE
    }
    fn GetInputState() -> BOOL {
        peek_pending() as BOOL
    }
    fn GetQueueStatus(_flags: u32) -> u32 {
        if peek_pending() { 0x0008_0008 } else { 0 }
    }
    fn MsgWaitForMultipleObjects(count: u32, handles_p: *const usize, wait_all: BOOL, ms: u32, _mask: u32) -> u32 {
        msg_wait(count, handles_p, wait_all, ms)
    }
    fn MsgWaitForMultipleObjectsEx(count: u32, handles_p: *const usize, ms: u32, _mask: u32, flags: u32) -> u32 {
        msg_wait(count, handles_p, (flags & 1) as BOOL, ms)
    }
    fn DefWindowProcA(h: usize, m: u32, wp: usize, lp: isize) -> isize {
        msg::def_window_proc(h, m, wp, lp, true)
    }
    fn DefWindowProcW(h: usize, m: u32, wp: usize, lp: isize) -> isize {
        msg::def_window_proc(h, m, wp, lp, false)
    }
    fn CallWindowProcA(prev: usize, h: usize, m: u32, wp: usize, lp: isize) -> isize {
        with_ansi(true, || call_proc(prev, h, m, wp, lp))
    }
    fn CallWindowProcW(prev: usize, h: usize, m: u32, wp: usize, lp: isize) -> isize {
        with_ansi(false, || call_proc(prev, h, m, wp, lp))
    }
    fn SetTimer(h: usize, id: usize, ms: u32, proc_: usize) -> usize {
        msg::set_timer(h, id, ms, proc_)
    }
    fn KillTimer(h: usize, id: usize) -> BOOL {
        msg::kill_timer(h, id) as BOOL
    }
    fn GetMessageTime() -> i32 {
        LAST_MSG.0.load(Ordering::Relaxed)
    }
    fn GetMessagePos() -> u32 {
        let (x, y) = (LAST_MSG.1.load(Ordering::Relaxed), LAST_MSG.2.load(Ordering::Relaxed));
        (x as u16 as u32) | ((y as u16 as u32) << 16)
    }
    fn RegisterWindowMessageA(name: usize) -> u32 {
        register_message(&text_arg(name, false))
    }
    fn RegisterWindowMessageW(name: usize) -> u32 {
        register_message(&text_arg(name, true))
    }
    fn IsDialogMessageA(_dlg: usize, _m: *mut u8) -> BOOL {
        FALSE
    }
    fn IsDialogMessageW(_dlg: usize, _m: *mut u8) -> BOOL {
        FALSE
    }
    fn TranslateAcceleratorA(_h: usize, _accel: usize, _m: *mut u8) -> i32 {
        0
    }
    fn TranslateAcceleratorW(_h: usize, _accel: usize, _m: *mut u8) -> i32 {
        0
    }

    // ---- input ----
    fn GetKeyState(vk: i32) -> i16 {
        with(|g| {
            let i = (vk & 0xff) as usize;
            let down = (g.keys[i] & 0x80) as i16;
            let toggle = matches!(i, 0x14 | 0x90 | 0x91) && g.toggles[i];
            (down << 8) | toggle as i16
        })
    }
    fn GetAsyncKeyState(vk: i32) -> i16 {
        with(|g| ((g.keys[(vk & 0xff) as usize] & 0x80) as i16) << 8)
    }
    fn GetKeyboardState(out: *mut u8) -> BOOL {
        with(|g| {
            for i in 0..256 {
                *out.add(i) = g.keys[i] | (matches!(i, 0x14 | 0x90 | 0x91) && g.toggles[i]) as u8;
            }
        });
        TRUE
    }
    fn SetKeyboardState(_p: *const u8) -> BOOL {
        TRUE
    }
    fn GetKeyboardLayout(_tid: u32) -> usize {
        0x0409_0409
    }
    fn GetKeyboardType(kind: i32) -> i32 {
        match kind {
            0 => 4,
            2 => 12,
            _ => 0,
        }
    }
    fn MapVirtualKeyA(code: u32, kind: u32) -> u32 {
        map_virtual_key(code, kind)
    }
    fn MapVirtualKeyW(code: u32, kind: u32) -> u32 {
        map_virtual_key(code, kind)
    }
    fn VkKeyScanA(ch: u8) -> i16 {
        vk_key_scan(ch as char)
    }
    fn VkKeyScanW(ch: u16) -> i16 {
        vk_key_scan(char::from_u32(ch as u32).unwrap_or(' '))
    }
    fn GetCursorPos(p: *mut i32) -> BOOL {
        let (x, y) = get_pointer();
        *p = x;
        *p.add(1) = y;
        TRUE
    }
    fn SetCursorPos(_x: i32, _y: i32) -> BOOL {
        TRUE
    }
    fn ShowCursor(show: BOOL) -> i32 {
        static COUNT: AtomicI32 = AtomicI32::new(0);
        if show != 0 { COUNT.fetch_add(1, Ordering::Relaxed) + 1 } else { COUNT.fetch_sub(1, Ordering::Relaxed) - 1 }
    }
    fn ClipCursor(_r: *const i32) -> BOOL {
        TRUE
    }
    fn SetCursor(h: usize) -> usize {
        let kind = api::cursor_kind(h);
        with(|g| {
            let target = if g.focus != 0 { g.focus } else { g.active };
            g.set_cursor_for(target, kind);
        });
        0
    }
    fn GetCursor() -> usize {
        stub_handle(1, 32512)
    }
    fn LoadCursorA(inst: usize, name: usize) -> usize {
        load_cursor(inst, name)
    }
    fn LoadCursorW(inst: usize, name: usize) -> usize {
        load_cursor(inst, name)
    }
    fn LoadIconA(inst: usize, name: usize) -> usize {
        load_icon(inst, name, false)
    }
    fn LoadIconW(inst: usize, name: usize) -> usize {
        load_icon(inst, name, true)
    }
    fn LoadBitmapA(inst: usize, name: usize) -> usize {
        load_bitmap(inst, name, false)
    }
    fn LoadBitmapW(inst: usize, name: usize) -> usize {
        load_bitmap(inst, name, true)
    }
    fn LoadImageA(inst: usize, name: usize, kind: u32, cx: i32, cy: i32, flags: u32) -> usize {
        load_image(inst, name, kind, cx, cy, flags, false)
    }
    fn LoadImageW(inst: usize, name: usize, kind: u32, cx: i32, cy: i32, flags: u32) -> usize {
        load_image(inst, name, kind, cx, cy, flags, true)
    }
    fn LoadStringA(inst: usize, id: u32, buf: *mut u8, max: i32) -> i32 {
        load_string_a(inst, id, buf, max)
    }
    fn LoadStringW(inst: usize, id: u32, buf: *mut u16, max: i32) -> i32 {
        load_string_w(inst, id, buf, max)
    }
    fn DestroyCursor(_h: usize) -> BOOL {
        TRUE
    }
    fn DestroyIcon(h: usize) -> BOOL {
        with(|g| {
            if matches!(g.objects.get(&h), Some(Object::Icon(_))) {
                g.objects.remove(&h);
            }
        });
        TRUE
    }
    fn CopyIcon(h: usize) -> usize {
        with(|g| match g.objects.get(&h) {
            Some(Object::Icon(i)) => {
                let copy = i.clone();
                g.add_object(Object::Icon(copy))
            }
            _ => h,
        })
    }
    fn TrackMouseEvent(t: *mut u8) -> BOOL {
        // TRACKMOUSEEVENT { cbSize, dwFlags, hwndTrack, dwHoverTime }
        if t.is_null() {
            return FALSE;
        }
        let flags = *(t.add(4) as *const u32);
        let h = *(t.add(8) as *const u64) as usize;
        with(|g| {
            if let Some(w) = g.windows.get_mut(&h) {
                if flags & 0x8000_0000 != 0 {
                    w.track_leave = false;
                } else if flags & 2 != 0 {
                    w.track_leave = true;
                }
            }
        });
        TRUE
    }
    fn GetDoubleClickTime() -> u32 {
        500
    }
    fn GetCaretBlinkTime() -> u32 {
        530
    }
    fn CreateCaret(_h: usize, _bmp: usize, _w: i32, _hh: i32) -> BOOL {
        TRUE
    }
    fn DestroyCaret() -> BOOL {
        TRUE
    }
    fn ShowCaret(_h: usize) -> BOOL {
        TRUE
    }
    fn HideCaret(_h: usize) -> BOOL {
        TRUE
    }
    fn SetCaretPos(_x: i32, _y: i32) -> BOOL {
        TRUE
    }
    fn RegisterHotKey(_h: usize, _id: i32, _mods: u32, _vk: u32) -> BOOL {
        FALSE
    }
    fn UnregisterHotKey(_h: usize, _id: i32) -> BOOL {
        TRUE
    }
    fn SetWindowsHookExA(_id: i32, _proc: usize, _inst: usize, _tid: u32) -> usize {
        0x5151
    }
    fn SetWindowsHookExW(_id: i32, _proc: usize, _inst: usize, _tid: u32) -> usize {
        0x5151
    }
    fn UnhookWindowsHookEx(_h: usize) -> BOOL {
        TRUE
    }
    fn CallNextHookEx(_h: usize, _code: i32, _wp: usize, _lp: isize) -> isize {
        0
    }

    // ---- system ----
    fn GetSystemMetrics(index: i32) -> i32 {
        system_metric(index)
    }
    fn GetSysColor(index: i32) -> u32 {
        crate::gui::surface::to_colorref(crate::gui::gdi::sys_color(index))
    }
    fn GetSysColorBrush(index: i32) -> usize {
        with(|g| g.sys_brush(index))
    }
    fn SetSysColors(_n: i32, _idx: *const i32, _vals: *const u32) -> BOOL {
        TRUE
    }
    fn SystemParametersInfoA(action: u32, param: u32, out: *mut u8, _flags: u32) -> BOOL {
        system_parameters(action, param, out)
    }
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut u8, _flags: u32) -> BOOL {
        system_parameters(action, param, out)
    }
    fn MonitorFromWindow(_h: usize, _flags: u32) -> usize {
        0x10001
    }
    fn MonitorFromPoint(_pt: i64, _flags: u32) -> usize {
        0x10001
    }
    fn MonitorFromRect(_r: *const i32, _flags: u32) -> usize {
        0x10001
    }
    fn GetMonitorInfoA(_m: usize, info: *mut u8) -> BOOL {
        fill_monitor_info(info, false)
    }
    fn GetMonitorInfoW(_m: usize, info: *mut u8) -> BOOL {
        fill_monitor_info(info, true)
    }
    fn EnumDisplayMonitors(hdc: usize, _clip: *const i32, cb: usize, data: isize) -> BOOL {
        let (w, h) = with(|g| g.screen_size());
        let rect = [0i32, 0, w, h];
        type Cb = unsafe extern "win64" fn(usize, usize, *const i32, isize) -> BOOL;
        let f: Cb = std::mem::transmute(cb);
        f(0x10001, hdc, rect.as_ptr(), data);
        TRUE
    }
    fn MessageBoxA(owner: usize, text: usize, caption: usize, flags: u32) -> i32 {
        crate::gui::msgbox::message_box(owner, &text_arg(text, false), &text_arg(caption, false), flags)
    }
    fn MessageBoxW(owner: usize, text: usize, caption: usize, flags: u32) -> i32 {
        crate::gui::msgbox::message_box(owner, &text_arg(text, true), &text_arg(caption, true), flags)
    }
    fn MessageBoxExA(owner: usize, text: usize, caption: usize, flags: u32, _lang: u16) -> i32 {
        crate::gui::msgbox::message_box(owner, &text_arg(text, false), &text_arg(caption, false), flags)
    }
    fn MessageBoxExW(owner: usize, text: usize, caption: usize, flags: u32, _lang: u16) -> i32 {
        crate::gui::msgbox::message_box(owner, &text_arg(text, true), &text_arg(caption, true), flags)
    }
    fn MessageBeep(_kind: u32) -> BOOL {
        with(|g| {
            if let Some(b) = g.backend.clone() {
                b.bell();
            }
        });
        TRUE
    }

    // ---- painting ----
    fn BeginPaint(h: usize, ps: *mut u8) -> usize {
        api::begin_paint(h, ps)
    }
    fn EndPaint(h: usize, ps: *const u8) -> BOOL {
        api::end_paint(h, ps) as BOOL
    }
    fn GetDC(h: usize) -> usize {
        api::get_dc(h)
    }
    fn GetWindowDC(h: usize) -> usize {
        api::get_dc(h)
    }
    fn GetDCEx(h: usize, _clip: usize, _flags: u32) -> usize {
        api::get_dc(h)
    }
    fn ReleaseDC(h: usize, dc: usize) -> i32 {
        api::release_dc(h, dc) as i32
    }
    fn InvalidateRect(h: usize, r: *const i32, erase: BOOL) -> BOOL {
        let rect = if r.is_null() { None } else { Some(rect_from(r)) };
        with(|g| {
            if h == 0 {
                let all: Vec<usize> = g.windows.values().filter(|w| w.parent == 0).map(|w| w.hwnd).collect();
                for t in all {
                    g.invalidate(t, None, erase != 0);
                }
            } else {
                g.invalidate(h, rect, erase != 0);
            }
        });
        TRUE
    }
    fn ValidateRect(h: usize, r: *const i32) -> BOOL {
        let rect = if r.is_null() { None } else { Some(rect_from(r)) };
        with(|g| g.validate(h, rect));
        TRUE
    }
    fn InvalidateRgn(h: usize, rgn: usize, erase: BOOL) -> BOOL {
        let bounds = with(|g| match g.objects.get(&rgn) {
            Some(Object::Region(list)) => Some(list.iter().fold(Rect::default(), |a, r| a.union(r))),
            _ => None,
        });
        with(|g| g.invalidate(h, bounds, erase != 0));
        TRUE
    }
    fn ValidateRgn(h: usize, _rgn: usize) -> BOOL {
        with(|g| g.validate(h, None));
        TRUE
    }
    fn RedrawWindow(h: usize, r: *const i32, _rgn: usize, flags: u32) -> BOOL {
        api::redraw_window(h, if r.is_null() { None } else { Some(rect_from(r)) }, flags) as BOOL
    }
    fn GetUpdateRect(h: usize, out: *mut i32, erase: BOOL) -> BOOL {
        let u = with(|g| g.windows.get(&h).map(|w| w.update)).unwrap_or_default();
        if !out.is_null() {
            rect_to(out, u);
        }
        if erase != 0 && !u.is_empty() {
            let _ = ();
        }
        !u.is_empty() as BOOL
    }
    fn GetUpdateRgn(h: usize, rgn: usize, _erase: BOOL) -> i32 {
        let u = with(|g| g.windows.get(&h).map(|w| w.update)).unwrap_or_default();
        with(|g| {
            if let Some(Object::Region(list)) = g.objects.get_mut(&rgn) {
                *list = if u.is_empty() { vec![] } else { vec![u] };
            }
        });
        if u.is_empty() { 1 } else { 2 }
    }
    fn LockWindowUpdate(_h: usize) -> BOOL {
        TRUE
    }
    fn FillRect(hdc: usize, r: *const i32, brush: usize) -> i32 {
        let rect = rect_from(r);
        with(|g| {
            let b = g.brush_arg(brush);
            g.fill_rect(hdc, rect, &b)
        }) as i32
    }
    fn FrameRect(hdc: usize, r: *const i32, brush: usize) -> i32 {
        let rect = rect_from(r).normalized();
        with(|g| {
            let b = g.brush_arg(brush);
            let crate::gui::surface::Brush::Solid(c) = b else { return 0 };
            g.draw(hdc, |cv, _| cv.frame_rect(rect, c, 1));
            1
        })
    }
    fn InvertRect(hdc: usize, r: *const i32) -> BOOL {
        let rect = rect_from(r).normalized();
        with(|g| g.pat_blt(hdc, rect.l, rect.t, rect.width(), rect.height(), 0x00550009)) as BOOL
    }
    fn DrawFocusRect(hdc: usize, r: *const i32) -> BOOL {
        let rect = rect_from(r).normalized();
        with(|g| {
            g.draw(hdc, |cv, _| {
                cv.rop2 = 7;
                let dotted = crate::gui::surface::Pen { style: crate::gui::surface::PenStyle::Dot, width: 1, color: 0xffffff };
                cv.line(rect.l, rect.t, rect.r - 1, rect.t, &dotted);
                cv.line(rect.r - 1, rect.t, rect.r - 1, rect.b - 1, &dotted);
                cv.line(rect.r - 1, rect.b - 1, rect.l, rect.b - 1, &dotted);
                cv.line(rect.l, rect.b - 1, rect.l, rect.t, &dotted);
            });
        });
        TRUE
    }
    fn DrawEdge(hdc: usize, r: *mut i32, edge: u32, flags: u32) -> BOOL {
        draw_edge(hdc, r, edge, flags)
    }
    fn DrawTextA(hdc: usize, text: usize, n: i32, r: *mut i32, flags: u32) -> i32 {
        draw_text(hdc, text, n, r, flags, false)
    }
    fn DrawTextW(hdc: usize, text: usize, n: i32, r: *mut i32, flags: u32) -> i32 {
        draw_text(hdc, text, n, r, flags, true)
    }
    fn DrawTextExA(hdc: usize, text: usize, n: i32, r: *mut i32, flags: u32, _p: usize) -> i32 {
        draw_text(hdc, text, n, r, flags, false)
    }
    fn DrawTextExW(hdc: usize, text: usize, n: i32, r: *mut i32, flags: u32, _p: usize) -> i32 {
        draw_text(hdc, text, n, r, flags, true)
    }
    fn DrawIcon(hdc: usize, x: i32, y: i32, icon: usize) -> BOOL {
        with(|g| g.draw_icon(hdc, x, y, icon, 0, 0)) as BOOL
    }
    fn DrawIconEx(hdc: usize, x: i32, y: i32, icon: usize, w: i32, h: i32, _step: u32, _brush: usize, _flags: u32) -> BOOL {
        with(|g| g.draw_icon(hdc, x, y, icon, w, h)) as BOOL
    }

    // ---- scroll bars ----
    fn SetScrollInfo(h: usize, bar: i32, info: *const u8, redraw: BOOL) -> i32 {
        api::set_scroll_info(h, bar, info, redraw != 0)
    }
    fn GetScrollInfo(h: usize, bar: i32, info: *mut u8) -> BOOL {
        api::get_scroll_info(h, bar, info) as BOOL
    }
    fn SetScrollPos(h: usize, bar: i32, pos: i32, redraw: BOOL) -> i32 {
        api::set_scroll_pos(h, bar, pos, redraw != 0)
    }
    fn GetScrollPos(h: usize, bar: i32) -> i32 {
        with(|g| g.windows.get(&h).and_then(|w| w.sb.get(bar).map(|b| b.pos))).unwrap_or(0)
    }
    fn SetScrollRange(h: usize, bar: i32, min: i32, max: i32, redraw: BOOL) -> BOOL {
        api::set_scroll_range(h, bar, min, max, redraw != 0) as BOOL
    }
    fn GetScrollRange(h: usize, bar: i32, min: *mut i32, max: *mut i32) -> BOOL {
        match with(|g| g.windows.get(&h).and_then(|w| w.sb.get(bar).copied())) {
            Some(b) => {
                if !min.is_null() {
                    *min = b.min;
                }
                if !max.is_null() {
                    *max = b.max;
                }
                TRUE
            }
            None => FALSE,
        }
    }
    fn ShowScrollBar(h: usize, bar: i32, show: BOOL) -> BOOL {
        api::show_scroll_bar(h, bar, show != 0) as BOOL
    }
    fn EnableScrollBar(h: usize, bar: i32, arrows: u32) -> BOOL {
        api::enable_scroll_bar(h, bar, arrows) as BOOL
    }
    fn ScrollWindow(h: usize, dx: i32, dy: i32, area: *const i32, _clip: *const i32) -> BOOL {
        api::scroll_window(h, dx, dy, if area.is_null() { None } else { Some(rect_from(area)) }) as BOOL
    }
    fn ScrollWindowEx(h: usize, dx: i32, dy: i32, area: *const i32, _clip: *const i32, _rgn: usize, update: *mut i32, _flags: u32) -> i32 {
        let ok = api::scroll_window(h, dx, dy, if area.is_null() { None } else { Some(rect_from(area)) });
        if !update.is_null() {
            rect_to(update, with(|g| g.windows.get(&h).map(|w| w.update).unwrap_or_default()));
        }
        if ok { 1 } else { 0 }
    }

    // ---- rectangles ----
    fn SetRect(r: *mut i32, l: i32, t: i32, rr: i32, b: i32) -> BOOL {
        rect_to(r, Rect::new(l, t, rr, b));
        TRUE
    }
    fn SetRectEmpty(r: *mut i32) -> BOOL {
        rect_to(r, Rect::default());
        TRUE
    }
    fn CopyRect(dst: *mut i32, src: *const i32) -> BOOL {
        rect_to(dst, rect_from(src));
        TRUE
    }
    fn InflateRect(r: *mut i32, dx: i32, dy: i32) -> BOOL {
        let v = rect_from(r);
        rect_to(r, Rect::new(v.l - dx, v.t - dy, v.r + dx, v.b + dy));
        TRUE
    }
    fn OffsetRect(r: *mut i32, dx: i32, dy: i32) -> BOOL {
        rect_to(r, rect_from(r).offset(dx, dy));
        TRUE
    }
    fn IntersectRect(dst: *mut i32, a: *const i32, b: *const i32) -> BOOL {
        let r = rect_from(a).intersect(&rect_from(b));
        rect_to(dst, r);
        !r.is_empty() as BOOL
    }
    fn UnionRect(dst: *mut i32, a: *const i32, b: *const i32) -> BOOL {
        let r = rect_from(a).union(&rect_from(b));
        rect_to(dst, r);
        !r.is_empty() as BOOL
    }
    fn SubtractRect(dst: *mut i32, a: *const i32, b: *const i32) -> BOOL {
        let (ra, rb) = (rect_from(a), rect_from(b));
        let parts = crate::gui::gdi::rect_subtract(ra, rb);
        let r = if parts.len() == 1 { parts[0] } else { ra };
        rect_to(dst, r);
        !r.is_empty() as BOOL
    }
    fn IsRectEmpty(r: *const i32) -> BOOL {
        rect_from(r).is_empty() as BOOL
    }
    fn PtInRect(r: *const i32, pt: i64) -> BOOL {
        let (x, y) = point_of(pt);
        rect_from(r).contains(x, y) as BOOL
    }
    fn EqualRect(a: *const i32, b: *const i32) -> BOOL {
        (rect_from(a) == rect_from(b)) as BOOL
    }

    // ---- clipboard ----
    fn OpenClipboard(_h: usize) -> BOOL {
        TRUE
    }
    fn CloseClipboard() -> BOOL {
        TRUE
    }
    fn EmptyClipboard() -> BOOL {
        with(|g| {
            if let Some(b) = g.ensure_backend() {
                b.set_clipboard("");
            }
        });
        TRUE
    }
    fn IsClipboardFormatAvailable(fmt: u32) -> BOOL {
        if fmt != 1 && fmt != 13 {
            return FALSE;
        }
        with(|g| g.ensure_backend().is_some_and(|b| !b.get_clipboard().is_empty())) as BOOL
    }
    fn CountClipboardFormats() -> i32 {
        with(|g| g.ensure_backend().is_some_and(|b| !b.get_clipboard().is_empty())) as i32
    }
    fn SetClipboardData(fmt: u32, data: usize) -> usize {
        if data == 0 || (fmt != 1 && fmt != 13) {
            return 0;
        }
        let text = if fmt == 13 { w_ptr_to_string(data as *const u16) } else { a_to_string(data as *const u8) };
        with(|g| {
            if let Some(b) = g.ensure_backend() {
                b.set_clipboard(&text);
            }
        });
        data
    }
    fn GetClipboardData(fmt: u32) -> usize {
        if fmt != 1 && fmt != 13 {
            return 0;
        }
        let text = with(|g| g.ensure_backend().map(|b| b.get_clipboard())).unwrap_or_default();
        if text.is_empty() {
            return 0;
        }
        // a GlobalAlloc-style block the program may lock and read
        let bytes: Vec<u8> = if fmt == 13 { to_wide_nul(&text).iter().flat_map(|u| u.to_le_bytes()).collect() } else { [text.as_bytes(), &[0]].concat() };
        let p = libc::malloc(bytes.len()) as *mut u8;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        p as usize
    }
    fn RegisterClipboardFormatA(_name: usize) -> u32 {
        0xC100
    }
    fn RegisterClipboardFormatW(_name: usize) -> u32 {
        0xC100
    }

    // ---- menus (structure only; there is no menu bar yet) ----
    fn CreateMenu() -> usize {
        with(|g| g.add_object(Object::Stub(0x4D45)))
    }
    fn CreatePopupMenu() -> usize {
        with(|g| g.add_object(Object::Stub(0x4D45)))
    }
    fn DestroyMenu(m: usize) -> BOOL {
        with(|g| g.objects.remove(&m).is_some()) as BOOL
    }
    fn AppendMenuA(_m: usize, _flags: u32, _id: usize, _item: usize) -> BOOL {
        TRUE
    }
    fn AppendMenuW(_m: usize, _flags: u32, _id: usize, _item: usize) -> BOOL {
        TRUE
    }
    fn InsertMenuA(_m: usize, _pos: u32, _flags: u32, _id: usize, _item: usize) -> BOOL {
        TRUE
    }
    fn InsertMenuW(_m: usize, _pos: u32, _flags: u32, _id: usize, _item: usize) -> BOOL {
        TRUE
    }
    fn SetMenu(_h: usize, _m: usize) -> BOOL {
        TRUE
    }
    fn GetMenu(_h: usize) -> usize {
        0
    }
    fn GetSubMenu(_m: usize, _pos: i32) -> usize {
        0
    }
    fn GetMenuItemCount(_m: usize) -> i32 {
        0
    }
    fn DrawMenuBar(_h: usize) -> BOOL {
        TRUE
    }
    fn CheckMenuItem(_m: usize, _id: u32, _flags: u32) -> u32 {
        0
    }
    fn TrackPopupMenu(_m: usize, _flags: u32, _x: i32, _y: i32, _r: i32, _h: usize, _rect: usize) -> BOOL {
        0
    }
    fn TrackPopupMenuEx(_m: usize, _flags: u32, _x: i32, _y: i32, _h: usize, _p: usize) -> BOOL {
        0
    }
}

/// Window properties live beside the library's own bookkeeping, under a prefix that cannot clash.
fn prop_key(name: usize, wide: bool) -> String {
    if name < 0x10000 {
        format!("prop:#{name}")
    } else {
        format!("prop:{}", unsafe { text_arg(name, wide) }.to_lowercase())
    }
}

fn set_prop(h: usize, key: String, data: usize) -> BOOL {
    with(|g| match g.windows.get_mut(&h) {
        Some(w) => {
            w.props.insert(key, data);
            TRUE
        }
        None => FALSE,
    })
}

fn get_prop(h: usize, key: String) -> usize {
    with(|g| g.windows.get(&h).and_then(|w| w.props.get(&key).copied()).unwrap_or(0))
}

fn remove_prop(h: usize, key: String) -> usize {
    with(|g| g.windows.get_mut(&h).and_then(|w| w.props.remove(&key)).unwrap_or(0))
}

fn class_long(h: usize, index: i32) -> usize {
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return 0 };
        match index {
            -10 => w.class.background,
            -12 => w.class.cursor,
            -14 => w.class.icon,
            -16 => w.class.hinstance,
            -18 => w.class.wnd_extra as usize,
            -20 => w.class.cls_extra as usize,
            -24 => w.class.wndproc,
            -26 => w.class.style as usize,
            -32 => w.class.atom as usize,
            _ => 0,
        }
    })
}

unsafe fn adjust_rect(r: *mut i32, style: u32, menu: BOOL, ex: u32) -> BOOL {
    // scroll bars are not part of the calculation, as on Windows
    let style = style & !(gui::WS_VSCROLL | gui::WS_HSCROLL);
    let i = gui::wm::insets(style, ex, menu != 0, style & WS_CHILD == 0);
    let v = rect_from(r);
    rect_to(r, Rect::new(v.l - i.l, v.t - i.t, v.r + i.r, v.b + i.b));
    TRUE
}

fn register_message(name: &str) -> u32 {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static MESSAGES: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);
    let mut g = MESSAGES.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    let next = 0xC000 + map.len() as u32;
    *map.entry(name.to_lowercase()).or_insert(next)
}

fn map_virtual_key(code: u32, kind: u32) -> u32 {
    match kind {
        0 => match code {
            0x41..=0x5A => 0x1E + [0, 24, 22, 8, 6, 9, 10, 11, 23, 12, 13, 14, 26, 25, 24, 27, 4, 7, 5, 8, 10, 21, 5, 23, 20, 1][(code - 0x41) as usize].min(0),
            0x0D => 0x1C,
            0x1B => 0x01,
            0x20 => 0x39,
            _ => 0,
        },
        2 => match code {
            0x41..=0x5A => code + 0x20 - 0x20,
            0x30..=0x39 | 0x20 => code,
            _ => 0,
        },
        _ => 0,
    }
}

fn vk_key_scan(c: char) -> i16 {
    match c {
        'a'..='z' => c.to_ascii_uppercase() as i16,
        'A'..='Z' => (c as i16) | 0x100,
        '0'..='9' => c as i16,
        ' ' => 0x20,
        '\r' | '\n' => 0x0D,
        '\t' => 0x09,
        _ => -1,
    }
}

unsafe fn system_parameters(action: u32, _param: u32, out: *mut u8) -> BOOL {
    match action {
        0x0030 => {
            let (w, h) = with(|g| g.screen_size());
            rect_to(out as *mut i32, Rect::new(0, 0, w, h - 40));
            TRUE
        }
        0x0029 => {
            // NONCLIENTMETRICSW: fill the message, menu and status fonts with the system UI font
            std::ptr::write_bytes(out.add(4), 0, 500);
            *(out as *mut u32) = 504;
            *(out.add(4) as *mut i32) = 1;
            *(out.add(8) as *mut i32) = 17;
            *(out.add(12) as *mut i32) = 17;
            *(out.add(20) as *mut i32) = crate::gui::wm::CAPTION_H;
            for off in [24usize, 124, 224, 316, 408] {
                let f = out.add(off);
                *(f as *mut i32) = -12;
                *(f.add(16) as *mut i32) = 400;
                let face = to_wide_nul("Segoe UI");
                std::ptr::copy_nonoverlapping(face.as_ptr(), f.add(28) as *mut u16, face.len());
            }
            TRUE
        }
        0x0001 | 0x0004 | 0x0006 | 0x0008 | 0x0010 | 0x0011 | 0x0012 | 0x0014 | 0x0015 | 0x0016 | 0x0017 | 0x0019 | 0x001A | 0x001B | 0x001C | 0x0026 | 0x0034 | 0x0049 | 0x004A | 0x0058 | 0x005A | 0x005C | 0x005F | 0x0067 | 0x0068 | 0x006A | 0x0070 | 0x0072 | 0x0074 | 0x0076 | 0x0078 | 0x007A => {
            if !out.is_null() {
                *(out as *mut u32) = 0;
            }
            TRUE
        }
        0x0048 => {
            // SPI_GETWHEELSCROLLLINES
            if !out.is_null() {
                *(out as *mut u32) = 3;
            }
            TRUE
        }
        _ => FALSE,
    }
}

unsafe fn fill_monitor_info(info: *mut u8, wide: bool) -> BOOL {
    let (w, h) = with(|g| g.screen_size());
    rect_to(info.add(4) as *mut i32, Rect::new(0, 0, w, h));
    rect_to(info.add(20) as *mut i32, Rect::new(0, 0, w, h - 40));
    *(info.add(36) as *mut u32) = 1;
    if *(info as *const u32) >= 72 {
        let name = if wide { to_wide_nul("\\\\.\\DISPLAY1").iter().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>() } else { b"\\\\.\\DISPLAY1\0".to_vec() };
        std::ptr::copy_nonoverlapping(name.as_ptr(), info.add(40), name.len());
    }
    TRUE
}

unsafe fn draw_text(hdc: usize, text: usize, n: i32, r: *mut i32, flags: u32, wide: bool) -> i32 {
    let s = if text == 0 {
        String::new()
    } else if n < 0 {
        text_arg(text, wide)
    } else if wide {
        w_to_string(wstr_n(text as *const u16, n))
    } else {
        String::from_utf8_lossy(astr_n(text as *const u8, n)).into_owned()
    };
    let mut rect = rect_from(r);
    let h = with(|g| g.draw_text(hdc, &s, &mut rect, flags));
    if flags & 0x400 != 0 {
        rect_to(r, rect);
    }
    h
}

/// `DrawEdge`: raised / sunken / etched bevels in the system 3-D colours.
unsafe fn draw_edge(hdc: usize, r: *mut i32, edge: u32, flags: u32) -> BOOL {
    use crate::gui::gdi::sys_color;
    let rect = rect_from(r);
    let (hi, lt, sh, dk) = (sys_color(20), sys_color(22), sys_color(16), sys_color(21));
    let (tl, br): (u32, u32) = match edge & 0xf {
        5 | 1 => (hi, dk),   // EDGE_RAISED: BDR_RAISEDOUTER|INNER
        10 | 2 => (dk, hi),  // EDGE_SUNKEN
        6 => (sh, hi),       // EDGE_ETCHED
        9 => (hi, sh),       // EDGE_BUMP
        _ => (hi, dk),
    };
    let width = if edge & 0xf == 5 || edge & 0xf == 10 { 2 } else { 1 };
    with(|g| {
        g.draw(hdc, |cv, _| {
            for i in 0..width {
                let (l, t, rr, b) = (rect.l + i, rect.t + i, rect.r - i, rect.b - i);
                let (top_left, bottom_right) = if i == 0 { (tl, br) } else { (lt, sh) };
                cv.fill_rect(Rect::new(l, t, rr, t + 1), top_left);
                cv.fill_rect(Rect::new(l, t, l + 1, b), top_left);
                cv.fill_rect(Rect::new(l, b - 1, rr, b), bottom_right);
                cv.fill_rect(Rect::new(rr - 1, t, rr, b), bottom_right);
            }
        });
    });
    if flags & 0x2000 != 0 {
        rect_to(r, Rect::new(rect.l + width, rect.t + width, rect.r - width, rect.b - width));
    }
    let _ = (Target::Screen, Cursor::Arrow, current_ansi());
    TRUE
}
