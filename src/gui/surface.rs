//! Pixel surfaces and the software drawing primitives behind GDI.
//!
//! Everything is drawn here, in this process, into BGRA (or packed 24-bit) pixel memory; the
//! display backend only ever receives finished pixels. Colours inside this module are `0x00RRGGBB`;
//! the Win32 `COLORREF` (`0x00BBGGRR`) is converted at the API edge.

use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl Rect {
    pub const fn new(l: i32, t: i32, r: i32, b: i32) -> Rect {
        Rect { l, t, r, b }
    }
    pub fn width(&self) -> i32 {
        self.r - self.l
    }
    pub fn height(&self) -> i32 {
        self.b - self.t
    }
    /// Width and height.
    pub fn wh(&self) -> (i32, i32) {
        (self.width(), self.height())
    }

    pub fn is_empty(&self) -> bool {
        self.r <= self.l || self.b <= self.t
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let r = Rect::new(self.l.max(o.l), self.t.max(o.t), self.r.min(o.r), self.b.min(o.b));
        if r.is_empty() {
            Rect::default()
        } else {
            r
        }
    }
    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect::new(self.l.min(o.l), self.t.min(o.t), self.r.max(o.r), self.b.max(o.b))
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.l + dx, self.t + dy, self.r + dx, self.b + dy)
    }
    /// Swaps edges that are the wrong way round, as GDI does for rectangle arguments.
    pub fn normalized(&self) -> Rect {
        Rect::new(self.l.min(self.r), self.t.min(self.b), self.l.max(self.r), self.t.max(self.b))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra32,
    Bgr24,
}

impl PixelFormat {
    pub fn bytes(self) -> usize {
        match self {
            PixelFormat::Bgra32 => 4,
            PixelFormat::Bgr24 => 3,
        }
    }
}

enum Storage {
    Owned(Vec<u8>),
    /// Pixel memory owned by the guest (a DIB section); it must outlive the surface.
    External(*mut u8, usize),
}

// The pointer is only dereferenced under the GUI lock.
unsafe impl Send for Storage {}

pub struct Surface {
    pub w: i32,
    pub h: i32,
    pub stride: usize,
    pub fmt: PixelFormat,
    /// Rows are stored bottom-up (positive `biHeight` DIB sections).
    pub bottom_up: bool,
    storage: Storage,
    /// Bounding box of everything drawn since the last time it was taken (for presenting).
    pub dirty: Rect,
}

pub fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// `COLORREF` (0x00BBGGRR) to 0x00RRGGBB.
pub fn from_colorref(c: u32) -> u32 {
    (c & 0xff) << 16 | (c & 0xff00) | (c >> 16) & 0xff
}

pub fn to_colorref(c: u32) -> u32 {
    from_colorref(c)
}

impl Surface {
    pub fn new(w: i32, h: i32) -> Surface {
        let (w, h) = (w.max(0), h.max(0));
        let stride = w as usize * 4;
        Surface { w, h, stride, fmt: PixelFormat::Bgra32, bottom_up: false, storage: Storage::Owned(vec![0xff; stride * h as usize]), dirty: Rect::default() }
    }

    /// A surface over guest memory (`CreateDIBSection`).
    pub unsafe fn external(bits: *mut u8, w: i32, h: i32, fmt: PixelFormat, bottom_up: bool) -> Surface {
        let stride = (w as usize * fmt.bytes() * 8).div_ceil(32) * 4;
        Surface { w, h, stride, fmt, bottom_up, storage: Storage::External(bits, stride * h as usize), dirty: Rect::default() }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }

    fn data(&self) -> &[u8] {
        match &self.storage {
            Storage::Owned(v) => v,
            Storage::External(p, n) => unsafe { std::slice::from_raw_parts(*p, *n) },
        }
    }

    fn data_mut(&mut self) -> &mut [u8] {
        match &mut self.storage {
            Storage::Owned(v) => v,
            Storage::External(p, n) => unsafe { std::slice::from_raw_parts_mut(*p, *n) },
        }
    }

