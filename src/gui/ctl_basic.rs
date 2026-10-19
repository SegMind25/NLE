//! Common controls, part one: the progress bar, trackbar (slider), up-down (spin) control, status
//! bar, and tooltips (which only keep their tools; nothing is shown).
//!
//! They are ordinary window classes (`msctls_progress32`, ...) registered at start-up, so a program
//! can create them whether or not it called `InitCommonControls`.

use super::api;
use super::controls::{font_of, Ctl};
use super::gdi::sys_color;
use super::msg::{current_ansi, def_window_proc, kill_timer, set_timer};
use super::surface::*;
use super::wm::Class;
use super::*;

pub const NM_FIRST: i32 = 0;
pub const NM_CLICK: i32 = NM_FIRST - 2;
pub const NM_DBLCLK: i32 = NM_FIRST - 3;
pub const NM_RCLICK: i32 = NM_FIRST - 5;
pub const NM_RETURN: i32 = NM_FIRST - 4;
pub const NM_SETFOCUS: i32 = NM_FIRST - 7;
pub const NM_KILLFOCUS: i32 = NM_FIRST - 8;
pub const NM_CUSTOMDRAW: i32 = NM_FIRST - 12;

/// Sends `WM_NOTIFY` to the parent of `h` with an `NMHDR` (hwndFrom, idFrom, code) followed by `extra` bytes. Returns the
/// parent's result, and the (possibly changed) extra bytes.
pub unsafe fn notify_with(h: usize, code: i32, extra: &[u8]) -> (isize, Vec<u8>) {
    let (parent, id) = with(|g| g.windows.get(&h).map_or((0, 0), |w| (w.parent, w.id)));
    if parent == 0 {
        return (0, extra.to_vec());
    }
    let mut buf = vec![0u8; 24 + extra.len()];
    buf[0..8].copy_from_slice(&(h as u64).to_le_bytes());
    buf[8..16].copy_from_slice(&(id as u64).to_le_bytes());
    buf[16..20].copy_from_slice(&code.to_le_bytes());
    buf[24..].copy_from_slice(extra);
    let r = api::send(parent, WM_NOTIFY, id, buf.as_ptr() as isize);
    (r, buf[24..].to_vec())
}

pub unsafe fn notify(h: usize, code: i32) -> isize {
    notify_with(h, code, &[]).0
}

fn register(g: &mut Gui, name: &str, proc_: usize, style: u32) {
    g.register_class(Class { name: name.into(), atom: 0, style, wndproc: proc_, cls_extra: 0, wnd_extra: 0, hinstance: 0, icon: 0, cursor: 0, background: 0, menu_name: None, unicode: true, native: true });
}

pub fn register_classes(g: &mut Gui) {
    register(g, "msctls_progress32", progress_proc as *const () as usize, 0);
    register(g, "msctls_trackbar32", trackbar_proc as *const () as usize, 0);
    register(g, "msctls_updown32", updown_proc as *const () as usize, 0);
    register(g, "msctls_statusbar32", statusbar_proc as *const () as usize, 0x8);
    register(g, "tooltips_class32", tooltips_proc as *const () as usize, 0);
}

fn lo16(v: isize) -> i32 {
    (v & 0xffff) as u16 as i16 as i32
}

fn hi16(v: isize) -> i32 {
    ((v >> 16) & 0xffff) as u16 as i16 as i32
}

fn pack(lo: i32, hi: i32) -> isize {
    ((lo as u16 as u32) | ((hi as u16 as u32) << 16)) as isize
}

macro_rules! state_accessors {
    ($get:ident, $get_mut:ident, $variant:ident, $ty:ty) => {
        fn $get(g: &Gui, h: usize) -> Option<&$ty> {
            match &g.windows.get(&h)?.ctl {
                Ctl::$variant(s) => Some(s),
                _ => None,
            }
        }
        #[allow(dead_code)]
        fn $get_mut(g: &mut Gui, h: usize) -> Option<&mut $ty> {
            match &mut g.windows.get_mut(&h)?.ctl {
                Ctl::$variant(s) => Some(s),
                _ => None,
            }
        }
    };
}

// =======================================================================================================
// progress bar
// =======================================================================================================

#[derive(Clone, Debug)]
pub struct Progress {
    pub min: i32,
    pub max: i32,
    pub pos: i32,
    pub step: i32,
    pub bar: Option<u32>,
    pub bk: Option<u32>,
    /// 1 normal, 2 error, 3 paused
    pub state: u32,
    pub marquee: bool,
    pub phase: i32,
}

impl Default for Progress {
    fn default() -> Progress {
        Progress { min: 0, max: 100, pos: 0, step: 10, bar: None, bk: None, state: 1, marquee: false, phase: 0 }
    }
}

state_accessors!(progress_ref, progress_mut, Progress, Progress);

const PBS_VERTICAL: u32 = 4;
const PBS_MARQUEE: u32 = 8;
const MARQUEE_TIMER: usize = 0x3D;

unsafe fn paint_progress(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return };
        let (cw, ch) = w.client_size();
        let vertical = w.style & PBS_VERTICAL != 0;
        let Some(p) = progress_ref(g, h).cloned() else { return };
        let enabled = g.is_enabled(h);
        let trough = p.bk.unwrap_or(0xE6E6E6);
        let color = p.bar.unwrap_or(match p.state {
            2 => 0xDA2626,
            3 => 0xFFB900,
            _ => 0x06B025,
        });
        let area = Rect::new(0, 0, cw, ch);
        g.draw(hdc, |cv, _| {
            cv.fill_rect(area, trough);
            cv.frame_rect(area, 0xBCBCBC, 1);
            let inner = Rect::new(1, 1, cw - 1, ch - 1);
            let len = if vertical { inner.height() } else { inner.width() };
            let (from, to) = if p.marquee {
                let seg = (len * 3 / 10).max(8);
                let start = (p.phase % (len + seg)) - seg;
                (start.max(0), (start + seg).min(len))
            } else {
                let span = (p.max - p.min).max(1) as i64;
                (0, ((p.pos - p.min).clamp(0, p.max - p.min) as i64 * len as i64 / span) as i32)
            };
            if to > from {
                let fill = if vertical { Rect::new(inner.l, inner.b - to, inner.r, inner.b - from) } else { Rect::new(inner.l + from, inner.t, inner.l + to, inner.b) };
                cv.fill_rect(fill, if enabled { color } else { 0xA0A0A0 });
            }
        });
    });
    api::end_paint(h, ps.as_ptr());
}

