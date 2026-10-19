//! The window manager: window classes, the window tree, geometry, invalidation and painting
//! support, focus, and hit testing. Functions on `Gui` only change state; the free functions at
//! the bottom also send messages (which runs guest code, so they never hold the lock).

use super::backend::{Backend, Cursor, WinSpec};
use super::controls::Ctl;
use super::gdi::{Dc, Object, Target};
use super::surface::*;
use super::*;
use std::sync::Arc;

pub struct Class {
    pub name: String,
    pub atom: u16,
    pub style: u32,
    pub wndproc: usize,
    pub cls_extra: i32,
    pub wnd_extra: i32,
    pub hinstance: usize,
    pub icon: usize,
    pub cursor: usize,
    pub background: usize,
    pub menu_name: Option<String>,
    pub unicode: bool,
    pub native: bool,
}

pub struct Wnd {
    pub hwnd: usize,
    pub class: Arc<Class>,
    pub wndproc: usize,
    pub unicode: bool,
    pub parent: usize,
    pub owner: usize,
    pub thread: u32,
    /// Child windows, bottom of the z-order first.
    pub children: Vec<usize>,
    pub style: u32,
    pub ex_style: u32,
    /// Outer rectangle: screen coordinates for top-level windows, parent-client coordinates for children.
    pub rect: Rect,
    pub text: String,
    pub id: usize,
    pub userdata: isize,
    pub extra: Vec<u8>,
    pub native: u64,
    pub surface: Option<Surface>,
    pub update: Rect,
    pub erase: bool,
    pub menu: usize,
    pub font: usize,
    pub props: HashMap<String, usize>,
    pub ctl: Ctl,
    pub shown_once: bool,
    pub maximized: bool,
    pub minimized: bool,
    pub restore_rect: Option<Rect>,
    pub cursor: Cursor,
    pub sb: super::nc::Bars,
    /// `TrackMouseEvent(TME_LEAVE)` is pending.
    pub track_leave: bool,
    /// `SetWindowSubclass`: (procedure, id, reference data), the one installed last first; the window's own procedure
    /// is kept in `orig_proc` while `wndproc` points at the dispatcher.
    pub subclass: Vec<(usize, usize, usize)>,
    pub orig_proc: usize,
}

// ---- metrics (Windows 10 look) -----------------------------------------------------------------

pub const CAPTION_H: i32 = 23;
pub const MENU_H: i32 = 20;
/// Width of a scroll bar (and the height of a horizontal one).
pub const SB_W: i32 = 17;

pub fn frame_width(style: u32, ex: u32) -> i32 {
    if style & WS_THICKFRAME != 0 {
        8
    } else if style & WS_DLGFRAME != 0 || ex & WS_EX_DLGMODALFRAME != 0 {
        3
    } else if style & WS_BORDER != 0 {
        1
    } else {
        0
    }
}

/// Menus and drop-down lists: top-level windows that the window manager does not decorate or manage, so their
/// border is drawn here, inside the window.
pub fn is_popup(style: u32, ex: u32) -> bool {
    style & WS_POPUP != 0 && style & (WS_DLGFRAME | WS_THICKFRAME) == 0 && ex & WS_EX_NOACTIVATE != 0 && ex & WS_EX_TOOLWINDOW != 0
}

/// The frame and caption the display's window manager draws around a top-level window:
/// (left, top, right, bottom). Everything inside it is ours.
pub fn frame_inset(style: u32, ex: u32) -> Rect {
    if is_popup(style, ex) {
        return Rect::new(0, 0, 0, 0);
    }
    let f = frame_width(style, ex);
    let caption = if style & WS_CAPTION == WS_CAPTION { CAPTION_H } else { 0 };
    Rect::new(f, f + caption, f, f)
}

/// How far the client area sits inside the window's outer rectangle: (left, top, right, bottom).
/// A menu bar takes the top, scroll bars take the right and bottom.
pub fn insets(style: u32, ex: u32, has_menu: bool, top_level: bool) -> Rect {
    let sbv = if style & WS_VSCROLL != 0 { SB_W } else { 0 };
    let sbh = if style & WS_HSCROLL != 0 { SB_W } else { 0 };
    if top_level && !is_popup(style, ex) {
        let f = frame_inset(style, ex);
        let menu = if has_menu { MENU_H } else { 0 };
        Rect::new(f.l, f.t + menu, f.r + sbv, f.b + sbh)
    } else {
        let b = if ex & WS_EX_CLIENTEDGE != 0 {
            2
        } else if style & WS_BORDER != 0 || ex & WS_EX_STATICEDGE != 0 {
            1
        } else {
            0
        };
        Rect::new(b, b, b + sbv, b + sbh)
    }
}

