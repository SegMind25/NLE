//! Image lists (`HIMAGELIST`): a set of equally sized images with transparency, used by the list
//! view, tree view, toolbar and tab controls.

use super::gdi::Object;
use super::res::IconImage;
use super::surface::*;
use super::Gui;

pub const ILC_MASK: u32 = 1;
pub const ILD_TRANSPARENT: u32 = 1;
pub const ILD_NORMAL: u32 = 0;

#[derive(Clone, Debug)]
pub struct ImageList {
    pub w: i32,
    pub h: i32,
    pub flags: u32,
    pub bk: u32,
    /// One entry per image: colour and alpha, `w * h` each.
    pub images: Vec<(Vec<u32>, Vec<u8>)>,
}

pub const CLR_NONE: u32 = 0xFFFF_FFFF;

impl ImageList {
    pub fn new(w: i32, h: i32, flags: u32) -> ImageList {
        ImageList { w: w.max(1), h: h.max(1), flags, bk: CLR_NONE, images: Vec::new() }
    }

    /// Adds the images in a bitmap laid out in a row. `transparent` marks pixels (a colour, or a separate mask surface
    /// whose white pixels are transparent) that become see-through. Returns the index of the first image added.
    pub fn add_strip(&mut self, bmp: &Surface, mask: Option<&Surface>, color_key: Option<u32>) -> usize {
        let first = self.images.len();
        let count = (bmp.w / self.w).max(1);
        for n in 0..count {
            let mut rgb = Vec::with_capacity((self.w * self.h) as usize);
            let mut alpha = Vec::with_capacity((self.w * self.h) as usize);
            for y in 0..self.h {
                for x in 0..self.w {
                    let (sx, sy) = (n * self.w + x, y);
                    let c = if sx < bmp.w && sy < bmp.h { bmp.get(sx, sy) } else { 0 };
                    let transparent = match (mask, color_key) {
                        (Some(m), _) => sx < m.w && sy < m.h && m.get(sx, sy) & 0xFFFFFF == 0xFFFFFF,
                        (None, Some(k)) => c & 0xFFFFFF == k & 0xFFFFFF,
                        _ => false,
                    };
                    rgb.push(if transparent { 0 } else { c });
                    alpha.push(if transparent { 0 } else { 255 });
                }
            }
            self.images.push((rgb, alpha));
        }
        first
    }

    /// Adds an icon scaled to the list's image size (nearest neighbour). `at` replaces an existing image.
    pub fn add_icon(&mut self, icon: &IconImage, at: Option<usize>) -> usize {
        let mut rgb = Vec::with_capacity((self.w * self.h) as usize);
        let mut alpha = Vec::with_capacity((self.w * self.h) as usize);
        for y in 0..self.h {
            for x in 0..self.w {
                let i = ((y * icon.h / self.h) * icon.w + x * icon.w / self.w) as usize;
                rgb.push(icon.rgb[i]);
                alpha.push(icon.alpha[i]);
            }
        }
        match at {
            Some(i) if i < self.images.len() => {
                self.images[i] = (rgb, alpha);
                i
            }
            _ => {
                self.images.push((rgb, alpha));
                self.images.len() - 1
            }
        }
    }

    /// An image as an `IconImage` (for `ImageList_GetIcon`).
    pub fn icon(&self, i: usize) -> Option<IconImage> {
        let (rgb, alpha) = self.images.get(i)?;
        Some(IconImage { w: self.w, h: self.h, rgb: rgb.clone(), alpha: alpha.clone(), hotspot: (0, 0) })
    }
}

impl Gui {
    pub fn imagelist(&self, h: usize) -> Option<&ImageList> {
        match self.objects.get(&h)? {
            Object::ImageList(l) => Some(l),
            _ => None,
        }
    }

    pub fn imagelist_mut(&mut self, h: usize) -> Option<&mut ImageList> {
        match self.objects.get_mut(&h)? {
            Object::ImageList(l) => Some(l),
            _ => None,
        }
    }

