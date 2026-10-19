//! The tree view (`SysTreeView32`): nested items that expand and collapse, drawn with +/- buttons and dotted lines,
//! optional images and check boxes, keyboard and mouse selection, and the TVM_* messages programs build trees with.
//! Item handles (`HTREEITEM`) are small integers that are never reused.

use super::api;
use super::controls::{dotted_focus, font_of, Ctl};
use super::ctl_basic::{notify_with, NM_CLICK, NM_DBLCLK, NM_RCLICK, NM_RETURN};
use super::imagelist::CLR_NONE;
use super::msg::{current_ansi, def_window_proc};
use super::scroll::{self, SB_HORZ, SB_VERT};
use super::surface::*;
use super::wm::Class;
use super::*;

pub const TVS_HASBUTTONS: u32 = 1;
pub const TVS_HASLINES: u32 = 2;
pub const TVS_LINESATROOT: u32 = 4;
pub const TVS_SHOWSELALWAYS: u32 = 0x20;
pub const TVS_CHECKBOXES: u32 = 0x100;
pub const TVS_FULLROWSELECT: u32 = 0x1000;

const TVIS_SELECTED: u32 = 2;
const TVIS_BOLD: u32 = 0x10;
const TVIS_EXPANDED: u32 = 0x20;
const TVIS_STATEIMAGEMASK: u32 = 0xF000;

const TVIF_TEXT: u32 = 1;
const TVIF_IMAGE: u32 = 2;
const TVIF_PARAM: u32 = 4;
const TVIF_STATE: u32 = 8;
const TVIF_HANDLE: u32 = 0x10;
const TVIF_SELECTEDIMAGE: u32 = 0x20;
const TVIF_CHILDREN: u32 = 0x40;

const TVI_ROOT: isize = -0x10000;
const TVI_FIRST: isize = -0x0FFFF;
const TVI_LAST: isize = -0x0FFFE;
const TVI_SORT: isize = -0x0FFFD;

const TVE_COLLAPSE: u32 = 1;
const TVE_EXPAND: u32 = 2;
const TVE_TOGGLE: u32 = 3;

const TVC_UNKNOWN: u32 = 0;
const TVC_BYMOUSE: u32 = 1;
const TVC_BYKEYBOARD: u32 = 2;

#[derive(Clone, Debug, Default)]
pub struct Node {
    pub alive: bool,
    pub parent: usize,
    pub children: Vec<usize>,
    pub text: String,
    pub image: i32,
    pub sel_image: i32,
    pub state: u32,
    /// `cChildren`: a program can say an item has children before it adds them (so the button shows).
    pub kids_hint: i32,
    pub param: isize,
}

#[derive(Clone, Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub roots: Vec<usize>,
    pub sel: usize,
    pub indent: i32,
    pub item_h: i32,
    /// Normal and state image lists.
    pub images: [usize; 2],
    pub bk: u32,
    pub text_color: u32,
    pub line_color: u32,
    pub xoff: i32,
    pub yoff: i32,
    pub hot: usize,
    pub typed: String,
    pub typed_at: u32,
}

impl Default for Tree {
    fn default() -> Tree {
        Tree {
            nodes: Vec::new(),
            roots: Vec::new(),
            sel: 0,
            indent: 19,
            item_h: 0,
            images: [0; 2],
            bk: CLR_NONE,
            text_color: CLR_NONE,
            line_color: CLR_NONE,
            xoff: 0,
            yoff: 0,
            hot: 0,
            typed: String::new(),
            typed_at: 0,
        }
    }
}

impl Tree {
    pub fn node(&self, id: usize) -> Option<&Node> {
        id.checked_sub(1).and_then(|i| self.nodes.get(i)).filter(|n| n.alive)
    }

    fn node_mut(&mut self, id: usize) -> Option<&mut Node> {
        id.checked_sub(1).and_then(|i| self.nodes.get_mut(i)).filter(|n| n.alive)
    }

    /// The ordered siblings of an item (the roots for a top-level item or for id 0).
    fn siblings(&self, parent: usize) -> &Vec<usize> {
        match self.node(parent) {
            Some(n) => &n.children,
            None => &self.roots,
        }
    }

    fn expanded(&self, id: usize) -> bool {
        self.node(id).is_some_and(|n| n.state & TVIS_EXPANDED != 0)
    }

    fn has_children(&self, id: usize) -> bool {
        self.node(id).is_some_and(|n| !n.children.is_empty() || n.kids_hint > 0)
    }

    pub fn count(&self) -> usize {
        self.nodes.iter().filter(|n| n.alive).count()
    }

