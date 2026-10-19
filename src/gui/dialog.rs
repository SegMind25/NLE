//! Dialog boxes: parsing dialog templates (classic `DLGTEMPLATE` and `DLGTEMPLATEEX`), creating the
//! dialog window and its controls, the modal loop of `DialogBox`, `EndDialog`, the default dialog
//! procedure of the `#32770` class, and keyboard navigation (`IsDialogMessage`).
//!
//! A dialog window belongs to the `#32770` class, whose window procedure is `def_dlg_proc`. The
//! program's dialog procedure is kept in the window's extra bytes (`DWLP_DLGPROC`); a message goes
//! to it first, and only what it declines is handled here.

use super::api;
use super::gdi::{LogFont, Object};
use super::msg::{call_proc, current_ansi, def_window_proc};
use super::res::{self, Key};
use super::surface::*;
use super::wm::{ClassRef, CreateArgs};
use super::*;

pub const IDOK: usize = 1;
pub const IDCANCEL: usize = 2;
pub const DM_GETDEFID: u32 = 0x400;
pub const DM_SETDEFID: u32 = 0x401;
pub const WM_NEXTDLGCTL: u32 = 0x28;
const DC_HASDEFID: usize = 0x534B;

pub const DLGC_WANTARROWS: usize = 1;
pub const DLGC_WANTTAB: usize = 2;
pub const DLGC_WANTALLKEYS: usize = 4;
pub const DLGC_WANTCHARS: usize = 0x80;
pub const DLGC_BUTTON: usize = 0x2000;
pub const DLGC_RADIOBUTTON: usize = 0x40;
pub const DLGC_DEFPUSHBUTTON: usize = 0x10;
pub const DLGC_UNDEFPUSHBUTTON: usize = 0x20;
pub const DLGC_STATIC: usize = 0x100;

const DS_SETFONT: u32 = 0x40;
const DS_MODALFRAME: u32 = 0x80;
const DS_CENTER: u32 = 0x800;
const DS_ABSALIGN: u32 = 1;

// DWLP_* offsets in a dialog window's extra bytes
const EXTRA_MSGRESULT: usize = 0;
const EXTRA_DLGPROC: usize = 8;

// ---- templates ------------------------------------------------------------------------------------------

/// A class or title in a template: absent, an ordinal, or a string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ord {
    None,
    Id(u16),
    Name(String),
}

#[derive(Clone, Debug)]
pub struct DlgFont {
    pub points: u16,
    pub weight: u16,
    pub italic: bool,
    pub face: String,
}

#[derive(Clone, Debug)]
pub struct DlgItem {
    pub style: u32,
    pub ex_style: u32,
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    pub id: u32,
    pub class: Ord,
    pub title: Ord,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct DlgTemplate {
    pub style: u32,
    pub ex_style: u32,
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    pub menu: Ord,
    pub class: Ord,
    pub title: String,
    pub font: Option<DlgFont>,
    pub items: Vec<DlgItem>,
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let b = self.d.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(b)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.bytes(2)?.try_into().ok()?))
    }
    fn i16(&mut self) -> Option<i32> {
        Some(self.u16()? as i16 as i32)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }
    fn align4(&mut self) {
        self.p = (self.p + 3) & !3;
    }
    fn wstr(&mut self) -> Option<String> {
        let mut units = Vec::new();
        loop {
            let u = self.u16()?;
            if u == 0 {
                return Some(String::from_utf16_lossy(&units));
            }
            units.push(u);
            if units.len() > 4096 {
                return None;
            }
        }
    }
    /// A menu, class or title: 0 for none, 0xFFFF and an ordinal, or a string.
    fn ord(&mut self) -> Option<Ord> {
        let first = u16::from_le_bytes(self.d.get(self.p..self.p + 2)?.try_into().ok()?);
        match first {
            0 => {
                self.p += 2;
                Some(Ord::None)
            }
            0xFFFF => {
                self.p += 2;
                Some(Ord::Id(self.u16()?))
            }
            _ => Some(Ord::Name(self.wstr()?)),
        }
    }
}

