//! user32 and gdi32 functions the larger applications call beyond the basics: batched window positioning, frame
//! controls, key to character translation, icon images, clipboard viewer registration, text extents with per
//! character positions, printing entry points (which fail: there is no printer to spool to).

use crate::gui::gdi::*;
use crate::gui::surface::*;
use crate::gui::with;
use crate::runtime::strings::*;
use crate::shim::gdi32::text_extent_of;
use crate::shim::user32::{rect_from, rect_to};
use crate::win::*;
use std::sync::Mutex;

// ---- printing: every entry point reports that there is no printer ----------------------------------------------

crate::shims! { "gdi32" => GDI_EXT_SHIMS;
    fn StartDocW(_hdc: usize, _doc: usize) -> i32 {
        set_last_error(ERROR_NOT_SUPPORTED);
        -1
    }
    fn StartDocA(_hdc: usize, _doc: usize) -> i32 {
        set_last_error(ERROR_NOT_SUPPORTED);
        -1
    }
    fn EndDoc(_hdc: usize) -> i32 {
        -1
    }
    fn AbortDoc(_hdc: usize) -> i32 {
        -1
    }
    fn StartPage(_hdc: usize) -> i32 {
        -1
    }
    fn EndPage(_hdc: usize) -> i32 {
        -1
    }
    // logical and device coordinates are the same in the mapping mode this implementation draws in
    fn DPtoLP(_hdc: usize, _pts: *mut i32, _n: i32) -> BOOL {
        TRUE
    }
    fn LPtoDP(_hdc: usize, _pts: *mut i32, _n: i32) -> BOOL {
        TRUE
    }
    fn OffsetWindowOrgEx(hdc: usize, dx: i32, dy: i32, old: *mut i32) -> BOOL {
        with(|g| match g.dc_mut(hdc) {
            Some(d) => {
                if !old.is_null() {
                    *old = d.win_org.0;
                    *old.add(1) = d.win_org.1;
                }
                d.win_org = (d.win_org.0 + dx, d.win_org.1 + dy);
                TRUE
            }
            None => FALSE,
        })
    }
    fn GdiAlphaBlend(dst: usize, dx: i32, dy: i32, dw: i32, dh: i32, src: usize, sx: i32, sy: i32, sw: i32, sh: i32, blend: u32) -> BOOL {
        crate::shim::gdi32::AlphaBlend(dst, dx, dy, dw, dh, src, sx, sy, sw, sh, blend)
    }
    // GetTextExtentExPoint: how many characters fit in `max` units, the running width after each, and the total
    fn GetTextExtentExPointW(hdc: usize, text: usize, n: i32, max: i32, fit: *mut i32, dx: *mut i32, size: *mut i32) -> BOOL {
        extent_ex(hdc, text, n, max, fit, dx, size, true)
    }
    fn GetTextExtentExPointA(hdc: usize, text: usize, n: i32, max: i32, fit: *mut i32, dx: *mut i32, size: *mut i32) -> BOOL {
        extent_ex(hdc, text, n, max, fit, dx, size, false)
    }
}

unsafe fn extent_ex(hdc: usize, text: usize, n: i32, max: i32, fit: *mut i32, dx: *mut i32, size: *mut i32, wide: bool) -> BOOL {
    if text == 0 || n < 0 {
        return FALSE;
    }
    let chars: Vec<char> = if wide {
        char::decode_utf16(wstr_n(text as *const u16, n).iter().copied()).map(|c| c.unwrap_or('?')).collect()
    } else {
        astr_n(text as *const u8, n).iter().map(|b| *b as char).collect()
    };
    let mut prefix = String::new();
    let mut fitted = 0;
    let mut last = (0, 0);
    for (i, c) in chars.iter().enumerate() {
        prefix.push(*c);
        last = text_extent_of(hdc, &prefix);
        if !dx.is_null() {
            *dx.add(i) = last.0;
        }
        if max < 0 || last.0 <= max {
            fitted = i as i32 + 1;
        }
    }
    if !fit.is_null() {
        *fit = fitted;
    }
    if !size.is_null() {
        let (w, h) = if chars.is_empty() { text_extent_of(hdc, "") } else { last };
        *size = w;
        *size.add(1) = h;
    }
    TRUE
}