impl Wnd {
    pub fn is_top_level(&self) -> bool {
        self.parent == 0
    }

    pub fn inset(&self) -> Rect {
        insets(self.style, self.ex_style, self.menu != 0, self.is_top_level())
    }

    /// The frame the window manager draws around a top-level window.
    pub fn frame(&self) -> Rect {
        frame_inset(self.style, self.ex_style)
    }

    /// Size of a top-level window's native window and surface: its outer size less the window manager's frame.
    /// That holds the menu bar and scroll bars as well as the client area.
    pub fn native_size(&self) -> (i32, i32) {
        let f = self.frame();
        ((self.rect.width() - f.l - f.r).max(1), (self.rect.height() - f.t - f.b).max(1))
    }

    pub fn client_size(&self) -> (i32, i32) {
        let i = self.inset();
        ((self.rect.width() - i.l - i.r).max(0), (self.rect.height() - i.t - i.b).max(0))
    }

    pub fn visible_flag(&self) -> bool {
        self.style & WS_VISIBLE != 0
    }
}

pub struct CreateArgs {
    pub ex_style: u32,
    pub class: ClassRef,
    pub title: String,
    pub style: u32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub parent: usize,
    pub menu: usize,
    pub hinstance: usize,
    pub param: usize,
    pub ansi: bool,
}

pub enum ClassRef {
    Name(String),
    Atom(u16),
}

pub const ERROR_CANNOT_FIND_WND_CLASS: u32 = 1407;
pub const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;
pub const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
pub const ERROR_TLW_WITH_WSCHILD: u32 = 1406;

impl Gui {
    // ---- backend -----------------------------------------------------------------------------

    pub fn ensure_backend(&mut self) -> Option<Arc<dyn Backend>> {
        if let Some(b) = &self.backend {
            return Some(b.clone());
        }
        if self.backend_failed {
            return None;
        }
        match super::backend::start() {
            Ok(b) => {
                self.backend = Some(b.clone());
                Some(b)
            }
            Err(e) => {
                eprintln!("nle: cannot open a window: {e}");
                self.backend_failed = true;
                None
            }
        }
    }

    pub fn screen_size(&mut self) -> (i32, i32) {
        match self.ensure_backend() {
            Some(b) => b.screen_size(),
            None => (1280, 800),
        }
    }

    // ---- classes -----------------------------------------------------------------------------

    pub fn register_class(&mut self, mut c: Class) -> Option<u16> {
        let key = c.name.to_lowercase();
        if self.classes.contains_key(&key) {
            return None;
        }
        c.atom = self.next_atom;
        self.next_atom += 1;
        self.atoms.insert(c.atom, key.clone());
        let atom = c.atom;
        self.classes.insert(key, Arc::new(c));
        Some(atom)
    }

    pub fn find_class(&self, r: &ClassRef) -> Option<Arc<Class>> {
        match r {
            ClassRef::Name(n) => self.classes.get(&n.to_lowercase()).cloned(),
            ClassRef::Atom(a) => self.atoms.get(a).and_then(|k| self.classes.get(k)).cloned(),
        }
    }

    // ---- lookups -----------------------------------------------------------------------------

    pub fn window(&self, h: usize) -> Option<&Wnd> {
        self.windows.get(&h)
    }

    pub fn window_mut(&mut self, h: usize) -> Option<&mut Wnd> {
        self.windows.get_mut(&h)
    }

    pub fn top_level_of(&self, mut h: usize) -> usize {
        while let Some(w) = self.windows.get(&h) {
            if w.parent == 0 {
                return h;
            }
            h = w.parent;
        }
        0
    }

    pub fn is_descendant(&self, h: usize, ancestor: usize) -> bool {
        let mut cur = h;
        while cur != 0 {
            if cur == ancestor {
                return true;
            }
            cur = self.windows.get(&cur).map_or(0, |w| w.parent);
        }
        false
    }

    /// Client-area origin of `h` in the coordinates of its top-level window's surface (which also holds the menu bar).
    pub fn client_origin_in_top(&self, h: usize) -> (i32, i32) {
        let (mut x, mut y) = (0, 0);
        let mut cur = h;
        while let Some(w) = self.windows.get(&cur) {
            let i = w.inset();
            if w.parent == 0 {
                let f = w.frame();
                x += i.l - f.l;
                y += i.t - f.t;
                break;
            }
            x += w.rect.l + i.l;
            y += w.rect.t + i.t;
            cur = w.parent;
        }
        (x, y)
    }