    /// Raw pixel memory, top row first for owned surfaces (what the display backends send).
    pub fn bytes(&self) -> &[u8] {
        self.data()
    }

    fn offset(&self, x: i32, y: i32) -> usize {
        let row = if self.bottom_up { self.h - 1 - y } else { y };
        row as usize * self.stride + x as usize * self.fmt.bytes()
    }

    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return 0;
        }
        let o = self.offset(x, y);
        let d = self.data();
        (d[o + 2] as u32) << 16 | (d[o + 1] as u32) << 8 | d[o] as u32
    }

    pub fn put(&mut self, x: i32, y: i32, c: u32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let o = self.offset(x, y);
        let bpp = self.fmt.bytes();
        let d = self.data_mut();
        d[o] = c as u8;
        d[o + 1] = (c >> 8) as u8;
        d[o + 2] = (c >> 16) as u8;
        if bpp == 4 {
            d[o + 3] = 0xff;
        }
    }

    pub fn mark(&mut self, r: Rect) {
        let r = r.intersect(&self.bounds());
        self.dirty = self.dirty.union(&r);
    }

    pub fn take_dirty(&mut self) -> Rect {
        std::mem::take(&mut self.dirty)
    }

    pub fn fill(&mut self, r: Rect, c: u32) {
        let r = r.intersect(&self.bounds());
        if r.is_empty() {
            return;
        }
        for y in r.t..r.b {
            for x in r.l..r.r {
                self.put(x, y, c);
            }
        }
        self.mark(r);
    }

    /// Copies `src` over this surface, resizing to `w` x `h` and keeping what fits.
    pub fn resized(&self, w: i32, h: i32, bg: u32) -> Surface {
        let mut n = Surface::new(w, h);
        n.fill(n.bounds(), bg);
        for y in 0..self.h.min(h) {
            for x in 0..self.w.min(w) {
                n.put(x, y, self.get(x, y));
            }
        }
        n.dirty = n.bounds();
        n
    }
}

// ---- raster operations -------------------------------------------------------------------------

/// Binary raster operation (`SetROP2`) applied to a pen or brush colour over the destination.
pub fn rop2(dst: u32, src: u32, mode: i32) -> u32 {
    let m = |v: u32| v & 0xff_ffff;
    match mode {
        1 => 0,
        2 => m(!(dst | src)),
        3 => m(dst & !src),
        4 => m(!src),
        5 => m(src & !dst),
        6 => m(!dst),
        7 => m(dst ^ src),
        8 => m(!(dst & src)),
        9 => m(dst & src),
        10 => m(!(dst ^ src)),
        11 => dst,
        12 => m(dst | !src),
        14 => m(src | !dst),
        15 => m(dst | src),
        16 => 0xff_ffff,
        _ => src,
    }
}

/// Ternary raster operation for the common codes (`BitBlt`/`PatBlt`).
pub fn rop3(dst: u32, src: u32, pat: u32, code: u32) -> u32 {
    let m = |v: u32| v & 0xff_ffff;
    match code {
        0x00CC0020 => src,
        0x00EE0086 => m(src | dst),
        0x008800C6 => m(src & dst),
        0x00660046 => m(src ^ dst),
        0x00440328 => m(src & !dst),
        0x00330008 => m(!src),
        0x001100A6 => m(!(src | dst)),
        0x00BB0226 => m(dst | !src),
        0x00F00021 => pat,
        0x005A0049 => m(pat ^ dst),
        0x000F0001 => m(!pat),
        0x00550009 => m(!dst),
        0x00000042 => 0,
        0x00FF0062 => 0xff_ffff,
        0x00FA0089 => m(pat | dst),
        0x00A000C9 => m(pat & dst),
        0x00B8074A => m((pat & src) | (!pat & dst)),
        _ => src,
    }
}