/// Parses a dialog template of either format. Returns `None` for anything malformed.
pub fn parse_template(d: &[u8]) -> Option<DlgTemplate> {
    let mut r = Reader { d, p: 0 };
    let extended = d.len() >= 4 && d[0..2] == [1, 0] && d[2..4] == [0xFF, 0xFF];
    let (style, ex_style, count, x, y, cx, cy);
    if extended {
        r.bytes(8)?; // version, signature, help id
        ex_style = r.u32()?;
        style = r.u32()?;
    } else {
        style = r.u32()?;
        ex_style = r.u32()?;
    }
    count = r.u16()?;
    x = r.i16()?;
    y = r.i16()?;
    cx = r.i16()?;
    cy = r.i16()?;
    let menu = r.ord()?;
    let class = r.ord()?;
    let title = r.wstr()?;
    let font = if style & DS_SETFONT != 0 {
        let points = r.u16()?;
        let (weight, italic) = if extended {
            let w = r.u16()?;
            let i = r.u8()? != 0;
            r.u8()?; // character set
            (w, i)
        } else {
            (0, false)
        };
        Some(DlgFont { points, weight, italic, face: r.wstr()? })
    } else {
        None
    };
    let mut items = Vec::new();
    for _ in 0..count {
        r.align4();
        let (istyle, iex, ix, iy, icx, icy, id);
        if extended {
            r.u32()?; // help id
            iex = r.u32()?;
            istyle = r.u32()?;
            ix = r.i16()?;
            iy = r.i16()?;
            icx = r.i16()?;
            icy = r.i16()?;
            id = r.u32()?;
        } else {
            istyle = r.u32()?;
            iex = r.u32()?;
            ix = r.i16()?;
            iy = r.i16()?;
            icx = r.i16()?;
            icy = r.i16()?;
            id = r.u16()? as u32;
        }
        let class = r.ord()?;
        let title = r.ord()?;
        let extra = r.u16()? as usize;
        let data = r.bytes(extra)?.to_vec();
        items.push(DlgItem { style: istyle, ex_style: iex, x: ix, y: iy, cx: icx, cy: icy, id, class, title, data });
    }
    Some(DlgTemplate { style, ex_style, x, y, cx, cy, menu, class, title, font, items })
}

/// The window class a template item names: a predefined ordinal, or its own class name.
pub fn class_name(c: &Ord) -> Option<String> {
    match c {
        Ord::Id(0x80) => Some("BUTTON".into()),
        Ord::Id(0x81) => Some("EDIT".into()),
        Ord::Id(0x82) => Some("STATIC".into()),
        Ord::Id(0x83) => Some("LISTBOX".into()),
        Ord::Id(0x84) => Some("SCROLLBAR".into()),
        Ord::Id(0x85) => Some("COMBOBOX".into()),
        Ord::Name(n) => Some(n.clone()),
        _ => None,
    }
}

/// Dialog units to pixels: x by `base_x / 4`, y by `base_y / 8` (rounded like `MulDiv`).
pub fn units_to_px(v: i32, base: i32, per: i32) -> i32 {
    (v * base + per / 2) / per
}

// ---- creation -------------------------------------------------------------------------------------------

impl Gui {
    /// The font a dialog uses and its base units: (font handle, average character width, height).
    fn dialog_font(&mut self, t: &DlgTemplate) -> (usize, i32, i32) {
        let font = match &t.font {
            Some(f) => {
                let lf = LogFont { height: -(f.points as i32 * 96 / 72), width: 0, weight: if f.weight > 0 { f.weight as i32 } else { 400 }, italic: f.italic, underline: false, strike: false, pitch_family: 0x22, face: f.face.clone() };
                self.add_object(Object::Font(lf))
            }
            None => self.stock_object(17),
        };
        let (bx, by) = self.base_units(font);
        (font, bx, by)
    }

    /// Windows' dialog base units for a font: the average width of the lower- and upper-case alphabet, and the height.
    pub fn base_units(&self, font: usize) -> (i32, i32) {
        let f = self.sized_font(font);
        let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        (((f.text_width(alphabet) + 26) / 52).max(1), f.height().max(1))
    }

    /// The first control a dialog gives the focus to: a visible, enabled tab stop.
    pub fn first_tab_stop(&self, dlg: usize) -> usize {
        self.windows.get(&dlg).map_or(Vec::new(), |w| w.children.clone()).into_iter().find(|c| self.tabbable(*c)).unwrap_or(0)
    }

    pub fn tabbable(&self, c: usize) -> bool {
        self.windows.get(&c).is_some_and(|w| w.style & WS_TABSTOP != 0 && w.style & WS_DISABLED == 0 && w.visible_flag())
    }

