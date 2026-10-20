//! The rebar (`ReBarWindow32`): a container that lays child windows (usually tool bars) out in bands, left to right,
//! wrapping into rows. It sets each band's child to the band's area and reports its own height, which changes with
//! the band layout; the frame places the rebar itself. Bands keep the sizes the program gave them, the last band of
//! a row takes what is left of the row.

use super::api;
use super::controls::Ctl;
use super::ctl_basic::notify_with;
use super::gdi::sys_color;
use super::msg::def_window_proc;
use super::surface::*;
use super::wm::Class;
use super::*;

const RBBIM_STYLE: u32 = 1;
const RBBIM_COLORS: u32 = 2;
const RBBIM_TEXT: u32 = 4;
const RBBIM_IMAGE: u32 = 8;
const RBBIM_CHILD: u32 = 0x10;
const RBBIM_CHILDSIZE: u32 = 0x20;
const RBBIM_SIZE: u32 = 0x40;
const RBBIM_BACKGROUND: u32 = 0x80;
const RBBIM_ID: u32 = 0x100;
const RBBIM_IDEALSIZE: u32 = 0x200;
const RBBIM_LPARAM: u32 = 0x400;
const RBBIM_HEADERSIZE: u32 = 0x800;

const RBBS_BREAK: u32 = 1;
const RBBS_FIXEDSIZE: u32 = 2;
const RBBS_HIDDEN: u32 = 8;
const RBBS_GRIPPERALWAYS: u32 = 0x80;
const RBBS_NOGRIPPER: u32 = 0x100;

const RBS_BANDBORDERS: u32 = 0x400;
const CCS_NORESIZE: u32 = 4;
const CCS_NOPARENTALIGN: u32 = 8;
const CCS_NODIVIDER: u32 = 0x40;

const RBN_HEIGHTCHANGE: i32 = -831;
const RBN_DELETINGBAND: i32 = -837;

const GRIPPER: i32 = 10;

