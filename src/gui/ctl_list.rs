//! The SCROLLBAR and LISTBOX window classes.
//!
//! A SCROLLBAR control reports to its parent (`WM_HSCROLL` / `WM_VSCROLL` with the control as
//! `lParam`) and leaves the position to the parent, as on Windows. A list box keeps its items,
//! selection and scrolling itself, uses the window's own scroll bars when it has `WS_VSCROLL` /
//! `WS_HSCROLL`, and notifies its parent (or, inside a combo box, the combo box) of selections.

use super::api;
use super::controls::{client_rect, dotted_focus, font_of, notify_parent, Ctl};
use super::gdi::sys_color;
use super::msg::{current_ansi, def_window_proc, kill_timer, set_timer};
use super::scroll::{self, Part, SB_CTL, SB_HORZ, SB_VERT};
use super::surface::*;
use super::*;

// ---- messages and styles --------------------------------------------------------------------------

pub const LB_ADDSTRING: u32 = 0x180;
pub const LB_INSERTSTRING: u32 = 0x181;
pub const LB_DELETESTRING: u32 = 0x182;
pub const LB_SELITEMRANGEEX: u32 = 0x183;
pub const LB_RESETCONTENT: u32 = 0x184;
pub const LB_SETSEL: u32 = 0x185;
pub const LB_SETCURSEL: u32 = 0x186;
pub const LB_GETSEL: u32 = 0x187;
pub const LB_GETCURSEL: u32 = 0x188;
pub const LB_GETTEXT: u32 = 0x189;
pub const LB_GETTEXTLEN: u32 = 0x18A;
pub const LB_GETCOUNT: u32 = 0x18B;
pub const LB_SELECTSTRING: u32 = 0x18C;
pub const LB_GETTOPINDEX: u32 = 0x18E;
pub const LB_FINDSTRING: u32 = 0x18F;
pub const LB_GETSELCOUNT: u32 = 0x190;
pub const LB_GETSELITEMS: u32 = 0x191;
pub const LB_SETTABSTOPS: u32 = 0x192;
pub const LB_GETHORIZONTALEXTENT: u32 = 0x193;
pub const LB_SETHORIZONTALEXTENT: u32 = 0x194;
pub const LB_SETCOLUMNWIDTH: u32 = 0x195;
pub const LB_SETTOPINDEX: u32 = 0x197;
pub const LB_GETITEMRECT: u32 = 0x198;
pub const LB_GETITEMDATA: u32 = 0x199;
pub const LB_SETITEMDATA: u32 = 0x19A;
pub const LB_SELITEMRANGE: u32 = 0x19B;
pub const LB_SETANCHORINDEX: u32 = 0x19C;
pub const LB_GETANCHORINDEX: u32 = 0x19D;
pub const LB_SETCARETINDEX: u32 = 0x19E;
pub const LB_GETCARETINDEX: u32 = 0x19F;
pub const LB_SETITEMHEIGHT: u32 = 0x1A0;
pub const LB_GETITEMHEIGHT: u32 = 0x1A1;
pub const LB_FINDSTRINGEXACT: u32 = 0x1A2;
pub const LB_SETLOCALE: u32 = 0x1A5;
pub const LB_GETLOCALE: u32 = 0x1A6;
pub const LB_SETCOUNT: u32 = 0x1A7;
pub const LB_INITSTORAGE: u32 = 0x1A8;
pub const LB_ITEMFROMPOINT: u32 = 0x1A9;
pub const LB_ERR: isize = -1;

pub const LBN_SELCHANGE: u32 = 1;
pub const LBN_DBLCLK: u32 = 2;
pub const LBN_SETFOCUS: u32 = 4;
pub const LBN_KILLFOCUS: u32 = 5;

const LBS_NOTIFY: u32 = 1;
const LBS_SORT: u32 = 2;
const LBS_MULTIPLESEL: u32 = 8;
const LBS_OWNERDRAWFIXED: u32 = 0x10;
const LBS_OWNERDRAWVARIABLE: u32 = 0x20;
const LBS_HASSTRINGS: u32 = 0x40;
const LBS_DISABLENOSCROLL: u32 = 0x1000;
const LBS_NOSEL: u32 = 0x4000;
const LBS_EXTENDEDSEL: u32 = 0x800;
pub const LBS_COMBOBOX: u32 = 0x8000;

/// Private messages a list box inside a combo box sends to its owner.
pub const LBC_COMMIT: u32 = WM_APP + 0x70;
pub const LBC_CANCEL: u32 = WM_APP + 0x71;

const SBM_SETPOS: u32 = 0xE0;
const SBM_GETPOS: u32 = 0xE1;
const SBM_SETRANGE: u32 = 0xE2;
const SBM_GETRANGE: u32 = 0xE3;
const SBM_ENABLE_ARROWS: u32 = 0xE4;
const SBM_SETRANGEREDRAW: u32 = 0xE6;
const SBM_SETSCROLLINFO: u32 = 0xE9;
const SBM_GETSCROLLINFO: u32 = 0xEA;
const SBS_VERT: u32 = 1;

/// Timer id the scroll bar control and list box use for auto-repeat.
const REPEAT_TIMER: usize = 0xCB;

// =======================================================================================================
// SCROLLBAR
// =======================================================================================================

fn is_vertical(g: &Gui, h: usize) -> bool {
    g.windows.get(&h).is_some_and(|w| w.style & SBS_VERT != 0)
}

unsafe fn scroll_notify(h: usize, code: u32, pos: i32) {
    let (parent, vertical) = with(|g| (g.windows.get(&h).map_or(0, |w| w.parent), is_vertical(g, h)));
    if parent != 0 {
        let msg = if vertical { WM_VSCROLL } else { WM_HSCROLL };
        api::send(parent, msg, code as usize | ((pos as u16 as usize) << 16), h as isize);
    }
}

fn bar_layout(g: &Gui, h: usize) -> Option<scroll::Layout> {
    let w = g.windows.get(&h)?;
    let (cw, ch) = w.client_size();
    Some(scroll::layout(&w.sb.ctl, Rect::new(0, 0, cw, ch), w.style & SBS_VERT != 0))
}