    /// The dialog's default push button id: set with `DM_SETDEFID`, else the button with `BS_DEFPUSHBUTTON`, else IDOK.
    pub fn default_id(&self, dlg: usize) -> usize {
        let w = match self.windows.get(&dlg) {
            Some(w) => w,
            None => return IDOK,
        };
        if let Some(id) = w.props.get("__defid") {
            return *id;
        }
        w.children.iter().find_map(|c| self.windows.get(c).filter(|cw| cw.class.native && cw.class.name == "BUTTON" && cw.style & 0xF == 1).map(|cw| cw.id)).unwrap_or(IDOK)
    }
}

/// Creates the dialog window and its controls and sends `WM_INITDIALOG`. Returns the dialog's handle (0 on failure).
unsafe fn create_dialog(hinst: usize, t: &DlgTemplate, owner: usize, dlgproc: usize, param: isize, ansi: bool) -> usize {
    let (font, bx, by) = with(|g| g.dialog_font(t));
    let (cw, ch) = (units_to_px(t.cx, bx, 4), units_to_px(t.cy, by, 8));
    let mut style = t.style;
    let mut ex = t.ex_style;
    if style & DS_MODALFRAME != 0 {
        ex |= WS_EX_DLGMODALFRAME;
    }
    // a dialog window is a popup
    if style & WS_CHILD == 0 {
        style |= WS_POPUP;
    }
    let menu = match &t.menu {
        Ord::Id(id) => with(|g| g.menu_from_resource(hinst, &Key::Id(*id))).unwrap_or(0),
        Ord::Name(n) => with(|g| g.menu_from_resource(hinst, &Key::Name(n.clone()))).unwrap_or(0),
        Ord::None => 0,
    };
    let inset = super::wm::insets(style & !WS_VISIBLE, ex, menu != 0, true);
    let (ow, oh) = (cw + inset.l + inset.r, ch + inset.t + inset.b);
    let (sw, sh) = with(|g| g.screen_size());
    let owner_rect = with(|g| g.windows.contains_key(&owner).then(|| (g.window_rect_screen(owner), g.client_to_screen(owner, 0, 0))));
    let (x, y) = if style & DS_CENTER != 0 {
        let (cx, cy) = owner_rect.map_or((sw / 2, sh / 2), |(r, _)| ((r.l + r.r) / 2, (r.t + r.b) / 2));
        ((cx - ow / 2).clamp(0, (sw - ow).max(0)), (cy - oh / 2).clamp(0, (sh - oh).max(0)))
    } else {
        let (px, py) = (units_to_px(t.x, bx, 4), units_to_px(t.y, by, 8));
        match owner_rect {
            Some((_, (ox, oy))) if style & DS_ABSALIGN == 0 => (ox + px - inset.l, oy + py - inset.t),
            _ => (px, py),
        }
    };
    let class = match &t.class {
        Ord::Name(n) if !n.is_empty() => n.clone(),
        _ => "#32770".into(),
    };
    let dlg = api::create_window_ex(CreateArgs { ex_style: ex, class: ClassRef::Name(class), title: t.title.clone(), style: style & !WS_VISIBLE, x, y, w: ow, h: oh, parent: owner, menu, hinstance: hinst, param: 0, ansi });
    if dlg == 0 {
        return 0;
    }
    with(|g| {
        if let Some(w) = g.windows.get_mut(&dlg) {
            if w.extra.len() >= 16 {
                w.extra[EXTRA_DLGPROC..EXTRA_DLGPROC + 8].copy_from_slice(&(dlgproc as u64).to_le_bytes());
            }
            w.font = font;
            w.props.insert("__dialog".into(), 1);
        }
    });
    let mut visible_flag = style & WS_VISIBLE;
    for it in &t.items {
        let Some(cls) = class_name(&it.class) else { continue };
        let title = match &it.title {
            Ord::Name(n) => n.clone(),
            _ => String::new(),
        };
        let (x, y, w, h) = (units_to_px(it.x, bx, 4), units_to_px(it.y, by, 8), units_to_px(it.cx, bx, 4), units_to_px(it.cy, by, 8));
        let c = api::create_window_ex(CreateArgs { ex_style: it.ex_style, class: ClassRef::Name(cls), title, style: it.style | WS_CHILD, x, y, w, h, parent: dlg, menu: it.id as usize, hinstance: hinst, param: if it.data.is_empty() { 0 } else { it.data.as_ptr() as usize }, ansi });
        if c == 0 {
            continue;
        }
        api::send(c, WM_SETFONT, font, 0);
        // a static control that shows an icon or bitmap names it by ordinal
        if let (Ord::Id(rid), "STATIC") = (&it.title, with(|g| g.windows.get(&c).map_or(String::new(), |w| w.class.name.clone())).as_str()) {
            let kind = it.style & 0x1F;
            if kind == 3 {
                let img = res::load_icon_image(hinst, &Key::Id(*rid), 32);
                let icon = with(|g| match img {
                    Some(i) => g.add_object(Object::Icon(std::sync::Arc::new(i))),
                    None => 0,
                });
                api::send(c, 0x170, icon, 0);
            } else if kind == 0xE {
                let surf = res::load(hinst, res::RT_BITMAP, &Key::Id(*rid)).and_then(res::bitmap_from_dib);
                let bmp = with(|g| match surf {
                    Some(s) => g.add_object(Object::Bitmap(super::gdi::Bitmap { surf: s, bpp: 32, bits: 0 })),
                    None => 0,
                });
                api::send(c, 0x172, 0, bmp as isize);
            }
        }
    }
    let focus = with(|g| g.first_tab_stop(dlg));
    let handled = api::send_ansi(dlg, WM_INITDIALOG, focus, param, ansi);
    if handled != 0 && focus != 0 {
        // the program did not move the focus itself
        let moved = with(|g| g.focus != 0 && g.is_descendant(g.focus, dlg));
        if !moved {
            api::set_focus(focus);
        }
    }
    visible_flag &= WS_VISIBLE;
    if visible_flag != 0 {
        api::show_window(dlg, SW_SHOWNORMAL);
    }
    dlg
}