pub unsafe extern "win64" fn progress_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    let redraw = |g: &mut Gui| g.invalidate(h, None, false);
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::Progress(Progress::default());
                    // PBS_MARQUEE starts animating at once
                }
            });
            1
        }
        WM_CREATE => {
            if with(|g| g.windows.get(&h).is_some_and(|w| w.style & PBS_MARQUEE != 0)) {
                with(|g| {
                    if let Some(p) = progress_mut(g, h) {
                        p.marquee = true;
                    }
                });
                set_timer(h, MARQUEE_TIMER, 30, 0);
            }
            0
        }
        WM_DESTROY => {
            kill_timer(h, MARQUEE_TIMER);
            0
        }
        WM_TIMER if wp == MARQUEE_TIMER => {
            with(|g| {
                if let Some(p) = progress_mut(g, h) {
                    p.phase += 4;
                }
                redraw(g);
            });
            0
        }
        WM_PAINT => {
            paint_progress(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_ENABLE => {
            with(redraw);
            0
        }
        0x401 => with(|g| {
            // PBM_SETRANGE: the low word is the minimum, the high word the maximum
            let old = progress_ref(g, h).map_or(0, |p| pack(p.min, p.max));
            if let Some(p) = progress_mut(g, h) {
                p.min = lo16(lp);
                p.max = hi16(lp);
                p.pos = p.pos.clamp(p.min, p.max.max(p.min));
            }
            redraw(g);
            old
        }),
        0x406 => with(|g| {
            // PBM_SETRANGE32
            let old = progress_ref(g, h).map_or(0, |p| pack(p.min, p.max));
            if let Some(p) = progress_mut(g, h) {
                p.min = wp as i32;
                p.max = lp as i32;
                p.pos = p.pos.clamp(p.min, p.max.max(p.min));
            }
            redraw(g);
            old
        }),
        0x407 => with(|g| {
            // PBM_GETRANGE: wParam TRUE asks for the low limit; lParam, when given, receives both
            let Some(p) = progress_ref(g, h).cloned() else { return 0 };
            if lp != 0 {
                *(lp as *mut i32) = p.min;
                *(lp as *mut i32).add(1) = p.max;
            }
            (if wp != 0 { p.min } else { p.max }) as isize
        }),
        0x402 => with(|g| {
            // PBM_SETPOS
            let old = progress_ref(g, h).map_or(0, |p| p.pos);
            if let Some(p) = progress_mut(g, h) {
                p.pos = (wp as i32).clamp(p.min, p.max.max(p.min));
            }
            redraw(g);
            old as isize
        }),
        0x403 => with(|g| {
            // PBM_DELTAPOS
            let old = progress_ref(g, h).map_or(0, |p| p.pos);
            if let Some(p) = progress_mut(g, h) {
                p.pos = (p.pos + wp as i32).clamp(p.min, p.max.max(p.min));
            }
            redraw(g);
            old as isize
        }),
        0x404 => with(|g| {
            // PBM_SETSTEP
            let old = progress_ref(g, h).map_or(0, |p| p.step);
            if let Some(p) = progress_mut(g, h) {
                p.step = wp as i32;
            }
            old as isize
        }),
        0x405 => with(|g| {
            // PBM_STEPIT: advance by the step; past the maximum the bar starts over
            let old = progress_ref(g, h).map_or(0, |p| p.pos);
            if let Some(p) = progress_mut(g, h) {
                p.pos += p.step;
                if p.pos > p.max {
                    p.pos = p.min + (p.pos - p.max - 1).max(0) % (p.max - p.min).max(1);
                }
            }
            redraw(g);
            old as isize
        }),
        0x408 => with(|g| progress_ref(g, h).map_or(0, |p| p.pos as isize)),
        0x40D => with(|g| progress_ref(g, h).map_or(0, |p| p.step as isize)),
        0x409 => with(|g| {
            // PBM_SETBARCOLOR (CLR_DEFAULT 0xFF000000 restores the theme colour)
            if let Some(p) = progress_mut(g, h) {
                p.bar = if lp as u32 == 0xFF00_0000 { None } else { Some(from_colorref(lp as u32)) };
            }
            redraw(g);
            0
        }),
        0x2001 => with(|g| {
            // PBM_SETBKCOLOR
            if let Some(p) = progress_mut(g, h) {
                p.bk = if lp as u32 == 0xFF00_0000 { None } else { Some(from_colorref(lp as u32)) };
            }
            redraw(g);
            0
        }),
        0x40F => with(|g| progress_ref(g, h).and_then(|p| p.bar).map_or(0xFF00_0000, to_colorref) as isize),
        0x40E => with(|g| progress_ref(g, h).and_then(|p| p.bk).map_or(0xFF00_0000, to_colorref) as isize),
        0x40A => {
            // PBM_SETMARQUEE
            let on = wp != 0;
            with(|g| {
                if let Some(p) = progress_mut(g, h) {
                    p.marquee = on;
                }
                redraw(g);
            });
            if on {
                set_timer(h, MARQUEE_TIMER, if lp > 0 { lp as u32 } else { 30 }, 0);
            } else {
                kill_timer(h, MARQUEE_TIMER);
            }
            1
        }
        0x410 => with(|g| {
            // PBM_SETSTATE
            let old = progress_ref(g, h).map_or(1, |p| p.state);
            if let Some(p) = progress_mut(g, h) {
                p.state = wp as u32;
            }
            redraw(g);
            old as isize
        }),
        0x411 => with(|g| progress_ref(g, h).map_or(1, |p| p.state as isize)),
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

// =======================================================================================================
// trackbar
// =======================================================================================================

#[derive(Clone, Debug)]
pub struct Trackbar {
    pub min: i32,
    pub max: i32,
    pub pos: i32,
    pub page: i32,
    pub line: i32,
    pub tic_freq: i32,
    pub sel: (i32, i32),
    pub dragging: bool,
    pub grab: i32,
    pub hot: bool,
}

impl Default for Trackbar {
    fn default() -> Trackbar {
        Trackbar { min: 0, max: 100, pos: 0, page: 0, line: 1, tic_freq: 1, sel: (0, 0), dragging: false, grab: 0, hot: false }
    }
}

state_accessors!(track_ref, track_mut, Trackbar, Trackbar);

const TBS_VERT: u32 = 2;
const TBS_NOTICKS: u32 = 0x10;
const TBS_ENABLESELRANGE: u32 = 0x20;
const TBS_NOTHUMB: u32 = 0x80;
const THUMB_W: i32 = 11;
const THUMB_L: i32 = 21;
const CHANNEL_INSET: i32 = 6;

impl Trackbar {
    fn page_size(&self) -> i32 {
        if self.page > 0 {
            self.page
        } else {
            ((self.max - self.min) / 5).max(1)
        }
    }
}

/// The channel rectangle and the thumb rectangle in client coordinates.
fn track_geometry(tb: &Trackbar, cw: i32, ch: i32, vertical: bool, pos: i32) -> (Rect, Rect) {
    let len = if vertical { ch } else { cw };
    let cross = if vertical { cw } else { ch };
    let travel = (len - 2 * CHANNEL_INSET - THUMB_W).max(1);
    let span = (tb.max - tb.min).max(1) as i64;
    let off = CHANNEL_INSET + ((pos - tb.min).clamp(0, tb.max - tb.min) as i64 * travel as i64 / span) as i32;
    let mid = cross / 2;
    if vertical {
        (Rect::new(mid - 2, CHANNEL_INSET, mid + 2, len - CHANNEL_INSET), Rect::new(mid - THUMB_L / 2, off, mid + THUMB_L / 2 + 1, off + THUMB_W))
    } else {
        (Rect::new(CHANNEL_INSET, mid - 2, len - CHANNEL_INSET, mid + 2), Rect::new(off, mid - THUMB_L / 2, off + THUMB_W, mid + THUMB_L / 2 + 1))
    }
}

fn track_pos_at(tb: &Trackbar, cw: i32, ch: i32, vertical: bool, coord: i32) -> i32 {
    let len = if vertical { ch } else { cw };
    let travel = (len - 2 * CHANNEL_INSET - THUMB_W).max(1);
    let rel = (coord - CHANNEL_INSET - THUMB_W / 2).clamp(0, travel) as i64;
    (tb.min as i64 + (rel * (tb.max - tb.min) as i64 + travel as i64 / 2) / travel as i64) as i32
}

unsafe fn track_notify(h: usize, code: u32, pos: i32) {
    let (parent, vertical) = with(|g| (g.windows.get(&h).map_or(0, |w| w.parent), g.windows.get(&h).is_some_and(|w| w.style & TBS_VERT != 0)));
    if parent != 0 {
        api::send(parent, if vertical { WM_VSCROLL } else { WM_HSCROLL }, code as usize | ((pos as u16 as usize) << 16), h as isize);
    }
}

unsafe fn track_set(h: usize, pos: i32, notify_code: Option<u32>) {
    let changed = with(|g| {
        let Some(tb) = track_mut(g, h) else { return None };
        let new = pos.clamp(tb.min, tb.max);
        let moved = tb.pos != new;
        tb.pos = new;
        g.invalidate(h, None, false);
        Some((moved, new))
    });
    if let (Some((_, new)), Some(code)) = (changed, notify_code) {
        track_notify(h, code, new);
    }
}

unsafe fn paint_trackbar(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return };
        let (cw, ch) = w.client_size();
        let (vertical, style) = (w.style & TBS_VERT != 0, w.style);
        let Some(tb) = track_ref(g, h).cloned() else { return };
        let enabled = g.is_enabled(h);
        let focused = g.focus == h;
        let (channel, thumb) = track_geometry(&tb, cw, ch, vertical, tb.pos);
        g.fill_rect(hdc, Rect::new(0, 0, cw, ch), &Brush::Solid(sys_color(15)));
        g.draw(hdc, |cv, _| {
            cv.fill_rect(channel, 0xD6D6D6);
            cv.frame_rect(channel, 0xA0A0A0, 1);
            if style & TBS_ENABLESELRANGE != 0 && tb.sel.1 > tb.sel.0 {
                let (_, a) = track_geometry(&tb, cw, ch, vertical, tb.sel.0);
                let (_, b) = track_geometry(&tb, cw, ch, vertical, tb.sel.1);
                let sel = if vertical { Rect::new(channel.l + 1, a.t, channel.r - 1, b.b) } else { Rect::new(a.l, channel.t + 1, b.r, channel.b - 1) };
                cv.fill_rect(sel, 0x0078D7);
            }
            if style & TBS_NOTICKS == 0 {
                let freq = tb.tic_freq.max(1);
                let mut v = tb.min;
                while v <= tb.max {
                    let (_, t) = track_geometry(&tb, cw, ch, vertical, v);
                    let c = if vertical { (t.t + t.b) / 2 } else { (t.l + t.r) / 2 };
                    if vertical {
                        cv.fill_rect(Rect::new(channel.r + 4, c, channel.r + 8, c + 1), 0x767676);
                    } else {
                        cv.fill_rect(Rect::new(c, channel.b + 4, c + 1, channel.b + 8), 0x767676);
                    }
                    v += freq;
                }
            }
            if style & TBS_NOTHUMB == 0 {
                let color = if !enabled { 0xB0B0B0 } else if tb.dragging { 0x005499 } else if tb.hot || focused { 0x0067BE } else { 0x0078D7 };
                cv.fill_rect(thumb, color);
            }
        });
    });
    api::end_paint(h, ps.as_ptr());
}