unsafe fn paint_scrollbar(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return };
        let (cw, ch) = w.client_size();
        let (bar, vertical) = (w.sb.ctl, w.style & SBS_VERT != 0);
        let (hot, pressed) = (w.sb.hot.map_or(Part::None, |p| p.1), w.sb.pressed.map_or(Part::None, |p| p.1));
        let enabled = g.is_enabled(h);
        g.draw(hdc, |cv, _| scroll::paint(cv, Rect::new(0, 0, cw, ch), vertical, &bar, enabled, hot, pressed));
    });
    api::end_paint(h, ps.as_ptr());
}

pub unsafe extern "win64" fn scrollbar_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::Scroll { grab: 0, dragging: false };
                }
            });
            1
        }
        WM_PAINT => {
            paint_scrollbar(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS => {
            with(|g| g.invalidate(h, None, false));
            0
        }
        WM_GETDLGCODE => 0x0001, // DLGC_WANTARROWS
        SBM_SETPOS => with(|g| {
            let prev = g.windows.get(&h).map_or(0, |w| w.sb.ctl.pos);
            g.scroll_set(h, SB_CTL, scroll::SIF_POS, 0, 0, 0, wp as i32);
            if lp != 0 {
                g.invalidate(h, None, false);
            }
            prev as isize
        }),
        SBM_GETPOS => with(|g| g.windows.get(&h).map_or(0, |w| w.sb.ctl.pos)) as isize,
        SBM_SETRANGE | SBM_SETRANGEREDRAW => with(|g| {
            g.scroll_set(h, SB_CTL, scroll::SIF_RANGE, wp as i32, lp as i32, 0, 0);
            g.invalidate(h, None, false);
            0
        }),
        SBM_GETRANGE => {
            if let Some(b) = with(|g| g.windows.get(&h).map(|w| w.sb.ctl)) {
                if wp != 0 {
                    *(wp as *mut i32) = b.min;
                }
                if lp != 0 {
                    *(lp as *mut i32) = b.max;
                }
            }
            0
        }
        SBM_ENABLE_ARROWS => {
            api::enable_scroll_bar(h, SB_CTL, wp as u32);
            1
        }
        SBM_SETSCROLLINFO => api::set_scroll_info(h, SB_CTL, lp as *const u8, wp != 0) as isize,
        SBM_GETSCROLLINFO => api::get_scroll_info(h, SB_CTL, lp as *mut u8) as isize,
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if !with(|g| g.is_enabled(h)) {
                return 0;
            }
            api::set_focus(h);
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            let press = with(|g| {
                let lay = bar_layout(g, h)?;
                let w = g.windows.get(&h)?;
                let vertical = w.style & SBS_VERT != 0;
                let part = lay.part_at(x, y);
                if part == Part::None || !w.sb.ctl.scrollable() {
                    return None;
                }
                let ok = match part {
                    Part::Arrow1 => w.sb.ctl.up_enabled,
                    Part::Arrow2 => w.sb.ctl.down_enabled,
                    _ => true,
                };
                if !ok {
                    return None;
                }
                let grab = if part == Part::Thumb { (if vertical { y - lay.thumb.t } else { x - lay.thumb.l }) as i32 } else { 0 };
                g.capture = h;
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::Scroll { grab, dragging: part == Part::Thumb };
                    w.sb.pressed = Some((SB_CTL, part));
                }
                g.invalidate(h, None, false);
                Some(part)
            });
            if let Some(part) = press {
                if let Some(code) = scroll::code_for(part) {
                    scroll_notify(h, code, 0);
                    set_timer(h, REPEAT_TIMER, 400, 0);
                }
            }
            0
        }
        WM_TIMER if wp == REPEAT_TIMER => {
            let (pressed, over) = with(|g| {
                let pressed = g.windows.get(&h).and_then(|w| w.sb.pressed).map(|p| p.1);
                let (sx, sy) = g.pointer;
                let (cx, cy) = g.screen_to_client(h, sx, sy);
                (pressed, bar_layout(g, h).map(|l| l.part_at(cx, cy)))
            });
            match pressed {
                Some(part) if Some(part) == over => {
                    set_timer(h, REPEAT_TIMER, 50, 0);
                    if let Some(code) = scroll::code_for(part) {
                        scroll_notify(h, code, 0);
                    }
                }
                _ => {}
            }
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            let drag = with(|g| {
                let (grab, dragging) = match g.windows.get(&h).map(|w| &w.ctl) {
                    Some(Ctl::Scroll { grab, dragging }) => (*grab, *dragging),
                    _ => (0, false),
                };
                let lay = bar_layout(g, h)?;
                if dragging && g.capture == h {
                    let w = g.windows.get_mut(&h)?;
                    let vertical = w.style & SBS_VERT != 0;
                    let along = if vertical { y } else { x };
                    let track_start = if vertical { lay.track.t } else { lay.track.l };
                    let pos = lay.pos_for_thumb(&w.sb.ctl, along - grab - track_start);
                    if std::mem::replace(&mut w.sb.ctl.track, pos) != pos {
                        g.invalidate(h, None, false);
                        return Some(pos);
                    }
                    return None;
                }
                let hot = match lay.part_at(x, y) {
                    Part::None => None,
                    p => Some((SB_CTL, p)),
                };
                if let Some(w) = g.windows.get_mut(&h) {
                    if std::mem::replace(&mut w.sb.hot, hot) != hot {
                        g.invalidate(h, None, false);
                    }
                }
                None
            });
            if let Some(pos) = drag {
                scroll_notify(h, scroll::SB_THUMBTRACK, pos);
            }
            0
        }
        WM_LBUTTONUP => {
            kill_timer(h, REPEAT_TIMER);
            let end = with(|g| {
                if g.capture == h {
                    g.capture = 0;
                }
                let dragging = matches!(g.windows.get(&h).map(|w| &w.ctl), Some(Ctl::Scroll { dragging: true, .. }));
                let pressed = g.windows.get(&h).is_some_and(|w| w.sb.pressed.is_some());
                let pos = g.windows.get(&h).map_or(0, |w| w.sb.ctl.track);
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::Scroll { grab: 0, dragging: false };
                    w.sb.pressed = None;
                    w.sb.ctl.track = w.sb.ctl.pos;
                }
                g.invalidate(h, None, false);
                (pressed, dragging, pos)
            });
            if end.1 {
                scroll_notify(h, scroll::SB_THUMBPOSITION, end.2);
            }
            if end.0 {
                scroll_notify(h, scroll::SB_ENDSCROLL, 0);
            }
            0
        }
        WM_KEYDOWN => {
            let code = match wp as u32 {
                0x25 | 0x26 => scroll::SB_LINEUP,
                0x27 | 0x28 => scroll::SB_LINEDOWN,
                0x21 => scroll::SB_PAGEUP,
                0x22 => scroll::SB_PAGEDOWN,
                0x24 => scroll::SB_TOP,
                0x23 => scroll::SB_BOTTOM,
                _ => return 0,
            };
            scroll_notify(h, code, 0);
            scroll_notify(h, scroll::SB_ENDSCROLL, 0);
            0
        }
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