fn template_bytes(hinst: usize, name: usize, wide: bool) -> Option<&'static [u8]> {
    res::load(hinst, res::RT_DIALOG, &Key::from_arg(name, wide))
}

/// A template in memory has no stated length; this reads far enough to cover it.
unsafe fn template_in_memory(p: *const u8) -> &'static [u8] {
    std::slice::from_raw_parts(p, 0x10000)
}

pub unsafe fn create_dialog_param(hinst: usize, template: Option<&[u8]>, owner: usize, dlgproc: usize, param: isize, ansi: bool) -> usize {
    let Some(t) = template.and_then(parse_template) else {
        crate::win::set_last_error(1814);
        return 0;
    };
    create_dialog(hinst, &t, owner, dlgproc, param, ansi)
}

/// `DialogBoxParam` and friends: runs the dialog until `EndDialog`, and returns its result (-1 if it could not be made).
pub unsafe fn dialog_box_param(hinst: usize, template: Option<&[u8]>, owner: usize, dlgproc: usize, param: isize, ansi: bool) -> isize {
    use super::msg::{dispatch, next_message, translate, Filter, Next};
    let Some(t) = template.and_then(parse_template) else {
        crate::win::set_last_error(1814);
        return -1;
    };
    // a modal dialog is shown as soon as it exists
    let mut t = t;
    t.style |= WS_VISIBLE;
    let owner = if with(|g| g.windows.contains_key(&owner)) { owner } else { 0 };
    let owner_was_enabled = owner != 0 && with(|g| g.windows.get(&owner).is_some_and(|w| w.style & WS_DISABLED == 0));
    if owner_was_enabled {
        with(|g| {
            if let Some(w) = g.windows.get_mut(&owner) {
                w.style |= WS_DISABLED;
            }
        });
    }
    let dlg = create_dialog(hinst, &t, owner, dlgproc, param, ansi);
    if dlg == 0 {
        if owner_was_enabled {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&owner) {
                    w.style &= !WS_DISABLED;
                }
            });
        }
        return -1;
    }
    let filter = Filter { hwnd: 0, min: 0, max: 0 };
    let result = loop {
        if let Some(r) = with(|g| g.dialog_results.remove(&dlg)) {
            break r;
        }
        if !with(|g| g.windows.contains_key(&dlg)) {
            break -1;
        }
        match next_message(&filter, true, true) {
            Next::Msg(m) => {
                if !is_dialog_message(dlg, &m) {
                    translate(&m);
                    dispatch(&m);
                }
            }
            Next::Quit(code) => {
                // a quit posted while the dialog was open still has to reach the program
                with(|g| g.thread_state(crate::runtime::handles::current_tid()).quit = Some(code));
                break -1;
            }
            Next::None => {}
        }
    };
    if owner_was_enabled {
        with(|g| {
            if let Some(w) = g.windows.get_mut(&owner) {
                w.style &= !WS_DISABLED;
            }
        });
    }
    api::destroy_window(dlg);
    if owner != 0 {
        api::activate_window(owner);
    }
    result
}

