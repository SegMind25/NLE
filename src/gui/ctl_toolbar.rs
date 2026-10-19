//! The tool bar (`ToolbarWindow32`): a row of picture buttons, separators and check/radio/drop-down buttons. Programs add
//! buttons with `TB_ADDBUTTONS` and images with `TB_ADDBITMAP` or an image list; a click sends `WM_COMMAND` to the parent.
//! The standard images (`IDB_STD_SMALL_COLOR` and friends) are drawn here from small pixel maps.

use super::api;
use super::controls::{font_of, Ctl};
use super::ctl_basic::notify_with;
use super::gdi::{sys_color, Object};
use super::imagelist::ImageList;
use super::msg::{current_ansi, def_window_proc};
use super::surface::*;
use super::wm::Class;
use super::*;

const TBSTATE_CHECKED: u8 = 1;
const TBSTATE_PRESSED: u8 = 2;
const TBSTATE_ENABLED: u8 = 4;
const TBSTATE_HIDDEN: u8 = 8;
const TBSTATE_INDETERMINATE: u8 = 0x10;

const BTNS_SEP: u8 = 1;
const BTNS_CHECK: u8 = 2;
const BTNS_GROUP: u8 = 4;
const BTNS_DROPDOWN: u8 = 8;
const BTNS_SHOWTEXT: u8 = 0x40;
const BTNS_WHOLEDROPDOWN: u8 = 0x80;

const TBSTYLE_FLAT: u32 = 0x800;
const TBSTYLE_LIST: u32 = 0x1000;
const CCS_NORESIZE: u32 = 4;
const CCS_NOPARENTALIGN: u32 = 8;
const CCS_NODIVIDER: u32 = 0x40;
const CCS_BOTTOM: u32 = 3;

const TBN_DROPDOWN: i32 = -710;

#[derive(Clone, Debug, Default)]
pub struct Button {
    pub image: i32,
    pub id: i32,
    pub state: u8,
    pub style: u8,
    pub data: isize,
    pub text: String,
    /// Width of a separator (it is stored in `iBitmap`).
    pub sep_w: i32,
    pub width: i32,
}

#[derive(Clone, Debug)]
pub struct Toolbar {
    pub buttons: Vec<Button>,
    pub strings: Vec<String>,
    pub himl: usize,
    pub bitmap: (i32, i32),
    pub size: Option<(i32, i32)>,
    pub hot: i32,
    pub pressed: i32,
    pub ext: u32,
    pub padding: (i32, i32),
    /// Image lists the tool bar made for `TB_ADDBITMAP`, keyed by the bitmap's size.
    pub own_list: usize,
}

impl Default for Toolbar {
    fn default() -> Toolbar {
        Toolbar { buttons: Vec::new(), strings: Vec::new(), himl: 0, bitmap: (16, 15), size: None, hot: -1, pressed: -1, ext: 0, padding: (7, 6), own_list: 0 }
    }
}

fn tb(g: &Gui, h: usize) -> Option<&Toolbar> {
    match &g.windows.get(&h)?.ctl {
        Ctl::Toolbar(t) => Some(t),
        _ => None,
    }
}

fn tb_mut(g: &mut Gui, h: usize) -> Option<&mut Toolbar> {
    match &mut g.windows.get_mut(&h)?.ctl {
        Ctl::Toolbar(t) => Some(t),
        _ => None,
    }
}

pub fn register_class(g: &mut Gui) {
    g.register_class(Class { name: "ToolbarWindow32".into(), atom: 0, style: 0, wndproc: toolbar_proc as *const () as usize, cls_extra: 0, wnd_extra: 0, hinstance: 0, icon: 0, cursor: 0, background: 0, menu_name: None, unicode: true, native: true });
}

// ---- the standard image strips ------------------------------------------------------------------------------