// =======================================================================================================
// LISTBOX
// =======================================================================================================

#[derive(Clone, Default)]
pub struct Item {
    pub text: String,
    pub data: usize,
    pub sel: bool,
    pub height: i32,
}

#[derive(Clone)]
pub struct ListState {
    pub items: Vec<Item>,
    /// The selected item of a single-selection list; the caret item otherwise (-1 for none).
    pub cur: i32,
    pub top: usize,
    pub anchor: i32,
    pub item_h: i32,
    pub hextent: i32,
    pub xoff: i32,
    /// Whether the style asked for scroll bars, so they may be shown and hidden as the content needs.
    pub vbar: bool,
    pub hbar: bool,
    pub typed: String,
    pub typed_at: u32,
    pub dragging: bool,
}

impl Default for ListState {
    fn default() -> ListState {
        ListState { items: Vec::new(), cur: -1, top: 0, anchor: -1, item_h: 0, hextent: 0, xoff: 0, vbar: false, hbar: false, typed: String::new(), typed_at: 0, dragging: false }
    }
}

fn list_mut(g: &mut Gui, h: usize) -> Option<&mut ListState> {
    match &mut g.windows.get_mut(&h)?.ctl {
        Ctl::List(l) => Some(l),
        _ => None,
    }
}

fn list_ref(g: &Gui, h: usize) -> Option<&ListState> {
    match &g.windows.get(&h)?.ctl {
        Ctl::List(l) => Some(l),
        _ => None,
    }
}

fn style_of(g: &Gui, h: usize) -> u32 {
    g.windows.get(&h).map_or(0, |w| w.style)
}

fn multi(style: u32) -> bool {
    style & (LBS_MULTIPLESEL | LBS_EXTENDEDSEL) != 0
}

fn owner_draw(style: u32) -> bool {
    style & (LBS_OWNERDRAWFIXED | LBS_OWNERDRAWVARIABLE) != 0
}

/// Who gets `WM_DRAWITEM` / `WM_MEASUREITEM` for a list: its parent, or the combo box's parent when the list belongs to
/// a combo box. Returns (window, control id, ODT_* type, control handle).
fn owner_info(g: &Gui, h: usize) -> (usize, usize, u32, usize) {
    let Some(w) = g.windows.get(&h) else { return (0, 0, 2, h) };
    if w.style & LBS_COMBOBOX != 0 {
        if let Some(c) = g.windows.get(&w.owner) {
            return (c.parent, c.id, 3, w.owner);
        }
    }
    (w.parent, w.id, 2, h)
}

unsafe fn str_arg(lp: isize) -> String {
    if lp == 0 {
        String::new()
    } else if current_ansi() {
        crate::runtime::strings::a_to_string(lp as *const u8)
    } else {
        crate::runtime::strings::w_ptr_to_string(lp as *const u16)
    }
}

/// Height of one row: a fixed height the program set, or the font's.
fn row_height(g: &Gui, h: usize) -> i32 {
    let st = match list_ref(g, h) {
        Some(s) => s,
        None => return 16,
    };
    if st.item_h > 0 {
        st.item_h
    } else {
        g.sized_font(font_of(g, h)).height().max(8)
    }
}

fn item_height_at(g: &Gui, h: usize, i: usize) -> i32 {
    let style = style_of(g, h);
    match list_ref(g, h).and_then(|s| s.items.get(i)) {
        Some(it) if style & LBS_OWNERDRAWVARIABLE != 0 && it.height > 0 => it.height,
        _ => row_height(g, h),
    }
}

/// How many whole rows fit in the client area.
fn page_rows(g: &Gui, h: usize) -> usize {
    let ch = client_rect(g, h).height();
    (ch / row_height(g, h).max(1)).max(1) as usize
}

/// The index at client y, and whether that is a real item.
fn index_at(g: &Gui, h: usize, y: i32) -> Option<usize> {
    let st = list_ref(g, h)?;
    let mut top_y = 0;
    let mut i = st.top;
    while i < st.items.len() {
        let ih = item_height_at(g, h, i);
        if y >= top_y && y < top_y + ih {
            return Some(i);
        }
        top_y += ih;
        i += 1;
    }
    None
}

fn item_top(g: &Gui, h: usize, idx: usize) -> i32 {
    let top = list_ref(g, h).map_or(0, |s| s.top);
    if idx >= top {
        (top..idx).map(|i| item_height_at(g, h, i)).sum()
    } else {
        -((idx..top).map(|i| item_height_at(g, h, i)).sum::<i32>())
    }
}

/// Keeps the window's scroll bars in step with the content.
fn sync_bars(g: &mut Gui, h: usize) {
    let Some(st) = list_ref(g, h) else { return };
    let (vbar, hbar, count, top, hext, xoff) = (st.vbar, st.hbar, st.items.len() as i32, st.top as i32, st.hextent, st.xoff);
    let style = style_of(g, h);
    let flag = if style & LBS_DISABLENOSCROLL != 0 { scroll::SIF_DISABLENOSCROLL } else { 0 };
    for _ in 0..2 {
        if vbar {
            let rows = page_rows(g, h) as u32;
            g.scroll_set(h, SB_VERT, scroll::SIF_RANGE | scroll::SIF_PAGE | scroll::SIF_POS | flag, 0, (count - 1).max(0), rows, top);
        }
        if hbar {
            let cw = client_rect(g, h).width() as u32;
            g.scroll_set(h, SB_HORZ, scroll::SIF_RANGE | scroll::SIF_PAGE | scroll::SIF_POS | flag, 0, (hext - 1).max(0), cw, xoff);
        }
    }
    // the position the bar settled on is the truth
    let rows = page_rows(g, h) as i32;
    let max_top = (count - rows).max(0);
    if let Some(st) = list_mut(g, h) {
        st.top = st.top.min(max_top as usize);
    }
    g.paint_nc(h);
}

