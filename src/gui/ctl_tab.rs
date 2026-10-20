//! The tab control (`SysTabControl32`): a row of tabs above (or below) a body area. It keeps its
//! tabs and the selected one; what a tab shows is up to the program, which hears of a change
//! through `TCN_SELCHANGING` / `TCN_SELCHANGE`.

use super::api;
use super::controls::{font_of, Ctl};
use super::ctl_basic::{notify, NM_CLICK, NM_RCLICK};
use super::msg::{current_ansi, def_window_proc};
use super::surface::*;
use super::wm::Class;
use super::*;

const TCS_BOTTOM: u32 = 2;
const TCS_BUTTONS: u32 = 0x100;
const TCS_FIXEDWIDTH: u32 = 0x400;
const TCS_OWNERDRAWFIXED: u32 = 0x2000;
const TCN_SELCHANGE: i32 = -551;
const TCN_SELCHANGING: i32 = -552;
const TCIF_TEXT: u32 = 1;
const TCIF_IMAGE: u32 = 2;
const TCIF_PARAM: u32 = 8;

#[derive(Clone, Debug, Default)]
pub struct Tab {
    pub text: String,
    pub image: i32,
    pub param: isize,
}

#[derive(Clone, Debug, Default)]
pub struct TabControl {
    pub items: Vec<Tab>,
    pub cur: i32,
    pub focus: i32,
    pub images: usize,
    pub fixed: (i32, i32),
    pub pad: (i32, i32),
}

fn st(g: &Gui, h: usize) -> Option<&TabControl> {
    match &g.windows.get(&h)?.ctl {
        Ctl::Tab(t) => Some(t),
        _ => None,
    }
}

fn st_mut(g: &mut Gui, h: usize) -> Option<&mut TabControl> {
    match &mut g.windows.get_mut(&h)?.ctl {
        Ctl::Tab(t) => Some(t),
        _ => None,
    }
}

pub fn register_class(g: &mut Gui) {
    g.register_class(Class { name: "SysTabControl32".into(), atom: 0, style: 0, wndproc: tab_proc as *const () as usize, cls_extra: 0, wnd_extra: 0, hinstance: 0, icon: 0, cursor: 0, background: 0, menu_name: None, unicode: true, native: true });
}

fn tab_height(g: &Gui, h: usize) -> i32 {
    let t = st(g, h);
    let pad = t.map_or(3, |t| t.pad.1.max(3));
    let font = g.sized_font(font_of(g, h)).height();
    let icon = t.and_then(|t| g.imagelist(t.images)).map_or(0, |l| l.h);
    font.max(icon) + 2 * pad + 2
}

/// Rectangle of every tab, in client coordinates.
fn tab_rects(g: &Gui, h: usize) -> Vec<Rect> {
    let (Some(t), Some(w)) = (st(g, h), g.windows.get(&h)) else { return Vec::new() };
    let (cw, ch) = w.client_size();
    let th = tab_height(g, h);
    let font = g.sized_font(font_of(g, h));
    let bottom = w.style & TCS_BOTTOM != 0;
    let y0 = if bottom { ch - th } else { 0 };
    let icon_w = g.imagelist(t.images).map_or(0, |l| l.w + 4);
    let pad = t.pad.0.max(6);
    let mut x = 2;
    let mut out = Vec::new();
    for (i, it) in t.items.iter().enumerate() {
        let mut width = if w.style & TCS_FIXEDWIDTH != 0 && t.fixed.0 > 0 { t.fixed.0 } else { font.text_width(&it.text) + 2 * pad + if it.image >= 0 { icon_w } else { 0 } };
        width = width.max(24);
        let sel = t.cur == i as i32;
        // the selected tab stands a little taller
        let r = if bottom { Rect::new(x, y0 - if sel { 0 } else { 0 }, x + width, y0 + th) } else { Rect::new(x, if sel { 0 } else { 2 }, x + width, th) };
        out.push(r);
        x += width - if w.style & TCS_BUTTONS != 0 { -2 } else { 0 };
        if x > cw {
            // tabs that do not fit are cut off; a multi-line control is not modelled
        }
    }
    out
}