pub fn blend(dst: u32, src: u32, alpha: u32) -> u32 {
    if alpha >= 255 {
        return src;
    }
    if alpha == 0 {
        return dst;
    }
    let mix = |d: u32, s: u32| (s * alpha + d * (255 - alpha) + 127) / 255;
    let r = mix((dst >> 16) & 0xff, (src >> 16) & 0xff);
    let g = mix((dst >> 8) & 0xff, (src >> 8) & 0xff);
    let b = mix(dst & 0xff, src & 0xff);
    r << 16 | g << 8 | b
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenStyle {
    Solid,
    Dash,
    Dot,
    DashDot,
    DashDotDot,
    Null,
    InsideFrame,
}

#[derive(Clone, Debug)]
pub enum Brush {
    Solid(u32),
    Null,
    /// Hatch pattern index (`HS_*`) and the line colour; the background is left alone.
    Hatch(i32, u32),
    Pattern(Arc<Vec<u32>>, i32, i32),
}

#[derive(Clone, Debug)]
pub struct Pen {
    pub style: PenStyle,
    pub width: i32,
    pub color: u32,
}

/// A drawing target: a surface seen through an origin shift and a clip rectangle.
pub struct Canvas<'a> {
    pub surf: &'a mut Surface,
    /// Added to every logical coordinate to get surface coordinates.
    pub ox: i32,
    pub oy: i32,
    pub clip: Rect,
    pub rop2: i32,
}

impl<'a> Canvas<'a> {
    pub fn new(surf: &'a mut Surface, ox: i32, oy: i32, clip: Rect) -> Canvas<'a> {
        let clip = clip.intersect(&surf.bounds());
        Canvas { surf, ox, oy, clip, rop2: 13 }
    }

    /// Plot in surface coordinates, honouring the clip.
    #[inline]
    pub fn plot_abs(&mut self, x: i32, y: i32, c: u32) {
        if self.clip.contains(x, y) {
            let v = if self.rop2 == 13 { c } else { rop2(self.surf.get(x, y), c, self.rop2) };
            self.surf.put(x, y, v);
        }
    }

    #[inline]
    pub fn plot(&mut self, x: i32, y: i32, c: u32) {
        self.plot_abs(x + self.ox, y + self.oy, c);
    }

    pub fn pixel(&self, x: i32, y: i32) -> u32 {
        self.surf.get(x + self.ox, y + self.oy)
    }

    pub fn fill_rect(&mut self, r: Rect, c: u32) {
        let r = r.normalized().offset(self.ox, self.oy).intersect(&self.clip);
        if r.is_empty() {
            return;
        }
        for y in r.t..r.b {
            for x in r.l..r.r {
                let v = if self.rop2 == 13 { c } else { rop2(self.surf.get(x, y), c, self.rop2) };
                self.surf.put(x, y, v);
            }
        }
        self.surf.mark(r);
    }

    fn brush_color_at(brush: &Brush, x: i32, y: i32, under: u32) -> Option<u32> {
        match brush {
            Brush::Solid(c) => Some(*c),
            Brush::Null => None,
            Brush::Hatch(style, c) => {
                let (xm, ym) = (x.rem_euclid(8), y.rem_euclid(8));
                let on = match style {
                    0 => ym == 4,
                    1 => xm == 4,
                    2 => (xm + ym) % 8 == 0 || (xm + ym) % 8 == 8,
                    3 => (xm - ym).rem_euclid(8) == 0,
                    4 => xm == 4 || ym == 4,
                    5 => (xm - ym).rem_euclid(8) == 0 || (xm + ym) % 8 == 0,
                    _ => false,
                };
                Some(if on { *c } else { under })
            }
            Brush::Pattern(bits, w, h) => {
                if *w <= 0 || *h <= 0 {
                    return None;
                }
                Some(bits[((y.rem_euclid(*h)) * *w + x.rem_euclid(*w)) as usize])
            }
        }
    }

    /// Fills a rectangle with a brush (pattern anchored at the surface origin).
    pub fn fill_rect_brush(&mut self, r: Rect, brush: &Brush) {
        if let Brush::Solid(c) = brush {
            return self.fill_rect(r, *c);
        }
        let r = r.normalized().offset(self.ox, self.oy).intersect(&self.clip);
        for y in r.t..r.b {
            for x in r.l..r.r {
                let under = self.surf.get(x, y);
                if let Some(c) = Self::brush_color_at(brush, x - self.ox, y - self.oy, under) {
                    let v = if self.rop2 == 13 { c } else { rop2(under, c, self.rop2) };
                    self.surf.put(x, y, v);
                }
            }
        }
        self.surf.mark(r);
    }

    fn dash_on(style: PenStyle, step: u32) -> bool {
        match style {
            PenStyle::Dash => step % 24 < 18,
            PenStyle::Dot => step % 6 < 3,
            PenStyle::DashDot => matches!(step % 30, 0..=17 | 21..=23),
            PenStyle::DashDotDot => matches!(step % 36, 0..=17 | 21..=23 | 27..=29),
            PenStyle::Null => false,
            _ => true,
        }
    }

    /// A line from (x0,y0) up to but not including (x1,y1), like `LineTo`.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, pen: &Pen) {
        if pen.style == PenStyle::Null {
            return;
        }
        if pen.width > 1 && pen.style == PenStyle::Solid {
            return self.thick_line(x0, y0, x1, y1, pen.width, pen.color);
        }
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        let mut step = 0u32;
        let mut min = (x0, y0);
        let mut max = (x0, y0);
        loop {
            if x == x1 && y == y1 {
                break;
            }
            if Self::dash_on(pen.style, step) {
                self.plot(x, y, pen.color);
            }
            step += 1;
            min = (min.0.min(x), min.1.min(y));
            max = (max.0.max(x), max.1.max(y));
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
        self.surf.mark(Rect::new(min.0 + self.ox, min.1 + self.oy, max.0 + self.ox + 1, max.1 + self.oy + 1));
    }

    /// Like `line` but includes the end point (used for outlines).
    pub fn line_inclusive(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, pen: &Pen) {
        self.line(x0, y0, x1, y1, pen);
        if pen.style != PenStyle::Null && pen.width <= 1 {
            self.plot(x1, y1, pen.color);
        }
    }

    fn thick_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, width: i32, c: u32) {
        let (fx0, fy0, fx1, fy1) = (x0 as f64, y0 as f64, x1 as f64, y1 as f64);
        let (dx, dy) = (fx1 - fx0, fy1 - fy0);
        let len = (dx * dx + dy * dy).sqrt();
        let hw = width as f64 / 2.0;
        // round cap at the start, body, and a cap at the end (LineTo omits the last pixel only for width 1)
        self.disc(x0, y0, hw, c);
        if len > 0.0 {
            let (nx, ny) = (-dy / len * hw, dx / len * hw);
            let quad = [(fx0 + nx, fy0 + ny), (fx1 + nx, fy1 + ny), (fx1 - nx, fy1 - ny), (fx0 - nx, fy0 - ny)];
            let pts: Vec<(i32, i32)> = quad.iter().map(|&(x, y)| (x.round() as i32, y.round() as i32)).collect();
            self.fill_polygon_color(&pts, c, false);
        }
        self.disc(x1, y1, hw, c);
    }

