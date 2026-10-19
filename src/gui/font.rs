//! Fonts: finds TrueType files on the system and rasterizes glyphs with `fontdue`.
//!
//! Windows font names are mapped onto what a Linux system has: Arial, Segoe UI, Tahoma and
//! friends become a sans-serif face, Times New Roman a serif face, Courier New and Consolas a
//! monospaced one. Bold and italic use the matching face when it exists.

use crate::gui::surface::{Canvas, Rect};
use fontdue::{Font, FontSettings};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Family {
    Sans,
    Serif,
    Mono,
}

pub struct Face {
    pub font: Font,
    pub id: usize,
}

struct Index {
    files: Vec<(String, PathBuf)>,
}

fn index() -> &'static Index {
    static IDX: OnceLock<Index> = OnceLock::new();
    IDX.get_or_init(|| {
        let mut files = Vec::new();
        let mut roots: Vec<PathBuf> = vec!["/usr/share/fonts".into(), "/usr/local/share/fonts".into(), "/usr/X11R6/lib/X11/fonts".into(), "/usr/share/X11/fonts".into()];
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(&home).join(".fonts"));
            roots.push(PathBuf::from(&home).join(".local/share/fonts"));
        }
        if let Some(dir) = std::env::var_os("NLE_FONT_DIR") {
            roots.insert(0, PathBuf::from(dir));
        }
        fn walk(dir: &std::path::Path, depth: u32, out: &mut Vec<(String, PathBuf)>) {
            if depth > 5 || out.len() > 4000 {
                return;
            }
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, depth + 1, out);
                } else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    let lower = name.to_ascii_lowercase();
                    if lower.ends_with(".ttf") || lower.ends_with(".otf") {
                        out.push((lower, p.clone()));
                    }
                }
            }
        }
        for r in roots {
            walk(&r, 0, &mut files);
        }
        files.sort();
        Index { files }
    })
}

fn family_names(family: Family) -> &'static [&'static str] {
    match family {
        Family::Sans => &["liberationsans", "arimo", "dejavusans", "notosans", "ubuntu", "cantarell", "freesans", "opensans", "roboto"],
        Family::Serif => &["liberationserif", "tinos", "dejavuserif", "notoserif", "freeserif"],
        Family::Mono => &["liberationmono", "cousine", "dejavusansmono", "notosansmono", "ubuntumono", "freemono"],
    }
}

/// Picks a font file for the family and style, or `None` when the system has no suitable one.
fn find_file(family: Family, bold: bool, italic: bool) -> Option<(PathBuf, bool, bool)> {
    let idx = index();
    let style_ok = |name: &str, bold: bool, italic: bool| {
        let has_bold = name.contains("bold");
        let has_italic = name.contains("italic") || name.contains("oblique");
        has_bold == bold && has_italic == italic
    };
    for want in [(bold, italic), (bold, false), (false, italic), (false, false)] {
        for base in family_names(family) {
            for (name, path) in &idx.files {
                let stem = name.trim_end_matches(".ttf").trim_end_matches(".otf");
                let compact: String = stem.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                if compact.starts_with(base) && !(family == Family::Sans && compact.starts_with("dejavusansmono")) && !(family == Family::Sans && compact.starts_with("liberationsansnarrow")) && style_ok(&compact, want.0, want.1) {
                    return Some((path.clone(), want.0, want.1));
                }
            }
        }
    }
    // Anything at all is better than no text.
    idx.files.first().map(|(_, p)| (p.clone(), false, false))
}

fn faces() -> &'static Mutex<HashMap<(Family, bool, bool), Option<(Arc<Face>, bool, bool)>>> {
    static F: OnceLock<Mutex<HashMap<(Family, bool, bool), Option<(Arc<Face>, bool, bool)>>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The loaded face for a family and style, with whether bold / italic still need to be faked.
pub fn face(family: Family, bold: bool, italic: bool) -> Option<(Arc<Face>, bool, bool)> {
    let mut map = faces().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = map.get(&(family, bold, italic)) {
        return v.clone();
    }
    let id = map.len() + 1;
    let loaded = find_file(family, bold, italic).and_then(|(path, got_bold, got_italic)| {
        let data = std::fs::read(&path).ok()?;
        let font = Font::from_bytes(data, FontSettings::default()).ok()?;
        Some((Arc::new(Face { font, id }), bold && !got_bold, italic && !got_italic))
    });
    map.insert((family, bold, italic), loaded.clone());
    loaded
}