pub unsafe fn dialog_box_resource(hinst: usize, name: usize, owner: usize, dlgproc: usize, param: isize, wide: bool) -> isize {
    dialog_box_param(hinst, template_bytes(hinst, name, wide), owner, dlgproc, param, !wide)
}

pub unsafe fn dialog_box_indirect(hinst: usize, template: *const u8, owner: usize, dlgproc: usize, param: isize, wide: bool) -> isize {
    dialog_box_param(hinst, Some(template_in_memory(template)), owner, dlgproc, param, !wide)
}

pub unsafe fn create_dialog_resource(hinst: usize, name: usize, owner: usize, dlgproc: usize, param: isize, wide: bool) -> usize {
    create_dialog_param(hinst, template_bytes(hinst, name, wide), owner, dlgproc, param, !wide)
}

pub unsafe fn create_dialog_indirect(hinst: usize, template: *const u8, owner: usize, dlgproc: usize, param: isize, wide: bool) -> usize {
    create_dialog_param(hinst, Some(template_in_memory(template)), owner, dlgproc, param, !wide)
}

/// `EndDialog`: ends the modal loop of `dlg` with `result` and hides it.
pub unsafe fn end_dialog(dlg: usize, result: isize) -> bool {
    let ok = with(|g| {
        if !g.windows.contains_key(&dlg) {
            return false;
        }
        g.dialog_results.insert(dlg, result);
        true
    });
    if ok {
        api::show_window(dlg, SW_HIDE);
    }
    ok
}

// ---- the default dialog procedure ---------------------------------------------------------------------------

fn extra_u64(g: &Gui, h: usize, at: usize) -> u64 {
    g.windows.get(&h).and_then(|w| w.extra.get(at..at + 8)).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap()))
}

pub unsafe extern "win64" fn def_dlg_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    // the program's procedure sees the message first
    let dlgproc = with(|g| extra_u64(g, h, EXTRA_DLGPROC)) as usize;
    if dlgproc != 0 {
        let r = call_proc(dlgproc, h, msg, wp, lp);
        if r != 0 {
            return match msg {
                // these return their value directly; everything else leaves it in DWLP_MSGRESULT
                WM_INITDIALOG | WM_CTLCOLORMSGBOX | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN | WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | 0x39 | 0x2F | 0x3A | 0x3B => r,
                _ => with(|g| extra_u64(g, h, EXTRA_MSGRESULT)) as isize,
            };
        }
    }
    match msg {
        WM_INITDIALOG => 0,
        WM_ERASEBKGND => {
            let brush = api::send(h, WM_CTLCOLORDLG, wp, h as isize) as usize;
            with(|g| {
                let (cw, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
                let b = if brush != 0 { g.brush_arg(brush) } else { Brush::Solid(super::gdi::sys_color(15)) };
                g.fill_rect(wp, Rect::new(0, 0, cw, ch), &b);
            });
            1
        }
        WM_CLOSE => {
            api::send(h, WM_COMMAND, IDCANCEL, 0);
            0
        }
        WM_SETFOCUS => {
            // focus goes to the first control, unless a control of the dialog already has it
            let target = with(|g| if g.focus != 0 && g.is_descendant(g.focus, h) && g.focus != h { 0 } else { g.first_tab_stop(h) });
            if target != 0 {
                api::set_focus(target);
            }
            0
        }
        DM_GETDEFID => {
            let id = with(|g| g.default_id(h));
            ((DC_HASDEFID << 16) | id) as isize
        }
        DM_SETDEFID => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.props.insert("__defid".into(), wp);
                }
                let kids: Vec<usize> = g.windows.get(&h).map(|w| w.children.clone()).unwrap_or_default();
                for k in kids {
                    if let Some(c) = g.windows.get_mut(&k) {
                        if c.class.native && c.class.name == "BUTTON" && c.style & 0xF <= 1 {
                            c.style = (c.style & !0xF) | if c.id == wp { 1 } else { 0 };
                            g.invalidate(k, None, true);
                        }
                    }
                }
            });
            1
        }
        WM_NEXTDLGCTL => {
            let target = if lp != 0 { wp } else { with(|g| next_tab(g, h, g.focus, wp != 0)) };
            if target != 0 {
                api::set_focus(target);
                select_all_if_edit(target);
            }
            0
        }
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

