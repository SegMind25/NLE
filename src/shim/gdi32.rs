//! gdi32 and msimg32: device contexts, drawing objects, shapes, text, bitmaps, regions.
//! The model and the drawing live in `crate::gui::gdi`; these are the Win32 entry points.

use crate::gui::gdi::*;
use crate::gui::surface::*;
use crate::gui::with;
use crate::runtime::strings::*;
use crate::shim::user32::{rect_from, rect_to, text_arg};
use crate::win::*;

fn color_ok(c: u32) -> u32 {
    c & 0x00ff_ffff
}

unsafe fn text_of(p: usize, n: i32, wide: bool) -> String {
    if p == 0 || n == 0 {
        return String::new();
    }
    if n < 0 {
        return text_arg(p, wide);
    }
    if wide {
        w_to_string(wstr_n(p as *const u16, n))
    } else {
        String::from_utf8_lossy(astr_n(p as *const u8, n)).into_owned()
    }
}

unsafe fn points(p: *const i32, n: i32) -> Vec<(i32, i32)> {
    (0..n.max(0) as usize).map(|i| (*p.add(i * 2), *p.add(i * 2 + 1))).collect()
}

unsafe fn logfont_from(p: *const u8, wide: bool) -> LogFont {
    let face = if wide { w_to_string(wstr_n(p.add(28) as *const u16, 32)) } else { String::from_utf8_lossy(astr_n(p.add(28), 32)).into_owned() };
    let face = face.split('\0').next().unwrap_or("").to_string();
    LogFont { height: *(p as *const i32), width: *(p.add(4) as *const i32), weight: *(p.add(16) as *const i32), italic: *p.add(20) != 0, underline: *p.add(21) != 0, strike: *p.add(22) != 0, pitch_family: *p.add(27) as u32, face }
}