/// Maps a Windows face name (and pitch / family hints) to a family.
pub fn family_for(name: &str, pitch_and_family: u32) -> Family {
    let n = name.to_ascii_lowercase();
    const MONO: &[&str] = &["courier", "consolas", "lucida console", "fixedsys", "terminal", "monospace", "mono", "andale", "monaco", "menlo", "system fixed"];
    const SERIF: &[&str] = &["times", "serif", "georgia", "garamond", "palatino", "book antiqua", "cambria", "century", "minion"];
    if MONO.iter().any(|m| n.contains(m)) && !n.contains("sans serif") {
        return Family::Mono;
    }
    if SERIF.iter().any(|s| n.contains(s)) && !n.contains("sans") {
        return Family::Serif;
    }
    if n.is_empty() {
        // FF_ROMAN = 0x10, FF_MODERN = 0x30; FIXED_PITCH = 1
        if pitch_and_family & 3 == 1 || pitch_and_family & 0xf0 == 0x30 {
            return Family::Mono;
        }
        if pitch_and_family & 0xf0 == 0x10 {
            return Family::Serif;
        }
    }
    Family::Sans
}

#[derive(Clone)]
pub struct Glyph {
    pub xmin: i32,
    pub ymin: i32,
    pub w: usize,
    pub h: usize,
    pub advance: f32,
    pub cov: Vec<u8>,
}

fn glyphs() -> &'static Mutex<HashMap<(usize, u32, char), Arc<Glyph>>> {
    static G: OnceLock<Mutex<HashMap<(usize, u32, char), Arc<Glyph>>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A face at one pixel size, ready to measure and draw.
#[derive(Clone)]
pub struct Sized {
    pub face: Option<Arc<Face>>,
    pub px: f32,
    pub faux_bold: bool,
    pub faux_italic: bool,
    pub ascent: i32,
    pub descent: i32,
    pub avg_width: i32,
    pub max_width: i32,
}

impl Sized {
    pub fn new(family: Family, bold: bool, italic: bool, px: f32) -> Sized {
        let px = px.max(4.0);
        let (face, fb, fi) = match face(family, bold, italic) {
            Some((f, fb, fi)) => (Some(f), fb, fi),
            None => (None, false, false),
        };
        let (ascent, descent) = match &face {
            Some(f) => match f.font.horizontal_line_metrics(px) {
                Some(m) => (m.ascent.ceil() as i32, (-m.descent).ceil() as i32),
                None => ((px * 0.8).ceil() as i32, (px * 0.2).ceil() as i32),
            },
            None => ((px * 0.8).ceil() as i32, (px * 0.2).ceil() as i32),
        };
        let mut s = Sized { face, px, faux_bold: fb, faux_italic: fi, ascent, descent, avg_width: 0, max_width: 0 };
        s.avg_width = s.advance('x').round().max(1.0) as i32;
        s.max_width = s.advance('M').round().max(1.0) as i32 + 1;
        s
    }

    pub fn height(&self) -> i32 {
        self.ascent + self.descent
    }

    /// Pixel size at which a font's ascent plus descent equals `cell_height` (positive `lfHeight`).
    pub fn px_for_cell(family: Family, bold: bool, italic: bool, cell_height: f32) -> f32 {
        let probe = Sized::new(family, bold, italic, 100.0);
        let total = (probe.ascent + probe.descent) as f32;
        if total <= 0.0 {
            return cell_height;
        }
        cell_height * 100.0 / total
    }