#[derive(Clone, Debug, Default)]
pub struct Band {
    pub style: u32,
    pub fore: u32,
    pub back: u32,
    pub text: String,
    pub image: i32,
    pub child: usize,
    pub min_w: i32,
    pub min_h: i32,
    pub cx: i32,
    pub bitmap: usize,
    pub id: u32,
    pub child_h: i32,
    pub max_h: i32,
    pub integral: i32,
    pub ideal: i32,
    pub lparam: isize,
    pub header: i32,
    /// Where the last layout put the band, in the rebar's client coordinates.
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct Rebar {
    pub bands: Vec<Band>,
    pub bk: u32,
    pub fg: u32,
    pub rows: i32,
    pub height: i32,
}

impl Default for Rebar {
    fn default() -> Rebar {
        Rebar { bands: Vec::new(), bk: 0xff00_0000, fg: 0xff00_0000, rows: 0, height: 0 }
    }
}

fn rb(g: &Gui, h: usize) -> Option<&Rebar> {
    match &g.windows.get(&h)?.ctl {
        Ctl::Rebar(r) => Some(r),
        _ => None,
    }
}

fn rb_mut(g: &mut Gui, h: usize) -> Option<&mut Rebar> {
    match &mut g.windows.get_mut(&h)?.ctl {
        Ctl::Rebar(r) => Some(r),
        _ => None,
    }
}

pub fn register_class(g: &mut Gui) {
    g.register_class(Class { name: "ReBarWindow32".into(), atom: 0, style: 0x8, wndproc: rebar_proc as *const () as usize, cls_extra: 0, wnd_extra: 0, hinstance: 0, icon: 0, cursor: 0, background: 0, menu_name: None, unicode: true, native: true });
}

fn header_width(style: u32, bar_style: u32, b: &Band) -> i32 {
    if b.header > 0 {
        return b.header;
    }
    let gripper = style & RBBS_NOGRIPPER == 0 && (style & RBBS_FIXEDSIZE == 0 || style & RBBS_GRIPPERALWAYS != 0);
    let _ = bar_style;
    if gripper { GRIPPER } else { 0 }
}

/// Places every band and returns (row count, total height). Pure: it only reads the bands and the width.
pub fn layout_bands(bands: &mut [Band], bar_style: u32, width: i32) -> (i32, i32) {
    let divider = if bar_style & CCS_NODIVIDER == 0 { 2 } else { 0 };
    let border = if bar_style & RBS_BANDBORDERS != 0 { 1 } else { 0 };
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut used = 0;
    for (i, b) in bands.iter().enumerate() {
        if b.style & RBBS_HIDDEN != 0 {
            continue;
        }
        let w = b.cx.max(b.min_w + header_width(b.style, bar_style, b));
        let new_row = rows.is_empty() || b.style & RBBS_BREAK != 0 || (used + w > width && used > 0 && b.min_w + header_width(b.style, bar_style, b) + used > width);
        if new_row {
            rows.push(Vec::new());
            used = 0;
        }
        rows.last_mut().unwrap().push(i);
        used += w;
    }
    let mut y = divider;
    for (ri, row) in rows.iter().enumerate() {
        let row_h = row.iter().map(|i| bands[*i].min_h.max(bands[*i].child_h).max(0)).max().unwrap_or(0) + border * 2;
        let mut x = 0;
        for (k, i) in row.iter().enumerate() {
            let last = k + 1 == row.len();
            let b = &mut bands[*i];
            let hw = header_width(b.style, bar_style, b);
            let own = b.cx.max(b.min_w + hw);
            let w = if last { (width - x).max(b.min_w + hw) } else { own };
            b.rect = Rect::new(x, y, x + w, y + row_h);
            x += w;
        }
        y += row_h;
        let _ = ri;
    }
    (rows.len() as i32, y)
}

/// Lays the bands out for the rebar's current width, moves the children into place and tells the parent when the
/// height changed.
unsafe fn relayout(h: usize, notify_height: bool) {
    let Some((style, cw, old_h)) = with(|g| g.windows.get(&h).map(|w| (w.style, w.client_size().0, rb(g, h).map_or(0, |r| r.height)))) else { return };
    let (children, rows, height) = with(|g| {
        let Some(r) = rb_mut(g, h) else { return (Vec::new(), 0, 0) };
        let (rows, height) = layout_bands(&mut r.bands, style, cw);
        r.rows = rows;
        r.height = height;
        let hw: Vec<(usize, Rect, u32, i32)> = r.bands.iter().map(|b| (b.child, b.rect, b.style, header_width(b.style, style, b))).collect();
        (hw, rows, height)
    });
    let _ = rows;
    for (child, rect, bstyle, header) in children {
        if child == 0 {
            continue;
        }
        if bstyle & RBBS_HIDDEN != 0 {
            api::show_window(child, 0);
            continue;
        }
        let border = if style & RBS_BANDBORDERS != 0 { 1 } else { 0 };
        let (x, y) = (rect.l + header, rect.t + border);
        let (w, hh) = ((rect.width() - header).max(0), (rect.height() - 2 * border).max(0));
        api::set_window_pos(child, x, y, w, hh, 0x4 | 0x10 | 0x40);
    }
    // unless told not to (CCS_NORESIZE), the rebar is as high as its bands; the program asks the window for the height
    let (rect, nowrap) = with(|g| g.windows.get(&h).map_or((Rect::default(), true), |w| (w.rect, w.style & CCS_NORESIZE != 0)));
    if !nowrap && rect.height() != height {
        api::set_window_pos(h, rect.l, rect.t, rect.width(), height, 0x4 | 0x10);
    }
    if notify_height && height != old_h {
        notify_with(h, RBN_HEIGHTCHANGE, &[]);
    }
}

/// Sizes the rebar to the parent's width and its own height, unless the program places it.
unsafe fn auto_size(h: usize) {
    let (style, parent, rect, height) = with(|g| g.windows.get(&h).map_or((0, 0, Rect::default(), 0), |w| (w.style, w.parent, w.rect, rb(g, h).map_or(0, |r| r.height))));
    if style & (CCS_NORESIZE | CCS_NOPARENTALIGN) != 0 || parent == 0 {
        return;
    }
    let (pw, _) = with(|g| g.windows.get(&parent).map_or((0, 0), |w| w.client_size()));
    if rect.l != 0 || rect.t != 0 || rect.width() != pw || rect.height() != height {
        api::set_window_pos(h, 0, 0, pw, height, 0x4 | 0x10);
    }
}

unsafe fn read_band(info: *const u8, wide: bool) -> (u32, Band) {
    let u32_at = |o: usize| *(info.add(o) as *const u32);
    let mask = u32_at(4);
    let mut b = Band { image: -1, ..Default::default() };
    if mask & RBBIM_STYLE != 0 {
        b.style = u32_at(8);
    }
    if mask & RBBIM_COLORS != 0 {
        b.fore = u32_at(12);
        b.back = u32_at(16);
    }
    if mask & RBBIM_TEXT != 0 {
        let p = *(info.add(24) as *const usize);
        if p != 0 {
            b.text = if wide { crate::runtime::strings::w_ptr_to_string(p as *const u16) } else { crate::runtime::strings::a_to_string(p as *const u8) };
        }
    }
    if mask & RBBIM_IMAGE != 0 {
        b.image = u32_at(36) as i32;
    }
    if mask & RBBIM_CHILD != 0 {
        b.child = *(info.add(40) as *const usize);
    }
    if mask & RBBIM_CHILDSIZE != 0 {
        b.min_w = u32_at(48) as i32;
        b.min_h = u32_at(52) as i32;
        let cb = u32_at(0) as usize;
        if cb >= 88 {
            b.child_h = u32_at(76) as i32;
            b.max_h = u32_at(80) as i32;
            b.integral = u32_at(84) as i32;
        }
    }
    if mask & RBBIM_SIZE != 0 {
        b.cx = u32_at(56) as i32;
    }
    if mask & RBBIM_BACKGROUND != 0 {
        b.bitmap = *(info.add(64) as *const usize);
    }
    if mask & RBBIM_ID != 0 {
        b.id = u32_at(72);
    }
    if mask & RBBIM_IDEALSIZE != 0 && u32_at(0) >= 96 {
        b.ideal = u32_at(88) as i32;
    }
    if mask & RBBIM_LPARAM != 0 && u32_at(0) >= 104 {
        b.lparam = *(info.add(96) as *const isize);
    }
    if mask & RBBIM_HEADERSIZE != 0 && u32_at(0) >= 108 {
        b.header = u32_at(104) as i32;
    }
    (mask, b)
}

/// Copies the fields `mask` selects from a stored band into the program's structure.
unsafe fn write_band(info: *mut u8, b: &Band, wide: bool) {
    let mask = *(info.add(4) as *const u32);
    let put = |o: usize, v: u32| *(info.add(o) as *mut u32) = v;
    if mask & RBBIM_STYLE != 0 {
        put(8, b.style);
    }
    if mask & RBBIM_COLORS != 0 {
        put(12, b.fore);
        put(16, b.back);
    }
    if mask & RBBIM_TEXT != 0 {
        let p = *(info.add(24) as *const usize);
        let cch = *(info.add(32) as *const u32);
        if p != 0 && cch > 0 {
            if wide {
                crate::runtime::strings::return_w(p as *mut u16, cch, &b.text);
            } else {
                crate::runtime::strings::return_a(p as *mut u8, cch, &b.text);
            }
        }
    }
    if mask & RBBIM_IMAGE != 0 {
        put(36, b.image as u32);
    }
    if mask & RBBIM_CHILD != 0 {
        *(info.add(40) as *mut usize) = b.child;
    }
    if mask & RBBIM_CHILDSIZE != 0 {
        put(48, b.min_w as u32);
        put(52, b.min_h as u32);
        if *(info as *const u32) >= 88 {
            put(76, b.child_h as u32);
            put(80, b.max_h as u32);
            put(84, b.integral as u32);
        }
    }
    if mask & RBBIM_SIZE != 0 {
        put(56, b.cx as u32);
    }
    if mask & RBBIM_ID != 0 {
        put(72, b.id);
    }
    if mask & RBBIM_IDEALSIZE != 0 && *(info as *const u32) >= 96 {
        put(88, b.ideal as u32);
    }
    if mask & RBBIM_LPARAM != 0 && *(info as *const u32) >= 104 {
        *(info.add(96) as *mut isize) = b.lparam;
    }
    if mask & RBBIM_HEADERSIZE != 0 && *(info as *const u32) >= 108 {
        put(104, b.header as u32);
    }
}

/// Merges the fields `mask` selects of `new` into `old`.
fn merge_band(old: &mut Band, mask: u32, new: Band) {
    if mask & RBBIM_STYLE != 0 {
        old.style = new.style;
    }
    if mask & RBBIM_COLORS != 0 {
        old.fore = new.fore;
        old.back = new.back;
    }
    if mask & RBBIM_TEXT != 0 {
        old.text = new.text;
    }
    if mask & RBBIM_IMAGE != 0 {
        old.image = new.image;
    }
    if mask & RBBIM_CHILD != 0 {
        old.child = new.child;
    }
    if mask & RBBIM_CHILDSIZE != 0 {
        old.min_w = new.min_w;
        old.min_h = new.min_h;
        old.child_h = new.child_h;
        old.max_h = new.max_h;
        old.integral = new.integral;
    }
    if mask & RBBIM_SIZE != 0 {
        old.cx = new.cx;
    }
    if mask & RBBIM_BACKGROUND != 0 {
        old.bitmap = new.bitmap;
    }
    if mask & RBBIM_ID != 0 {
        old.id = new.id;
    }
    if mask & RBBIM_IDEALSIZE != 0 {
        old.ideal = new.ideal;
    }
    if mask & RBBIM_LPARAM != 0 {
        old.lparam = new.lparam;
    }
    if mask & RBBIM_HEADERSIZE != 0 {
        old.header = new.header;
    }
}

/// A child of a band belongs to the rebar: it moves with it and is clipped to it.
fn adopt(g: &mut Gui, rebar: usize, child: usize) {
    if child == 0 {
        return;
    }
    let Some(old) = g.windows.get(&child).map(|w| w.parent) else { return };
    if old == rebar {
        return;
    }
    if let Some(p) = g.windows.get_mut(&old) {
        p.children.retain(|c| *c != child);
    }
    if let Some(w) = g.windows.get_mut(&child) {
        w.parent = rebar;
    }
    if let Some(p) = g.windows.get_mut(&rebar) {
        p.children.push(child);
    }
}

unsafe fn paint(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    with(|g| draw(g, h, hdc));
    api::end_paint(h, ps.as_ptr());
}

fn draw(g: &mut Gui, h: usize, hdc: usize) {
    let Some(r) = rb(g, h).cloned() else { return };
    let Some((style, (cw, ch))) = g.windows.get(&h).map(|w| (w.style, w.client_size())) else { return };
    let face = if r.bk & 0xff00_0000 != 0 { sys_color(15) } else { r.bk & 0x00ff_ffff };
    g.fill_rect(hdc, Rect::new(0, 0, cw, ch), &Brush::Solid(face));
    g.draw(hdc, |cv, _| {
        if style & CCS_NODIVIDER == 0 {
            cv.fill_rect(Rect::new(0, 0, cw, 1), sys_color(16));
            cv.fill_rect(Rect::new(0, 1, cw, 2), sys_color(20));
        }
        for (i, b) in r.bands.iter().enumerate() {
            if b.style & RBBS_HIDDEN != 0 {
                continue;
            }
            let bg = if b.back & 0xff00_0000 != 0 { None } else { Some(b.back & 0x00ff_ffff) };
            if let Some(c) = bg {
                cv.fill_rect(b.rect, c);
            }
            let hw = header_width(b.style, style, b);
            if hw > 0 && b.style & RBBS_FIXEDSIZE == 0 || b.style & RBBS_GRIPPERALWAYS != 0 {
                // the gripper: a raised double bar at the left of the band
                let (x, t, bt) = (b.rect.l + 2, b.rect.t + 2, b.rect.b - 2);
                if bt > t {
                    cv.fill_rect(Rect::new(x, t, x + 1, bt), sys_color(20));
                    cv.fill_rect(Rect::new(x + 1, t, x + 2, bt), sys_color(16));
                    cv.fill_rect(Rect::new(x + 3, t, x + 4, bt), sys_color(20));
                    cv.fill_rect(Rect::new(x + 4, t, x + 5, bt), sys_color(16));
                }
            }
            if style & RBS_BANDBORDERS != 0 && b.rect.t > 2 && b.rect.l == 0 && i > 0 {
                cv.fill_rect(Rect::new(0, b.rect.t, cw, b.rect.t + 1), sys_color(16));
            }
        }
    });
}

pub unsafe extern "win64" fn rebar_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::Rebar(Box::default());
                }
            });
            1
        }
        WM_CREATE => {
            relayout(h, false);
            auto_size(h);
            0
        }
        WM_SIZE => {
            relayout(h, true);
            with(|g| g.invalidate(h, None, true));
            0
        }
        WM_PAINT => {
            paint(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETFONT => 0,
        // RB_INSERTBANDA / RB_INSERTBANDW: the band goes at index wp, -1 appends
        0x0401 | 0x040A => {
            let (mask, band) = read_band(lp as *const u8, msg == 0x040A);
            if crate::runtime::cfg().trace.api {
                eprintln!("nle[gui]: rebar {h:#x} insert band {wp} mask={mask:#x} {band:?}");
            }
            let child = band.child;
            let ok = with(|g| {
                let Some(r) = rb_mut(g, h) else { return false };
                let at = if wp as i32 == -1 || wp >= r.bands.len() { r.bands.len() } else { wp };
                r.bands.insert(at, band);
                adopt(g, h, child);
                true
            });
            if ok {
                relayout(h, true);
                with(|g| g.invalidate(h, None, true));
            }
            ok as isize
        }
        0x0402 => {
            let child = with(|g| {
                let r = rb_mut(g, h)?;
                (wp < r.bands.len()).then(|| r.bands.remove(wp).child)
            });
            match child {
                Some(_) => {
                    notify_with(h, RBN_DELETINGBAND, &(wp as u32).to_le_bytes());
                    relayout(h, true);
                    with(|g| g.invalidate(h, None, true));
                    1
                }
                None => 0,
            }
        }
        // RB_SETBANDINFOA / W
        0x0406 | 0x040B => {
            let (mask, band) = read_band(lp as *const u8, msg == 0x040B);
            let child = if mask & RBBIM_CHILD != 0 { band.child } else { 0 };
            let ok = with(|g| {
                let Some(r) = rb_mut(g, h) else { return false };
                let Some(old) = r.bands.get_mut(wp) else { return false };
                merge_band(old, mask, band);
                adopt(g, h, child);
                true
            });
            if ok {
                relayout(h, true);
                with(|g| g.invalidate(h, None, true));
            }
            ok as isize
        }
        // RB_GETBANDINFOW / A
        0x041C | 0x041D => {
            let band = with(|g| rb(g, h).and_then(|r| r.bands.get(wp).cloned()));
            match band {
                Some(b) => {
                    write_band(lp as *mut u8, &b, msg == 0x041C);
                    1
                }
                None => 0,
            }
        }
        0x040C => with(|g| rb(g, h).map_or(0, |r| r.bands.len())) as isize,
        0x040D => with(|g| rb(g, h).map_or(0, |r| r.rows)) as isize,
        0x040E => with(|g| rb(g, h).and_then(|r| r.bands.get(wp)).map_or(0, |b| b.rect.height())) as isize,
        0x041B => with(|g| rb(g, h).map_or(0, |r| r.height)) as isize,
        // RB_IDTOINDEX
        0x0410 => with(|g| rb(g, h).and_then(|r| r.bands.iter().position(|b| b.id == wp as u32)).map_or(-1, |i| i as isize)),
        // RB_GETRECT
        0x0409 => {
            let r = with(|g| rb(g, h).and_then(|r| r.bands.get(wp)).map(|b| b.rect));
            match r {
                Some(rect) if lp != 0 => {
                    let p = lp as *mut i32;
                    *p = rect.l;
                    *p.add(1) = rect.t;
                    *p.add(2) = rect.r;
                    *p.add(3) = rect.b;
                    1
                }
                _ => 0,
            }
        }
        // RB_SETBKCOLOR / RB_GETBKCOLOR / RB_SETTEXTCOLOR / RB_GETTEXTCOLOR
        0x0413 => {
            let old = with(|g| {
                let r = rb_mut(g, h)?;
                let old = r.bk;
                r.bk = lp as u32;
                g.invalidate(h, None, true);
                Some(old)
            });
            old.unwrap_or(0) as isize
        }
        0x0414 => with(|g| rb(g, h).map_or(0, |r| r.bk)) as isize,
        0x0415 => {
            let old = with(|g| rb_mut(g, h).map(|r| std::mem::replace(&mut r.fg, lp as u32)));
            old.unwrap_or(0) as isize
        }
        0x0416 => with(|g| rb(g, h).map_or(0, |r| r.fg)) as isize,
        // RB_SHOWBAND
        0x0423 => {
            let ok = with(|g| {
                let Some(r) = rb_mut(g, h) else { return false };
                let Some(b) = r.bands.get_mut(wp) else { return false };
                if lp != 0 {
                    b.style &= !RBBS_HIDDEN;
                } else {
                    b.style |= RBBS_HIDDEN;
                }
                true
            });
            if ok {
                relayout(h, true);
                with(|g| g.invalidate(h, None, true));
            }
            ok as isize
        }
        // RB_SIZETORECT: the rebar takes the rectangle's size
        0x0417 => {
            if lp != 0 {
                let p = lp as *const i32;
                let (l, t, r, b) = (*p, *p.add(1), *p.add(2), *p.add(3));
                api::set_window_pos(h, l, t, r - l, b - t, 0x4 | 0x10);
            }
            1
        }
        // RB_HITTEST: RBHITTESTINFO is a point, flags and a band index
        0x0408 => {
            let p = lp as *mut i32;
            let (x, y) = (*p, *p.add(1));
            let hit = with(|g| rb(g, h).and_then(|r| r.bands.iter().position(|b| b.style & RBBS_HIDDEN == 0 && b.rect.contains(x, y))));
            match hit {
                Some(i) => {
                    *p.add(2) = 1; // RBHT_CLIENT
                    *p.add(3) = i as i32;
                    i as isize
                }
                None => {
                    *p.add(2) = 1;
                    *p.add(3) = -1;
                    -1
                }
            }
        }
        // RB_GETBANDBORDERS: left of the header, top and bottom edge
        0x0422 => {
            if lp != 0 {
                let p = lp as *mut i32;
                *p = 0;
                *p.add(1) = 0;
                *p.add(2) = 0;
                *p.add(3) = 0;
            }
            0
        }
        // RB_MOVEBAND
        0x0427 => {
            let ok = with(|g| {
                let Some(r) = rb_mut(g, h) else { return false };
                let to = lp as usize;
                if wp >= r.bands.len() || to >= r.bands.len() {
                    return false;
                }
                let b = r.bands.remove(wp);
                r.bands.insert(to, b);
                true
            });
            if ok {
                relayout(h, true);
                with(|g| g.invalidate(h, None, true));
            }
            ok as isize
        }
        // RB_SETPARENT, RB_GETTOOLTIPS, RB_SETTOOLTIPS, RB_GETBARINFO, RB_SETBARINFO, RB_GETEXTENDEDSTYLE ...
        0x0407 | 0x0411 | 0x0412 | 0x0403 | 0x0404 | 0x042A | 0x0429 | 0x0428 => 0,
        // RB_MINIMIZEBAND / RB_MAXIMIZEBAND
        0x041E | 0x041F => 0,
        // CCM_SETUNICODEFORMAT / GETUNICODEFORMAT
        0x2005 => 1,
        0x2006 => 1,
        WM_DESTROY => 0,
        _ => def_window_proc(h, msg, wp, lp, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(min_w: i32, min_h: i32, cx: i32, style: u32) -> Band {
        Band { min_w, min_h, cx, style, image: -1, ..Default::default() }
    }

    #[test]
    fn one_band_fills_the_row() {
        let mut bands = vec![band(100, 28, 100, RBBS_NOGRIPPER)];
        let (rows, h) = layout_bands(&mut bands, CCS_NODIVIDER, 800);
        assert_eq!(rows, 1);
        assert_eq!(h, 28);
        assert_eq!(bands[0].rect, Rect::new(0, 0, 800, 28));
    }

    #[test]
    fn bands_sit_side_by_side_and_the_last_takes_the_rest() {
        let mut bands = vec![band(100, 28, 300, RBBS_NOGRIPPER), band(50, 24, 100, RBBS_NOGRIPPER)];
        let (rows, h) = layout_bands(&mut bands, CCS_NODIVIDER, 800);
        assert_eq!((rows, h), (1, 28));
        assert_eq!(bands[0].rect, Rect::new(0, 0, 300, 28));
        assert_eq!(bands[1].rect, Rect::new(300, 0, 800, 28));
    }

    #[test]
    fn a_break_starts_a_new_row() {
        let mut bands = vec![band(10, 20, 100, RBBS_NOGRIPPER), band(10, 30, 100, RBBS_NOGRIPPER | RBBS_BREAK)];
        let (rows, h) = layout_bands(&mut bands, CCS_NODIVIDER, 400);
        assert_eq!((rows, h), (2, 50));
        assert_eq!(bands[1].rect.t, 20);
    }

    #[test]
    fn a_gripper_takes_room_from_the_child() {
        let mut bands = vec![band(100, 28, 200, 0)];
        layout_bands(&mut bands, CCS_NODIVIDER, 800);
        assert_eq!(header_width(0, 0, &bands[0]), GRIPPER);
        assert_eq!(header_width(RBBS_NOGRIPPER, 0, &bands[0]), 0);
        assert_eq!(header_width(RBBS_FIXEDSIZE, 0, &bands[0]), 0);
    }

    #[test]
    fn hidden_bands_take_no_room() {
        let mut bands = vec![band(10, 20, 100, RBBS_NOGRIPPER | RBBS_HIDDEN), band(10, 30, 100, RBBS_NOGRIPPER)];
        let (rows, h) = layout_bands(&mut bands, CCS_NODIVIDER, 400);
        assert_eq!((rows, h), (1, 30));
    }

    #[test]
    fn the_divider_adds_height() {
        let mut bands = vec![band(10, 20, 100, RBBS_NOGRIPPER)];
        let (_, h) = layout_bands(&mut bands, 0, 400);
        assert_eq!(h, 22);
    }
}