pub unsafe extern "win64" fn trackbar_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::Trackbar(Trackbar::default());
                }
            });
            1
        }
        WM_PAINT => {
            paint_trackbar(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS => {
            with(|g| g.invalidate(h, None, false));
            0
        }
        WM_GETDLGCODE => 1,
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if !with(|g| g.is_enabled(h)) {
                return 0;
            }
            api::set_focus(h);
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            let action = with(|g| {
                let w = g.windows.get(&h)?;
                let (cw, ch) = w.client_size();
                let vertical = w.style & TBS_VERT != 0;
                let tb = track_ref(g, h)?.clone();
                let (_, thumb) = track_geometry(&tb, cw, ch, vertical, tb.pos);
                let coord = if vertical { y } else { x };
                if thumb.contains(x, y) {
                    g.capture = h;
                    if let Some(t) = track_mut(g, h) {
                        t.dragging = true;
                        t.grab = coord - if vertical { thumb.t } else { thumb.l };
                    }
                    g.invalidate(h, None, false);
                    Some(None)
                } else {
                    let before = coord < if vertical { thumb.t } else { thumb.l };
                    Some(Some((if before { -1 } else { 1 }) * tb.page_size()))
                }
            });
            if let Some(Some(delta)) = action {
                let pos = with(|g| track_ref(g, h).map_or(0, |t| t.pos));
                track_set(h, pos + delta, Some(if delta < 0 { 2 } else { 3 }));
                track_notify(h, 8, 0);
            }
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            let drag = with(|g| {
                let w = g.windows.get(&h)?;
                let (cw, ch) = w.client_size();
                let vertical = w.style & TBS_VERT != 0;
                let tb = track_ref(g, h)?.clone();
                let (_, thumb) = track_geometry(&tb, cw, ch, vertical, tb.pos);
                let inside = thumb.contains(x, y);
                if tb.hot != inside {
                    if let Some(t) = track_mut(g, h) {
                        t.hot = inside;
                    }
                    g.invalidate(h, None, false);
                }
                if inside {
                    g.track_leave(h);
                }
                if tb.dragging && g.capture == h {
                    let coord = if vertical { y } else { x };
                    Some(track_pos_at(&tb, cw, ch, vertical, coord - tb.grab + THUMB_W / 2))
                } else {
                    None
                }
            });
            if let Some(pos) = drag {
                track_set(h, pos, Some(5));
            }
            0
        }
        WM_MOUSELEAVE => {
            with(|g| {
                if let Some(t) = track_mut(g, h) {
                    t.hot = false;
                }
                g.invalidate(h, None, false);
            });
            0
        }
        WM_LBUTTONUP => {
            let was = with(|g| {
                let was = track_ref(g, h).is_some_and(|t| t.dragging);
                if let Some(t) = track_mut(g, h) {
                    t.dragging = false;
                }
                if g.capture == h {
                    g.capture = 0;
                }
                g.invalidate(h, None, false);
                was
            });
            if was {
                let pos = with(|g| track_ref(g, h).map_or(0, |t| t.pos));
                track_notify(h, 4, pos);
                track_notify(h, 8, 0);
            }
            0
        }
        WM_KEYDOWN => {
            let (vertical, tb) = with(|g| (g.windows.get(&h).is_some_and(|w| w.style & TBS_VERT != 0), track_ref(g, h).cloned()));
            let Some(tb) = tb else { return 0 };
            let (target, code) = match wp as u32 {
                0x25 | 0x26 => (tb.pos - tb.line, 0),
                0x27 | 0x28 => (tb.pos + tb.line, 1),
                0x21 => (tb.pos - tb.page_size(), 2),
                0x22 => (tb.pos + tb.page_size(), 3),
                0x24 => (tb.min, 6),
                0x23 => (tb.max, 7),
                _ => return 0,
            };
            // for a vertical trackbar the top is the minimum: Up decreases, as written above
            let _ = vertical;
            track_set(h, target, Some(code));
            track_notify(h, 8, 0);
            0
        }
        0x400 => with(|g| track_ref(g, h).map_or(0, |t| t.pos as isize)),
        0x401 => with(|g| track_ref(g, h).map_or(0, |t| t.min as isize)),
        0x402 => with(|g| track_ref(g, h).map_or(0, |t| t.max as isize)),
        0x405 => {
            track_set(h, lp as i32, None);
            0
        }
        0x406 => with(|g| {
            if let Some(t) = track_mut(g, h) {
                t.min = lo16(lp);
                t.max = hi16(lp);
                t.pos = t.pos.clamp(t.min, t.max.max(t.min));
            }
            g.invalidate(h, None, false);
            0
        }),
        0x407 => with(|g| {
            if let Some(t) = track_mut(g, h) {
                t.min = lp as i32;
                t.pos = t.pos.max(t.min);
            }
            g.invalidate(h, None, false);
            0
        }),
        0x408 => with(|g| {
            if let Some(t) = track_mut(g, h) {
                t.max = lp as i32;
                t.pos = t.pos.min(t.max);
            }
            g.invalidate(h, None, false);
            0
        }),
        0x414 => with(|g| {
            if let Some(t) = track_mut(g, h) {
                t.tic_freq = (wp as i32).max(1);
            }
            g.invalidate(h, None, false);
            0
        }),
        0x415 => with(|g| {
            let old = track_ref(g, h).map_or(0, |t| t.page_size());
            if let Some(t) = track_mut(g, h) {
                t.page = lp as i32;
            }
            old as isize
        }),
        0x416 => with(|g| track_ref(g, h).map_or(0, |t| t.page_size() as isize)),
        0x417 => with(|g| {
            let old = track_ref(g, h).map_or(0, |t| t.line);
            if let Some(t) = track_mut(g, h) {
                t.line = (lp as i32).max(1);
            }
            old as isize
        }),
        0x418 => with(|g| track_ref(g, h).map_or(0, |t| t.line as isize)),
        0x40A => with(|g| {
            if let Some(t) = track_mut(g, h) {
                t.sel = (lo16(lp), hi16(lp));
            }
            g.invalidate(h, None, false);
            0
        }),
        0x40B => with(|g| {
            if let Some(t) = track_mut(g, h) {
                t.sel.0 = lp as i32;
            }
            0
        }),
        0x40C => with(|g| {
            if let Some(t) = track_mut(g, h) {
                t.sel.1 = lp as i32;
            }
            0
        }),
        0x411 => with(|g| track_ref(g, h).map_or(0, |t| t.sel.0 as isize)),
        0x412 => with(|g| track_ref(g, h).map_or(0, |t| t.sel.1 as isize)),
        0x413 => with(|g| {
            if let Some(t) = track_mut(g, h) {
                t.sel = (0, 0);
            }
            g.invalidate(h, None, false);
            0
        }),
        0x419 => with(|g| {
            if lp != 0 {
                let Some(w) = g.windows.get(&h) else { return 0 };
                let (cw, ch) = w.client_size();
                let vertical = w.style & TBS_VERT != 0;
                if let Some(tb) = track_ref(g, h) {
                    let (_, t) = track_geometry(tb, cw, ch, vertical, tb.pos);
                    let r = lp as *mut i32;
                    *r = t.l;
                    *r.add(1) = t.t;
                    *r.add(2) = t.r;
                    *r.add(3) = t.b;
                }
            }
            0
        }),
        0x409 | 0x404 | 0x41B | 0x41D | 0x41F => 0,
        0x410 => with(|g| track_ref(g, h).map_or(0, |t| ((t.max - t.min) / t.tic_freq.max(1) + 1) as isize)),
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

// =======================================================================================================
// up-down (spin) control
// =======================================================================================================

#[derive(Clone, Debug)]
pub struct UpDown {
    pub min: i32,
    pub max: i32,
    pub pos: i32,
    pub buddy: usize,
    pub base: u32,
    pub pressed: u8,
    pub hot: u8,
}

impl Default for UpDown {
    fn default() -> UpDown {
        // the default range runs from 100 down to 0, as on Windows
        UpDown { min: 100, max: 0, pos: 0, buddy: 0, base: 10, pressed: 0, hot: 0 }
    }
}

state_accessors!(ud_ref, ud_mut, UpDown, UpDown);

const UDS_WRAP: u32 = 1;
const UDS_SETBUDDYINT: u32 = 2;
const UDS_ALIGNRIGHT: u32 = 4;
const UDS_ALIGNLEFT: u32 = 8;
const UDS_AUTOBUDDY: u32 = 0x10;
const UDS_HORZ: u32 = 0x40;
const UDS_NOTHOUSANDS: u32 = 0x80;
const UDN_DELTAPOS: i32 = -722;
const UD_TIMER: usize = 0x3E;

fn format_pos(pos: i32, base: u32, nothousands: bool) -> String {
    if base == 16 {
        format!("{:X}", pos as u32)
    } else if nothousands {
        pos.to_string()
    } else {
        let digits = pos.abs().to_string();
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(c);
        }
        if pos < 0 {
            out.insert(0, '-');
        }
        out
    }
}