unsafe fn paint_tab(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    // owner-drawn tabs are painted by the parent: (index, rectangle, data, selected, focused)
    let mut owner: Vec<(usize, Rect, isize, bool, bool)> = Vec::new();
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return };
        let (cw, ch) = w.client_size();
        let style = w.style;
        let Some(t) = st(g, h).cloned() else { return };
        let rects = tab_rects(g, h);
        let th = tab_height(g, h);
        let bottom = style & TCS_BOTTOM != 0;
        let buttons = style & TCS_BUTTONS != 0;
        let focused = g.focus == h;
        let enabled = g.is_enabled(h);
        let owner_draw = style & TCS_OWNERDRAWFIXED != 0;
        if crate::runtime::cfg().trace.api {
            eprintln!("nle[gui]: paint tab {h:#x} style={style:#x} items={} cur={} rects={:?}", t.items.len(), t.cur, rects);
        }
        if owner_draw {
            owner = rects.iter().enumerate().map(|(i, r)| (i, *r, t.items[i].param, t.cur == i as i32, focused && t.focus == i as i32)).collect();
        }
        let f = font_of(g, h);
        g.select_object(hdc, f);
        g.fill_rect(hdc, Rect::new(0, 0, cw, ch), &Brush::Solid(super::gdi::sys_color(15)));
        let font = g.sized_font(f);
        let list = g.imagelist(t.images).cloned();
        let body = if bottom { Rect::new(0, 0, cw, ch - th + 1) } else { Rect::new(0, th - 1, cw, ch) };
        g.draw(hdc, |cv, _| {
            if !buttons {
                cv.fill_rect(body, 0xFFFFFF);
                cv.frame_rect(body, 0xD9D9D9, 1);
            }
            for (i, r) in rects.iter().enumerate() {
                if owner_draw {
                    break;
                }
                let sel = t.cur == i as i32;
                if buttons {
                    cv.fill_rect(*r, if sel { 0xCCE4F7 } else { 0xE1E1E1 });
                    cv.frame_rect(*r, if sel { 0x005499 } else { 0xADADAD }, 1);
                } else {
                    cv.fill_rect(*r, if sel { 0xFFFFFF } else { 0xF0F0F0 });
                    cv.frame_rect(*r, 0xD9D9D9, 1);
                    if sel {
                        // the selected tab opens into the body
                        let y = if bottom { r.t } else { r.b - 1 };
                        cv.fill_rect(Rect::new(r.l + 1, y, r.r - 1, y + 1), 0xFFFFFF);
                        let top = if bottom { r.b - 2 } else { r.t };
                        cv.fill_rect(Rect::new(r.l, top, r.r, top + 2), 0x0078D7);
                    }
                }
                let mut x = r.l + t.pad.0.max(6);
                let it = &t.items[i];
                if let (Some(l), true) = (&list, it.image >= 0) {
                    let _ = l;
                    x += 0;
                }
                let ty = r.t + (r.height() - font.height()) / 2;
                font.draw(cv, x + list.as_ref().map_or(0, |l| if it.image >= 0 { l.w + 4 } else { 0 }), ty, &it.text, if enabled { 0 } else { 0x838383 }, None);
                if focused && t.focus == i as i32 {
                    super::controls::dotted_focus(cv, Rect::new(r.l + 3, r.t + 3, r.r - 3, r.b - 3));
                }
            }
        });
        // images after the text loop so they are not covered
        let list = if owner_draw { None } else { list };
        if let Some(l) = &list {
            let imgs: Vec<(usize, i32, i32)> = rects.iter().enumerate().filter(|(i, _)| t.items[*i].image >= 0).map(|(i, r)| (t.items[i].image as usize, r.l + t.pad.0.max(6), r.t + (r.height() - l.h) / 2)).collect();
            for (idx, x, y) in imgs {
                g.draw_imagelist(hdc, t.images, idx, x, y, 0, 0);
            }
        }
    });
    if !owner.is_empty() {
        let (parent, id) = with(|g| g.windows.get(&h).map_or((0, 0), |w| (w.parent, w.id)));
        for (i, r, data, sel, focus) in owner {
            // DRAWITEMSTRUCT: type ODT_TAB, control id, item, action ODA_DRAWENTIRE, state, window, dc, rectangle, data
            let mut di = [0u8; 64];
            di[0..4].copy_from_slice(&101u32.to_le_bytes());
            di[4..8].copy_from_slice(&(id as u32).to_le_bytes());
            di[8..12].copy_from_slice(&(i as u32).to_le_bytes());
            di[12..16].copy_from_slice(&1u32.to_le_bytes());
            let state = u32::from(sel) | if focus { 0x10 } else { 0 };
            di[16..20].copy_from_slice(&state.to_le_bytes());
            di[24..32].copy_from_slice(&(h as u64).to_le_bytes());
            di[32..40].copy_from_slice(&(hdc as u64).to_le_bytes());
            for (k, v) in [r.l, r.t, r.r, r.b].iter().enumerate() {
                di[40 + k * 4..44 + k * 4].copy_from_slice(&v.to_le_bytes());
            }
            di[56..64].copy_from_slice(&(data as u64).to_le_bytes());
            api::send(parent, WM_DRAWITEM, id, di.as_ptr() as isize);
        }
    }
    api::end_paint(h, ps.as_ptr());
}