unsafe fn write_logfont(p: *mut u8, lf: &LogFont, wide: bool) {
    std::ptr::write_bytes(p, 0, if wide { 92 } else { 60 });
    *(p as *mut i32) = lf.height;
    *(p.add(4) as *mut i32) = lf.width;
    *(p.add(16) as *mut i32) = lf.weight;
    *p.add(20) = lf.italic as u8;
    *p.add(21) = lf.underline as u8;
    *p.add(22) = lf.strike as u8;
    *p.add(27) = lf.pitch_family as u8;
    if wide {
        let w = to_wide(&lf.face);
        std::ptr::copy_nonoverlapping(w.as_ptr(), p.add(28) as *mut u16, w.len().min(31));
    } else {
        let b = lf.face.as_bytes();
        std::ptr::copy_nonoverlapping(b.as_ptr(), p.add(28), b.len().min(31));
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_font(h: i32, w: i32, weight: i32, italic: u32, underline: u32, strike: u32, pitch_family: u32, face: String) -> usize {
    with(|g| g.add_object(Object::Font(LogFont { height: h, width: w, weight: if weight == 0 { 400 } else { weight }, italic: italic & 0xff != 0, underline: underline & 0xff != 0, strike: strike & 0xff != 0, pitch_family, face })))
}

unsafe fn text_out(hdc: usize, x: i32, y: i32, text: &str) -> BOOL {
    with(|g| g.text_out(hdc, x, y, text, None)) as BOOL
}

unsafe fn ext_text_out(hdc: usize, x: i32, y: i32, options: u32, rect: *const i32, text: &str) -> BOOL {
    let opaque = if options & 2 != 0 && !rect.is_null() { Some(rect_from(rect)) } else { None };
    with(|g| g.text_out(hdc, x, y, text, opaque)) as BOOL
}

/// Width and height of `text` in the font selected into `hdc`.
pub fn text_extent_of(hdc: usize, text: &str) -> (i32, i32) {
    with(|g| g.text_extent(hdc, text))
}

unsafe fn text_extent(hdc: usize, text: &str, size: *mut i32) -> BOOL {
    let (w, h) = with(|g| g.text_extent(hdc, text));
    *size = w;
    *size.add(1) = h;
    TRUE
}

unsafe fn text_metrics(hdc: usize, out: *mut u8, wide: bool) -> BOOL {
    let Some(v) = with(|g| g.view(hdc)) else { return FALSE };
    let f = &v.font;
    let size = if wide { 60 } else { 56 };
    std::ptr::write_bytes(out, 0, size);
    let w = |o: usize, val: i32| *(out.add(o) as *mut i32) = val;
    w(0, f.height());
    w(4, f.ascent);
    w(8, f.descent);
    w(12, (f.height() as f32 * 0.15) as i32);
    w(16, 0);
    w(20, f.avg_width);
    w(24, f.max_width);
    w(28, v.lf.weight);
    w(36, 96);
    w(40, 96);
    if wide {
        *(out.add(44) as *mut u16) = 0x20;
        *(out.add(46) as *mut u16) = 0xFFFE;
        *(out.add(48) as *mut u16) = 0x7F;
        *(out.add(50) as *mut u16) = 0x20;
        *out.add(52) = v.lf.italic as u8;
        *out.add(53) = v.lf.underline as u8;
        *out.add(54) = v.lf.strike as u8;
        *out.add(55) = if v.lf.family() == crate::gui::font::Family::Mono { 0x31 } else { 0x26 };
        *out.add(56) = 0;
    } else {
        *out.add(44) = 0x20;
        *out.add(45) = 0xFF;
        *out.add(46) = 0x7F;
        *out.add(47) = 0x20;
        *out.add(48) = v.lf.italic as u8;
        *out.add(49) = v.lf.underline as u8;
        *out.add(50) = v.lf.strike as u8;
        *out.add(51) = if v.lf.family() == crate::gui::font::Family::Mono { 0x31 } else { 0x26 };
    }
    TRUE
}

fn bitmap_dims(g: &crate::gui::Gui, hbmp: usize) -> Option<(i32, i32, u16)> {
    match g.objects.get(&hbmp) {
        Some(Object::Bitmap(b)) => Some((b.surf.w, b.surf.h, b.bpp)),
        _ => None,
    }
}

unsafe fn create_dib_section(bmi: *const u8, bits_out: *mut usize) -> usize {
    let Some(info) = dib_info(bmi) else { return 0 };
    if info.width <= 0 || info.height <= 0 || !matches!(info.bpp, 24 | 32) {
        set_last_error(87);
        return 0;
    }
    let fmt = if info.bpp == 32 { PixelFormat::Bgra32 } else { PixelFormat::Bgr24 };
    let stride = (info.width as usize * info.bpp as usize).div_ceil(32) * 4;
    let mem = libc::calloc(1, stride * info.height as usize + 16) as *mut u8;
    if mem.is_null() {
        return 0;
    }
    let surf = Surface::external(mem, info.width, info.height, fmt, info.bottom_up);
    if !bits_out.is_null() {
        *bits_out = mem as usize;
    }
    with(|g| g.add_object(Object::Bitmap(Bitmap { surf, bpp: info.bpp, bits: mem as usize })))
}

unsafe fn create_dibitmap(bmi: *const u8, init: u32, bits: *const u8) -> usize {
    let Some(info) = dib_info(bmi) else { return 0 };
    let mut surf = Surface::new(info.width.max(1), info.height.max(1));
    if init & 4 != 0 && !bits.is_null() {
        if let Some(s) = dib_to_surface(bmi, bits) {
            surf = s;
        }
    }
    with(|g| g.add_object(Object::Bitmap(Bitmap { surf, bpp: 32, bits: 0 })))
}

/// Pixels of a DIB the program passes to `StretchDIBits` / `SetDIBitsToDevice`, with the source
/// rectangle converted to top-down coordinates.
unsafe fn dib_source(bmi: *const u8, bits: *const u8, xs: i32, ys: i32, h: i32) -> Option<(Surface, i32, i32)> {
    let info = dib_info(bmi)?;
    let s = dib_to_surface(bmi, bits)?;
    let top = if info.bottom_up { info.height - ys - h } else { ys };
    Some((s, xs, top))
}

fn region_kind(list: &[Rect]) -> i32 {
    match list.len() {
        0 => 1,
        1 => 2,
        _ => 3,
    }
}

crate::shims! { "gdi32" => GDI32_SHIMS;
    fn GetStockObject(idx: i32) -> usize {
        with(|g| g.stock_object(idx))
    }
    fn CreatePen(style: i32, width: i32, color: u32) -> usize {
        with(|g| g.create_pen(style, width, color))
    }
    fn CreatePenIndirect(lp: *const u32) -> usize {
        with(|g| g.create_pen(*lp as i32, *lp.add(1) as i32, *lp.add(3)))
    }
    fn ExtCreatePen(style: u32, width: u32, lb: *const u32, _n: u32, _dashes: *const u32) -> usize {
        with(|g| g.create_pen((style & 0xf) as i32, width as i32, *lb.add(1)))
    }
    fn CreateSolidBrush(color: u32) -> usize {
        with(|g| g.add_object(Object::Brush(Brush::Solid(from_colorref(color)))))
    }
    fn CreateHatchBrush(style: i32, color: u32) -> usize {
        with(|g| g.add_object(Object::Brush(Brush::Hatch(style, from_colorref(color)))))
    }
    fn CreateBrushIndirect(lb: *const u32) -> usize {
        with(|g| {
            let b = match *lb {
                1 => Brush::Null,
                2 => Brush::Hatch(*lb.add(2) as i32, from_colorref(*lb.add(1))),
                _ => Brush::Solid(from_colorref(*lb.add(1))),
            };
            g.add_object(Object::Brush(b))
        })
    }
    fn CreatePatternBrush(hbmp: usize) -> usize {
        with(|g| {
            let (w, h) = bitmap_dims(g, hbmp).map_or((0, 0), |d| (d.0, d.1));
            let pixels: Vec<u32> = match g.objects.get(&hbmp) {
                Some(Object::Bitmap(b)) => (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| b.surf.get(x, y)).collect(),
                _ => Vec::new(),
            };
            g.add_object(Object::Brush(Brush::Pattern(std::sync::Arc::new(pixels), w, h)))
        })
    }
    fn CreateFontA(h: i32, w: i32, _esc: i32, _orient: i32, weight: i32, italic: u32, underline: u32, strike: u32, _charset: u32, _outprec: u32, _clip: u32, _quality: u32, pitch_family: u32, face: usize) -> usize {
        create_font(h, w, weight, italic, underline, strike, pitch_family, text_arg(face, false))
    }
    fn CreateFontW(h: i32, w: i32, _esc: i32, _orient: i32, weight: i32, italic: u32, underline: u32, strike: u32, _charset: u32, _outprec: u32, _clip: u32, _quality: u32, pitch_family: u32, face: usize) -> usize {
        create_font(h, w, weight, italic, underline, strike, pitch_family, text_arg(face, true))
    }
    fn CreateFontIndirectA(lf: *const u8) -> usize {
        let f = logfont_from(lf, false);
        with(|g| g.add_object(Object::Font(f)))
    }
    fn CreateFontIndirectW(lf: *const u8) -> usize {
        let f = logfont_from(lf, true);
        with(|g| g.add_object(Object::Font(f)))
    }
    fn CreateCompatibleDC(hdc: usize) -> usize {
        let _ = hdc;
        with(|g| g.new_dc(Target::Memory, 0, false))
    }
    fn CreateDCA(_driver: usize, _device: usize, _port: usize, _init: usize) -> usize {
        with(|g| g.new_dc(Target::Screen, 0, false))
    }
    fn CreateDCW(_driver: usize, _device: usize, _port: usize, _init: usize) -> usize {
        with(|g| g.new_dc(Target::Screen, 0, false))
    }
    fn CreateICA(_driver: usize, _device: usize, _port: usize, _init: usize) -> usize {
        with(|g| g.new_dc(Target::Screen, 0, false))
    }
    fn CreateICW(_driver: usize, _device: usize, _port: usize, _init: usize) -> usize {
        with(|g| g.new_dc(Target::Screen, 0, false))
    }
    fn DeleteDC(hdc: usize) -> BOOL {
        with(|g| g.delete_dc(hdc)) as BOOL
    }
    fn CreateCompatibleBitmap(_hdc: usize, w: i32, h: i32) -> usize {
        if w <= 0 || h <= 0 {
            return 0;
        }
        with(|g| g.add_object(Object::Bitmap(Bitmap { surf: Surface::new(w, h), bpp: 32, bits: 0 })))
    }
    fn CreateBitmap(w: i32, h: i32, _planes: u32, bpp: u32, bits: *const u8) -> usize {
        if w <= 0 || h <= 0 {
            return 0;
        }
        let mut s = Surface::new(w, h);
        if !bits.is_null() && bpp == 1 {
            let stride = ((w as usize + 15) / 16) * 2;
            for y in 0..h {
                for x in 0..w {
                    let on = (*bits.add(y as usize * stride + x as usize / 8) >> (7 - x % 8)) & 1 != 0;
                    s.put(x, y, if on { 0xffffff } else { 0 });
                }
            }
        } else if !bits.is_null() && bpp == 32 {
            for y in 0..h {
                for x in 0..w {
                    let p = bits.add((y * w + x) as usize * 4);
                    s.put(x, y, rgb(*p.add(2), *p.add(1), *p));
                }
            }
        }
        with(|g| g.add_object(Object::Bitmap(Bitmap { surf: s, bpp: bpp.max(1) as u16, bits: 0 })))
    }
    fn CreateBitmapIndirect(bm: *const u8) -> usize {
        let (w, h, bpp, bits) = (*(bm.add(4) as *const i32), *(bm.add(8) as *const i32), *(bm.add(18) as *const u16), *(bm.add(24) as *const usize));
        CreateBitmap(w, h, 1, bpp as u32, bits as *const u8)
    }
    fn CreateDIBSection(_hdc: usize, bmi: *const u8, _usage: u32, bits: *mut usize, _section: usize, _offset: u32) -> usize {
        create_dib_section(bmi, bits)
    }
    fn CreateDIBitmap(_hdc: usize, hdr: *const u8, init: u32, bits: *const u8, bmi: *const u8, _usage: u32) -> usize {
        let _ = hdr;
        create_dibitmap(if bmi.is_null() { hdr } else { bmi }, init, bits)
    }
    fn DeleteObject(h: usize) -> BOOL {
        with(|g| g.delete_object(h)) as BOOL
    }
    fn SelectObject(hdc: usize, obj: usize) -> usize {
        with(|g| g.select_object(hdc, obj))
    }
    fn GetCurrentObject(hdc: usize, kind: u32) -> usize {
        with(|g| g.dc(hdc).map_or(0, |d| match kind {
            1 => d.pen,
            2 => d.brush,
            6 => d.font,
            7 => d.bitmap,
            _ => 0,
        }))
    }
    fn GetObjectType(h: usize) -> u32 {
        with(|g| match g.objects.get(&h) {
            Some(Object::Pen(_)) => 1,
            Some(Object::Brush(_)) => 2,
            Some(Object::Dc(d)) => if matches!(d.target, Target::Memory) { 10 } else { 3 },
            Some(Object::Bitmap(_)) => 7,
            Some(Object::Region(_)) => 8,
            Some(Object::Font(_)) => 6,
            _ => 0,
        })
    }
    fn GetObjectA(h: usize, n: i32, out: *mut u8) -> i32 {
        get_object(h, n, out, false)
    }
    fn GetObjectW(h: usize, n: i32, out: *mut u8) -> i32 {
        get_object(h, n, out, true)
    }
    fn SaveDC(hdc: usize) -> i32 {
        with(|g| g.save_dc(hdc))
    }
    fn RestoreDC(hdc: usize, level: i32) -> BOOL {
        with(|g| g.restore_dc(hdc, level)) as BOOL
    }
    fn SetTextColor(hdc: usize, c: u32) -> u32 {
        with(|g| g.dc_mut(hdc).map_or(0xffff_ffff, |d| std::mem::replace(&mut d.text_color, color_ok(c))))
    }
    fn GetTextColor(hdc: usize) -> u32 {
        with(|g| g.dc(hdc).map_or(0xffff_ffff, |d| d.text_color))
    }
    fn SetBkColor(hdc: usize, c: u32) -> u32 {
        with(|g| g.dc_mut(hdc).map_or(0xffff_ffff, |d| std::mem::replace(&mut d.bk_color, color_ok(c))))
    }
    fn GetBkColor(hdc: usize) -> u32 {
        with(|g| g.dc(hdc).map_or(0xffff_ffff, |d| d.bk_color))
    }
    fn SetBkMode(hdc: usize, mode: i32) -> i32 {
        with(|g| g.dc_mut(hdc).map_or(0, |d| std::mem::replace(&mut d.bk_mode, mode)))
    }
    fn GetBkMode(hdc: usize) -> i32 {
        with(|g| g.dc(hdc).map_or(0, |d| d.bk_mode))
    }
    fn SetROP2(hdc: usize, mode: i32) -> i32 {
        with(|g| g.dc_mut(hdc).map_or(0, |d| std::mem::replace(&mut d.rop2, mode)))
    }
    fn GetROP2(hdc: usize) -> i32 {
        with(|g| g.dc(hdc).map_or(0, |d| d.rop2))
    }
    fn SetTextAlign(hdc: usize, a: u32) -> u32 {
        with(|g| g.dc_mut(hdc).map_or(0xffff_ffff, |d| std::mem::replace(&mut d.text_align, a)))
    }
    fn GetTextAlign(hdc: usize) -> u32 {
        with(|g| g.dc(hdc).map_or(0xffff_ffff, |d| d.text_align))
    }
    fn SetMapMode(hdc: usize, mode: i32) -> i32 {
        with(|g| g.dc_mut(hdc).map_or(0, |d| std::mem::replace(&mut d.map_mode, mode)))
    }
    fn GetMapMode(hdc: usize) -> i32 {
        with(|g| g.dc(hdc).map_or(0, |d| d.map_mode))
    }
    fn SetPolyFillMode(hdc: usize, mode: i32) -> i32 {
        with(|g| g.dc_mut(hdc).map_or(0, |d| std::mem::replace(&mut d.poly_fill, mode)))
    }
    fn GetPolyFillMode(hdc: usize) -> i32 {
        with(|g| g.dc(hdc).map_or(0, |d| d.poly_fill))
    }
    fn SetStretchBltMode(_hdc: usize, _mode: i32) -> i32 {
        3
    }
    fn GetStretchBltMode(_hdc: usize) -> i32 {
        3
    }
    fn SetGraphicsMode(_hdc: usize, _mode: i32) -> i32 {
        1
    }
    fn SetBrushOrgEx(_hdc: usize, _x: i32, _y: i32, old: *mut i32) -> BOOL {
        if !old.is_null() {
            *old = 0;
            *old.add(1) = 0;
        }
        TRUE
    }
    fn SetViewportOrgEx(hdc: usize, x: i32, y: i32, old: *mut i32) -> BOOL {
        with(|g| match g.dc_mut(hdc) {
            Some(d) => {
                if !old.is_null() {
                    *old = d.vp_org.0;
                    *old.add(1) = d.vp_org.1;
                }
                d.vp_org = (x, y);
                TRUE
            }
            None => FALSE,
        })
    }
    fn SetWindowOrgEx(hdc: usize, x: i32, y: i32, old: *mut i32) -> BOOL {
        with(|g| match g.dc_mut(hdc) {
            Some(d) => {
                if !old.is_null() {
                    *old = d.win_org.0;
                    *old.add(1) = d.win_org.1;
                }
                d.win_org = (x, y);
                TRUE
            }
            None => FALSE,
        })
    }
    fn OffsetViewportOrgEx(hdc: usize, dx: i32, dy: i32, old: *mut i32) -> BOOL {
        with(|g| match g.dc_mut(hdc) {
            Some(d) => {
                if !old.is_null() {
                    *old = d.vp_org.0;
                    *old.add(1) = d.vp_org.1;
                }
                d.vp_org = (d.vp_org.0 + dx, d.vp_org.1 + dy);
                TRUE
            }
            None => FALSE,
        })
    }
    fn GetViewportOrgEx(hdc: usize, out: *mut i32) -> BOOL {
        with(|g| g.dc(hdc).map(|d| {
            *out = d.vp_org.0;
            *out.add(1) = d.vp_org.1;
        })).is_some() as BOOL
    }
    fn GetWindowOrgEx(hdc: usize, out: *mut i32) -> BOOL {
        with(|g| g.dc(hdc).map(|d| {
            *out = d.win_org.0;
            *out.add(1) = d.win_org.1;
        })).is_some() as BOOL
    }
    fn SetViewportExtEx(_hdc: usize, _x: i32, _y: i32, _old: *mut i32) -> BOOL {
        TRUE
    }
    fn SetWindowExtEx(_hdc: usize, _x: i32, _y: i32, _old: *mut i32) -> BOOL {
        TRUE
    }
    fn SetDCBrushColor(hdc: usize, c: u32) -> u32 {
        with(|g| g.dc_mut(hdc).map_or(0xffff_ffff, |d| std::mem::replace(&mut d.dc_brush, from_colorref(c))))
    }
    fn SetDCPenColor(hdc: usize, c: u32) -> u32 {
        with(|g| g.dc_mut(hdc).map_or(0xffff_ffff, |d| std::mem::replace(&mut d.dc_pen, from_colorref(c))))
    }
    fn GetDCOrgEx(_hdc: usize, out: *mut i32) -> BOOL {
        *out = 0;
        *out.add(1) = 0;
        TRUE
    }
    fn GetDeviceCaps(_hdc: usize, index: i32) -> i32 {
        let (w, h) = with(|g| g.screen_size());
        match index {
            0 => 0x300,
            2 => 1,
            4 => w * 254 / 960,
            6 => h * 254 / 960,
            8 => w,
            10 => h,
            12 => 32,
            14 => 1,
            16 | 18 | 24 => -1,
            22 => 0,
            28 | 30 | 32 => 0xff,
            34 => 0x7fff,
            36 => 1,
            38 => 0x2A89,
            40 | 42 => 36,
            44 => 51,
            88 | 90 => 96,
            104 => 0,
            106 => 0,
            108 => 24,
            110 => w,
            111 => h,
            114 | 115 => 1,
            116 => 60,
            _ => 0,
        }
    }
    fn GetNearestColor(_hdc: usize, c: u32) -> u32 {
        c
    }
    fn CreatePalette(_lp: *const u8) -> usize {
        with(|g| g.add_object(Object::Stub(15)))
    }
    fn SelectPalette(_hdc: usize, _pal: usize, _bg: BOOL) -> usize {
        with(|g| g.stock_object(15))
    }
    fn RealizePalette(_hdc: usize) -> u32 {
        0
    }
    fn GdiFlush() -> BOOL {
        with(|g| g.flush());
        TRUE
    }
    fn GdiSetBatchLimit(_n: u32) -> u32 {
        1
    }
    fn GdiGetBatchLimit() -> u32 {
        1
    }
    fn SetLayout(_hdc: usize, _l: u32) -> u32 {
        0
    }
    fn AddFontResourceA(_name: usize) -> i32 {
        0
    }
    fn AddFontResourceW(_name: usize) -> i32 {
        0
    }
    fn RemoveFontResourceA(_name: usize) -> BOOL {
        TRUE
    }
    fn RemoveFontResourceW(_name: usize) -> BOOL {
        TRUE
    }

    // ---- shapes ----
    fn MoveToEx(hdc: usize, x: i32, y: i32, old: *mut i32) -> BOOL {
        with(|g| match g.dc_mut(hdc) {
            Some(d) => {
                if !old.is_null() {
                    *old = d.pos.0;
                    *old.add(1) = d.pos.1;
                }
                d.pos = (x, y);
                TRUE
            }
            None => FALSE,
        })
    }
    fn LineTo(hdc: usize, x: i32, y: i32) -> BOOL {
        with(|g| g.line_to(hdc, x, y)) as BOOL
    }
    fn GetCurrentPositionEx(hdc: usize, out: *mut i32) -> BOOL {
        with(|g| g.dc(hdc).map(|d| {
            *out = d.pos.0;
            *out.add(1) = d.pos.1;
        })).is_some() as BOOL
    }
    fn Rectangle(hdc: usize, l: i32, t: i32, r: i32, b: i32) -> BOOL {
        with(|g| g.rectangle(hdc, Rect::new(l, t, r, b))) as BOOL
    }
    fn RoundRect(hdc: usize, l: i32, t: i32, r: i32, b: i32, w: i32, h: i32) -> BOOL {
        with(|g| g.round_rect(hdc, Rect::new(l, t, r, b), w, h)) as BOOL
    }
    fn Ellipse(hdc: usize, l: i32, t: i32, r: i32, b: i32) -> BOOL {
        with(|g| g.ellipse(hdc, Rect::new(l, t, r, b))) as BOOL
    }
    fn Arc(hdc: usize, l: i32, t: i32, r: i32, b: i32, x1: i32, y1: i32, x2: i32, y2: i32) -> BOOL {
        with(|g| g.arc(hdc, Rect::new(l, t, r, b), (x1, y1), (x2, y2), 0)) as BOOL
    }
    fn Pie(hdc: usize, l: i32, t: i32, r: i32, b: i32, x1: i32, y1: i32, x2: i32, y2: i32) -> BOOL {
        with(|g| g.arc(hdc, Rect::new(l, t, r, b), (x1, y1), (x2, y2), 1)) as BOOL
    }
    fn Chord(hdc: usize, l: i32, t: i32, r: i32, b: i32, x1: i32, y1: i32, x2: i32, y2: i32) -> BOOL {
        with(|g| g.arc(hdc, Rect::new(l, t, r, b), (x1, y1), (x2, y2), 2)) as BOOL
    }
    fn Polygon(hdc: usize, pts: *const i32, n: i32) -> BOOL {
        with(|g| g.polygon(hdc, &points(pts, n))) as BOOL
    }
    fn Polyline(hdc: usize, pts: *const i32, n: i32) -> BOOL {
        with(|g| g.polyline(hdc, &points(pts, n))) as BOOL
    }
    fn PolylineTo(hdc: usize, pts: *const i32, n: u32) -> BOOL {
        let mut all = vec![with(|g| g.dc(hdc).map_or((0, 0), |d| d.pos))];
        all.extend(points(pts, n as i32));
        let last = *all.last().unwrap();
        with(|g| {
            g.polyline(hdc, &all);
            if let Some(d) = g.dc_mut(hdc) {
                d.pos = last;
            }
        });
        TRUE
    }
    fn PolyPolygon(hdc: usize, pts: *const i32, counts: *const i32, n: i32) -> BOOL {
        let mut off = 0usize;
        for i in 0..n.max(0) as usize {
            let c = *counts.add(i);
            let poly = points(pts.add(off * 2), c);
            with(|g| g.polygon(hdc, &poly));
            off += c.max(0) as usize;
        }
        TRUE
    }
    fn PolyBezier(hdc: usize, pts: *const i32, n: u32) -> BOOL {
        let p = points(pts, n as i32);
        let mut out = vec![p[0]];
        for seg in p[1..].chunks(3).filter(|s| s.len() == 3) {
            let start = *out.last().unwrap();
            for step in 1..=16 {
                let t = step as f64 / 16.0;
                let u = 1.0 - t;
                let x = u * u * u * start.0 as f64 + 3.0 * u * u * t * seg[0].0 as f64 + 3.0 * u * t * t * seg[1].0 as f64 + t * t * t * seg[2].0 as f64;
                let y = u * u * u * start.1 as f64 + 3.0 * u * u * t * seg[0].1 as f64 + 3.0 * u * t * t * seg[1].1 as f64 + t * t * t * seg[2].1 as f64;
                out.push((x.round() as i32, y.round() as i32));
            }
        }
        with(|g| g.polyline(hdc, &out)) as BOOL
    }
    fn SetPixel(hdc: usize, x: i32, y: i32, c: u32) -> u32 {
        with(|g| g.set_pixel(hdc, x, y, c));
        c
    }
    fn SetPixelV(hdc: usize, x: i32, y: i32, c: u32) -> BOOL {
        with(|g| g.set_pixel(hdc, x, y, c)) as BOOL
    }
    fn GetPixel(hdc: usize, x: i32, y: i32) -> u32 {
        with(|g| g.get_pixel(hdc, x, y))
    }
    fn PatBlt(hdc: usize, x: i32, y: i32, w: i32, h: i32, rop: u32) -> BOOL {
        with(|g| g.pat_blt(hdc, x, y, w, h, rop)) as BOOL
    }
    fn BitBlt(dst: usize, dx: i32, dy: i32, w: i32, h: i32, src: usize, sx: i32, sy: i32, rop: u32) -> BOOL {
        if crate::runtime::cfg().trace.gdi {
            eprintln!("nle[gdi]: BitBlt dst={dst:#x} ({dx},{dy} {w}x{h}) src={src:#x} ({sx},{sy}) rop={rop:#x}");
        }
        with(|g| g.bit_blt(dst, dx, dy, w, h, src, sx, sy, rop)) as BOOL
    }
    fn StretchBlt(dst: usize, dx: i32, dy: i32, dw: i32, dh: i32, src: usize, sx: i32, sy: i32, sw: i32, sh: i32, rop: u32) -> BOOL {
        with(|g| g.stretch_blt(dst, dx, dy, dw, dh, src, sx, sy, sw, sh, rop)) as BOOL
    }
    fn MaskBlt(dst: usize, dx: i32, dy: i32, w: i32, h: i32, src: usize, sx: i32, sy: i32, _mask: usize, _mx: i32, _my: i32, rop: u32) -> BOOL {
        with(|g| g.bit_blt(dst, dx, dy, w, h, src, sx, sy, rop & 0x00ff_0000 | 0x20)) as BOOL
    }
    fn StretchDIBits(hdc: usize, xd: i32, yd: i32, wd: i32, hd: i32, xs: i32, ys: i32, ws: i32, hs: i32, bits: *const u8, bmi: *const u8, _usage: u32, rop: u32) -> i32 {
        let Some((surf, sx, sy)) = dib_source(bmi, bits, xs, ys, hs) else { return 0 };
        let ok = with(|g| g.draw(hdc, |cv, _| cv.stretch(xd, yd, wd, hd, &surf, sx, sy, ws, hs, rop)).is_some());
        if ok { hs.abs() } else { 0 }
    }
    fn SetDIBitsToDevice(hdc: usize, xd: i32, yd: i32, w: u32, h: u32, xs: i32, ys: i32, _start: u32, _lines: u32, bits: *const u8, bmi: *const u8, _usage: u32) -> i32 {
        let Some((surf, sx, sy)) = dib_source(bmi, bits, xs, ys, h as i32) else { return 0 };
        let ok = with(|g| g.draw(hdc, |cv, v| cv.blit(xd, yd, w as i32, h as i32, &surf, sx, sy, 0x00CC0020, &v.brush)).is_some());
        if ok { h as i32 } else { 0 }
    }
    fn GetDIBits(_hdc: usize, hbmp: usize, start: u32, lines: u32, bits: *mut u8, bmi: *mut u8, _usage: u32) -> i32 {
        let Some(info) = dib_info(bmi) else { return 0 };
        let (w, h) = with(|g| bitmap_dims(g, hbmp)).map_or((0, 0), |d| (d.0, d.1));
        if w == 0 {
            return 0;
        }
        if bits.is_null() {
            // fill in the header the program will use to ask for pixels
            *(bmi.add(4) as *mut i32) = w;
            *(bmi.add(8) as *mut i32) = h;
            *(bmi.add(12) as *mut u16) = 1;
            if *(bmi.add(14) as *const u16) == 0 {
                *(bmi.add(14) as *mut u16) = 32;
            }
            *(bmi.add(16) as *mut u32) = 0;
            return h;
        }
        if !matches!(info.bpp, 24 | 32) {
            return 0;
        }
        let bytes = info.bpp as usize / 8;
        let stride = (w as usize * info.bpp as usize).div_ceil(32) * 4;
        let rows = (lines as i32).min(h - start as i32).max(0);
        with(|g| {
            if let Some(Object::Bitmap(b)) = g.objects.get(&hbmp) {
                for r in 0..rows {
                    let src_y = if info.bottom_up { h - 1 - (start as i32 + r) } else { start as i32 + r };
                    for x in 0..w {
                        let c = b.surf.get(x, src_y);
                        let p = bits.add(r as usize * stride + x as usize * bytes);
                        *p = c as u8;
                        *p.add(1) = (c >> 8) as u8;
                        *p.add(2) = (c >> 16) as u8;
                        if bytes == 4 {
                            *p.add(3) = 0;
                        }
                    }
                }
            }
        });
        rows
    }
    fn SetDIBits(_hdc: usize, hbmp: usize, _start: u32, _lines: u32, bits: *const u8, bmi: *const u8, _usage: u32) -> i32 {
        let Some(s) = dib_to_surface(bmi, bits) else { return 0 };
        let h = s.h;
        with(|g| {
            if let Some(Object::Bitmap(b)) = g.objects.get_mut(&hbmp) {
                for y in 0..h.min(b.surf.h) {
                    for x in 0..s.w.min(b.surf.w) {
                        b.surf.put(x, y, s.get(x, y));
                    }
                }
            }
        });
        h
    }

    // ---- text ----
    fn TextOutA(hdc: usize, x: i32, y: i32, text: usize, n: i32) -> BOOL {
        text_out(hdc, x, y, &text_of(text, n, false))
    }
    fn TextOutW(hdc: usize, x: i32, y: i32, text: usize, n: i32) -> BOOL {
        text_out(hdc, x, y, &text_of(text, n, true))
    }
    fn ExtTextOutA(hdc: usize, x: i32, y: i32, options: u32, rect: *const i32, text: usize, n: u32, _dx: *const i32) -> BOOL {
        ext_text_out(hdc, x, y, options, rect, &text_of(text, n as i32, false))
    }
    fn ExtTextOutW(hdc: usize, x: i32, y: i32, options: u32, rect: *const i32, text: usize, n: u32, _dx: *const i32) -> BOOL {
        ext_text_out(hdc, x, y, options, rect, &text_of(text, n as i32, true))
    }
    fn GetTextExtentPoint32A(hdc: usize, text: usize, n: i32, size: *mut i32) -> BOOL {
        text_extent(hdc, &text_of(text, n, false), size)
    }
    fn GetTextExtentPoint32W(hdc: usize, text: usize, n: i32, size: *mut i32) -> BOOL {
        text_extent(hdc, &text_of(text, n, true), size)
    }
    fn GetTextExtentPointA(hdc: usize, text: usize, n: i32, size: *mut i32) -> BOOL {
        text_extent(hdc, &text_of(text, n, false), size)
    }
    fn GetTextExtentPointW(hdc: usize, text: usize, n: i32, size: *mut i32) -> BOOL {
        text_extent(hdc, &text_of(text, n, true), size)
    }
    fn GetTextMetricsA(hdc: usize, out: *mut u8) -> BOOL {
        text_metrics(hdc, out, false)
    }
    fn GetTextMetricsW(hdc: usize, out: *mut u8) -> BOOL {
        text_metrics(hdc, out, true)
    }
    fn GetTextFaceA(hdc: usize, n: i32, buf: usize) -> i32 {
        let face = with(|g| g.view(hdc).map(|v| v.lf.face)).unwrap_or_default();
        if buf == 0 {
            return face.len() as i32 + 1;
        }
        return_a(buf as *mut u8, n as u32, &face) as i32
    }
    fn GetTextFaceW(hdc: usize, n: i32, buf: usize) -> i32 {
        let face = with(|g| g.view(hdc).map(|v| v.lf.face)).unwrap_or_default();
        if buf == 0 {
            return face.encode_utf16().count() as i32 + 1;
        }
        return_w(buf as *mut u16, n as u32, &face) as i32
    }
    fn GetCharWidth32A(hdc: usize, first: u32, last: u32, out: *mut i32) -> BOOL {
        char_widths(hdc, first, last, out)
    }
    fn GetCharWidth32W(hdc: usize, first: u32, last: u32, out: *mut i32) -> BOOL {
        char_widths(hdc, first, last, out)
    }
    fn GetCharWidthA(hdc: usize, first: u32, last: u32, out: *mut i32) -> BOOL {
        char_widths(hdc, first, last, out)
    }
    fn GetCharWidthW(hdc: usize, first: u32, last: u32, out: *mut i32) -> BOOL {
        char_widths(hdc, first, last, out)
    }
    fn EnumFontFamiliesExA(_hdc: usize, _lf: *const u8, _cb: usize, _lp: isize, _flags: u32) -> i32 {
        1
    }
    fn EnumFontFamiliesExW(_hdc: usize, _lf: *const u8, _cb: usize, _lp: isize, _flags: u32) -> i32 {
        1
    }

    // ---- regions and clipping ----
    fn CreateRectRgn(l: i32, t: i32, r: i32, b: i32) -> usize {
        with(|g| g.add_object(Object::Region(vec![Rect::new(l, t, r, b).normalized()])))
    }
    fn CreateRectRgnIndirect(r: *const i32) -> usize {
        let rect = rect_from(r).normalized();
        with(|g| g.add_object(Object::Region(vec![rect])))
    }
    fn CreateRoundRectRgn(l: i32, t: i32, r: i32, b: i32, _w: i32, _h: i32) -> usize {
        with(|g| g.add_object(Object::Region(vec![Rect::new(l, t, r, b).normalized()])))
    }
    fn CreateEllipticRgn(l: i32, t: i32, r: i32, b: i32) -> usize {
        with(|g| g.add_object(Object::Region(vec![Rect::new(l, t, r, b).normalized()])))
    }
    fn CreateEllipticRgnIndirect(r: *const i32) -> usize {
        let rect = rect_from(r).normalized();
        with(|g| g.add_object(Object::Region(vec![rect])))
    }
    fn CreatePolygonRgn(pts: *const i32, n: i32, _mode: i32) -> usize {
        let p = points(pts, n);
        let (minx, maxx) = p.iter().fold((i32::MAX, i32::MIN), |(a, b), q| (a.min(q.0), b.max(q.0)));
        let (miny, maxy) = p.iter().fold((i32::MAX, i32::MIN), |(a, b), q| (a.min(q.1), b.max(q.1)));
        with(|g| g.add_object(Object::Region(vec![Rect::new(minx, miny, maxx, maxy)])))
    }
    fn SetRectRgn(rgn: usize, l: i32, t: i32, r: i32, b: i32) -> BOOL {
        with(|g| match g.objects.get_mut(&rgn) {
            Some(Object::Region(list)) => {
                *list = vec![Rect::new(l, t, r, b).normalized()];
                TRUE
            }
            _ => FALSE,
        })
    }
    fn CombineRgn(dst: usize, a: usize, b: usize, mode: i32) -> i32 {
        with(|g| {
            let list = |g: &crate::gui::Gui, h: usize| match g.objects.get(&h) {
                Some(Object::Region(l)) => Some(l.clone()),
                _ => None,
            };
            let (Some(la), lb) = (list(g, a), list(g, b)) else { return 0 };
            let out = combine(mode, &la, &lb.unwrap_or_default());
            let kind = region_kind(&out);
            match g.objects.get_mut(&dst) {
                Some(Object::Region(l)) => {
                    *l = out;
                    kind
                }
                _ => 0,
            }
        })
    }
    fn OffsetRgn(rgn: usize, dx: i32, dy: i32) -> i32 {
        with(|g| match g.objects.get_mut(&rgn) {
            Some(Object::Region(l)) => {
                for r in l.iter_mut() {
                    *r = r.offset(dx, dy);
                }
                region_kind(l)
            }
            _ => 0,
        })
    }
    fn PtInRegion(rgn: usize, x: i32, y: i32) -> BOOL {
        with(|g| matches!(g.objects.get(&rgn), Some(Object::Region(l)) if l.iter().any(|r| r.contains(x, y)))) as BOOL
    }
    fn RectInRegion(rgn: usize, r: *const i32) -> BOOL {
        let rect = rect_from(r);
        with(|g| matches!(g.objects.get(&rgn), Some(Object::Region(l)) if l.iter().any(|x| !x.intersect(&rect).is_empty()))) as BOOL
    }
    fn GetRgnBox(rgn: usize, out: *mut i32) -> i32 {
        with(|g| match g.objects.get(&rgn) {
            Some(Object::Region(l)) => {
                rect_to(out, l.iter().fold(Rect::default(), |a, r| a.union(r)));
                region_kind(l)
            }
            _ => 0,
        })
    }
    fn EqualRgn(a: usize, b: usize) -> BOOL {
        with(|g| match (g.objects.get(&a), g.objects.get(&b)) {
            (Some(Object::Region(x)), Some(Object::Region(y))) => x == y,
            _ => false,
        }) as BOOL
    }
    fn SelectClipRgn(hdc: usize, rgn: usize) -> i32 {
        with(|g| {
            let list = if rgn == 0 { None } else { match g.objects.get(&rgn) {
                Some(Object::Region(l)) => Some(l.clone()),
                _ => None,
            } };
            match g.dc_mut(hdc) {
                Some(d) => {
                    let kind = list.as_ref().map_or(2, |l| region_kind(l));
                    d.clip = list;
                    kind
                }
                None => 0,
            }
        })
    }
    fn ExtSelectClipRgn(hdc: usize, rgn: usize, _mode: i32) -> i32 {
        SelectClipRgn(hdc, rgn)
    }
    fn GetClipRgn(hdc: usize, rgn: usize) -> i32 {
        with(|g| {
            let list = g.dc(hdc).and_then(|d| d.clip.clone());
            match (list, g.objects.get_mut(&rgn)) {
                (Some(l), Some(Object::Region(r))) => {
                    *r = l;
                    1
                }
                _ => 0,
            }
        })
    }
    fn IntersectClipRect(hdc: usize, l: i32, t: i32, r: i32, b: i32) -> i32 {
        with(|g| g.intersect_clip(hdc, Rect::new(l, t, r, b)))
    }
    fn ExcludeClipRect(hdc: usize, l: i32, t: i32, r: i32, b: i32) -> i32 {
        with(|g| {
            let full = g.clip_box(hdc).unwrap_or_default();
            let cur = g.dc(hdc).and_then(|d| d.clip.clone()).unwrap_or_else(|| vec![full]);
            let out = combine(4, &cur, &[Rect::new(l, t, r, b).normalized()]);
            let kind = region_kind(&out);
            if let Some(d) = g.dc_mut(hdc) {
                d.clip = Some(out);
            }
            kind
        })
    }
    fn OffsetClipRgn(hdc: usize, dx: i32, dy: i32) -> i32 {
        with(|g| match g.dc_mut(hdc) {
            Some(d) => {
                if let Some(l) = d.clip.as_mut() {
                    for r in l.iter_mut() {
                        *r = r.offset(dx, dy);
                    }
                }
                2
            }
            None => 0,
        })
    }
    fn GetClipBox(hdc: usize, out: *mut i32) -> i32 {
        match with(|g| g.clip_box(hdc)) {
            Some(r) => {
                rect_to(out, r);
                if r.is_empty() { 1 } else { 2 }
            }
            None => 0,
        }
    }
    fn PtVisible(hdc: usize, x: i32, y: i32) -> BOOL {
        with(|g| g.clip_box(hdc).is_some_and(|r| r.contains(x, y))) as BOOL
    }
    fn RectVisible(hdc: usize, r: *const i32) -> BOOL {
        let rect = rect_from(r);
        with(|g| g.clip_box(hdc).is_some_and(|c| !c.intersect(&rect).is_empty())) as BOOL
    }
}

crate::shims! { "msimg32" => MSIMG32_SHIMS;
    fn AlphaBlend(dst: usize, dx: i32, dy: i32, dw: i32, dh: i32, src: usize, sx: i32, sy: i32, sw: i32, sh: i32, blend: u32) -> BOOL {
        let alpha = (blend >> 16) & 0xff;
        with(|g| g.blend_blt(dst, dx, dy, dw, dh, src, sx, sy, sw, sh, None, alpha)) as BOOL
    }
    fn TransparentBlt(dst: usize, dx: i32, dy: i32, dw: i32, dh: i32, src: usize, sx: i32, sy: i32, sw: i32, sh: i32, key: u32) -> BOOL {
        with(|g| g.blend_blt(dst, dx, dy, dw, dh, src, sx, sy, sw, sh, Some(from_colorref(key)), 255)) as BOOL
    }
}

unsafe fn char_widths(hdc: usize, first: u32, last: u32, out: *mut i32) -> BOOL {
    let Some(v) = with(|g| g.view(hdc)) else { return FALSE };
    for (i, c) in (first..=last).enumerate() {
        *out.add(i) = v.font.advance(char::from_u32(c).unwrap_or(' ')).round() as i32;
    }
    TRUE
}

unsafe fn get_object(h: usize, n: i32, out: *mut u8, wide: bool) -> i32 {
    with(|g| match g.objects.get(&h) {
        Some(Object::Bitmap(b)) => {
            if out.is_null() {
                return 32;
            }
            if n < 24 {
                return 0;
            }
            std::ptr::write_bytes(out, 0, 32.min(n as usize));
            *(out.add(4) as *mut i32) = b.surf.w;
            *(out.add(8) as *mut i32) = b.surf.h;
            *(out.add(12) as *mut i32) = ((b.surf.w * b.bpp as i32 + 31) / 32) * 4;
            *(out.add(16) as *mut u16) = 1;
            *(out.add(18) as *mut u16) = b.bpp;
            *(out.add(24) as *mut usize) = b.bits;
            32
        }
        Some(Object::Font(f)) => {
            let size = if wide { 92 } else { 60 };
            if out.is_null() {
                return size;
            }
            if (n as usize) < size as usize {
                return 0;
            }
            write_logfont(out, f, wide);
            size
        }
        Some(Object::Pen(p)) => {
            if out.is_null() {
                return 16;
            }
            std::ptr::write_bytes(out, 0, 16);
            *(out as *mut u32) = match p.style {
                PenStyle::Dash => 1,
                PenStyle::Dot => 2,
                PenStyle::DashDot => 3,
                PenStyle::DashDotDot => 4,
                PenStyle::Null => 5,
                PenStyle::InsideFrame => 6,
                PenStyle::Solid => 0,
            };
            *(out.add(4) as *mut i32) = p.width;
            *(out.add(12) as *mut u32) = to_colorref(p.color);
            16
        }
        Some(Object::Brush(b)) => {
            if out.is_null() {
                return 16;
            }
            std::ptr::write_bytes(out, 0, 16);
            match b {
                Brush::Solid(c) => *(out.add(4) as *mut u32) = to_colorref(*c),
                Brush::Null => *(out as *mut u32) = 1,
                Brush::Hatch(s, c) => {
                    *(out as *mut u32) = 2;
                    *(out.add(4) as *mut u32) = to_colorref(*c);
                    *(out.add(8) as *mut u32) = *s as u32;
                }
                Brush::Pattern(..) => *(out as *mut u32) = 3,
            }
            16
        }
        _ => 0,
    })
}
