//! The GDI object model (device contexts, pens, brushes, fonts, bitmaps, regions) and the
//! drawing operations on them. Everything draws through `surface::Canvas`.

use super::font::{family_for, Family, Sized};
use super::surface::*;
use super::Gui;

#[derive(Clone, Debug)]
pub struct LogFont {
    pub height: i32,
    pub width: i32,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub pitch_family: u32,
    pub face: String,
}

impl LogFont {
    pub fn family(&self) -> Family {
        family_for(&self.face, self.pitch_family)
    }

    pub fn sized(&self) -> Sized {
        let family = self.family();
        let bold = self.weight >= 600;
        let px = if self.height < 0 {
            (-self.height) as f32
        } else if self.height > 0 {
            Sized::px_for_cell(family, bold, self.italic, self.height as f32)
        } else {
            16.0
        };
        Sized::new(family, bold, self.italic, px)
    }
}

pub struct Bitmap {
    pub surf: Surface,
    pub bpp: u16,
    /// Pixel memory handed to the program by `CreateDIBSection` (freed with the bitmap), or 0.
    pub bits: usize,
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        if self.bits != 0 {
            unsafe { libc::free(self.bits as *mut libc::c_void) };
        }
    }
}

#[derive(Clone, Debug)]
pub enum Target {
    /// The client area of a window, drawn into its top-level window's surface.
    /// `exclude` holds the parts of the client area that belong to other windows (children, siblings above): clipped away.
    Window { top: usize, ox: i32, oy: i32, clip: Rect, exclude: Vec<Rect> },
    /// Memory DC: draws into the selected bitmap.
    Memory,
    /// The desktop: drawing is accepted and discarded.
    Screen,
}

#[derive(Clone)]
pub struct Saved {
    pen: usize,
    brush: usize,
    font: usize,
    text_color: u32,
    bk_color: u32,
    bk_mode: i32,
    rop2: i32,
    text_align: u32,
    win_org: (i32, i32),
    vp_org: (i32, i32),
    clip: Option<Vec<Rect>>,
}

pub struct Dc {
    pub target: Target,
    pub hwnd: usize,
    pub paint: bool,
    pub pen: usize,
    pub brush: usize,
    pub font: usize,
    pub bitmap: usize,
    pub text_color: u32,
    pub bk_color: u32,
    pub bk_mode: i32,
    pub rop2: i32,
    pub text_align: u32,
    pub poly_fill: i32,
    pub pos: (i32, i32),
    pub win_org: (i32, i32),
    pub vp_org: (i32, i32),
    pub map_mode: i32,
    pub clip: Option<Vec<Rect>>,
    pub dc_pen: u32,
    pub dc_brush: u32,
    pub saved: Vec<Saved>,
}

pub enum Object {
    Pen(Pen),
    Brush(Brush),
    Font(LogFont),
    Bitmap(Bitmap),
    Region(Vec<Rect>),
    Dc(Dc),
    /// An icon or cursor image loaded from a resource or file.
    Icon(std::sync::Arc<super::res::IconImage>),
    ImageList(Box<super::imagelist::ImageList>),
    /// Stock cursors and icons, palettes and other objects that only need to exist.
    Stub(u32),
}

// ---- system colours --------------------------------------------------------------------------------

/// `GetSysColor` values as 0x00RRGGBB (Windows 10 light theme).
pub fn sys_color(index: i32) -> u32 {
    match index {
        0 => 0xC8C8C8,
        1 => 0x000000,
        2 => 0x99B4D1,
        3 => 0xBFCDDB,
        4 => 0xF0F0F0,
        5 => 0xFFFFFF,
        6 => 0x646464,
        7 | 8 | 9 | 18 | 19 | 23 => 0x000000,
        10 => 0xB4B4B4,
        11 => 0xF4F7FC,
        12 => 0xABABAB,
        13 => 0x0078D7,
        14 => 0xFFFFFF,
        15 => 0xF0F0F0,
        16 => 0xA0A0A0,
        17 => 0x6D6D6D,
        20 => 0xFFFFFF,
        21 => 0x696969,
        22 => 0xE3E3E3,
        24 => 0xFFFFE1,
        26 => 0x0066CC,
        27 => 0xB9D1EA,
        28 => 0xD7E4F2,
        29 => 0x3399FF,
        30 => 0xF0F0F0,
        _ => 0xF0F0F0,
    }
}

pub fn init_stock(g: &mut Gui) {
    let add = |g: &mut Gui, idx: i32, obj: Object| {
        let h = g.alloc_handle();
        g.objects.insert(h, obj);
        g.stock.insert(idx, h);
    };
    add(g, 0, Object::Brush(Brush::Solid(0xffffff)));
    add(g, 1, Object::Brush(Brush::Solid(0xc0c0c0)));
    add(g, 2, Object::Brush(Brush::Solid(0x808080)));
    add(g, 3, Object::Brush(Brush::Solid(0x404040)));
    add(g, 4, Object::Brush(Brush::Solid(0x000000)));
    add(g, 5, Object::Brush(Brush::Null));
    add(g, 6, Object::Pen(Pen { style: PenStyle::Solid, width: 1, color: 0xffffff }));
    add(g, 7, Object::Pen(Pen { style: PenStyle::Solid, width: 1, color: 0 }));
    add(g, 8, Object::Pen(Pen { style: PenStyle::Null, width: 1, color: 0 }));
    let font = |face: &str, h: i32, pf: u32| Object::Font(LogFont { height: h, width: 0, weight: 400, italic: false, underline: false, strike: false, pitch_family: pf, face: face.to_string() });
    add(g, 10, font("Terminal", -12, 0x31));
    add(g, 11, font("Courier New", -12, 0x31));
    add(g, 12, font("MS Sans Serif", -12, 0x22));
    add(g, 13, font("System", -16, 0x22));
    add(g, 14, font("System", -16, 0x22));
    add(g, 16, font("Fixedsys", -16, 0x31));
    add(g, 17, font("Segoe UI", -12, 0x22));
    add(g, 15, Object::Stub(15));
    add(g, 18, Object::Brush(Brush::Solid(0xffffff)));
    add(g, 19, Object::Pen(Pen { style: PenStyle::Solid, width: 1, color: 0 }));
}

