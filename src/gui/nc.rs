//! The non-client areas of windows that are drawn here: borders and edges, scroll bars, and (in
//! the top-level surface) the menu bar. The window manager's own frame and caption are not ours.
//!
//! Everything is expressed relative to a window's client rectangle: a vertical scroll bar sits to
//! the right of the client area and spans its height, a horizontal one sits below and spans its
//! width, and the menu bar sits above the client area of a top-level window.

use super::scroll::{self, Bar, Part, SB_CTL, SB_HORZ, SB_VERT};
use super::surface::*;
use super::wm::{MENU_H, SB_W};
use super::*;

/// What a point (in a window's client coordinates) is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nc {
    Client,
    Scroll { which: i32, part: Part },
    Corner,
    Menu,
    Border,
    Nowhere,
}

/// A scroll bar press in progress: the thumb is being dragged, or an arrow / page area is held.
#[derive(Clone, Copy, Debug)]
pub struct SbDrag {
    pub hwnd: usize,
    pub which: i32,
    pub part: Part,
    /// Distance from the thumb's start to where the mouse took hold of it.
    pub grab: i32,
}

/// The system timer that repeats a scroll bar press that is held (handled by `DefWindowProc`).
pub const SB_REPEAT_TIMER: usize = 0xFFF0;

/// Per-window scroll bar state.
#[derive(Clone, Debug, Default)]
pub struct Bars {
    pub vert: Bar,
    pub horz: Bar,
    /// The bar of a SCROLLBAR control.
    pub ctl: Bar,
    pub hot: Option<(i32, Part)>,
    pub pressed: Option<(i32, Part)>,
}

impl Bars {
    pub fn get(&self, which: i32) -> Option<&Bar> {
        match which {
            SB_HORZ => Some(&self.horz),
            SB_VERT => Some(&self.vert),
            SB_CTL => Some(&self.ctl),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, which: i32) -> Option<&mut Bar> {
        match which {
            SB_HORZ => Some(&mut self.horz),
            SB_VERT => Some(&mut self.vert),
            SB_CTL => Some(&mut self.ctl),
            _ => None,
        }
    }
}

const SUNKEN_OUTER_TL: u32 = 0xA0A0A0;
const SUNKEN_INNER_TL: u32 = 0x696969;
const SUNKEN_OUTER_BR: u32 = 0xFFFFFF;
const SUNKEN_INNER_BR: u32 = 0xE3E3E3;
const THIN_BORDER: u32 = 0x646464;
const BAR_FACE: u32 = 0xF0F0F0;

impl Gui {
    /// The vertical and horizontal bar rectangles of a window, in client coordinates.
    pub fn bar_rects(&self, h: usize) -> (Option<Rect>, Option<Rect>) {
        let Some(w) = self.windows.get(&h) else { return (None, None) };
        let (cw, ch) = w.client_size();
        let v = (w.style & WS_VSCROLL != 0).then(|| Rect::new(cw, 0, cw + SB_W, ch));
        let hz = (w.style & WS_HSCROLL != 0).then(|| Rect::new(0, ch, cw, ch + SB_W));
        (v, hz)
    }

    /// Classifies a point given in `h`'s client coordinates.
    pub fn nc_hit(&self, h: usize, x: i32, y: i32) -> Nc {
        let Some(w) = self.windows.get(&h) else { return Nc::Nowhere };
        let (cw, ch) = w.client_size();
        if x >= 0 && y >= 0 && x < cw && y < ch {
            return Nc::Client;
        }
        let (v, hz) = self.bar_rects(h);
        if let Some(r) = v {
            if r.contains(x, y) {
                return Nc::Scroll { which: SB_VERT, part: scroll::layout(&w.sb.vert, r, true).part_at(x, y) };
            }
        }
        if let Some(r) = hz {
            if r.contains(x, y) {
                return Nc::Scroll { which: SB_HORZ, part: scroll::layout(&w.sb.horz, r, false).part_at(x, y) };
            }
        }
        if v.is_some() && hz.is_some() && Rect::new(cw, ch, cw + SB_W, ch + SB_W).contains(x, y) {
            return Nc::Corner;
        }
        if w.is_top_level() && w.menu != 0 && (-MENU_H..0).contains(&y) && x >= 0 && x < cw + if v.is_some() { SB_W } else { 0 } {
            return Nc::Menu;
        }
        if w.is_top_level() {
            Nc::Nowhere
        } else {
            Nc::Border
        }
    }

