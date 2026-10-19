//! The list view (`SysListView32`): items with sub-items shown as a table with a header (report
//! view), or as icons, small icons or a list. Items, columns and selection live in the control;
//! virtual lists (`LVS_OWNERDATA`) ask the parent for text with `LVN_GETDISPINFO`.

use super::api;
use super::controls::{dotted_focus, font_of, Ctl};
use super::ctl_basic::{notify_with, NM_CLICK, NM_DBLCLK, NM_RCLICK, NM_RETURN};
use super::imagelist::CLR_NONE;
use super::msg::{current_ansi, def_window_proc};
use super::scroll::{self, SB_HORZ, SB_VERT};
use super::surface::*;
use super::wm::Class;
use super::*;

// ---- constants --------------------------------------------------------------------------------------
const LVS_TYPEMASK: u32 = 3;
const LVS_REPORT: u32 = 1;
const LVS_SMALLICON: u32 = 2;
const LVS_LIST: u32 = 3;
const LVS_SINGLESEL: u32 = 4;
const LVS_SHOWSELALWAYS: u32 = 8;
const LVS_OWNERDATA: u32 = 0x1000;
const LVS_NOCOLUMNHEADER: u32 = 0x4000;
const LVS_NOSORTHEADER: u32 = 0x8000;

const LVS_EX_GRIDLINES: u32 = 1;
const LVS_EX_CHECKBOXES: u32 = 4;
const LVS_EX_FULLROWSELECT: u32 = 0x20;

pub const LVIS_FOCUSED: u32 = 1;
pub const LVIS_SELECTED: u32 = 2;
const LVIS_STATEIMAGEMASK: u32 = 0xF000;

const LVIF_TEXT: u32 = 1;
const LVIF_IMAGE: u32 = 2;
const LVIF_PARAM: u32 = 4;
const LVIF_STATE: u32 = 8;
const LVIF_INDENT: u32 = 0x10;

const LVN_ITEMCHANGING: i32 = -100;
const LVN_ITEMCHANGED: i32 = -101;
const LVN_COLUMNCLICK: i32 = -108;
const LVN_ITEMACTIVATE: i32 = -114;
const LVN_GETDISPINFOA: i32 = -150;
const LVN_GETDISPINFOW: i32 = -177;
const LVN_KEYDOWN: i32 = -155;

const SMALL: usize = 1;

#[derive(Clone, Debug, Default)]
pub struct Column {
    pub text: String,
    pub width: i32,
    pub fmt: i32,
    pub image: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    /// The text of the item and of each sub-item.
    pub texts: Vec<String>,
    pub image: i32,
    pub state: u32,
    pub param: isize,
    pub indent: i32,
}

#[derive(Clone, Debug)]
pub struct ListView {
    pub columns: Vec<Column>,
    pub items: Vec<Item>,
    pub ext: u32,
    pub xoff: i32,
    pub yoff: i32,
    pub focus: i32,
    pub anchor: i32,
    /// Normal (large), small and state image lists.
    pub images: [usize; 3],
    pub text_color: u32,
    pub text_bk: u32,
    pub bk: u32,
    pub virtual_count: usize,
    pub hot: i32,
    pub col_drag: Option<(usize, i32, i32)>,
    pub col_press: i32,
    pub typed: String,
    pub typed_at: u32,
    pub dragging: bool,
}

impl Default for ListView {
    fn default() -> ListView {
        ListView {
            columns: Vec::new(),
            items: Vec::new(),
            ext: 0,
            xoff: 0,
            yoff: 0,
            focus: -1,
            anchor: -1,
            images: [0; 3],
            text_color: CLR_NONE,
            text_bk: CLR_NONE,
            bk: CLR_NONE,
            virtual_count: 0,
            hot: -1,
            col_drag: None,
            col_press: -1,
            typed: String::new(),
            typed_at: 0,
            dragging: false,
        }
    }
}

fn lv(g: &Gui, h: usize) -> Option<&ListView> {
    match &g.windows.get(&h)?.ctl {
        Ctl::ListView(l) => Some(l),
        _ => None,
    }
}

fn lv_mut(g: &mut Gui, h: usize) -> Option<&mut ListView> {
    match &mut g.windows.get_mut(&h)?.ctl {
        Ctl::ListView(l) => Some(l),
        _ => None,
    }
}

pub fn register_class(g: &mut Gui) {
    g.register_class(Class { name: "SysListView32".into(), atom: 0, style: 0x8, wndproc: listview_proc as *const () as usize, cls_extra: 0, wnd_extra: 0, hinstance: 0, icon: 0, cursor: 0, background: 0, menu_name: None, unicode: true, native: true });
}

// ---- geometry --------------------------------------------------------------------------------------------

fn view_of(g: &Gui, h: usize) -> u32 {
    g.windows.get(&h).map_or(0, |w| w.style & LVS_TYPEMASK)
}

fn style_of(g: &Gui, h: usize) -> u32 {
    g.windows.get(&h).map_or(0, |w| w.style)
}

fn count(g: &Gui, h: usize) -> usize {
    let Some(l) = lv(g, h) else { return 0 };
    if style_of(g, h) & LVS_OWNERDATA != 0 {
        l.virtual_count
    } else {
        l.items.len()
    }
}

fn header_h(g: &Gui, h: usize) -> i32 {
    if view_of(g, h) == LVS_REPORT && style_of(g, h) & LVS_NOCOLUMNHEADER == 0 {
        g.sized_font(font_of(g, h)).height() + 7
    } else {
        0
    }
}

fn row_h(g: &Gui, h: usize) -> i32 {
    let small = lv(g, h).and_then(|l| g.imagelist(l.images[SMALL])).map_or(0, |l| l.h);
    match view_of(g, h) {
        0 => {
            let big = lv(g, h).and_then(|l| g.imagelist(l.images[0])).map_or(32, |l| l.h);
            big + g.sized_font(font_of(g, h)).height() * 2 + 14
        }
        _ => (g.sized_font(font_of(g, h)).height().max(small) + 3).max(18),
    }
}

fn cell_w(g: &Gui, h: usize) -> i32 {
    match view_of(g, h) {
        0 => 80,
        2 => 140,
        _ => {
            // list view: as wide as the widest label
            let font = g.sized_font(font_of(g, h));
            lv(g, h).map_or(100, |l| l.items.iter().map(|i| font.text_width(i.texts.first().map_or("", |s| s.as_str()))).max().unwrap_or(60) + 40)
        }
    }
}