    /// The items currently shown, top to bottom, with their depth.
    pub fn rows(&self) -> Vec<(usize, i32)> {
        fn walk(t: &Tree, ids: &[usize], depth: i32, out: &mut Vec<(usize, i32)>) {
            for &id in ids {
                out.push((id, depth));
                if t.expanded(id) {
                    if let Some(n) = t.node(id) {
                        walk(t, &n.children, depth + 1, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(self, &self.roots, 0, &mut out);
        out
    }

    fn next_sibling(&self, id: usize) -> Option<usize> {
        let parent = self.node(id)?.parent;
        let sib = self.siblings(parent);
        let at = sib.iter().position(|s| *s == id)?;
        sib.get(at + 1).copied()
    }

    fn prev_sibling(&self, id: usize) -> Option<usize> {
        let parent = self.node(id)?.parent;
        let sib = self.siblings(parent);
        let at = sib.iter().position(|s| *s == id)?;
        at.checked_sub(1).map(|a| sib[a])
    }

    fn is_descendant(&self, id: usize, of: usize) -> bool {
        let mut cur = self.node(id).map_or(0, |n| n.parent);
        while cur != 0 {
            if cur == of {
                return true;
            }
            cur = self.node(cur).map_or(0, |n| n.parent);
        }
        false
    }

    /// Adds an item under `parent` at the position `after` names; returns its handle.
    fn insert(&mut self, parent: usize, after: isize, mut node: Node, sort: bool) -> usize {
        node.alive = true;
        node.parent = parent;
        self.nodes.push(node);
        let id = self.nodes.len();
        let text = self.nodes[id - 1].text.to_lowercase();
        let pos = {
            let sib = self.siblings(parent);
            match after {
                TVI_FIRST => 0,
                TVI_LAST => sib.len(),
                _ if sort => sib.iter().position(|s| self.node(*s).is_some_and(|n| n.text.to_lowercase() > text)).unwrap_or(sib.len()),
                TVI_SORT => sib.iter().position(|s| self.node(*s).is_some_and(|n| n.text.to_lowercase() > text)).unwrap_or(sib.len()),
                a => sib.iter().position(|s| *s as isize == a).map_or(sib.len(), |p| p + 1),
            }
        };
        match self.node_mut(parent) {
            Some(p) => p.children.insert(pos, id),
            None => self.roots.insert(pos, id),
        }
        id
    }

    /// Removes an item and everything under it.
    fn delete(&mut self, id: usize) -> bool {
        let Some(parent) = self.node(id).map(|n| n.parent) else { return false };
        let doomed = {
            let mut all = vec![id];
            let mut i = 0;
            while i < all.len() {
                if let Some(n) = self.node(all[i]) {
                    all.extend(n.children.iter().copied());
                }
                i += 1;
            }
            all
        };
        if doomed.contains(&self.sel) {
            self.sel = if parent != 0 { parent } else { self.next_sibling(id).or_else(|| self.prev_sibling(id)).unwrap_or(0) };
        }
        match self.node_mut(parent) {
            Some(p) => p.children.retain(|c| *c != id),
            None => self.roots.retain(|c| *c != id),
        }
        for d in doomed {
            if let Some(n) = d.checked_sub(1).and_then(|i| self.nodes.get_mut(i)) {
                n.alive = false;
                n.children.clear();
            }
        }
        true
    }

    fn expand_to(&mut self, id: usize) {
        let mut cur = self.node(id).map_or(0, |n| n.parent);
        while cur != 0 {
            if let Some(n) = self.node_mut(cur) {
                n.state |= TVIS_EXPANDED;
            }
            cur = self.node(cur).map_or(0, |n| n.parent);
        }
    }

    fn sort_children(&mut self, parent: usize, recurse: bool) {
        let mut ids = self.siblings(parent).clone();
        ids.sort_by_key(|i| self.node(*i).map(|n| n.text.to_lowercase()).unwrap_or_default());
        match self.node_mut(parent) {
            Some(p) => p.children = ids.clone(),
            None => self.roots = ids.clone(),
        }
        if recurse {
            for i in ids {
                self.sort_children(i, true);
            }
        }
    }
}

fn tv(g: &Gui, h: usize) -> Option<&Tree> {
    match &g.windows.get(&h)?.ctl {
        Ctl::Tree(t) => Some(t),
        _ => None,
    }
}

fn tv_mut(g: &mut Gui, h: usize) -> Option<&mut Tree> {
    match &mut g.windows.get_mut(&h)?.ctl {
        Ctl::Tree(t) => Some(t),
        _ => None,
    }
}

pub fn register_class(g: &mut Gui) {
    g.register_class(Class { name: "SysTreeView32".into(), atom: 0, style: 0x8, wndproc: treeview_proc as *const () as usize, cls_extra: 0, wnd_extra: 0, hinstance: 0, icon: 0, cursor: 0, background: 0, menu_name: None, unicode: true, native: true });
}

// ---- geometry ----------------------------------------------------------------------------------------------

fn style_of(g: &Gui, h: usize) -> u32 {
    g.windows.get(&h).map_or(0, |w| w.style)
}

fn row_h(g: &Gui, h: usize) -> i32 {
    match tv(g, h) {
        Some(t) if t.item_h > 0 => t.item_h,
        _ => (g.sized_font(font_of(g, h)).height() + 4).max(18),
    }
}

/// 1 when the first level is indented to make room for buttons or root lines.
fn root_off(style: u32) -> i32 {
    (style & (TVS_HASBUTTONS | TVS_LINESATROOT) != 0) as i32
}

/// Left edge of an item's image/label area at the given depth, before scrolling.
fn item_x(t: &Tree, style: u32, depth: i32) -> i32 {
    (depth + root_off(style)) * t.indent + 2
}

fn image_size(g: &Gui, t: &Tree) -> (i32, i32) {
    g.imagelist(t.images[0]).map_or((0, 0), |l| (l.w, l.h))
}

/// Where an item's label starts (x) given its depth and the widths of what comes before it.
fn label_x(g: &Gui, t: &Tree, style: u32, depth: i32) -> i32 {
    let mut x = item_x(t, style, depth);
    if style & TVS_CHECKBOXES != 0 {
        x += 18;
    }
    let (iw, _) = image_size(g, t);
    if iw > 0 {
        x += iw + 3;
    } else {
        x += 1;
    }
    x
}

fn content_size(g: &Gui, h: usize) -> (i32, i32) {
    let Some(t) = tv(g, h) else { return (0, 0) };
    let style = style_of(g, h);
    let font = g.sized_font(font_of(g, h));
    let rows = t.rows();
    let w = rows.iter().map(|(id, d)| label_x(g, t, style, *d) + font.text_width(&t.node(*id).map_or(String::new(), |n| n.text.clone())) + 6).max().unwrap_or(0);
    (w, rows.len() as i32 * row_h(g, h))
}

fn sync_bars(g: &mut Gui, h: usize) {
    let (cw0, ch0) = content_size(g, h);
    for _ in 0..2 {
        let (vw, vh) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
        let (xo, yo) = tv(g, h).map_or((0, 0), |t| (t.xoff, t.yoff));
        g.scroll_set(h, SB_VERT, scroll::SIF_RANGE | scroll::SIF_PAGE | scroll::SIF_POS, 0, (ch0 - 1).max(0), vh as u32, yo);
        g.scroll_set(h, SB_HORZ, scroll::SIF_RANGE | scroll::SIF_PAGE | scroll::SIF_POS, 0, (cw0 - 1).max(0), vw as u32, xo);
    }
    let (vw, vh) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
    if let Some(t) = tv_mut(g, h) {
        t.xoff = t.xoff.clamp(0, (cw0 - vw).max(0));
        t.yoff = t.yoff.clamp(0, (ch0 - vh).max(0));
    }
    g.paint_nc(h);
}

fn refresh(g: &mut Gui, h: usize) {
    sync_bars(g, h);
    g.invalidate(h, None, true);
}

/// Scrolls so the item's row is inside the client area (the item must be shown).
fn scroll_to(g: &mut Gui, h: usize, id: usize) {
    let rh = row_h(g, h);
    let (_, vh) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
    let Some(t) = tv_mut(g, h) else { return };
    let Some(r) = t.rows().iter().position(|(i, _)| *i == id) else { return };
    let top = r as i32 * rh;
    if top < t.yoff {
        t.yoff = top;
    } else if top + rh > t.yoff + vh {
        t.yoff = top + rh - vh;
    }
    sync_bars(g, h);
}

/// The client rectangle of an item's row (whole row), if the item is shown.
fn row_rect(g: &Gui, h: usize, id: usize) -> Option<Rect> {
    let t = tv(g, h)?;
    let rh = row_h(g, h);
    let (cw, _) = g.windows.get(&h)?.client_size();
    let r = t.rows().iter().position(|(i, _)| *i == id)? as i32;
    Some(Rect::new(0, r * rh - t.yoff, cw, (r + 1) * rh - t.yoff))
}

/// The bounds of an item's label (or image and label) in client coordinates.
fn item_bounds(g: &Gui, h: usize, id: usize, text_only: bool) -> Option<Rect> {
    let t = tv(g, h)?;
    let style = style_of(g, h);
    let row = row_rect(g, h, id)?;
    let depth = t.rows().iter().find(|(i, _)| *i == id)?.1;
    let font = g.sized_font(font_of(g, h));
    let lx = label_x(g, t, style, depth) - t.xoff;
    let tw = font.text_width(&t.node(id)?.text);
    let left = if text_only { lx - 2 } else { item_x(t, style, depth) - t.xoff };
    Some(Rect::new(left, row.t, lx + tw + 3, row.b))
}

/// What is at a client point: the TVHT_* flags and the item.
fn hit_test(g: &Gui, h: usize, x: i32, y: i32) -> (u32, usize) {
    let Some(t) = tv(g, h) else { return (1, 0) };
    let (style, rh) = (style_of(g, h), row_h(g, h));
    let (cw, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
    if x < 0 || y < 0 || x >= cw || y >= ch {
        return (if x < 0 { 0x100 } else if x >= cw { 0x200 } else if y < 0 { 0x400 } else { 0x800 }, 0);
    }
    let rows = t.rows();
    let r = ((y + t.yoff) / rh) as usize;
    let Some(&(id, depth)) = rows.get(r) else { return (1, 0) };
    let xi = x + t.xoff;
    let x_img = item_x(t, style, depth);
    let bx = (depth + root_off(style)) * t.indent - t.indent / 2;
    if style & TVS_HASBUTTONS != 0 && t.has_children(id) && (xi - bx).abs() <= 5 {
        return (0x10, id);
    }
    if xi < x_img {
        return (8, id);
    }
    let font = g.sized_font(font_of(g, h));
    let mut cur = x_img;
    if style & TVS_CHECKBOXES != 0 {
        if xi < cur + 18 {
            return (0x40, id);
        }
        cur += 18;
    }
    let (iw, _) = image_size(g, t);
    if iw > 0 {
        if xi < cur + iw + 3 {
            return (2, id);
        }
        cur += iw + 3;
    } else {
        cur += 1;
    }
    let tw = font.text_width(&t.node(id).map_or("", |n| n.text.as_str()));
    if xi < cur + tw + 3 || style & TVS_FULLROWSELECT != 0 {
        (4, id)
    } else {
        (0x20, id)
    }
}

// ---- notifications -----------------------------------------------------------------------------------------

/// A TVITEM (56 bytes) describing a node, as the notifications carry it.
fn put_item(b: &mut [u8], at: usize, id: usize, n: &Node) {
    b[at..at + 4].copy_from_slice(&(TVIF_HANDLE | TVIF_STATE | TVIF_PARAM | TVIF_IMAGE).to_le_bytes());
    b[at + 8..at + 16].copy_from_slice(&(id as u64).to_le_bytes());
    b[at + 16..at + 20].copy_from_slice(&n.state.to_le_bytes());
    b[at + 20..at + 24].copy_from_slice(&u32::MAX.to_le_bytes());
    b[at + 36..at + 40].copy_from_slice(&n.image.to_le_bytes());
    b[at + 40..at + 44].copy_from_slice(&n.sel_image.to_le_bytes());
    b[at + 48..at + 56].copy_from_slice(&(n.param as i64).to_le_bytes());
}

/// An NMTREEVIEW body (after the NMHDR): action, itemOld, itemNew, ptDrag.
fn nm_tree(action: u32, old: Option<(usize, &Node)>, new: Option<(usize, &Node)>, pt: (i32, i32)) -> Vec<u8> {
    let mut b = vec![0u8; 128];
    b[0..4].copy_from_slice(&action.to_le_bytes());
    if let Some((id, n)) = old {
        put_item(&mut b, 8, id, n);
    }
    if let Some((id, n)) = new {
        put_item(&mut b, 64, id, n);
    }
    b[120..124].copy_from_slice(&pt.0.to_le_bytes());
    b[124..128].copy_from_slice(&pt.1.to_le_bytes());
    b
}

/// ANSI or Unicode notification code, following the parent window.
fn code(h: usize, ansi: i32, wide: i32) -> i32 {
    let unicode = with(|g| {
        let p = g.windows.get(&h).map_or(0, |w| w.parent);
        g.windows.get(&p).is_none_or(|w| w.unicode)
    });
    if unicode {
        wide
    } else {
        ansi
    }
}

unsafe fn send_tree(h: usize, ansi: i32, wide: i32, action: u32, old: usize, new: usize, pt: (i32, i32)) -> isize {
    let body = with(|g| {
        let t = tv(g, h)?;
        Some(nm_tree(action, t.node(old).map(|n| (old, n)), t.node(new).map(|n| (new, n)), pt))
    });
    let Some(body) = body else { return 0 };
    notify_with(h, code(h, ansi, wide), &body).0
}

/// Moves the selection, with TVN_SELCHANGING / TVN_SELCHANGED around it.
unsafe fn select(h: usize, id: usize, action: u32) -> bool {
    let old = with(|g| tv(g, h).map_or(0, |t| t.sel));
    if old == id {
        return true;
    }
    if send_tree(h, -401, -450, action, old, id, (0, 0)) != 0 {
        return false;
    }
    with(|g| {
        if let Some(t) = tv_mut(g, h) {
            if let Some(n) = t.node_mut(old) {
                n.state &= !TVIS_SELECTED;
            }
            if id != 0 {
                t.expand_to(id);
            }
            if let Some(n) = t.node_mut(id) {
                n.state |= TVIS_SELECTED;
            }
            t.sel = id;
        }
        sync_bars(g, h);
        scroll_to(g, h, id);
        g.invalidate(h, None, false);
    });
    send_tree(h, -402, -451, action, old, id, (0, 0));
    true
}

/// Expands or collapses an item, telling the parent first so it can refuse.
unsafe fn expand(h: usize, id: usize, how: u32) -> bool {
    let (expanded, has) = with(|g| tv(g, h).map_or((false, false), |t| (t.expanded(id), t.has_children(id))));
    if !has {
        return false;
    }
    let want = match how {
        TVE_EXPAND => true,
        TVE_COLLAPSE => false,
        TVE_TOGGLE => !expanded,
        _ => return false,
    };
    if want == expanded {
        return true;
    }
    let action = if want { 2 } else { 1 };
    if send_tree(h, -405, -454, action, 0, id, (0, 0)) != 0 {
        return false;
    }
    let moved = with(|g| {
        let mut moved = false;
        if let Some(t) = tv_mut(g, h) {
            if let Some(n) = t.node_mut(id) {
                n.state = if want { n.state | TVIS_EXPANDED } else { n.state & !TVIS_EXPANDED };
            }
            // a collapsed branch cannot keep the selection inside it
            if !want && t.is_descendant(t.sel, id) {
                moved = true;
            }
        }
        refresh(g, h);
        moved
    });
    if moved {
        select(h, id, TVC_UNKNOWN);
    }
    send_tree(h, -406, -455, action, 0, id, (0, 0));
    true
}

unsafe fn toggle_check(h: usize, id: usize) {
    with(|g| {
        if let Some(n) = tv_mut(g, h).and_then(|t| t.node_mut(id)) {
            let checked = (n.state & TVIS_STATEIMAGEMASK) >> 12 == 2;
            n.state = (n.state & !TVIS_STATEIMAGEMASK) | if checked { 1 << 12 } else { 2 << 12 };
        }
        g.invalidate(h, None, false);
    });
}

// ---- painting --------------------------------------------------------------------------------------------------

unsafe fn paint_tree(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    with(|g| draw_tree(g, h, hdc));
    api::end_paint(h, ps.as_ptr());
}

fn draw_tree(g: &mut Gui, h: usize, hdc: usize) {
    let Some(t) = tv(g, h).cloned() else { return };
    let style = style_of(g, h);
    let (focused, enabled) = (g.focus == h, g.is_enabled(h));
    let f = font_of(g, h);
    g.select_object(hdc, f);
    let font = g.sized_font(f);
    let (cw, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
    let rh = row_h(g, h);
    let bg = if t.bk != CLR_NONE { from_colorref(t.bk) } else { 0xFFFFFF };
    let fg = if !enabled {
        0x838383
    } else if t.text_color != CLR_NONE {
        from_colorref(t.text_color)
    } else {
        0
    };
    let line = if t.line_color != CLR_NONE { from_colorref(t.line_color) } else { 0x808080 };
    g.fill_rect(hdc, Rect::new(0, 0, cw, ch), &Brush::Solid(bg));
    let rows = t.rows();
    let (iw, ih) = image_size(g, &t);
    let has_images = iw > 0;
    let first = (t.yoff / rh).max(0) as usize;
    let last = (((t.yoff + ch) / rh + 1) as usize).min(rows.len());
    let sel_bg = if focused { 0xCCE8FF } else { 0xD9D9D9 };
    let mut images: Vec<(usize, usize, i32, i32)> = Vec::new();
    let buttons = style & TVS_HASBUTTONS != 0;
    let lines = style & TVS_HASLINES != 0;
    let roots_lines = style & TVS_LINESATROOT != 0;
    let dot = Pen { style: PenStyle::Dot, width: 1, color: line };
    let off = root_off(style);
    for r in first..last {
        let (id, depth) = rows[r];
        let Some(n) = t.node(id) else { continue };
        let top = r as i32 * rh - t.yoff;
        let mid = top + rh / 2;
        let x_img = item_x(&t, style, depth) - t.xoff;
        let bx = (depth + off) * t.indent - t.indent / 2 - t.xoff;
        let selected = n.state & TVIS_SELECTED != 0 && (focused || style & TVS_SHOWSELALWAYS != 0);
        let hot = t.hot == id && enabled && !selected;
        let mut x = x_img;
        let check = if style & TVS_CHECKBOXES != 0 {
            let cx = x;
            x += 18;
            Some((cx, (n.state & TVIS_STATEIMAGEMASK) >> 12 == 2))
        } else {
            None
        };
        let image = if n.state & TVIS_SELECTED != 0 && n.sel_image >= 0 { n.sel_image } else { n.image };
        let img_at = if has_images && image >= 0 {
            let p = (x, top + (rh - ih) / 2);
            Some(p)
        } else {
            None
        };
        if has_images {
            x += iw + 3;
        } else {
            x += 1;
        }
        let tw = font.text_width(&n.text) + if n.state & TVIS_BOLD != 0 { 1 } else { 0 };
        let label = Rect::new(x - 2, top + 1, x + tw + 3, top + rh - 1);
        let has_kids = t.has_children(id);
        let expanded = t.expanded(id);
        let next = t.next_sibling(id).is_some();
        // ancestors with later siblings keep a vertical line running through this row
        let mut ancestors: Vec<i32> = Vec::new();
        if lines {
            let mut cur = n.parent;
            let mut d = depth - 1;
            while cur != 0 {
                if t.next_sibling(cur).is_some() && (d >= 1 || roots_lines && d >= 0) {
                    ancestors.push((d + off) * t.indent - t.indent / 2 - t.xoff);
                }
                cur = t.node(cur).map_or(0, |a| a.parent);
                d -= 1;
            }
        }
        let lines_here = lines && (depth > 0 || roots_lines);
        g.draw(hdc, |cv, _| {
            if selected {
                cv.fill_rect(label, sel_bg);
            } else if hot {
                cv.fill_rect(label, 0xE5F3FF);
            }
            if style & TVS_FULLROWSELECT != 0 && selected {
                cv.fill_rect(Rect::new(0, top, cw, top + rh), sel_bg);
            }
            for ax in &ancestors {
                cv.line(*ax, top, *ax, top + rh, &dot);
            }
            if lines_here {
                cv.line(bx, top, bx, if next { top + rh } else { mid }, &dot);
                let end = x_img - 2;
                let start = if buttons && has_kids { bx + 5 } else { bx };
                if end > start {
                    cv.line(start, mid, end, mid, &dot);
                }
            }
            if buttons && has_kids && (depth > 0 || off == 1) {
                let b = Rect::new(bx - 4, mid - 4, bx + 5, mid + 5);
                cv.fill_rect(b, 0xFFFFFF);
                cv.frame_rect(b, 0x808080, 1);
                let pen = Pen { style: PenStyle::Solid, width: 1, color: 0 };
                cv.line(bx - 2, mid, bx + 3, mid, &pen);
                if !expanded {
                    cv.line(bx, mid - 2, bx, mid + 3, &pen);
                }
            }
            if let Some((cx, on)) = check {
                let cb = Rect::new(cx + 1, mid - 6, cx + 14, mid + 7);
                cv.fill_rect(cb, 0xFFFFFF);
                cv.frame_rect(cb, 0x333333, 1);
                if on {
                    let pen = Pen { style: PenStyle::Solid, width: 1, color: 0x0078D7 };
                    for d in 0..2 {
                        cv.line(cb.l + 3, cb.t + 6 + d, cb.l + 5, cb.t + 9 + d, &pen);
                        cv.line(cb.l + 5, cb.t + 9 + d, cb.l + 10, cb.t + 3 + d, &pen);
                    }
                }
            }
            font.draw(cv, x, mid - font.height() / 2, &n.text, fg, None);
            if n.state & TVIS_BOLD != 0 {
                font.draw(cv, x + 1, mid - font.height() / 2, &n.text, fg, None);
            }
            if t.sel == id && focused {
                dotted_focus(cv, label);
            }
        });
        if let Some((px, py)) = img_at {
            images.push((t.images[0], image as usize, px + 0, py));
        }
    }
    for (list, idx, x, y) in images {
        g.draw_imagelist(hdc, list, idx, x, y, 0, 0);
    }
}

// ---- TVITEM / TVINSERTSTRUCT -----------------------------------------------------------------------------

unsafe fn str_arg(p: usize, wide: bool) -> String {
    if p == 0 || p == usize::MAX {
        String::new()
    } else if wide {
        crate::runtime::strings::w_ptr_to_string(p as *const u16)
    } else {
        crate::runtime::strings::a_to_string(p as *const u8)
    }
}

/// Reads a TVITEM (mask@0, hItem@8, state@16, stateMask@20, pszText@24, cchTextMax@32, iImage@36, iSelectedImage@40,
/// cChildren@44, lParam@48) into a node.
unsafe fn apply_tvitem(n: &mut Node, p: *const u8, wide: bool) {
    let mask = *(p as *const u32);
    if mask & TVIF_TEXT != 0 {
        n.text = str_arg(*(p.add(24) as *const u64) as usize, wide);
    }
    if mask & TVIF_IMAGE != 0 {
        n.image = *(p.add(36) as *const i32);
    }
    if mask & TVIF_SELECTEDIMAGE != 0 {
        n.sel_image = *(p.add(40) as *const i32);
    }
    if mask & TVIF_CHILDREN != 0 {
        n.kids_hint = *(p.add(44) as *const i32);
    }
    if mask & TVIF_PARAM != 0 {
        n.param = *(p.add(48) as *const i64) as isize;
    }
    if mask & TVIF_STATE != 0 {
        let (state, smask) = (*(p.add(16) as *const u32), *(p.add(20) as *const u32));
        n.state = (n.state & !smask) | (state & smask);
    }
}

unsafe fn fill_tvitem(id: usize, n: &Node, children: bool, p: *mut u8, wide: bool) {
    let mask = *(p as *const u32);
    if mask & TVIF_HANDLE != 0 {
        *(p.add(8) as *mut u64) = id as u64;
    }
    if mask & TVIF_TEXT != 0 {
        let buf = *(p.add(24) as *const u64) as usize;
        let max = *(p.add(32) as *const i32);
        if buf != 0 && max > 0 {
            if wide {
                crate::runtime::strings::return_w(buf as *mut u16, max as u32, &n.text);
            } else {
                crate::runtime::strings::return_a(buf as *mut u8, max as u32, &n.text);
            }
        }
    }
    if mask & TVIF_IMAGE != 0 {
        *(p.add(36) as *mut i32) = n.image;
    }
    if mask & TVIF_SELECTEDIMAGE != 0 {
        *(p.add(40) as *mut i32) = n.sel_image;
    }
    if mask & TVIF_CHILDREN != 0 {
        *(p.add(44) as *mut i32) = children as i32;
    }
    if mask & TVIF_PARAM != 0 {
        *(p.add(48) as *mut i64) = n.param as i64;
    }
    if mask & TVIF_STATE != 0 {
        let smask = *(p.add(20) as *const u32);
        *(p.add(16) as *mut u32) = n.state & smask;
    }
}

/// The item a key press moves to, if any.
fn key_target(t: &Tree, key: u32, rows_per_page: i32) -> Option<usize> {
    let rows = t.rows();
    let at = rows.iter().position(|(i, _)| *i == t.sel);
    let pick = |i: i32| rows.get(i.clamp(0, rows.len() as i32 - 1) as usize).map(|(id, _)| *id);
    match key {
        0x26 => at.and_then(|a| pick(a as i32 - 1)).or_else(|| pick(0)),
        0x28 => at.and_then(|a| pick(a as i32 + 1)).or_else(|| pick(0)),
        0x21 => at.and_then(|a| pick(a as i32 - rows_per_page)),
        0x22 => at.and_then(|a| pick(a as i32 + rows_per_page)),
        0x24 => pick(0),
        0x23 => pick(rows.len() as i32 - 1),
        _ => None,
    }
}

pub unsafe extern "win64" fn treeview_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::Tree(Box::default());
                }
            });
            1
        }
        WM_CREATE | WM_SIZE => {
            with(refresh_all(h));
            0
        }
        WM_PAINT => {
            paint_tree(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETFONT => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.font = wp;
                }
                refresh(g, h);
            });
            0
        }
        WM_GETFONT => with(|g| g.windows.get(&h).map_or(0, |w| w.font)) as isize,
        WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS => {
            with(|g| g.invalidate(h, None, false));
            0
        }
        WM_GETDLGCODE => 0x81,
        WM_VSCROLL | WM_HSCROLL => {
            let code = loword(wp) as u32;
            let vertical = msg == WM_VSCROLL;
            let (pos, page, max, track, line) = with(|g| {
                let (cw, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
                let (conw, conh) = content_size(g, h);
                let t = tv(g, h).unwrap();
                let w = g.windows.get(&h).unwrap();
                let bar = if vertical { w.sb.vert } else { w.sb.horz };
                (if vertical { t.yoff } else { t.xoff }, if vertical { ch } else { cw }, if vertical { conh } else { conw }, bar.track, if vertical { row_h(g, h) } else { 16 })
            });
            let new = match code {
                scroll::SB_LINEUP => pos - line,
                scroll::SB_LINEDOWN => pos + line,
                scroll::SB_PAGEUP => pos - page,
                scroll::SB_PAGEDOWN => pos + page,
                scroll::SB_THUMBTRACK | scroll::SB_THUMBPOSITION => track,
                scroll::SB_TOP => 0,
                scroll::SB_BOTTOM => max,
                _ => return 0,
            };
            with(|g| {
                if let Some(t) = tv_mut(g, h) {
                    let v = new.clamp(0, (max - page).max(0));
                    if vertical {
                        t.yoff = v;
                    } else {
                        t.xoff = v;
                    }
                }
                sync_bars(g, h);
                g.invalidate(h, None, false);
            });
            0
        }
        WM_MOUSEWHEEL => {
            let delta = (wp >> 16) as u16 as i16 as i32;
            with(|g| {
                let step = row_h(g, h) * 3;
                let (_, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
                let max = (content_size(g, h).1 - ch).max(0);
                if let Some(t) = tv_mut(g, h) {
                    t.yoff = (t.yoff - delta / 120 * step).clamp(0, max);
                }
                sync_bars(g, h);
                g.invalidate(h, None, false);
            });
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN => {
            if !with(|g| g.is_enabled(h)) {
                return 0;
            }
            api::set_focus(h);
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            let right = msg == WM_RBUTTONDOWN;
            let (flags, id) = with(|g| hit_test(g, h, x, y));
            if id == 0 {
                if !right {
                    send_hit(h, NM_CLICK, (x, y));
                }
                return 0;
            }
            if flags & 0x10 != 0 && !right {
                expand(h, id, TVE_TOGGLE);
                return 0;
            }
            if flags & 0x40 != 0 && !right {
                toggle_check(h, id);
                return 0;
            }
            if flags & 0x46 != 0 || (flags & 0x20 != 0 && with(|g| style_of(g, h) & TVS_FULLROWSELECT != 0)) {
                select(h, id, TVC_BYMOUSE);
            }
            if msg == WM_LBUTTONDBLCLK {
                // the parent may take the double click itself; otherwise it expands or collapses the item
                if send_hit(h, NM_DBLCLK, (x, y)) == 0 {
                    expand(h, id, TVE_TOGGLE);
                }
            } else if !right {
                send_hit(h, NM_CLICK, (x, y));
            }
            0
        }
        WM_RBUTTONUP => {
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            if send_hit(h, NM_RCLICK, (x, y)) == 0 {
                let (sx, sy) = with(|g| g.client_to_screen(h, x, y));
                api::send(h, 0x007B, h, ((sy as isize) << 16) | (sx as isize & 0xFFFF));
            }
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            with(|g| {
                let (flags, id) = hit_test(g, h, x, y);
                let hot = if flags & 0x46 != 0 { id } else { 0 };
                if tv(g, h).is_some_and(|t| t.hot != hot) {
                    if let Some(t) = tv_mut(g, h) {
                        t.hot = hot;
                    }
                    g.invalidate(h, None, false);
                }
                g.track_leave(h);
            });
            0
        }
        WM_MOUSELEAVE => {
            with(|g| {
                if let Some(t) = tv_mut(g, h) {
                    t.hot = 0;
                }
                g.invalidate(h, None, false);
            });
            0
        }
        WM_KEYDOWN => {
            let key = wp as u32;
            let mut kd = [0u8; 8];
            kd[0..2].copy_from_slice(&(key as u16).to_le_bytes());
            notify_with(h, -412, &kd);
            let (sel, rows_per_page) = with(|g| {
                let (_, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
                (tv(g, h).map_or(0, |t| t.sel), (ch / row_h(g, h)).max(1))
            });
            match key {
                0x26 | 0x28 | 0x21 | 0x22 | 0x24 | 0x23 => {
                    if let Some(t) = with(|g| tv(g, h).and_then(|t| key_target(t, key, rows_per_page))) {
                        select(h, t, TVC_BYKEYBOARD);
                    }
                }
                0x27 | 0x6B => {
                    // right/plus: open the item, or go to its first child
                    let (open, kid) = with(|g| tv(g, h).map_or((false, 0), |t| (t.expanded(sel), t.node(sel).and_then(|n| n.children.first().copied()).unwrap_or(0))));
                    if !open && sel != 0 {
                        expand(h, sel, TVE_EXPAND);
                    } else if kid != 0 && key == 0x27 {
                        select(h, kid, TVC_BYKEYBOARD);
                    }
                }
                0x25 | 0x6D => {
                    let (open, parent) = with(|g| tv(g, h).map_or((false, 0), |t| (t.expanded(sel), t.node(sel).map_or(0, |n| n.parent))));
                    if open {
                        expand(h, sel, TVE_COLLAPSE);
                    } else if parent != 0 && key == 0x25 {
                        select(h, parent, TVC_BYKEYBOARD);
                    }
                }
                0x0D => {
                    if sel != 0 {
                        let r = notify_with(h, NM_RETURN, &[]).0;
                        if r == 0 {
                            expand(h, sel, TVE_TOGGLE);
                        }
                    }
                }
                0x20 => {
                    if sel != 0 && with(|g| style_of(g, h) & TVS_CHECKBOXES != 0) {
                        toggle_check(h, sel);
                    }
                }
                _ => {}
            }
            0
        }
        WM_CHAR => {
            let c = char::from_u32(wp as u32).unwrap_or('\0');
            if c.is_control() {
                return 0;
            }
            let now = with(|g| g.tick());
            let found = with(|g| {
                let t = tv_mut(g, h)?;
                if now.wrapping_sub(t.typed_at) > 1000 {
                    t.typed.clear();
                }
                t.typed.push(c);
                t.typed_at = now;
                let prefix = t.typed.to_lowercase();
                let rows = t.rows();
                let n = rows.len();
                let at = rows.iter().position(|(i, _)| *i == t.sel).unwrap_or(0);
                let start = if prefix.chars().count() == 1 { at + 1 } else { at };
                (0..n).map(|k| rows[(start + k) % n].0).find(|id| t.node(*id).is_some_and(|nd| nd.text.to_lowercase().starts_with(&prefix)))
            });
            if let Some(id) = found {
                select(h, id, TVC_BYKEYBOARD);
            }
            0
        }
        // TVM_INSERTITEMA / W
        0x1100 | 0x1132 => {
            let p = lp as *const u8;
            let parent_raw = *(p as *const i64) as isize;
            let after = *(p.add(8) as *const i64) as isize;
            let parent = if parent_raw == TVI_ROOT || parent_raw == 0 { 0 } else { parent_raw as usize };
            let mut node = Node { image: -1, sel_image: -1, ..Default::default() };
            apply_tvitem(&mut node, p.add(16), msg == 0x1132);
            let checkboxes = with(|g| style_of(g, h) & TVS_CHECKBOXES != 0);
            if checkboxes && node.state & TVIS_STATEIMAGEMASK == 0 {
                node.state |= 1 << 12;
            }
            let id = with(|g| {
                let t = tv_mut(g, h)?;
                if parent != 0 && t.node(parent).is_none() {
                    return None;
                }
                let sort = after == TVI_SORT;
                let id = t.insert(parent, after, node, sort);
                refresh(g, h);
                Some(id)
            });
            id.unwrap_or(0) as isize
        }
        // TVM_DELETEITEM
        0x1101 => {
            let item = lp;
            let was = with(|g| tv(g, h).map_or(0, |t| t.sel));
            let ok = with(|g| {
                let Some(t) = tv_mut(g, h) else { return false };
                if item == TVI_ROOT || item == 0 {
                    t.nodes.iter_mut().for_each(|n| {
                        n.alive = false;
                        n.children.clear();
                    });
                    t.roots.clear();
                    t.sel = 0;
                    t.xoff = 0;
                    t.yoff = 0;
                    true
                } else {
                    t.delete(item as usize)
                }
            });
            if ok {
                let now = with(|g| {
                    refresh(g, h);
                    tv(g, h).map_or(0, |t| t.sel)
                });
                if now != was {
                    send_tree(h, -402, -451, TVC_UNKNOWN, was, now, (0, 0));
                }
            }
            ok as isize
        }
        // TVM_EXPAND
        0x1102 => expand(h, lp as usize, wp as u32) as isize,
        // TVM_GETITEMRECT: wParam text-only; lParam a RECT whose first 8 bytes hold the HTREEITEM
        0x1104 => with(|g| {
            let o = lp as *mut i32;
            let id = *(o as *const u64) as usize;
            match item_bounds(g, h, id, wp != 0) {
                Some(r) => {
                    *o = r.l;
                    *o.add(1) = r.t;
                    *o.add(2) = r.r;
                    *o.add(3) = r.b;
                    1
                }
                None => 0,
            }
        }),
        0x1105 => with(|g| tv(g, h).map_or(0, |t| t.count() as isize)),
        0x1106 => with(|g| tv(g, h).map_or(0, |t| t.indent as isize)),
        0x1107 => with(|g| {
            if let Some(t) = tv_mut(g, h) {
                t.indent = (wp as i32).max(5);
            }
            refresh(g, h);
            0
        }),
        // TVM_GETIMAGELIST / TVM_SETIMAGELIST (0 normal, 2 state)
        0x1108 => with(|g| tv(g, h).map_or(0, |t| t.images[(wp == 2) as usize] as isize)),
        0x1109 => with(|g| {
            let Some(t) = tv_mut(g, h) else { return 0 };
            let old = std::mem::replace(&mut t.images[(wp == 2) as usize], lp as usize);
            refresh(g, h);
            old as isize
        }),
        // TVM_GETNEXTITEM
        0x110A => with(|g| {
            let Some(t) = tv(g, h) else { return 0 };
            let item = lp as usize;
            let rows = t.rows();
            let at = rows.iter().position(|(i, _)| *i == item);
            let rh = row_h(g, h);
            (match wp {
                0 => t.roots.first().copied().unwrap_or(0),
                1 => t.next_sibling(item).unwrap_or(0),
                2 => t.prev_sibling(item).unwrap_or(0),
                3 => t.node(item).map_or(0, |n| n.parent),
                4 => {
                    if item == 0 || item as isize == TVI_ROOT {
                        t.roots.first().copied().unwrap_or(0)
                    } else {
                        t.node(item).and_then(|n| n.children.first().copied()).unwrap_or(0)
                    }
                }
                5 => rows.get(((t.yoff + rh - 1) / rh) as usize).map_or(0, |r| r.0),
                6 => at.and_then(|a| rows.get(a + 1)).map_or(0, |r| r.0),
                7 => at.and_then(|a| a.checked_sub(1)).and_then(|a| rows.get(a)).map_or(0, |r| r.0),
                9 => t.sel,
                _ => 0,
            }) as isize
        }),
        // TVM_SELECTITEM: TVGN_CARET selects, FIRSTVISIBLE scrolls, DROPHILITE is accepted and ignored
        0x110B => {
            let id = lp as usize;
            match wp {
                9 => {
                    if id == 0 {
                        return with(|g| {
                            if let Some(t) = tv_mut(g, h) {
                                if let Some(n) = t.node_mut(t.sel) {
                                    n.state &= !TVIS_SELECTED;
                                }
                                t.sel = 0;
                            }
                            g.invalidate(h, None, false);
                            1
                        });
                    }
                    if with(|g| tv(g, h).is_none_or(|t| t.node(id).is_none())) {
                        return 0;
                    }
                    select(h, id, TVC_UNKNOWN) as isize
                }
                5 => with(|g| {
                    let rh = row_h(g, h);
                    if let Some(t) = tv_mut(g, h) {
                        if let Some(r) = t.rows().iter().position(|(i, _)| *i == id) {
                            t.yoff = r as i32 * rh;
                        }
                    }
                    sync_bars(g, h);
                    g.invalidate(h, None, false);
                    1
                }),
                _ => 1,
            }
        }
        // TVM_GETITEMA / W
        0x110C | 0x113E => with(|g| {
            let p = lp as *mut u8;
            let id = *(p.add(8) as *const u64) as usize;
            match tv(g, h).and_then(|t| t.node(id).map(|n| (n.clone(), t.has_children(id)))) {
                Some((n, kids)) => {
                    fill_tvitem(id, &n, kids, p, msg == 0x113E);
                    1
                }
                None => 0,
            }
        }),
        // TVM_SETITEMA / W
        0x110D | 0x113F => with(|g| {
            let p = lp as *const u8;
            let id = *(p.add(8) as *const u64) as usize;
            let Some(n) = tv_mut(g, h).and_then(|t| t.node_mut(id)) else { return 0 };
            apply_tvitem(n, p, msg == 0x113F);
            refresh(g, h);
            1
        }),
        0x1110 => with(|g| {
            let (_, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
            (ch / row_h(g, h)) as isize
        }),
        // TVM_HITTEST: TVHITTESTINFO { POINT pt; UINT flags; HTREEITEM hItem }
        0x1111 => with(|g| {
            let p = lp as *mut u8;
            let (x, y) = (*(p as *const i32), *(p.add(4) as *const i32));
            let (flags, id) = hit_test(g, h, x, y);
            *(p.add(8) as *mut u32) = flags;
            *(p.add(16) as *mut u64) = id as u64;
            id as isize
        }),
        // TVM_SORTCHILDREN: wParam recurse, lParam the parent
        0x1113 => with(|g| {
            let parent = if lp == TVI_ROOT { 0 } else { lp as usize };
            if let Some(t) = tv_mut(g, h) {
                t.sort_children(parent, wp != 0);
            }
            refresh(g, h);
            1
        }),
        // TVM_ENSUREVISIBLE
        0x1114 => with(|g| {
            let id = lp as usize;
            if let Some(t) = tv_mut(g, h) {
                t.expand_to(id);
            }
            sync_bars(g, h);
            scroll_to(g, h, id);
            g.invalidate(h, None, true);
            0
        }),
        // TVM_SORTCHILDRENCB: TVSORTCB { HTREEITEM hParent; PFNTVCOMPARE lpfnCompare; LPARAM lParam }
        0x1115 => {
            let p = lp as *const u64;
            let parent = if *p as i64 as isize == TVI_ROOT { 0 } else { *p as usize };
            let cmp: unsafe extern "win64" fn(isize, isize, isize) -> i32 = std::mem::transmute(*p.add(1) as usize);
            let sort_param = *p.add(2) as isize;
            let mut ids = with(|g| tv(g, h).map(|t| t.siblings(parent).iter().map(|i| (*i, t.node(*i).map_or(0, |n| n.param))).collect::<Vec<_>>())).unwrap_or_default();
            for i in 1..ids.len() {
                let mut j = i;
                while j > 0 && cmp(ids[j - 1].1, ids[j].1, sort_param) > 0 {
                    ids.swap(j - 1, j);
                    j -= 1;
                }
            }
            with(|g| {
                if let Some(t) = tv_mut(g, h) {
                    let order: Vec<usize> = ids.iter().map(|x| x.0).collect();
                    match t.node_mut(parent) {
                        Some(n) => n.children = order,
                        None => t.roots = order,
                    }
                }
                refresh(g, h);
            });
            1
        }
        0x111B => with(|g| {
            if let Some(t) = tv_mut(g, h) {
                t.item_h = wp as i32;
            }
            refresh(g, h);
            0
        }),
        0x111C => with(|g| row_h(g, h) as isize),
        0x111D => with(|g| {
            let old = tv(g, h).map_or(0, |t| t.bk);
            if let Some(t) = tv_mut(g, h) {
                t.bk = lp as u32;
            }
            g.invalidate(h, None, true);
            old as isize
        }),
        0x111F => with(|g| tv(g, h).map_or(0, |t| t.bk as isize)),
        0x111E => with(|g| {
            let old = tv(g, h).map_or(0, |t| t.text_color);
            if let Some(t) = tv_mut(g, h) {
                t.text_color = lp as u32;
            }
            g.invalidate(h, None, true);
            old as isize
        }),
        0x1120 => with(|g| tv(g, h).map_or(0, |t| t.text_color as isize)),
        0x1129 => with(|g| {
            let old = tv(g, h).map_or(0, |t| t.line_color);
            if let Some(t) = tv_mut(g, h) {
                t.line_color = lp as u32;
            }
            g.invalidate(h, None, true);
            old as isize
        }),
        0x112A => with(|g| tv(g, h).map_or(0, |t| t.line_color as isize)),
        // label editing, tool tips, insertion marks and the like: accepted, nothing to do
        0x110E | 0x110F | 0x1112 | 0x1116 | 0x1117 | 0x1118 | 0x1119 | 0x111A | 0x1121 | 0x1122 | 0x1123 | 0x1124 | 0x1125 | 0x1126 | 0x1127 | 0x1128 | 0x112B | 0x112C | 0x112D | 0x112E => 0,
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

fn refresh_all(h: usize) -> impl FnOnce(&mut Gui) {
    move |g| refresh(g, h)
}

/// Sends a click-like notification carrying the item under the point, and returns the parent's answer.
unsafe fn send_hit(h: usize, code: i32, pt: (i32, i32)) -> isize {
    notify_with(h, code, &pt_body(pt)).0
}

fn pt_body(pt: (i32, i32)) -> Vec<u8> {
    let mut b = vec![0u8; 8];
    b[0..4].copy_from_slice(&pt.0.to_le_bytes());
    b[4..8].copy_from_slice(&pt.1.to_le_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        let mut t = Tree::default();
        let a = t.insert(0, TVI_LAST, Node { text: "a".into(), ..Default::default() }, false);
        let b = t.insert(0, TVI_LAST, Node { text: "b".into(), ..Default::default() }, false);
        let a1 = t.insert(a, TVI_LAST, Node { text: "a1".into(), ..Default::default() }, false);
        t.insert(a, TVI_LAST, Node { text: "a2".into(), ..Default::default() }, false);
        t.insert(a1, TVI_LAST, Node { text: "deep".into(), ..Default::default() }, false);
        let _ = b;
        t
    }

    #[test]
    fn only_expanded_branches_show() {
        let mut t = tree();
        assert_eq!(t.rows().len(), 2);
        t.node_mut(1).unwrap().state |= TVIS_EXPANDED;
        let names: Vec<_> = t.rows().iter().map(|(i, d)| (t.node(*i).unwrap().text.clone(), *d)).collect();
        assert_eq!(names, vec![("a".into(), 0), ("a1".into(), 1), ("a2".into(), 1), ("b".into(), 0)]);
    }

    #[test]
    fn deleting_an_item_takes_its_subtree_and_moves_the_selection() {
        let mut t = tree();
        t.sel = 5; // "deep"
        assert!(t.delete(1)); // "a"
        assert_eq!(t.count(), 1);
        assert_eq!(t.roots, vec![2]);
        assert_eq!(t.sel, 2);
        assert!(!t.delete(1));
    }

    #[test]
    fn insertion_order_follows_hinsertafter() {
        let mut t = Tree::default();
        let x = t.insert(0, TVI_LAST, Node { text: "x".into(), ..Default::default() }, false);
        let y = t.insert(0, TVI_FIRST, Node { text: "y".into(), ..Default::default() }, false);
        let z = t.insert(0, x as isize, Node { text: "z".into(), ..Default::default() }, false);
        assert_eq!(t.roots, vec![y, x, z]);
        let m = t.insert(0, TVI_SORT, Node { text: "m".into(), ..Default::default() }, false);
        assert_eq!(t.roots.iter().position(|i| *i == m), Some(0), "\"m\" sorts before \"y\", \"x\" and \"z\"");
    }

    #[test]
    fn sorting_orders_children_by_label() {
        let mut t = Tree::default();
        for s in ["pear", "Apple", "fig"] {
            t.insert(0, TVI_LAST, Node { text: s.into(), ..Default::default() }, false);
        }
        t.sort_children(0, false);
        let names: Vec<_> = t.roots.iter().map(|i| t.node(*i).unwrap().text.clone()).collect();
        assert_eq!(names, vec!["Apple", "fig", "pear"]);
    }

    #[test]
    fn keys_move_through_the_shown_rows() {
        let mut t = tree();
        t.sel = 1;
        assert_eq!(key_target(&t, 0x28, 5), Some(2));
        assert_eq!(key_target(&t, 0x26, 5), Some(1));
        assert_eq!(key_target(&t, 0x23, 5), Some(2));
    }
}