/// A resolved device-context state: everything a drawing call needs, with the objects looked up.
pub struct View {
    pub target: Target,
    pub bitmap: usize,
    pub pen: Pen,
    pub brush: Brush,
    pub lf: LogFont,
    pub font: Sized,
    pub text_color: u32,
    pub bk_color: u32,
    pub bk_mode: i32,
    pub rop2: i32,
    pub text_align: u32,
    pub poly_fill: i32,
    pub pos: (i32, i32),
    pub ox: i32,
    pub oy: i32,
    pub clips: Vec<Rect>,
}

impl Gui {
    pub fn stock_object(&self, idx: i32) -> usize {
        self.stock.get(&idx).copied().unwrap_or(0)
    }

    pub fn new_dc(&mut self, target: Target, hwnd: usize, paint: bool) -> usize {
        let h = self.alloc_handle();
        let bitmap = if matches!(target, Target::Memory) { self.default_bitmap() } else { 0 };
        let dc = Dc {
            target,
            hwnd,
            paint,
            pen: self.stock_object(7),
            brush: self.stock_object(0),
            font: self.stock_object(13),
            bitmap,
            text_color: 0,
            bk_color: 0xffffff,
            bk_mode: 2,
            rop2: 13,
            text_align: 0,
            poly_fill: 1,
            pos: (0, 0),
            win_org: (0, 0),
            vp_org: (0, 0),
            map_mode: 1,
            clip: None,
            dc_pen: 0,
            dc_brush: 0xffffff,
            saved: Vec::new(),
        };
        self.objects.insert(h, Object::Dc(dc));
        h
    }

    fn default_bitmap(&mut self) -> usize {
        if let Some(h) = self.stock.get(&100) {
            return *h;
        }
        let h = self.alloc_handle();
        self.objects.insert(h, Object::Bitmap(Bitmap { surf: Surface::new(1, 1), bpp: 1, bits: 0 }));
        self.stock.insert(100, h);
        h
    }

    pub fn dc(&self, hdc: usize) -> Option<&Dc> {
        match self.objects.get(&hdc) {
            Some(Object::Dc(d)) => Some(d),
            _ => None,
        }
    }

    pub fn dc_mut(&mut self, hdc: usize) -> Option<&mut Dc> {
        match self.objects.get_mut(&hdc) {
            Some(Object::Dc(d)) => Some(d),
            _ => None,
        }
    }

    pub fn delete_dc(&mut self, hdc: usize) -> bool {
        matches!(self.objects.remove(&hdc), Some(Object::Dc(_)))
    }

    pub fn pen_of(&self, h: usize, dc_pen: u32) -> Pen {
        if h == self.stock_object(19) {
            return Pen { style: PenStyle::Solid, width: 1, color: dc_pen };
        }
        match self.objects.get(&h) {
            Some(Object::Pen(p)) => p.clone(),
            _ => Pen { style: PenStyle::Solid, width: 1, color: 0 },
        }
    }

    pub fn brush_of(&self, h: usize, dc_brush: u32) -> Brush {
        if h == self.stock_object(18) {
            return Brush::Solid(dc_brush);
        }
        match self.objects.get(&h) {
            Some(Object::Brush(b)) => b.clone(),
            _ => Brush::Null,
        }
    }

    /// A class background or `FillRect` brush argument: a brush handle or `COLOR_xxx + 1`.
    pub fn brush_arg(&self, v: usize) -> Brush {
        if v == 0 {
            return Brush::Null;
        }
        if v <= 32 {
            return Brush::Solid(sys_color(v as i32 - 1));
        }
        self.brush_of(v, 0xffffff)
    }