    /// Client rectangle of `h` in top-level surface coordinates.
    pub fn client_rect_in_top(&self, h: usize) -> Rect {
        let (ox, oy) = self.client_origin_in_top(h);
        match self.windows.get(&h) {
            Some(w) => {
                let (cw, ch) = w.client_size();
                Rect::new(ox, oy, ox + cw, oy + ch)
            }
            None => Rect::default(),
        }
    }

    /// Screen position of the top-level window's surface (the native window), just inside the manager's frame.
    pub fn top_client_screen(&self, top: usize) -> (i32, i32) {
        match self.windows.get(&top) {
            Some(w) => {
                let f = w.frame();
                (w.rect.l + f.l, w.rect.t + f.t)
            }
            None => (0, 0),
        }
    }

    pub fn client_to_screen(&self, h: usize, x: i32, y: i32) -> (i32, i32) {
        let top = self.top_level_of(h);
        let (ox, oy) = self.client_origin_in_top(h);
        let (sx, sy) = self.top_client_screen(top);
        (sx + ox + x, sy + oy + y)
    }

    pub fn screen_to_client(&self, h: usize, x: i32, y: i32) -> (i32, i32) {
        let (sx, sy) = self.client_to_screen(h, 0, 0);
        (x - sx, y - sy)
    }

    /// The window's outer rectangle in screen coordinates.
    pub fn window_rect_screen(&self, h: usize) -> Rect {
        match self.windows.get(&h) {
            Some(w) if w.parent == 0 => w.rect,
            Some(w) => {
                let i = w.inset();
                let (cx, cy) = self.client_to_screen(h, 0, 0);
                Rect::new(cx - i.l, cy - i.t, cx - i.l + w.rect.width(), cy - i.t + w.rect.height())
            }
            None => Rect::default(),
        }
    }

    pub fn is_visible(&self, h: usize) -> bool {
        let mut cur = h;
        while cur != 0 {
            match self.windows.get(&cur) {
                Some(w) if w.visible_flag() && !w.minimized => cur = w.parent,
                _ => return false,
            }
        }
        h != 0
    }

    pub fn is_enabled(&self, h: usize) -> bool {
        let mut cur = h;
        while cur != 0 {
            match self.windows.get(&cur) {
                Some(w) if w.style & WS_DISABLED == 0 => cur = w.parent,
                _ => return false,
            }
        }
        true
    }

    /// Visible, clipped client rectangle of `h` within the top-level surface.
    pub fn visible_client_in_top(&self, h: usize) -> Rect {
        let mut r = self.client_rect_in_top(h);
        let mut cur = h;
        while let Some(w) = self.windows.get(&cur) {
            if w.parent == 0 {
                break;
            }
            r = r.intersect(&self.client_rect_in_top(w.parent));
            cur = w.parent;
        }
        r
    }

    /// Deepest visible, enabled window containing the point (surface coordinates of `top`); the result is in that
    /// window's client coordinates, which lie outside its client rectangle when the point is on a menu bar or scroll bar.
    pub fn hit_test(&self, top: usize, x: i32, y: i32) -> (usize, i32, i32) {
        fn walk(g: &Gui, h: usize, x: i32, y: i32) -> (usize, i32, i32) {
            let Some(w) = g.windows.get(&h) else { return (h, x, y) };
            for &c in w.children.iter().rev() {
                let Some(cw) = g.windows.get(&c) else { continue };
                if !cw.visible_flag() {
                    continue;
                }
                // a plain STATIC label lets the mouse through to what is behind it
                if cw.class.native && cw.class.name == "STATIC" && cw.style & 0x100 == 0 {
                    continue;
                }
                let outer = cw.rect;
                if outer.contains(x, y) {
                    let ci = cw.inset();
                    return walk(g, c, x - outer.l - ci.l, y - outer.t - ci.t);
                }
            }
            (h, x, y)
        }
        let (ox, oy) = self.client_origin_in_top(top);
        walk(self, top, x - ox, y - oy)
    }

    // ---- window creation and destruction (state only) -----------------------------------------------

