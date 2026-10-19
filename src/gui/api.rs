//! Window operations that send messages (and so run guest window procedures): creating and
//! destroying windows, showing, moving, focus, painting. None of these hold the lock across a
//! message.

use super::backend::Cursor;
use super::gdi::Object;
use super::msg::{current_ansi, send_message};
use super::surface::Rect;
use super::wm::*;
use super::*;
use crate::runtime::handles::current_tid;
use crate::runtime::strings::*;

pub unsafe fn send(hwnd: usize, msg: u32, wp: usize, lp: isize) -> isize {
    send_message(hwnd, msg, wp, lp, current_ansi())
}

pub unsafe fn send_ansi(hwnd: usize, msg: u32, wp: usize, lp: isize, ansi: bool) -> isize {
    send_message(hwnd, msg, wp, lp, ansi)
}

// ---- window creation ------------------------------------------------------------------------------

/// Builds the CREATESTRUCT a window procedure gets in `WM_NCCREATE` / `WM_CREATE`.
unsafe fn create_struct(a: &CreateArgs, class_name: &str, unicode: bool, rect: Rect) -> Vec<u8> {
    let mut b = vec![0u8; 80];
    let put = |b: &mut Vec<u8>, off: usize, v: &[u8]| b[off..off + v.len()].copy_from_slice(v);
    put(&mut b, 0, &(a.param as u64).to_le_bytes());
    put(&mut b, 8, &(a.hinstance as u64).to_le_bytes());
    put(&mut b, 16, &(a.menu as u64).to_le_bytes());
    put(&mut b, 24, &(a.parent as u64).to_le_bytes());
    put(&mut b, 32, &rect.height().to_le_bytes());
    put(&mut b, 36, &rect.width().to_le_bytes());
    put(&mut b, 40, &rect.t.to_le_bytes());
    put(&mut b, 44, &rect.l.to_le_bytes());
    put(&mut b, 48, &(a.style as i32).to_le_bytes());
    // the two name strings live as long as the Vec the caller keeps; see `with_strings`
    let _ = (class_name, unicode);
    put(&mut b, 72, &a.ex_style.to_le_bytes());
    b
}

pub unsafe fn create_window_ex(a: CreateArgs) -> usize {
    let tid = current_tid();
    let created = with(|g| {
        let r = g.create_window_raw(&a, tid);
        r.map(|h| {
            let w = g.windows.get(&h).unwrap();
            (h, w.unicode, w.class.name.clone(), w.rect)
        })
    });
    let (hwnd, unicode, class_name, rect) = match created {
        Ok(v) => v,
        Err(8) => {
            // `ensure_backend` has already said why; a program that wants a window and cannot get
            // one would only hang in its message loop, so stop here.
            libc::fflush(std::ptr::null_mut());
            eprintln!("nle: fatal: this program needs a window; run it where DISPLAY is set, or use NLE_GUI=headless");
            crate::loader::exit_process(70);
        }
        Err(e) => {
            crate::win::set_last_error(e);
            return 0;
        }
    };
    // strings for the CREATESTRUCT in the encoding the window's class expects
    let (name_a, name_w, class_a, class_w);
    let (name_ptr, class_ptr): (usize, usize) = if unicode {
        name_w = to_wide_nul(&a.title);
        class_w = to_wide_nul(&class_name);
        (name_w.as_ptr() as usize, class_w.as_ptr() as usize)
    } else {
        name_a = [a.title.as_bytes(), &[0]].concat();
        class_a = [class_name.as_bytes(), &[0]].concat();
        (name_a.as_ptr() as usize, class_a.as_ptr() as usize)
    };
    let mut cs = create_struct(&a, &class_name, unicode, rect);
    cs[56..64].copy_from_slice(&(name_ptr as u64).to_le_bytes());
    cs[64..72].copy_from_slice(&(class_ptr as u64).to_le_bytes());
    let lp = cs.as_ptr() as isize;
    let ansi = !unicode;
    if send_message(hwnd, WM_NCCREATE, 0, lp, ansi) == 0 {
        destroy_window(hwnd);
        return 0;
    }
    if send_message(hwnd, WM_CREATE, 0, lp, ansi) == -1 {
        destroy_window(hwnd);
        return 0;
    }
    // the window now exists; Windows reports its geometry once
    let geometry = with(|g| g.windows.get(&hwnd).map(|w| (w.client_size(), g.client_to_screen(hwnd, 0, 0), w.is_top_level(), w.rect, w.inset())));
    if let Some(((cw, ch), (sx, sy), top, rect, inset)) = geometry {
        send_message(hwnd, WM_SIZE, 0, make_lparam(cw, ch), ansi);
        let (mx, my) = if top { (sx, sy) } else { (rect.l + inset.l, rect.t + inset.t) };
        send_message(hwnd, WM_MOVE, 0, make_lparam(mx, my), ansi);
    }
    if a.style & WS_VISIBLE != 0 {
        show_window(hwnd, SW_SHOWNORMAL);
    }
    hwnd
}