    pub fn view(&self, hdc: usize) -> Option<View> {
        let d = self.dc(hdc)?;
        let lf = match self.objects.get(&d.font) {
            Some(Object::Font(f)) => f.clone(),
            _ => LogFont { height: -12, width: 0, weight: 400, italic: false, underline: false, strike: false, pitch_family: 0x22, face: "Segoe UI".into() },
        };
        let (base_ox, base_oy, base_clip, excluded) = match &d.target {
            Target::Window { ox, oy, clip, exclude, .. } => (*ox, *oy, *clip, exclude.clone()),
            Target::Memory => match self.objects.get(&d.bitmap) {
                Some(Object::Bitmap(b)) => (0, 0, b.surf.bounds(), Vec::new()),
                _ => (0, 0, Rect::default(), Vec::new()),
            },
            Target::Screen => (0, 0, Rect::default(), Vec::new()),
        };
        let ox = base_ox - d.win_org.0 + d.vp_org.0;
        let oy = base_oy - d.win_org.1 + d.vp_org.1;
        // what the window may draw on: its clip without the areas other windows own
        let mut visible = vec![base_clip];
        for e in &excluded {
            visible = visible.into_iter().flat_map(|r| rect_subtract(r, *e)).filter(|r| !r.is_empty()).collect();
        }
        let clips: Vec<Rect> = match &d.clip {
            None => visible,
            Some(list) => list.iter().map(|r| r.offset(base_ox, base_oy)).flat_map(|r| visible.iter().map(move |v| v.intersect(&r))).filter(|r| !r.is_empty()).collect(),
        };
        Some(View {
            target: d.target.clone(),
            bitmap: d.bitmap,
            pen: self.pen_of(d.pen, d.dc_pen),
            brush: self.brush_of(d.brush, d.dc_brush),
            font: lf.sized(),
            lf,
            text_color: from_colorref(d.text_color),
            bk_color: from_colorref(d.bk_color),
            bk_mode: d.bk_mode,
            rop2: d.rop2,
            text_align: d.text_align,
            poly_fill: d.poly_fill,
            pos: d.pos,
            ox,
            oy,
            clips,
        })
    }

    fn target_surface(&mut self, t: &Target, bitmap: usize) -> Option<&mut Surface> {
        match t {
            Target::Window { top, .. } => self.windows.get_mut(top)?.surface.as_mut(),
            Target::Memory => match self.objects.get_mut(&bitmap) {
                Some(Object::Bitmap(b)) => Some(&mut b.surf),
                _ => None,
            },
            Target::Screen => None,
        }
    }

    /// Runs a drawing closure against the DC's surface once per clip rectangle.
    pub fn draw<R>(&mut self, hdc: usize, mut f: impl FnMut(&mut Canvas, &View) -> R) -> Option<R> {
        let view = self.view(hdc)?;
        let (ox, oy, rop) = (view.ox, view.oy, view.rop2);
        let clips = view.clips.clone();
        if crate::runtime::cfg().trace.gdi {
            eprintln!("nle[gdi]: draw hdc={hdc:#x} target={:?} origin=({ox},{oy}) clips={:?}", view.target, clips);
        }
        let surf = self.target_surface(&view.target, view.bitmap)?;
        let mut last = None;
        for clip in clips {
            let mut cv = Canvas::new(surf, ox, oy, clip);
            cv.rop2 = rop;
            last = Some(f(&mut cv, &view));
        }
        last
    }

    /// Copies a rectangle out of a DC's surface (a blit source); coordinates are logical.
    pub fn read_source(&mut self, hdc: usize, x: i32, y: i32, w: i32, h: i32) -> Option<Surface> {
        let view = self.view(hdc)?;
        let (ox, oy) = (view.ox, view.oy);
        let surf = self.target_surface(&view.target, view.bitmap)?;
        let mut out = Surface::new(w.max(0), h.max(0));
        for yy in 0..h {
            for xx in 0..w {
                out.put(xx, yy, surf.get(x + xx + ox, y + yy + oy));
            }
        }
        Some(out)
    }

    pub fn present_target(&mut self, hdc: usize) {
        // Memory and screen DCs have nothing to present; window DCs are flushed by the pump.
        let _ = hdc;
    }

    // ---- object creation -------------------------------------------------------------------------

    pub fn add_object(&mut self, o: Object) -> usize {
        let h = self.alloc_handle();
        self.objects.insert(h, o);
        h
    }

    pub fn create_pen(&mut self, style: i32, width: i32, colorref: u32) -> usize {
        let ps = match style & 0xf {
            1 => PenStyle::Dash,
            2 => PenStyle::Dot,
            3 => PenStyle::DashDot,
            4 => PenStyle::DashDotDot,
            5 => PenStyle::Null,
            6 => PenStyle::InsideFrame,
            _ => PenStyle::Solid,
        };
        let width = if width <= 0 { 1 } else { width };
        // Cosmetic dashed pens are one pixel wide.
        let width = if matches!(ps, PenStyle::Dash | PenStyle::Dot | PenStyle::DashDot | PenStyle::DashDotDot) { 1 } else { width };
        self.add_object(Object::Pen(Pen { style: ps, width, color: from_colorref(colorref) }))
    }

    pub fn select_object(&mut self, hdc: usize, obj: usize) -> usize {
        enum Kind {
            Pen,
            Brush,
            Font,
            Bitmap,
            Other,
        }
        let kind = match self.objects.get(&obj) {
            Some(Object::Pen(_)) => Kind::Pen,
            Some(Object::Brush(_)) => Kind::Brush,
            Some(Object::Font(_)) => Kind::Font,
            Some(Object::Bitmap(_)) => Kind::Bitmap,
            _ => Kind::Other,
        };
        let is_memory = matches!(self.dc(hdc).map(|d| &d.target), Some(Target::Memory));
        let Some(d) = self.dc_mut(hdc) else { return 0 };
        match kind {
            Kind::Pen => std::mem::replace(&mut d.pen, obj),
            Kind::Brush => std::mem::replace(&mut d.brush, obj),
            Kind::Font => std::mem::replace(&mut d.font, obj),
            Kind::Bitmap if is_memory => std::mem::replace(&mut d.bitmap, obj),
            _ => 0,
        }
    }

    pub fn delete_object(&mut self, h: usize) -> bool {
        if self.stock.values().any(|s| *s == h) {
            return true; // stock objects are never freed
        }
        self.objects.remove(&h).is_some()
    }