    pub fn glyph(&self, c: char) -> Option<Arc<Glyph>> {
        let face = self.face.as_ref()?;
        let key = (face.id, (self.px * 4.0) as u32, c);
        if let Some(g) = glyphs().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Some(g.clone());
        }
        let (m, cov) = face.font.rasterize(c, self.px);
        let g = Arc::new(Glyph { xmin: m.xmin, ymin: m.ymin, w: m.width, h: m.height, advance: m.advance_width, cov });
        glyphs().lock().unwrap_or_else(|e| e.into_inner()).insert(key, g.clone());
        Some(g)
    }

    pub fn advance(&self, c: char) -> f32 {
        match self.glyph(c) {
            Some(g) => g.advance,
            None => self.px * 0.6,
        }
    }

    fn step(&self, c: char) -> i32 {
        self.advance(c).round() as i32 + self.faux_bold as i32
    }

    pub fn text_width(&self, s: &str) -> i32 {
        s.chars().filter(|c| *c != '\n' && *c != '\r').map(|c| if c == '\t' { self.step(' ') * 8 } else { self.step(c) }).sum()
    }

    /// Draws `s` with its top-left at (x, y) in canvas coordinates; returns the advance width.
    pub fn draw(&self, cv: &mut Canvas, x: i32, y: i32, s: &str, color: u32, bk: Option<u32>) -> i32 {
        let width = self.text_width(s);
        if let Some(b) = bk {
            cv.fill_rect(Rect::new(x, y, x + width, y + self.height()), b);
        }
        let baseline = y + self.ascent;
        let mut pen_x = x;
        for c in s.chars() {
            if c == '\n' || c == '\r' {
                continue;
            }
            if c == '\t' {
                pen_x += self.step(' ') * 8;
                continue;
            }
            match self.glyph(c) {
                Some(g) => {
                    let passes = if self.faux_bold { 2 } else { 1 };
                    for pass in 0..passes {
                        for row in 0..g.h {
                            let gy = baseline - (g.ymin + g.h as i32) + row as i32;
                            let slant = if self.faux_italic { ((g.h - row) as i32) / 4 } else { 0 };
                            for col in 0..g.w {
                                let a = g.cov[row * g.w + col] as u32;
                                if a == 0 {
                                    continue;
                                }
                                let gx = pen_x + g.xmin + col as i32 + pass + slant;
                                let under = cv.pixel(gx, gy);
                                cv.plot(gx, gy, crate::gui::surface::blend(under, color, a));
                            }
                        }
                    }
                }
                None => {
                    // No font on this machine: a box per character keeps the layout honest.
                    let w = (self.px * 0.5) as i32;
                    cv.fill_rect(Rect::new(pen_x + 1, baseline - (self.px * 0.7) as i32, pen_x + w, baseline), color);
                }
            }
            pen_x += self.step(c);
        }
        cv.surf.mark(Rect::new(x, y, x + width, y + self.height()).offset(cv.ox, cv.oy));
        width
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::surface::Surface;

    #[test]
    fn family_mapping_follows_windows_names() {
        assert_eq!(family_for("Courier New", 0), Family::Mono);
        assert_eq!(family_for("Consolas", 0), Family::Mono);
        assert_eq!(family_for("Times New Roman", 0), Family::Serif);
        assert_eq!(family_for("MS Sans Serif", 0), Family::Sans);
        assert_eq!(family_for("Segoe UI", 0), Family::Sans);
        assert_eq!(family_for("", 0x30), Family::Mono);
        assert_eq!(family_for("Unheard Of", 0), Family::Sans);
    }

    #[test]
    fn text_has_width_and_draws_something() {
        let f = Sized::new(Family::Sans, false, false, 16.0);
        assert!(f.height() >= 14, "height {}", f.height());
        let w = f.text_width("Hello");
        assert!(w > 20 && w < 80, "width {w}");
        assert!(f.text_width("Hello") > f.text_width("Hi"));
        if f.face.is_none() {
            return; // no fonts installed: nothing more to check
        }
        let mut s = Surface::new(100, 30);
        s.fill(s.bounds(), 0xffffff);
        let b = s.bounds();
        let mut cv = Canvas::new(&mut s, 0, 0, b);
        f.draw(&mut cv, 2, 2, "Hello", 0, None);
        let dark = (0..100).flat_map(|x| (0..30).map(move |y| (x, y))).filter(|&(x, y)| s.get(x, y) < 0x808080).count();
        assert!(dark > 30, "only {dark} dark pixels");
    }

    #[test]
    fn mono_font_has_equal_advances() {
        let f = Sized::new(Family::Mono, false, false, 14.0);
        if f.face.is_none() {
            return;
        }
        assert_eq!(f.text_width("iiii"), f.text_width("WWWW"));
    }

    #[test]
    fn cell_height_sizing_hits_the_target() {
        let px = Sized::px_for_cell(Family::Sans, false, false, 16.0);
        let f = Sized::new(Family::Sans, false, false, px);
        assert!((f.height() - 16).abs() <= 2, "height {}", f.height());
    }
}