    /// Draws image `i` of an image list at (x, y), blended over what is there (and over the list's background colour if it has one).
    pub fn draw_imagelist(&mut self, hdc: usize, himl: usize, i: usize, x: i32, y: i32, dw: i32, dh: i32) -> bool {
        self.draw_imagelist_mode(hdc, himl, i, x, y, dw, dh, false)
    }

    /// Like `draw_imagelist`; `disabled` draws the image washed out, as a disabled tool bar button shows it.
    pub fn draw_imagelist_mode(&mut self, hdc: usize, himl: usize, i: usize, x: i32, y: i32, dw: i32, dh: i32, disabled: bool) -> bool {
        let Some(list) = self.imagelist(himl).cloned() else { return false };
        let Some((rgb, alpha)) = list.images.get(i) else { return false };
        let (w, h) = (if dw > 0 { dw } else { list.w }, if dh > 0 { dh } else { list.h });
        self.draw(hdc, |cv, _| {
            for dy in 0..h {
                for dx in 0..w {
                    let k = ((dy * list.h / h) * list.w + dx * list.w / w) as usize;
                    let (px, py) = (x + dx, y + dy);
                    if !cv.clip.contains(px + cv.ox, py + cv.oy) {
                        continue;
                    }
                    let a = alpha[k] as u32;
                    if a == 0 {
                        if list.bk != CLR_NONE {
                            cv.plot(px, py, from_colorref(list.bk));
                        }
                        continue;
                    }
                    let mut src = rgb[k];
                    if disabled {
                        let (r, g, b) = ((src >> 16) & 0xff, (src >> 8) & 0xff, src & 0xff);
                        let y = (r * 30 + g * 59 + b * 11) / 100;
                        let y = 0xA0 + y / 5;
                        src = y << 16 | y << 8 | y;
                    }
                    let c = if a == 255 { src } else { blend(cv.pixel(px, py), src, a) };
                    cv.plot(px, py, c);
                }
            }
        })
        .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip() -> Surface {
        let mut s = Surface::new(8, 4);
        for y in 0..4 {
            for x in 0..8 {
                s.put(x, y, if x < 4 { 0xff0000 } else { 0x00ff00 });
            }
        }
        s
    }

    #[test]
    fn a_strip_becomes_several_images() {
        let mut l = ImageList::new(4, 4, 0);
        assert_eq!(l.add_strip(&strip(), None, None), 0);
        assert_eq!(l.images.len(), 2);
        assert_eq!(l.images[0].0[0], 0xff0000);
        assert_eq!(l.images[1].0[0], 0x00ff00);
        assert!(l.images[1].1.iter().all(|a| *a == 255));
    }

    #[test]
    fn a_colour_key_and_a_mask_make_pixels_transparent() {
        let mut l = ImageList::new(4, 4, ILC_MASK);
        l.add_strip(&strip(), None, Some(0xff0000));
        assert!(l.images[0].1.iter().all(|a| *a == 0), "the first image is all key colour");
        assert!(l.images[1].1.iter().all(|a| *a == 255));
        let mut mask = Surface::new(8, 4);
        mask.fill(Rect::new(0, 0, 8, 4), 0);
        mask.fill(Rect::new(0, 0, 2, 4), 0xffffff);
        let mut m = ImageList::new(8, 4, ILC_MASK);
        m.add_strip(&strip(), Some(&mask), None);
        assert_eq!(m.images[0].1[0], 0);
        assert_eq!(m.images[0].1[2], 255);
    }

    #[test]
    fn an_icon_is_scaled_into_the_list() {
        let icon = IconImage { w: 2, h: 2, rgb: vec![1, 2, 3, 4], alpha: vec![255, 0, 255, 255], hotspot: (0, 0) };
        let mut l = ImageList::new(4, 4, ILC_MASK);
        assert_eq!(l.add_icon(&icon, None), 0);
        assert_eq!(l.images[0].0[0], 1);
        assert_eq!(l.images[0].0[3], 2);
        assert_eq!(l.images[0].1[3], 0);
        assert_eq!(l.add_icon(&icon, Some(0)), 0, "replacing keeps the count");
        assert_eq!(l.images.len(), 1);
        assert_eq!(l.icon(0).unwrap().w, 4);
    }
}