/// 8x8 pixel maps for the standard images, in `STD_CUT` .. `STD_PRINT` order.
const GLYPHS: [[&str; 8]; 15] = [
    ["k.....k.", ".k...k..", "..k.k...", "...k....", "..k.k...", ".rr.rr..", ".r...r..", ".rr.rr.."],
    ["kkkkk...", "kwwwk...", "kwkkkkk.", "kwkwwwk.", "kkkwwwk.", "..kwwwk.", "..kwwwk.", "..kkkkk."],
    [".kkkkk..", "kyyyyyk.", "kykkkyk.", "kykwwwkk", "kykwwwwk", "kykwwwwk", "kyyywwwk", ".kkkkkkk"],
    ["...bb...", "..bbbbb.", ".bb...bb", "bbb....b", ".bb....b", "..b...bb", "......b.", ".....bb."],
    ["...bb...", ".bbbbb..", "bb...bb.", "b....bbb", "b....bb.", "bb...b..", ".b......", ".bb....."],
    ["rr....rr", "rrr..rrr", ".rrrrrr.", "..rrrr..", "..rrrr..", ".rrrrrr.", "rrr..rrr", "rr....rr"],
    [".kkkkk..", ".kwwwkk.", ".kwwwkwk", ".kwwwkkk", ".kwwwwwk", ".kwwwwwk", ".kwwwwwk", ".kkkkkkk"],
    ["........", "yyy.....", "yyyyyyy.", "ykkkkkkk", "ykyyyyyk", "ykyyyyyk", "ykyyyyyk", ".kkkkkkk"],
    ["kkkkkkk.", "kgwwwgkk", "kgwwwgkk", "kggggggk", "kgkkkgkk", "kgkkkgkk", "kgggggkk", "kkkkkkkk"],
    ["kkkkk...", "kwwwk...", "kwwwkbb.", "kwwbbwbb", "kwwbwwwb", "kwwbbwbb", "kkkkkbb.", "......bb"],
    ["kkkkkkk.", "kwwwwwk.", "kwkkkwk.", "kwwwwwk.", "kwkkkkk.", "kwwwwwk.", "kwkkkwk.", "kkkkkkk."],
    ["..bbbb..", ".bb..bb.", ".....bb.", "....bb..", "...bb...", "...bb...", "........", "...bb..."],
    [".kkk....", "k...k...", "k...k...", "k...k...", ".kkk....", "..kkk...", "....kk..", ".....kk."],
    ["bbbb....", "....b...", "...b....", "bbbb....", "....bbbb", "...b....", "..b.....", "....bbbb"],
    ["..kkkk..", "..kwwk..", "kkkkkkkk", "kgggggkk", "kkkkkkkk", ".kwwwwk.", ".kwwwwk.", ".kkkkkk."],
];

fn glyph_color(c: char) -> Option<u32> {
    Some(match c {
        'k' => 0x303030,
        'w' => 0xFFFFFF,
        'g' => 0x909090,
        'b' => 0x2060C0,
        'r' => 0xC82020,
        'y' => 0xE6BC3C,
        'G' => 0x30A030,
        'l' => 0x9CC8F0,
        'o' => 0xD08020,
        _ => return None,
    })
}

/// The standard images at the given size (a multiple of 8 pixels is crisp).
pub fn standard_images(size: i32) -> ImageList {
    let mut list = ImageList::new(size, size, 0x20);
    let scale = (size / 8).max(1);
    for rows in GLYPHS.iter() {
        let mut rgb = vec![0u32; (size * size) as usize];
        let mut alpha = vec![0u8; (size * size) as usize];
        for (gy, row) in rows.iter().enumerate() {
            for (gx, ch) in row.chars().enumerate() {
                let Some(c) = glyph_color(ch) else { continue };
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (x, y) = (gx as i32 * scale + dx, gy as i32 * scale + dy);
                        if x < size && y < size {
                            let k = (y * size + x) as usize;
                            rgb[k] = c;
                            alpha[k] = 255;
                        }
                    }
                }
            }
        }
        list.images.push((rgb, alpha));
    }
    list
}

// ---- layout ---------------------------------------------------------------------------------------------------

fn style_of(g: &Gui, h: usize) -> u32 {
    g.windows.get(&h).map_or(0, |w| w.style)
}

fn image_size(g: &Gui, t: &Toolbar) -> (i32, i32) {
    g.imagelist(t.himl).map_or(t.bitmap, |l| (l.w, l.h))
}

fn shows_text(style: u32, b: &Button) -> bool {
    !b.text.is_empty() && (style & TBSTYLE_LIST == 0 || b.style & BTNS_SHOWTEXT != 0)
}

fn button_size(g: &Gui, h: usize, t: &Toolbar, b: &Button) -> (i32, i32) {
    let style = style_of(g, h);
    let font = g.sized_font(font_of(g, h));
    let (iw, ih) = image_size(g, t);
    if b.style & BTNS_SEP != 0 {
        return (if b.sep_w > 0 { b.sep_w } else { 8 }, t.size.map_or(ih + t.padding.1, |s| s.1));
    }
    let any_text = t.buttons.iter().any(|x| shows_text(style, x));
    let (mut w, mut hh) = t.size.unwrap_or((iw + t.padding.0, ih + t.padding.1));
    if shows_text(style, b) {
        let tw = font.text_width(&b.text);
        if style & TBSTYLE_LIST != 0 {
            w = w.max(iw + tw + t.padding.0 + 4);
        } else {
            w = w.max(tw + 10);
        }
    }
    if any_text && style & TBSTYLE_LIST == 0 && t.size.is_none() {
        hh += font.height() + 2;
    }
    if b.style & BTNS_DROPDOWN != 0 && b.style & BTNS_WHOLEDROPDOWN == 0 {
        w += 14;
    }
    if b.width > 0 {
        w = b.width;
    }
    (w, hh)
}

fn top_of(style: u32) -> i32 {
    if style & CCS_NODIVIDER != 0 {
        0
    } else {
        2
    }
}

/// Button rectangles in client coordinates, in button order; hidden buttons get an empty rectangle.
fn layout(g: &Gui, h: usize) -> Vec<Rect> {
    let Some(t) = tb(g, h) else { return Vec::new() };
    let style = style_of(g, h);
    let top = top_of(style);
    let mut x = 0;
    t.buttons
        .iter()
        .map(|b| {
            if b.state & TBSTATE_HIDDEN != 0 {
                return Rect::default();
            }
            let (w, hh) = button_size(g, h, t, b);
            let r = Rect::new(x, top, x + w, top + hh);
            x += w;
            r
        })
        .collect()
}