// ---- keyboard navigation -----------------------------------------------------------------------------------

/// The next (or previous) tab stop after `from` among the dialog's controls, wrapping around.
pub fn next_tab(g: &Gui, dlg: usize, from: usize, prev: bool) -> usize {
    let kids: Vec<usize> = g.windows.get(&dlg).map_or(Vec::new(), |w| w.children.clone());
    if kids.is_empty() {
        return 0;
    }
    let start = kids.iter().position(|k| *k == from).map_or(if prev { 0 } else { kids.len() - 1 }, |i| i);
    for step in 1..=kids.len() {
        let i = if prev { (start + kids.len() - step % kids.len()) % kids.len() } else { (start + step) % kids.len() };
        if g.tabbable(kids[i]) {
            return kids[i];
        }
    }
    0
}

unsafe fn select_all_if_edit(h: usize) {
    if with(|g| g.windows.get(&h).is_some_and(|w| w.class.native && w.class.name == "EDIT")) {
        api::send(h, 0xB1, 0, -1);
    }
}

fn is_button_of(g: &Gui, h: usize, types: &[u32]) -> bool {
    g.windows.get(&h).is_some_and(|w| w.class.native && w.class.name == "BUTTON" && types.contains(&(w.style & 0xF)))
}

/// The controls of the group `from` belongs to (a group starts at a `WS_GROUP` control).
fn group_of(g: &Gui, dlg: usize, from: usize) -> Vec<usize> {
    let kids: Vec<usize> = g.windows.get(&dlg).map_or(Vec::new(), |w| w.children.clone());
    let Some(pos) = kids.iter().position(|k| *k == from) else { return Vec::new() };
    let start = (0..=pos).rev().find(|i| g.windows.get(&kids[*i]).is_some_and(|w| w.style & WS_GROUP != 0)).unwrap_or(0);
    let end = (pos + 1..kids.len()).find(|i| g.windows.get(&kids[*i]).is_some_and(|w| w.style & WS_GROUP != 0)).unwrap_or(kids.len());
    kids[start..end].to_vec()
}

unsafe fn press_default(dlg: usize) {
    let id = with(|g| g.default_id(dlg));
    let btn = with(|g| g.windows.get(&dlg).and_then(|w| w.children.iter().copied().find(|c| g.windows.get(c).is_some_and(|cw| cw.id == id))));
    match btn {
        Some(b) if with(|g| g.is_enabled(b)) => {
            api::send(dlg, WM_COMMAND, id | ((0usize) << 16), b as isize);
        }
        Some(_) => {}
        None => {
            if id == IDOK {
                api::send(dlg, WM_COMMAND, IDOK, 0);
            }
        }
    }
}

unsafe fn mnemonic_target(dlg: usize, c: char) -> bool {
    let c = c.to_ascii_lowercase();
    let hit = with(|g| {
        let kids = g.windows.get(&dlg).map(|w| w.children.clone()).unwrap_or_default();
        kids.iter().position(|k| g.windows.get(k).is_some_and(|w| w.visible_flag() && w.style & WS_DISABLED == 0 && (w.class.name == "BUTTON" || w.class.name == "STATIC") && super::menu::split_text(&w.text).1.and_then(|i| super::menu::split_text(&w.text).0.chars().nth(i)).map(|x| x.to_ascii_lowercase()) == Some(c)))
    });
    let Some(i) = hit else { return false };
    let kids = with(|g| g.windows.get(&dlg).map(|w| w.children.clone()).unwrap_or_default());
    let target = kids[i];
    let is_button = with(|g| g.windows.get(&target).is_some_and(|w| w.class.name == "BUTTON"));
    if is_button {
        api::set_focus(target);
        api::send(target, 0xF5, 0, 0); // BM_CLICK
    } else {
        // a label hands the focus to the next control
        let next = with(|g| (i + 1..kids.len()).map(|j| kids[j]).find(|k| g.tabbable(*k)).unwrap_or(0));
        if next != 0 {
            api::set_focus(next);
            select_all_if_edit(next);
        }
    }
    true
}