    /// Allocates the window and its native counterpart. Returns the handle, or sets the error code.
    pub fn create_window_raw(&mut self, a: &CreateArgs, thread: u32) -> Result<usize, u32> {
        let class = self.find_class(&a.class).ok_or(ERROR_CANNOT_FIND_WND_CLASS)?;
        let is_child = a.style & WS_CHILD != 0;
        if is_child && !self.windows.contains_key(&a.parent) {
            return Err(ERROR_TLW_WITH_WSCHILD);
        }
        let (sw, sh) = if is_child { (0, 0) } else { self.screen_size() };
        let parent = if is_child { a.parent } else { 0 };
        let owner = if !is_child && self.windows.contains_key(&a.parent) { a.parent } else { 0 };
        let default_size = (800, 600);
        let (x, y) = if a.x == CW_USEDEFAULT || a.y == CW_USEDEFAULT {
            let n = self.windows.values().filter(|w| w.parent == 0).count() as i32;
            ((60 + 28 * n).min((sw - 200).max(0)), (60 + 28 * n).min((sh - 200).max(0)))
        } else {
            (a.x, a.y)
        };
        let explicit_pos = a.x != CW_USEDEFAULT && a.y != CW_USEDEFAULT;
        let w = if a.w == CW_USEDEFAULT { if is_child { 100 } else { default_size.0 } } else { a.w };
        let h = if a.h == CW_USEDEFAULT { if is_child { 30 } else { default_size.1 } } else { a.h };
        let hwnd = self.alloc_handle();
        let menu = if !is_child && self.menus.contains_key(&a.menu) { a.menu } else { 0 };
        let style = a.style & !WS_VISIBLE;
        let frame = frame_inset(style, a.ex_style);
        let (cw, ch) = ((w - frame.l - frame.r).max(1), (h - frame.t - frame.b).max(1));
        let mut wnd = Wnd {
            hwnd,
            class: class.clone(),
            wndproc: class.wndproc,
            unicode: !a.ansi && class.unicode || (class.native && !a.ansi),
            parent,
            owner,
            thread,
            children: Vec::new(),
            style,
            ex_style: a.ex_style,
            rect: Rect::new(x, y, x + w, y + h),
            text: a.title.clone(),
            id: if is_child { a.menu } else { 0 },
            userdata: 0,
            extra: vec![0; class.wnd_extra.max(0) as usize],
            native: 0,
            surface: None,
            update: Rect::default(),
            erase: false,
            menu,
            font: 0,
            props: HashMap::new(),
            ctl: Ctl::default(),
            shown_once: false,
            maximized: false,
            minimized: false,
            restore_rect: None,
            cursor: Cursor::Arrow,
            sb: Default::default(),
            track_leave: false,
            subclass: Vec::new(),
            orig_proc: 0,
        };
        wnd.unicode = if class.native { !a.ansi } else { class.unicode };
        if !is_child {
            let Some(backend) = self.ensure_backend() else { return Err(8) };
            let transient = self.windows.get(&owner).map(|o| o.native).filter(|n| *n != 0);
            let spec = WinSpec {
                x: if explicit_pos { Some(x + frame.l) } else { None },
                y: if explicit_pos { Some(y + frame.t) } else { None },
                w: cw,
                h: ch,
                title: a.title.clone(),
                resizable: style & WS_THICKFRAME != 0,
                decorated: style & (WS_CAPTION | WS_THICKFRAME) != 0 || a.ex_style & WS_EX_DLGMODALFRAME != 0,
                tool: a.ex_style & WS_EX_TOOLWINDOW != 0,
                topmost: a.ex_style & WS_EX_TOPMOST != 0,
                popup: is_popup(style, a.ex_style),
                transient_for: transient,
            };
            wnd.native = backend.create_window(&spec);
            let mut s = Surface::new(cw, ch);
            s.fill(s.bounds(), 0xffffff);
            wnd.surface = Some(s);
            self.native_to_hwnd.insert(wnd.native, hwnd);
        }
        self.windows.insert(hwnd, wnd);
        if parent != 0 {
            if let Some(p) = self.windows.get_mut(&parent) {
                p.children.push(hwnd);
            }
        }
        Ok(hwnd)
    }

    /// Handles of `h` and everything below it, parents first.
    pub fn subtree(&self, h: usize) -> Vec<usize> {
        let mut out = vec![h];
        let mut i = 0;
        while i < out.len() {
            if let Some(w) = self.windows.get(&out[i]) {
                out.extend(w.children.iter().copied());
            }
            i += 1;
        }
        out
    }