    /// Redraws the borders, scroll bars and menu bar of a window into its top-level surface.
    pub fn paint_nc(&mut self, h: usize) {
        let Some(w) = self.windows.get(&h) else { return };
        if !self.is_visible(h) {
            return;
        }
        let top = self.top_level_of(h);
        let inset = w.inset();
        let (cw, ch) = w.client_size();
        let (ox, oy) = self.client_origin_in_top(h);
        let top_level = w.is_top_level();
        let (style, ex) = (w.style, w.ex_style);
        let enabled = self.is_enabled(h);
        let sb = w.sb.clone();
        let outer = Rect::new(ox - inset.l, oy - inset.t, ox - inset.l + w.rect.width(), oy - inset.t + w.rect.height());
        let menu = if top_level { w.menu } else { 0 };
        let mut clip = if top_level { Rect::new(i32::MIN / 2, i32::MIN / 2, i32::MAX / 2, i32::MAX / 2) } else { outer };
        if let Some(parent) = self.windows.get(&h).map(|w| w.parent).filter(|p| *p != 0) {
            clip = clip.intersect(&self.visible_client_in_top(parent));
        }
        let has_bars = style & (WS_VSCROLL | WS_HSCROLL) != 0;
        let border = (!top_level || super::wm::is_popup(style, ex)) && (ex & (WS_EX_CLIENTEDGE | WS_EX_STATICEDGE) != 0 || style & WS_BORDER != 0);
        if !has_bars && !border && menu == 0 {
            return;
        }
        let (v_rect, h_rect) = self.bar_rects(h);
        let Some(surf) = self.windows.get_mut(&top).and_then(|t| t.surface.as_mut()) else { return };
        let mut cv = Canvas::new(surf, 0, 0, clip);
        if border {
            draw_border(&mut cv, outer, style, ex);
        }
        let at = |r: Rect| r.offset(ox, oy);
        for (rect, which, vertical) in [(v_rect, SB_VERT, true), (h_rect, SB_HORZ, false)] {
            let Some(r) = rect else { continue };
            let (hot, pressed) = (part_of(sb.hot, which), part_of(sb.pressed, which));
            scroll::paint(&mut cv, at(r), vertical, sb.get(which).unwrap(), enabled, hot, pressed);
        }
        if v_rect.is_some() && h_rect.is_some() {
            cv.fill_rect(at(Rect::new(cw, ch, cw + SB_W, ch + SB_W)), BAR_FACE);
        }
        if menu != 0 {
            self.paint_menu_bar(h);
        }
    }

    /// The menu bar of a top-level window.
    pub fn paint_menu_bar(&mut self, h: usize) {
        self.menu_paint_bar(h);
    }

    /// Scroll bar information as `SetScrollInfo` sets it. Returns the new position and whether showing or hiding the
    /// bar changed the client area.
    pub fn scroll_set(&mut self, h: usize, which: i32, mask: u32, min: i32, max: i32, page: u32, pos: i32) -> Option<(i32, bool)> {
        let w = self.windows.get_mut(&h)?;
        let bar = w.sb.get_mut(which)?;
        bar.set(mask, min, max, page, pos);
        let (new_pos, need) = (bar.pos, bar.scrollable());
        let mut changed = false;
        if which != SB_CTL && mask & (scroll::SIF_RANGE | scroll::SIF_PAGE) != 0 {
            let flag = if which == SB_VERT { WS_VSCROLL } else { WS_HSCROLL };
            let shown = w.style & flag != 0;
            if need && !shown {
                w.style |= flag;
                changed = true;
            } else if !need && shown && mask & scroll::SIF_DISABLENOSCROLL == 0 {
                w.style &= !flag;
                changed = true;
            }
        }
        if changed {
            self.invalidate(h, None, true);
        }
        Some((new_pos, changed))
    }

    /// `ShowScrollBar`: adds or removes the style bit that makes a bar part of the window.
    pub fn scroll_show(&mut self, h: usize, which: i32, show: bool) -> bool {
        let Some(w) = self.windows.get_mut(&h) else { return false };
        let mut flags = 0;
        if which == SB_VERT || which == scroll::SB_BOTH {
            flags |= WS_VSCROLL;
        }
        if which == SB_HORZ || which == scroll::SB_BOTH {
            flags |= WS_HSCROLL;
        }
        let before = w.style;
        if show {
            w.style |= flags;
        } else {
            w.style &= !flags;
        }
        let changed = w.style != before;
        if changed {
            self.invalidate(h, None, true);
        }
        changed
    }

    // ---- mouse ---------------------------------------------------------------------------------------