unsafe fn ud_sync_buddy(h: usize) {
    let info = with(|g| {
        let st = ud_ref(g, h)?;
        let style = g.windows.get(&h)?.style;
        (style & UDS_SETBUDDYINT != 0 && st.buddy != 0).then(|| (st.buddy, format_pos(st.pos, st.base, style & UDS_NOTHOUSANDS != 0)))
    });
    if let Some((buddy, text)) = info {
        let w = crate::runtime::strings::to_wide_nul(&text);
        api::send_ansi(buddy, WM_SETTEXT, 0, w.as_ptr() as isize, false);
    }
}

/// Moves the position by `delta` as an arrow click does, asking the parent first (`UDN_DELTAPOS`).
unsafe fn ud_step(h: usize, delta: i32) {
    let Some((pos, min, max, style, vertical)) = with(|g| {
        let st = ud_ref(g, h)?;
        let w = g.windows.get(&h)?;
        Some((st.pos, st.min, st.max, w.style, w.style & UDS_HORZ == 0))
    }) else {
        return;
    };
    let (lo, hi) = (min.min(max), min.max(max));
    // an up arrow moves toward the larger end of the range, whichever way it was given
    let signed = if min > max { -delta } else { delta };
    let mut new = pos + signed;
    if new > hi {
        new = if style & UDS_WRAP != 0 { lo } else { hi };
    } else if new < lo {
        new = if style & UDS_WRAP != 0 { hi } else { lo };
    }
    let mut nm = [0u8; 8];
    nm[0..4].copy_from_slice(&pos.to_le_bytes());
    nm[4..8].copy_from_slice(&signed.to_le_bytes());
    if notify_with(h, UDN_DELTAPOS, &nm).0 != 0 {
        return;
    }
    with(|g| {
        if let Some(st) = ud_mut(g, h) {
            st.pos = new;
        }
        g.invalidate(h, None, false);
    });
    ud_sync_buddy(h);
    let parent = with(|g| g.windows.get(&h).map_or(0, |w| w.parent));
    if parent != 0 {
        api::send(parent, if vertical { WM_VSCROLL } else { WM_HSCROLL }, 4usize | ((new as u16 as usize) << 16), h as isize);
        api::send(parent, if vertical { WM_VSCROLL } else { WM_HSCROLL }, 8, h as isize);
    }
}