pub unsafe fn destroy_window(h: usize) -> bool {
    let tree = with(|g| g.windows.contains_key(&h).then(|| g.subtree(h)));
    let Some(tree) = tree else {
        crate::win::set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return false;
    };
    // hide first so nothing flickers, and release input state held inside the subtree
    let (was_focus, was_active) = with(|g| {
        let focus = g.focus != 0 && tree.contains(&g.focus);
        let active = g.active != 0 && tree.contains(&g.active);
        if g.capture != 0 && tree.contains(&g.capture) {
            g.capture = 0;
        }
        if let Some(w) = g.windows.get_mut(&h) {
            w.style &= !WS_VISIBLE;
            if w.native != 0 {
                if let Some(b) = &g.backend {
                    b.show(w.native, false);
                }
            }
        }
        (focus, active)
    });
    if was_focus {
        let f = with(|g| std::mem::take(&mut g.focus));
        send(f, WM_KILLFOCUS, 0, 0);
    }
    if was_active {
        send(h, WM_ACTIVATE, 0, 0);
        with(|g| g.active = 0);
    }
    for &w in &tree {
        if with(|g| g.windows.contains_key(&w)) {
            send(w, WM_DESTROY, 0, 0);
        }
    }
    for &w in tree.iter().rev() {
        if with(|g| g.windows.contains_key(&w)) {
            send(w, WM_NCDESTROY, 0, 0);
        }
    }
    let parent = with(|g| g.windows.get(&h).map_or(0, |w| (w.parent, g.window_rect_screen(h)).0));
    let area = with(|g| {
        let r = g.windows.get(&h).map(|w| w.rect);
        for &w in tree.iter().rev() {
            g.remove_window_raw(w);
        }
        r
    });
    if parent != 0 {
        with(|g| g.invalidate(parent, area, true));
    }
    true
}

// ---- showing, activation, focus --------------------------------------------------------------------