/// `IsDialogMessage`: handles Tab, Enter, Esc, arrow keys in groups and mnemonics for a message meant for the dialog
/// or one of its controls. Returns true when the message was dealt with and must not be dispatched.
pub unsafe fn is_dialog_message(dlg: usize, m: &Msg) -> bool {
    if !with(|g| g.windows.contains_key(&dlg) && (m.hwnd == dlg || g.is_descendant(m.hwnd, dlg))) {
        return false;
    }
    let focus = with(|g| if g.focus != 0 && g.is_descendant(g.focus, dlg) { g.focus } else { m.hwnd });
    let code = api::send(focus, WM_GETDLGCODE, m.wparam, 0) as usize;
    match m.message {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = m.wparam as u32;
            match vk {
                0x09 => {
                    if code & DLGC_WANTTAB != 0 {
                        return false;
                    }
                    let shift = with(|g| g.keys[0x10] & 0x80 != 0);
                    let next = with(|g| next_tab(g, dlg, focus, shift));
                    if next != 0 {
                        api::set_focus(next);
                        select_all_if_edit(next);
                    }
                    true
                }
                0x0D => {
                    if code & DLGC_WANTALLKEYS != 0 {
                        return false;
                    }
                    if with(|g| is_button_of(g, focus, &[0, 1])) {
                        api::send(focus, 0xF5, 0, 0);
                    } else {
                        press_default(dlg);
                    }
                    true
                }
                0x1B => {
                    if code & DLGC_WANTALLKEYS != 0 {
                        return false;
                    }
                    let btn = with(|g| g.windows.get(&dlg).and_then(|w| w.children.iter().copied().find(|c| g.windows.get(c).is_some_and(|cw| cw.id == IDCANCEL))));
                    if btn.is_some_and(|b| !with(|g| g.is_enabled(b))) {
                        return true;
                    }
                    api::send(dlg, WM_COMMAND, IDCANCEL, btn.unwrap_or(0) as isize);
                    true
                }
                0x25..=0x28 => {
                    if code & DLGC_WANTARROWS != 0 || !with(|g| is_button_of(g, focus, &[0, 1, 4, 9])) {
                        return false;
                    }
                    let forward = matches!(vk, 0x27 | 0x28);
                    let group: Vec<usize> = with(|g| group_of(g, dlg, focus).into_iter().filter(|c| g.windows.get(c).is_some_and(|w| w.visible_flag() && w.style & WS_DISABLED == 0 && w.class.name == "BUTTON")).collect());
                    let Some(pos) = group.iter().position(|c| *c == focus) else { return false };
                    let next = group[(pos as i32 + if forward { 1 } else { -1 }).rem_euclid(group.len() as i32) as usize];
                    api::set_focus(next);
                    if with(|g| is_button_of(g, next, &[9])) {
                        api::send(next, 0xF5, 0, 0);
                    }
                    true
                }
                _ => false,
            }
        }
        WM_CHAR => {
            // the characters of keys handled above must not reach an edit control
            matches!(m.wparam, 0x09 | 0x0D | 0x1B) && code & (DLGC_WANTALLKEYS | DLGC_WANTTAB) == 0 && (m.wparam != 0x0D || code & DLGC_WANTCHARS == 0 || true)
        }
        WM_SYSCHAR => char::from_u32(m.wparam as u32).is_some_and(|c| mnemonic_target(dlg, c)),
        _ => false,
    }
}

