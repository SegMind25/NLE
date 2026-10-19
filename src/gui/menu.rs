//! Menus: the menu objects (`HMENU`), the menu bar in a top-level window, popup menus, and the
//! tracking of mouse and keyboard while a menu is open. Accelerator tables live here too.
//!
//! Tracking is a small state machine inside the window system (`MenuMode`): events that arrive
//! while a menu is open are consumed here instead of being turned into window messages. Popups
//! are real top-level windows (override-redirect on X11, so the window manager leaves them alone);
//! the commands a menu produces are posted to its owner as `WM_COMMAND`, as on Windows, or handed
//! back to `TrackPopupMenu` when it was asked for `TPM_RETURNCMD`.

use super::backend::Event;
use super::msg::Out;
use super::res::{self, Key};
use super::surface::*;
use super::wm::{ClassRef, CreateArgs, MENU_H};
use super::*;

// ---- flags -------------------------------------------------------------------------------------------
pub const MF_BYCOMMAND: u32 = 0;
pub const MF_GRAYED: u32 = 1;
pub const MF_DISABLED: u32 = 2;
pub const MF_BITMAP: u32 = 4;
pub const MF_CHECKED: u32 = 8;
pub const MF_POPUP: u32 = 0x10;
pub const MF_MENUBARBREAK: u32 = 0x20;
pub const MF_MENUBREAK: u32 = 0x40;
pub const MF_OWNERDRAW: u32 = 0x100;
pub const MFT_RADIOCHECK: u32 = 0x200;
pub const MF_BYPOSITION: u32 = 0x400;
pub const MF_SEPARATOR: u32 = 0x800;
pub const MF_DEFAULT: u32 = 0x1000;
pub const MF_RIGHTJUSTIFY: u32 = 0x4000;
const MF_END: u32 = 0x80;

pub const TPM_RIGHTALIGN: u32 = 8;
pub const TPM_BOTTOMALIGN: u32 = 0x20;
pub const TPM_RETURNCMD: u32 = 0x100;

pub const WM_INITMENU: u32 = 0x116;
pub const WM_INITMENUPOPUP: u32 = 0x117;
pub const WM_MENUSELECT: u32 = 0x11F;
pub const WM_ENTERMENULOOP: u32 = 0x211;
pub const WM_EXITMENULOOP: u32 = 0x212;
pub const SC_KEYMENU: usize = 0xF100;

#[derive(Clone, Debug, Default)]
pub struct MenuItem {
    /// The command id (for a popup item, the handle of the submenu is in `sub`).
    pub id: usize,
    pub text: String,
    pub sub: usize,
    /// `MF_*` bits: grayed, disabled, checked, separator, default, radio check, owner draw.
    pub flags: u32,
    pub data: usize,
}

impl MenuItem {
    pub fn separator(&self) -> bool {
        self.flags & MF_SEPARATOR != 0
    }

    pub fn grayed(&self) -> bool {
        self.flags & (MF_GRAYED | MF_DISABLED) != 0
    }

    pub fn is_popup(&self) -> bool {
        self.flags & MF_POPUP != 0 && self.sub != 0
    }
}

#[derive(Clone, Debug, Default)]
pub struct Menu {
    pub items: Vec<MenuItem>,
    pub popup: bool,
}

/// An entry of an accelerator table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accel {
    pub flags: u8,
    pub key: u16,
    pub cmd: u16,
}

pub const FVIRTKEY: u8 = 1;
pub const FSHIFT: u8 = 4;
pub const FCONTROL: u8 = 8;
pub const FALT: u8 = 0x10;

/// One open popup of the menu being tracked.
#[derive(Clone, Debug)]
pub struct Open {
    pub hmenu: usize,
    pub hwnd: usize,
    pub sel: i32,
    /// Item rectangles in the popup's client coordinates.
    pub rects: Vec<Rect>,
    pub size: (i32, i32),
    /// Screen position of the popup.
    pub at: (i32, i32),
}

#[derive(Clone, Debug, Default)]
pub struct MenuMode {
    pub owner: usize,
    /// The top-level window whose menu bar is active, or 0 for a context menu.
    pub bar: usize,
    pub bar_sel: i32,
    pub chain: Vec<Open>,
    pub keyboard: bool,
    pub ret_cmd: bool,
    pub result: Option<usize>,
    /// A `TrackPopupMenu` call is waiting for the menu to end.
    pub modal: bool,
}

/// Work that has to happen after the window procedures have been told (`WM_INITMENUPOPUP`).
#[derive(Clone, Copy, Debug)]
pub enum MenuOp {
    Open { hmenu: usize, x: i32, y: i32, select_first: bool },
}

// ---- look --------------------------------------------------------------------------------------------
const BAR_FACE: u32 = 0xF0F0F0;
const BAR_HOT: u32 = 0xE0E0E0;
const BAR_OPEN: u32 = 0xCCE4F7;
const POPUP_FACE: u32 = 0xF2F2F2;
const POPUP_BORDER: u32 = 0xA0A0A0;
const ITEM_HOT: u32 = 0x91C9F7;
const TEXT: u32 = 0x000000;
const TEXT_OFF: u32 = 0x838383;
const ITEM_H: i32 = 22;
const SEP_H: i32 = 9;
const PAD_Y: i32 = 3;
const CHECK_W: i32 = 30;