/// The area items occupy: below the header, inside the client rectangle.
fn body(g: &Gui, h: usize) -> Rect {
    let (cw, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
    Rect::new(0, header_h(g, h), cw, ch)
}

fn total_cols_w(l: &ListView) -> i32 {
    l.columns.iter().map(|c| c.width).sum()
}

/// Item rectangle in content coordinates (before scrolling).
fn item_rect_content(g: &Gui, h: usize, i: usize) -> Rect {
    let body = body(g, h);
    let (rh, view) = (row_h(g, h), view_of(g, h));
    match view {
        LVS_REPORT => {
            let w = lv(g, h).map_or(0, |l| total_cols_w(l)).max(body.width());
            Rect::new(0, i as i32 * rh, w, (i as i32 + 1) * rh)
        }
        0 | LVS_SMALLICON => {
            let cw = cell_w(g, h);
            let per_row = (body.width() / cw).max(1);
            let (col, row) = (i as i32 % per_row, i as i32 / per_row);
            Rect::new(col * cw, row * rh, (col + 1) * cw, (row + 1) * rh)
        }
        _ => {
            let cw = cell_w(g, h);
            let per_col = (body.height() / rh).max(1);
            let (col, row) = (i as i32 / per_col, i as i32 % per_col);
            Rect::new(col * cw, row * rh, (col + 1) * cw, (row + 1) * rh)
        }
    }
}

/// Item rectangle in client coordinates.
fn item_rect(g: &Gui, h: usize, i: usize) -> Rect {
    let r = item_rect_content(g, h, i);
    let b = body(g, h);
    let (xo, yo) = lv(g, h).map_or((0, 0), |l| (l.xoff, l.yoff));
    r.offset(b.l - xo, b.t - yo)
}

fn content_size(g: &Gui, h: usize) -> (i32, i32) {
    let n = count(g, h);
    if n == 0 {
        return (lv(g, h).map_or(0, total_cols_w), 0);
    }
    let last = item_rect_content(g, h, n - 1);
    match view_of(g, h) {
        LVS_REPORT => (lv(g, h).map_or(0, total_cols_w), last.b),
        0 | LVS_SMALLICON => (0, last.b),
        _ => (last.r, 0),
    }
}

/// Keeps the scroll bars (the window's own) in step with the content.
fn sync_bars(g: &mut Gui, h: usize) {
    let (cw0, ch0) = content_size(g, h);
    for _ in 0..2 {
        let b = body(g, h);
        let (vw, vh) = (b.width(), b.height());
        let (xo, yo) = lv(g, h).map_or((0, 0), |l| (l.xoff, l.yoff));
        let (cw, ch) = (cw0.max(0), ch0.max(0));
        g.scroll_set(h, SB_VERT, scroll::SIF_RANGE | scroll::SIF_PAGE | scroll::SIF_POS, 0, (ch - 1).max(0), vh.max(0) as u32, yo);
        g.scroll_set(h, SB_HORZ, scroll::SIF_RANGE | scroll::SIF_PAGE | scroll::SIF_POS, 0, (cw - 1).max(0), vw.max(0) as u32, xo);
    }
    let b = body(g, h);
    let (max_x, max_y) = ((cw0 - b.width()).max(0), (ch0 - b.height()).max(0));
    if let Some(l) = lv_mut(g, h) {
        l.xoff = l.xoff.clamp(0, max_x);
        l.yoff = l.yoff.clamp(0, max_y);
    }
    g.paint_nc(h);
}

fn ensure_visible(g: &mut Gui, h: usize, i: usize) {
    if i >= count(g, h) {
        return;
    }
    let r = item_rect_content(g, h, i);
    let b = body(g, h);
    let horizontal = view_of(g, h) == LVS_LIST;
    if let Some(l) = lv_mut(g, h) {
        if horizontal {
            if r.l < l.xoff {
                l.xoff = r.l;
            } else if r.r > l.xoff + b.width() {
                l.xoff = r.r - b.width();
            }
        } else if r.t < l.yoff {
            l.yoff = r.t;
        } else if r.b > l.yoff + b.height() {
            l.yoff = r.b - b.height();
        }
    }
    sync_bars(g, h);
}

// ---- text of an item -----------------------------------------------------------------------------------

/// Whether the parent window takes Unicode notifications (notification codes and strings differ for ANSI parents).
fn parent_unicode(h: usize) -> bool {
    with(|g| {
        let p = g.windows.get(&h).map_or(0, |w| w.parent);
        g.windows.get(&p).is_none_or(|w| w.unicode)
    })
}

/// The text shown for (item, sub-item); asks the parent for a virtual list.
unsafe fn text_of(h: usize, i: usize, sub: usize) -> String {
    let (virt, own) = with(|g| (style_of(g, h) & LVS_OWNERDATA != 0, lv(g, h).and_then(|l| l.items.get(i)).and_then(|it| it.texts.get(sub)).cloned()));
    if !virt {
        return own.unwrap_or_default();
    }
    // NMLVDISPINFO: the NMHDR, then an LVITEM (mask@0, iItem@4, iSubItem@8, pszText@24, cchTextMax@32); the parent fills pszText
    let wide = parent_unicode(h);
    let mut buf = vec![0u8; 520];
    let mut extra = vec![0u8; 88];
    extra[0..4].copy_from_slice(&LVIF_TEXT.to_le_bytes());
    extra[4..8].copy_from_slice(&(i as i32).to_le_bytes());
    extra[8..12].copy_from_slice(&(sub as i32).to_le_bytes());
    extra[24..32].copy_from_slice(&(buf.as_mut_ptr() as u64).to_le_bytes());
    extra[32..36].copy_from_slice(&(if wide { 259i32 } else { 519 }).to_le_bytes());
    let (_, back) = notify_with(h, if wide { LVN_GETDISPINFOW } else { LVN_GETDISPINFOA }, &extra);
    let ptr = u64::from_le_bytes(back[24..32].try_into().unwrap()) as usize;
    if ptr == 0 {
        String::new()
    } else if wide {
        crate::runtime::strings::w_ptr_to_string(ptr as *const u16)
    } else {
        crate::runtime::strings::a_to_string(ptr as *const u8)
    }
}

unsafe fn str_arg(p: usize, wide: bool) -> String {
    if p == 0 {
        String::new()
    } else if wide {
        crate::runtime::strings::w_ptr_to_string(p as *const u16)
    } else {
        crate::runtime::strings::a_to_string(p as *const u8)
    }
}

// ---- notifications ---------------------------------------------------------------------------------------

/// NMLISTVIEW / NMITEMACTIVATE body after the NMHDR: iItem, iSubItem, uNewState, uOldState, uChanged, ptAction, lParam, uKeyFlags.
fn nm_body(item: i32, sub: i32, new: u32, old: u32, changed: u32, pt: (i32, i32), param: isize) -> Vec<u8> {
    let mut b = vec![0u8; 48];
    b[0..4].copy_from_slice(&item.to_le_bytes());
    b[4..8].copy_from_slice(&sub.to_le_bytes());
    b[8..12].copy_from_slice(&new.to_le_bytes());
    b[12..16].copy_from_slice(&old.to_le_bytes());
    b[16..20].copy_from_slice(&changed.to_le_bytes());
    b[20..24].copy_from_slice(&pt.0.to_le_bytes());
    b[24..28].copy_from_slice(&pt.1.to_le_bytes());
    b[32..40].copy_from_slice(&(param as i64).to_le_bytes());
    b
}

unsafe fn notify_item(h: usize, code: i32, item: i32, sub: i32, pt: (i32, i32)) -> isize {
    let (state, param) = with(|g| lv(g, h).and_then(|l| l.items.get(item.max(0) as usize)).map_or((0, 0), |i| (i.state, i.param)));
    notify_with(h, code, &nm_body(item, sub, state, 0, 0, pt, param)).0
}

/// Changes the state bits `mask` of an item to `state`, with `LVN_ITEMCHANGING` / `LVN_ITEMCHANGED` around it.
unsafe fn set_state(h: usize, i: usize, state: u32, mask: u32) {
    let Some(old) = with(|g| lv(g, h).and_then(|l| l.items.get(i)).map(|it| it.state)) else {
        // a virtual list keeps no per-item state worth reporting
        return;
    };
    let new = (old & !mask) | (state & mask);
    if new == old {
        return;
    }
    let param = with(|g| lv(g, h).and_then(|l| l.items.get(i)).map_or(0, |it| it.param));
    if notify_with(h, LVN_ITEMCHANGING, &nm_body(i as i32, 0, new, old, LVIF_STATE, (0, 0), param)).0 != 0 {
        return;
    }
    with(|g| {
        if let Some(it) = lv_mut(g, h).and_then(|l| l.items.get_mut(i)) {
            it.state = new;
        }
        g.invalidate(h, None, false);
    });
    notify_with(h, LVN_ITEMCHANGED, &nm_body(i as i32, 0, new, old, LVIF_STATE, (0, 0), param));
}

unsafe fn select_only(h: usize, i: Option<usize>) {
    let n = with(|g| lv(g, h).map_or(0, |l| l.items.len()));
    for k in 0..n {
        let want = Some(k) == i;
        let cur = with(|g| lv(g, h).map_or(0, |l| l.items[k].state));
        let new = if want { LVIS_SELECTED | LVIS_FOCUSED } else { 0 };
        if cur & (LVIS_SELECTED | LVIS_FOCUSED) != new {
            set_state(h, k, new, LVIS_SELECTED | LVIS_FOCUSED);
        }
    }
    with(|g| {
        if let Some(l) = lv_mut(g, h) {
            l.focus = i.map_or(-1, |i| i as i32);
            l.anchor = l.focus;
        }
    });
}

// ---- painting -------------------------------------------------------------------------------------------------

/// The rows a repaint needs text for: those on screen in a report view, otherwise the first screenful.
fn visible_range(g: &Gui, h: usize) -> (usize, usize, usize) {
    let (b, rh, n) = (body(g, h), row_h(g, h).max(1), count(g, h));
    let cols = lv(g, h).map_or(1, |l| l.columns.len().max(1));
    if view_of(g, h) == LVS_REPORT {
        let yo = lv(g, h).map_or(0, |l| l.yoff);
        (((yo / rh) as usize).min(n), (((yo + b.height()) / rh + 1) as usize).min(n), cols)
    } else {
        (0, n.min(2000), 1)
    }
}

unsafe fn paint_lv(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    // a virtual list gets its text from the parent, which has to happen outside the window system's lock
    let mut cache: std::collections::HashMap<(usize, usize), String> = std::collections::HashMap::new();
    if with(|g| style_of(g, h) & LVS_OWNERDATA != 0) {
        let (first, last, cols) = with(|g| visible_range(g, h));
        for i in first..last {
            for s in 0..cols {
                cache.insert((i, s), text_of(h, i, s));
            }
        }
    }
    with(|g| draw_items(g, h, hdc, &cache));
    api::end_paint(h, ps.as_ptr());
}

fn draw_items(g: &mut Gui, h: usize, hdc: usize, cache: &std::collections::HashMap<(usize, usize), String>) {
    let Some(l) = lv(g, h).cloned() else { return };
    let (view, style) = (view_of(g, h), style_of(g, h));
    let (focused, enabled) = (g.focus == h, g.is_enabled(h));
    let f = font_of(g, h);
    g.select_object(hdc, f);
    let font = g.sized_font(f);
    let hh = header_h(g, h);
    let b = body(g, h);
    let (cw, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
    let rh = row_h(g, h);
    let n = count(g, h);
    let virt = style & LVS_OWNERDATA != 0;
    let full_row = l.ext & LVS_EX_FULLROWSELECT != 0;
    let checks = l.ext & LVS_EX_CHECKBOXES != 0;
    let grid = l.ext & LVS_EX_GRIDLINES != 0 && view == LVS_REPORT;
    let text_color = if !enabled {
        0x838383
    } else if l.text_color != CLR_NONE {
        from_colorref(l.text_color)
    } else {
        0
    };
    let bg = if l.bk != CLR_NONE { from_colorref(l.bk) } else { 0xFFFFFF };
    let small_h = g.imagelist(l.images[SMALL]).map(|i| (i.w, i.h));
    let big_h = g.imagelist(l.images[0]).map(|i| (i.w, i.h)).unwrap_or((32, 32));
    let sel_bg = if focused { 0xCCE8FF } else { 0xD9D9D9 };
    let hot_bg = 0xE5F3FF;
    g.fill_rect(hdc, Rect::new(0, 0, cw, ch), &Brush::Solid(bg));
    let col_x: Vec<i32> = l
        .columns
        .iter()
        .scan(-l.xoff, |x, c| {
            let s = *x;
            *x += c.width;
            Some(s)
        })
        .collect();
    let row_w = total_cols_w(&l).max(b.width());
    let rects: Vec<(usize, Rect)> = (0..n)
        .map(|i| (i, item_rect(g, h, i)))
        .filter(|(_, r)| r.r > b.l && r.l < b.r && r.b > b.t && r.t < b.b)
        .collect();
    let text = |i: usize, s: usize| -> String {
        if virt {
            cache.get(&(i, s)).cloned().unwrap_or_default()
        } else {
            l.items.get(i).and_then(|it| it.texts.get(s)).cloned().unwrap_or_default()
        }
    };
    // (image list, image, x, y) drawn once the text pass is done
    let mut images: Vec<(usize, usize, i32, i32)> = Vec::new();
    g.draw(hdc, |cv, _| {
        let saved = cv.clip;
        cv.clip = cv.clip.intersect(&b.offset(cv.ox, cv.oy));
        for (i, r) in &rects {
            let (state, image, indent) = if virt { (0, -1, 0) } else { l.items.get(*i).map_or((0, -1, 0), |it| (it.state, it.image, it.indent)) };
            let selected = state & LVIS_SELECTED != 0 && (focused || style & LVS_SHOWSELALWAYS != 0);
            let hot = l.hot == *i as i32 && enabled;
            let ty = r.t + (rh - font.height()) / 2;
            let shade = |cv: &mut Canvas, area: Rect| {
                if selected {
                    cv.fill_rect(area, sel_bg);
                } else if hot {
                    cv.fill_rect(area, hot_bg);
                } else if l.text_bk != CLR_NONE {
                    cv.fill_rect(area, from_colorref(l.text_bk));
                }
            };
            if view == LVS_REPORT {
                let last_right = l.columns.last().map_or(cw, |c| col_x[l.columns.len() - 1] + c.width);
                if full_row {
                    shade(cv, Rect::new(r.l, r.t, last_right.max(r.l), r.b));
                }
                for (c, col) in l.columns.iter().enumerate() {
                    let cell = Rect::new(col_x[c], r.t, col_x[c] + col.width, r.b);
                    let mut x = cell.l + 4 + if c == 0 { indent * 8 } else { 0 };
                    if c == 0 {
                        if checks {
                            let cb = Rect::new(x, r.t + (rh - 13) / 2, x + 13, r.t + (rh - 13) / 2 + 13);
                            cv.fill_rect(cb, 0xFFFFFF);
                            cv.frame_rect(cb, 0x333333, 1);
                            if (state & LVIS_STATEIMAGEMASK) >> 12 == 2 {
                                let pen = Pen { style: PenStyle::Solid, width: 1, color: 0x0078D7 };
                                for d in 0..2 {
                                    cv.line(cb.l + 3, cb.t + 6 + d, cb.l + 5, cb.t + 9 + d, &pen);
                                    cv.line(cb.l + 5, cb.t + 9 + d, cb.l + 10, cb.t + 3 + d, &pen);
                                }
                            }
                            x += 18;
                        }
                        if let (Some((iw, ih)), true) = (small_h, image >= 0) {
                            images.push((l.images[SMALL], image as usize, x, r.t + (rh - ih) / 2));
                            x += iw + 4;
                        }
                        if !full_row {
                            let tw = font.text_width(&text(*i, 0));
                            shade(cv, Rect::new(x - 2, r.t, (x + tw + 3).min(cell.r), r.b));
                        }
                    }
                    let label = text(*i, c);
                    let tw = font.text_width(&label);
                    let tx = match col.fmt & 3 {
                        1 => (cell.r - 6 - tw).max(x),
                        2 => (x + (cell.r - x - tw) / 2).max(x),
                        _ => x,
                    };
                    let outer = cv.clip;
                    cv.clip = outer.intersect(&Rect::new(cell.l, cell.t, cell.r - 2, cell.b).offset(cv.ox, cv.oy));
                    font.draw(cv, tx, ty, &label, text_color, None);
                    cv.clip = outer;
                }
                if l.focus == *i as i32 && focused {
                    dotted_focus(cv, Rect::new(r.l + 1, r.t, last_right.max(r.l + 2) - 1, r.b));
                }
                if grid {
                    cv.fill_rect(Rect::new(r.l, r.b - 1, r.l + row_w, r.b), 0xD8D8D8);
                    for (c, col) in l.columns.iter().enumerate() {
                        cv.fill_rect(Rect::new(col_x[c] + col.width - 1, r.t, col_x[c] + col.width, r.b), 0xD8D8D8);
                    }
                }
            } else if view == 0 {
                // a large icon with its label underneath
                let label = text(*i, 0);
                let tw = font.text_width(&label);
                if image >= 0 {
                    images.push((l.images[0], image as usize, r.l + (r.width() - big_h.0) / 2, r.t + 4));
                }
                let (tx, ty) = (r.l + ((r.width() - tw) / 2).max(2), r.t + 4 + big_h.1 + 4);
                if selected {
                    cv.fill_rect(Rect::new(tx - 2, ty - 1, (tx + tw + 2).min(r.r), ty + font.height() + 1), sel_bg);
                }
                let outer = cv.clip;
                cv.clip = outer.intersect(&Rect::new(r.l, r.t, r.r, r.b).offset(cv.ox, cv.oy));
                font.draw(cv, tx, ty, &label, text_color, None);
                cv.clip = outer;
            } else {
                // small icons and list view: icon and label side by side
                let label = text(*i, 0);
                let tw = font.text_width(&label);
                let mut x = r.l + 4;
                if let (Some((iw, ih)), true) = (small_h, image >= 0) {
                    images.push((l.images[SMALL], image as usize, x, r.t + (rh - ih) / 2));
                    x += iw + 4;
                }
                shade(cv, Rect::new(x - 2, r.t, (x + tw + 3).min(r.r), r.b));
                font.draw(cv, x, ty, &label, text_color, None);
                if l.focus == *i as i32 && focused {
                    dotted_focus(cv, Rect::new(x - 2, r.t, (x + tw + 3).min(r.r), r.b));
                }
            }
        }
        cv.clip = saved;
    });
    for (list, idx, x, y) in images {
        g.draw_imagelist(hdc, list, idx, x, y, 0, 0);
    }
    if hh > 0 {
        draw_header(g, h, hdc, &l);
    }
}

fn draw_header(g: &mut Gui, h: usize, hdc: usize, l: &ListView) {
    let (cw, _) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
    let hh = header_h(g, h);
    let font = g.sized_font(font_of(g, h));
    let enabled = g.is_enabled(h);
    let nosort = style_of(g, h) & LVS_NOSORTHEADER != 0;
    let mut x = -l.xoff;
    let mut cols = Vec::new();
    for (i, c) in l.columns.iter().enumerate() {
        cols.push((i, Rect::new(x, 0, x + c.width, hh), c.clone()));
        x += c.width;
    }
    let end = x;
    g.draw(hdc, |cv, _| {
        cv.fill_rect(Rect::new(0, 0, cw, hh), 0xFFFFFF);
        for (i, r, c) in &cols {
            let pressed = l.col_press == *i as i32;
            if pressed {
                cv.fill_rect(*r, 0xCCE8FF);
            }
            let saved = cv.clip;
            cv.clip = cv.clip.intersect(&Rect::new(r.l, r.t, r.r - 1, r.b).offset(cv.ox, cv.oy));
            let tw = font.text_width(&c.text);
            let tx = match c.fmt & 3 {
                1 => r.r - 8 - tw,
                2 => r.l + (r.width() - tw) / 2,
                _ => r.l + 6,
            };
            font.draw(cv, tx.max(r.l + 3), (hh - font.height()) / 2, &c.text, if enabled && !nosort || enabled { 0x4C4C4C } else { 0x838383 }, None);
            cv.clip = saved;
            cv.fill_rect(Rect::new(r.r - 1, 3, r.r, hh - 3), 0xE5E5E5);
        }
        let _ = end;
        cv.fill_rect(Rect::new(0, hh - 1, cw, hh), 0xE5E5E5);
    });
}

// ---- hit testing ------------------------------------------------------------------------------------------------

/// The item at a client point and the sub-item column, if any.
fn item_at(g: &Gui, h: usize, x: i32, y: i32) -> Option<(usize, usize)> {
    let b = body(g, h);
    if !b.contains(x, y) {
        return None;
    }
    let n = count(g, h);
    let l = lv(g, h)?;
    if view_of(g, h) == LVS_REPORT {
        let rh = row_h(g, h);
        let i = ((y - b.t + l.yoff) / rh) as usize;
        if i >= n {
            return None;
        }
        let mut cx = -l.xoff;
        for (c, col) in l.columns.iter().enumerate() {
            if x < cx + col.width {
                return Some((i, c));
            }
            cx += col.width;
        }
        return Some((i, 0));
    }
    (0..n).find(|i| item_rect(g, h, *i).contains(x, y)).map(|i| (i, 0))
}

/// The column separator within 3 pixels of x in the header, if any.
fn separator_at(g: &Gui, h: usize, x: i32) -> Option<usize> {
    let l = lv(g, h)?;
    let mut cx = -l.xoff;
    for (i, c) in l.columns.iter().enumerate() {
        cx += c.width;
        if (x - cx).abs() <= 3 {
            return Some(i);
        }
    }
    None
}

fn header_column_at(g: &Gui, h: usize, x: i32) -> Option<usize> {
    let l = lv(g, h)?;
    let mut cx = -l.xoff;
    for (i, c) in l.columns.iter().enumerate() {
        if x >= cx && x < cx + c.width {
            return Some(i);
        }
        cx += c.width;
    }
    None
}

// ---- items ---------------------------------------------------------------------------------------------------------

/// Reads an LVITEM (mask @0, iItem @4, iSubItem @8, state @12, stateMask @16, pszText @24, iImage @36, lParam @40, iIndent @48).
unsafe fn apply_lvitem(it: &mut Item, p: *const u8, wide: bool) {
    let mask = *(p as *const u32);
    let sub = *(p.add(8) as *const i32) as usize;
    if mask & LVIF_TEXT != 0 {
        let t = str_arg(*(p.add(24) as *const u64) as usize, wide);
        if it.texts.len() <= sub {
            it.texts.resize(sub + 1, String::new());
        }
        it.texts[sub] = t;
    }
    if mask & LVIF_IMAGE != 0 && sub == 0 {
        it.image = *(p.add(36) as *const i32);
    }
    if mask & LVIF_PARAM != 0 {
        it.param = *(p.add(40) as *const i64) as isize;
    }
    if mask & LVIF_STATE != 0 {
        let (state, smask) = (*(p.add(12) as *const u32), *(p.add(16) as *const u32));
        it.state = (it.state & !smask) | (state & smask);
    }
    if mask & LVIF_INDENT != 0 {
        it.indent = *(p.add(48) as *const i32);
    }
}

unsafe fn fill_lvitem(it: &Item, p: *mut u8, wide: bool) {
    let mask = *(p as *const u32);
    let sub = *(p.add(8) as *const i32) as usize;
    if mask & LVIF_TEXT != 0 {
        let buf = *(p.add(24) as *const u64) as usize;
        let max = *(p.add(32) as *const i32);
        if buf != 0 && max > 0 {
            let t = it.texts.get(sub).map_or("", |s| s.as_str());
            if wide {
                crate::runtime::strings::return_w(buf as *mut u16, max as u32, t);
            } else {
                crate::runtime::strings::return_a(buf as *mut u8, max as u32, t);
            }
        }
    }
    if mask & LVIF_IMAGE != 0 {
        *(p.add(36) as *mut i32) = it.image;
    }
    if mask & LVIF_PARAM != 0 {
        *(p.add(40) as *mut i64) = it.param as i64;
    }
    if mask & LVIF_STATE != 0 {
        let smask = *(p.add(16) as *const u32);
        *(p.add(12) as *mut u32) = it.state & smask;
    }
    if mask & LVIF_INDENT != 0 {
        *(p.add(48) as *mut i32) = it.indent;
    }
}

fn next_item(g: &Gui, h: usize, start: i32, flags: u32) -> i32 {
    let Some(l) = lv(g, h) else { return -1 };
    let n = l.items.len() as i32;
    let matches = |it: &Item| -> bool {
        let want = flags & 0x0F;
        want == 0 || (want & 1 != 0 && it.state & LVIS_FOCUSED != 0) || (want & 2 != 0 && it.state & LVIS_SELECTED != 0)
    };
    if flags & 0x0F00 != 0 {
        // above / below: neighbours in the order of the list
        let step = if flags & 0x100 != 0 { -1 } else { 1 };
        let i = start + step;
        return if i >= 0 && i < n { i } else { -1 };
    }
    ((start + 1).max(0)..n).find(|i| matches(&l.items[*i as usize])).unwrap_or(-1)
}

unsafe fn set_selection_to(h: usize, i: usize, shift: bool, ctrl: bool) {
    let single = with(|g| style_of(g, h) & LVS_SINGLESEL != 0);
    if single || (!shift && !ctrl) {
        select_only(h, Some(i));
    } else if ctrl {
        let cur = with(|g| lv(g, h).map_or(0, |l| l.items[i].state));
        set_state(h, i, if cur & LVIS_SELECTED != 0 { 0 } else { LVIS_SELECTED }, LVIS_SELECTED);
        with(|g| {
            if let Some(l) = lv_mut(g, h) {
                l.focus = i as i32;
                l.anchor = i as i32;
            }
        });
    } else {
        let anchor = with(|g| lv(g, h).map_or(0, |l| l.anchor.max(0) as usize));
        let (lo, hi) = (anchor.min(i), anchor.max(i));
        let n = with(|g| lv(g, h).map_or(0, |l| l.items.len()));
        for k in 0..n {
            set_state(h, k, if (lo..=hi).contains(&k) { LVIS_SELECTED } else { 0 }, LVIS_SELECTED);
        }
        with(|g| {
            if let Some(l) = lv_mut(g, h) {
                l.focus = i as i32;
            }
        });
    }
    with(|g| {
        ensure_visible(g, h, i);
        g.invalidate(h, None, false);
    });
}

unsafe fn toggle_check(h: usize, i: usize) {
    let cur = with(|g| lv(g, h).map_or(0, |l| l.items[i].state));
    let checked = (cur & LVIS_STATEIMAGEMASK) >> 12 == 2;
    set_state(h, i, if checked { 1 << 12 } else { 2 << 12 }, LVIS_STATEIMAGEMASK);
}

pub unsafe extern "win64" fn listview_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::ListView(Box::default());
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
            paint_lv(h);
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
        WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS => {
            with(|g| g.invalidate(h, None, false));
            0
        }
        WM_GETDLGCODE => 0x81,
        WM_VSCROLL | WM_HSCROLL => {
            let code = loword(wp) as u32;
            let vertical = msg == WM_VSCROLL;
            let (pos, page, max, track, line) = with(|g| {
                let b = body(g, h);
                let (cw, ch) = content_size(g, h);
                let l = lv(g, h).unwrap();
                let w = g.windows.get(&h).unwrap();
                let bar = if vertical { w.sb.vert } else { w.sb.horz };
                (if vertical { l.yoff } else { l.xoff }, if vertical { b.height() } else { b.width() }, if vertical { ch } else { cw }, bar.track, if vertical { row_h(g, h) } else { 16 })
            });
            let new = match code {
                scroll::SB_LINEUP => pos - line,
                scroll::SB_LINEDOWN => pos + line,
                scroll::SB_PAGEUP => pos - page,
                scroll::SB_PAGEDOWN => pos + page,
                scroll::SB_THUMBTRACK | scroll::SB_THUMBPOSITION => track,
                scroll::SB_TOP => 0,
                scroll::SB_BOTTOM => max,
                _ => return 0,
            };
            with(|g| {
                if let Some(l) = lv_mut(g, h) {
                    if vertical {
                        l.yoff = new.clamp(0, (max - page).max(0));
                    } else {
                        l.xoff = new.clamp(0, (max - page).max(0));
                    }
                }
                sync_bars(g, h);
                g.invalidate(h, None, false);
            });
            0
        }
        WM_MOUSEWHEEL => {
            let delta = (wp >> 16) as u16 as i16 as i32;
            with(|g| {
                let step = row_h(g, h) * 3;
                let max = (content_size(g, h).1 - body(g, h).height()).max(0);
                if let Some(l) = lv_mut(g, h) {
                    l.yoff = (l.yoff - delta / 120 * step).clamp(0, max);
                }
                sync_bars(g, h);
                g.invalidate(h, None, false);
            });
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN => {
            if !with(|g| g.is_enabled(h)) {
                return 0;
            }
            api::set_focus(h);
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            let right = msg == WM_RBUTTONDOWN;
            let in_header = with(|g| header_h(g, h) > 0 && y >= 0 && y < header_h(g, h));
            if in_header {
                if right {
                    return 0;
                }
                let drag = with(|g| separator_at(g, h, x).map(|c| (c, x, lv(g, h).map_or(0, |l| l.columns[c].width))));
                if let Some(d) = drag {
                    with(|g| {
                        g.capture = h;
                        if let Some(l) = lv_mut(g, h) {
                            l.col_drag = Some(d);
                        }
                    });
                } else if let Some(c) = with(|g| header_column_at(g, h, x)) {
                    with(|g| {
                        g.capture = h;
                        if let Some(l) = lv_mut(g, h) {
                            l.col_press = c as i32;
                        }
                        g.invalidate(h, None, false);
                    });
                }
                return 0;
            }
            let hit = with(|g| item_at(g, h, x, y));
            let virt = with(|g| style_of(g, h) & LVS_OWNERDATA != 0);
            match hit {
                Some((i, sub)) if !virt => {
                    let (shift, ctrl) = with(|g| (g.keys[0x10] & 0x80 != 0, g.keys[0x11] & 0x80 != 0));
                    // a click on the check box toggles it
                    let on_check = with(|g| {
                        let l = lv(g, h).unwrap();
                        l.ext & LVS_EX_CHECKBOXES != 0 && view_of(g, h) == LVS_REPORT && x - (item_rect(g, h, i).l + 4) < 16 && sub == 0
                    });
                    if on_check && !right {
                        toggle_check(h, i);
                    } else if right {
                        // a right click selects an item that was not selected
                        let already = with(|g| lv(g, h).is_some_and(|l| l.items[i].state & LVIS_SELECTED != 0));
                        if !already {
                            set_selection_to(h, i, false, false);
                        }
                    } else {
                        set_selection_to(h, i, shift, ctrl);
                    }
                    if !right {
                        notify_item(h, if msg == WM_LBUTTONDBLCLK { NM_DBLCLK } else { NM_CLICK }, i as i32, sub as i32, (x, y));
                        if msg == WM_LBUTTONDBLCLK {
                            notify_item(h, LVN_ITEMACTIVATE, i as i32, sub as i32, (x, y));
                        }
                    }
                }
                Some((i, sub)) => {
                    if !right {
                        notify_item(h, NM_CLICK, i as i32, sub as i32, (x, y));
                    }
                }
                None => {
                    if !virt && !right {
                        select_only(h, None);
                    }
                    if !right {
                        notify_item(h, NM_CLICK, -1, 0, (x, y));
                    }
                }
            }
            0
        }
        WM_RBUTTONUP => {
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            let hit = with(|g| item_at(g, h, x, y));
            let (i, sub) = hit.map_or((-1, 0), |(i, s)| (i as i32, s as i32));
            let r = notify_item(h, NM_RCLICK, i, sub, (x, y));
            if r == 0 {
                // the default turns the click into WM_CONTEXTMENU for the parent
                let (sx, sy) = with(|g| g.client_to_screen(h, x, y));
                api::send(h, 0x007B, h, ((sy as isize) << 16) | (sx as isize & 0xFFFF));
            }
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            with(|g| {
                if let Some((c, x0, w0)) = lv(g, h).and_then(|l| l.col_drag) {
                    let new = (w0 + x - x0).max(10);
                    if let Some(l) = lv_mut(g, h) {
                        l.columns[c].width = new;
                    }
                    sync_bars(g, h);
                    g.invalidate(h, None, true);
                    return;
                }
                let hot = item_at(g, h, x, y).map_or(-1, |(i, _)| i as i32);
                if lv(g, h).is_some_and(|l| l.hot != hot) {
                    if let Some(l) = lv_mut(g, h) {
                        l.hot = hot;
                    }
                    g.invalidate(h, None, false);
                }
                g.track_leave(h);
                // the resize cursor over a column separator
                let on_sep = header_h(g, h) > 0 && y >= 0 && y < header_h(g, h) && separator_at(g, h, x).is_some();
                g.set_cursor_for(h, if on_sep { super::backend::Cursor::SizeWE } else { super::backend::Cursor::Arrow });
            });
            0
        }
        WM_MOUSELEAVE => {
            with(|g| {
                if let Some(l) = lv_mut(g, h) {
                    l.hot = -1;
                }
                g.invalidate(h, None, false);
            });
            0
        }
        WM_LBUTTONUP => {
            let (pressed, x) = (with(|g| lv(g, h).map_or(-1, |l| l.col_press)), lo_signed(lp));
            let (released, dragging) = with(|g| {
                let dragging = lv(g, h).is_some_and(|l| l.col_drag.is_some());
                if let Some(l) = lv_mut(g, h) {
                    l.col_drag = None;
                    l.col_press = -1;
                }
                if g.capture == h {
                    g.capture = 0;
                }
                g.invalidate(h, None, true);
                (header_column_at(g, h, x), dragging)
            });
            if pressed >= 0 && !dragging && released == Some(pressed as usize) {
                let pt = (x, hi_signed(lp));
                notify_with(h, LVN_COLUMNCLICK, &nm_body(-1, pressed, 0, 0, 0, pt, 0));
            }
            0
        }
        WM_KEYDOWN => {
            let (n, focus, view, virt) = with(|g| (lv(g, h).map_or(0, |l| l.items.len()) as i32, lv(g, h).map_or(-1, |l| l.focus), view_of(g, h), style_of(g, h) & LVS_OWNERDATA != 0));
            let mut kd = [0u8; 8];
            kd[0..2].copy_from_slice(&(wp as u16).to_le_bytes());
            notify_with(h, LVN_KEYDOWN, &kd);
            if virt {
                return 0;
            }
            let rows = with(|g| ((body(g, h).height() / row_h(g, h)).max(1)) as i32);
            let (shift, ctrl) = with(|g| (g.keys[0x10] & 0x80 != 0, g.keys[0x11] & 0x80 != 0));
            let target = match wp as u32 {
                0x26 => Some((focus - 1).max(0)),
                0x28 => Some((focus + 1).min(n - 1)),
                0x25 if view != LVS_REPORT => Some((focus - 1).max(0)),
                0x27 if view != LVS_REPORT => Some((focus + 1).min(n - 1)),
                0x21 => Some((focus - rows).max(0)),
                0x22 => Some((focus + rows).min(n - 1)),
                0x24 => Some(0),
                0x23 => Some(n - 1),
                0x20 => {
                    if focus >= 0 && with(|g| lv(g, h).is_some_and(|l| l.ext & LVS_EX_CHECKBOXES != 0)) {
                        toggle_check(h, focus as usize);
                    }
                    None
                }
                0x0D => {
                    if focus >= 0 {
                        notify_item(h, NM_RETURN, focus, 0, (0, 0));
                        notify_item(h, LVN_ITEMACTIVATE, focus, 0, (0, 0));
                    }
                    None
                }
                0x41 if ctrl => {
                    for k in 0..n {
                        set_state(h, k as usize, LVIS_SELECTED, LVIS_SELECTED);
                    }
                    None
                }
                _ => None,
            };
            if let Some(t) = target.filter(|t| n > 0 && *t >= 0 && *t < n) {
                set_selection_to(h, t as usize, shift, false);
            }
            0
        }
        WM_CHAR => {
            // type-ahead on the item labels
            let c = char::from_u32(wp as u32).unwrap_or('\0');
            if c.is_control() || with(|g| style_of(g, h) & LVS_OWNERDATA != 0) {
                return 0;
            }
            let now = with(|g| g.tick());
            let prefix = with(|g| {
                let l = lv_mut(g, h)?;
                if now.wrapping_sub(l.typed_at) > 1000 {
                    l.typed.clear();
                }
                l.typed.push(c);
                l.typed_at = now;
                Some(l.typed.to_lowercase())
            });
            if let Some(p) = prefix {
                let found = with(|g| {
                    let l = lv(g, h)?;
                    let n = l.items.len();
                    let start = if p.chars().count() == 1 { (l.focus + 1).max(0) as usize } else { l.focus.max(0) as usize };
                    (0..n).map(|k| (start + k) % n).find(|i| l.items[*i].texts.first().is_some_and(|t| t.to_lowercase().starts_with(&p)))
                });
                if let Some(i) = found {
                    set_selection_to(h, i, false, false);
                }
            }
            0
        }
        // LVM_GETITEMCOUNT
        0x1004 => with(|g| count(g, h) as isize),
        // LVM_INSERTITEMA / W
        0x1007 | 0x104D => {
            let p = lp as *const u8;
            let mut it = Item { image: -1, texts: vec![String::new()], ..Default::default() };
            apply_lvitem(&mut it, p, msg == 0x104D);
            let at = (*(p.add(4) as *const i32)).max(0) as usize;
            let virt = with(|g| style_of(g, h) & LVS_OWNERDATA != 0);
            if virt {
                return -1;
            }
            let idx = with(|g| {
                let l = lv_mut(g, h)?;
                let at = at.min(l.items.len());
                l.items.insert(at, it);
                if l.focus >= at as i32 {
                    l.focus += 1;
                }
                sync_bars(g, h);
                g.invalidate(h, None, true);
                Some(at)
            });
            idx.map_or(-1, |i| i as isize)
        }
        // LVM_GETITEMA / W
        0x1005 | 0x104B => with(|g| {
            let p = lp as *mut u8;
            let i = *(p.add(4) as *const i32);
            match lv(g, h).and_then(|l| l.items.get(i.max(0) as usize)) {
                Some(it) => {
                    fill_lvitem(it, p, msg == 0x104B);
                    1
                }
                None => 0,
            }
        }),
        // LVM_SETITEMA / W
        0x1006 | 0x104C => {
            let p = lp as *const u8;
            let i = *(p.add(4) as *const i32);
            let ok = with(|g| {
                let it = lv_mut(g, h).and_then(|l| l.items.get_mut(i.max(0) as usize))?;
                apply_lvitem(it, p, msg == 0x104C);
                g.invalidate(h, None, false);
                Some(())
            });
            ok.is_some() as isize
        }
        // LVM_DELETEITEM
        0x1008 => with(|g| {
            let Some(l) = lv_mut(g, h) else { return 0 };
            if wp >= l.items.len() {
                return 0;
            }
            l.items.remove(wp);
            if l.focus == wp as i32 {
                l.focus = -1;
            } else if l.focus > wp as i32 {
                l.focus -= 1;
            }
            sync_bars(g, h);
            g.invalidate(h, None, true);
            1
        }),
        // LVM_DELETEALLITEMS
        0x1009 => with(|g| {
            if let Some(l) = lv_mut(g, h) {
                l.items.clear();
                l.focus = -1;
                l.xoff = 0;
                l.yoff = 0;
            }
            sync_bars(g, h);
            g.invalidate(h, None, true);
            1
        }),
        // LVM_SETITEMCOUNT (virtual lists)
        0x102F => with(|g| {
            if let Some(l) = lv_mut(g, h) {
                l.virtual_count = wp;
            }
            sync_bars(g, h);
            g.invalidate(h, None, true);
            1
        }),
        // LVM_INSERTCOLUMNA / W
        0x101B | 0x1061 => {
            let p = lp as *const u8;
            let mask = *(p as *const u32);
            let mut c = Column { image: -1, width: if mask & 2 != 0 { *(p.add(8) as *const i32) } else { 80 }, fmt: if mask & 1 != 0 { *(p.add(4) as *const i32) } else { 0 }, ..Default::default() };
            if mask & 4 != 0 {
                c.text = str_arg(*(p.add(16) as *const u64) as usize, msg == 0x1061);
            }
            let at = wp;
            with(|g| {
                let Some(l) = lv_mut(g, h) else { return -1 };
                let at = at.min(l.columns.len());
                l.columns.insert(at, c);
                sync_bars(g, h);
                g.invalidate(h, None, true);
                at as isize
            })
        }
        // LVM_GETCOLUMNA / W, SETCOLUMNA / W
        0x1019 | 0x105F => with(|g| {
            let p = lp as *mut u8;
            let Some(c) = lv(g, h).and_then(|l| l.columns.get(wp)).cloned() else { return 0 };
            let mask = *(p as *const u32);
            if mask & 1 != 0 {
                *(p.add(4) as *mut i32) = c.fmt;
            }
            if mask & 2 != 0 {
                *(p.add(8) as *mut i32) = c.width;
            }
            if mask & 4 != 0 {
                let buf = *(p.add(16) as *const u64) as usize;
                let max = *(p.add(24) as *const i32);
                if buf != 0 && max > 0 {
                    if msg == 0x105F {
                        crate::runtime::strings::return_w(buf as *mut u16, max as u32, &c.text);
                    } else {
                        crate::runtime::strings::return_a(buf as *mut u8, max as u32, &c.text);
                    }
                }
            }
            1
        }),
        0x101A | 0x1060 => {
            let p = lp as *const u8;
            let mask = *(p as *const u32);
            with(|g| {
                let Some(c) = lv_mut(g, h).and_then(|l| l.columns.get_mut(wp)) else { return 0 };
                if mask & 1 != 0 {
                    c.fmt = *(p.add(4) as *const i32);
                }
                if mask & 2 != 0 {
                    c.width = *(p.add(8) as *const i32);
                }
                if mask & 4 != 0 {
                    c.text = str_arg(*(p.add(16) as *const u64) as usize, msg == 0x1060);
                }
                sync_bars(g, h);
                g.invalidate(h, None, true);
                1
            })
        }
        0x101C => with(|g| {
            let Some(l) = lv_mut(g, h) else { return 0 };
            if wp >= l.columns.len() {
                return 0;
            }
            l.columns.remove(wp);
            sync_bars(g, h);
            g.invalidate(h, None, true);
            1
        }),
        0x101D => with(|g| lv(g, h).and_then(|l| l.columns.get(wp)).map_or(0, |c| c.width as isize)),
        // LVM_SETCOLUMNWIDTH: -1 fits the content, -2 the header too
        0x101E => {
            let width = lo_signed(lp);
            let new = if width < 0 {
                let texts: Vec<String> = with(|g| lv(g, h).map_or(Vec::new(), |l| l.items.iter().map(|i| i.texts.get(wp).cloned().unwrap_or_default()).collect()));
                with(|g| {
                    let font = g.sized_font(font_of(g, h));
                    let mut w = texts.iter().map(|t| font.text_width(t)).max().unwrap_or(0) + 12 + if wp == 0 { 20 } else { 0 };
                    if width == -2 {
                        w = w.max(lv(g, h).and_then(|l| l.columns.get(wp)).map_or(0, |c| font.text_width(&c.text) + 16));
                    }
                    w
                })
            } else {
                width
            };
            with(|g| {
                let Some(c) = lv_mut(g, h).and_then(|l| l.columns.get_mut(wp)) else { return 0 };
                c.width = new;
                sync_bars(g, h);
                g.invalidate(h, None, true);
                1
            })
        }
        // LVM_GETITEMTEXTA / W, SETITEMTEXTA / W: LVITEM with iSubItem, pszText, cchTextMax
        0x102D | 0x1073 => {
            let p = lp as *const u8;
            let sub = *(p.add(8) as *const i32) as usize;
            let text = text_of(h, wp, sub);
            let buf = *(p.add(24) as *const u64) as usize;
            let max = *(p.add(32) as *const i32);
            if buf != 0 && max > 0 {
                if msg == 0x1073 {
                    return crate::runtime::strings::return_w(buf as *mut u16, max as u32, &text) as isize;
                }
                return crate::runtime::strings::return_a(buf as *mut u8, max as u32, &text) as isize;
            }
            0
        }
        0x102E | 0x1074 => {
            let p = lp as *const u8;
            let sub = *(p.add(8) as *const i32) as usize;
            let text = str_arg(*(p.add(24) as *const u64) as usize, msg == 0x1074);
            with(|g| {
                let Some(it) = lv_mut(g, h).and_then(|l| l.items.get_mut(wp)) else { return 0 };
                if it.texts.len() <= sub {
                    it.texts.resize(sub + 1, String::new());
                }
                it.texts[sub] = text;
                g.invalidate(h, None, false);
                1
            })
        }
        // LVM_SETITEMSTATE: wParam item (-1 for all), lParam an LVITEM with state and stateMask
        0x102B => {
            let p = lp as *const u8;
            let (state, mask) = (*(p.add(12) as *const u32), *(p.add(16) as *const u32));
            let n = with(|g| lv(g, h).map_or(0, |l| l.items.len()));
            let targets: Vec<usize> = if wp as i32 == -1 { (0..n).collect() } else { vec![wp] };
            for t in targets {
                if t < n {
                    set_state(h, t, state, mask);
                    if state & LVIS_FOCUSED != 0 {
                        with(|g| {
                            if let Some(l) = lv_mut(g, h) {
                                l.focus = t as i32;
                            }
                        });
                    }
                }
            }
            1
        }
        0x102C => with(|g| lv(g, h).and_then(|l| l.items.get(wp)).map_or(0, |i| (i.state & lp as u32) as isize)),
        0x1032 => with(|g| lv(g, h).map_or(0, |l| l.items.iter().filter(|i| i.state & LVIS_SELECTED != 0).count() as isize)),
        // LVM_GETNEXTITEM
        0x100C => with(|g| next_item(g, h, wp as i32, lp as u32) as isize),
        // LVM_FINDITEMA / W: a text or lParam search (LVFINDINFO flags @0, psz @8, lParam @16)
        0x100D | 0x1053 => {
            let p = lp as *const u8;
            let flags = *(p as *const u32);
            let text = if flags & 2 != 0 || flags & 8 != 0 { str_arg(*(p.add(8) as *const u64) as usize, msg == 0x1053) } else { String::new() };
            let param = *(p.add(16) as *const i64) as isize;
            let start = (wp as i32 + 1).max(0) as usize;
            with(|g| {
                let Some(l) = lv(g, h) else { return -1 };
                (start..l.items.len())
                    .find(|i| {
                        let it = &l.items[*i];
                        (flags & 1 != 0 && it.param == param) || (flags & 2 != 0 && it.texts.first().is_some_and(|t| t.eq_ignore_ascii_case(&text))) || (flags & 8 != 0 && it.texts.first().is_some_and(|t| t.to_lowercase().starts_with(&text.to_lowercase())))
                    })
                    .map_or(-1, |i| i as isize)
            })
        }
        // LVM_GETITEMRECT: the rectangle's left says which (0 bounds, 1 icon, 2 label, 3 select bounds)
        0x100E => with(|g| {
            if wp >= count(g, h) || lp == 0 {
                return 0;
            }
            let r = item_rect(g, h, wp);
            let o = lp as *mut i32;
            *o = r.l;
            *o.add(1) = r.t;
            *o.add(2) = r.r;
            *o.add(3) = r.b;
            1
        }),
        // LVM_GETSUBITEMRECT: wParam item, lParam RECT with top = sub-item
        0x1038 => with(|g| {
            if wp >= count(g, h) || lp == 0 {
                return 0;
            }
            let o = lp as *mut i32;
            let sub = *o.add(1) as usize;
            let r = item_rect(g, h, wp);
            let Some(l) = lv(g, h) else { return 0 };
            let x: i32 = l.columns.iter().take(sub).map(|c| c.width).sum::<i32>() - l.xoff;
            let w = l.columns.get(sub).map_or(r.width(), |c| c.width);
            *o = x;
            *o.add(1) = r.t;
            *o.add(2) = x + w;
            *o.add(3) = r.b;
            1
        }),
        // LVM_HITTEST / SUBITEMHITTEST: LVHITTESTINFO { POINT pt; UINT flags; int iItem; int iSubItem }
        0x1012 | 0x1039 => with(|g| {
            let p = lp as *mut i32;
            let (x, y) = (*p, *p.add(1));
            match item_at(g, h, x, y) {
                Some((i, sub)) => {
                    *(p.add(2) as *mut u32) = 0x0E; // LVHT_ONITEM
                    *p.add(3) = i as i32;
                    *p.add(4) = sub as i32;
                    i as isize
                }
                None => {
                    *(p.add(2) as *mut u32) = 1; // LVHT_NOWHERE
                    *p.add(3) = -1;
                    -1
                }
            }
        }),
        0x1013 => {
            with(|g| {
                ensure_visible(g, h, wp);
                g.invalidate(h, None, false);
            });
            1
        }
        0x1027 => with(|g| {
            let rh = row_h(g, h).max(1);
            lv(g, h).map_or(0, |l| (l.yoff / rh) as isize)
        }),
        0x1028 => with(|g| (body(g, h).height() / row_h(g, h).max(1)) as isize),
        0x1014 => with(|g| {
            let max = (content_size(g, h).1 - body(g, h).height()).max(0);
            if let Some(l) = lv_mut(g, h) {
                l.xoff = (l.xoff + wp as i32).max(0);
                l.yoff = (l.yoff + lp as i32).clamp(0, max);
            }
            sync_bars(g, h);
            g.invalidate(h, None, false);
            1
        }),
        0x1015 | 0x102A => {
            with(|g| g.invalidate(h, None, false));
            1
        }
        0x1011 | 0x1057 => {
            let text = str_arg(lp as usize, msg == 0x1057);
            with(|g| g.sized_font(font_of(g, h)).text_width(&text) as isize + 12)
        }
        // LVM_SETIMAGELIST / GETIMAGELIST (0 large, 1 small, 2 state)
        0x1003 => {
            let old = with(|g| {
                let idx = wp.min(2);
                let Some(l) = lv_mut(g, h) else { return 0 };
                let old = std::mem::replace(&mut l.images[idx], lp as usize);
                sync_bars(g, h);
                g.invalidate(h, None, true);
                old
            });
            old as isize
        }
        0x1002 => with(|g| lv(g, h).map_or(0, |l| l.images[wp.min(2)] as isize)),
        0x1000 => with(|g| lv(g, h).map_or(0, |l| l.bk as isize)),
        0x1001 => with(|g| {
            if let Some(l) = lv_mut(g, h) {
                l.bk = lp as u32;
            }
            g.invalidate(h, None, true);
            1
        }),
        0x1023 => with(|g| lv(g, h).map_or(0, |l| l.text_color as isize)),
        0x1024 => with(|g| {
            if let Some(l) = lv_mut(g, h) {
                l.text_color = lp as u32;
            }
            g.invalidate(h, None, true);
            1
        }),
        0x1025 => with(|g| lv(g, h).map_or(0, |l| l.text_bk as isize)),
        0x1026 => with(|g| {
            if let Some(l) = lv_mut(g, h) {
                l.text_bk = lp as u32;
            }
            g.invalidate(h, None, true);
            1
        }),
        // LVM_SETEXTENDEDLISTVIEWSTYLE: wParam is the mask (0 for all)
        0x1036 => with(|g| {
            let Some(l) = lv_mut(g, h) else { return 0 };
            let old = l.ext;
            let mask = if wp == 0 { u32::MAX } else { wp as u32 };
            l.ext = (l.ext & !mask) | (lp as u32 & mask);
            g.invalidate(h, None, true);
            old as isize
        }),
        0x1037 => with(|g| lv(g, h).map_or(0, |l| l.ext as isize)),
        // LVM_SETVIEW / GETVIEW (LV_VIEW_ICON 0, DETAILS 1, SMALLICON 2, LIST 3)
        0x108E => with(|g| {
            if let Some(w) = g.windows.get_mut(&h) {
                w.style = (w.style & !LVS_TYPEMASK) | (wp as u32 & 3);
            }
            sync_bars(g, h);
            g.invalidate(h, None, true);
            1
        }),
        0x108F => with(|g| view_of(g, h) as isize),
        // LVM_SORTITEMS: wParam lParamSort, lParam the compare function (item lParam, item lParam, lParamSort)
        0x1030 => {
            let cmp: unsafe extern "win64" fn(isize, isize, isize) -> i32 = std::mem::transmute(lp);
            let mut items = with(|g| lv(g, h).map(|l| l.items.clone())).unwrap_or_default();
            // a simple insertion sort calls the guest's comparison without holding the window system's lock
            for i in 1..items.len() {
                let mut j = i;
                while j > 0 && cmp(items[j - 1].param, items[j].param, wp as isize) > 0 {
                    items.swap(j - 1, j);
                    j -= 1;
                }
            }
            with(|g| {
                if let Some(l) = lv_mut(g, h) {
                    l.items = items;
                    l.focus = -1;
                }
                g.invalidate(h, None, true);
            });
            1
        }
        0x1018 | 0x1017 | 0x101F | 0x1016 | 0x1035 | 0x1033 | 0x1010 | 0x100F | 0x1031 | 0x100A | 0x100B | 0x1029 | 0x1022 | 0x103A | 0x103B | 0x103C | 0x1034 => 0,
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notification_body_has_the_documented_layout() {
        let b = nm_body(3, 1, 2, 0, LVIF_STATE, (10, 20), 99);
        assert_eq!(i32::from_le_bytes(b[0..4].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(b[20..24].try_into().unwrap()), 10);
        assert_eq!(i64::from_le_bytes(b[32..40].try_into().unwrap()), 99);
        assert_eq!(b.len(), 48);
    }

    #[test]
    fn lvitem_fields_come_from_the_mask() {
        let mut raw = vec![0u8; 88];
        raw[0..4].copy_from_slice(&(LVIF_PARAM | LVIF_IMAGE | LVIF_STATE).to_le_bytes());
        raw[12..16].copy_from_slice(&2u32.to_le_bytes());
        raw[16..20].copy_from_slice(&3u32.to_le_bytes());
        raw[36..40].copy_from_slice(&4i32.to_le_bytes());
        raw[40..48].copy_from_slice(&123i64.to_le_bytes());
        let mut it = Item { state: 1, image: -1, ..Default::default() };
        unsafe { apply_lvitem(&mut it, raw.as_ptr(), true) };
        assert_eq!((it.image, it.param, it.state), (4, 123, 2));
    }
}