fn ensure_visible(g: &mut Gui, h: usize, idx: usize) {
    let rows = page_rows(g, h);
    if let Some(st) = list_mut(g, h) {
        if idx < st.top {
            st.top = idx;
        } else if idx >= st.top + rows {
            st.top = idx + 1 - rows;
        }
    }
    sync_bars(g, h);
}

/// Where a list box sends its notifications: its owner when it is the list of a combo box.
unsafe fn notify(h: usize, code: u32) {
    let (style, owner, id) = with(|g| g.windows.get(&h).map_or((0, 0, 0), |w| (w.style, w.owner, w.id)));
    if style & LBS_COMBOBOX != 0 {
        return;
    }
    let _ = (owner, id);
    if style & LBS_NOTIFY != 0 || code == LBN_SETFOCUS || code == LBN_KILLFOCUS {
        if style & LBS_NOTIFY != 0 {
            notify_parent(h, code);
        }
    }
}

fn clear_selection(g: &mut Gui, h: usize) {
    if let Some(st) = list_mut(g, h) {
        for it in &mut st.items {
            it.sel = false;
        }
    }
}

fn select_only(g: &mut Gui, h: usize, idx: i32) {
    let style = style_of(g, h);
    if let Some(st) = list_mut(g, h) {
        st.cur = idx;
        st.anchor = idx;
        if multi(style) {
            for it in &mut st.items {
                it.sel = false;
            }
        }
        if idx >= 0 {
            if let Some(it) = st.items.get_mut(idx as usize) {
                it.sel = true;
            }
        }
    }
}

fn select_range(g: &mut Gui, h: usize, a: i32, b: i32, on: bool) {
    if let Some(st) = list_mut(g, h) {
        let (lo, hi) = (a.min(b).max(0), a.max(b));
        for i in lo..=hi.min(st.items.len() as i32 - 1) {
            st.items[i as usize].sel = on;
        }
    }
}

fn sorted_position(st: &ListState, text: &str) -> usize {
    let key = text.to_lowercase();
    st.items.iter().position(|it| it.text.to_lowercase() > key).unwrap_or(st.items.len())
}

unsafe fn add_item(h: usize, at: Option<usize>, lp: isize) -> isize {
    let style = with(|g| style_of(g, h));
    let (text, data) = if style & LBS_HASSTRINGS != 0 || !owner_draw(style) { (str_arg(lp), 0) } else { (String::new(), lp as usize) };
    // owner-drawn lists ask their owner how tall the item is
    let mut height = 0;
    if style & LBS_OWNERDRAWVARIABLE != 0 {
        let ((parent, id, ctype, _), count) = with(|g| (owner_info(g, h), list_ref(g, h).map_or(0, |s| s.items.len())));
        height = measure_item(parent, h, id, ctype, at.unwrap_or(count), data);
    }
    let idx = with(|g| {
        let sorted = style & LBS_SORT != 0 && at.is_none();
        let st = list_mut(g, h)?;
        let idx = match at {
            Some(i) => i.min(st.items.len()),
            None if sorted => sorted_position(st, &text),
            None => st.items.len(),
        };
        st.items.insert(idx, Item { text, data, sel: false, height });
        if st.cur >= idx as i32 && st.cur >= 0 {
            st.cur += 1;
        }
        sync_bars(g, h);
        g.invalidate(h, None, true);
        Some(idx)
    });
    idx.map_or(LB_ERR, |i| i as isize)
}

/// `WM_MEASUREITEM` to the owner of an owner-drawn list; returns the item height (0 when the owner leaves it).
unsafe fn measure_item(owner: usize, h: usize, id: usize, ctype: u32, idx: usize, data: usize) -> i32 {
    if owner == 0 {
        return 0;
    }
    let mut m = [0u8; 32];
    *(m.as_mut_ptr() as *mut u32) = ctype;
    *(m.as_mut_ptr().add(4) as *mut u32) = id as u32;
    *(m.as_mut_ptr().add(8) as *mut u32) = idx as u32;
    *(m.as_mut_ptr().add(16) as *mut u32) = with(|g| row_height(g, h)) as u32;
    *(m.as_mut_ptr().add(24) as *mut u64) = data as u64;
    api::send(owner, WM_MEASUREITEM, id, m.as_ptr() as isize);
    *(m.as_ptr().add(16) as *const u32) as i32
}

fn find_index(g: &Gui, h: usize, start: i32, text: &str, exact: bool) -> isize {
    let Some(st) = list_ref(g, h) else { return LB_ERR };
    let n = st.items.len();
    if n == 0 {
        return LB_ERR;
    }
    let want = text.to_lowercase();
    let first = (start + 1).rem_euclid(n as i32) as usize;
    for k in 0..n {
        let i = (first + k) % n;
        let t = st.items[i].text.to_lowercase();
        if (exact && t == want) || (!exact && t.starts_with(&want)) {
            return i as isize;
        }
    }
    LB_ERR
}

unsafe fn write_text(lp: isize, text: &str) -> isize {
    if lp == 0 {
        return text.len() as isize;
    }
    if current_ansi() {
        let b = text.as_bytes();
        std::ptr::copy_nonoverlapping(b.as_ptr(), lp as *mut u8, b.len());
        *(lp as *mut u8).add(b.len()) = 0;
        b.len() as isize
    } else {
        let w: Vec<u16> = text.encode_utf16().collect();
        std::ptr::copy_nonoverlapping(w.as_ptr(), lp as *mut u16, w.len());
        *(lp as *mut u16).add(w.len()) = 0;
        w.len() as isize
    }
}