pub unsafe fn show_window(h: usize, cmd: i32) -> bool {
    struct Plan {
        was: bool,
        top: bool,
        visible: bool,
        activate: bool,
        maximize: bool,
        minimize: bool,
        restore: bool,
    }
    let plan = with(|g| {
        let w = g.windows.get(&h)?;
        let (visible, activate, maximize, minimize, restore) = match cmd {
            SW_HIDE => (false, false, false, false, false),
            SW_SHOWNORMAL | SW_SHOW | SW_SHOWDEFAULT => (true, true, false, false, false),
            SW_RESTORE => (true, true, false, false, true),
            SW_SHOWNOACTIVATE | SW_SHOWNA => (true, false, false, false, false),
            SW_SHOWMAXIMIZED => (true, true, true, false, false),
            SW_SHOWMINIMIZED | SW_MINIMIZE => (true, false, false, true, false),
            SW_SHOWMINNOACTIVE => (true, false, false, true, false),
            _ => (true, true, false, false, false),
        };
        Some(Plan { was: w.visible_flag(), top: w.is_top_level(), visible, activate, maximize, minimize, restore })
    });
    let Some(p) = plan else { return false };
    with(|g| {
        let backend = g.backend.clone();
        let screen = g.screen_size();
        let mut resize: Option<Rect> = None;
        if let Some(w) = g.windows.get_mut(&h) {
            if p.visible {
                w.style |= WS_VISIBLE;
            } else {
                w.style &= !WS_VISIBLE;
            }
            if p.top {
                if p.maximize && !w.maximized {
                    w.restore_rect = Some(w.rect);
                    w.maximized = true;
                    w.minimized = false;
                    let f = w.frame();
                    resize = Some(Rect::new(-f.l, -f.t, screen.0 + f.r, screen.1 + f.b));
                } else if p.minimize {
                    w.minimized = true;
                } else if p.restore || (p.visible && !p.maximize && !p.minimize) {
                    if w.maximized && p.restore {
                        resize = w.restore_rect.take();
                        w.maximized = false;
                    }
                    w.minimized = false;
                }
                if w.native != 0 {
                    if let Some(b) = &backend {
                        b.show(w.native, p.visible && !w.minimized);
                    }
                }
            }
        }
        if let Some(r) = resize {
            g.set_rect(h, r);
        }
        if p.visible {
            g.invalidate(h, None, true);
            if let Some(w) = g.windows.get_mut(&h) {
                w.shown_once = true;
                if let Some(s) = w.surface.as_mut() {
                    s.mark(s.bounds());
                }
            }
        } else if !p.top {
            let parent = g.windows.get(&h).map_or(0, |w| w.parent);
            let rect = g.windows.get(&h).map(|w| w.rect);
            g.invalidate(parent, rect, true);
        }
    });
    send(h, WM_SHOWWINDOW, p.visible as usize, 0);
    if p.top && p.visible {
        let (maxd, mind, size) = with(|g| g.windows.get(&h).map(|w| (w.maximized, w.minimized, w.client_size())).unwrap_or_default());
        if maxd || mind {
            send(h, WM_SIZE, if mind { 1 } else { 2 }, make_lparam(size.0, size.1));
        }
        if p.activate {
            activate_window(h);
        }
    }
    p.was
}

pub unsafe fn activate_window(top: usize) {
    let (prev, exists) = with(|g| (g.active, g.windows.contains_key(&top)));
    if !exists || prev == top {
        return;
    }
    with(|g| g.active = top);
    if prev != 0 {
        send(prev, WM_ACTIVATE, 0, top as isize);
    }
    send(top, WM_ACTIVATE, 1, prev as isize);
    let need_focus = with(|g| g.focus == 0 || !g.is_descendant(g.focus, top));
    if need_focus {
        let target = with(|g| {
            let w = g.windows.get(&top)?;
            let first_tab = g.subtree(top).into_iter().skip(1).find(|c| g.windows.get(c).is_some_and(|cw| cw.style & WS_TABSTOP != 0 && cw.style & WS_DISABLED == 0 && cw.visible_flag()));
            Some(first_tab.unwrap_or(w.hwnd))
        });
        if let Some(t) = target {
            set_focus(t);
        }
    }
    with(|g| {
        if let Some(b) = g.backend.clone() {
            if let Some(w) = g.windows.get(&top) {
                if w.native != 0 {
                    b.focus(w.native);
                }
            }
        }
    });
}

pub unsafe fn deactivate_window(top: usize) {
    let focus = with(|g| {
        if g.active == top {
            g.active = 0;
        }
        std::mem::take(&mut g.focus)
    });
    if focus != 0 {
        send(focus, WM_KILLFOCUS, 0, 0);
    }
    send(top, WM_ACTIVATE, 0, 0);
}

pub unsafe fn set_focus(h: usize) -> usize {
    let ok = with(|g| g.windows.contains_key(&h) && g.is_visible(h) && g.is_enabled(h));
    if !ok {
        return with(|g| g.focus);
    }
    let prev = with(|g| std::mem::replace(&mut g.focus, h));
    if prev == h {
        return prev;
    }
    if prev != 0 {
        send(prev, WM_KILLFOCUS, h, 0);
    }
    send(h, WM_SETFOCUS, prev, 0);
    prev
}

