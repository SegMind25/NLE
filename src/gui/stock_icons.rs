//! The icons Windows has built in (`IDI_APPLICATION`, `IDI_ERROR`, ...), drawn in code at 32x32 so that programs which put
//! them in image lists, draw them or give them to a window class get real pixels.

use super::res::IconImage;
use super::surface::*;

const KEY: u32 = 0xFF00FF;

/// The icon for a stock identifier (`IDI_*`); unknown identifiers get the application icon.
pub fn stock_icon(id: u32) -> IconImage {
    let mut s = Surface::new(32, 32);
    s.fill(s.bounds(), KEY);
    let bounds = s.bounds();
    {
        let mut cv = Canvas::new(&mut s, 0, 0, bounds);
        let none = Pen { style: PenStyle::Null, width: 1, color: 0 };
        let white = 0xFFFFFF;
        match id {
            // IDI_ERROR / IDI_HAND
            32513 => {
                cv.ellipse(Rect::new(1, 1, 31, 31), &none, &Brush::Solid(0xE81123));
                let pen = Pen { style: PenStyle::Solid, width: 3, color: white };
                cv.line(10, 10, 22, 22, &pen);
                cv.line(22, 10, 10, 22, &pen);
            }
            // IDI_WARNING / IDI_EXCLAMATION
            32515 => {
                cv.polygon(&[(16, 1), (31, 29), (1, 29)], &Pen { style: PenStyle::Solid, width: 1, color: 0x7A6200 }, &Brush::Solid(0xFFD42A), false);
                cv.fill_rect(Rect::new(15, 10, 17, 20), 0x222222);
                cv.fill_rect(Rect::new(15, 22, 17, 25), 0x222222);
            }
            // IDI_INFORMATION / IDI_ASTERISK
            32516 => {
                cv.ellipse(Rect::new(1, 1, 31, 31), &none, &Brush::Solid(0x0078D7));
                cv.fill_rect(Rect::new(15, 7, 18, 10), white);
                cv.fill_rect(Rect::new(15, 13, 18, 25), white);
            }
            // IDI_QUESTION
            32514 => {
                cv.ellipse(Rect::new(1, 1, 31, 31), &none, &Brush::Solid(0x0078D7));
                let big = super::font::Sized::new(super::font::Family::Sans, true, false, 24.0);
                let w = big.text_width("?");
                big.draw(&mut cv, (32 - w) / 2, 2, "?", white, None);
            }
            // IDI_SHIELD
            32518 => {
                cv.polygon(&[(16, 1), (29, 6), (28, 18), (16, 31), (4, 18), (3, 6)], &Pen { style: PenStyle::Solid, width: 1, color: 0x0B4A8B }, &Brush::Solid(0x1E78D0), false);
                cv.polygon(&[(16, 4), (26, 8), (25, 17), (16, 27)], &none, &Brush::Solid(0x5AA2E8), false);
            }
            // IDI_APPLICATION, IDI_WINLOGO and anything else: a small window
            _ => {
                cv.fill_rect(Rect::new(3, 5, 29, 27), 0x404040);
                cv.fill_rect(Rect::new(4, 6, 28, 26), white);
                cv.fill_rect(Rect::new(4, 6, 28, 12), 0x0078D7);
                cv.fill_rect(Rect::new(24, 8, 26, 10), white);
                cv.fill_rect(Rect::new(7, 15, 25, 17), 0xB0B0B0);
                cv.fill_rect(Rect::new(7, 19, 21, 21), 0xB0B0B0);
            }
        }
    }
    let mut rgb = Vec::with_capacity(32 * 32);
    let mut alpha = Vec::with_capacity(32 * 32);
    for y in 0..32 {
        for x in 0..32 {
            let c = s.get(x, y) & 0xFFFFFF;
            rgb.push(c);
            alpha.push(if c == KEY { 0 } else { 255 });
        }
    }
    IconImage { w: 32, h: 32, rgb, alpha, hotspot: (0, 0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stock_icon_draws_something_and_keeps_a_transparent_corner() {
        for id in [32512, 32513, 32514, 32515, 32516, 32517, 32518, 1] {
            let i = stock_icon(id);
            assert_eq!((i.w, i.h, i.rgb.len(), i.alpha.len()), (32, 32, 1024, 1024));
            assert!(i.alpha.iter().filter(|a| **a == 255).count() > 100, "{id} has pixels");
            assert_eq!(i.alpha[0], 0, "{id} is not a filled square");
        }
    }
}