/// Splits a menu string into its label (without `&`), the index of the mnemonic character, and the accelerator
/// text after a tab.
pub fn split_text(text: &str) -> (String, Option<usize>, String) {
    let (label, accel) = text.split_once('\t').map_or((text, ""), |(a, b)| (a, b));
    let mut out = String::new();
    let mut mnemonic = None;
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.peek() {
                Some('&') => {
                    out.push('&');
                    chars.next();
                }
                Some(_) if mnemonic.is_none() => mnemonic = Some(out.chars().count()),
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    (out, mnemonic, accel.to_string())
}

fn mnemonic_of(text: &str) -> Option<char> {
    let (label, m, _) = split_text(text);
    m.and_then(|i| label.chars().nth(i)).map(|c| c.to_ascii_lowercase())
}

impl Gui {
    // ---- the objects -------------------------------------------------------------------------------

    pub fn menu_create(&mut self, popup: bool) -> usize {
        let h = self.alloc_handle();
        self.menus.insert(h, Menu { items: Vec::new(), popup });
        h
    }

    /// Destroys a menu and the menus below it.
    pub fn menu_destroy(&mut self, h: usize) -> bool {
        let Some(m) = self.menus.remove(&h) else { return false };
        for it in m.items {
            if it.is_popup() {
                self.menu_destroy(it.sub);
            }
        }
        true
    }

    /// Finds an item by command (searching submenus too) or by position. Returns the menu it is in and its index.
    pub fn menu_find(&self, h: usize, id: usize, flags: u32) -> Option<(usize, usize)> {
        let m = self.menus.get(&h)?;
        if flags & MF_BYPOSITION != 0 {
            return (id < m.items.len()).then_some((h, id));
        }
        for (i, it) in m.items.iter().enumerate() {
            if it.is_popup() {
                if let Some(r) = self.menu_find(it.sub, id, flags) {
                    return Some(r);
                }
            } else if it.id == id && !it.separator() {
                return Some((h, i));
            }
        }
        None
    }

    pub fn menu_insert(&mut self, h: usize, pos: Option<usize>, flags: u32, id: usize, text: String, data: usize) -> bool {
        let Some(m) = self.menus.get_mut(&h) else { return false };
        let sub = if flags & MF_POPUP != 0 { id } else { 0 };
        let item = MenuItem { id: if sub != 0 { 0 } else { id }, text, sub, flags: flags & !(MF_BYPOSITION | MF_BITMAP), data };
        match pos {
            Some(p) if p < m.items.len() => m.items.insert(p, item),
            _ => m.items.push(item),
        }
        true
    }

    pub fn menu_remove(&mut self, h: usize, id: usize, flags: u32, destroy_sub: bool) -> bool {
        let Some((mh, i)) = self.menu_find(h, id, flags) else { return false };
        let item = self.menus.get_mut(&mh).map(|m| m.items.remove(i));
        if let Some(it) = item {
            if destroy_sub && it.is_popup() {
                self.menu_destroy(it.sub);
            }
        }
        true
    }

    pub fn menu_item_mut(&mut self, h: usize, id: usize, flags: u32) -> Option<&mut MenuItem> {
        let (mh, i) = self.menu_find(h, id, flags)?;
        self.menus.get_mut(&mh)?.items.get_mut(i)
    }

    // ---- resources -----------------------------------------------------------------------------------

    /// Builds a menu from the bytes of an `RT_MENU` resource (`MENUITEMTEMPLATE` form).
    pub fn menu_from_template(&mut self, data: &[u8]) -> Option<usize> {
        let version = u16::from_le_bytes(data.get(0..2)?.try_into().ok()?);
        let header = u16::from_le_bytes(data.get(2..4)?.try_into().ok()?) as usize;
        if version != 0 {
            return None;
        }
        let mut pos = 4 + header;
        self.parse_menu_items(data, &mut pos, false)
    }

    fn parse_menu_items(&mut self, d: &[u8], pos: &mut usize, popup: bool) -> Option<usize> {
        let h = self.menu_create(popup);
        loop {
            let flags = u16::from_le_bytes(d.get(*pos..*pos + 2)?.try_into().ok()?) as u32;
            *pos += 2;
            let is_popup = flags & MF_POPUP != 0;
            let id = if is_popup {
                0
            } else {
                let v = u16::from_le_bytes(d.get(*pos..*pos + 2)?.try_into().ok()?) as usize;
                *pos += 2;
                v
            };
            let mut units = Vec::new();
            loop {
                let u = u16::from_le_bytes(d.get(*pos..*pos + 2)?.try_into().ok()?);
                *pos += 2;
                if u == 0 {
                    break;
                }
                units.push(u);
            }
            let text = String::from_utf16_lossy(&units);
            let mut item_flags = flags & !(MF_END | MF_POPUP);
            if text.is_empty() && id == 0 && !is_popup {
                item_flags |= MF_SEPARATOR;
            }
            if is_popup {
                let sub = self.parse_menu_items(d, pos, true)?;
                self.menu_insert(h, None, item_flags | MF_POPUP, sub, text, 0);
            } else {
                self.menu_insert(h, None, item_flags, id, text, 0);
            }
            if flags & MF_END != 0 {
                break;
            }
        }
        Some(h)
    }

    pub fn menu_from_resource(&mut self, hinst: usize, name: &Key) -> Option<usize> {
        let data = res::load(hinst, res::RT_MENU, name)?;
        self.menu_from_template(data)
    }

    // ---- accelerators ----------------------------------------------------------------------------

    pub fn accel_create(&mut self, table: Vec<Accel>) -> usize {
        let h = self.alloc_handle();
        self.accels.insert(h, table);
        h
    }

    /// Parses an `RT_ACCELERATOR` resource: 8-byte entries (flags, key, command), the last marked with 0x80.
    pub fn accel_from_resource(&mut self, hinst: usize, name: &Key) -> Option<usize> {
        let data = res::load(hinst, res::RT_ACCELERATOR, name)?;
        let mut table = Vec::new();
        for e in data.chunks_exact(8) {
            table.push(Accel { flags: e[0] & 0x7f, key: u16::from_le_bytes([e[2], e[3]]), cmd: u16::from_le_bytes([e[4], e[5]]) });
            if e[0] & 0x80 != 0 {
                break;
            }
        }
        Some(self.accel_create(table))
    }

    /// The command an accelerator table assigns to a key message, if any.
    pub fn accel_lookup(&self, table: usize, message: u32, wparam: usize, lparam: isize) -> Option<u16> {
        let t = self.accels.get(&table)?;
        // the modifiers recorded in the key message (see `process_events`)
        let (shift, ctrl, alt) = (lparam & (1 << 25) != 0, lparam & (1 << 26) != 0, lparam & (1 << 29) != 0);
        t.iter().find_map(|a| {
            let virt = a.flags & FVIRTKEY != 0;
            let hit = if virt {
                matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN) && wparam as u16 == a.key && (a.flags & FSHIFT != 0) == shift && (a.flags & FCONTROL != 0) == ctrl && (a.flags & FALT != 0) == alt
            } else {
                matches!(message, WM_CHAR | WM_SYSCHAR) && wparam as u16 == a.key
            };
            hit.then_some(a.cmd)
        })
    }

    // ---- the menu bar ----------------------------------------------------------------------------------

    fn menu_font(&self) -> super::font::Sized {
        self.sized_font(self.stock_object(17))
    }

    /// Rectangles of the menu bar's items in top-level surface coordinates.
    pub fn bar_item_rects(&self, top: usize) -> Vec<Rect> {
        let Some(w) = self.windows.get(&top) else { return Vec::new() };
        let Some(m) = self.menus.get(&w.menu) else { return Vec::new() };
        let font = self.menu_font();
        let mut x = 0;
        m.items
            .iter()
            .map(|it| {
                let (label, _, _) = split_text(&it.text);
                let width = if it.separator() { 8 } else { font.text_width(&label) + 16 };
                let r = Rect::new(x, 0, x + width, MENU_H);
                x += width;
                r
            })
            .collect()
    }

    pub fn bar_item_at(&self, top: usize, x: i32, y: i32) -> Option<usize> {
        if !(0..MENU_H).contains(&y) {
            return None;
        }
        self.bar_item_rects(top).iter().position(|r| r.contains(x, y))
    }

    /// Draws the menu bar of a top-level window into its surface.
    pub fn menu_paint_bar(&mut self, top: usize) {
        let Some(w) = self.windows.get(&top) else { return };
        let (menu, (cw, _)) = (w.menu, w.client_size());
        let Some(m) = self.menus.get(&menu).cloned() else { return };
        let width = cw + if w.style & WS_VSCROLL != 0 { super::wm::SB_W } else { 0 };
        let rects = self.bar_item_rects(top);
        let font = self.menu_font();
        let (sel, open, keyboard) = match &self.menu_mode {
            Some(mm) if mm.bar == top => (mm.bar_sel, !mm.chain.is_empty(), mm.keyboard),
            _ => (-1, false, false),
        };
        let enabled = self.is_enabled(top);
        let Some(surf) = self.windows.get_mut(&top).and_then(|t| t.surface.as_mut()) else { return };
        let mut cv = Canvas::new(surf, 0, 0, Rect::new(0, 0, width, MENU_H));
        cv.fill_rect(Rect::new(0, 0, width, MENU_H), BAR_FACE);
        cv.fill_rect(Rect::new(0, MENU_H - 1, width, MENU_H), 0xE0E0E0);
        for (i, (it, r)) in m.items.iter().zip(&rects).enumerate() {
            if it.separator() {
                continue;
            }
            if i as i32 == sel {
                cv.fill_rect(Rect::new(r.l, r.t, r.r, r.b - 1), if open { BAR_OPEN } else { BAR_HOT });
            }
            let (label, mnem, _) = split_text(&it.text);
            let color = if enabled && !it.grayed() { TEXT } else { TEXT_OFF };
            let ty = (MENU_H - 1 - font.height()) / 2;
            font.draw(&mut cv, r.l + 8, ty, &label, color, None);
            if let (Some(mi), true) = (mnem, keyboard) {
                let before = font.text_width(&label.chars().take(mi).collect::<String>());
                let cw1 = font.text_width(&label.chars().nth(mi).map(String::from).unwrap_or_default());
                cv.fill_rect(Rect::new(r.l + 8 + before, ty + font.height() - 1, r.l + 8 + before + cw1, ty + font.height()), color);
            }
        }
    }

    // ---- popup layout and painting --------------------------------------------------------------------

    /// Item rectangles and the size of the popup for `hmenu`.
    fn popup_layout(&self, hmenu: usize) -> (Vec<Rect>, (i32, i32)) {
        let Some(m) = self.menus.get(&hmenu) else { return (Vec::new(), (60, 8)) };
        let font = self.menu_font();
        let (mut label_w, mut accel_w) = (0, 0);
        for it in &m.items {
            let (label, _, accel) = split_text(&it.text);
            label_w = label_w.max(font.text_width(&label));
            accel_w = accel_w.max(font.text_width(&accel));
        }
        let width = (CHECK_W + label_w + if accel_w > 0 { 24 + accel_w } else { 0 } + 30).max(110);
        let mut y = PAD_Y;
        let rects = m
            .items
            .iter()
            .map(|it| {
                let h = if it.separator() { SEP_H } else { ITEM_H };
                let r = Rect::new(1, y, width - 1, y + h);
                y += h;
                r
            })
            .collect();
        (rects, (width, y + PAD_Y))
    }

    pub fn menu_paint_popup(&mut self, hwnd: usize) {
        let Some(open) = self.menu_mode.as_ref().and_then(|m| m.chain.iter().find(|o| o.hwnd == hwnd).cloned()) else { return };
        let Some(menu) = self.menus.get(&open.hmenu).cloned() else { return };
        let keyboard = self.menu_mode.as_ref().is_some_and(|m| m.keyboard);
        let font = self.menu_font();
        let Some(surf) = self.windows.get_mut(&hwnd).and_then(|w| w.surface.as_mut()) else { return };
        let bounds = surf.bounds();
        let mut cv = Canvas::new(surf, 0, 0, bounds);
        cv.fill_rect(bounds, POPUP_FACE);
        cv.frame_rect(bounds, POPUP_BORDER, 1);
        for (i, (it, r)) in menu.items.iter().zip(&open.rects).enumerate() {
            if it.separator() {
                cv.fill_rect(Rect::new(r.l + 28, r.t + SEP_H / 2, r.r - 4, r.t + SEP_H / 2 + 1), 0xCCCCCC);
                continue;
            }
            let hot = open.sel == i as i32;
            if hot {
                cv.fill_rect(*r, if it.grayed() { 0xE5E5E5 } else { ITEM_HOT });
            }
            let color = if it.grayed() { TEXT_OFF } else { TEXT };
            let (label, mnem, accel) = split_text(&it.text);
            let ty = r.t + (ITEM_H - font.height()) / 2;
            font.draw(&mut cv, r.l + CHECK_W - 2, ty, &label, color, None);
            if let (Some(mi), true) = (mnem, keyboard) {
                let before = font.text_width(&label.chars().take(mi).collect::<String>());
                let cw1 = font.text_width(&label.chars().nth(mi).map(String::from).unwrap_or_default());
                let x = r.l + CHECK_W - 2 + before;
                cv.fill_rect(Rect::new(x, ty + font.height() - 1, x + cw1, ty + font.height()), color);
            }
            if !accel.is_empty() {
                let w = font.text_width(&accel);
                font.draw(&mut cv, r.r - 30 - w, ty, &accel, color, None);
            }
            if it.flags & MF_CHECKED != 0 {
                let (cx, cy) = (r.l + 12, r.t + ITEM_H / 2);
                let pen = Pen { style: PenStyle::Solid, width: 1, color };
                if it.flags & MFT_RADIOCHECK != 0 {
                    cv.ellipse(Rect::new(cx - 3, cy - 3, cx + 4, cy + 4), &pen, &Brush::Solid(color));
                } else {
                    cv.line(cx - 4, cy, cx - 1, cy + 3, &pen);
                    cv.line(cx - 1, cy + 3, cx + 5, cy - 4, &pen);
                    cv.line(cx - 4, cy + 1, cx - 1, cy + 4, &pen);
                    cv.line(cx - 1, cy + 4, cx + 5, cy - 3, &pen);
                }
            }
            if it.is_popup() {
                let (ax, ay) = (r.r - 14, r.t + ITEM_H / 2);
                cv.fill_polygon_color(&[(ax, ay - 4), (ax, ay + 4), (ax + 4, ay)], color, false);
            }
        }
        surf.mark(bounds);
        self.flush();
    }

    // ---- opening and closing -----------------------------------------------------------------------------

    /// Starts tracking `hmenu` as a popup at screen position (x, y), or the menu bar of `bar`.
    pub fn menu_begin(&mut self, owner: usize, bar: usize, ret_cmd: bool, modal: bool, keyboard: bool) {
        if let Some(b) = self.backend.clone() {
            let top = if bar != 0 { bar } else { self.top_level_of(owner) };
            if let Some(w) = self.windows.get(&top) {
                b.grab_pointer(w.native, true);
            }
        }
        self.menu_mode = Some(MenuMode { owner, bar, bar_sel: -1, chain: Vec::new(), keyboard, ret_cmd, result: None, modal });
    }

    /// Ends menu tracking: closes every popup and gives the pointer back.
    pub fn menu_end(&mut self, command: Option<usize>) {
        let Some(mut mm) = self.menu_mode.take() else { return };
        for o in std::mem::take(&mut mm.chain) {
            self.remove_window_raw(o.hwnd);
        }
        if let Some(b) = self.backend.clone() {
            b.grab_pointer(0, false);
        }
        let bar = mm.bar;
        if let Some(cmd) = command {
            if mm.modal && mm.ret_cmd {
                self.menu_result = Some(cmd);
            } else if mm.owner != 0 {
                self.post_message(mm.owner, WM_COMMAND, cmd & 0xffff, 0);
            }
        }
        if mm.modal {
            self.menu_done = true;
        }
        let owner = mm.owner;
        self.post_message(owner, WM_EXITMENULOOP, 0, 0);
        if bar != 0 {
            self.paint_nc(bar);
        }
        self.flush();
    }

    fn close_from(&mut self, depth: usize) {
        let closed: Vec<Open> = match self.menu_mode.as_mut() {
            Some(mm) if depth < mm.chain.len() => mm.chain.drain(depth..).collect(),
            _ => return,
        };
        for o in closed {
            self.remove_window_raw(o.hwnd);
        }
    }

    /// Asks for a popup to be opened once the owner has been told (`WM_INITMENUPOPUP`).
    fn request_open(&mut self, hmenu: usize, x: i32, y: i32, select_first: bool, out: &mut Out) {
        let owner = self.menu_mode.as_ref().map_or(0, |m| m.owner);
        let time = self.tick();
        out.sends.push(Msg { hwnd: owner, message: WM_INITMENUPOPUP, wparam: hmenu, lparam: 0, time, pt: self.pointer });
        out.menu_ops.push(MenuOp::Open { hmenu, x, y, select_first });
    }

    /// Creates the popup window and shows it (the second half of `request_open`).
    pub fn menu_apply(&mut self, op: MenuOp) {
        let MenuOp::Open { hmenu, x, y, select_first } = op;
        if self.menu_mode.is_none() || !self.menus.contains_key(&hmenu) {
            return;
        }
        let (rects, size) = self.popup_layout(hmenu);
        let (sw, sh) = self.screen_size();
        let px = if x + size.0 > sw { (sw - size.0).max(0) } else { x };
        let py = if y + size.1 > sh { (sh - size.1).max(0) } else { y };
        let args = CreateArgs {
            ex_style: WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class: ClassRef::Name("#32768".into()),
            title: String::new(),
            style: WS_POPUP,
            x: px,
            y: py,
            w: size.0,
            h: size.1,
            parent: 0,
            menu: 0,
            hinstance: 0,
            param: 0,
            ansi: false,
        };
        let Ok(hwnd) = self.create_window_raw(&args, crate::runtime::handles::current_tid()) else { return };
        if let Some(w) = self.windows.get_mut(&hwnd) {
            w.style |= WS_VISIBLE;
            if let (Some(b), true) = (self.backend.clone(), w.native != 0) {
                b.show(w.native, true);
                b.raise(w.native);
            }
        }
        let sel = if select_first { rects.iter().zip(self.menus[&hmenu].items.iter()).position(|(_, it)| !it.separator()).map_or(-1, |i| i as i32) } else { -1 };
        if let Some(mm) = self.menu_mode.as_mut() {
            mm.chain.push(Open { hmenu, hwnd, sel, rects, size, at: (px, py) });
        }
        self.menu_paint_popup(hwnd);
    }

    // ---- choosing -------------------------------------------------------------------------------------------

    fn item_of(&self, hmenu: usize, i: i32) -> Option<MenuItem> {
        self.menus.get(&hmenu)?.items.get(usize::try_from(i).ok()?).cloned()
    }

    /// Highlights item `i` of the popup at `depth`, opening its submenu (and closing deeper popups).
    fn select_in(&mut self, depth: usize, i: i32, open_sub: bool, select_first: bool, out: &mut Out) {
        let Some(hmenu) = self.menu_mode.as_ref().and_then(|m| m.chain.get(depth)).map(|o| o.hmenu) else { return };
        let changed = self.menu_mode.as_mut().is_some_and(|m| std::mem::replace(&mut m.chain[depth].sel, i) != i);
        let reopen = changed || self.menu_mode.as_ref().is_some_and(|m| m.chain.len() > depth + 1) && open_sub;
        if changed || open_sub {
            if reopen {
                self.close_from(depth + 1);
            }
            let item = self.item_of(hmenu, i);
            if let Some(it) = &item {
                let time = self.tick();
                let owner = self.menu_mode.as_ref().map_or(0, |m| m.owner);
                let wp = (if it.is_popup() { i as usize } else { it.id } & 0xffff) | ((if it.is_popup() { 0x10usize } else { 0 } | if it.grayed() { 3 } else { 0 }) << 16);
                out.sends.push(Msg { hwnd: owner, message: WM_MENUSELECT, wparam: wp, lparam: hmenu as isize, time, pt: self.pointer });
                if it.is_popup() && !it.grayed() && open_sub && self.menu_mode.as_ref().is_some_and(|m| m.chain.len() == depth + 1) {
                    let open = self.menu_mode.as_ref().unwrap().chain[depth].clone();
                    if let Some(r) = open.rects.get(i as usize) {
                        self.request_open(it.sub, open.at.0 + r.r - 2, open.at.1 + r.t - 3, select_first, out);
                    }
                }
            }
        }
        let hwnd = self.menu_mode.as_ref().map_or(0, |m| m.chain[depth].hwnd);
        self.menu_paint_popup(hwnd);
    }

    /// Activates the highlighted item of the popup at `depth`: runs its command, or opens its submenu.
    fn activate(&mut self, depth: usize, out: &mut Out) {
        let Some(open) = self.menu_mode.as_ref().and_then(|m| m.chain.get(depth)).cloned() else { return };
        let Some(it) = self.item_of(open.hmenu, open.sel) else { return };
        if it.separator() || it.grayed() {
            return;
        }
        if it.is_popup() {
            self.select_in(depth, open.sel, true, self.menu_mode.as_ref().is_some_and(|m| m.keyboard), out);
        } else {
            self.menu_end(Some(it.id));
        }
    }

    // ---- input while a menu is open --------------------------------------------------------------------------

    /// The popup (innermost first) under a screen point, with the item there.
    fn popup_at(&self, x: i32, y: i32) -> Option<(usize, i32)> {
        let mm = self.menu_mode.as_ref()?;
        for (depth, o) in mm.chain.iter().enumerate().rev() {
            if x >= o.at.0 && y >= o.at.1 && x < o.at.0 + o.size.0 && y < o.at.1 + o.size.1 {
                let (lx, ly) = (x - o.at.0, y - o.at.1);
                let item = o.rects.iter().position(|r| r.contains(lx, ly)).map_or(-1, |i| i as i32);
                return Some((depth, item));
            }
        }
        None
    }

    /// Opens the popup of bar item `i`.
    fn open_bar_item(&mut self, top: usize, i: i32, select_first: bool, out: &mut Out) {
        let Some(w) = self.windows.get(&top) else { return };
        let Some(it) = self.item_of(w.menu, i) else { return };
        let rect = self.bar_item_rects(top).get(i as usize).copied().unwrap_or_default();
        self.close_from(0);
        if let Some(mm) = self.menu_mode.as_mut() {
            mm.bar_sel = i;
        }
        if it.is_popup() && !it.grayed() {
            let (sx, sy) = self.top_client_screen(top);
            self.request_open(it.sub, sx + rect.l, sy + MENU_H, select_first, out);
        } else if !it.is_popup() && !it.grayed() && !it.separator() {
            // a command on the bar itself
        }
        self.paint_nc(top);
    }

    pub fn open_bar_item_pub(&mut self, top: usize, i: i32, out: &mut Out) {
        self.open_bar_item(top, i, false, out);
    }

    /// Starts a context menu: asks for the first popup at (x, y), honouring the alignment flags.
    pub fn request_popup(&mut self, hmenu: usize, x: i32, y: i32, flags: u32, out: &mut Out) {
        let (_, size) = self.popup_layout(hmenu);
        let px = if flags & TPM_RIGHTALIGN != 0 { x - size.0 } else { x };
        let py = if flags & TPM_BOTTOMALIGN != 0 { y - size.1 } else { y };
        self.request_open(hmenu, px, py, false, out);
    }

    /// Consumes a mouse event while a menu is open; `(sx, sy)` is the pointer in screen coordinates.
    pub fn menu_mouse(&mut self, ev: &Event, sx: i32, sy: i32, top: usize, out: &mut Out) -> bool {
        let (down, up, motion) = match ev {
            Event::Button { down: true, button: 1, .. } => (true, false, false),
            Event::Button { down: false, button: 1, .. } => (false, true, false),
            Event::Motion { .. } => (false, false, true),
            Event::Button { .. } => return true,
            _ => return false,
        };
        let bar = self.menu_mode.as_ref().map_or(0, |m| m.bar);
        let popup = self.popup_at(sx, sy);
        // the menu bar of the owner window
        let bar_item = if bar != 0 {
            let (bx, by) = self.top_client_screen(bar);
            self.bar_item_at(bar, sx - bx, sy - by).map(|i| i as i32)
        } else {
            None
        };
        let _ = top;
        match popup {
            Some((depth, item)) => {
                if motion || down {
                    if item >= 0 {
                        let it = self.item_of(self.menu_mode.as_ref().unwrap().chain[depth].hmenu, item);
                        let open_sub = it.is_some_and(|i| i.is_popup());
                        self.select_in(depth, item, open_sub, false, out);
                    } else {
                        // between items
                        self.close_from(depth + 1);
                    }
                }
                if up && item >= 0 {
                    let on_sub = self.item_of(self.menu_mode.as_ref().unwrap().chain[depth].hmenu, item).is_some_and(|i| i.is_popup());
                    if let Some(m) = self.menu_mode.as_mut() {
                        m.chain[depth].sel = item;
                    }
                    if !on_sub {
                        self.activate(depth, out);
                    }
                }
            }
            None => match bar_item {
                Some(i) if self.menu_mode.as_ref().is_some_and(|m| m.bar_sel != i || m.chain.is_empty()) && (down || (motion && self.menu_mode.as_ref().is_some_and(|m| !m.chain.is_empty() || m.keyboard))) => {
                    let toggling = down && self.menu_mode.as_ref().is_some_and(|m| m.bar_sel == i);
                    if toggling {
                        self.menu_end(None);
                    } else {
                        self.open_bar_item(bar, i, false, out);
                    }
                }
                Some(i) if down && self.menu_mode.as_ref().is_some_and(|m| m.bar_sel == i && !m.chain.is_empty()) => self.menu_end(None),
                Some(_) => {}
                None => {
                    if down {
                        self.menu_end(None);
                    }
                }
            },
        }
        true
    }

    /// Consumes a key press while a menu is open.
    pub fn menu_key(&mut self, vk: u32, ch: Option<char>, out: &mut Out) -> bool {
        let Some(mm) = self.menu_mode.clone() else { return false };
        if let Some(m) = self.menu_mode.as_mut() {
            m.keyboard = true;
        }
        let depth = mm.chain.len();
        let dir = |sel: i32, hmenu: usize, step: i32, g: &Gui| -> i32 {
            let n = g.menus.get(&hmenu).map_or(0, |m| m.items.len() as i32);
            if n == 0 {
                return -1;
            }
            let mut i = sel;
            for _ in 0..n {
                i = if i < 0 { if step > 0 { 0 } else { n - 1 } } else { (i + step).rem_euclid(n) };
                if g.item_of(hmenu, i).is_some_and(|it| !it.separator()) {
                    return i;
                }
            }
            sel
        };
        match vk {
            0x1B => {
                // Esc: close the innermost popup; with none left, leave menu mode
                if depth > 0 {
                    self.close_from(depth - 1);
                    if depth == 1 && mm.bar == 0 {
                        self.menu_end(None);
                    } else if depth == 1 {
                        self.paint_nc(mm.bar);
                    }
                } else {
                    self.menu_end(None);
                }
            }
            0x26 | 0x28 => {
                let step = if vk == 0x28 { 1 } else { -1 };
                if depth > 0 {
                    let o = &mm.chain[depth - 1];
                    let i = dir(o.sel, o.hmenu, step, self);
                    self.select_in(depth - 1, i, false, false, out);
                } else if mm.bar != 0 && vk == 0x28 && mm.bar_sel >= 0 {
                    self.open_bar_item(mm.bar, mm.bar_sel, true, out);
                }
            }
            0x25 | 0x27 => {
                let right = vk == 0x27;
                if depth > 0 {
                    let o = mm.chain[depth - 1].clone();
                    let on_sub = self.item_of(o.hmenu, o.sel).is_some_and(|i| i.is_popup() && !i.grayed());
                    if right && on_sub {
                        self.select_in(depth - 1, o.sel, true, true, out);
                    } else if !right && depth > 1 {
                        self.close_from(depth - 1);
                    } else if mm.bar != 0 {
                        // move along the bar
                        let w = self.windows.get(&mm.bar).map_or(0, |w| w.menu);
                        let i = dir(mm.bar_sel, w, if right { 1 } else { -1 }, self);
                        self.open_bar_item(mm.bar, i, true, out);
                    }
                } else if mm.bar != 0 {
                    let w = self.windows.get(&mm.bar).map_or(0, |w| w.menu);
                    let i = dir(mm.bar_sel, w, if right { 1 } else { -1 }, self);
                    if let Some(m) = self.menu_mode.as_mut() {
                        m.bar_sel = i;
                    }
                    self.paint_nc(mm.bar);
                }
            }
            0x0D | 0x20 => {
                if depth > 0 {
                    self.activate(depth - 1, out);
                } else if mm.bar != 0 && mm.bar_sel >= 0 {
                    self.open_bar_item(mm.bar, mm.bar_sel, true, out);
                }
            }
            _ => {
                // a mnemonic letter picks an item of the menu that is open
                if let Some(c) = ch.map(|c| c.to_ascii_lowercase()).filter(|c| !c.is_control()) {
                    let (hmenu, in_bar) = if depth > 0 { (mm.chain[depth - 1].hmenu, false) } else { (self.windows.get(&mm.bar).map_or(0, |w| w.menu), true) };
                    let hits: Vec<usize> = self.menus.get(&hmenu).map_or(Vec::new(), |m| m.items.iter().enumerate().filter(|(_, it)| mnemonic_of(&it.text) == Some(c)).map(|(i, _)| i).collect());
                    if let Some(&first) = hits.first() {
                        if in_bar {
                            self.open_bar_item(mm.bar, first as i32, true, out);
                        } else {
                            let cur = mm.chain[depth - 1].sel.max(-1) as usize;
                            let pick = hits.iter().copied().find(|i| (*i as i32) > cur as i32).unwrap_or(first);
                            self.select_in(depth - 1, pick as i32, false, false, out);
                            if hits.len() == 1 {
                                self.activate(depth - 1, out);
                            }
                        }
                    }
                }
            }
        }
        true
    }

    /// Alt, F10 or Alt+letter: starts keyboard tracking of the menu bar of `top` (`SC_KEYMENU`).
    pub fn menu_key_start(&mut self, top: usize, ch: Option<char>, out: &mut Out) {
        let Some(w) = self.windows.get(&top) else { return };
        if w.menu == 0 || self.menu_mode.is_some() {
            return;
        }
        let menu = w.menu;
        self.menu_begin(top, top, false, false, true);
        let time = self.tick();
        out.sends.push(Msg { hwnd: top, message: WM_ENTERMENULOOP, wparam: 0, lparam: 0, time, pt: self.pointer });
        out.sends.push(Msg { hwnd: top, message: WM_INITMENU, wparam: menu, lparam: 0, time, pt: self.pointer });
        match ch {
            None => {
                let first = self.menus.get(&menu).and_then(|m| m.items.iter().position(|it| !it.separator())).map_or(-1, |i| i as i32);
                if let Some(mm) = self.menu_mode.as_mut() {
                    mm.bar_sel = first;
                }
                self.paint_nc(top);
            }
            Some(c) => {
                let c = c.to_ascii_lowercase();
                let hit = self.menus.get(&menu).and_then(|m| m.items.iter().position(|it| mnemonic_of(&it.text) == Some(c)));
                match hit {
                    Some(i) => self.open_bar_item(top, i as i32, true, out),
                    None => self.menu_end(None),
                }
            }
        }
    }
}