fn wanted_height(g: &Gui, h: usize) -> i32 {
    let Some(t) = tb(g, h) else { return 0 };
    let style = style_of(g, h);
    let hh = t.buttons.iter().map(|b| button_size(g, h, t, b).1).max().unwrap_or_else(|| {
        let (_, ih) = image_size(g, t);
        t.size.map_or(ih + t.padding.1, |s| s.1)
    });
    top_of(style) + hh + if style & TBSTYLE_FLAT != 0 { 0 } else { 2 }
}

/// Puts the tool bar at the top (or bottom) of its parent unless the program asked for manual placement.
unsafe fn auto_size(h: usize) {
    let (style, parent, rect) = with(|g| g.windows.get(&h).map_or((0, 0, Rect::default()), |w| (w.style, w.parent, w.rect)));
    if style & (CCS_NORESIZE | CCS_NOPARENTALIGN) != 0 || parent == 0 {
        return;
    }
    let (pw, ph) = with(|g| g.windows.get(&parent).map_or((0, 0), |w| w.client_size()));
    let height = with(|g| wanted_height(g, h));
    let y = if style & 3 == CCS_BOTTOM { ph - height } else { 0 };
    if rect.l != 0 || rect.t != y || rect.width() != pw || rect.height() != height {
        api::set_window_pos(h, 0, y, pw, height, 0x4 | 0x10);
    }
}

fn index_of(t: &Toolbar, id: usize) -> Option<usize> {
    t.buttons.iter().position(|b| b.id == id as i32 && b.style & BTNS_SEP == 0)
}

// ---- painting -------------------------------------------------------------------------------------------------

unsafe fn paint_toolbar(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    with(|g| draw_toolbar(g, h, hdc));
    api::end_paint(h, ps.as_ptr());
}