    /// Handles a button event on a scroll bar (or the end of a drag). Returns true when it was consumed;
    /// scroll messages for the window to receive go into `out`.
    pub fn nc_button(&mut self, h: usize, lx: i32, ly: i32, button: u32, down: bool, out: &mut msg::Out) -> bool {
        if let Some(d) = self.sb_drag {
            if !down && button == 1 {
                self.sb_finish(d, out);
            }
            return true;
        }
        if self.nc_hit(h, lx, ly) == Nc::Menu {
            if down && button == 1 && self.menu_mode.is_none() {
                if let Some(i) = self.bar_item_at(h, lx, ly + MENU_H) {
                    let menu = self.windows.get(&h).map_or(0, |w| w.menu);
                    let time = self.tick();
                    self.menu_begin(h, h, false, false, false);
                    out.sends.push(Msg { hwnd: h, message: super::menu::WM_ENTERMENULOOP, wparam: 0, lparam: 0, time, pt: self.pointer });
                    out.sends.push(Msg { hwnd: h, message: super::menu::WM_INITMENU, wparam: menu, lparam: 0, time, pt: self.pointer });
                    self.open_bar_item_pub(h, i as i32, out);
                }
            }
            return true;
        }
        if button != 1 || !down {
            return matches!(self.nc_hit(h, lx, ly), Nc::Scroll { .. } | Nc::Corner);
        }
        let Nc::Scroll { which, part } = self.nc_hit(h, lx, ly) else { return matches!(self.nc_hit(h, lx, ly), Nc::Corner) };
        if !self.is_enabled(h) || part == Part::None {
            return true;
        }
        let Some((rect, vertical)) = self.bar_rect_of(h, which) else { return true };
        let (lay, enabled_arrow) = {
            let w = &self.windows[&h];
            let bar = w.sb.get(which).copied().unwrap_or_default();
            let ok = match part {
                Part::Arrow1 => bar.up_enabled,
                Part::Arrow2 => bar.down_enabled,
                _ => true,
            };
            (scroll::layout(&bar, rect, vertical), ok && bar.scrollable())
        };
        if !enabled_arrow {
            return true;
        }
        let grab = if part == Part::Thumb { (if vertical { ly } else { lx }) - (if vertical { lay.thumb.t } else { lay.thumb.l }) } else { 0 };
        self.sb_drag = Some(SbDrag { hwnd: h, which, part, grab });
        if let Some(w) = self.windows.get_mut(&h) {
            w.sb.pressed = Some((which, part));
        }
        if let Some(code) = scroll::code_for(part) {
            self.send_scroll(h, which, code, 0, out);
            self.arm_repeat(h, 400);
        }
        self.paint_nc(h);
        true
    }

    /// (Re)starts the repeat timer of a window's scroll bar after `ms` milliseconds.
    fn arm_repeat(&mut self, h: usize, ms: u64) {
        let Some(tid) = self.windows.get(&h).map(|w| w.thread) else { return };
        let interval = std::time::Duration::from_millis(ms);
        let t = self.thread_state(tid);
        t.timers.retain(|tm| !(tm.hwnd == h && tm.id == SB_REPEAT_TIMER));
        t.timers.push(msg::Timer { hwnd: h, id: SB_REPEAT_TIMER, interval, due: std::time::Instant::now() + interval, proc_: 0 });
        self.wake(tid);
    }

    fn stop_repeat(&mut self, h: usize) {
        if let Some(tid) = self.windows.get(&h).map(|w| w.thread) {
            self.thread_state(tid).timers.retain(|tm| !(tm.hwnd == h && tm.id == SB_REPEAT_TIMER));
        }
    }

    /// The repeat timer fired: scroll again while the mouse is still over the part that was pressed.
    pub fn nc_repeat(&mut self, h: usize) -> msg::Out {
        let mut out = msg::Out::default();
        let Some(d) = self.sb_drag.filter(|d| d.hwnd == h && d.part != Part::Thumb) else {
            self.stop_repeat(h);
            return out;
        };
        let (sx, sy) = self.pointer;
        let (lx, ly) = self.screen_to_client(h, sx, sy);
        if self.nc_hit(h, lx, ly) == (Nc::Scroll { which: d.which, part: d.part }) {
            if let Some(code) = scroll::code_for(d.part) {
                self.send_scroll(h, d.which, code, 0, &mut out);
            }
        }
        self.arm_repeat(h, 50);
        out
    }

