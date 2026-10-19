//! comctl32: the library functions around the common controls. The controls themselves are window
//! classes (`msctls_progress32`, `SysListView32`, ...) registered by the window system, so a program
//! gets them whether or not it calls `InitCommonControls`.

use crate::gui::api::{self, send_ansi};
use crate::gui::gdi::Object;
use crate::gui::imagelist::{ImageList, CLR_NONE, ILD_NORMAL};
use crate::gui::msg::call_proc;
use crate::gui::res::{self, Key};
use crate::gui::surface::Surface;
use crate::gui::wm::{ClassRef, CreateArgs};
use crate::gui::{with, WS_CHILD};
use crate::runtime::strings::*;
use crate::win::*;
use std::cell::RefCell;

// ---- window subclassing -----------------------------------------------------------------------------

thread_local! {
    /// Subclass procedures being run on this thread: (window, index of the next one to call).
    static CHAIN: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
}

type SubclassProc = unsafe extern "win64" fn(usize, u32, usize, isize, usize, usize) -> isize;

/// The window procedure of a subclassed window: runs the chain from its first entry.
pub unsafe extern "win64" fn subclass_dispatch(hwnd: usize, msg: u32, wp: usize, lp: isize) -> isize {
    run_chain(hwnd, msg, wp, lp, 0)
}

unsafe fn run_chain(hwnd: usize, msg: u32, wp: usize, lp: isize, index: usize) -> isize {
    let (entry, orig) = with(|g| g.windows.get(&hwnd).map_or((None, 0), |w| (w.subclass.get(index).copied(), w.orig_proc)));
    match entry {
        Some((proc_, id, refdata)) => {
            CHAIN.with(|c| c.borrow_mut().push((hwnd, index + 1)));
            let f: SubclassProc = std::mem::transmute(proc_);
            let r = f(hwnd, msg, wp, lp, id, refdata);
            CHAIN.with(|c| c.borrow_mut().pop());
            r
        }
        None => call_proc(orig, hwnd, msg, wp, lp),
    }
}

// ---- helpers --------------------------------------------------------------------------------------------

fn bitmap_copy(g: &crate::gui::Gui, h: usize) -> Option<Surface> {
    match g.objects.get(&h)? {
        Object::Bitmap(b) => Some(b.surf.resized(b.surf.w, b.surf.h, 0)),
        _ => None,
    }
}

fn icon_of(g: &crate::gui::Gui, h: usize) -> Option<std::sync::Arc<res::IconImage>> {
    match g.objects.get(&h)? {
        Object::Icon(i) => Some(i.clone()),
        _ => None,
    }
}

unsafe fn create_child(class: &str, title: &str, style: u32, parent: usize, id: usize) -> usize {
    api::create_window_ex(CreateArgs { ex_style: 0, class: ClassRef::Name(class.into()), title: title.into(), style: style | WS_CHILD, x: 0, y: 0, w: 0, h: 0, parent, menu: id, hinstance: 0, param: 0, ansi: false })
}

/// `TaskDialog` as a message box: the main instruction and the content form the text, the common buttons the button set.
unsafe fn task_dialog(owner: usize, title: usize, instruction: usize, content: usize, buttons: u32, icon: usize, button: *mut i32) -> i32 {
    let arg = |p: usize| if p == 0 { String::new() } else if p < 0x10000 { String::new() } else { w_ptr_to_string(p as *const u16) };
    let text = match (arg(instruction), arg(content)) {
        (a, b) if !a.is_empty() && !b.is_empty() => format!("{a}\n\n{b}"),
        (a, b) => format!("{a}{b}"),
    };
    // TDCBF_OK 1, YES 2, NO 4, CANCEL 8, RETRY 0x10, CLOSE 0x20
    let mb = match buttons & 0x1F {
        0 | 1 => 0,
        9 => 1,
        6 => 4,
        14 => 3,
        24 => 5,
        8 => 1,
        _ => 0,
    };
    let kind = match icon as u16 {
        0xFFFF => 0x30,
        0xFFFE => 0x10,
        0xFFFD => 0x40,
        _ => 0,
    };
    let caption = if title == 0 { "NLE".to_string() } else { arg(title) };
    let r = crate::gui::msgbox::message_box(owner, &text, &caption, mb | kind);
    if !button.is_null() {
        *button = r;
    }
    0
}