/// Which arrow is at (x, y): 1 for the up (or right) one, 2 for the down (or left) one, 0 for none.
fn ud_arrow_at(cw: i32, ch: i32, horz: bool, x: i32, y: i32) -> u8 {
    if x < 0 || y < 0 || x >= cw || y >= ch {
        return 0;
    }
    if horz {
        if x >= cw / 2 {
            1
        } else {
            2
        }
    } else if y < ch / 2 {
        1
    } else {
        2
    }
}

unsafe fn paint_updown(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return };
        let (cw, ch) = w.client_size();
        let horz = w.style & UDS_HORZ != 0;
        let Some(st) = ud_ref(g, h).cloned() else { return };
        let enabled = g.is_enabled(h);
        g.draw(hdc, |cv, _| {
            let halves = if horz { [(Rect::new(cw / 2, 0, cw, ch), 1u8), (Rect::new(0, 0, cw / 2, ch), 2u8)] } else { [(Rect::new(0, 0, cw, ch / 2), 1u8), (Rect::new(0, ch / 2, cw, ch), 2u8)] };
            for (r, which) in halves {
                let bg = if !enabled { 0xCCCCCC } else if st.pressed == which { 0xCCE4F7 } else if st.hot == which { 0xE5F1FB } else { 0xE1E1E1 };
                cv.fill_rect(r, bg);
                cv.frame_rect(r, if st.pressed == which { 0x005499 } else { 0xADADAD }, 1);
                let (cx, cy) = ((r.l + r.r) / 2, (r.t + r.b) / 2);
                let ink = if enabled { 0x404040 } else { 0x838383 };
                let tri: [(i32, i32); 3] = match (horz, which) {
                    (false, 1) => [(cx - 3, cy + 1), (cx + 3, cy + 1), (cx, cy - 2)],
                    (false, _) => [(cx - 3, cy - 1), (cx + 3, cy - 1), (cx, cy + 2)],
                    (true, 1) => [(cx - 1, cy - 3), (cx - 1, cy + 3), (cx + 2, cy)],
                    (true, _) => [(cx + 1, cy - 3), (cx + 1, cy + 3), (cx - 2, cy)],
                };
                cv.fill_polygon_color(&tri, ink, false);
            }
        });
    });
    api::end_paint(h, ps.as_ptr());
}

/// Places an up-down control next to its buddy (`UDS_ALIGNRIGHT` / `UDS_ALIGNLEFT`), shrinking the buddy to make room.
fn ud_attach(g: &mut Gui, h: usize) {
    let (style, buddy) = match (g.windows.get(&h), ud_ref(g, h)) {
        (Some(w), Some(s)) => (w.style, s.buddy),
        _ => return,
    };
    if buddy == 0 || style & (UDS_ALIGNRIGHT | UDS_ALIGNLEFT) == 0 {
        return;
    }
    let Some(br) = g.windows.get(&buddy).map(|w| w.rect) else { return };
    let width = g.windows.get(&h).map_or(15, |w| w.rect.width()).clamp(12, 24);
    if style & UDS_ALIGNRIGHT != 0 {
        g.set_rect(buddy, Rect::new(br.l, br.t, br.r - width, br.b));
        g.set_rect(h, Rect::new(br.r - width, br.t, br.r, br.b));
    } else {
        g.set_rect(buddy, Rect::new(br.l + width, br.t, br.r, br.b));
        g.set_rect(h, Rect::new(br.l, br.t, br.l + width, br.b));
    }
}