unsafe fn select(h: usize, i: i32, notify_parent: bool) -> bool {
    let (count, cur) = with(|g| st(g, h).map_or((0, -1), |t| (t.items.len() as i32, t.cur)));
    if i < -1 || i >= count || i == cur {
        return false;
    }
    if notify_parent && notify(h, TCN_SELCHANGING) != 0 {
        return false;
    }
    with(|g| {
        if let Some(t) = st_mut(g, h) {
            t.cur = i;
            t.focus = i;
        }
        g.invalidate(h, None, true);
    });
    if notify_parent {
        notify(h, TCN_SELCHANGE);
    }
    true
}

unsafe fn item_text(lp: isize, wide: bool) -> String {
    if lp == 0 {
        String::new()
    } else if wide {
        crate::runtime::strings::w_ptr_to_string(lp as *const u16)
    } else {
        crate::runtime::strings::a_to_string(lp as *const u8)
    }
}

/// Reads a TCITEM (mask @0, pszText @16, iImage @28, lParam @32).
unsafe fn read_item(p: *const u8, wide: bool, base: Tab) -> Tab {
    let mask = *(p as *const u32);
    let mut t = base;
    if mask & TCIF_TEXT != 0 {
        t.text = item_text(*(p.add(16) as *const u64) as isize, wide);
    }
    if mask & TCIF_IMAGE != 0 {
        t.image = *(p.add(28) as *const i32);
    }
    if mask & TCIF_PARAM != 0 {
        t.param = *(p.add(32) as *const i64) as isize;
    }
    t
}