    /// Removes a window from the tables (after its messages have been sent).
    pub fn remove_window_raw(&mut self, h: usize) {
        let Some(w) = self.windows.remove(&h) else { return };
        if w.parent != 0 {
            if let Some(p) = self.windows.get_mut(&w.parent) {
                p.children.retain(|c| *c != h);
            }
        }
        if w.native != 0 {
            self.native_to_hwnd.remove(&w.native);
            if let Some(b) = &self.backend {
                b.destroy_window(w.native);
            }
        }
        if w.menu != 0 {
            self.menu_destroy(w.menu);
        }
        if self.menu_mode.as_ref().is_some_and(|m| m.bar == h || m.owner == h) {
            self.menu_mode = None;
        }
        if self.focus == h {
            self.focus = 0;
        }
        if self.active == h {
            self.active = 0;
        }
        if self.capture == h {
            self.capture = 0;
        }
        for t in self.threads.values_mut() {
            t.queue.retain(|m| m.hwnd != h);
            t.timers.retain(|tm| tm.hwnd != h);
        }
        // device contexts of a window that is gone are useless
        let stale: Vec<usize> = self
            .objects
            .iter()
            .filter_map(|(k, o)| match o {
                Object::Dc(Dc { hwnd, paint: true, .. }) if *hwnd == h => Some(*k),
                _ => None,
            })
            .collect();
        for k in stale {
            self.objects.remove(&k);
        }
    }

    // ---- invalidation and painting ----------------------------------------------------------------

    pub fn invalidate(&mut self, h: usize, rect: Option<Rect>, erase: bool) {
        let Some(w) = self.windows.get(&h) else { return };
        let (cw, ch) = w.client_size();
        let client = Rect::new(0, 0, cw, ch);
        let r = match rect {
            Some(r) => r.normalized().intersect(&client),
            None => client,
        };
        if r.is_empty() {
            return;
        }
        let clip_children = w.style & WS_CLIPCHILDREN != 0;
        let children = w.children.clone();
        // the update area is one rectangle, so what gets painted can be larger than `r`: children have to be told about
        // all of it, or the parent's erase wipes them where nobody asked them to repaint
        let mut total = r;
        if let Some(w) = self.windows.get_mut(&h) {
            w.update = w.update.union(&r);
            w.erase |= erase;
            total = w.update;
        }
        if !clip_children {
            for c in children {
                let Some(cwnd) = self.windows.get(&c) else { continue };
                if !cwnd.visible_flag() {
                    continue;
                }
                let ci = cwnd.inset();
                let origin = (cwnd.rect.l + ci.l, cwnd.rect.t + ci.t);
                let in_child = total.offset(-origin.0, -origin.1);
                self.invalidate(c, Some(in_child), erase);
            }
        }
    }

    pub fn validate(&mut self, h: usize, rect: Option<Rect>) {
        let Some(w) = self.windows.get_mut(&h) else { return };
        match rect {
            None => {
                w.update = Rect::default();
                w.erase = false;
            }
            Some(r) => {
                let parts = super::gdi::rect_subtract(w.update, r.normalized());
                w.update = parts.iter().fold(Rect::default(), |a, p| a.union(p));
                if w.update.is_empty() {
                    w.erase = false;
                }
            }
        }
    }

    /// A DC for a window's client area. `clip_to` limits drawing (the update rectangle for `BeginPaint`).
    pub fn window_dc(&mut self, h: usize, paint: bool, clip_to: Option<Rect>) -> Option<usize> {
        let w = self.windows.get(&h)?;
        let top = self.top_level_of(h);
        self.windows.get(&top)?.surface.as_ref()?;
        let (ox, oy) = self.client_origin_in_top(h);
        let mut clip = self.visible_client_in_top(h);
        if let Some(r) = clip_to {
            clip = clip.intersect(&r.offset(ox, oy));
        }
        if w.style & WS_CLIPCHILDREN != 0 && paint {
            // children paint themselves; the parent's DC leaves them alone
            let kids: Vec<Rect> = w.children.iter().filter(|c| self.windows.get(c).is_some_and(|k| k.visible_flag())).map(|c| self.client_rect_in_top(*c)).collect();
            let _ = kids; // exclusion is approximated by painting children after their parent
        }
        Some(self.new_dc(Target::Window { top, ox, oy, clip }, h, paint))
    }

    /// Pushes everything drawn since the last flush to the display.
    pub fn flush(&mut self) {
        let Some(b) = self.backend.clone() else { return };
        for w in self.windows.values_mut() {
            if w.native == 0 {
                continue;
            }
            if let Some(s) = w.surface.as_mut() {
                let d = s.take_dirty();
                if !d.is_empty() && w.style & WS_VISIBLE != 0 {
                    b.present(w.native, s, d);
                }
            }
        }
    }

    pub fn set_title(&mut self, h: usize, text: &str) {
        if let Some(w) = self.windows.get_mut(&h) {
            w.text = text.to_string();
            if w.native != 0 {
                if let Some(b) = &self.backend {
                    b.set_title(w.native, text);
                }
            }
        }
    }

