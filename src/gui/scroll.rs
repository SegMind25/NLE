//! Scroll bars: the range / page / position model, the geometry of arrows, track and thumb, hit
//! testing, and painting in the Windows 10 look. Used by the scroll bars of ordinary windows
//! (`WS_VSCROLL` / `WS_HSCROLL`, drawn in the non-client area) and by the SCROLLBAR control.

use super::surface::*;

pub const SB_HORZ: i32 = 0;
pub const SB_VERT: i32 = 1;
pub const SB_CTL: i32 = 2;
pub const SB_BOTH: i32 = 3;

// scroll codes in the low word of WM_HSCROLL / WM_VSCROLL
pub const SB_LINEUP: u32 = 0;
pub const SB_LINEDOWN: u32 = 1;
pub const SB_PAGEUP: u32 = 2;
pub const SB_PAGEDOWN: u32 = 3;
pub const SB_THUMBPOSITION: u32 = 4;
pub const SB_THUMBTRACK: u32 = 5;
pub const SB_TOP: u32 = 6;
pub const SB_BOTTOM: u32 = 7;
pub const SB_ENDSCROLL: u32 = 8;

// SCROLLINFO masks
pub const SIF_RANGE: u32 = 1;
pub const SIF_PAGE: u32 = 2;
pub const SIF_POS: u32 = 4;
pub const SIF_DISABLENOSCROLL: u32 = 8;
pub const SIF_TRACKPOS: u32 = 0x10;

/// Smallest thumb, in pixels.
const MIN_THUMB: i32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    pub min: i32,
    pub max: i32,
    pub page: u32,
    pub pos: i32,
    /// Where the thumb is while it is being dragged.
    pub track: i32,
    /// `EnableScrollBar` can disable either arrow independently.
    pub up_enabled: bool,
    pub down_enabled: bool,
}

impl Default for Bar {
    fn default() -> Bar {
        Bar { min: 0, max: 0, page: 0, pos: 0, track: 0, up_enabled: true, down_enabled: true }
    }
}

impl Bar {
    /// The largest position the thumb can reach: `max - page + 1` when there is a page, else `max`.
    pub fn max_pos(&self) -> i32 {
        if self.page == 0 {
            self.max
        } else {
            (self.max - (self.page as i32 - 1)).max(self.min)
        }
    }

    pub fn clamp(&mut self) {
        if self.max < self.min {
            self.max = self.min;
        }
        self.pos = self.pos.clamp(self.min, self.max_pos());
    }

    /// Whether the range is larger than one page, so there is something to scroll.
    pub fn scrollable(&self) -> bool {
        self.max_pos() > self.min
    }

    /// `SetScrollInfo` for the fields selected by `mask`.
    pub fn set(&mut self, mask: u32, min: i32, max: i32, page: u32, pos: i32) {
        if mask & SIF_RANGE != 0 {
            self.min = min;
            self.max = max;
        }
        if mask & SIF_PAGE != 0 {
            self.page = page.min(i32::MAX as u32);
        }
        if mask & SIF_POS != 0 {
            self.pos = pos;
        }
        self.clamp();
        self.track = self.pos;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    None,
    Arrow1,
    Arrow2,
    Page1,
    Page2,
    Thumb,
}

/// The pieces of a bar inside its rectangle (all in the same coordinates as the rectangle).
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub arrow1: Rect,
    pub arrow2: Rect,
    pub track: Rect,
    /// Empty when there is nothing to scroll or no room.
    pub thumb: Rect,
    pub vertical: bool,
}

pub fn layout(bar: &Bar, r: Rect, vertical: bool) -> Layout {
    let len = if vertical { r.height() } else { r.width() };
    let cross = if vertical { r.width() } else { r.height() };
    let arrow = cross.min(len / 2).max(0);
    let along = |a: i32, b: i32| if vertical { Rect::new(r.l, r.t + a, r.r, r.t + b) } else { Rect::new(r.l + a, r.t, r.l + b, r.b) };
    let arrow1 = along(0, arrow);
    let arrow2 = along(len - arrow, len);
    let track = along(arrow, len - arrow);
    let track_len = (len - 2 * arrow).max(0);
    let mut thumb = Rect::default();
    if bar.scrollable() && track_len > 0 {
        let range = (bar.max - bar.min + 1) as i64;
        let want = if bar.page == 0 { cross as i64 } else { track_len as i64 * bar.page as i64 / range };
        let tl = (want as i32).max(MIN_THUMB.min(track_len)).min(track_len);
        let span = (bar.max_pos() - bar.min) as i64;
        let pos = bar.track.clamp(bar.min, bar.max_pos()) - bar.min;
        let off = if span == 0 { 0 } else { ((track_len - tl) as i64 * pos as i64 / span) as i32 };
        thumb = along(arrow + off, arrow + off + tl);
    }
    Layout { arrow1, arrow2, track, thumb, vertical }
}