pub unsafe extern "win64" fn menu_popup_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_PAINT => {
            with(|g| {
                g.menu_paint_popup(h);
                g.validate(h, None);
            });
            0
        }
        WM_ERASEBKGND => 1,
        _ => super::msg::def_window_proc(h, msg, wp, lp, false),
    }
}

/// `TrackPopupMenu`: shows the menu at screen position (x, y) and waits for it to end. Returns the chosen command with
/// `TPM_RETURNCMD` (0 when none), else posts it to the owner as `WM_COMMAND` and returns 1.
pub unsafe fn track_popup(owner: usize, hmenu: usize, flags: u32, x: i32, y: i32) -> i32 {
    use super::msg::{dispatch, finish, next_message, translate, Filter, Next};
    let started = with(|g| {
        if !g.menus.contains_key(&hmenu) || g.menu_mode.is_some() {
            return false;
        }
        g.menu_done = false;
        g.menu_result = None;
        g.menu_begin(owner, 0, flags & TPM_RETURNCMD != 0, true, false);
        true
    });
    if !started {
        return 0;
    }
    let mut out = Out::default();
    with(|g| {
        let time = g.tick();
        out.sends.push(Msg { hwnd: owner, message: WM_ENTERMENULOOP, wparam: 1, lparam: 0, time, pt: g.pointer });
        g.request_popup(hmenu, x, y, flags, &mut out);
    });
    finish(out);
    let filter = Filter { hwnd: 0, min: 0, max: 0 };
    while !with(|g| g.menu_done) {
        match next_message(&filter, true, true) {
            Next::Msg(m) => {
                translate(&m);
                dispatch(&m);
            }
            Next::Quit(code) => {
                with(|g| {
                    g.menu_end(None);
                    g.thread_state(crate::runtime::handles::current_tid()).quit = Some(code);
                });
                break;
            }
            Next::None => {}
        }
    }
    let r = with(|g| {
        g.menu_done = false;
        g.menu_result.take()
    });
    if flags & TPM_RETURNCMD != 0 {
        r.map_or(0, |c| c as i32)
    } else {
        1
    }
}

/// `SC_KEYMENU`: Alt, F10 or Alt+letter pressed in a window (`ch` is the letter, if any).
pub unsafe fn key_start(h: usize, ch: Option<char>) {
    let top = with(|g| g.top_level_of(h));
    let mut out = Out::default();
    with(|g| g.menu_key_start(top, ch, &mut out));
    super::msg::finish(out);
}