pub unsafe extern "win64" fn tab_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    if crate::runtime::cfg().trace.api && msg != 0x84 && msg != 0x20 && msg != 0x200 {
        eprintln!("nle[gui]: tab {h:#x} msg={msg:#x} wp={wp:#x} lp={lp:#x}");
    }
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::Tab(Box::new(TabControl { cur: -1, focus: -1, ..Default::default() }));
                }
            });
            1
        }
        WM_PAINT => {
            paint_tab(h);
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
        WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS => {
            with(|g| g.invalidate(h, None, true));
            0
        }
        WM_GETDLGCODE => 1,
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if !with(|g| g.is_enabled(h)) {
                return 0;
            }
            api::set_focus(h);
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            let hit = with(|g| tab_rects(g, h).iter().position(|r| r.contains(x, y)));
            if let Some(i) = hit {
                select(h, i as i32, true);
            }
            notify(h, NM_CLICK);
            0
        }
        0x0205 => {
            notify(h, NM_RCLICK);
            0
        }
        WM_KEYDOWN => {
            let (cur, count) = with(|g| st(g, h).map_or((-1, 0), |t| (t.cur, t.items.len() as i32)));
            match wp as u32 {
                0x25 if cur > 0 => {
                    select(h, cur - 1, true);
                }
                0x27 if cur + 1 < count => {
                    select(h, cur + 1, true);
                }
                _ => {}
            }
            0
        }
        // TCM_INSERTITEMA / W
        0x1307 | 0x133E => {
            let t = read_item(lp as *const u8, msg == 0x133E, Tab { image: -1, ..Default::default() });
            with(|g| {
                let Some(ctl) = st_mut(g, h) else { return -1 };
                let at = wp.min(ctl.items.len());
                ctl.items.insert(at, t);
                if ctl.cur < 0 {
                    ctl.cur = 0;
                    ctl.focus = 0;
                } else if at as i32 <= ctl.cur {
                    ctl.cur += 1;
                }
                g.invalidate(h, None, true);
                at as isize
            })
        }
        // TCM_GETITEMA / W: only the fields the mask asks for
        0x1305 | 0x133C => with(|g| {
            let Some(t) = st(g, h).and_then(|c| c.items.get(wp)).cloned() else { return 0 };
            let p = lp as *mut u8;
            let mask = *(p as *const u32);
            if mask & TCIF_TEXT != 0 {
                let buf = *(p.add(16) as *const u64) as usize;
                let max = *(p.add(24) as *const i32);
                if buf != 0 && max > 0 {
                    if msg == 0x133C {
                        crate::runtime::strings::return_w(buf as *mut u16, max as u32, &t.text);
                    } else {
                        crate::runtime::strings::return_a(buf as *mut u8, max as u32, &t.text);
                    }
                }
            }
            if mask & TCIF_IMAGE != 0 {
                *(p.add(28) as *mut i32) = t.image;
            }
            if mask & TCIF_PARAM != 0 {
                *(p.add(32) as *mut i64) = t.param as i64;
            }
            1
        }),
        // TCM_SETITEMA / W
        0x1306 | 0x133D => {
            let ok = with(|g| st(g, h).and_then(|c| c.items.get(wp)).cloned());
            let Some(old) = ok else { return 0 };
            let t = read_item(lp as *const u8, msg == 0x133D, old);
            with(|g| {
                if let Some(c) = st_mut(g, h) {
                    c.items[wp] = t;
                }
                g.invalidate(h, None, true);
            });
            1
        }
        0x1308 => with(|g| {
            let Some(c) = st_mut(g, h) else { return 0 };
            if wp >= c.items.len() {
                return 0;
            }
            c.items.remove(wp);
            c.cur = if c.items.is_empty() { -1 } else { c.cur.min(c.items.len() as i32 - 1).max(0) };
            g.invalidate(h, None, true);
            1
        }),
        0x1309 => with(|g| {
            if let Some(c) = st_mut(g, h) {
                c.items.clear();
                c.cur = -1;
            }
            g.invalidate(h, None, true);
            1
        }),
        0x1304 => with(|g| st(g, h).map_or(0, |c| c.items.len() as isize)),
        0x130B => with(|g| st(g, h).map_or(-1, |c| c.cur as isize)),
        0x130C => {
            let prev = with(|g| st(g, h).map_or(-1, |c| c.cur));
            select(h, wp as i32, false);
            prev as isize
        }
        0x132F => with(|g| st(g, h).map_or(-1, |c| c.focus as isize)),
        0x1330 => {
            select(h, wp as i32, true);
            0
        }
        0x130A => with(|g| match tab_rects(g, h).get(wp) {
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
        // TCM_HITTEST: TCHITTESTINFO { POINT pt; UINT flags }
        0x130D => with(|g| {
            let p = lp as *mut i32;
            let (x, y) = (*p, *p.add(1));
            match tab_rects(g, h).iter().position(|r| r.contains(x, y)) {
                Some(i) => {
                    *(p.add(2) as *mut u32) = 6; // TCHT_ONITEM
                    i as isize
                }
                None => {
                    *(p.add(2) as *mut u32) = 1; // TCHT_NOWHERE
                    -1
                }
            }
        }),
        // TCM_ADJUSTRECT: the body area for a given window rectangle (wParam FALSE) or the reverse
        0x1328 => with(|g| {
            if lp == 0 {
                return 0;
            }
            let th = tab_height(g, h);
            let bottom = g.windows.get(&h).is_some_and(|w| w.style & TCS_BOTTOM != 0);
            let r = lp as *mut i32;
            let sign = if wp != 0 { 1 } else { -1 };
            // wParam TRUE: from the display area to the whole control (grow); FALSE: the other way
            let grow = sign;
            *r -= 2 * grow;
            *r.add(2) += 2 * grow;
            if bottom {
                *r.add(1) -= 2 * grow;
                *r.add(3) += (th + 2) * grow;
            } else {
                *r.add(1) -= (th + 2) * grow;
                *r.add(3) += 2 * grow;
            }
            0
        }),
        0x132C => 1,
        0x1302 => with(|g| st(g, h).map_or(0, |c| c.images as isize)),
        0x1303 => {
            let old = with(|g| {
                let old = st(g, h).map_or(0, |c| c.images);
                if let Some(c) = st_mut(g, h) {
                    c.images = lp as usize;
                }
                g.invalidate(h, None, true);
                old
            });
            old as isize
        }
        0x1329 => with(|g| {
            if let Some(c) = st_mut(g, h) {
                c.fixed = (lo_signed(lp), hi_signed(lp));
            }
            g.invalidate(h, None, true);
            0
        }),
        0x132B => with(|g| {
            if let Some(c) = st_mut(g, h) {
                c.pad = (lo_signed(lp), hi_signed(lp));
            }
            g.invalidate(h, None, true);
            0
        }),
        0x1332 | 0x1333 | 0x1334 | 0x130E | 0x132E | 0x1331 | 0x132A => 0,
        0x1335 | 0x132D => 0,
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tcitem_is_read_field_by_field() {
        let mut raw = vec![0u8; 40];
        raw[0..4].copy_from_slice(&(TCIF_IMAGE | TCIF_PARAM).to_le_bytes());
        raw[28..32].copy_from_slice(&5i32.to_le_bytes());
        raw[32..40].copy_from_slice(&77i64.to_le_bytes());
        let t = unsafe { read_item(raw.as_ptr(), true, Tab { text: "keep".into(), image: -1, param: 0 }) };
        assert_eq!((t.text.as_str(), t.image, t.param), ("keep", 5, 77));
    }
}