impl Layout {
    pub fn part_at(&self, x: i32, y: i32) -> Part {
        if self.arrow1.contains(x, y) {
            Part::Arrow1
        } else if self.arrow2.contains(x, y) {
            Part::Arrow2
        } else if !self.thumb.is_empty() && self.thumb.contains(x, y) {
            Part::Thumb
        } else if self.track.contains(x, y) {
            let before = if self.vertical { y < self.thumb.t } else { x < self.thumb.l };
            if self.thumb.is_empty() {
                Part::None
            } else if before {
                Part::Page1
            } else {
                Part::Page2
            }
        } else {
            Part::None
        }
    }

    /// The scroll position that puts the thumb's start at `thumb_start` (an offset from the track start).
    pub fn pos_for_thumb(&self, bar: &Bar, thumb_start: i32) -> i32 {
        let track_len = if self.vertical { self.track.height() } else { self.track.width() };
        let tl = if self.vertical { self.thumb.height() } else { self.thumb.width() };
        let room = (track_len - tl).max(1);
        let span = (bar.max_pos() - bar.min) as i64;
        let p = (thumb_start.clamp(0, room) as i64 * span + room as i64 / 2) / room as i64;
        (bar.min + p as i32).clamp(bar.min, bar.max_pos())
    }

    /// Distance of the thumb's start from the track's start.
    pub fn thumb_offset(&self) -> i32 {
        if self.vertical {
            self.thumb.t - self.track.t
        } else {
            self.thumb.l - self.track.l
        }
    }
}

/// What a press on `part` asks the owner to do, as a scroll code.
pub fn code_for(part: Part) -> Option<u32> {
    match part {
        Part::Arrow1 => Some(SB_LINEUP),
        Part::Arrow2 => Some(SB_LINEDOWN),
        Part::Page1 => Some(SB_PAGEUP),
        Part::Page2 => Some(SB_PAGEDOWN),
        _ => None,
    }
}

// ---- painting ----------------------------------------------------------------------------------

const TRACK: u32 = 0xF0F0F0;
const THUMB: u32 = 0xCDCDCD;
const THUMB_HOT: u32 = 0xA6A6A6;
const THUMB_DOWN: u32 = 0x606060;
const GLYPH: u32 = 0x606060;
const GLYPH_OFF: u32 = 0xBFBFBF;
const ARROW_HOT: u32 = 0xDADADA;
const ARROW_DOWN: u32 = 0x606060;

fn arrow_glyph(cv: &mut Canvas, r: Rect, vertical: bool, first: bool, color: u32) {
    let (cx, cy) = ((r.l + r.r) / 2, (r.t + r.b) / 2);
    let pts: [(i32, i32); 3] = match (vertical, first) {
        (true, true) => [(cx - 4, cy + 2), (cx + 4, cy + 2), (cx, cy - 2)],
        (true, false) => [(cx - 4, cy - 2), (cx + 4, cy - 2), (cx, cy + 2)],
        (false, true) => [(cx + 2, cy - 4), (cx + 2, cy + 4), (cx - 2, cy)],
        (false, false) => [(cx - 2, cy - 4), (cx - 2, cy + 4), (cx + 2, cy)],
    };
    cv.fill_polygon_color(&pts, color, false);
}