    // ---- state ----------------------------------------------------------------------------------

    pub fn save_dc(&mut self, hdc: usize) -> i32 {
        let Some(d) = self.dc_mut(hdc) else { return 0 };
        d.saved.push(Saved { pen: d.pen, brush: d.brush, font: d.font, text_color: d.text_color, bk_color: d.bk_color, bk_mode: d.bk_mode, rop2: d.rop2, text_align: d.text_align, win_org: d.win_org, vp_org: d.vp_org, clip: d.clip.clone() });
        d.saved.len() as i32
    }

    pub fn restore_dc(&mut self, hdc: usize, level: i32) -> bool {
        let Some(d) = self.dc_mut(hdc) else { return false };
        let n = d.saved.len() as i32;
        let idx = if level < 0 { n + level } else { level - 1 };
        if idx < 0 || idx >= n {
            return false;
        }
        let s = d.saved[idx as usize].clone();
        d.saved.truncate(idx as usize);
        d.pen = s.pen;
        d.brush = s.brush;
        d.font = s.font;
        d.text_color = s.text_color;
        d.bk_color = s.bk_color;
        d.bk_mode = s.bk_mode;
        d.rop2 = s.rop2;
        d.text_align = s.text_align;
        d.win_org = s.win_org;
        d.vp_org = s.vp_org;
        d.clip = s.clip;
        true
    }

    pub fn intersect_clip(&mut self, hdc: usize, r: Rect) -> i32 {
        let Some(d) = self.dc_mut(hdc) else { return 0 };
        let list = d.clip.take().unwrap_or_else(|| vec![Rect::new(-1_000_000, -1_000_000, 1_000_000, 1_000_000)]);
        let out: Vec<Rect> = list.iter().map(|c| c.intersect(&r.normalized())).filter(|c| !c.is_empty()).collect();
        let empty = out.is_empty();
        d.clip = Some(out);
        if empty {
            1
        } else {
            2
        }
    }

    pub fn clip_box(&self, hdc: usize) -> Option<Rect> {
        let d = self.dc(hdc)?;
        match &d.clip {
            Some(list) => Some(list.iter().fold(Rect::default(), |a, r| a.union(r))),
            None => match &d.target {
                Target::Window { ox, oy, clip, .. } => Some(clip.offset(-ox, -oy)),
                Target::Memory => match self.objects.get(&d.bitmap) {
                    Some(Object::Bitmap(b)) => Some(b.surf.bounds()),
                    _ => Some(Rect::default()),
                },
                Target::Screen => Some(Rect::default()),
            },
        }
    }

    // ---- drawing ---------------------------------------------------------------------------------

    pub fn line_to(&mut self, hdc: usize, x: i32, y: i32) -> bool {
        let Some(from) = self.dc(hdc).map(|d| d.pos) else { return false };
        let ok = self.draw(hdc, |cv, v| cv.line(from.0, from.1, x, y, &v.pen)).is_some();
        if let Some(d) = self.dc_mut(hdc) {
            d.pos = (x, y);
        }
        ok
    }

    pub fn rectangle(&mut self, hdc: usize, r: Rect) -> bool {
        self.draw(hdc, |cv, v| cv.rectangle(r, &v.pen, &v.brush)).is_some()
    }

    pub fn round_rect(&mut self, hdc: usize, r: Rect, w: i32, h: i32) -> bool {
        self.draw(hdc, |cv, v| cv.round_rect(r, w, h, &v.pen, &v.brush)).is_some()
    }

    pub fn ellipse(&mut self, hdc: usize, r: Rect) -> bool {
        self.draw(hdc, |cv, v| cv.ellipse(r, &v.pen, &v.brush)).is_some()
    }

    pub fn polygon(&mut self, hdc: usize, pts: &[(i32, i32)]) -> bool {
        self.draw(hdc, |cv, v| cv.polygon(pts, &v.pen, &v.brush, v.poly_fill == 2)).is_some()
    }

    pub fn polyline(&mut self, hdc: usize, pts: &[(i32, i32)]) -> bool {
        self.draw(hdc, |cv, v| cv.polyline(pts, &v.pen)).is_some()
    }

    /// `Arc`, `Pie` and `Chord` from the bounding box and the two radial end points.
    pub fn arc(&mut self, hdc: usize, r: Rect, start: (i32, i32), end: (i32, i32), kind: i32) -> bool {
        let (cx, cy) = ((r.l + r.r) as f64 / 2.0, (r.t + r.b) as f64 / 2.0);
        let ang = |p: (i32, i32)| (-(p.1 as f64 - cy)).atan2(p.0 as f64 - cx).to_degrees();
        let a0 = ang(start);
        let mut sweep = ang(end) - a0;
        if sweep <= 0.0 {
            sweep += 360.0;
        }
        let pts = Canvas::arc_points(r.normalized(), a0, sweep);
        self.draw(hdc, |cv, v| match kind {
            0 => cv.polyline(&pts, &v.pen),
            1 => {
                let mut poly = vec![((cx as i32), (cy as i32))];
                poly.extend(pts.iter().copied());
                cv.polygon(&poly, &v.pen, &v.brush, false)
            }
            _ => cv.polygon(&pts, &v.pen, &v.brush, false),
        })
        .is_some()
    }

    pub fn fill_rect(&mut self, hdc: usize, r: Rect, brush: &Brush) -> bool {
        self.draw(hdc, |cv, _| cv.fill_rect_brush(r, brush)).is_some()
    }