pub unsafe fn set_window_pos(h: usize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> bool {
    const NOSIZE: u32 = 1;
    const NOMOVE: u32 = 2;
    const NOZORDER: u32 = 4;
    const SHOWWINDOW: u32 = 0x40;
    const HIDEWINDOW: u32 = 0x80;
    let info = with(|g| {
        let w = g.windows.get(&h)?;
        let old = w.rect;
        let new = Rect::new(if flags & NOMOVE != 0 { old.l } else { x }, if flags & NOMOVE != 0 { old.t } else { y }, 0, 0);
        let (nw, nh) = if flags & NOSIZE != 0 { (old.width(), old.height()) } else { (cx, cy) };
        let new = Rect::new(new.l, new.t, new.l + nw, new.t + nh);
        Some((old, new))
    });
    let Some((old, new)) = info else { return false };
    if old != new {
        with(|g| g.set_rect(h, new));
        let (cw, ch, i, top) = with(|g| g.windows.get(&h).map(|w| (w.client_size().0, w.client_size().1, w.inset(), w.is_top_level())).unwrap_or((0, 0, Rect::default(), true)));
        if old.width() != new.width() || old.height() != new.height() {
            send(h, WM_SIZE, 0, make_lparam(cw, ch));
        }
        if (old.l, old.t) != (new.l, new.t) {
            let (mx, my) = if top { with(|g| g.client_to_screen(h, 0, 0)) } else { (new.l + i.l, new.t + i.t) };
            send(h, WM_MOVE, 0, make_lparam(mx, my));
        }
    }
    if flags & NOZORDER == 0 {
        with(|g| g.bring_to_top(h));
    }
    if flags & SHOWWINDOW != 0 {
        show_window(h, SW_SHOW);
    }
    if flags & HIDEWINDOW != 0 {
        show_window(h, SW_HIDE);
    }
    true
}

pub unsafe fn set_window_text(h: usize, text: &str) {
    with(|g| {
        g.set_title(h, text);
        g.invalidate(h, None, true);
    });
}

pub fn cursor_kind(hcursor: usize) -> Cursor {
    with(|g| match g.objects.get(&hcursor) {
        Some(Object::Stub(id)) => match id {
            32513 => Cursor::IBeam,
            32514 | 32650 => Cursor::Wait,
            32515 => Cursor::Cross,
            32649 => Cursor::Hand,
            32646 => Cursor::SizeAll,
            32644 => Cursor::SizeWE,
            32645 => Cursor::SizeNS,
            32642 => Cursor::SizeNWSE,
            32643 => Cursor::SizeNESW,
            32648 => Cursor::No,
            _ => Cursor::Arrow,
        },
        _ => Cursor::Arrow,
    })
}

// ---- painting ----------------------------------------------------------------------------------------

/// `BeginPaint`: erases the invalid area if needed, validates it, and returns a DC clipped to it.
pub unsafe fn begin_paint(h: usize, ps: *mut u8) -> usize {
    let taken = with(|g| {
        g.paint_nc(h);
        let w = g.windows.get_mut(&h)?;
        let rc = w.update;
        let erase = w.erase;
        w.update = Rect::default();
        w.erase = false;
        let hdc = g.window_dc(h, true, Some(rc))?;
        Some((hdc, rc, erase))
    });
    let Some((hdc, rc, erase)) = taken else { return 0 };
    let mut f_erase = 0u32;
    if erase && send_message(h, WM_ERASEBKGND, hdc, 0, false) == 0 {
        f_erase = 1;
    }
    if !ps.is_null() {
        std::ptr::write_bytes(ps, 0, 72);
        *(ps as *mut u64) = hdc as u64;
        *(ps.add(8) as *mut u32) = f_erase;
        let r = ps.add(12) as *mut i32;
        *r = rc.l;
        *r.add(1) = rc.t;
        *r.add(2) = rc.r;
        *r.add(3) = rc.b;
    }
    hdc
}

pub unsafe fn end_paint(_h: usize, ps: *const u8) -> bool {
    if ps.is_null() {
        return false;
    }
    let hdc = *(ps as *const u64) as usize;
    with(|g| {
        g.delete_dc(hdc);
        g.flush();
    });
    true
}

pub fn get_dc(h: usize) -> usize {
    with(|g| {
        if h == 0 {
            return g.new_dc(super::gdi::Target::Screen, 0, false);
        }
        g.window_dc(h, false, None).unwrap_or(0)
    })
}

pub fn release_dc(_h: usize, hdc: usize) -> bool {
    with(|g| {
        let ok = g.delete_dc(hdc);
        g.flush();
        ok
    })
}

pub unsafe fn update_window(h: usize) -> bool {
    let pending = with(|g| g.windows.get(&h).is_some_and(|w| !w.update.is_empty()));
    if pending {
        send(h, WM_PAINT, 0, 0);
        with(|g| g.flush());
    }
    true
}

/// `RedrawWindow` flags that matter here.
pub unsafe fn redraw_window(h: usize, rect: Option<Rect>, flags: u32) -> bool {
    const INVALIDATE: u32 = 1;
    const ERASE: u32 = 4;
    const VALIDATE: u32 = 8;
    const UPDATENOW: u32 = 0x100;
    const ALLCHILDREN: u32 = 0x80;
    let h = if h == 0 { with(|g| g.main_window()) } else { h };
    with(|g| {
        let targets = if flags & ALLCHILDREN != 0 { g.subtree(h) } else { vec![h] };
        for t in targets {
            if flags & VALIDATE != 0 {
                g.validate(t, rect);
            }
            if flags & INVALIDATE != 0 {
                g.invalidate(t, rect, flags & ERASE != 0);
            }
        }
    });
    if flags & UPDATENOW != 0 {
        let wins = with(|g| g.subtree(h));
        for w in wins {
            update_window(w);
        }
    }
    true
}

// ---- scroll bars --------------------------------------------------------------------------------------

use super::scroll::{SB_CTL, SIF_POS, SIF_RANGE};

/// Finishes a scroll bar change: tells the window if its client area changed, and redraws.
unsafe fn scroll_done(h: usize, which: i32, r: Option<(i32, bool)>, redraw: bool) -> (i32, bool) {
    let Some((pos, changed)) = r else { return (0, false) };
    if changed {
        let (cw, ch) = with(|g| g.windows.get(&h).map_or((0, 0), |w| w.client_size()));
        send(h, WM_SIZE, 0, make_lparam(cw, ch));
    } else if redraw {
        with(|g| {
            if which == SB_CTL {
                g.invalidate(h, None, false);
            } else {
                g.paint_nc(h);
                g.flush();
            }
        });
    }
    (pos, changed)
}

/// `SetScrollInfo`; `info` is a `SCROLLINFO`. Returns the new position.
pub unsafe fn set_scroll_info(h: usize, which: i32, info: *const u8, redraw: bool) -> i32 {
    if info.is_null() {
        return 0;
    }
    let rd = |o: usize| *(info.add(o) as *const i32);
    let (mask, page) = (*(info.add(4) as *const u32), *(info.add(16) as *const u32));
    let r = with(|g| g.scroll_set(h, which, mask, rd(8), rd(12), page, rd(20)));
    scroll_done(h, which, r, redraw).0
}

/// `GetScrollInfo`: fills the `SCROLLINFO` fields its `fMask` selects.
pub unsafe fn get_scroll_info(h: usize, which: i32, info: *mut u8) -> bool {
    if info.is_null() {
        return false;
    }
    let Some(bar) = with(|g| g.windows.get(&h).and_then(|w| w.sb.get(which).copied())) else { return false };
    let mask = *(info.add(4) as *const u32);
    let wr = |o: usize, v: i32| *(info.add(o) as *mut i32) = v;
    if mask & super::scroll::SIF_RANGE != 0 {
        wr(8, bar.min);
        wr(12, bar.max);
    }
    if mask & super::scroll::SIF_PAGE != 0 {
        *(info.add(16) as *mut u32) = bar.page;
    }
    if mask & SIF_POS != 0 {
        wr(20, bar.pos);
    }
    if mask & super::scroll::SIF_TRACKPOS != 0 {
        wr(24, bar.track);
    }
    true
}

/// `SetScrollPos`: returns the previous position.
pub unsafe fn set_scroll_pos(h: usize, which: i32, pos: i32, redraw: bool) -> i32 {
    let prev = with(|g| g.windows.get(&h).and_then(|w| w.sb.get(which).map(|b| b.pos))).unwrap_or(0);
    let r = with(|g| g.scroll_set(h, which, SIF_POS, 0, 0, 0, pos));
    scroll_done(h, which, r, redraw);
    prev
}

/// `SetScrollRange`: a range that leaves nothing to scroll hides a window's bar.
pub unsafe fn set_scroll_range(h: usize, which: i32, min: i32, max: i32, redraw: bool) -> bool {
    let r = with(|g| g.scroll_set(h, which, SIF_RANGE, min, max, 0, 0));
    scroll_done(h, which, r, redraw);
    r.is_some()
}

pub unsafe fn show_scroll_bar(h: usize, which: i32, show: bool) -> bool {
    let changed = with(|g| g.scroll_show(h, which, show));
    if changed {
        let (cw, ch) = with(|g| g.windows.get(&h).map_or((0, 0), |w| w.client_size()));
        send(h, WM_SIZE, 0, make_lparam(cw, ch));
    }
    true
}

/// `EnableScrollBar`: `arrows` 0 enables both, 1 disables the up / left arrow, 2 the down / right one, 3 both.
pub unsafe fn enable_scroll_bar(h: usize, which: i32, arrows: u32) -> bool {
    let ok = with(|g| {
        let Some(w) = g.windows.get_mut(&h) else { return false };
        let targets: &[i32] = match which {
            super::scroll::SB_BOTH => &[super::scroll::SB_HORZ, super::scroll::SB_VERT],
            1 => &[super::scroll::SB_VERT],
            0 => &[super::scroll::SB_HORZ],
            _ => &[SB_CTL],
        };
        for t in targets {
            if let Some(b) = w.sb.get_mut(*t) {
                b.up_enabled = arrows & 1 == 0;
                b.down_enabled = arrows & 2 == 0;
            }
        }
        true
    });
    if ok {
        with(|g| {
            g.paint_nc(h);
            g.invalidate(h, None, false);
            g.flush();
        });
    }
    ok
}

/// `ScrollWindow(Ex)`: moves the client pixels (or the part inside `area`) by (dx, dy), moves the child windows
/// when the whole client area scrolls, and invalidates what was uncovered.
pub unsafe fn scroll_window(h: usize, dx: i32, dy: i32, area: Option<Rect>) -> bool {
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return false };
        let (cw, ch) = w.client_size();
        let client = Rect::new(0, 0, cw, ch);
        let region = area.map_or(client, |a| a.normalized().intersect(&client));
        if region.is_empty() {
            return true;
        }
        let (ox, oy) = g.client_origin_in_top(h);
        let top = g.top_level_of(h);
        if let Some(s) = g.windows.get_mut(&top).and_then(|t| t.surface.as_mut()) {
            let dst = region.offset(dx, dy).intersect(&region);
            if !dst.is_empty() {
                let src = dst.offset(-dx, -dy);
                let mut copy = super::surface::Surface::new(src.width(), src.height());
                for y in 0..src.height() {
                    for x in 0..src.width() {
                        copy.put(x, y, s.get(ox + src.l + x, oy + src.t + y));
                    }
                }
                for y in 0..dst.height() {
                    for x in 0..dst.width() {
                        s.put(ox + dst.l + x, oy + dst.t + y, copy.get(x, y));
                    }
                }
                s.mark(dst.offset(ox, oy));
            }
        }
        let uncovered = super::gdi::rect_subtract(region, region.offset(dx, dy).intersect(&region));
        if area.is_none() {
            let kids: Vec<usize> = g.windows.get(&h).map(|w| w.children.clone()).unwrap_or_default();
            for k in kids {
                if let Some(c) = g.windows.get_mut(&k) {
                    c.rect = c.rect.offset(dx, dy);
                }
            }
        }
        for r in uncovered {
            g.invalidate(h, Some(r), true);
        }
        true
    })
}