// ---- user32 ---------------------------------------------------------------------------------------------------

/// Pending `BeginDeferWindowPos` batches: handle -> windows moved so far.
static DEFERRED: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

fn us_char(vk: u32, shift: bool, caps: bool, ctrl: bool) -> Option<u16> {
    let c = match vk {
        0x41..=0x5a => {
            let upper = shift != caps;
            let base = (vk as u8 - 0x41) + if upper { b'A' } else { b'a' };
            if ctrl {
                return Some((vk - 0x40) as u16);
            }
            base as u16
        }
        0x30..=0x39 => {
            if shift {
                b")!@#$%^&*(".get((vk - 0x30) as usize).copied()? as u16
            } else {
                vk as u16
            }
        }
        0x60..=0x69 => (vk - 0x60 + 0x30) as u16,
        0x20 => 0x20,
        0x0d => 0x0d,
        0x09 => 0x09,
        0x08 => 0x08,
        0x1b => 0x1b,
        0x6a => b'*' as u16,
        0x6b => b'+' as u16,
        0x6d => b'-' as u16,
        0x6e => b'.' as u16,
        0x6f => b'/' as u16,
        0xba => (if shift { b':' } else { b';' }) as u16,
        0xbb => (if shift { b'+' } else { b'=' }) as u16,
        0xbc => (if shift { b'<' } else { b',' }) as u16,
        0xbd => (if shift { b'_' } else { b'-' }) as u16,
        0xbe => (if shift { b'>' } else { b'.' }) as u16,
        0xbf => (if shift { b'?' } else { b'/' }) as u16,
        0xc0 => (if shift { b'~' } else { b'`' }) as u16,
        0xdb => (if shift { b'{' } else { b'[' }) as u16,
        0xdc => (if shift { b'|' } else { b'\\' }) as u16,
        0xdd => (if shift { b'}' } else { b']' }) as u16,
        0xde => (if shift { b'"' } else { b'\'' }) as u16,
        _ => return None,
    };
    Some(c)
}