fn draw_toolbar(g: &mut Gui, h: usize, hdc: usize) {
    let Some(t) = tb(g, h).cloned() else { return };
    let style = style_of(g, h);
    let flat = style & TBSTYLE_FLAT != 0;
    let (cw, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
    let f = font_of(g, h);
    g.select_object(hdc, f);
    let font = g.sized_font(f);
    let enabled_all = g.is_enabled(h);
    let face = sys_color(15);
    g.fill_rect(hdc, Rect::new(0, 0, cw, ch), &Brush::Solid(face));
    let rects = layout(g, h);
    let (iw, ih) = image_size(g, &t);
    let top = top_of(style);
    let mut pictures: Vec<(usize, i32, i32, bool)> = Vec::new();
    g.draw(hdc, |cv, _| {
        if top > 0 {
            cv.fill_rect(Rect::new(0, 0, cw, 1), 0xA0A0A0);
            cv.fill_rect(Rect::new(0, 1, cw, 2), 0xFFFFFF);
        }
        if !flat {
            cv.fill_rect(Rect::new(0, ch - 1, cw, ch), 0xA0A0A0);
        }
        for (i, (b, r)) in t.buttons.iter().zip(rects.iter()).enumerate() {
            if b.state & TBSTATE_HIDDEN != 0 {
                continue;
            }
            if b.style & BTNS_SEP != 0 {
                if b.sep_w == 0 || b.sep_w >= 6 {
                    let x = r.l + r.width() / 2;
                    cv.fill_rect(Rect::new(x - 1, r.t + 2, x, r.b - 2), 0xA0A0A0);
                    cv.fill_rect(Rect::new(x, r.t + 2, x + 1, r.b - 2), 0xFFFFFF);
                }
                continue;
            }
            let usable = enabled_all && b.state & TBSTATE_ENABLED != 0;
            let pressed = (t.pressed == i as i32 && t.hot == i as i32) || b.state & TBSTATE_PRESSED != 0;
            let checked = b.state & TBSTATE_CHECKED != 0;
            let hot = t.hot == i as i32 && usable && t.pressed < 0;
            let down = pressed || checked;
            if flat {
                if down && usable {
                    cv.fill_rect(*r, 0xCCE4F7);
                    cv.frame_rect(*r, 0x66AFE9, 1);
                } else if checked {
                    cv.fill_rect(*r, 0xE0E0E0);
                    cv.frame_rect(*r, 0xA0A0A0, 1);
                } else if hot {
                    cv.fill_rect(*r, 0xE5F3FF);
                    cv.frame_rect(*r, 0x99D1FF, 1);
                }
            } else if down {
                cv.fill_rect(*r, if checked && !pressed { 0xFFFFFF } else { face });
                cv.frame_rect(*r, 0x696969, 1);
                cv.fill_rect(Rect::new(r.l + 1, r.t + 1, r.r - 1, r.t + 2), 0xA0A0A0);
                cv.fill_rect(Rect::new(r.l + 1, r.t + 1, r.l + 2, r.b - 1), 0xA0A0A0);
            } else {
                cv.fill_rect(*r, face);
                cv.fill_rect(Rect::new(r.l, r.t, r.r - 1, r.t + 1), 0xFFFFFF);
                cv.fill_rect(Rect::new(r.l, r.t, r.l + 1, r.b - 1), 0xFFFFFF);
                cv.fill_rect(Rect::new(r.l, r.b - 1, r.r, r.b), 0x696969);
                cv.fill_rect(Rect::new(r.r - 1, r.t, r.r, r.b), 0x696969);
                cv.fill_rect(Rect::new(r.l + 1, r.b - 2, r.r - 1, r.b - 1), 0xA0A0A0);
                cv.fill_rect(Rect::new(r.r - 2, r.t + 1, r.r - 1, r.b - 1), 0xA0A0A0);
            }
            let shift = if down && !flat { 1 } else { 0 };
            let drop_w = if b.style & BTNS_DROPDOWN != 0 && b.style & BTNS_WHOLEDROPDOWN == 0 { 14 } else { 0 };
            let inner_w = r.width() - drop_w;
            let text = shows_text(style, b);
            let color = if usable { 0 } else { 0x838383 };
            if style & TBSTYLE_LIST != 0 {
                let mut x = r.l + t.padding.0 / 2 + shift;
                if b.image >= 0 {
                    pictures.push((b.image as usize, x, r.t + (r.height() - ih) / 2 + shift, !usable));
                    x += iw + 3;
                }
                if text {
                    font.draw(cv, x, r.t + (r.height() - font.height()) / 2 + shift, &b.text, color, None);
                }
            } else {
                let text_h = if text { font.height() + 2 } else { 0 };
                let img_top = r.t + (r.height() - ih - text_h) / 2 + shift;
                if b.image >= 0 {
                    pictures.push((b.image as usize, r.l + (inner_w - iw) / 2 + shift, img_top, !usable));
                }
                if text {
                    let tw = font.text_width(&b.text);
                    font.draw(cv, r.l + (inner_w - tw) / 2 + shift, img_top + ih + 2, &b.text, color, None);
                }
            }
            if drop_w > 0 {
                let (ax, ay) = (r.r - 8, r.t + r.height() / 2);
                for d in 0..3 {
                    cv.fill_rect(Rect::new(ax - 2 + d, ay - 1 + d, ax + 3 - d, ay + d), if usable { 0 } else { 0x838383 });
                }
            }
            if b.state & TBSTATE_INDETERMINATE != 0 {
                cv.fill_rect(Rect::new(r.l + 1, r.t + 1, r.r - 1, r.b - 1), 0xE0E0E0);
            }
        }
    });
    for (idx, x, y, dim) in pictures {
        g.draw_imagelist_mode(hdc, t.himl, idx, x, y, 0, 0, dim);
    }
}

// ---- buttons and messages ---------------------------------------------------------------------------------

/// Reads a TBBUTTON (iBitmap@0, idCommand@4, fsState@8, fsStyle@9, dwData@16, iString@24).
unsafe fn read_button(p: *const u8, strings: &[String], wide: bool) -> Button {
    let style = *p.add(9);
    let image = *(p as *const i32);
    let istring = *(p.add(24) as *const i64);
    let text = if istring >= 0 && (istring as usize) < strings.len() && istring < 0x10000 {
        strings[istring as usize].clone()
    } else if istring >= 0x10000 {
        if wide {
            crate::runtime::strings::w_ptr_to_string(istring as usize as *const u16)
        } else {
            crate::runtime::strings::a_to_string(istring as usize as *const u8)
        }
    } else {
        String::new()
    };
    Button {
        image: if style & BTNS_SEP != 0 { -1 } else { image },
        id: *(p.add(4) as *const i32),
        state: *p.add(8),
        style,
        data: *(p.add(16) as *const i64) as isize,
        text,
        sep_w: if style & BTNS_SEP != 0 { image } else { 0 },
        width: 0,
    }
}

unsafe fn write_button(p: *mut u8, b: &Button) {
    *(p as *mut i32) = if b.style & BTNS_SEP != 0 { b.sep_w } else { b.image };
    *(p.add(4) as *mut i32) = b.id;
    *p.add(8) = b.state;
    *p.add(9) = b.style;
    *(p.add(16) as *mut i64) = b.data as i64;
    *(p.add(24) as *mut i64) = -1;
}

fn push_buttons(g: &mut Gui, h: usize, at: Option<usize>, new: Vec<Button>) {
    if let Some(t) = tb_mut(g, h) {
        let at = at.unwrap_or(t.buttons.len()).min(t.buttons.len());
        for (k, b) in new.into_iter().enumerate() {
            t.buttons.insert(at + k, b);
        }
    }
    g.invalidate(h, None, true);
}

/// Works out which button (by index) a message names: by command id, or by position.
fn by_id(g: &Gui, h: usize, id: usize) -> Option<usize> {
    tb(g, h).and_then(|t| index_of(t, id))
}

fn set_flag(g: &mut Gui, h: usize, id: usize, flag: u8, on: bool) -> bool {
    let Some(i) = by_id(g, h, id) else { return false };
    if let Some(t) = tb_mut(g, h) {
        let b = &mut t.buttons[i];
        if on {
            b.state |= flag;
        } else {
            b.state &= !flag;
        }
    }
    g.invalidate(h, None, true);
    true
}

fn get_flag(g: &Gui, h: usize, id: usize, flag: u8) -> bool {
    by_id(g, h, id).and_then(|i| tb(g, h).map(|t| t.buttons[i].state & flag != 0)).unwrap_or(false)
}

/// A click that completed on button `i`: toggles check buttons, runs radio groups, tells the parent.
unsafe fn activate(h: usize, i: usize, x: i32) {
    let Some((b, parent, rect)) = with(|g| {
        let t = tb(g, h)?;
        let r = layout(g, h).get(i).copied()?;
        Some((t.buttons.get(i)?.clone(), g.windows.get(&h)?.parent, r))
    }) else {
        return;
    };
    if b.style & BTNS_SEP != 0 || b.state & TBSTATE_ENABLED == 0 {
        return;
    }
    if b.style & BTNS_DROPDOWN != 0 && (b.style & BTNS_WHOLEDROPDOWN != 0 || x >= rect.r - 14) {
        let mut extra = vec![0u8; 64];
        extra[0..4].copy_from_slice(&b.id.to_le_bytes());
        extra[56..60].copy_from_slice(&rect.l.to_le_bytes());
        extra[60..64].copy_from_slice(&rect.t.to_le_bytes());
        notify_with(h, TBN_DROPDOWN, &extra);
        if b.style & BTNS_WHOLEDROPDOWN != 0 || x >= rect.r - 14 {
            return;
        }
    }
    with(|g| {
        let Some(t) = tb_mut(g, h) else { return };
        if b.style & BTNS_CHECK != 0 {
            if b.style & BTNS_GROUP != 0 {
                // a radio group: the run of group buttons around this one
                let (mut lo, mut hi) = (i, i);
                while lo > 0 && t.buttons[lo - 1].style & BTNS_GROUP != 0 && t.buttons[lo - 1].style & BTNS_SEP == 0 {
                    lo -= 1;
                }
                while hi + 1 < t.buttons.len() && t.buttons[hi + 1].style & BTNS_GROUP != 0 && t.buttons[hi + 1].style & BTNS_SEP == 0 {
                    hi += 1;
                }
                for k in lo..=hi {
                    t.buttons[k].state &= !TBSTATE_CHECKED;
                }
                t.buttons[i].state |= TBSTATE_CHECKED;
            } else {
                t.buttons[i].state ^= TBSTATE_CHECKED;
            }
        }
        g.invalidate(h, None, false);
    });
    if parent != 0 {
        api::send(parent, WM_COMMAND, (b.id as u32 as usize) & 0xffff, h as isize);
    }
}

fn hit_button(g: &Gui, h: usize, x: i32, y: i32) -> Option<usize> {
    let t = tb(g, h)?;
    layout(g, h).iter().enumerate().find(|(i, r)| r.contains(x, y) && t.buttons[*i].style & BTNS_SEP == 0 && t.buttons[*i].state & TBSTATE_HIDDEN == 0).map(|(i, _)| i)
}

pub unsafe extern "win64" fn toolbar_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::Toolbar(Box::default());
                }
            });
            1
        }
        WM_CREATE => {
            auto_size(h);
            0
        }
        WM_SIZE => {
            auto_size(h);
            with(|g| g.invalidate(h, None, true));
            0
        }
        WM_PAINT => {
            paint_toolbar(h);
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
        WM_ENABLE => {
            with(|g| g.invalidate(h, None, true));
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if !with(|g| g.is_enabled(h)) {
                return 0;
            }
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            with(|g| {
                if let Some(i) = hit_button(g, h, x, y) {
                    if tb(g, h).is_some_and(|t| t.buttons[i].state & TBSTATE_ENABLED != 0) {
                        g.capture = h;
                        if let Some(t) = tb_mut(g, h) {
                            t.pressed = i as i32;
                            t.hot = i as i32;
                        }
                        g.invalidate(h, None, false);
                    }
                }
            });
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            with(|g| {
                let hot = hit_button(g, h, x, y).map_or(-1, |i| i as i32);
                if tb(g, h).is_some_and(|t| t.hot != hot) {
                    if let Some(t) = tb_mut(g, h) {
                        t.hot = hot;
                    }
                    g.invalidate(h, None, false);
                }
                g.track_leave(h);
            });
            0
        }
        WM_MOUSELEAVE => {
            with(|g| {
                if let Some(t) = tb_mut(g, h) {
                    if t.pressed < 0 {
                        t.hot = -1;
                    }
                }
                g.invalidate(h, None, false);
            });
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            let pressed = with(|g| {
                if g.capture == h {
                    g.capture = 0;
                }
                let p = tb(g, h).map_or(-1, |t| t.pressed);
                let over = hit_button(g, h, x, y);
                if let Some(t) = tb_mut(g, h) {
                    t.pressed = -1;
                    t.hot = over.map_or(-1, |i| i as i32);
                }
                g.invalidate(h, None, false);
                if p >= 0 && over == Some(p as usize) {
                    Some(p as usize)
                } else {
                    None
                }
            });
            if let Some(i) = pressed {
                activate(h, i, x);
            }
            0
        }
        // TB_ENABLEBUTTON / CHECKBUTTON / PRESSBUTTON / HIDEBUTTON / INDETERMINATE: wParam id, LOWORD(lParam) on or off
        0x401 => with(|g| set_flag(g, h, wp, TBSTATE_ENABLED, lp & 0xffff != 0)) as isize,
        0x402 => with(|g| set_flag(g, h, wp, TBSTATE_CHECKED, lp & 0xffff != 0)) as isize,
        0x403 => with(|g| set_flag(g, h, wp, TBSTATE_PRESSED, lp & 0xffff != 0)) as isize,
        0x404 => {
            let r = with(|g| set_flag(g, h, wp, TBSTATE_HIDDEN, lp & 0xffff != 0)) as isize;
            auto_size(h);
            r
        }
        0x405 => with(|g| set_flag(g, h, wp, TBSTATE_INDETERMINATE, lp & 0xffff != 0)) as isize,
        0x409 => with(|g| get_flag(g, h, wp, TBSTATE_ENABLED)) as isize,
        0x40A => with(|g| get_flag(g, h, wp, TBSTATE_CHECKED)) as isize,
        0x40B => with(|g| get_flag(g, h, wp, TBSTATE_PRESSED)) as isize,
        0x40C => with(|g| get_flag(g, h, wp, TBSTATE_HIDDEN)) as isize,
        0x40D => with(|g| get_flag(g, h, wp, TBSTATE_INDETERMINATE)) as isize,
        // TB_SETSTATE / TB_GETSTATE
        0x411 => with(|g| {
            let Some(i) = by_id(g, h, wp) else { return 0 };
            if let Some(t) = tb_mut(g, h) {
                t.buttons[i].state = lp as u8;
            }
            g.invalidate(h, None, true);
            1
        }),
        0x412 => with(|g| by_id(g, h, wp).and_then(|i| tb(g, h).map(|t| t.buttons[i].state as isize)).unwrap_or(-1)),
        // TB_ADDBITMAP: wParam the number of images; lParam TBADDBITMAP { HINSTANCE hInst; UINT_PTR nID }
        0x413 => {
            let p = lp as *const u64;
            let (inst, id) = (*p as i64, *p.add(1) as usize);
            let (w, hh) = with(|g| tb(g, h).map_or((16, 15), |t| t.bitmap));
            let (list, _) = with(|g| {
                // find or make the tool bar's own image list for this image size
                let own = tb(g, h).map_or(0, |t| t.own_list);
                let list = if own != 0 && g.imagelist(own).is_some_and(|l| l.w == w && l.h == hh) {
                    own
                } else {
                    let l = g.add_object(Object::ImageList(Box::new(ImageList::new(w, hh, 0x20))));
                    if let Some(t) = tb_mut(g, h) {
                        t.own_list = l;
                        t.himl = l;
                    }
                    l
                };
                (list, g.imagelist(list).map_or(0, |l| l.images.len()))
            });
            if inst == -1 {
                // HINST_COMMCTRL: IDB_STD_SMALL_COLOR 0, IDB_STD_LARGE_COLOR 1, IDB_VIEW_SMALL_COLOR 4, IDB_VIEW_LARGE_COLOR 5
                let std = standard_images(if id & 1 == 1 { 24 } else { 16 });
                with(|g| {
                    let size = if id & 1 == 1 { 24 } else { 16 };
                    // a tool bar showing standard images uses their size
                    let ids = g.add_object(Object::ImageList(Box::new(std)));
                    if let Some(t) = tb_mut(g, h) {
                        t.himl = ids;
                        t.bitmap = (size, size);
                        t.own_list = ids;
                    }
                });
                return 0;
            }
            let hbm = if inst == 0 {
                id
            } else {
                crate::shim::user32::load_bitmap(inst as usize, id, false)
            };
            with(|g| {
                let b = match g.objects.get(&hbm) {
                    Some(Object::Bitmap(b)) => Some(b.surf.resized(b.surf.w, b.surf.h, 0)),
                    _ => None,
                };
                let Some(b) = b else { return -1isize };
                // the toolbar maps the standard light grey to the button face
                match g.imagelist_mut(list) {
                    Some(l) => l.add_strip(&b, None, Some(0xC0C0C0)) as isize,
                    None => -1,
                }
            })
        }
        // TB_ADDBUTTONSA / W
        0x414 | 0x444 => {
            let n = wp;
            let strings = with(|g| tb(g, h).map_or(Vec::new(), |t| t.strings.clone()));
            let p = lp as *const u8;
            let new: Vec<Button> = (0..n).map(|k| read_button(p.add(k * 32), &strings, msg == 0x444)).collect();
            with(|g| push_buttons(g, h, None, new));
            auto_size(h);
            1
        }
        // TB_INSERTBUTTONA / W
        0x415 | 0x443 => {
            let strings = with(|g| tb(g, h).map_or(Vec::new(), |t| t.strings.clone()));
            let b = read_button(lp as *const u8, &strings, msg == 0x443);
            with(|g| push_buttons(g, h, Some(wp), vec![b]));
            auto_size(h);
            1
        }
        // TB_DELETEBUTTON
        0x416 => {
            let ok = with(|g| {
                let Some(t) = tb_mut(g, h) else { return false };
                if wp < t.buttons.len() {
                    t.buttons.remove(wp);
                    g.invalidate(h, None, true);
                    true
                } else {
                    false
                }
            });
            auto_size(h);
            ok as isize
        }
        // TB_GETBUTTON
        0x417 => with(|g| match tb(g, h).and_then(|t| t.buttons.get(wp)) {
            Some(b) => {
                write_button(lp as *mut u8, b);
                1
            }
            None => 0,
        }),
        0x418 => with(|g| tb(g, h).map_or(0, |t| t.buttons.len() as isize)),
        0x419 => with(|g| by_id(g, h, wp).map_or(-1, |i| i as isize)),
        // TB_ADDSTRINGA / W: wParam the instance (0 for a pointer), lParam strings separated by NULs and ended by two
        0x41C | 0x44D => {
            if wp != 0 {
                return -1;
            }
            let mut strings = Vec::new();
            if msg == 0x44D {
                let mut p = lp as *const u16;
                while *p != 0 {
                    let s = crate::runtime::strings::w_ptr_to_string(p);
                    let n = s.encode_utf16().count();
                    strings.push(s);
                    p = p.add(n + 1);
                }
            } else {
                let mut p = lp as *const u8;
                while *p != 0 {
                    let s = crate::runtime::strings::a_to_string(p);
                    let n = s.len();
                    p = p.add(n + 1);
                    strings.push(s);
                }
            }
            with(|g| {
                let Some(t) = tb_mut(g, h) else { return -1 };
                let first = t.strings.len() as isize;
                t.strings.extend(strings);
                first
            })
        }
        // TB_GETITEMRECT: wParam the index
        0x41D => with(|g| match layout(g, h).get(wp) {
            Some(r) if lp != 0 => {
                let o = lp as *mut i32;
                *o = r.l;
                *o.add(1) = r.t;
                *o.add(2) = r.r;
                *o.add(3) = r.b;
                1
            }
            _ => 0,
        }),
        // TB_GETRECT: wParam the command id
        0x433 => with(|g| {
            let Some(i) = by_id(g, h, wp) else { return 0 };
            let r = layout(g, h)[i];
            let o = lp as *mut i32;
            *o = r.l;
            *o.add(1) = r.t;
            *o.add(2) = r.r;
            *o.add(3) = r.b;
            1
        }),
        0x41E => 0,
        // TB_SETBUTTONSIZE / TB_SETBITMAPSIZE: LOWORD width, HIWORD height
        0x41F => with(|g| {
            if let Some(t) = tb_mut(g, h) {
                t.size = Some((lo_signed(lp), hi_signed(lp)));
            }
            g.invalidate(h, None, true);
            1
        }),
        0x420 => with(|g| {
            if let Some(t) = tb_mut(g, h) {
                t.bitmap = (lo_signed(lp), hi_signed(lp));
            }
            1
        }),
        0x43A => with(|g| {
            let Some(t) = tb(g, h) else { return 0 };
            let (iw, ih) = image_size(g, t);
            let (w, hh) = t.size.unwrap_or((iw + t.padding.0, ih + t.padding.1));
            ((hh as isize) << 16) | (w as isize & 0xffff)
        }),
        0x421 => {
            auto_size(h);
            with(|g| g.invalidate(h, None, true));
            0
        }
        // TB_SETIMAGELIST / TB_GETIMAGELIST
        0x430 => with(|g| {
            let Some(t) = tb_mut(g, h) else { return 0 };
            let old = std::mem::replace(&mut t.himl, lp as usize);
            g.invalidate(h, None, true);
            old as isize
        }),
        0x431 => with(|g| tb(g, h).map_or(0, |t| t.himl as isize)),
        0x434 | 0x436 | 0x435 | 0x437 => 0,
        // TB_SETBUTTONINFOA / W and TB_GETBUTTONINFOA / W: TBBUTTONINFO { cbSize; dwMask; idCommand; iImage; fsState; fsStyle; cx; lParam; pszText; cchText }
        0x440 | 0x442 | 0x43F | 0x441 => {
            let p = lp as *mut u8;
            let mask = *(p.add(4) as *const u32);
            let wide = msg == 0x440 || msg == 0x43F;
            let setting = msg == 0x440 || msg == 0x442;
            let idx = with(|g| if mask & 0x8000_0000 != 0 { Some(wp) } else { by_id(g, h, wp) });
            let Some(idx) = idx else { return -1 };
            with(|g| {
                let Some(b) = tb_mut(g, h).and_then(|t| t.buttons.get_mut(idx)) else { return -1 };
                if setting {
                    if mask & 0x20 != 0 {
                        b.id = *(p.add(8) as *const i32);
                    }
                    if mask & 1 != 0 {
                        b.image = *(p.add(12) as *const i32);
                    }
                    if mask & 4 != 0 {
                        b.state = *p.add(16);
                    }
                    if mask & 8 != 0 {
                        b.style = *p.add(17);
                    }
                    if mask & 0x40 != 0 {
                        b.width = *(p.add(18) as *const u16) as i32;
                    }
                    if mask & 0x10 != 0 {
                        b.data = *(p.add(24) as *const i64) as isize;
                    }
                    if mask & 2 != 0 {
                        let ptr = *(p.add(32) as *const u64) as usize;
                        b.text = if ptr == 0 {
                            String::new()
                        } else if wide {
                            crate::runtime::strings::w_ptr_to_string(ptr as *const u16)
                        } else {
                            crate::runtime::strings::a_to_string(ptr as *const u8)
                        };
                    }
                } else {
                    if mask & 0x20 != 0 {
                        *(p.add(8) as *mut i32) = b.id;
                    }
                    if mask & 1 != 0 {
                        *(p.add(12) as *mut i32) = b.image;
                    }
                    if mask & 4 != 0 {
                        *p.add(16) = b.state;
                    }
                    if mask & 8 != 0 {
                        *p.add(17) = b.style;
                    }
                    if mask & 0x10 != 0 {
                        *(p.add(24) as *mut i64) = b.data as i64;
                    }
                    if mask & 2 != 0 {
                        let ptr = *(p.add(32) as *const u64) as usize;
                        let max = *(p.add(40) as *const i32);
                        if ptr != 0 && max > 0 {
                            if wide {
                                crate::runtime::strings::return_w(ptr as *mut u16, max as u32, &b.text);
                            } else {
                                crate::runtime::strings::return_a(ptr as *mut u8, max as u32, &b.text);
                            }
                        }
                    }
                }
                g.invalidate(h, None, true);
                idx as isize
            })
        }
        // TB_HITTEST: lParam a POINT; the index, or the negative of the nearest separator's
        0x445 => with(|g| {
            let p = lp as *const i32;
            let (x, y) = (*p, *p.add(1));
            hit_button(g, h, x, y).map_or(-1, |i| i as isize)
        }),
        0x454 => with(|g| {
            let Some(t) = tb_mut(g, h) else { return 0 };
            let old = t.ext;
            let mask = if wp == 0 { u32::MAX } else { wp as u32 };
            t.ext = (t.ext & !mask) | (lp as u32 & mask);
            old as isize
        }),
        0x455 => with(|g| tb(g, h).map_or(0, |t| t.ext as isize)),
        0x456 => with(|g| tb(g, h).map_or(0, |t| ((t.padding.1 as isize) << 16) | (t.padding.0 as isize & 0xffff))),
        0x457 => with(|g| {
            let Some(t) = tb_mut(g, h) else { return 0 };
            let old = ((t.padding.1 as isize) << 16) | (t.padding.0 as isize & 0xffff);
            t.padding = (lo_signed(lp), hi_signed(lp));
            g.invalidate(h, None, true);
            old
        }),
        // TB_SETMAXTEXTROWS, TB_SETTOOLTIPS, TB_SETPARENT, TB_SETROWS, TB_SETINDENT, TB_SETDRAWTEXTFLAGS, TB_SETHOTITEM ...
        0x43C | 0x424 | 0x425 | 0x427 | 0x42F | 0x446 | 0x448 | 0x41A | 0x41B | 0x432 | 0x43B | 0x452 => 0,
        0x428 => 1,
        0x43D => 1,
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_images_fill_the_requested_size() {
        for size in [16, 24] {
            let l = standard_images(size);
            assert_eq!(l.images.len(), 15);
            assert!(l.images.iter().all(|(rgb, a)| rgb.len() == (size * size) as usize && a.len() == rgb.len()));
            assert!(l.images.iter().all(|(_, a)| a.iter().any(|v| *v == 255)), "every glyph draws something");
        }
    }

    #[test]
    fn a_button_is_read_from_a_tbbutton() {
        let mut raw = [0u8; 32];
        raw[0..4].copy_from_slice(&3i32.to_le_bytes());
        raw[4..8].copy_from_slice(&1001i32.to_le_bytes());
        raw[8] = TBSTATE_ENABLED;
        raw[9] = BTNS_CHECK;
        raw[24..32].copy_from_slice(&1i64.to_le_bytes());
        let strings = vec!["zero".to_string(), "one".to_string()];
        let b = unsafe { read_button(raw.as_ptr(), &strings, true) };
        assert_eq!((b.image, b.id, b.state, b.style, b.text.as_str()), (3, 1001, TBSTATE_ENABLED, BTNS_CHECK, "one"));
    }

    #[test]
    fn a_separator_keeps_its_width_not_an_image() {
        let mut raw = [0u8; 32];
        raw[0..4].copy_from_slice(&12i32.to_le_bytes());
        raw[9] = BTNS_SEP;
        raw[24..32].copy_from_slice(&(-1i64).to_le_bytes());
        let b = unsafe { read_button(raw.as_ptr(), &[], false) };
        assert_eq!((b.image, b.sep_w), (-1, 12));
    }
}