pub unsafe extern "win64" fn updown_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::UpDown(UpDown::default());
                }
            });
            1
        }
        WM_CREATE => {
            // UDS_AUTOBUDDY: the control just before this one in the z-order
            with(|g| {
                let (style, parent) = g.windows.get(&h).map_or((0, 0), |w| (w.style, w.parent));
                if style & UDS_AUTOBUDDY != 0 {
                    let prev = g.windows.get(&parent).and_then(|p| p.children.iter().position(|c| *c == h).and_then(|i| i.checked_sub(1)).map(|i| p.children[i]));
                    if let (Some(b), Some(st)) = (prev, ud_mut(g, h)) {
                        st.buddy = b;
                    }
                }
                ud_attach(g, h);
            });
            ud_sync_buddy(h);
            0
        }
        WM_DESTROY => {
            kill_timer(h, UD_TIMER);
            0
        }
        WM_PAINT => {
            paint_updown(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_ENABLE => {
            with(|g| g.invalidate(h, None, false));
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if !with(|g| g.is_enabled(h)) {
                return 0;
            }
            let which = with(|g| {
                let w = g.windows.get(&h)?;
                let (cw, ch) = w.client_size();
                let which = ud_arrow_at(cw, ch, w.style & UDS_HORZ != 0, lo_signed(lp), hi_signed(lp));
                if which != 0 {
                    g.capture = h;
                    if let Some(st) = ud_mut(g, h) {
                        st.pressed = which;
                    }
                    g.invalidate(h, None, false);
                }
                Some(which)
            });
            if let Some(w) = which.filter(|w| *w != 0) {
                ud_step(h, if w == 1 { 1 } else { -1 });
                set_timer(h, UD_TIMER, 400, 0);
            }
            0
        }
        WM_TIMER if wp == UD_TIMER => {
            let pressed = with(|g| ud_ref(g, h).map_or(0, |s| s.pressed));
            if pressed != 0 {
                set_timer(h, UD_TIMER, 50, 0);
                ud_step(h, if pressed == 1 { 1 } else { -1 });
            }
            0
        }
        WM_LBUTTONUP => {
            kill_timer(h, UD_TIMER);
            with(|g| {
                if let Some(st) = ud_mut(g, h) {
                    st.pressed = 0;
                }
                if g.capture == h {
                    g.capture = 0;
                }
                g.invalidate(h, None, false);
            });
            0
        }
        WM_MOUSEMOVE => {
            with(|g| {
                let Some(w) = g.windows.get(&h) else { return };
                let (cw, ch) = w.client_size();
                let which = ud_arrow_at(cw, ch, w.style & UDS_HORZ != 0, lo_signed(lp), hi_signed(lp));
                if ud_ref(g, h).is_some_and(|s| s.hot != which) {
                    if let Some(st) = ud_mut(g, h) {
                        st.hot = which;
                    }
                    g.invalidate(h, None, false);
                }
                if which != 0 {
                    g.track_leave(h);
                }
            });
            0
        }
        WM_MOUSELEAVE => {
            with(|g| {
                if let Some(st) = ud_mut(g, h) {
                    st.hot = 0;
                }
                g.invalidate(h, None, false);
            });
            0
        }
        0x465 => with(|g| {
            // UDM_SETRANGE: the low word is the upper limit, the high word the lower
            if let Some(st) = ud_mut(g, h) {
                st.max = lo16(lp);
                st.min = hi16(lp);
                st.pos = st.pos.clamp(st.min.min(st.max), st.min.max(st.max));
            }
            0
        }),
        0x466 => with(|g| ud_ref(g, h).map_or(0, |s| pack(s.max, s.min))),
        0x46F => with(|g| {
            if let Some(st) = ud_mut(g, h) {
                st.min = wp as i32;
                st.max = lp as i32;
                st.pos = st.pos.clamp(st.min.min(st.max), st.min.max(st.max));
            }
            0
        }),
        0x470 => with(|g| {
            if let Some(st) = ud_ref(g, h) {
                if wp != 0 {
                    *(wp as *mut i32) = st.min;
                }
                if lp != 0 {
                    *(lp as *mut i32) = st.max;
                }
            }
            0
        }),
        0x467 | 0x471 => {
            let (old, new) = with(|g| {
                let st = ud_mut(g, h).unwrap();
                let old = st.pos;
                let value = if msg == 0x467 { lo16(lp) } else { lp as i32 };
                st.pos = value.clamp(st.min.min(st.max), st.min.max(st.max));
                let new = st.pos;
                g.invalidate(h, None, false);
                (old, new)
            });
            let _ = new;
            ud_sync_buddy(h);
            old as isize
        }
        0x468 => with(|g| ud_ref(g, h).map_or(0, |s| (s.pos as u16) as isize)),
        0x472 => {
            if lp != 0 {
                *(lp as *mut i32) = 0;
            }
            with(|g| ud_ref(g, h).map_or(0, |s| s.pos as isize))
        }
        0x469 => {
            let old = with(|g| {
                let old = ud_ref(g, h).map_or(0, |s| s.buddy);
                if let Some(st) = ud_mut(g, h) {
                    st.buddy = wp;
                }
                ud_attach(g, h);
                old
            });
            ud_sync_buddy(h);
            old as isize
        }
        0x46A => with(|g| ud_ref(g, h).map_or(0, |s| s.buddy as isize)),
        0x46D => with(|g| {
            let old = ud_ref(g, h).map_or(10, |s| s.base);
            if let Some(st) = ud_mut(g, h) {
                if wp == 10 || wp == 16 {
                    st.base = wp as u32;
                }
            }
            old as isize
        }),
        0x46E => with(|g| ud_ref(g, h).map_or(10, |s| s.base as isize)),
        0x46B | 0x46C => 1,
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

// =======================================================================================================
// status bar
// =======================================================================================================

#[derive(Clone, Debug, Default)]
pub struct StatusBar {
    /// Right edge of each part in pixels; -1 means "to the end".
    pub edges: Vec<i32>,
    pub texts: Vec<String>,
    pub simple: bool,
    pub simple_text: String,
    pub min_height: i32,
}

state_accessors!(sb_ref, sb_mut, Status, StatusBar);

const SBARS_SIZEGRIP: u32 = 0x100;
const CCS_NOPARENTALIGN: u32 = 8;
const CCS_TOP: u32 = 1;
const CCS_NORESIZE: u32 = 4;

fn bar_height(g: &Gui, h: usize) -> i32 {
    let min = sb_ref(g, h).map_or(0, |s| s.min_height);
    (g.sized_font(font_of(g, h)).height() + 8).max(min)
}

/// The rectangle of part `i` in client coordinates.
fn part_rect(st: &StatusBar, cw: i32, ch: i32, i: usize) -> Rect {
    let right = |k: usize| -> i32 {
        match st.edges.get(k) {
            Some(-1) | None => cw,
            Some(e) => (*e).min(cw),
        }
    };
    let left = if i == 0 { 0 } else { right(i - 1) };
    Rect::new(left, 0, right(i), ch)
}

unsafe fn sb_auto_size(h: usize) {
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return };
        if w.style & (CCS_NOPARENTALIGN | CCS_NORESIZE) != 0 {
            return;
        }
        let top = w.style & CCS_TOP != 0;
        let parent = w.parent;
        let Some((pw, ph)) = g.windows.get(&parent).map(|p| p.client_size()) else { return };
        let bh = bar_height(g, h);
        let r = if top { Rect::new(0, 0, pw, bh) } else { Rect::new(0, ph - bh, pw, ph) };
        if g.windows.get(&h).map(|w| w.rect) != Some(r) {
            g.set_rect(h, r);
        }
    });
}