    fn disc(&mut self, cx: i32, cy: i32, r: f64, c: u32) {
        let ri = r.ceil() as i32;
        for y in -ri..=ri {
            for x in -ri..=ri {
                if ((x * x + y * y) as f64) <= r * r + 0.25 {
                    self.plot(cx + x, cy + y, c);
                }
            }
        }
        self.surf.mark(Rect::new(cx - ri + self.ox, cy - ri + self.oy, cx + ri + 1 + self.ox, cy + ri + 1 + self.oy));
    }

    /// `Rectangle`: interior with the brush, border with the pen, both inside [l,r) x [t,b).
    pub fn rectangle(&mut self, r: Rect, pen: &Pen, brush: &Brush) {
        let r = r.normalized();
        if r.is_empty() {
            return;
        }
        let bw = if pen.style == PenStyle::Null { 0 } else { pen.width.max(1) };
        if !matches!(brush, Brush::Null) {
            self.fill_rect_brush(Rect::new(r.l + bw, r.t + bw, r.r - bw, r.b - bw), brush);
        }
        if bw > 0 {
            if pen.style == PenStyle::Solid || bw > 1 {
                let c = pen.color;
                self.fill_rect(Rect::new(r.l, r.t, r.r, r.t + bw), c);
                self.fill_rect(Rect::new(r.l, r.b - bw, r.r, r.b), c);
                self.fill_rect(Rect::new(r.l, r.t, r.l + bw, r.b), c);
                self.fill_rect(Rect::new(r.r - bw, r.t, r.r, r.b), c);
            } else {
                self.line(r.l, r.t, r.r - 1, r.t, pen);
                self.line(r.r - 1, r.t, r.r - 1, r.b - 1, pen);
                self.line(r.r - 1, r.b - 1, r.l, r.b - 1, pen);
                self.line(r.l, r.b - 1, r.l, r.t, pen);
            }
        }
    }