crate::shims! { "comctl32";
    fn InitCommonControls() {}
    fn InitCommonControlsEx(_icc: *const u8) -> BOOL {
        TRUE
    }

    // ---- creation helpers ----
    fn CreateStatusWindowA(style: u32, text: usize, parent: usize, id: usize) -> usize {
        create_child("msctls_statusbar32", &a_to_string(text as *const u8), style, parent, id)
    }
    fn CreateStatusWindowW(style: u32, text: usize, parent: usize, id: usize) -> usize {
        create_child("msctls_statusbar32", &w_ptr_to_string(text as *const u16), style, parent, id)
    }
    fn CreateUpDownControl(style: u32, x: i32, y: i32, cx: i32, cy: i32, parent: usize, id: usize, _inst: usize, buddy: usize, upper: i32, lower: i32, pos: i32) -> usize {
        let h = api::create_window_ex(CreateArgs { ex_style: 0, class: ClassRef::Name("msctls_updown32".into()), title: String::new(), style: style | WS_CHILD, x, y, w: cx, h: cy, parent, menu: id, hinstance: 0, param: 0, ansi: false });
        if h != 0 {
            api::send(h, 0x46F, lower as usize, upper as isize); // UDM_SETRANGE32
            if buddy != 0 {
                api::send(h, 0x469, buddy, 0); // UDM_SETBUDDY
            }
            api::send(h, 0x471, 0, pos as isize); // UDM_SETPOS32
        }
        h
    }
    fn DrawStatusTextA(hdc: usize, rect: *const i32, text: usize, _flags: u32) {
        draw_status_text(hdc, rect, a_to_string(text as *const u8));
    }
    fn DrawStatusTextW(hdc: usize, rect: *const i32, text: usize, _flags: u32) {
        draw_status_text(hdc, rect, w_ptr_to_string(text as *const u16));
    }
    fn MakeDragList(_lb: usize) -> BOOL {
        TRUE
    }
    fn LBItemFromPt(lb: usize, x: i32, y: i32, _autoscroll: BOOL) -> i32 {
        api::send(lb, 0x1A9, 0, crate::gui::make_lparam(x, y)) as i32 & 0xffff
    }
    fn _TrackMouseEvent(t: *mut u8) -> BOOL {
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

    // ---- window subclassing ----
    fn SetWindowSubclass(hwnd: usize, proc_: usize, id: usize, refdata: usize) -> BOOL {
        with(|g| {
            let Some(w) = g.windows.get_mut(&hwnd) else { return FALSE };
            if let Some(e) = w.subclass.iter_mut().find(|e| e.0 == proc_ && e.1 == id) {
                e.2 = refdata;
                return TRUE;
            }
            if w.subclass.is_empty() {
                w.orig_proc = w.wndproc;
                w.wndproc = subclass_dispatch as *const () as usize;
            }
            w.subclass.insert(0, (proc_, id, refdata));
            TRUE
        })
    }
    fn GetWindowSubclass(hwnd: usize, proc_: usize, id: usize, refdata: *mut usize) -> BOOL {
        with(|g| match g.windows.get(&hwnd).and_then(|w| w.subclass.iter().find(|e| e.0 == proc_ && e.1 == id).copied()) {
            Some(e) => {
                if !refdata.is_null() {
                    *refdata = e.2;
                }
                TRUE
            }
            None => FALSE,
        })
    }
    fn RemoveWindowSubclass(hwnd: usize, proc_: usize, id: usize) -> BOOL {
        with(|g| {
            let Some(w) = g.windows.get_mut(&hwnd) else { return FALSE };
            let before = w.subclass.len();
            w.subclass.retain(|e| !(e.0 == proc_ && e.1 == id));
            if w.subclass.is_empty() && before > 0 {
                w.wndproc = w.orig_proc;
            }
            (w.subclass.len() != before) as BOOL
        })
    }
    fn DefSubclassProc(hwnd: usize, msg: u32, wp: usize, lp: isize) -> isize {
        let next = CHAIN.with(|c| c.borrow().iter().rev().find(|e| e.0 == hwnd).map_or(usize::MAX, |e| e.1));
        // outside a subclass call there is nothing to continue: go straight to the window's own procedure
        let index = if next == usize::MAX { usize::MAX / 2 } else { next };
        run_chain(hwnd, msg, wp, lp, index)
    }

    // ---- message-box style dialog ----
    fn TaskDialog(owner: usize, _inst: usize, title: usize, instruction: usize, content: usize, buttons: u32, icon: usize, button: *mut i32) -> i32 {
        task_dialog(owner, title, instruction, content, buttons, icon, button)
    }

    // ---- image lists ----
    fn ImageList_Create(cx: i32, cy: i32, flags: u32, _initial: i32, _grow: i32) -> usize {
        if cx <= 0 || cy <= 0 {
            return 0;
        }
        with(|g| g.add_object(Object::ImageList(Box::new(ImageList::new(cx, cy, flags)))))
    }
    fn ImageList_Destroy(h: usize) -> BOOL {
        with(|g| matches!(g.objects.remove(&h), Some(Object::ImageList(_)))) as BOOL
    }
    fn ImageList_GetImageCount(h: usize) -> i32 {
        with(|g| g.imagelist(h).map_or(0, |l| l.images.len() as i32))
    }
    fn ImageList_SetImageCount(h: usize, n: u32) -> BOOL {
        with(|g| match g.imagelist_mut(h) {
            Some(l) => {
                let blank = (vec![0u32; (l.w * l.h) as usize], vec![0u8; (l.w * l.h) as usize]);
                l.images.resize(n as usize, blank);
                TRUE
            }
            None => FALSE,
        })
    }
    fn ImageList_Add(h: usize, bmp: usize, mask: usize) -> i32 {
        with(|g| {
            let b = bitmap_copy(g, bmp)?;
            let m = if mask != 0 { bitmap_copy(g, mask) } else { None };
            Some(g.imagelist_mut(h)?.add_strip(&b, m.as_ref(), None) as i32)
        })
        .unwrap_or(-1)
    }
    fn ImageList_AddMasked(h: usize, bmp: usize, color: u32) -> i32 {
        with(|g| {
            let b = bitmap_copy(g, bmp)?;
            let key = (color != CLR_NONE).then(|| crate::gui::surface::from_colorref(color));
            Some(g.imagelist_mut(h)?.add_strip(&b, None, key) as i32)
        })
        .unwrap_or(-1)
    }
    fn ImageList_ReplaceIcon(h: usize, index: i32, icon: usize) -> i32 {
        with(|g| {
            let ic = icon_of(g, icon)?;
            let l = g.imagelist_mut(h)?;
            Some(l.add_icon(&ic, if index < 0 { None } else { Some(index as usize) }) as i32)
        })
        .unwrap_or(-1)
    }
    fn ImageList_Replace(h: usize, index: i32, bmp: usize, mask: usize) -> BOOL {
        with(|g| {
            let b = bitmap_copy(g, bmp)?;
            let m = if mask != 0 { bitmap_copy(g, mask) } else { None };
            let l = g.imagelist_mut(h)?;
            if index < 0 || index as usize >= l.images.len() {
                return None;
            }
            let mut one = ImageList::new(l.w, l.h, l.flags);
            one.add_strip(&b, m.as_ref(), None);
            l.images[index as usize] = one.images.into_iter().next()?;
            Some(TRUE)
        })
        .unwrap_or(FALSE)
    }
    fn ImageList_Remove(h: usize, index: i32) -> BOOL {
        with(|g| match g.imagelist_mut(h) {
            Some(l) if index < 0 => {
                l.images.clear();
                TRUE
            }
            Some(l) if (index as usize) < l.images.len() => {
                l.images.remove(index as usize);
                TRUE
            }
            _ => FALSE,
        })
    }
    fn ImageList_Draw(h: usize, index: i32, hdc: usize, x: i32, y: i32, _style: u32) -> BOOL {
        let _ = ILD_NORMAL;
        with(|g| g.draw_imagelist(hdc, h, index.max(0) as usize, x, y, 0, 0)) as BOOL
    }
    fn ImageList_DrawEx(h: usize, index: i32, hdc: usize, x: i32, y: i32, dx: i32, dy: i32, _bk: u32, _fg: u32, _style: u32) -> BOOL {
        with(|g| g.draw_imagelist(hdc, h, index.max(0) as usize, x, y, dx, dy)) as BOOL
    }
    fn ImageList_SetBkColor(h: usize, color: u32) -> u32 {
        with(|g| match g.imagelist_mut(h) {
            Some(l) => std::mem::replace(&mut l.bk, color),
            None => CLR_NONE,
        })
    }
    fn ImageList_GetBkColor(h: usize) -> u32 {
        with(|g| g.imagelist(h).map_or(CLR_NONE, |l| l.bk))
    }
    fn ImageList_GetIconSize(h: usize, cx: *mut i32, cy: *mut i32) -> BOOL {
        with(|g| match g.imagelist(h) {
            Some(l) => {
                if !cx.is_null() {
                    *cx = l.w;
                }
                if !cy.is_null() {
                    *cy = l.h;
                }
                TRUE
            }
            None => FALSE,
        })
    }
    fn ImageList_SetIconSize(h: usize, cx: i32, cy: i32) -> BOOL {
        with(|g| match g.imagelist_mut(h) {
            Some(l) if cx > 0 && cy > 0 => {
                *l = ImageList::new(cx, cy, l.flags);
                TRUE
            }
            _ => FALSE,
        })
    }
    fn ImageList_GetIcon(h: usize, index: i32, _flags: u32) -> usize {
        with(|g| {
            let icon = g.imagelist(h)?.icon(index.max(0) as usize)?;
            Some(g.add_object(Object::Icon(std::sync::Arc::new(icon))))
        })
        .unwrap_or(0)
    }
    fn ImageList_Duplicate(h: usize) -> usize {
        with(|g| match g.imagelist(h).cloned() {
            Some(l) => g.add_object(Object::ImageList(Box::new(l))),
            None => 0,
        })
    }
    fn ImageList_SetOverlayImage(_h: usize, _image: i32, _overlay: i32) -> BOOL {
        TRUE
    }
    fn ImageList_LoadImageW(inst: usize, name: usize, cx: i32, _grow: i32, mask: u32, kind: u32, flags: u32) -> usize {
        load_image_list(inst, name, true, cx, mask, kind, flags)
    }
    fn ImageList_LoadImageA(inst: usize, name: usize, cx: i32, _grow: i32, mask: u32, kind: u32, flags: u32) -> usize {
        load_image_list(inst, name, false, cx, mask, kind, flags)
    }
    fn ImageList_BeginDrag(_h: usize, _track: i32, _x: i32, _y: i32) -> BOOL {
        TRUE
    }
    fn ImageList_EndDrag() {}
    fn ImageList_DragEnter(_h: usize, _x: i32, _y: i32) -> BOOL {
        TRUE
    }
    fn ImageList_DragLeave(_h: usize) -> BOOL {
        TRUE
    }
    fn ImageList_DragMove(_x: i32, _y: i32) -> BOOL {
        TRUE
    }
    fn ImageList_DragShowNolock(_show: BOOL) -> BOOL {
        TRUE
    }
}

unsafe fn load_image_list(inst: usize, name: usize, wide: bool, cx: i32, mask: u32, kind: u32, flags: u32) -> usize {
    const LR_LOADFROMFILE: u32 = 0x10;
    if kind != 0 {
        return 0;
    }
    let surf = if flags & LR_LOADFROMFILE != 0 {
        let path = if wide { w_ptr_to_string(name as *const u16) } else { a_to_string(name as *const u8) };
        std::fs::read(crate::runtime::paths::resolve(&path)).ok().and_then(|d| res::bitmap_from_file(&d))
    } else {
        res::load(inst, res::RT_BITMAP, &Key::from_arg(name, wide)).and_then(res::bitmap_from_dib)
    };
    let Some(s) = surf else { return 0 };
    let mut list = ImageList::new(if cx > 0 { cx } else { s.h }, s.h, if mask != CLR_NONE { crate::gui::imagelist::ILC_MASK } else { 0 });
    let key = (mask != CLR_NONE).then(|| crate::gui::surface::from_colorref(mask));
    list.add_strip(&s, None, key);
    with(|g| g.add_object(Object::ImageList(Box::new(list))))
}

fn draw_status_text(hdc: usize, rect: *const i32, text: String) {
    if rect.is_null() {
        return;
    }
    let r = unsafe { crate::shim::user32::rect_from(rect) };
    with(|g| {
        let mut tr = crate::gui::surface::Rect::new(r.l + 4, r.t + 2, r.r - 2, r.b);
        g.draw_text(hdc, &text, &mut tr, 0x20 | 0x800);
    });
    let _ = send_ansi;
}