unsafe fn paint_statusbar(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return };
        let (cw, ch) = w.client_size();
        let grip = w.style & SBARS_SIZEGRIP != 0;
        let Some(st) = sb_ref(g, h).cloned() else { return };
        let f = font_of(g, h);
        g.select_object(hdc, f);
        g.fill_rect(hdc, Rect::new(0, 0, cw, ch), &Brush::Solid(sys_color(15)));
        let font = g.sized_font(f);
        g.draw(hdc, |cv, _| {
            cv.fill_rect(Rect::new(0, 0, cw, 1), 0xD5D5D5);
            let ty = (ch - font.height()) / 2 + 1;
            if st.simple {
                font.draw(cv, 6, ty, &st.simple_text, 0, None);
            } else {
                let count = st.edges.len().max(1);
                for i in 0..count {
                    let r = part_rect(&st, cw, ch, i);
                    if i + 1 < count {
                        cv.fill_rect(Rect::new(r.r - 1, 3, r.r, ch - 3), 0xD0D0D0);
                    }
                    let text = st.texts.get(i).map_or("", |t| t.as_str());
                    let saved = cv.clip;
                    cv.clip = cv.clip.intersect(&Rect::new(r.l, r.t, r.r - 1, r.b).offset(cv.ox, cv.oy));
                    font.draw(cv, r.l + 6, ty, text, 0, None);
                    cv.clip = saved;
                }
            }
            if grip {
                for k in 0..3 {
                    for j in 0..=k {
                        let (x, y) = (cw - 4 - j * 4, ch - 4 - (k - j) * 4);
                        cv.fill_rect(Rect::new(x, y, x + 2, y + 2), 0x909090);
                    }
                }
            }
        });
    });
    api::end_paint(h, ps.as_ptr());
}

unsafe fn sb_text_arg(lp: isize, wide: bool) -> String {
    if lp == 0 {
        String::new()
    } else if wide {
        crate::runtime::strings::w_ptr_to_string(lp as *const u16)
    } else {
        crate::runtime::strings::a_to_string(lp as *const u8)
    }
}

pub unsafe extern "win64" fn statusbar_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    let st = StatusBar { edges: vec![-1], texts: vec![String::new()], ..Default::default() };
                    w.ctl = Ctl::Status(Box::new(st));
                }
            });
            1
        }
        WM_CREATE => {
            sb_auto_size(h);
            0
        }
        WM_SIZE => {
            sb_auto_size(h);
            with(|g| g.invalidate(h, None, true));
            0
        }
        WM_PAINT => {
            paint_statusbar(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETFONT => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.font = wp;
                }
                g.invalidate(h, None, true);
            });
            0
        }
        WM_GETFONT => with(|g| g.windows.get(&h).map_or(0, |w| w.font)) as isize,
        WM_SETTEXT => {
            let text = sb_text_arg(lp, !current_ansi());
            with(|g| {
                if let Some(st) = sb_mut(g, h) {
                    if st.texts.is_empty() {
                        st.texts.push(String::new());
                    }
                    st.texts[0] = text;
                }
                g.invalidate(h, None, true);
            });
            1
        }
        WM_GETTEXT | WM_GETTEXTLENGTH => {
            let text = with(|g| sb_ref(g, h).and_then(|s| s.texts.first().cloned())).unwrap_or_default();
            if msg == WM_GETTEXTLENGTH {
                return text.chars().count() as isize;
            }
            if lp == 0 || wp == 0 {
                return 0;
            }
            if current_ansi() {
                crate::runtime::strings::return_a(lp as *mut u8, wp as u32, &text) as isize
            } else {
                crate::runtime::strings::return_w(lp as *mut u16, wp as u32, &text) as isize
            }
        }
        // SB_SETPARTS
        0x404 => {
            let n = wp.clamp(1, 256);
            let edges: Vec<i32> = (0..n).map(|i| *(lp as *const i32).add(i)).collect();
            with(|g| {
                if let Some(st) = sb_mut(g, h) {
                    st.edges = edges;
                    st.texts.resize(n, String::new());
                }
                g.invalidate(h, None, true);
            });
            1
        }
        // SB_GETPARTS
        0x406 => with(|g| {
            let Some(st) = sb_ref(g, h) else { return 0 };
            if lp != 0 {
                for (i, e) in st.edges.iter().take(wp).enumerate() {
                    *(lp as *mut i32).add(i) = *e;
                }
            }
            st.edges.len() as isize
        }),
        // SB_SETTEXTA / SB_SETTEXTW
        0x401 | 0x40B => {
            let (part, text) = ((wp & 0xff) as usize, sb_text_arg(lp, msg == 0x40B));
            if wp & 0xff == 0xff {
                with(|g| {
                    if let Some(st) = sb_mut(g, h) {
                        st.simple_text = text;
                    }
                    g.invalidate(h, None, true);
                });
                return 1;
            }
            with(|g| {
                let Some(st) = sb_mut(g, h) else { return 0 };
                if part >= st.edges.len() {
                    return 0;
                }
                st.texts.resize(st.edges.len(), String::new());
                st.texts[part] = text;
                g.invalidate(h, None, true);
                1
            })
        }
        // SB_GETTEXTA / SB_GETTEXTW
        0x402 | 0x40D => {
            let text = with(|g| sb_ref(g, h).and_then(|s| s.texts.get(wp & 0xff).cloned())).unwrap_or_default();
            if lp != 0 {
                if msg == 0x40D {
                    crate::runtime::strings::return_w(lp as *mut u16, u32::MAX >> 4, &text);
                } else {
                    crate::runtime::strings::return_a(lp as *mut u8, u32::MAX >> 4, &text);
                }
            }
            text.chars().count() as isize
        }
        // SB_GETTEXTLENGTHA / W
        0x403 | 0x40C => with(|g| sb_ref(g, h).and_then(|s| s.texts.get(wp & 0xff)).map_or(0, |t| t.chars().count() as isize)),
        // SB_SETMINHEIGHT
        0x408 => {
            with(|g| {
                if let Some(st) = sb_mut(g, h) {
                    st.min_height = wp as i32;
                }
            });
            sb_auto_size(h);
            0
        }
        // SB_SIMPLE
        0x409 => {
            with(|g| {
                if let Some(st) = sb_mut(g, h) {
                    st.simple = wp != 0;
                }
                g.invalidate(h, None, true);
            });
            1
        }
        0x40E => with(|g| sb_ref(g, h).is_some_and(|s| s.simple)) as isize,
        // SB_GETRECT
        0x40A => with(|g| {
            let Some(w) = g.windows.get(&h) else { return 0 };
            let (cw, ch) = w.client_size();
            let Some(st) = sb_ref(g, h) else { return 0 };
            if wp >= st.edges.len() || lp == 0 {
                return 0;
            }
            let r = part_rect(st, cw, ch, wp);
            let out = lp as *mut i32;
            *out = r.l;
            *out.add(1) = r.t;
            *out.add(2) = r.r;
            *out.add(3) = r.b;
            1
        }),
        // SB_GETBORDERS: horizontal, vertical and between-parts widths
        0x407 => {
            if lp != 0 {
                *(lp as *mut i32) = 0;
                *(lp as *mut i32).add(1) = 2;
                *(lp as *mut i32).add(2) = 2;
            }
            1
        }
        0x2001 | 0x40F | 0x411 | 0x412 | 0x413 | 0x414 | 0x415 | 0x416 | 0x417 => 0,
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