crate::shims! { "user32" => USER_EXT2_SHIMS;
    fn BeginDeferWindowPos(n: i32) -> usize {
        let mut d = DEFERRED.lock().unwrap_or_else(|e| e.into_inner());
        let handle = 0x9000 + d.len() * 4 + 4;
        d.push((handle, n.max(0) as usize));
        handle
    }
    fn DeferWindowPos(batch: usize, hwnd: usize, after: usize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> usize {
        // applied at once: nothing observes the windows between the calls of one batch
        crate::shim::user32::SetWindowPos(hwnd, after, x, y, cx, cy, flags);
        batch
    }
    fn EndDeferWindowPos(batch: usize) -> BOOL {
        let mut d = DEFERRED.lock().unwrap_or_else(|e| e.into_inner());
        d.retain(|e| e.0 != batch);
        TRUE
    }
    fn ChildWindowFromPointEx(parent: usize, pt: i64, _flags: u32) -> usize {
        crate::shim::user32::ChildWindowFromPoint(parent, pt)
    }
    fn GetLastActivePopup(hwnd: usize) -> usize {
        hwnd
    }
    fn SetClipboardViewer(_hwnd: usize) -> usize {
        0
    }
    fn ChangeClipboardChain(_hwnd: usize, _next: usize) -> BOOL {
        TRUE
    }
    fn NotifyWinEvent(_event: u32, _hwnd: usize, _object: i32, _child: i32) {}
    fn mouse_event(_flags: u32, _dx: u32, _dy: u32, _data: u32, _extra: usize) {}
    fn GetMenuBarInfo(_hwnd: usize, _object: i32, _item: i32, _info: usize) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn GetComboBoxInfo(_hwnd: usize, _info: usize) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    // ToAscii(vk, scan, keyboard state[256], out, flags): characters for a key on a US layout
    fn ToAscii(vk: u32, _scan: u32, state: *const u8, out: *mut u16, _flags: u32) -> i32 {
        let down = |k: usize| !state.is_null() && *state.add(k) & 0x80 != 0;
        let caps = !state.is_null() && *state.add(0x14) & 1 != 0;
        match us_char(vk, down(0x10), caps, down(0x11)) {
            Some(c) => {
                *out = c;
                1
            }
            None => 0,
        }
    }
    // ICONINFO: fIcon @0, xHotspot @4, yHotspot @8, hbmMask @16, hbmColor @24
    fn CreateIconIndirect(info: *const u8) -> usize {
        let is_icon = *(info as *const BOOL) != 0;
        let hot = (*(info.add(4) as *const u32) as i32, *(info.add(8) as *const u32) as i32);
        let mask = *(info.add(16) as *const usize);
        let color = *(info.add(24) as *const usize);
        with(|g| {
            let take = |g: &crate::gui::Gui, h: usize| match g.objects.get(&h) {
                Some(Object::Bitmap(b)) => Some((b.surf.w, b.surf.h, (0..b.surf.h).flat_map(|y| (0..b.surf.w).map(move |x| (x, y))).map(|(x, y)| b.surf.get(x, y)).collect::<Vec<u32>>())),
                _ => None,
            };
            let (w, h, rgb) = match take(g, color) {
                Some(c) => c,
                None => return 0,
            };
            // the mask is 1 where the icon is transparent; without one the whole image shows
            let alpha: Vec<u8> = match take(g, mask) {
                Some((mw, mh, bits)) if mw == w && mh >= h => bits.iter().take((w * h) as usize).map(|b| if *b & 0xff > 0x7f { 0 } else { 255 }).collect(),
                _ => vec![255; (w * h) as usize],
            };
            let img = crate::gui::res::IconImage { w, h, rgb, alpha, hotspot: if is_icon { (w / 2, h / 2) } else { hot } };
            g.add_object(Object::Icon(std::sync::Arc::new(img)))
        })
    }
    fn GetIconInfo(icon: usize, info: *mut u8) -> BOOL {
        with(|g| {
            let Some(Object::Icon(img)) = g.objects.get(&icon).map(|o| match o {
                Object::Icon(i) => Object::Icon(i.clone()),
                _ => Object::Stub(0),
            }) else {
                return FALSE;
            };
            let mut color = Surface::new(img.w, img.h);
            let mut mask = Surface::new(img.w, img.h);
            for y in 0..img.h {
                for x in 0..img.w {
                    let i = (y * img.w + x) as usize;
                    color.put(x, y, img.rgb[i]);
                    mask.put(x, y, if img.alpha[i] > 0x7f { 0 } else { 0xffffff });
                }
            }
            let hc = g.add_object(Object::Bitmap(Bitmap { surf: color, bpp: 32, bits: 0 }));
            let hm = g.add_object(Object::Bitmap(Bitmap { surf: mask, bpp: 1, bits: 0 }));
            *(info as *mut BOOL) = TRUE;
            *(info.add(4) as *mut u32) = img.hotspot.0 as u32;
            *(info.add(8) as *mut u32) = img.hotspot.1 as u32;
            *(info.add(16) as *mut usize) = hm;
            *(info.add(24) as *mut usize) = hc;
            TRUE
        })
    }
    // DrawFrameControl(hdc, rect, type, state): the classic look of buttons, scroll arrows and the like
    fn DrawFrameControl(hdc: usize, r: *mut i32, kind: u32, state: u32) -> BOOL {
        let rect = rect_from(r);
        let sunken = state & 0x0200 != 0 || state & 0x0020 != 0; // DFCS_PUSHED | DFCS_CHECKED
        let flat = state & 0x4000 != 0;
        let face = sys_color(15);
        let text = if state & 0x0100 != 0 { sys_color(17) } else { sys_color(18) };
        let mut edge = [rect.l, rect.t, rect.r, rect.b];
        match kind {
            // DFC_BUTTON
            4 => {
                let which = state & 0xff;
                match which {
                    // check box, three state: a white square with a sunken edge, a tick when checked
                    0 | 5 | 4 | 6 | 3 => {
                        let side = rect.width().min(rect.height()).min(13);
                        let (l, t) = (rect.l + (rect.width() - side) / 2, rect.t + (rect.height() - side) / 2);
                        let sq = Rect::new(l, t, l + side, t + side);
                        with(|g| g.draw(hdc, |cv, _| cv.fill_rect(sq, if state & 0x0100 != 0 { face } else { 0xffffff })));
                        let mut e = [sq.l, sq.t, sq.r, sq.b];
                        crate::shim::user32::draw_edge_pub(hdc, e.as_mut_ptr(), 2, 0);
                        if state & 0x0400 != 0 {
                            with(|g| g.draw(hdc, |cv, _| {
                                let pen = Pen { style: PenStyle::Solid, width: 1, color: text };
                                let (cx, cy) = (sq.l + side / 2, sq.t + side / 2);
                                cv.line(cx - 3, cy, cx - 1, cy + 2, &pen);
                                cv.line(cx - 1, cy + 2, cx + 3, cy - 2, &pen);
                            }));
                        }
                    }
                    // radio button: a disc
                    1 | 2 => {
                        let side = rect.width().min(rect.height()).min(13);
                        let (l, t) = (rect.l + (rect.width() - side) / 2, rect.t + (rect.height() - side) / 2);
                        let sq = Rect::new(l, t, l + side, t + side);
                        with(|g| g.draw(hdc, |cv, _| {
                            let pen = Pen { style: PenStyle::Solid, width: 1, color: sys_color(16) };
                            cv.ellipse(sq, &pen, &Brush::Solid(0xffffff));
                            if state & 0x0400 != 0 {
                                let dot = Rect::new(sq.l + 4, sq.t + 4, sq.r - 4, sq.b - 4);
                                cv.ellipse(dot, &Pen { style: PenStyle::Solid, width: 1, color: text }, &Brush::Solid(text));
                            }
                        }));
                    }
                    // push button
                    _ => {
                        with(|g| g.draw(hdc, |cv, _| cv.fill_rect(rect, face)));
                        crate::shim::user32::draw_edge_pub(hdc, edge.as_mut_ptr(), if sunken { 10 } else { 5 }, if flat { 0x4000 } else { 0 });
                    }
                }
            }
            // DFC_SCROLL and DFC_CAPTION / DFC_MENU / DFC_POPUPMENU: the face, with an edge unless flat
            _ => {
                with(|g| g.draw(hdc, |cv, _| cv.fill_rect(rect, face)));
                if !flat {
                    crate::shim::user32::draw_edge_pub(hdc, edge.as_mut_ptr(), if sunken { 10 } else { 5 }, 0);
                }
            }
        }
        rect_to(r, rect);
        TRUE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn us_layout_characters() {
        assert_eq!(us_char(0x41, false, false, false), Some(b'a' as u16));
        assert_eq!(us_char(0x41, true, false, false), Some(b'A' as u16));
        assert_eq!(us_char(0x41, true, true, false), Some(b'a' as u16));
        assert_eq!(us_char(0x31, true, false, false), Some(b'!' as u16));
        assert_eq!(us_char(0xbd, true, false, false), Some(b'_' as u16));
        assert_eq!(us_char(0x41, false, false, true), Some(1));
        assert_eq!(us_char(0x70, false, false, false), None);
    }
}