    /// Mouse movement: drags the thumb, or tracks which part of a bar the pointer is over.
    pub fn nc_motion(&mut self, h: usize, lx: i32, ly: i32, out: &mut msg::Out) -> bool {
        if let Some(d) = self.sb_drag {
            if d.part == Part::Thumb {
                let Some((rect, vertical)) = self.bar_rect_of(d.hwnd, d.which) else { return true };
                let (lx2, ly2) = if d.hwnd == h { (lx, ly) } else { (lx, ly) };
                let along = if vertical { ly2 } else { lx2 };
                let pos = {
                    let w = &self.windows[&d.hwnd];
                    let bar = *w.sb.get(d.which).unwrap();
                    let lay = scroll::layout(&bar, rect, vertical);
                    let track_start = if vertical { lay.track.t } else { lay.track.l };
                    lay.pos_for_thumb(&bar, along - d.grab - track_start)
                };
                let changed = self.windows.get_mut(&d.hwnd).and_then(|w| w.sb.get_mut(d.which)).is_some_and(|b| std::mem::replace(&mut b.track, pos) != pos);
                if changed {
                    self.send_scroll(d.hwnd, d.which, scroll::SB_THUMBTRACK, pos, out);
                    self.paint_nc(d.hwnd);
                }
            }
            return true;
        }
        let hot = match self.nc_hit(h, lx, ly) {
            Nc::Scroll { which, part } if part != Part::None => Some((which, part)),
            _ => None,
        };
        let changed = self.windows.get_mut(&h).is_some_and(|w| std::mem::replace(&mut w.sb.hot, hot) != hot);
        if changed {
            self.paint_nc(h);
        }
        // anything else that was lit up elsewhere goes dark
        let others: Vec<usize> = self.windows.iter().filter(|(k, w)| **k != h && w.sb.hot.is_some()).map(|(k, _)| *k).collect();
        for o in others {
            if let Some(w) = self.windows.get_mut(&o) {
                w.sb.hot = None;
            }
            self.paint_nc(o);
        }
        matches!(self.nc_hit(h, lx, ly), Nc::Scroll { .. } | Nc::Corner)
    }

    fn bar_rect_of(&self, h: usize, which: i32) -> Option<(Rect, bool)> {
        let (v, hz) = self.bar_rects(h);
        match which {
            SB_VERT => v.map(|r| (r, true)),
            SB_HORZ => hz.map(|r| (r, false)),
            _ => None,
        }
    }

    fn send_scroll(&mut self, h: usize, which: i32, code: u32, pos: i32, out: &mut msg::Out) {
        let message = if which == SB_HORZ { WM_HSCROLL } else { WM_VSCROLL };
        let wparam = code as usize | ((pos as u16 as usize) << 16);
        let time = self.tick();
        out.sends.push(Msg { hwnd: h, message, wparam, lparam: 0, time, pt: self.pointer });
    }

    fn sb_finish(&mut self, d: SbDrag, out: &mut msg::Out) {
        self.sb_drag = None;
        self.stop_repeat(d.hwnd);
        if d.part == Part::Thumb {
            let pos = self.windows.get(&d.hwnd).and_then(|w| w.sb.get(d.which)).map_or(0, |b| b.track);
            self.send_scroll(d.hwnd, d.which, scroll::SB_THUMBPOSITION, pos, out);
        }
        self.send_scroll(d.hwnd, d.which, scroll::SB_ENDSCROLL, 0, out);
        if let Some(w) = self.windows.get_mut(&d.hwnd) {
            w.sb.pressed = None;
            if let Some(b) = w.sb.get_mut(d.which) {
                b.track = b.pos;
            }
        }
        self.paint_nc(d.hwnd);
    }
}

fn part_of(state: Option<(i32, Part)>, which: i32) -> Part {
    match state {
        Some((w, p)) if w == which => p,
        _ => Part::None,
    }
}

/// The 3D edges of Windows 10: a sunken two-pixel edge (`WS_EX_CLIENTEDGE`), or one-pixel borders.
pub fn draw_border(cv: &mut Canvas, r: Rect, style: u32, ex: u32) {
    let ring = |cv: &mut Canvas, r: Rect, tl: u32, br: u32| {
        cv.fill_rect(Rect::new(r.l, r.t, r.r, r.t + 1), tl);
        cv.fill_rect(Rect::new(r.l, r.t, r.l + 1, r.b), tl);
        cv.fill_rect(Rect::new(r.l, r.b - 1, r.r, r.b), br);
        cv.fill_rect(Rect::new(r.r - 1, r.t, r.r, r.b), br);
    };
    if ex & WS_EX_CLIENTEDGE != 0 {
        ring(cv, r, SUNKEN_OUTER_TL, SUNKEN_OUTER_BR);
        let inner = Rect::new(r.l + 1, r.t + 1, r.r - 1, r.b - 1);
        ring(cv, inner, SUNKEN_INNER_TL, SUNKEN_INNER_BR);
    } else if ex & WS_EX_STATICEDGE != 0 {
        ring(cv, r, SUNKEN_OUTER_TL, SUNKEN_OUTER_BR);
    } else if style & WS_BORDER != 0 {
        ring(cv, r, THIN_BORDER, THIN_BORDER);
    }
}