    /// Resizes a window's outer rectangle; top-level windows also move / resize their native window
    /// and reallocate the surface.
    pub fn set_rect(&mut self, h: usize, r: Rect) {
        let Some(w) = self.windows.get_mut(&h) else { return };
        let old = w.rect;
        w.rect = r;
        let (cw, ch) = w.client_size();
        let f = w.frame();
        let (nw, nh) = w.native_size();
        if w.parent == 0 {
            if old.width() != r.width() || old.height() != r.height() || w.surface.as_ref().is_none_or(|s| s.w != nw || s.h != nh) {
                let bg = 0xffffff;
                if let Some(s) = w.surface.take() {
                    w.surface = Some(s.resized(nw, nh, bg));
                }
                w.update = Rect::new(0, 0, cw, ch);
                w.erase = true;
            }
            if w.native != 0 {
                if let Some(b) = &self.backend {
                    b.move_resize(w.native, r.l + f.l, r.t + f.t, nw, nh);
                    b.set_resizable(w.native, w.style & WS_THICKFRAME != 0, nw, nh);
                }
            }
        } else if old.width() != r.width() || old.height() != r.height() {
            w.update = Rect::new(0, 0, cw, ch);
            w.erase = true;
        }
        let parent = w.parent;
        if parent != 0 {
            // the area the child left and the one it covers now both need repainting
            let area = old.union(&r);
            if let Some(p) = self.windows.get(&parent) {
                let _ = p;
            }
            self.invalidate(parent, Some(area), true);
        }
    }

    pub fn bring_to_top(&mut self, h: usize) {
        let Some(w) = self.windows.get(&h) else { return };
        let parent = w.parent;
        if parent == 0 {
            if let (Some(b), n) = (self.backend.clone(), w.native) {
                if n != 0 {
                    b.raise(n);
                }
            }
            return;
        }
        if let Some(p) = self.windows.get_mut(&parent) {
            p.children.retain(|c| *c != h);
            p.children.push(h);
        }
        self.invalidate(h, None, true);
    }

    /// Next window of `thread` that has something to paint (parents before children).
    pub fn next_paint_window(&self, thread: u32) -> Option<usize> {
        let mut tops: Vec<usize> = self.windows.values().filter(|w| w.parent == 0 && w.thread == thread).map(|w| w.hwnd).collect();
        tops.sort();
        for t in tops {
            for h in self.subtree(t) {
                if let Some(w) = self.windows.get(&h) {
                    if !w.update.is_empty() && self.is_visible(h) {
                        return Some(h);
                    }
                }
            }
        }
        None
    }
}