    pub fn frame_rect(&mut self, r: Rect, c: u32, w: i32) {
        let r = r.normalized();
        self.fill_rect(Rect::new(r.l, r.t, r.r, r.t + w), c);
        self.fill_rect(Rect::new(r.l, r.b - w, r.r, r.b), c);
        self.fill_rect(Rect::new(r.l, r.t, r.l + w, r.b), c);
        self.fill_rect(Rect::new(r.r - w, r.t, r.r, r.b), c);
    }

    /// Ellipse inscribed in [l,r) x [t,b).
    pub fn ellipse(&mut self, r: Rect, pen: &Pen, brush: &Brush) {
        let r = r.normalized();
        if r.is_empty() {
            return;
        }
        let (cx, cy) = ((r.l + r.r) as f64 / 2.0, (r.t + r.b) as f64 / 2.0);
        let (a, b) = (r.width() as f64 / 2.0, r.height() as f64 / 2.0);
        let inside = |x: f64, y: f64| ((x - cx) / a).powi(2) + ((y - cy) / b).powi(2) <= 1.0;
        let bw = if pen.style == PenStyle::Null { 0.0 } else { pen.width.max(1) as f64 };
        for y in r.t..r.b {
            for x in r.l..r.r {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                if !inside(px, py) {
                    continue;
                }
                let edge = bw > 0.0 && {
                    let (ia, ib) = ((a - bw).max(0.0), (b - bw).max(0.0));
                    ia <= 0.0 || ib <= 0.0 || ((px - cx) / ia).powi(2) + ((py - cy) / ib).powi(2) > 1.0
                };
                if edge {
                    self.plot(x, y, pen.color);
                } else if let Some(c) = Self::brush_color_at(brush, x, y, self.pixel(x, y)) {
                    self.plot(x, y, c);
                }
            }
        }
        self.surf.mark(r.offset(self.ox, self.oy));
    }

    pub fn round_rect(&mut self, r: Rect, rw: i32, rh: i32, pen: &Pen, brush: &Brush) {
        let r = r.normalized();
        let (rx, ry) = ((rw / 2).max(0) as f64, (rh / 2).max(0) as f64);
        let bw = if pen.style == PenStyle::Null { 0.0 } else { pen.width.max(1) as f64 };
        let inside = |x: f64, y: f64, rx: f64, ry: f64, rect: &Rect| {
            let (l, t, rr, b) = (rect.l as f64, rect.t as f64, rect.r as f64, rect.b as f64);
            if x < l || x >= rr || y < t || y >= b {
                return false;
            }
            let cxl = l + rx;
            let cxr = rr - rx;
            let cyt = t + ry;
            let cyb = b - ry;
            let (dx, dy) = (if x < cxl { cxl - x } else if x > cxr { x - cxr } else { 0.0 }, if y < cyt { cyt - y } else if y > cyb { y - cyb } else { 0.0 });
            if rx <= 0.0 || ry <= 0.0 {
                return true;
            }
            (dx / rx).powi(2) + (dy / ry).powi(2) <= 1.0
        };
        let inner = Rect::new(r.l + bw as i32, r.t + bw as i32, r.r - bw as i32, r.b - bw as i32);
        for y in r.t..r.b {
            for x in r.l..r.r {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                if !inside(px, py, rx, ry, &r) {
                    continue;
                }
                let on_border = bw > 0.0 && !inside(px, py, (rx - bw).max(0.0), (ry - bw).max(0.0), &inner);
                if on_border {
                    self.plot(x, y, pen.color);
                } else if let Some(c) = Self::brush_color_at(brush, x, y, self.pixel(x, y)) {
                    self.plot(x, y, c);
                }
            }
        }
        self.surf.mark(r.offset(self.ox, self.oy));
    }