// =======================================================================================================
// tooltips: the tools are remembered, nothing is shown
// =======================================================================================================

pub unsafe extern "win64" fn tooltips_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    const TTM_ADDTOOLA: u32 = 0x404;
    const TTM_DELTOOLA: u32 = 0x405;
    const TTM_GETTOOLCOUNT: u32 = 0x40D;
    const TTM_ADDTOOLW: u32 = 0x432;
    const TTM_DELTOOLW: u32 = 0x433;
    match msg {
        WM_NCCREATE => 1,
        WM_PAINT => {
            let mut ps = [0u8; 72];
            api::begin_paint(h, ps.as_mut_ptr());
            api::end_paint(h, ps.as_ptr());
            0
        }
        WM_ERASEBKGND => 1,
        TTM_ADDTOOLA | TTM_ADDTOOLW => with(|g| {
            if let Some(w) = g.windows.get_mut(&h) {
                *w.props.entry("tools".into()).or_insert(0) += 1;
            }
            1
        }),
        TTM_DELTOOLA | TTM_DELTOOLW => with(|g| {
            if let Some(w) = g.windows.get_mut(&h) {
                if let Some(n) = w.props.get_mut("tools") {
                    *n = n.saturating_sub(1);
                }
            }
            0
        }),
        TTM_GETTOOLCOUNT => with(|g| g.windows.get(&h).and_then(|w| w.props.get("tools").copied()).unwrap_or(0) as isize),
        // everything else a program may send (activate, delay times, text updates, relay) is accepted and ignored
        0x401..=0x4FF | 0x2001..=0x2008 => 0,
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spin_numbers_get_thousands_separators_unless_told_not_to() {
        assert_eq!(format_pos(1234567, 10, false), "1,234,567");
        assert_eq!(format_pos(-1234, 10, false), "-1,234");
        assert_eq!(format_pos(1234, 10, true), "1234");
        assert_eq!(format_pos(255, 16, false), "FF");
        assert_eq!(format_pos(7, 10, false), "7");
    }

    #[test]
    fn trackbar_thumb_and_positions_are_inverses() {
        let tb = Trackbar { min: 0, max: 100, ..Default::default() };
        let (_, t0) = track_geometry(&tb, 200, 30, false, 0);
        let (_, t100) = track_geometry(&tb, 200, 30, false, 100);
        assert!(t0.l < t100.l);
        for pos in [0, 10, 37, 50, 99, 100] {
            let (_, t) = track_geometry(&tb, 200, 30, false, pos);
            let back = track_pos_at(&tb, 200, 30, false, (t.l + t.r) / 2);
            assert!((back - pos).abs() <= 1, "{pos} -> {back}");
        }
        let (_, v) = track_geometry(&tb, 30, 200, true, 50);
        assert!(v.height() == THUMB_W && v.width() > v.height());
    }

    #[test]
    fn status_bar_parts_cover_the_width() {
        let st = StatusBar { edges: vec![100, 200, -1], texts: vec![], ..Default::default() };
        assert_eq!(part_rect(&st, 400, 22, 0), Rect::new(0, 0, 100, 22));
        assert_eq!(part_rect(&st, 400, 22, 1), Rect::new(100, 0, 200, 22));
        assert_eq!(part_rect(&st, 400, 22, 2), Rect::new(200, 0, 400, 22));
    }

    #[test]
    fn arrows_of_a_spin_control() {
        assert_eq!(ud_arrow_at(16, 22, false, 8, 3), 1);
        assert_eq!(ud_arrow_at(16, 22, false, 8, 18), 2);
        assert_eq!(ud_arrow_at(16, 22, false, 20, 3), 0);
        assert_eq!(ud_arrow_at(30, 16, true, 25, 8), 1);
        assert_eq!(ud_arrow_at(30, 16, true, 5, 8), 2);
    }
}