fn text_len(text: &str) -> isize {
    if current_ansi() {
        text.len() as isize
    } else {
        text.encode_utf16().count() as isize
    }
}

unsafe fn paint_list(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    let parent = with(|g| g.windows.get(&h).map_or(0, |w| w.parent));
    let (draw_parent, draw_id, draw_type, item_hwnd) = with(|g| owner_info(g, h));
    let brush = if parent != 0 { api::send(parent, WM_CTLCOLORLISTBOX, hdc, h as isize) as usize } else { 0 };
    // pass 1 (locked): background and the rows the list draws itself; owner-drawn rows are collected
    let mut owner_rows: Vec<(usize, Rect, u32, usize)> = Vec::new();
    let (_, style) = with(|g| {
        let Some(st) = list_ref(g, h).cloned() else { return (0, 0) };
        let w = g.windows.get(&h).unwrap();
        let (id, style) = (w.id, w.style);
        let (cw, ch) = w.client_size();
        let enabled = g.is_enabled(h);
        let focused = g.focus == h;
        let f = font_of(g, h);
        g.select_object(hdc, f);
        let bg = if brush != 0 { g.brush_arg(brush) } else { Brush::Solid(if enabled { 0xFFFFFF } else { sys_color(15) }) };
        g.fill_rect(hdc, Rect::new(0, 0, cw, ch), &bg);
        let Some(view) = g.view(hdc) else { return (id, style) };
        let mut y = 0;
        let mut i = st.top;
        while i < st.items.len() && y < ch {
            let it = &st.items[i];
            let ih = item_height_at(g, h, i);
            let row = Rect::new(0, y, cw, y + ih);
            let selected = it.sel || (!multi(style) && st.cur == i as i32);
            let state = (selected as u32) | ((!enabled as u32) << 2) | (((focused && st.cur == i as i32) as u32) << 4);
            if owner_draw(style) {
                owner_rows.push((i, row, state, it.data));
            } else {
                let (fg, back) = if !enabled {
                    (0x838383, None)
                } else if selected && style & LBS_NOSEL == 0 {
                    (0xFFFFFF, Some(sys_color(13)))
                } else {
                    (view.text_color, None)
                };
                let text = it.text.replace('\t', "    ");
                let ty = y + (ih - view.font.height()) / 2;
                let xo = st.xoff;
                g.draw(hdc, |cv, v| {
                    if let Some(b) = back {
                        cv.fill_rect(row, b);
                    }
                    let saved = cv.clip;
                    cv.clip = cv.clip.intersect(&row.offset(cv.ox, cv.oy));
                    v.font.draw(cv, 2 - xo, ty, &text, fg, None);
                    if focused && st.cur == i as i32 && style & LBS_NOSEL == 0 {
                        dotted_focus(cv, Rect::new(row.l, row.t, row.r, row.b));
                    }
                    cv.clip = saved;
                });
            }
            y += ih;
            i += 1;
        }
        (id, style)
    });
    // pass 2 (unlocked): the owner draws its rows
    if owner_draw(style) && draw_parent != 0 {
        for (i, row, state, data) in owner_rows {
            let mut di = [0u8; 64];
            *(di.as_mut_ptr() as *mut u32) = draw_type;
            *(di.as_mut_ptr().add(4) as *mut u32) = draw_id as u32;
            *(di.as_mut_ptr().add(8) as *mut u32) = i as u32;
            *(di.as_mut_ptr().add(12) as *mut u32) = 1; // ODA_DRAWENTIRE
            *(di.as_mut_ptr().add(16) as *mut u32) = state;
            *(di.as_mut_ptr().add(24) as *mut u64) = item_hwnd as u64;
            *(di.as_mut_ptr().add(32) as *mut u64) = hdc as u64;
            let r = di.as_mut_ptr().add(40) as *mut i32;
            *r = row.l;
            *r.add(1) = row.t;
            *r.add(2) = row.r;
            *r.add(3) = row.b;
            *(di.as_mut_ptr().add(56) as *mut u64) = data as u64;
            api::send(draw_parent, WM_DRAWITEM, draw_id, di.as_ptr() as isize);
        }
    }
    api::end_paint(h, ps.as_ptr());
}

/// Moves the selection to `idx` as a key or mouse action does, and tells the parent.
unsafe fn user_select(h: usize, idx: i32, extend: bool, toggle: bool) {
    let changed = with(|g| {
        let style = style_of(g, h);
        let count = list_ref(g, h).map_or(0, |s| s.items.len() as i32);
        if style & LBS_NOSEL != 0 || count == 0 {
            return false;
        }
        let idx = idx.clamp(0, count - 1);
        let (before_cur, anchor) = list_ref(g, h).map_or((-1, -1), |s| (s.cur, s.anchor));
        if style & LBS_MULTIPLESEL != 0 {
            if let Some(st) = list_mut(g, h) {
                st.items[idx as usize].sel = !st.items[idx as usize].sel;
                st.cur = idx;
                st.anchor = idx;
            }
        } else if style & LBS_EXTENDEDSEL != 0 {
            if toggle {
                if let Some(st) = list_mut(g, h) {
                    st.items[idx as usize].sel = !st.items[idx as usize].sel;
                    st.cur = idx;
                    st.anchor = idx;
                }
            } else if extend && anchor >= 0 {
                clear_selection(g, h);
                select_range(g, h, anchor, idx, true);
                if let Some(st) = list_mut(g, h) {
                    st.cur = idx;
                }
            } else {
                select_only(g, h, idx);
            }
        } else {
            select_only(g, h, idx);
        }
        ensure_visible(g, h, idx as usize);
        g.invalidate(h, None, true);
        before_cur != idx || multi(style)
    });
    if changed {
        notify(h, LBN_SELCHANGE);
    }
}

unsafe fn list_scroll_to(h: usize, top: i64) {
    with(|g| {
        let count = list_ref(g, h).map_or(0, |s| s.items.len() as i64);
        let rows = page_rows(g, h) as i64;
        let top = top.clamp(0, (count - rows).max(0)) as usize;
        if let Some(st) = list_mut(g, h) {
            st.top = top;
        }
        sync_bars(g, h);
        g.invalidate(h, None, true);
    });
}