    /// Scanline polygon fill; `winding` selects non-zero winding instead of alternate (even-odd).
    pub fn fill_polygon_color(&mut self, pts: &[(i32, i32)], c: u32, winding: bool) {
        self.fill_polygon(pts, &Brush::Solid(c), winding);
    }

    pub fn fill_polygon(&mut self, pts: &[(i32, i32)], brush: &Brush, winding: bool) {
        if pts.len() < 3 {
            return;
        }
        let (miny, maxy) = pts.iter().fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        let mut bounds = Rect::new(i32::MAX, miny, i32::MIN, maxy + 1);
        for y in miny..=maxy {
            let sy = y as f64 + 0.5;
            let mut xs: Vec<(f64, i32)> = Vec::new();
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                if a.1 == b.1 {
                    continue;
                }
                let (lo, hi, dir) = if a.1 < b.1 { (a, b, 1) } else { (b, a, -1) };
                if sy >= lo.1 as f64 && sy < hi.1 as f64 {
                    let t = (sy - lo.1 as f64) / (hi.1 - lo.1) as f64;
                    xs.push((lo.0 as f64 + t * (hi.0 - lo.0) as f64, dir));
                }
            }
            xs.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
            let mut wind = 0;
            for w in 0..xs.len().saturating_sub(1) {
                wind += xs[w].1;
                let inside = if winding { wind != 0 } else { w % 2 == 0 };
                if inside {
                    let (x0, x1) = (xs[w].0.round() as i32, xs[w + 1].0.round() as i32);
                    bounds.l = bounds.l.min(x0);
                    bounds.r = bounds.r.max(x1);
                    for x in x0..x1 {
                        if let Some(col) = Self::brush_color_at(brush, x, y, self.pixel(x, y)) {
                            self.plot(x, y, col);
                        }
                    }
                }
            }
        }
        if bounds.l < bounds.r {
            self.surf.mark(bounds.offset(self.ox, self.oy));
        }
    }

    pub fn polygon(&mut self, pts: &[(i32, i32)], pen: &Pen, brush: &Brush, winding: bool) {
        if !matches!(brush, Brush::Null) {
            self.fill_polygon(pts, brush, winding);
        }
        if pts.len() >= 2 {
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                self.line_inclusive(a.0, a.1, b.0, b.1, pen);
            }
        }
    }

    pub fn polyline(&mut self, pts: &[(i32, i32)], pen: &Pen) {
        for w in pts.windows(2) {
            self.line_inclusive(w[0].0, w[0].1, w[1].0, w[1].1, pen);
        }
    }

    /// Elliptical arc as a polyline between two angles (degrees, counter-clockwise from 3 o'clock).
    pub fn arc_points(r: Rect, start: f64, sweep: f64) -> Vec<(i32, i32)> {
        let (cx, cy) = ((r.l + r.r) as f64 / 2.0, (r.t + r.b) as f64 / 2.0);
        let (a, b) = (r.width() as f64 / 2.0, r.height() as f64 / 2.0);
        let steps = ((a.max(b) * sweep.abs().to_radians()).ceil() as i32).clamp(8, 720);
        (0..=steps)
            .map(|i| {
                let t = (start + sweep * i as f64 / steps as f64).to_radians();
                ((cx + a * t.cos()).round() as i32, (cy - b * t.sin()).round() as i32)
            })
            .collect()
    }

    /// Copies a rectangle between surfaces with a ternary raster operation.
    #[allow(clippy::too_many_arguments)]
    pub fn blit(&mut self, dx: i32, dy: i32, w: i32, h: i32, src: &Surface, sx: i32, sy: i32, code: u32, pat: &Brush) {
        let dest = Rect::new(dx, dy, dx + w, dy + h).offset(self.ox, self.oy).intersect(&self.clip);
        for y in dest.t..dest.b {
            for x in dest.l..dest.r {
                let (ux, uy) = (x - self.ox - dx + sx, y - self.oy - dy + sy);
                let s = if code == 0x00F00021 || code == 0x00000042 || code == 0x00FF0062 || code == 0x005A0049 || code == 0x000F0001 || code == 0x00550009 { 0 } else { src.get(ux, uy) };
                let d = self.surf.get(x, y);
                let p = Self::brush_color_at(pat, x - self.ox, y - self.oy, d).unwrap_or(0);
                self.surf.put(x, y, rop3(d, s, p, code));
            }
        }
        self.surf.mark(dest);
    }

    /// Nearest-neighbour stretch copy.
    #[allow(clippy::too_many_arguments)]
    pub fn stretch(&mut self, dx: i32, dy: i32, dw: i32, dh: i32, src: &Surface, sx: i32, sy: i32, sw: i32, sh: i32, code: u32) {
        if dw == 0 || dh == 0 || sw == 0 || sh == 0 {
            return;
        }
        let (adw, adh) = (dw.abs(), dh.abs());
        let dest = Rect::new(dx.min(dx + dw), dy.min(dy + dh), dx.max(dx + dw), dy.max(dy + dh)).offset(self.ox, self.oy).intersect(&self.clip);
        for y in dest.t..dest.b {
            for x in dest.l..dest.r {
                let (rx, ry) = (x - self.ox - dx.min(dx + dw), y - self.oy - dy.min(dy + dh));
                let fx = if dw < 0 { adw - 1 - rx } else { rx };
                let fy = if dh < 0 { adh - 1 - ry } else { ry };
                let ux = sx + (fx as i64 * sw as i64 / adw as i64) as i32;
                let uy = sy + (fy as i64 * sh as i64 / adh as i64) as i32;
                let s = src.get(ux, uy);
                let d = self.surf.get(x, y);
                self.surf.put(x, y, rop3(d, s, 0, code));
            }
        }
        self.surf.mark(dest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(s: &mut Surface) -> Canvas<'_> {
        let b = s.bounds();
        Canvas::new(s, 0, 0, b)
    }

    fn solid(c: u32) -> Brush {
        Brush::Solid(c)
    }

    fn pen(c: u32, w: i32) -> Pen {
        Pen { style: PenStyle::Solid, width: w, color: c }
    }

    #[test]
    fn colorref_roundtrip() {
        assert_eq!(from_colorref(0x0000_00ff), 0xff0000);
        assert_eq!(from_colorref(0x00ff_0000), 0x0000ff);
        assert_eq!(to_colorref(from_colorref(0x00123456)), 0x00123456);
    }

    #[test]
    fn rectangle_covers_left_top_but_not_right_bottom() {
        let mut s = Surface::new(20, 20);
        s.fill(s.bounds(), 0xffffff);
        canvas(&mut s).rectangle(Rect::new(2, 3, 10, 9), &pen(0, 1), &solid(0xff0000));
        assert_eq!(s.get(2, 3), 0);
        assert_eq!(s.get(9, 8), 0);
        assert_eq!(s.get(10, 8), 0xffffff);
        assert_eq!(s.get(5, 5), 0xff0000);
        assert_eq!(s.get(5, 9), 0xffffff);
    }

    #[test]
    fn line_excludes_its_end_point() {
        let mut s = Surface::new(10, 10);
        s.fill(s.bounds(), 0xffffff);
        canvas(&mut s).line(1, 1, 6, 1, &pen(0, 1));
        assert_eq!(s.get(1, 1), 0);
        assert_eq!(s.get(5, 1), 0);
        assert_eq!(s.get(6, 1), 0xffffff);
    }

    #[test]
    fn diagonal_line_is_connected() {
        let mut s = Surface::new(12, 12);
        s.fill(s.bounds(), 0xffffff);
        canvas(&mut s).line(0, 0, 8, 8, &pen(0, 1));
        for i in 0..8 {
            assert_eq!(s.get(i, i), 0, "pixel {i}");
        }
    }

    #[test]
    fn clipping_keeps_drawing_inside() {
        let mut s = Surface::new(20, 20);
        s.fill(s.bounds(), 0xffffff);
        let mut c = Canvas::new(&mut s, 0, 0, Rect::new(5, 5, 10, 10));
        c.fill_rect(Rect::new(0, 0, 20, 20), 0);
        assert_eq!(s.get(4, 4), 0xffffff);
        assert_eq!(s.get(5, 5), 0);
        assert_eq!(s.get(9, 9), 0);
        assert_eq!(s.get(10, 10), 0xffffff);
    }

    #[test]
    fn origin_shifts_coordinates() {
        let mut s = Surface::new(20, 20);
        s.fill(s.bounds(), 0xffffff);
        let b = s.bounds();
        let mut c = Canvas::new(&mut s, 5, 6, b);
        c.plot(1, 1, 0);
        assert_eq!(s.get(6, 7), 0);
    }

    #[test]
    fn ellipse_fills_centre_and_leaves_corners() {
        let mut s = Surface::new(40, 40);
        s.fill(s.bounds(), 0xffffff);
        canvas(&mut s).ellipse(Rect::new(0, 0, 40, 40), &pen(0, 1), &solid(0x00ff00));
        assert_eq!(s.get(20, 20), 0x00ff00);
        assert_eq!(s.get(0, 0), 0xffffff);
        assert_eq!(s.get(39, 39), 0xffffff);
        assert_eq!(s.get(0, 20), 0);
    }

    #[test]
    fn polygon_triangle_fill() {
        let mut s = Surface::new(30, 30);
        s.fill(s.bounds(), 0xffffff);
        canvas(&mut s).fill_polygon(&[(2, 2), (26, 2), (2, 26)], &solid(0), false);
        assert_eq!(s.get(5, 5), 0);
        assert_eq!(s.get(25, 25), 0xffffff);
    }

    #[test]
    fn blit_with_srcinvert_twice_restores() {
        let mut dst = Surface::new(8, 8);
        dst.fill(dst.bounds(), 0x112233);
        let mut src = Surface::new(8, 8);
        src.fill(src.bounds(), 0xf0f0f0);
        let mut c = canvas(&mut dst);
        c.blit(0, 0, 8, 8, &src, 0, 0, 0x00660046, &Brush::Null);
        assert_ne!(dst.get(1, 1), 0x112233);
        let mut c = canvas(&mut dst);
        c.blit(0, 0, 8, 8, &src, 0, 0, 0x00660046, &Brush::Null);
        assert_eq!(dst.get(1, 1), 0x112233);
    }

    #[test]
    fn stretch_doubles_pixels() {
        let mut src = Surface::new(2, 1);
        src.put(0, 0, 0xff0000);
        src.put(1, 0, 0x0000ff);
        let mut dst = Surface::new(4, 2);
        canvas(&mut dst).stretch(0, 0, 4, 2, &src, 0, 0, 2, 1, 0x00CC0020);
        assert_eq!(dst.get(0, 1), 0xff0000);
        assert_eq!(dst.get(1, 0), 0xff0000);
        assert_eq!(dst.get(2, 1), 0x0000ff);
    }

    #[test]
    fn bottom_up_dib_surface_maps_rows() {
        let mut mem = vec![0u8; 4 * 2 * 2];
        let mut s = unsafe { Surface::external(mem.as_mut_ptr(), 2, 2, PixelFormat::Bgra32, true) };
        s.put(0, 0, 0xff0000);
        assert_eq!(s.get(0, 0), 0xff0000);
        // the top visible row is the last row in memory
        assert_eq!(mem[8 + 2], 0xff);
    }

    #[test]
    fn rop2_xor_twice_is_identity() {
        let d = 0x123456;
        assert_eq!(rop2(rop2(d, 0xabcdef, 7), 0xabcdef, 7), d);
        assert_eq!(rop2(d, 0xabcdef, 13), 0xabcdef);
    }
}