/// Draws a scroll bar into `r` (canvas coordinates). `hot` and `pressed` highlight a part.
pub fn paint(cv: &mut Canvas, r: Rect, vertical: bool, bar: &Bar, enabled: bool, hot: Part, pressed: Part) {
    let l = layout(bar, r, vertical);
    cv.fill_rect(r, TRACK);
    for (rect, part, first, on) in [(l.arrow1, Part::Arrow1, true, bar.up_enabled), (l.arrow2, Part::Arrow2, false, bar.down_enabled)] {
        if rect.is_empty() {
            continue;
        }
        let live = enabled && on && bar.scrollable();
        let (bg, glyph) = if !live {
            (TRACK, GLYPH_OFF)
        } else if pressed == part {
            (ARROW_DOWN, 0xFFFFFF)
        } else if hot == part {
            (ARROW_HOT, GLYPH)
        } else {
            (TRACK, GLYPH)
        };
        cv.fill_rect(rect, bg);
        arrow_glyph(cv, rect, vertical, first, glyph);
    }
    if enabled && !l.thumb.is_empty() {
        let color = if pressed == Part::Thumb { THUMB_DOWN } else if hot == Part::Thumb { THUMB_HOT } else { THUMB };
        let t = if vertical { Rect::new(l.thumb.l + 2, l.thumb.t, l.thumb.r - 2, l.thumb.b) } else { Rect::new(l.thumb.l, l.thumb.t + 2, l.thumb.r, l.thumb.b - 2) };
        cv.fill_rect(t, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(min: i32, max: i32, page: u32, pos: i32) -> Bar {
        let mut b = Bar::default();
        b.set(SIF_RANGE | SIF_PAGE | SIF_POS, min, max, page, pos);
        b
    }

    #[test]
    fn position_is_clamped_to_the_range_less_a_page() {
        assert_eq!(bar(0, 99, 10, 500).pos, 90);
        assert_eq!(bar(0, 99, 10, -5).pos, 0);
        assert_eq!(bar(0, 99, 0, 500).pos, 99, "without a page the whole range is reachable");
        assert_eq!(bar(0, 5, 100, 3).pos, 0, "a page larger than the range leaves nothing to scroll");
        assert!(!bar(0, 5, 100, 0).scrollable());
        assert!(bar(0, 99, 10, 0).scrollable());
    }

    #[test]
    fn thumb_size_follows_the_page_and_position_follows_pos() {
        let r = Rect::new(0, 0, 17, 117);
        let l = layout(&bar(0, 99, 50, 0), r, true);
        // arrows are 17 px, the track is 83 px; half the range is one page
        assert_eq!(l.track, Rect::new(0, 17, 17, 100));
        assert_eq!(l.thumb.height(), 41);
        assert_eq!(l.thumb.t, 17);
        let end = layout(&bar(0, 99, 50, 50), r, true);
        assert_eq!(end.thumb.b, 100, "the thumb reaches the end of the track at the last position");
        let tiny = layout(&bar(0, 9999, 10, 0), r, true);
        assert_eq!(tiny.thumb.height(), 16, "never smaller than the minimum");
        assert!(layout(&bar(0, 5, 100, 0), r, true).thumb.is_empty());
    }

    #[test]
    fn hit_testing_finds_arrows_pages_and_thumb() {
        let b = bar(0, 99, 20, 40);
        let l = layout(&b, Rect::new(0, 0, 200, 17), false);
        assert_eq!(l.part_at(5, 8), Part::Arrow1);
        assert_eq!(l.part_at(195, 8), Part::Arrow2);
        let mid = (l.thumb.l + l.thumb.r) / 2;
        assert_eq!(l.part_at(mid, 8), Part::Thumb);
        assert_eq!(l.part_at(l.thumb.l - 2, 8), Part::Page1);
        assert_eq!(l.part_at(l.thumb.r + 2, 8), Part::Page2);
        assert_eq!(l.part_at(100, 40), Part::None);
        assert_eq!(code_for(Part::Page2), Some(SB_PAGEDOWN));
        assert_eq!(code_for(Part::Thumb), None);
    }

    #[test]
    fn dragging_the_thumb_maps_back_to_positions() {
        let b = bar(0, 99, 10, 0);
        let l = layout(&b, Rect::new(0, 0, 17, 217), true);
        let room = (l.track.height() - l.thumb.height()) as i32;
        assert_eq!(l.pos_for_thumb(&b, 0), 0);
        assert_eq!(l.pos_for_thumb(&b, room), 90);
        assert_eq!(l.pos_for_thumb(&b, room * 10), 90, "dragging past the end stops at the last position");
        assert_eq!(l.pos_for_thumb(&b, -40), 0);
        let half = l.pos_for_thumb(&b, room / 2);
        assert!((44..=46).contains(&half), "{half}");
        let mut moved = b;
        moved.track = half;
        moved.pos = half;
        let l2 = layout(&moved, Rect::new(0, 0, 17, 217), true);
        assert!((l2.thumb_offset() - room / 2).abs() <= 2);
    }

    #[test]
    fn a_degenerate_rectangle_does_not_panic() {
        let b = bar(0, 99, 10, 5);
        for r in [Rect::new(0, 0, 0, 0), Rect::new(0, 0, 17, 5), Rect::new(0, 0, 3, 40)] {
            let l = layout(&b, r, true);
            let _ = l.part_at(1, 1);
        }
    }
}