    pub fn set_pixel(&mut self, hdc: usize, x: i32, y: i32, colorref: u32) -> bool {
        self.draw(hdc, |cv, _| {
            cv.plot(x, y, from_colorref(colorref));
            cv.surf.mark(Rect::new(x + cv.ox, y + cv.oy, x + cv.ox + 1, y + cv.oy + 1));
        })
        .is_some()
    }

    pub fn get_pixel(&mut self, hdc: usize, x: i32, y: i32) -> u32 {
        let Some(v) = self.view(hdc) else { return 0xffff_ffff };
        let (ox, oy) = (v.ox, v.oy);
        let clip = v.clips.iter().any(|c| c.contains(x + ox, y + oy));
        let Some(s) = self.target_surface(&v.target, v.bitmap) else { return 0xffff_ffff };
        if !clip {
            return 0xffff_ffff;
        }
        to_colorref(s.get(x + ox, y + oy))
    }

    pub fn pat_blt(&mut self, hdc: usize, x: i32, y: i32, w: i32, h: i32, code: u32) -> bool {
        let pattern_only = Surface::new(1, 1);
        self.draw(hdc, |cv, v| cv.blit(x, y, w, h, &pattern_only, 0, 0, code, &v.brush)).is_some()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn bit_blt(&mut self, dst: usize, dx: i32, dy: i32, w: i32, h: i32, src: usize, sx: i32, sy: i32, code: u32) -> bool {
        let source = if src == 0 { Surface::new(w.max(0), h.max(0)) } else { self.read_source(src, sx, sy, w, h).unwrap_or_else(|| Surface::new(w.max(0), h.max(0))) };
        self.draw(dst, |cv, v| cv.blit(dx, dy, w, h, &source, 0, 0, code, &v.brush)).is_some()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stretch_blt(&mut self, dst: usize, dx: i32, dy: i32, dw: i32, dh: i32, src: usize, sx: i32, sy: i32, sw: i32, sh: i32, code: u32) -> bool {
        let source = match self.read_source(src, sx.min(sx + sw), sy.min(sy + sh), sw.abs(), sh.abs()) {
            Some(s) => s,
            None => return false,
        };
        // A negative source extent mirrors the image; the copy already holds the normalised rectangle.
        let flip_x = sw < 0;
        let flip_y = sh < 0;
        let (dw2, dh2) = (if flip_x { -dw } else { dw }, if flip_y { -dh } else { dh });
        self.draw(dst, |cv, _| cv.stretch(dx, dy, dw2, dh2, &source, 0, 0, sw.abs(), sh.abs(), code)).is_some()
    }

    pub fn text_out(&mut self, hdc: usize, x: i32, y: i32, text: &str, opaque_rect: Option<Rect>) -> bool {
        let Some(v) = self.view(hdc) else { return false };
        let update_cp = v.text_align & 1 != 0;
        let (x, y) = if update_cp { v.pos } else { (x, y) };
        let w = v.font.text_width(text);
        let h = v.font.height();
        let left = match v.text_align & 6 {
            2 => x - w,
            6 => x - w / 2,
            _ => x,
        };
        let top = match v.text_align & 24 {
            8 => y - h,
            24 => y - v.font.ascent,
            _ => y,
        };
        let bk = if v.bk_mode == 2 { Some(v.bk_color) } else { None };
        let ok = self
            .draw(hdc, |cv, v| {
                if let Some(r) = opaque_rect {
                    cv.fill_rect(r, v.bk_color);
                }
                let end = v.font.draw(cv, left, top, text, v.text_color, if opaque_rect.is_some() { None } else { bk });
                if v.lf.underline {
                    cv.fill_rect(Rect::new(left, top + v.font.ascent + 1, left + end, top + v.font.ascent + 2), v.text_color);
                }
                if v.lf.strike {
                    cv.fill_rect(Rect::new(left, top + v.font.ascent * 2 / 3, left + end, top + v.font.ascent * 2 / 3 + 1), v.text_color);
                }
            })
            .is_some();
        if update_cp {
            if let Some(d) = self.dc_mut(hdc) {
                d.pos = (left + w, y);
            }
        }
        ok
    }

    /// `DrawText`: formats `text` inside `rect` and returns the height of the drawn text.
    pub fn draw_text(&mut self, hdc: usize, text: &str, rect: &mut Rect, flags: u32) -> i32 {
        const DT_CENTER: u32 = 1;
        const DT_RIGHT: u32 = 2;
        const DT_VCENTER: u32 = 4;
        const DT_BOTTOM: u32 = 8;
        const DT_WORDBREAK: u32 = 0x10;
        const DT_SINGLELINE: u32 = 0x20;
        const DT_NOCLIP: u32 = 0x100;
        const DT_CALCRECT: u32 = 0x400;
        const DT_NOPREFIX: u32 = 0x800;
        const DT_END_ELLIPSIS: u32 = 0x8000;
        let Some(v) = self.view(hdc) else { return 0 };
        let font = &v.font;
        // mnemonic prefixes: "&x" underlines x, "&&" is a literal ampersand
        let mut clean = String::new();
        let mut underline_at: Vec<usize> = Vec::new();
        if flags & DT_NOPREFIX != 0 {
            clean.push_str(text);
        } else {
            let mut it = text.chars().peekable();
            while let Some(c) = it.next() {
                if c == '&' {
                    match it.peek() {
                        Some('&') => {
                            clean.push('&');
                            it.next();
                        }
                        Some(_) => underline_at.push(clean.chars().count()),
                        None => {}
                    }
                } else {
                    clean.push(c);
                }
            }
        }
        let max_w = rect.width();
        let mut lines: Vec<String> = Vec::new();
        if flags & DT_SINGLELINE != 0 {
            lines.push(clean.replace(['\r', '\n'], " "));
        } else {
            for raw in clean.split('\n') {
                let raw = raw.trim_end_matches('\r');
                if flags & DT_WORDBREAK == 0 || max_w <= 0 {
                    lines.push(raw.to_string());
                    continue;
                }
                let mut cur = String::new();
                for word in raw.split(' ') {
                    let trial = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
                    if font.text_width(&trial) <= max_w || cur.is_empty() {
                        cur = trial;
                    } else {
                        lines.push(std::mem::take(&mut cur));
                        cur = word.to_string();
                    }
                }
                lines.push(cur);
            }
        }
        let lh = font.height();
        let total_h = lh * lines.len() as i32;
        if flags & DT_CALCRECT != 0 {
            let widest = lines.iter().map(|l| font.text_width(l)).max().unwrap_or(0);
            if flags & DT_WORDBREAK == 0 || max_w <= 0 {
                rect.r = rect.l + widest;
            }
            rect.b = rect.t + total_h;
            return total_h;
        }
        let top = if flags & DT_VCENTER != 0 {
            rect.t + (rect.height() - total_h) / 2
        } else if flags & DT_BOTTOM != 0 {
            rect.b - total_h
        } else {
            rect.t
        };
        let r = *rect;
        let bk = if v.bk_mode == 2 { Some(v.bk_color) } else { None };
        let clip_rect = if flags & DT_NOCLIP != 0 { None } else { Some(r) };
        let mut y = top;
        let mut char_base = 0usize;
        let lines_c = lines.clone();
        self.draw(hdc, |cv, v| {
            let saved = cv.clip;
            if let Some(c) = clip_rect {
                cv.clip = cv.clip.intersect(&c.offset(cv.ox, cv.oy));
            }
            let mut y = top;
            let mut base = 0usize;
            for line in &lines_c {
                let mut shown = line.clone();
                if flags & DT_END_ELLIPSIS != 0 && v.font.text_width(&shown) > max_w && max_w > 0 {
                    while !shown.is_empty() && v.font.text_width(&format!("{shown}...")) > max_w {
                        shown.pop();
                    }
                    shown.push_str("...");
                }
                let w = v.font.text_width(&shown);
                let x = if flags & DT_CENTER != 0 {
                    r.l + (r.width() - w) / 2
                } else if flags & DT_RIGHT != 0 {
                    r.r - w
                } else {
                    r.l
                };
                v.font.draw(cv, x, y, &shown, v.text_color, bk);
                for &u in &underline_at {
                    if u >= base && u < base + line.chars().count() {
                        let pre: String = shown.chars().take(u - base).collect();
                        let ux = x + v.font.text_width(&pre);
                        let cw = shown.chars().nth(u - base).map(|c| v.font.text_width(&c.to_string())).unwrap_or(0);
                        cv.fill_rect(Rect::new(ux, y + v.font.ascent + 1, ux + cw, y + v.font.ascent + 2), v.text_color);
                    }
                }
                base += line.chars().count() + 1;
                y += lh;
            }
            cv.clip = saved;
        });
        let _ = (&mut y, &mut char_base);
        total_h
    }

    pub fn text_extent(&self, hdc: usize, text: &str) -> (i32, i32) {
        match self.view(hdc) {
            Some(v) => (v.font.text_width(text), v.font.height()),
            None => (0, 0),
        }
    }
}

impl Gui {
    /// The rendered font behind a font handle (the default UI font when it is not one).
    pub fn sized_font(&self, font: usize) -> Sized {
        match self.objects.get(&font) {
            Some(Object::Font(f)) => f.sized(),
            _ => LogFont { height: -12, width: 0, weight: 400, italic: false, underline: false, strike: false, pitch_family: 0x22, face: "Segoe UI".into() }.sized(),
        }
    }

    /// Draws an icon with its transparency at `(x, y)`, scaled to `cw` x `ch` when both are positive.
    pub fn draw_icon(&mut self, hdc: usize, x: i32, y: i32, icon: usize, cw: i32, ch: i32) -> bool {
        let img = match self.objects.get(&icon) {
            Some(Object::Icon(i)) => i.clone(),
            // a stock icon has no image: leave the area as it is
            Some(Object::Stub(_)) => return true,
            _ => return false,
        };
        let (dw, dh) = if cw > 0 && ch > 0 { (cw, ch) } else { (img.w, img.h) };
        self.draw(hdc, |cv, _| {
            for dy in 0..dh {
                for dx in 0..dw {
                    let i = ((dy * img.h / dh) * img.w + dx * img.w / dw) as usize;
                    let a = img.alpha[i] as u32;
                    if a == 0 || !cv.clip.contains(x + dx + cv.ox, y + dy + cv.oy) {
                        continue;
                    }
                    let c = if a == 255 { img.rgb[i] } else { blend(cv.pixel(x + dx, y + dy), img.rgb[i], a) };
                    cv.plot(x + dx, y + dy, c);
                }
            }
        })
        .is_some()
    }
}

// ---- device-independent bitmaps ---------------------------------------------------------------------

/// What a `BITMAPINFO` says about its pixels.
#[derive(Clone, Copy, Debug)]
pub struct DibInfo {
    pub width: i32,
    pub height: i32,
    pub bottom_up: bool,
    pub bpp: u16,
    pub compression: u32,
    pub colors: usize,
    pub header: usize,
}

pub unsafe fn dib_info(bmi: *const u8) -> Option<DibInfo> {
    let size = *(bmi as *const u32) as usize;
    if size < 12 {
        return None;
    }
    let (width, height, bpp, compression, clr_used) = if size == 12 {
        (*(bmi.add(4) as *const u16) as i32, *(bmi.add(6) as *const u16) as i32, *(bmi.add(10) as *const u16), 0, 0)
    } else {
        (*(bmi.add(4) as *const i32), *(bmi.add(8) as *const i32), *(bmi.add(14) as *const u16), *(bmi.add(16) as *const u32), *(bmi.add(32) as *const u32) as usize)
    };
    let colors = if bpp <= 8 { if clr_used != 0 { clr_used } else { 1usize << bpp } } else { 0 };
    Some(DibInfo { width, height: height.abs(), bottom_up: height > 0, bpp, compression, colors, header: size })
}

/// Decodes DIB pixel data (1, 4, 8, 16, 24 or 32 bits per pixel) into a surface, top row first.
pub unsafe fn dib_to_surface(bmi: *const u8, bits: *const u8) -> Option<Surface> {
    let info = dib_info(bmi)?;
    if info.width <= 0 || info.height <= 0 || info.width > 32768 || info.height > 32768 {
        return None;
    }
    let palette_entry = if info.header == 12 { 3 } else { 4 };
    let palette = bmi.add(info.header + if info.compression == 3 && info.header == 40 { 12 } else { 0 });
    let stride = (info.width as usize * info.bpp as usize).div_ceil(32) * 4;
    let mut s = Surface::new(info.width, info.height);
    for y in 0..info.height {
        let src_row = if info.bottom_up { info.height - 1 - y } else { y };
        let row = bits.add(src_row as usize * stride);
        for x in 0..info.width {
            let xi = x as usize;
            let c = match info.bpp {
                32 => rgb(*row.add(xi * 4 + 2), *row.add(xi * 4 + 1), *row.add(xi * 4)),
                24 => rgb(*row.add(xi * 3 + 2), *row.add(xi * 3 + 1), *row.add(xi * 3)),
                16 => {
                    let v = *(row.add(xi * 2) as *const u16) as u32;
                    rgb((((v >> 10) & 31) * 255 / 31) as u8, (((v >> 5) & 31) * 255 / 31) as u8, ((v & 31) * 255 / 31) as u8)
                }
                8 | 4 | 1 => {
                    let idx = match info.bpp {
                        8 => *row.add(xi) as usize,
                        4 => ((*row.add(xi / 2) >> if xi % 2 == 0 { 4 } else { 0 }) & 15) as usize,
                        _ => ((*row.add(xi / 8) >> (7 - xi % 8)) & 1) as usize,
                    };
                    if idx < info.colors {
                        let e = palette.add(idx * palette_entry);
                        rgb(*e.add(2), *e.add(1), *e)
                    } else {
                        0
                    }
                }
                _ => return None,
            };
            s.put(x, y, c);
        }
    }
    s.dirty = Rect::default();
    Some(s)
}

// ---- regions ----------------------------------------------------------------------------------------

pub fn rect_subtract(a: Rect, b: Rect) -> Vec<Rect> {
    let i = a.intersect(&b);
    if i.is_empty() {
        return vec![a];
    }
    let mut out = Vec::new();
    if i.t > a.t {
        out.push(Rect::new(a.l, a.t, a.r, i.t));
    }
    if i.b < a.b {
        out.push(Rect::new(a.l, i.b, a.r, a.b));
    }
    if i.l > a.l {
        out.push(Rect::new(a.l, i.t, i.l, i.b));
    }
    if i.r < a.r {
        out.push(Rect::new(i.r, i.t, a.r, i.b));
    }
    out
}

/// `CombineRgn` on rectangle lists.
pub fn combine(mode: i32, a: &[Rect], b: &[Rect]) -> Vec<Rect> {
    match mode {
        1 => a.iter().flat_map(|x| b.iter().map(move |y| x.intersect(y))).filter(|r| !r.is_empty()).collect(),
        2 => a.iter().chain(b.iter()).copied().collect(),
        3 => {
            let mut out = combine(4, a, b);
            out.extend(combine(4, b, a));
            out
        }
        4 => {
            let mut cur: Vec<Rect> = a.to_vec();
            for sub in b {
                cur = cur.into_iter().flat_map(|r| rect_subtract(r, *sub)).collect();
            }
            cur
        }
        _ => a.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::{Gui, WM_PAINT};

    fn memory_dc(g: &mut Gui, w: i32, h: i32) -> usize {
        let dc = g.new_dc(Target::Memory, 0, false);
        let bmp = g.add_object(Object::Bitmap(Bitmap { surf: Surface::new(w, h), bpp: 32, bits: 0 }));
        g.select_object(dc, bmp);
        dc
    }

    #[test]
    fn stock_objects_exist_and_survive_deletion() {
        let mut g = Gui::new();
        let white = g.stock_object(0);
        assert!(matches!(g.objects.get(&white), Some(Object::Brush(Brush::Solid(0xffffff)))));
        assert!(g.delete_object(white));
        assert!(g.objects.contains_key(&white));
        let _ = WM_PAINT;
    }

    #[test]
    fn select_object_returns_the_previous_one() {
        let mut g = Gui::new();
        let dc = memory_dc(&mut g, 10, 10);
        let red = g.add_object(Object::Brush(Brush::Solid(0xff0000)));
        let old = g.select_object(dc, red);
        assert_eq!(old, g.stock_object(0));
        assert_eq!(g.select_object(dc, old), red);
    }

    #[test]
    fn drawing_into_a_memory_dc_changes_its_bitmap() {
        let mut g = Gui::new();
        let dc = memory_dc(&mut g, 20, 20);
        let brush = g.add_object(Object::Brush(Brush::Solid(0x00ff00)));
        g.select_object(dc, brush);
        assert!(g.rectangle(dc, Rect::new(2, 2, 12, 12)));
        // COLORREF is BGR: a pure green brush reads back as 0x0000FF00
        assert_eq!(g.get_pixel(dc, 5, 5), 0x0000_ff00);
        assert_eq!(g.get_pixel(dc, 19, 19), 0xffffff, "bitmaps start white in this implementation");
    }

    #[test]
    fn colorref_red_is_low_byte() {
        let mut g = Gui::new();
        let dc = memory_dc(&mut g, 4, 4);
        g.set_pixel(dc, 1, 1, 0x0000_00ff);
        assert_eq!(g.get_pixel(dc, 1, 1), 0x0000_00ff);
        let s = g.read_source(dc, 0, 0, 4, 4).unwrap();
        assert_eq!(s.get(1, 1), 0xff0000);
    }

    #[test]
    fn save_and_restore_dc() {
        let mut g = Gui::new();
        let dc = memory_dc(&mut g, 4, 4);
        let level = g.save_dc(dc);
        g.dc_mut(dc).unwrap().text_color = 0x123456;
        assert!(g.restore_dc(dc, level));
        assert_eq!(g.dc(dc).unwrap().text_color, 0);
    }

    #[test]
    fn clip_rect_limits_drawing() {
        let mut g = Gui::new();
        let dc = memory_dc(&mut g, 20, 20);
        g.intersect_clip(dc, Rect::new(5, 5, 10, 10));
        let brush = g.add_object(Object::Brush(Brush::Solid(0)));
        g.fill_rect(dc, Rect::new(0, 0, 20, 20), &Brush::Solid(0));
        let _ = brush;
        assert_eq!(g.get_pixel(dc, 7, 7), 0);
        assert_eq!(g.get_pixel(dc, 2, 2), 0xffff_ffff, "outside the clip reads as CLR_INVALID");
    }

    #[test]
    fn text_out_blackens_pixels_when_a_font_exists() {
        let mut g = Gui::new();
        let dc = memory_dc(&mut g, 120, 30);
        g.fill_rect(dc, Rect::new(0, 0, 120, 30), &Brush::Solid(0xffffff));
        g.text_out(dc, 2, 2, "NLE", None);
        let s = g.read_source(dc, 0, 0, 120, 30).unwrap();
        let dark = (0..120).flat_map(|x| (0..30).map(move |y| (x, y))).filter(|&(x, y)| s.get(x, y) < 0x808080).count();
        let font_ok = crate::gui::font::Sized::new(Family::Sans, false, false, 12.0).face.is_some();
        assert!(!font_ok || dark > 15, "dark pixels: {dark}");
    }

    #[test]
    fn region_combine_follows_set_rules() {
        let a = [Rect::new(0, 0, 10, 10)];
        let b = [Rect::new(5, 5, 15, 15)];
        assert_eq!(combine(1, &a, &b), vec![Rect::new(5, 5, 10, 10)]);
        let diff = combine(4, &a, &b);
        let area: i32 = diff.iter().map(|r| r.width() * r.height()).sum();
        assert_eq!(area, 100 - 25);
    }

    #[test]
    fn draw_text_calcrect_reports_the_size() {
        let mut g = Gui::new();
        let dc = memory_dc(&mut g, 100, 100);
        let mut r = Rect::new(0, 0, 100, 0);
        let h = g.draw_text(dc, "one\ntwo\nthree", &mut r, 0x400);
        assert_eq!(r.height(), h);
        assert!(h >= 3 * 10);
    }
}

impl Gui {
    /// `TransparentBlt` / constant-alpha `AlphaBlend` from a copy of the source.
    #[allow(clippy::too_many_arguments)]
    pub fn blend_blt(&mut self, dst: usize, dx: i32, dy: i32, dw: i32, dh: i32, src: usize, sx: i32, sy: i32, sw: i32, sh: i32, key: Option<u32>, alpha: u32) -> bool {
        let Some(source) = self.read_source(src, sx, sy, sw, sh) else { return false };
        if sw <= 0 || sh <= 0 || dw <= 0 || dh <= 0 {
            return false;
        }
        self.draw(dst, |cv, _| {
            for y in 0..dh {
                for x in 0..dw {
                    let s = source.get(x * sw / dw, y * sh / dh);
                    if key == Some(s) {
                        continue;
                    }
                    let under = cv.pixel(dx + x, dy + y);
                    cv.plot(dx + x, dy + y, blend(under, s, alpha));
                }
            }
            cv.surf.mark(Rect::new(dx + cv.ox, dy + cv.oy, dx + dw + cv.ox, dy + dh + cv.oy));
        })
        .is_some()
    }
}