/// The class registration for `#32770`.
pub fn register_class(g: &mut Gui) {
    g.register_class(super::wm::Class {
        name: "#32770".into(),
        atom: 0,
        style: 0,
        wndproc: def_dlg_proc as *const () as usize,
        cls_extra: 0,
        wnd_extra: 32,
        hinstance: 0,
        icon: 0,
        cursor: 0,
        background: 16,
        menu_name: None,
        unicode: true,
        native: true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn w32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn wstr(v: &mut Vec<u8>, s: &str) {
        for u in s.encode_utf16() {
            w16(v, u);
        }
        w16(v, 0);
    }
    fn align(v: &mut Vec<u8>) {
        while v.len() % 4 != 0 {
            v.push(0);
        }
    }

    /// A classic template: a dialog with a font and two controls (a button and an edit with creation data).
    fn classic() -> Vec<u8> {
        let mut v = Vec::new();
        w32(&mut v, WS_POPUP | WS_CAPTION | DS_SETFONT | DS_MODALFRAME);
        w32(&mut v, 0);
        w16(&mut v, 2);
        for n in [10i16, 20, 200, 100] {
            w16(&mut v, n as u16);
        }
        w16(&mut v, 0); // no menu
        w16(&mut v, 0); // default class
        wstr(&mut v, "Hello");
        w16(&mut v, 9);
        wstr(&mut v, "MS Shell Dlg");
        align(&mut v);
        w32(&mut v, WS_CHILD | WS_VISIBLE | WS_TABSTOP | 1);
        w32(&mut v, 0);
        for n in [140i16, 80, 50, 14] {
            w16(&mut v, n as u16);
        }
        w16(&mut v, 1); // id
        w16(&mut v, 0xFFFF);
        w16(&mut v, 0x80);
        wstr(&mut v, "OK");
        w16(&mut v, 0);
        align(&mut v);
        w32(&mut v, WS_CHILD | WS_VISIBLE | WS_TABSTOP);
        w32(&mut v, 0x200);
        for n in [7i16, 7, 120, 14] {
            w16(&mut v, n as u16);
        }
        w16(&mut v, 100);
        w16(&mut v, 0xFFFF);
        w16(&mut v, 0x81);
        w16(&mut v, 0xFFFF);
        w16(&mut v, 77); // title is an ordinal
        w16(&mut v, 3);
        v.extend_from_slice(&[1, 2, 3]);
        v
    }

    #[test]
    fn parses_a_classic_template() {
        let t = parse_template(&classic()).unwrap();
        assert_eq!((t.x, t.y, t.cx, t.cy), (10, 20, 200, 100));
        assert_eq!(t.title, "Hello");
        let f = t.font.unwrap();
        assert_eq!((f.points, f.face.as_str()), (9, "MS Shell Dlg"));
        assert_eq!(t.items.len(), 2);
        assert_eq!(class_name(&t.items[0].class).as_deref(), Some("BUTTON"));
        assert_eq!(t.items[0].title, Ord::Name("OK".into()));
        assert_eq!((t.items[0].id, t.items[0].x, t.items[0].cx), (1, 140, 50));
        assert_eq!(class_name(&t.items[1].class).as_deref(), Some("EDIT"));
        assert_eq!(t.items[1].title, Ord::Id(77));
        assert_eq!(t.items[1].data, vec![1, 2, 3]);
        assert_eq!(t.items[1].ex_style, 0x200);
    }

    #[test]
    fn parses_an_extended_template() {
        let mut v = Vec::new();
        w16(&mut v, 1);
        w16(&mut v, 0xFFFF);
        w32(&mut v, 0); // help id
        w32(&mut v, 0x10); // ex style
        w32(&mut v, WS_POPUP | DS_SETFONT);
        w16(&mut v, 1);
        for n in [0i16, 0, 100, 50] {
            w16(&mut v, n as u16);
        }
        w16(&mut v, 0);
        w16(&mut v, 0);
        wstr(&mut v, "Ex");
        w16(&mut v, 8);
        w16(&mut v, 700);
        v.extend_from_slice(&[1, 1]);
        wstr(&mut v, "Tahoma");
        align(&mut v);
        w32(&mut v, 0); // help id
        w32(&mut v, 0);
        w32(&mut v, WS_CHILD | WS_VISIBLE);
        for n in [4i16, 5, 40, 10] {
            w16(&mut v, n as u16);
        }
        w32(&mut v, 0x1234); // a 32-bit control id
        w16(&mut v, 0xFFFF);
        w16(&mut v, 0x82);
        wstr(&mut v, "Label");
        w16(&mut v, 0);
        let t = parse_template(&v).unwrap();
        assert_eq!(t.ex_style, 0x10);
        let f = t.font.unwrap();
        assert_eq!((f.weight, f.italic, f.face.as_str()), (700, true, "Tahoma"));
        assert_eq!(t.items[0].id, 0x1234);
        assert_eq!(class_name(&t.items[0].class).as_deref(), Some("STATIC"));
        assert_eq!(t.items[0].title, Ord::Name("Label".into()));
    }

    #[test]
    fn a_truncated_template_is_rejected() {
        let full = classic();
        for n in [0, 3, 12, 30, full.len() - 1] {
            assert!(parse_template(&full[..n]).is_none(), "{n}");
        }
    }

    #[test]
    fn dialog_units_convert_like_muldiv() {
        assert_eq!(units_to_px(100, 6, 4), 150);
        assert_eq!(units_to_px(10, 13, 8), 16); // 130 / 8 = 16.25
        assert_eq!(units_to_px(0, 7, 4), 0);
    }
}