pub fn client_rect_size(g: &Gui, h: usize) -> Option<(i32, i32)> {
    g.windows.get(&h).map(|w| w.client_size())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::headless::Headless;

    fn gui() -> Gui {
        let mut g = Gui::new();
        g.backend = Some(Arc::new(Headless::with_script((1280, 800), "")));
        g
    }

    fn class(g: &mut Gui, name: &str) {
        g.register_class(Class { name: name.into(), atom: 0, style: 0, wndproc: 0, cls_extra: 0, wnd_extra: 8, hinstance: 0, icon: 0, cursor: 0, background: 6, menu_name: None, unicode: false, native: false });
    }

    fn make(g: &mut Gui, name: &str, style: u32, x: i32, y: i32, w: i32, h: i32, parent: usize) -> usize {
        g.create_window_raw(&CreateArgs { ex_style: 0, class: ClassRef::Name(name.into()), title: "t".into(), style, x, y, w, h, parent, menu: 0, hinstance: 0, param: 0, ansi: true }, 1).unwrap()
    }

    #[test]
    fn overlapped_window_client_area_matches_windows_10() {
        let mut g = gui();
        class(&mut g, "Frame");
        let h = make(&mut g, "frame", 0x00CF_0000, 100, 100, 480, 240, 0);
        // WS_OVERLAPPEDWINDOW: 8 px frame, 23 px caption (31 on top)
        assert_eq!(g.window(h).unwrap().client_size(), (464, 201));
        assert_eq!(g.window(h).unwrap().surface.as_ref().unwrap().w, 464);
    }

    #[test]
    fn key_messages_follow_the_focus_at_the_time_they_are_fetched() {
        let mut g = gui();
        class(&mut g, "K");
        let top = make(&mut g, "k", 0x10CF_0000, 0, 0, 300, 200, 0);
        let a = make(&mut g, "k", WS_CHILD | WS_VISIBLE, 0, 0, 50, 20, top);
        let b = make(&mut g, "k", WS_CHILD | WS_VISIBLE, 60, 0, 50, 20, top);
        let other = make(&mut g, "k", 0x10CF_0000, 400, 0, 100, 100, 0);
        g.focus = a;
        let mut key = Msg { hwnd: a, message: WM_KEYDOWN, wparam: 0x20, lparam: 0, time: 0, pt: (0, 0) };
        // the key arrived while `a` had the focus, but the program moved it to `b` before fetching the message
        g.focus = b;
        g.retarget_key(&mut key);
        assert_eq!(key.hwnd, b);
        let mut click = Msg { hwnd: a, message: WM_LBUTTONDOWN, ..key };
        g.retarget_key(&mut click);
        assert_eq!(click.hwnd, a, "mouse messages are not redirected");
        let mut elsewhere = Msg { hwnd: other, message: WM_CHAR, ..key };
        g.retarget_key(&mut elsewhere);
        assert_eq!(elsewhere.hwnd, other, "a key for another top-level window stays where it is");
    }

    #[test]
    fn a_key_sent_to_a_window_that_has_not_taken_the_focus_yet_stays_in_that_window() {
        use crate::gui::backend::Event;
        let mut g = gui();
        class(&mut g, "K");
        let owner = make(&mut g, "k", 0x10CF_0000, 0, 0, 300, 200, 0);
        let field = make(&mut g, "k", WS_CHILD | WS_VISIBLE, 0, 0, 50, 20, owner);
        let dialog = make(&mut g, "k", 0x10CF_0000, 400, 0, 200, 100, 0);
        let edit = make(&mut g, "k", WS_CHILD | WS_VISIBLE, 0, 0, 50, 20, dialog);
        // the dialog is still opening: the focus is in its owner
        g.focus = field;
        let native = g.window(dialog).unwrap().native;
        assert_ne!(native, 0);
        g.process_events(vec![Event::Key { win: native, down: true, vk: 0x45, scan: 0x12, ch: Some('e'), ext: false, repeat: false }]);
        let queued = g.threads.get(&1).map(|t| t.queue.iter().map(|m| m.hwnd).collect::<Vec<_>>()).unwrap_or_default();
        assert_eq!(queued, vec![dialog], "the key goes to the dialog, not to the owner's focus");
        // once the dialog has the focus, fetching the message sends it to the control that has it
        g.focus = edit;
        let mut m = g.threads.get(&1).unwrap().queue[0];
        g.retarget_key(&mut m);
        assert_eq!(m.hwnd, edit);
    }

    #[test]
    fn a_character_is_queued_before_the_keys_that_came_in_after_its_press() {
        let mut g = gui();
        class(&mut g, "K");
        let top = make(&mut g, "k", 0x10CF_0000, 0, 0, 300, 200, 0);
        let key = |vk: usize| Msg { hwnd: top, message: WM_KEYDOWN, wparam: vk, lparam: 0, time: 0, pt: (0, 0) };
        let timer = Msg { hwnd: top, message: WM_TIMER, wparam: 1, lparam: 0, time: 0, pt: (0, 0) };
        // the program is busy: a timer message, then the keys "v" and Tab, are waiting; "e" has just been fetched
        for m in [timer, key(0x56), key(0x09)] {
            g.post_to_thread(1, m);
        }
        g.post_char(top, WM_CHAR, 'e' as usize, 0);
        let order: Vec<(u32, usize)> = g.threads.get(&1).unwrap().queue.iter().map(|m| (m.message, m.wparam)).collect();
        assert_eq!(order, vec![(WM_TIMER, 1), (WM_CHAR, 'e' as usize), (WM_KEYDOWN, 0x56), (WM_KEYDOWN, 0x09)]);
    }

    #[test]
    fn registering_a_class_twice_fails_and_lookup_ignores_case() {
        let mut g = gui();
        class(&mut g, "Same");
        assert!(g.register_class(Class { name: "SAME".into(), atom: 0, style: 0, wndproc: 0, cls_extra: 0, wnd_extra: 0, hinstance: 0, icon: 0, cursor: 0, background: 0, menu_name: None, unicode: false, native: false }).is_none());
        assert!(g.find_class(&ClassRef::Name("same".into())).is_some());
        assert!(g.create_window_raw(&CreateArgs { ex_style: 0, class: ClassRef::Name("nope".into()), title: String::new(), style: 0, x: 0, y: 0, w: 1, h: 1, parent: 0, menu: 0, hinstance: 0, param: 0, ansi: true }, 1).is_err());
    }

    #[test]
    fn child_coordinates_translate_through_the_tree() {
        let mut g = gui();
        class(&mut g, "C");
        let top = make(&mut g, "c", WS_POPUP, 200, 150, 300, 200, 0);
        let mid = make(&mut g, "c", WS_CHILD | WS_BORDER, 10, 20, 100, 100, top);
        let leaf = make(&mut g, "c", WS_CHILD, 5, 6, 20, 20, mid);
        // mid has a 1px border, so its client origin is (11, 21); leaf adds (5, 6)
        assert_eq!(g.client_origin_in_top(leaf), (16, 27));
        assert_eq!(g.client_to_screen(leaf, 0, 0), (216, 177));
        assert_eq!(g.screen_to_client(leaf, 216, 177), (0, 0));
        assert_eq!(g.top_level_of(leaf), top);
    }

    #[test]
    fn hit_testing_picks_the_topmost_child() {
        let mut g = gui();
        class(&mut g, "C");
        let top = make(&mut g, "c", WS_POPUP, 0, 0, 200, 200, 0);
        let a = make(&mut g, "c", WS_CHILD | WS_VISIBLE, 10, 10, 100, 100, top);
        let b = make(&mut g, "c", WS_CHILD | WS_VISIBLE, 50, 50, 100, 100, top);
        g.window_mut(a).unwrap().style |= WS_VISIBLE;
        g.window_mut(b).unwrap().style |= WS_VISIBLE;
        assert_eq!(g.hit_test(top, 60, 60).0, b, "b is later in the z-order");
        assert_eq!(g.hit_test(top, 20, 20).0, a);
        assert_eq!(g.hit_test(top, 190, 5).0, top);
        let (_, lx, ly) = g.hit_test(top, 60, 70);
        assert_eq!((lx, ly), (10, 20));
    }

    #[test]
    fn invalidation_accumulates_and_propagates_to_children() {
        let mut g = gui();
        class(&mut g, "C");
        let top = make(&mut g, "c", WS_POPUP, 0, 0, 200, 200, 0);
        let kid = make(&mut g, "c", WS_CHILD, 20, 20, 50, 50, top);
        g.window_mut(kid).unwrap().style |= WS_VISIBLE;
        g.invalidate(top, Some(Rect::new(10, 10, 40, 40)), true);
        g.invalidate(top, Some(Rect::new(30, 30, 60, 60)), false);
        assert_eq!(g.window(top).unwrap().update, Rect::new(10, 10, 60, 60));
        assert!(g.window(top).unwrap().erase);
        assert!(!g.window(kid).unwrap().update.is_empty(), "the child overlaps the dirty area");
        g.validate(top, None);
        assert!(g.window(top).unwrap().update.is_empty());
    }

    #[test]
    fn removing_a_window_cleans_up_references() {
        let mut g = gui();
        class(&mut g, "C");
        let top = make(&mut g, "c", WS_POPUP, 0, 0, 100, 100, 0);
        let kid = make(&mut g, "c", WS_CHILD, 0, 0, 10, 10, top);
        g.focus = kid;
        g.remove_window_raw(kid);
        assert!(g.window(top).unwrap().children.is_empty());
        assert_eq!(g.focus, 0);
        assert_eq!(g.subtree(top), vec![top]);
    }

    #[test]
    fn resizing_a_top_level_window_reallocates_its_surface() {
        let mut g = gui();
        class(&mut g, "C");
        let top = make(&mut g, "c", WS_POPUP, 0, 0, 100, 100, 0);
        g.set_rect(top, Rect::new(0, 0, 300, 150));
        let w = g.window(top).unwrap();
        assert_eq!((w.surface.as_ref().unwrap().w, w.surface.as_ref().unwrap().h), (300, 150));
        assert!(!w.update.is_empty() && w.erase);
    }

    #[test]
    fn insets_for_common_styles() {
        assert_eq!(insets(0, 0, false, false), Rect::new(0, 0, 0, 0));
        assert_eq!(insets(WS_BORDER, 0, false, false), Rect::new(1, 1, 1, 1));
        assert_eq!(insets(0, WS_EX_CLIENTEDGE, false, false), Rect::new(2, 2, 2, 2));
        let o = insets(0x00CF_0000, 0, true, true);
        assert_eq!(o, Rect::new(8, 8 + 23 + 20, 8, 8));
        // scroll bars take the right and bottom edges, inside any border
        assert_eq!(insets(WS_VSCROLL | WS_HSCROLL, WS_EX_CLIENTEDGE, false, false), Rect::new(2, 2, 2 + SB_W, 2 + SB_W));
    }
}