pub unsafe extern "win64" fn listbox_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    let st = ListState { vbar: w.style & WS_VSCROLL != 0, hbar: w.style & WS_HSCROLL != 0, ..Default::default() };
                    w.ctl = Ctl::List(Box::new(st));
                }
            });
            1
        }
        WM_CREATE | WM_SIZE => {
            with(|g| {
                sync_bars(g, h);
                g.invalidate(h, None, true);
            });
            0
        }
        WM_PAINT => {
            paint_list(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETFONT => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.font = wp;
                }
                sync_bars(g, h);
                g.invalidate(h, None, true);
            });
            0
        }
        WM_GETFONT => with(|g| g.windows.get(&h).map_or(0, |w| w.font)) as isize,
        WM_ENABLE => {
            with(|g| g.invalidate(h, None, true));
            0
        }
        WM_SETFOCUS => {
            with(|g| g.invalidate(h, None, true));
            notify(h, LBN_SETFOCUS);
            0
        }
        WM_KILLFOCUS => {
            with(|g| g.invalidate(h, None, true));
            notify(h, LBN_KILLFOCUS);
            0
        }
        WM_GETDLGCODE => 0x0083, // DLGC_WANTARROWS | DLGC_WANTCHARS
        WM_VSCROLL => {
            let (code, pos) = (loword(wp) as u32, hiword(wp));
            let (top, rows, count) = with(|g| (list_ref(g, h).map_or(0, |s| s.top as i64), page_rows(g, h) as i64, list_ref(g, h).map_or(0, |s| s.items.len() as i64)));
            let new = match code {
                scroll::SB_LINEUP => top - 1,
                scroll::SB_LINEDOWN => top + 1,
                scroll::SB_PAGEUP => top - rows,
                scroll::SB_PAGEDOWN => top + rows,
                scroll::SB_THUMBTRACK | scroll::SB_THUMBPOSITION => {
                    // the 16-bit position in the message is too small for long lists; the bar knows better
                    let tracked = with(|g| g.windows.get(&h).map_or(pos, |w| w.sb.vert.track));
                    tracked as i64
                }
                scroll::SB_TOP => 0,
                scroll::SB_BOTTOM => count,
                _ => return 0,
            };
            list_scroll_to(h, new);
            0
        }
        WM_HSCROLL => {
            let code = loword(wp) as u32;
            let (x, cw, ext) = with(|g| (list_ref(g, h).map_or(0, |s| s.xoff), client_rect(g, h).width(), list_ref(g, h).map_or(0, |s| s.hextent)));
            let new = match code {
                scroll::SB_LINEUP => x - 8,
                scroll::SB_LINEDOWN => x + 8,
                scroll::SB_PAGEUP => x - cw,
                scroll::SB_PAGEDOWN => x + cw,
                scroll::SB_THUMBTRACK | scroll::SB_THUMBPOSITION => with(|g| g.windows.get(&h).map_or(0, |w| w.sb.horz.track)),
                scroll::SB_TOP => 0,
                scroll::SB_BOTTOM => ext,
                _ => return 0,
            };
            with(|g| {
                if let Some(st) = list_mut(g, h) {
                    st.xoff = new.clamp(0, (ext - cw).max(0));
                }
                sync_bars(g, h);
                g.invalidate(h, None, true);
            });
            0
        }
        WM_MOUSEWHEEL => {
            let delta = (wp >> 16) as u16 as i16 as i64;
            let top = with(|g| list_ref(g, h).map_or(0, |s| s.top as i64));
            list_scroll_to(h, top - delta / 120 * 3);
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if !with(|g| g.is_enabled(h)) {
                return 0;
            }
            let style = with(|g| style_of(g, h));
            if style & LBS_COMBOBOX != 0 {
                // inside a combo box: a click outside the items closes it
                let (cw, ch) = with(|g| client_rect(g, h).wh());
                let (x, y) = (lo_signed(lp), hi_signed(lp));
                if x < 0 || y < 0 || x >= cw || y >= ch {
                    let owner = with(|g| g.windows.get(&h).map_or(0, |w| w.owner));
                    api::send(owner, LBC_CANCEL, h, 0);
                    return 0;
                }
            } else {
                api::set_focus(h);
            }
            let y = hi_signed(lp);
            let idx = with(|g| index_at(g, h, y));
            if let Some(i) = idx {
                let ctrl = with(|g| g.keys[0x11] & 0x80 != 0);
                let shift = with(|g| g.keys[0x10] & 0x80 != 0);
                user_select(h, i as i32, shift, ctrl);
                with(|g| {
                    g.capture = h;
                    if let Some(st) = list_mut(g, h) {
                        st.dragging = true;
                    }
                });
                if msg == WM_LBUTTONDBLCLK {
                    notify(h, LBN_DBLCLK);
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            let y = hi_signed(lp);
            let (style, dragging, ch, left_down) = with(|g| (style_of(g, h), list_ref(g, h).is_some_and(|s| s.dragging), client_rect(g, h).height(), g.buttons & 1 != 0));
            if style & LBS_COMBOBOX != 0 {
                // the item under the pointer is the selected one
                if let Some(i) = with(|g| index_at(g, h, y)) {
                    with(|g| {
                        if list_ref(g, h).is_some_and(|s| s.cur != i as i32) {
                            select_only(g, h, i as i32);
                            g.invalidate(h, None, true);
                        }
                    });
                }
                return 0;
            }
            if dragging && left_down {
                let idx = if y < 0 {
                    with(|g| list_ref(g, h).map_or(0, |s| s.top as i32 - 1))
                } else if y >= ch {
                    with(|g| list_ref(g, h).map_or(0, |s| (s.top + page_rows(g, h)) as i32))
                } else {
                    with(|g| index_at(g, h, y).map_or(-1, |i| i as i32))
                };
                if idx >= 0 && with(|g| list_ref(g, h).is_some_and(|s| s.cur != idx.min(s.items.len() as i32 - 1))) {
                    user_select(h, idx, style & LBS_EXTENDEDSEL != 0, false);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let (style, was) = with(|g| {
                let was = list_ref(g, h).is_some_and(|s| s.dragging);
                if let Some(st) = list_mut(g, h) {
                    st.dragging = false;
                }
                if g.capture == h {
                    g.capture = 0;
                }
                (style_of(g, h), was)
            });
            if style & LBS_COMBOBOX != 0 {
                let (cw, ch) = with(|g| client_rect(g, h).wh());
                let (x, y) = (lo_signed(lp), hi_signed(lp));
                let owner = with(|g| g.windows.get(&h).map_or(0, |w| w.owner));
                // releasing over an item chooses it (also after pressing on the combo box's button and dragging
                // down); a release elsewhere is the end of the click that opened the list, not a cancel
                if x >= 0 && y >= 0 && x < cw && y < ch {
                    if let Some(i) = with(|g| index_at(g, h, y)) {
                        api::send(owner, LBC_COMMIT, h, i as isize);
                    }
                }
            } else if was && style & LBS_NOSEL == 0 {
                // the selection was announced on press; nothing more to say on release
            }
            0
        }
        WM_KEYDOWN => {
            let style = with(|g| style_of(g, h));
            let (cur, count, rows) = with(|g| (list_ref(g, h).map_or(-1, |s| s.cur), list_ref(g, h).map_or(0, |s| s.items.len() as i32), page_rows(g, h) as i32));
            let shift = with(|g| g.keys[0x10] & 0x80 != 0);
            let ctrl = with(|g| g.keys[0x11] & 0x80 != 0);
            let target = match wp as u32 {
                0x26 | 0x25 => (cur - 1).max(0),
                0x28 | 0x27 => (cur + 1).min(count - 1).max(0),
                0x21 => (cur - rows + 1).max(0),
                0x22 => (cur + rows - 1).min(count - 1).max(0),
                0x24 => 0,
                0x23 => count - 1,
                0x20 if multi(style) => {
                    if cur >= 0 {
                        user_select(h, cur, false, style & LBS_EXTENDEDSEL != 0);
                    }
                    return 0;
                }
                _ => return 0,
            };
            if count > 0 && !(ctrl && style & LBS_EXTENDEDSEL != 0 && false) {
                user_select(h, target, shift, false);
            }
            0
        }
        WM_CHAR => {
            // type-ahead: characters typed in quick succession form a prefix
            let c = char::from_u32(wp as u32).unwrap_or('\0');
            if c.is_control() {
                return 0;
            }
            let now = with(|g| g.tick());
            let prefix = with(|g| {
                let st = list_mut(g, h)?;
                if now.wrapping_sub(st.typed_at) > 1000 {
                    st.typed.clear();
                }
                st.typed.push(c);
                st.typed_at = now;
                Some(st.typed.clone())
            });
            if let Some(p) = prefix {
                let start = with(|g| list_ref(g, h).map_or(-1, |s| if p.chars().count() == 1 { s.cur } else { s.cur - 1 }));
                let found = with(|g| find_index(g, h, start, &p, false));
                if found >= 0 {
                    user_select(h, found as i32, false, false);
                }
            }
            0
        }
        // ---- LB_* ----------------------------------------------------------------------------------
        LB_ADDSTRING => add_item(h, None, lp),
        LB_INSERTSTRING => {
            let at = if wp as i32 == -1 { None } else { Some(wp) };
            add_item(h, at, lp)
        }
        LB_DELETESTRING => with(|g| {
            let Some(st) = list_mut(g, h) else { return LB_ERR };
            if wp >= st.items.len() {
                return LB_ERR;
            }
            st.items.remove(wp);
            if st.cur == wp as i32 {
                st.cur = -1;
            } else if st.cur > wp as i32 {
                st.cur -= 1;
            }
            let n = st.items.len();
            sync_bars(g, h);
            g.invalidate(h, None, true);
            n as isize
        }),
        LB_RESETCONTENT => with(|g| {
            if let Some(st) = list_mut(g, h) {
                st.items.clear();
                st.cur = -1;
                st.top = 0;
                st.anchor = -1;
            }
            sync_bars(g, h);
            g.invalidate(h, None, true);
            0
        }),
        LB_GETCOUNT => with(|g| list_ref(g, h).map_or(LB_ERR, |s| s.items.len() as isize)),
        LB_GETCURSEL => with(|g| {
            let style = style_of(g, h);
            if multi(style) {
                return LB_ERR;
            }
            list_ref(g, h).map_or(LB_ERR, |s| if s.cur >= 0 { s.cur as isize } else { LB_ERR })
        }),
        LB_SETCURSEL => {
            let r = with(|g| {
                let style = style_of(g, h);
                if multi(style) || style & LBS_NOSEL != 0 {
                    return LB_ERR;
                }
                let n = list_ref(g, h).map_or(0, |s| s.items.len());
                let idx = wp as i32;
                if idx < 0 || idx as usize >= n {
                    select_only(g, h, -1);
                    clear_selection(g, h);
                    g.invalidate(h, None, true);
                    return if idx == -1 { 0 } else { LB_ERR };
                }
                clear_selection(g, h);
                select_only(g, h, idx);
                ensure_visible(g, h, idx as usize);
                g.invalidate(h, None, true);
                idx as isize
            });
            r
        }
        LB_GETTEXT => with(|g| match list_ref(g, h).and_then(|s| s.items.get(wp)) {
            Some(it) if !owner_draw(style_of(g, h)) || style_of(g, h) & LBS_HASSTRINGS != 0 => (it.text.clone(), true),
            Some(it) => (String::new(), {
                let _ = it;
                false
            }),
            None => (String::new(), false),
        })
        .pipe(|(text, ok)| {
            if ok {
                write_text(lp, &text)
            } else {
                LB_ERR
            }
        }),
        LB_GETTEXTLEN => with(|g| list_ref(g, h).and_then(|s| s.items.get(wp)).map_or(LB_ERR, |it| text_len(&it.text))),
        LB_GETITEMDATA => with(|g| list_ref(g, h).and_then(|s| s.items.get(wp)).map_or(LB_ERR, |it| it.data as isize)),
        LB_SETITEMDATA => with(|g| match list_mut(g, h).and_then(|s| s.items.get_mut(wp)) {
            Some(it) => {
                it.data = lp as usize;
                0
            }
            None => LB_ERR,
        }),
        LB_GETSEL => with(|g| {
            let style = style_of(g, h);
            let Some(st) = list_ref(g, h) else { return LB_ERR };
            match st.items.get(wp) {
                Some(it) => (if multi(style) { it.sel } else { st.cur == wp as i32 }) as isize,
                None => LB_ERR,
            }
        }),
        LB_SETSEL => with(|g| {
            if !multi(style_of(g, h)) {
                return LB_ERR;
            }
            let on = wp != 0;
            if lp as i32 == -1 {
                if let Some(st) = list_mut(g, h) {
                    for it in &mut st.items {
                        it.sel = on;
                    }
                }
            } else if let Some(it) = list_mut(g, h).and_then(|s| s.items.get_mut(lp as usize)) {
                it.sel = on;
            } else {
                return LB_ERR;
            }
            g.invalidate(h, None, true);
            0
        }),
        LB_GETSELCOUNT => with(|g| {
            if !multi(style_of(g, h)) {
                return LB_ERR;
            }
            list_ref(g, h).map_or(LB_ERR, |s| s.items.iter().filter(|i| i.sel).count() as isize)
        }),
        LB_GETSELITEMS => with(|g| {
            if !multi(style_of(g, h)) || lp == 0 {
                return LB_ERR;
            }
            let sel: Vec<i32> = list_ref(g, h).map_or(Vec::new(), |s| s.items.iter().enumerate().filter(|(_, i)| i.sel).map(|(k, _)| k as i32).collect());
            let n = sel.len().min(wp);
            for (k, v) in sel.iter().take(n).enumerate() {
                *(lp as *mut i32).add(k) = *v;
            }
            n as isize
        }),
        LB_SELITEMRANGE => with(|g| {
            if !multi(style_of(g, h)) {
                return LB_ERR;
            }
            select_range(g, h, loword(lp as usize), hiword(lp as usize), wp != 0);
            g.invalidate(h, None, true);
            0
        }),
        LB_SELITEMRANGEEX => with(|g| {
            if !multi(style_of(g, h)) {
                return LB_ERR;
            }
            let (a, b) = (wp as i32, lp as i32);
            select_range(g, h, a, b, a <= b);
            g.invalidate(h, None, true);
            0
        }),
        LB_GETTOPINDEX => with(|g| list_ref(g, h).map_or(LB_ERR, |s| s.top as isize)),
        LB_SETTOPINDEX => {
            if with(|g| list_ref(g, h).is_none_or(|s| wp >= s.items.len())) {
                return LB_ERR;
            }
            list_scroll_to(h, wp as i64);
            0
        }
        LB_GETCARETINDEX => with(|g| list_ref(g, h).map_or(0, |s| s.cur.max(0) as isize)),
        LB_SETCARETINDEX => with(|g| {
            if let Some(st) = list_mut(g, h) {
                st.cur = wp as i32;
            }
            ensure_visible(g, h, wp);
            g.invalidate(h, None, true);
            0
        }),
        LB_GETANCHORINDEX => with(|g| list_ref(g, h).map_or(0, |s| s.anchor as isize)),
        LB_SETANCHORINDEX => with(|g| {
            if let Some(st) = list_mut(g, h) {
                st.anchor = wp as i32;
            }
            0
        }),
        LB_FINDSTRING => with(|g| find_index(g, h, wp as i32, &str_arg(lp), false)),
        LB_FINDSTRINGEXACT => with(|g| find_index(g, h, wp as i32, &str_arg(lp), true)),
        LB_SELECTSTRING => {
            let found = with(|g| find_index(g, h, wp as i32, &str_arg(lp), false));
            if found >= 0 {
                with(|g| {
                    clear_selection(g, h);
                    select_only(g, h, found as i32);
                    ensure_visible(g, h, found as usize);
                    g.invalidate(h, None, true);
                });
            }
            found
        }
        LB_GETITEMRECT => with(|g| {
            let (n, cw) = (list_ref(g, h).map_or(0, |s| s.items.len()), client_rect(g, h).width());
            if wp >= n || lp == 0 {
                return LB_ERR;
            }
            let (t, ih) = (item_top(g, h, wp), item_height_at(g, h, wp));
            let r = lp as *mut i32;
            *r = 0;
            *r.add(1) = t;
            *r.add(2) = cw;
            *r.add(3) = t + ih;
            0
        }),
        LB_ITEMFROMPOINT => with(|g| {
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            let (cw, ch) = client_rect(g, h).wh();
            let outside = x < 0 || y < 0 || x >= cw || y >= ch;
            let n = list_ref(g, h).map_or(0, |s| s.items.len());
            let idx = index_at(g, h, y).unwrap_or(n.saturating_sub(1));
            ((idx as isize) & 0xffff) | ((outside as isize) << 16)
        }),
        LB_SETITEMHEIGHT => with(|g| {
            let var = style_of(g, h) & LBS_OWNERDRAWVARIABLE != 0;
            let height = (lp as i32 & 0xffff).max(1);
            match list_mut(g, h) {
                Some(st) if var => match st.items.get_mut(wp) {
                    Some(it) => it.height = height,
                    None => return LB_ERR,
                },
                Some(st) => st.item_h = height,
                None => return LB_ERR,
            }
            sync_bars(g, h);
            g.invalidate(h, None, true);
            0
        }),
        LB_GETITEMHEIGHT => with(|g| item_height_at(g, h, wp) as isize),
        LB_SETHORIZONTALEXTENT => with(|g| {
            if let Some(st) = list_mut(g, h) {
                st.hextent = wp as i32;
            }
            sync_bars(g, h);
            g.invalidate(h, None, true);
            0
        }),
        LB_GETHORIZONTALEXTENT => with(|g| list_ref(g, h).map_or(0, |s| s.hextent as isize)),
        LB_SETTABSTOPS | LB_SETCOLUMNWIDTH | LB_SETCOUNT => 0,
        LB_INITSTORAGE => wp as isize,
        LB_SETLOCALE => 0x409,
        LB_GETLOCALE => 0x409,
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

/// Tiny helper so a `with(...)` result can be post-processed without a temporary.
trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}
