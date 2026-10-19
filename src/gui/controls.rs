//! The built-in window classes BUTTON, STATIC and EDIT, drawn and driven entirely here.
//!
//! Their window procedures are ordinary `extern "win64"` functions registered as the classes'
//! procedures, so programs can send them messages, subclass them with `SetWindowLongPtr`, and call
//! `DefWindowProc` / `CallWindowProc` on them exactly as on Windows.

use super::api;
use super::gdi::sys_color;
use super::msg::{current_ansi, def_window_proc};
use super::surface::*;
use super::wm::Class;
use super::*;

// ---- messages and styles ---------------------------------------------------------------------------
const WM_CUT: u32 = 0x300;
const WM_COPY: u32 = 0x301;
const WM_PASTE: u32 = 0x302;
const WM_CLEAR: u32 = 0x303;
const BM_GETCHECK: u32 = 0xF0;
const BM_SETCHECK: u32 = 0xF1;
const BM_GETSTATE: u32 = 0xF2;
const BM_SETSTATE: u32 = 0xF3;
const BM_CLICK: u32 = 0xF5;
const EM_GETSEL: u32 = 0xB0;
const EM_SETSEL: u32 = 0xB1;
const EM_GETLINECOUNT: u32 = 0xBA;
const EM_LINEINDEX: u32 = 0xBB;
const EM_GETMODIFY: u32 = 0xB8;
const EM_SETMODIFY: u32 = 0xB9;
const EM_LINEFROMCHAR: u32 = 0xC9;
const EM_REPLACESEL: u32 = 0xC2;
const EM_LIMITTEXT: u32 = 0xC5;
const EM_SETPASSWORDCHAR: u32 = 0xCC;
const EM_SETREADONLY: u32 = 0xCF;
const EM_GETLIMITTEXT: u32 = 0xD5;
const EN_SETFOCUS: u32 = 0x100;
const EN_KILLFOCUS: u32 = 0x200;
const EN_CHANGE: u32 = 0x300;
const EN_UPDATE: u32 = 0x400;
const BN_CLICKED: u32 = 0;
const STN_CLICKED: u32 = 0;

const BS_TYPEMASK: u32 = 0xF;
const BS_DEFPUSHBUTTON: u32 = 1;
const BS_AUTOCHECKBOX: u32 = 3;
const BS_RADIOBUTTON: u32 = 4;
const BS_AUTO3STATE: u32 = 6;
const BS_GROUPBOX: u32 = 7;
const BS_AUTORADIOBUTTON: u32 = 9;
const BS_OWNERDRAW: u32 = 0xB;
const BS_LEFTTEXT: u32 = 0x20;
const BS_PUSHLIKE: u32 = 0x1000;

const ES_CENTER: u32 = 1;
const ES_RIGHT: u32 = 2;
const ES_MULTILINE: u32 = 4;
const ES_UPPERCASE: u32 = 8;
const ES_LOWERCASE: u32 = 0x10;
const ES_PASSWORD: u32 = 0x20;
const ES_NUMBER: u32 = 0x2000;
const ES_READONLY: u32 = 0x800;

const SS_TYPEMASK: u32 = 0x1F;
const SS_NOTIFY: u32 = 0x100;
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;

#[derive(Default, Clone)]
pub enum Ctl {
    #[default]
    None,
    Button(ButtonState),
    Edit(EditState),
    List(Box<super::ctl_list::ListState>),
    Combo(Box<super::ctl_combo::ComboState>),
    /// A SCROLLBAR control: where in the thumb the mouse took hold, and whether the thumb is being dragged.
    Scroll { grab: i32, dragging: bool },
}

#[derive(Default, Clone)]
pub struct ButtonState {
    pub check: u32,
    pub pressed: bool,
    pub hot: bool,
}

#[derive(Clone)]
pub struct EditState {
    pub caret: usize,
    pub anchor: usize,
    pub scroll_x: i32,
    pub top_line: usize,
    pub limit: usize,
    pub readonly: bool,
    pub password: Option<char>,
    pub modified: bool,
    pub caret_on: bool,
    pub dragging: bool,
}

impl Default for EditState {
    fn default() -> EditState {
        EditState { caret: 0, anchor: 0, scroll_x: 0, top_line: 0, limit: 30000, readonly: false, password: None, modified: false, caret_on: true, dragging: false }
    }
}

// ---- colours -----------------------------------------------------------------------------------------
const FACE: u32 = 0xE1E1E1;
const FACE_HOT: u32 = 0xE5F1FB;
const FACE_DOWN: u32 = 0xCCE4F7;
const FACE_OFF: u32 = 0xCCCCCC;
const EDGE: u32 = 0xADADAD;
const EDGE_HOT: u32 = 0x0078D7;
const EDGE_DOWN: u32 = 0x005499;
const EDGE_OFF: u32 = 0xBFBFBF;
const TEXT_OFF: u32 = 0x838383;

pub fn register_builtin_classes(g: &mut Gui) {
    let classes: [(&str, usize, u32); 6] = [
        ("BUTTON", button_proc as *const () as usize, 0x8),
        ("STATIC", static_proc as *const () as usize, 0),
        ("EDIT", edit_proc as *const () as usize, 0x8),
        ("SCROLLBAR", super::ctl_list::scrollbar_proc as *const () as usize, 0),
        ("LISTBOX", super::ctl_list::listbox_proc as *const () as usize, 0x8),
        ("COMBOBOX", super::ctl_combo::combobox_proc as *const () as usize, 0x8),
    ];
    for (name, proc_, style) in classes {
        g.register_class(Class { name: name.into(), atom: 0, style, wndproc: proc_, cls_extra: 0, wnd_extra: 0, hinstance: 0, icon: 0, cursor: 0, background: 0, menu_name: None, unicode: true, native: true });
    }
}

// ---- helpers ---------------------------------------------------------------------------------------

pub(super) fn default_font(g: &Gui) -> usize {
    g.stock_object(17)
}

/// The control's font handle (`WM_SETFONT`) or the default UI font.
pub(super) fn font_of(g: &Gui, h: usize) -> usize {
    match g.windows.get(&h) {
        Some(w) if w.font != 0 => w.font,
        _ => default_font(g),
    }
}

pub(super) unsafe fn notify_parent(h: usize, code: u32) {
    let info = with(|g| g.windows.get(&h).map(|w| (w.parent, w.id)));
    if let Some((parent, id)) = info {
        if parent != 0 {
            api::send(parent, WM_COMMAND, ((code as usize) << 16) | (id & 0xffff), h as isize);
        }
    }
}

pub(super) fn client_rect(g: &Gui, h: usize) -> Rect {
    let (w, hh) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
    Rect::new(0, 0, w, hh)
}

fn is_inside(g: &Gui, h: usize, lp: isize) -> bool {
    client_rect(g, h).contains(lo_signed(lp), hi_signed(lp))
}

/// Fills the control's background from its parent (`WM_CTLCOLOR*`) and returns the DC prepared with the parent's colours.
unsafe fn erase_with_parent(h: usize, hdc: usize, msg: u32) {
    let parent = with(|g| g.windows.get(&h).map_or(0, |w| w.parent));
    let brush = if parent != 0 { api::send(parent, msg, hdc, h as isize) as usize } else { 0 };
    with(|g| {
        let r = client_rect(g, h);
        let b = if brush != 0 { g.brush_arg(brush) } else { Brush::Solid(sys_color(15)) };
        g.fill_rect(hdc, r, &b);
    });
}

fn text_of(g: &Gui, h: usize) -> String {
    g.windows.get(&h).map(|w| w.text.clone()).unwrap_or_default()
}

fn set_text(g: &mut Gui, h: usize, t: String) {
    if let Some(w) = g.windows.get_mut(&h) {
        w.text = t;
    }
}

pub(super) fn dotted_focus(cv: &mut Canvas, r: Rect) {
    let pen = Pen { style: PenStyle::Dot, width: 1, color: 0 };
    cv.line(r.l, r.t, r.r - 1, r.t, &pen);
    cv.line(r.r - 1, r.t, r.r - 1, r.b - 1, &pen);
    cv.line(r.r - 1, r.b - 1, r.l, r.b - 1, &pen);
    cv.line(r.l, r.b - 1, r.l, r.t, &pen);
}

// ======================================================================================================
// BUTTON
// ======================================================================================================

unsafe fn button_click(h: usize) {
    let info = with(|g| {
        let w = g.windows.get(&h)?;
        let ty = w.style & BS_TYPEMASK;
        Some((ty, w.parent, w.style))
    });
    let Some((ty, parent, _style)) = info else { return };
    match ty {
        BS_AUTOCHECKBOX => with(|g| {
            if let Some(Ctl::Button(b)) = g.windows.get_mut(&h).map(|w| &mut w.ctl) {
                b.check = if b.check == 1 { 0 } else { 1 };
            }
        }),
        BS_AUTO3STATE => with(|g| {
            if let Some(Ctl::Button(b)) = g.windows.get_mut(&h).map(|w| &mut w.ctl) {
                b.check = (b.check + 1) % 3;
            }
        }),
        BS_AUTORADIOBUTTON => with(|g| {
            let siblings: Vec<usize> = g.windows.get(&parent).map(|p| p.children.clone()).unwrap_or_default();
            let pos = siblings.iter().position(|s| *s == h).unwrap_or(0);
            let start = (0..=pos).rev().find(|i| g.windows.get(&siblings[*i]).is_some_and(|w| w.style & WS_GROUP != 0)).unwrap_or(0);
            let end = (pos + 1..siblings.len()).find(|i| g.windows.get(&siblings[*i]).is_some_and(|w| w.style & WS_GROUP != 0)).unwrap_or(siblings.len());
            for s in &siblings[start..end] {
                let radio = g.windows.get(s).is_some_and(|w| w.style & BS_TYPEMASK == BS_AUTORADIOBUTTON);
                if radio {
                    if let Some(Ctl::Button(b)) = g.windows.get_mut(s).map(|w| &mut w.ctl) {
                        b.check = (*s == h) as u32;
                    }
                    g.invalidate(*s, None, true);
                }
            }
        }),
        _ => {}
    }
    with(|g| g.invalidate(h, None, true));
    notify_parent(h, BN_CLICKED);
}

unsafe fn paint_button(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    let info = with(|g| {
        let w = g.windows.get(&h)?;
        let b = match &w.ctl {
            Ctl::Button(b) => b.clone(),
            _ => ButtonState::default(),
        };
        let (cw, ch) = w.client_size();
        Some((w.style, w.text.clone(), b, cw, ch, !g.is_enabled(h), g.focus == h, font_of(g, h), w.parent, w.id))
    });
    let Some((style, text, st, cw, ch, disabled, focused, font, parent, id)) = info else {
        api::end_paint(h, ps.as_ptr());
        return;
    };
    let ty = style & BS_TYPEMASK;
    let area = Rect::new(0, 0, cw, ch);
    let push_like = matches!(ty, 0 | BS_DEFPUSHBUTTON) || style & BS_PUSHLIKE != 0;
    if ty == BS_OWNERDRAW {
        // the owner paints: DRAWITEMSTRUCT { CtlType, CtlID, itemID, itemAction, itemState, hwndItem, hDC, rcItem, itemData }
        let mut di = [0u8; 64];
        *(di.as_mut_ptr() as *mut u32) = 4;
        *(di.as_mut_ptr().add(4) as *mut u32) = id as u32;
        *(di.as_mut_ptr().add(12) as *mut u32) = 1;
        *(di.as_mut_ptr().add(16) as *mut u32) = (st.pressed as u32) | ((disabled as u32) << 2) | ((focused as u32) << 4);
        *(di.as_mut_ptr().add(24) as *mut u64) = h as u64;
        *(di.as_mut_ptr().add(32) as *mut u64) = hdc as u64;
        let r = di.as_mut_ptr().add(40) as *mut i32;
        *r = 0;
        *r.add(1) = 0;
        *r.add(2) = cw;
        *r.add(3) = ch;
        api::send(parent, WM_DRAWITEM, id, di.as_ptr() as isize);
        api::end_paint(h, ps.as_ptr());
        return;
    }
    let show_checked = st.check != 0;
    if push_like {
        let (face, edge) = if disabled {
            (FACE_OFF, EDGE_OFF)
        } else if st.pressed || (show_checked && ty != 0 && ty != BS_DEFPUSHBUTTON) {
            (FACE_DOWN, EDGE_DOWN)
        } else if st.hot {
            (FACE_HOT, EDGE_HOT)
        } else if ty == BS_DEFPUSHBUTTON {
            (FACE, EDGE_HOT)
        } else {
            (FACE, EDGE)
        };
        let defaulted = ty == BS_DEFPUSHBUTTON && !disabled;
        with(|g| {
            let f = font_of(g, h);
            g.select_object(hdc, f);
            g.draw(hdc, |cv, v| {
                cv.fill_rect(area, face);
                cv.frame_rect(area, edge, if defaulted { 2 } else { 1 });
                let _ = v;
            });
            if let Some(d) = g.dc_mut(hdc) {
                d.bk_mode = 1;
                d.text_color = to_colorref(if disabled { TEXT_OFF } else { 0 });
            }
            let mut r = Rect::new(2, 2, cw - 2, ch - 2);
            if st.pressed {
                r = r.offset(1, 1);
            }
            g.draw_text(hdc, &text, &mut r, 1 | 4 | 0x20);
            if focused && !disabled {
                g.draw(hdc, |cv, _| dotted_focus(cv, Rect::new(4, 4, cw - 4, ch - 4)));
            }
        });
        let _ = font;
    } else if ty == BS_GROUPBOX {
        erase_with_parent(h, hdc, WM_CTLCOLORBTN);
        with(|g| {
            let f = font_of(g, h);
            g.select_object(hdc, f);
            let th = g.text_extent(hdc, &text).1;
            g.draw(hdc, |cv, _| cv.frame_rect(Rect::new(0, th / 2, cw, ch), 0xDCDCDC, 1));
            let tw = g.text_extent(hdc, &text).0;
            if !text.is_empty() {
                g.fill_rect(hdc, Rect::new(7, 0, 7 + tw + 6, th), &Brush::Solid(sys_color(15)));
                if let Some(d) = g.dc_mut(hdc) {
                    d.bk_mode = 1;
                    d.text_color = to_colorref(if disabled { TEXT_OFF } else { 0 });
                }
                let mut r = Rect::new(10, 0, cw, th);
                g.draw_text(hdc, &text, &mut r, 0x20);
            }
        });
    } else {
        // check boxes and radio buttons
        erase_with_parent(h, hdc, WM_CTLCOLORSTATIC);
        let radio = matches!(ty, BS_RADIOBUTTON | BS_AUTORADIOBUTTON);
        let left_text = style & BS_LEFTTEXT != 0;
        with(|g| {
            let f = font_of(g, h);
            g.select_object(hdc, f);
            let box_size = 13;
            let box_x = if left_text { cw - box_size } else { 0 };
            let box_y = (ch - box_size) / 2;
            let edge = if disabled {
                EDGE_OFF
            } else if st.hot {
                EDGE_HOT
            } else {
                0x333333
            };
            let fill = if st.pressed && !disabled { FACE_DOWN } else { 0xFFFFFF };
            g.draw(hdc, |cv, _| {
                if radio {
                    let r = Rect::new(box_x, box_y, box_x + box_size, box_y + box_size);
                    cv.ellipse(r, &Pen { style: PenStyle::Solid, width: 1, color: edge }, &Brush::Solid(fill));
                    if st.check == 1 {
                        cv.ellipse(Rect::new(box_x + 4, box_y + 4, box_x + box_size - 4, box_y + box_size - 4), &Pen { style: PenStyle::Null, width: 1, color: 0 }, &Brush::Solid(if disabled { TEXT_OFF } else { 0x111111 }));
                    }
                } else {
                    cv.fill_rect(Rect::new(box_x, box_y, box_x + box_size, box_y + box_size), fill);
                    cv.frame_rect(Rect::new(box_x, box_y, box_x + box_size, box_y + box_size), edge, 1);
                    let ink = if disabled { TEXT_OFF } else { 0x111111 };
                    let pen = Pen { style: PenStyle::Solid, width: 1, color: ink };
                    match st.check {
                        1 => {
                            for d in 0..2 {
                                cv.line(box_x + 3, box_y + 6 + d, box_x + 5, box_y + 8 + d, &pen);
                                cv.line(box_x + 5, box_y + 8 + d, box_x + 10, box_y + 3 + d, &pen);
                            }
                        }
                        2 => cv.fill_rect(Rect::new(box_x + 3, box_y + 3, box_x + box_size - 3, box_y + box_size - 3), if disabled { 0xBFBFBF } else { 0x888888 }),
                        _ => {}
                    }
                }
            });
            if let Some(d) = g.dc_mut(hdc) {
                d.bk_mode = 1;
                d.text_color = to_colorref(if disabled { TEXT_OFF } else { 0 });
            }
            let tx = if left_text { 0 } else { box_size + 6 };
            let mut r = Rect::new(tx, 0, if left_text { cw - box_size - 6 } else { cw }, ch);
            g.draw_text(hdc, &text, &mut r, 4 | 0x20);
            if focused && !disabled {
                let tw = g.text_extent(hdc, &text).0;
                let th = g.text_extent(hdc, &text).1;
                let fr = Rect::new(tx - 2, (ch - th) / 2 - 1, tx + tw + 2, (ch + th) / 2 + 1);
                g.draw(hdc, |cv, _| dotted_focus(cv, fr));
            }
        });
    }
    api::end_paint(h, ps.as_ptr());
}

pub unsafe extern "win64" fn button_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.ctl = Ctl::Button(ButtonState::default());
                }
            });
            1
        }
        WM_PAINT => {
            paint_button(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETFONT => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.font = wp;
                }
                g.invalidate(h, None, true);
            });
            0
        }
        WM_GETFONT => with(|g| g.windows.get(&h).map_or(0, |w| w.font)) as isize,
        WM_SETTEXT => {
            let r = def_window_proc(h, msg, wp, lp, current_ansi());
            with(|g| g.invalidate(h, None, true));
            r
        }
        WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS => {
            with(|g| g.invalidate(h, None, true));
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let enabled = with(|g| g.is_enabled(h));
            if !enabled {
                return 0;
            }
            api::set_focus(h);
            with(|g| {
                g.capture = h;
                if let Some(Ctl::Button(b)) = g.windows.get_mut(&h).map(|w| &mut w.ctl) {
                    b.pressed = true;
                }
                g.invalidate(h, None, true);
            });
            0
        }
        WM_MOUSELEAVE => {
            with(|g| {
                if let Some(Ctl::Button(b)) = g.windows.get_mut(&h).map(|w| &mut w.ctl) {
                    b.hot = false;
                }
                g.invalidate(h, None, true);
            });
            0
        }
        WM_MOUSEMOVE => {
            with(|g| {
                g.track_leave(h);
                let inside = is_inside(g, h, lp);
                let captured = g.capture == h;
                let mut changed = false;
                if let Some(Ctl::Button(b)) = g.windows.get_mut(&h).map(|w| &mut w.ctl) {
                    if captured && b.pressed != inside {
                        b.pressed = inside;
                        changed = true;
                    }
                    if b.hot != inside {
                        b.hot = inside;
                        changed = true;
                    }
                }
                if changed {
                    g.invalidate(h, None, true);
                }
            });
            0
        }
        WM_LBUTTONUP => {
            let (was_pressed, inside) = with(|g| {
                let inside = is_inside(g, h, lp);
                let mut was = false;
                if let Some(Ctl::Button(b)) = g.windows.get_mut(&h).map(|w| &mut w.ctl) {
                    was = b.pressed;
                    b.pressed = false;
                }
                if g.capture == h {
                    g.capture = 0;
                }
                g.invalidate(h, None, true);
                (was, inside)
            });
            if was_pressed && inside {
                button_click(h);
            }
            0
        }
        WM_KEYDOWN if wp == 0x20 => {
            with(|g| {
                if let Some(Ctl::Button(b)) = g.windows.get_mut(&h).map(|w| &mut w.ctl) {
                    b.pressed = true;
                }
                g.invalidate(h, None, true);
            });
            0
        }
        WM_KEYUP if wp == 0x20 => {
            let was = with(|g| {
                let mut was = false;
                if let Some(Ctl::Button(b)) = g.windows.get_mut(&h).map(|w| &mut w.ctl) {
                    was = b.pressed;
                    b.pressed = false;
                }
                g.invalidate(h, None, true);
                was
            });
            if was {
                button_click(h);
            }
            0
        }
        BM_CLICK => {
            button_click(h);
            0
        }
        BM_GETCHECK => with(|g| match g.windows.get(&h).map(|w| &w.ctl) {
            Some(Ctl::Button(b)) => b.check as isize,
            _ => 0,
        }),
        BM_SETCHECK => {
            with(|g| {
                if let Some(Ctl::Button(b)) = g.windows.get_mut(&h).map(|w| &mut w.ctl) {
                    b.check = wp as u32;
                }
                g.invalidate(h, None, true);
            });
            0
        }
        BM_GETSTATE => with(|g| match g.windows.get(&h).map(|w| &w.ctl) {
            Some(Ctl::Button(b)) => (b.check | if b.pressed { 4 } else { 0 } | if g.focus == h { 8 } else { 0 }) as isize,
            _ => 0,
        }),
        BM_SETSTATE => {
            with(|g| {
                if let Some(Ctl::Button(b)) = g.windows.get_mut(&h).map(|w| &mut w.ctl) {
                    b.pressed = wp != 0;
                }
                g.invalidate(h, None, true);
            });
            0
        }
        0x0087 => 0x2000, // WM_GETDLGCODE: DLGC_BUTTON
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

// ======================================================================================================
// STATIC
// ======================================================================================================

unsafe fn paint_static(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    let info = with(|g| g.windows.get(&h).map(|w| (w.style, w.text.clone(), w.client_size(), !g.is_enabled(h))));
    let Some((style, text, (cw, ch), disabled)) = info else {
        api::end_paint(h, ps.as_ptr());
        return;
    };
    let kind = style & SS_TYPEMASK;
    let area = Rect::new(0, 0, cw, ch);
    match kind {
        4..=9 => {
            // coloured rectangles and frames
            let color = match kind {
                4 | 7 => sys_color(21),
                5 | 8 => sys_color(1) + 0xC0C0C0,
                _ => 0xFFFFFF,
            };
            with(|g| {
                g.draw(hdc, |cv, _| {
                    if kind <= 6 {
                        cv.fill_rect(area, color);
                    } else {
                        cv.frame_rect(area, color, 1);
                    }
                });
            });
        }
        0x10 | 0x11 | 0x12 => with(|g| {
            g.draw(hdc, |cv, _| {
                if kind == 0x10 {
                    cv.fill_rect(Rect::new(0, ch / 2, cw, ch / 2 + 1), 0xA0A0A0);
                    cv.fill_rect(Rect::new(0, ch / 2 + 1, cw, ch / 2 + 2), 0xFFFFFF);
                } else if kind == 0x11 {
                    cv.fill_rect(Rect::new(cw / 2, 0, cw / 2 + 1, ch), 0xA0A0A0);
                    cv.fill_rect(Rect::new(cw / 2 + 1, 0, cw / 2 + 2, ch), 0xFFFFFF);
                } else {
                    cv.frame_rect(area, 0xA0A0A0, 1);
                }
            });
        }),
        _ => {
            erase_with_parent(h, hdc, WM_CTLCOLORSTATIC);
            with(|g| {
                let f = font_of(g, h);
                g.select_object(hdc, f);
                if disabled {
                    if let Some(d) = g.dc_mut(hdc) {
                        d.text_color = to_colorref(TEXT_OFF);
                    }
                }
                if let Some(d) = g.dc_mut(hdc) {
                    d.bk_mode = 1;
                }
                let mut flags = match kind {
                    1 => 1,
                    2 => 2,
                    _ => 0,
                };
                if kind == 0xC || kind == 0xB {
                    flags |= 0x20;
                } else {
                    flags |= 0x10;
                }
                if style & SS_NOPREFIX != 0 {
                    flags |= 0x800;
                }
                if style & SS_CENTERIMAGE != 0 {
                    flags |= 0x24;
                }
                let mut r = area;
                g.draw_text(hdc, &text, &mut r, flags);
            });
        }
    }
    api::end_paint(h, ps.as_ptr());
}

pub unsafe extern "win64" fn static_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_PAINT => {
            paint_static(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETFONT => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.font = wp;
                }
                g.invalidate(h, None, true);
            });
            0
        }
        WM_GETFONT => with(|g| g.windows.get(&h).map_or(0, |w| w.font)) as isize,
        WM_SETTEXT => {
            let r = def_window_proc(h, msg, wp, lp, current_ansi());
            with(|g| {
                // the parent shows through, so the area it covers must be repainted too
                let parent = g.windows.get(&h).map_or(0, |w| w.parent);
                let rect = g.windows.get(&h).map(|w| w.rect);
                g.invalidate(h, None, true);
                if parent != 0 {
                    g.invalidate(parent, rect, true);
                }
            });
            r
        }
        WM_ENABLE => {
            with(|g| g.invalidate(h, None, true));
            0
        }
        WM_LBUTTONDOWN => {
            if with(|g| g.windows.get(&h).is_some_and(|w| w.style & SS_NOTIFY != 0)) {
                notify_parent(h, STN_CLICKED);
            }
            0
        }
        WM_NCHITTEST => with(|g| g.windows.get(&h).is_some_and(|w| w.style & SS_NOTIFY != 0)) as isize,
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

// ======================================================================================================
// EDIT
// ======================================================================================================

fn edit_state(g: &Gui, h: usize) -> Option<EditState> {
    match g.windows.get(&h).map(|w| &w.ctl) {
        Some(Ctl::Edit(e)) => Some(e.clone()),
        _ => None,
    }
}

fn set_edit_state(g: &mut Gui, h: usize, e: EditState) {
    if let Some(w) = g.windows.get_mut(&h) {
        w.ctl = Ctl::Edit(e);
    }
}

fn line_starts(chars: &[char]) -> Vec<usize> {
    let mut v = vec![0];
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            v.push(i + 1);
        }
    }
    v
}

fn line_of(chars: &[char], idx: usize) -> usize {
    line_starts(chars).iter().rposition(|s| *s <= idx).unwrap_or(0)
}

fn line_range(chars: &[char], line: usize) -> (usize, usize) {
    let starts = line_starts(chars);
    let s = starts.get(line).copied().unwrap_or(chars.len());
    let e = starts.get(line + 1).map(|n| n - 1).unwrap_or(chars.len());
    (s, e)
}

/// The text with `\n` line breaks (Windows stores `\r\n`; the edit keeps one character per break).
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn displayed(chars: &[char], password: Option<char>) -> String {
    match password {
        Some(p) => chars.iter().map(|c| if *c == '\n' { '\n' } else { p }).collect(),
        None => chars.iter().collect(),
    }
}

/// Metrics of the edit's font: (character x offset within its line, line height).
unsafe fn edit_metrics(h: usize) -> Option<(crate::gui::font::Sized, i32)> {
    with(|g| {
        let f = font_of(g, h);
        match g.objects.get(&f) {
            Some(crate::gui::gdi::Object::Font(lf)) => {
                let s = lf.sized();
                let lh = s.height();
                Some((s, lh))
            }
            _ => None,
        }
    })
}

unsafe fn edit_changed(h: usize) {
    notify_parent(h, EN_UPDATE);
    notify_parent(h, EN_CHANGE);
    with(|g| g.invalidate(h, None, true));
}

/// Replaces the selection (or inserts at the caret) with `s`, honouring limits and styles.
unsafe fn edit_insert(h: usize, s: &str) {
    let changed = with(|g| {
        let (Some(mut st), Some(w)) = (edit_state(g, h), g.windows.get(&h)) else { return false };
        if st.readonly {
            return false;
        }
        let multiline = w.style & ES_MULTILINE != 0;
        let mut chars: Vec<char> = w.text.chars().collect();
        let (a, b) = (st.anchor.min(st.caret), st.anchor.max(st.caret));
        let mut ins: Vec<char> = normalize_newlines(s).chars().collect();
        if !multiline {
            ins.retain(|c| *c != '\n');
        }
        if w.style & ES_NUMBER != 0 {
            ins.retain(|c| c.is_ascii_digit());
        }
        if w.style & ES_UPPERCASE != 0 {
            ins = ins.iter().flat_map(|c| c.to_uppercase()).collect();
        }
        if w.style & ES_LOWERCASE != 0 {
            ins = ins.iter().flat_map(|c| c.to_lowercase()).collect();
        }
        let room = st.limit.saturating_sub(chars.len() - (b - a));
        ins.truncate(room);
        if ins.is_empty() && a == b {
            return false;
        }
        chars.splice(a..b, ins.iter().copied());
        st.caret = a + ins.len();
        st.anchor = st.caret;
        st.modified = true;
        st.caret_on = true;
        let text: String = chars.iter().collect();
        set_text(g, h, text);
        set_edit_state(g, h, st);
        true
    });
    if changed {
        edit_changed(h);
    }
}

unsafe fn edit_delete(h: usize, forward: bool) {
    let changed = with(|g| {
        let (Some(mut st), Some(w)) = (edit_state(g, h), g.windows.get(&h)) else { return false };
        if st.readonly {
            return false;
        }
        let mut chars: Vec<char> = w.text.chars().collect();
        let (mut a, mut b) = (st.anchor.min(st.caret), st.anchor.max(st.caret));
        if a == b {
            if forward {
                if b >= chars.len() {
                    return false;
                }
                b += 1;
            } else {
                if a == 0 {
                    return false;
                }
                a -= 1;
            }
        }
        chars.drain(a..b);
        st.caret = a;
        st.anchor = a;
        st.modified = true;
        st.caret_on = true;
        let text: String = chars.iter().collect();
        set_text(g, h, text);
        set_edit_state(g, h, st);
        true
    });
    if changed {
        edit_changed(h);
    }
}

unsafe fn edit_selected_text(h: usize) -> String {
    with(|g| {
        let (Some(st), Some(w)) = (edit_state(g, h), g.windows.get(&h)) else { return String::new() };
        let chars: Vec<char> = w.text.chars().collect();
        chars[st.anchor.min(st.caret)..st.anchor.max(st.caret).min(chars.len())].iter().collect::<String>().replace('\n', "\r\n")
    })
}

fn move_caret(g: &mut Gui, h: usize, new: usize, extend: bool) {
    if let Some(mut st) = edit_state(g, h) {
        st.caret = new;
        if !extend {
            st.anchor = new;
        }
        st.caret_on = true;
        set_edit_state(g, h, st);
        g.invalidate(h, None, true);
    }
}

unsafe fn edit_key(h: usize, vk: usize) {
    let (shift, ctrl) = with(|g| (g.keys[0x10] & 0x80 != 0, g.keys[0x11] & 0x80 != 0));
    match vk {
        0x25 | 0x27 | 0x24 | 0x23 | 0x26 | 0x28 | 0x21 | 0x22 => with(|g| {
            let (Some(st), Some(w)) = (edit_state(g, h), g.windows.get(&h)) else { return };
            let chars: Vec<char> = w.text.chars().collect();
            let multiline = w.style & ES_MULTILINE != 0;
            let c = st.caret;
            let line = line_of(&chars, c);
            let (ls, le) = line_range(&chars, line);
            let new = match vk {
                0x25 => {
                    if ctrl {
                        word_left(&chars, c)
                    } else if st.anchor != c && !shift {
                        st.anchor.min(c)
                    } else {
                        c.saturating_sub(1)
                    }
                }
                0x27 => {
                    if ctrl {
                        word_right(&chars, c)
                    } else if st.anchor != c && !shift {
                        st.anchor.max(c)
                    } else {
                        (c + 1).min(chars.len())
                    }
                }
                0x24 => if ctrl { 0 } else { ls },
                0x23 => if ctrl { chars.len() } else { le },
                0x26 | 0x28 | 0x21 | 0x22 if multiline => {
                    let step = if vk == 0x21 || vk == 0x22 { 10 } else { 1 };
                    let target = if vk == 0x26 || vk == 0x21 { line.saturating_sub(step) } else { (line + step).min(line_starts(&chars).len() - 1) };
                    let col = c - ls;
                    let (ts, te) = line_range(&chars, target);
                    (ts + col).min(te)
                }
                0x26 | 0x21 => 0,
                _ => chars.len(),
            };
            move_caret(g, h, new, shift);
        }),
        0x2E => {
            if shift && with(|g| edit_state(g, h).is_some_and(|s| s.anchor != s.caret)) {
                edit_cut(h);
            } else {
                edit_delete(h, true);
            }
        }
        0x41 if ctrl => with(|g| {
            let len = g.windows.get(&h).map_or(0, |w| w.text.chars().count());
            if let Some(mut st) = edit_state(g, h) {
                st.anchor = 0;
                st.caret = len;
                set_edit_state(g, h, st);
                g.invalidate(h, None, true);
            }
        }),
        0x43 if ctrl => edit_copy(h),
        0x58 if ctrl => edit_cut(h),
        0x56 if ctrl => edit_paste(h),
        0x2D if ctrl => edit_copy(h),
        0x2D if shift => edit_paste(h),
        _ => {}
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_left(chars: &[char], mut i: usize) -> usize {
    while i > 0 && !is_word(chars[i - 1]) {
        i -= 1;
    }
    while i > 0 && is_word(chars[i - 1]) {
        i -= 1;
    }
    i
}

fn word_right(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && is_word(chars[i]) {
        i += 1;
    }
    while i < chars.len() && !is_word(chars[i]) {
        i += 1;
    }
    i
}

unsafe fn edit_copy(h: usize) {
    let t = edit_selected_text(h);
    if !t.is_empty() {
        with(|g| {
            if let Some(b) = g.ensure_backend() {
                b.set_clipboard(&t);
            }
        });
    }
}

unsafe fn edit_cut(h: usize) {
    edit_copy(h);
    edit_delete(h, true);
}

unsafe fn edit_paste(h: usize) {
    let t = with(|g| g.ensure_backend().map(|b| b.get_clipboard())).unwrap_or_default();
    if !t.is_empty() {
        edit_insert(h, &t);
    }
}

/// Character index under a point (client coordinates).
unsafe fn edit_hit(h: usize, x: i32, y: i32) -> usize {
    let Some((font, lh)) = edit_metrics(h) else { return 0 };
    with(|g| {
        let (Some(st), Some(w)) = (edit_state(g, h), g.windows.get(&h)) else { return 0 };
        let chars: Vec<char> = w.text.chars().collect();
        let multiline = w.style & ES_MULTILINE != 0;
        let disp: Vec<char> = displayed(&chars, st.password).chars().collect();
        let line = if multiline { (st.top_line + (y.max(0) / lh.max(1)) as usize).min(line_starts(&chars).len() - 1) } else { 0 };
        let (ls, le) = line_range(&chars, line);
        let target = x + st.scroll_x - 2;
        let mut acc = 0;
        for i in ls..le {
            let cw = font.advance(disp[i]).round() as i32;
            if target < acc + cw / 2 {
                return i;
            }
            acc += cw;
        }
        le
    })
}

unsafe fn paint_edit(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    let parent = with(|g| g.windows.get(&h).map_or(0, |w| w.parent));
    let disabled = with(|g| !g.is_enabled(h));
    let brush = if parent != 0 { api::send(parent, if disabled || with(|g| edit_state(g, h).is_some_and(|s| s.readonly)) { WM_CTLCOLORSTATIC } else { WM_CTLCOLOREDIT }, hdc, h as isize) as usize } else { 0 };
    with(|g| {
        let Some(st) = edit_state(g, h) else { return };
        let w = g.windows.get(&h).unwrap();
        let (cw, ch) = w.client_size();
        let text = w.text.clone();
        let style = w.style;
        let focused = g.focus == h;
        let multiline = style & ES_MULTILINE != 0;
        let f = font_of(g, h);
        g.select_object(hdc, f);
        let bg = if brush != 0 { g.brush_arg(brush) } else { Brush::Solid(if disabled { sys_color(15) } else { 0xFFFFFF }) };
        g.fill_rect(hdc, Rect::new(0, 0, cw, ch), &bg);
        let Some(view) = g.view(hdc) else { return };
        let lh = view.font.height();
        let chars: Vec<char> = text.chars().collect();
        let disp: Vec<char> = displayed(&chars, st.password).chars().collect();
        let (sel_a, sel_b) = (st.anchor.min(st.caret), st.anchor.max(st.caret));
        let starts = line_starts(&chars);
        let first = if multiline { st.top_line } else { 0 };
        let shown_lines = if multiline { (ch / lh.max(1) + 2) as usize } else { 1 };
        let text_color = if disabled { TEXT_OFF } else { from_colorref(view.text_color.min(0xffffff)) };
        let text_color = if disabled { text_color } else { view.text_color };
        let pad = 2;
        for li in first..(first + shown_lines).min(starts.len()) {
            let (ls, le) = line_range(&chars, li);
            let y = if multiline { (li - first) as i32 * lh } else { (ch - lh) / 2 };
            let mut x = pad - st.scroll_x;
            // alignment of a single line within the client width
            let line_w: i32 = disp[ls..le].iter().map(|c| view.font.advance(*c).round() as i32).sum();
            if style & ES_CENTER != 0 {
                x = (cw - line_w) / 2;
            } else if style & ES_RIGHT != 0 {
                x = cw - pad - line_w;
            }
            for i in ls..le {
                let cwid = view.font.advance(disp[i]).round() as i32;
                let selected = focused && i >= sel_a && i < sel_b || (!focused && false);
                let s: String = disp[i].to_string();
                let (fg, back) = if selected { (0xFFFFFF, Some(sys_color(13))) } else { (text_color, None) };
                g.draw(hdc, |cv, v| {
                    if let Some(b) = back {
                        cv.fill_rect(Rect::new(x, y, x + cwid, y + lh), b);
                    }
                    v.font.draw(cv, x, y, &s, fg, None);
                });
                x += cwid;
            }
            // a selected line break shows as a little extra highlight
            if focused && sel_a <= le && sel_b > le && li + 1 < starts.len() {
                g.fill_rect(hdc, Rect::new(x, y, x + 4, y + lh), &Brush::Solid(sys_color(13)));
            }
        }
        if focused && st.caret_on && !st.readonly && sel_a == sel_b {
            let li = line_of(&chars, st.caret);
            if li >= first {
                let (ls, _) = line_range(&chars, li);
                let before: i32 = disp[ls..st.caret.min(disp.len())].iter().map(|c| view.font.advance(*c).round() as i32).sum();
                let line_w: i32 = {
                    let (_, le) = line_range(&chars, li);
                    disp[ls..le].iter().map(|c| view.font.advance(*c).round() as i32).sum()
                };
                let base = if style & ES_CENTER != 0 { (cw - line_w) / 2 } else if style & ES_RIGHT != 0 { cw - pad - line_w } else { pad - st.scroll_x };
                let cx = base + before;
                let y = if multiline { (li - first) as i32 * lh } else { (ch - lh) / 2 };
                g.fill_rect(hdc, Rect::new(cx, y, cx + 1, y + lh), &Brush::Solid(0));
            }
        }
    });
    api::end_paint(h, ps.as_ptr());
}

/// Keeps the caret in view by adjusting the horizontal (or vertical) scroll.
unsafe fn edit_scroll_to_caret(h: usize) {
    let Some((font, lh)) = edit_metrics(h) else { return };
    with(|g| {
        let (Some(mut st), Some(w)) = (edit_state(g, h), g.windows.get(&h)) else { return };
        let (cw, ch) = w.client_size();
        let chars: Vec<char> = w.text.chars().collect();
        let disp: Vec<char> = displayed(&chars, st.password).chars().collect();
        let multiline = w.style & ES_MULTILINE != 0;
        let li = line_of(&chars, st.caret);
        let (ls, _) = line_range(&chars, li);
        let x: i32 = disp[ls..st.caret.min(disp.len())].iter().map(|c| font.advance(*c).round() as i32).sum();
        if x - st.scroll_x > cw - 6 {
            st.scroll_x = x - (cw - 6);
        }
        if x - st.scroll_x < 0 {
            st.scroll_x = x;
        }
        if multiline {
            let visible = (ch / lh.max(1)).max(1) as usize;
            if li < st.top_line {
                st.top_line = li;
            }
            if li >= st.top_line + visible {
                st.top_line = li + 1 - visible;
            }
        }
        set_edit_state(g, h, st);
    });
}

/// The combo box whose edit this is, or 0.
fn combo_parent(h: usize) -> usize {
    with(|g| {
        let p = g.windows.get(&h).map_or(0, |w| w.parent);
        match g.windows.get(&p) {
            Some(pw) if pw.class.native && pw.class.name == "COMBOBOX" => p,
            _ => 0,
        }
    })
}

pub unsafe extern "win64" fn edit_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    let mut st = EditState::default();
                    st.readonly = w.style & ES_READONLY != 0;
                    if w.style & ES_PASSWORD != 0 {
                        st.password = Some('*');
                    }
                    w.text = normalize_newlines(&w.text);
                    w.ctl = Ctl::Edit(st);
                }
            });
            1
        }
        WM_PAINT => {
            edit_scroll_to_caret(h);
            paint_edit(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETFONT => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.font = wp;
                }
                g.invalidate(h, None, true);
            });
            0
        }
        WM_GETFONT => with(|g| g.windows.get(&h).map_or(0, |w| w.font)) as isize,
        WM_SETTEXT => {
            let ansi = current_ansi();
            let text = if lp == 0 { String::new() } else if ansi { crate::runtime::strings::a_to_string(lp as *const u8) } else { crate::runtime::strings::w_ptr_to_string(lp as *const u16) };
            let text = normalize_newlines(&text);
            with(|g| {
                let len = text.chars().count();
                set_text(g, h, text);
                if let Some(mut st) = edit_state(g, h) {
                    st.caret = len.min(st.caret);
                    st.anchor = st.caret;
                    st.modified = false;
                    set_edit_state(g, h, st);
                }
                g.invalidate(h, None, true);
            });
            notify_parent(h, EN_UPDATE);
            notify_parent(h, EN_CHANGE);
            1
        }
        WM_GETTEXT => {
            let text = with(|g| text_of(g, h)).replace('\n', "\r\n");
            if lp == 0 || wp == 0 {
                return 0;
            }
            if current_ansi() {
                crate::runtime::strings::return_a(lp as *mut u8, wp as u32, &text) as isize
            } else {
                crate::runtime::strings::return_w(lp as *mut u16, wp as u32, &text) as isize
            }
        }
        WM_GETTEXTLENGTH => {
            let text = with(|g| text_of(g, h)).replace('\n', "\r\n");
            if current_ansi() { text.len() as isize } else { text.encode_utf16().count() as isize }
        }
        WM_SETFOCUS => {
            with(|g| {
                if let Some(mut st) = edit_state(g, h) {
                    st.caret_on = true;
                    set_edit_state(g, h, st);
                }
                g.invalidate(h, None, true);
            });
            super::msg::set_timer(h, 0xCA, 530, 0);
            notify_parent(h, EN_SETFOCUS);
            0
        }
        WM_KILLFOCUS => {
            super::msg::kill_timer(h, 0xCA);
            with(|g| g.invalidate(h, None, true));
            notify_parent(h, EN_KILLFOCUS);
            0
        }
        WM_TIMER if wp == 0xCA => {
            with(|g| {
                if let Some(mut st) = edit_state(g, h) {
                    st.caret_on = !st.caret_on;
                    set_edit_state(g, h, st);
                    g.invalidate(h, None, false);
                }
            });
            0
        }
        WM_ENABLE => {
            with(|g| g.invalidate(h, None, true));
            0
        }
        WM_CHAR => {
            let ch = char::from_u32(wp as u32).unwrap_or('\0');
            let multiline = with(|g| g.windows.get(&h).is_some_and(|w| w.style & ES_MULTILINE != 0));
            match ch {
                '\u{8}' => edit_delete(h, false),
                '\r' => {
                    if multiline {
                        edit_insert(h, "\n");
                    }
                }
                '\t' | '\u{1b}' => {}
                c if (c as u32) < 0x20 => {}
                c => {
                    edit_insert(h, &c.to_string());
                }
            }
            edit_scroll_to_caret(h);
            0
        }
        WM_KEYDOWN if matches!(wp, 0x26 | 0x28 | 0x21 | 0x22 | 0x73 | 0x0D | 0x1B) && combo_parent(h) != 0 => {
            // the edit of a combo box hands list navigation to the combo box
            let combo = combo_parent(h);
            let handled = api::send(combo, msg, wp, lp);
            let _ = handled;
            0
        }
        WM_KEYDOWN => {
            edit_key(h, wp);
            edit_scroll_to_caret(h);
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if !with(|g| g.is_enabled(h)) {
                return 0;
            }
            api::set_focus(h);
            let idx = edit_hit(h, lo_signed(lp), hi_signed(lp));
            let shift = with(|g| g.keys[0x10] & 0x80 != 0);
            with(|g| {
                g.capture = h;
                if let Some(mut st) = edit_state(g, h) {
                    st.dragging = true;
                    set_edit_state(g, h, st);
                }
                move_caret(g, h, idx, shift);
                if msg == WM_LBUTTONDBLCLK {
                    // select the word under the pointer
                    if let Some(w) = g.windows.get(&h) {
                        let chars: Vec<char> = w.text.chars().collect();
                        let (mut a, mut b) = (idx.min(chars.len()), idx.min(chars.len()));
                        while a > 0 && is_word(chars[a - 1]) {
                            a -= 1;
                        }
                        while b < chars.len() && is_word(chars[b]) {
                            b += 1;
                        }
                        if let Some(mut st) = edit_state(g, h) {
                            st.anchor = a;
                            st.caret = b;
                            set_edit_state(g, h, st);
                        }
                    }
                }
            });
            0
        }
        WM_MOUSEMOVE => {
            let dragging = with(|g| edit_state(g, h).is_some_and(|s| s.dragging));
            if dragging {
                let idx = edit_hit(h, lo_signed(lp), hi_signed(lp));
                with(|g| move_caret(g, h, idx, true));
                edit_scroll_to_caret(h);
            }
            0
        }
        WM_LBUTTONUP => {
            with(|g| {
                if let Some(mut st) = edit_state(g, h) {
                    st.dragging = false;
                    set_edit_state(g, h, st);
                }
                if g.capture == h {
                    g.capture = 0;
                }
            });
            0
        }
        WM_CUT => {
            edit_cut(h);
            0
        }
        WM_COPY => {
            edit_copy(h);
            0
        }
        WM_PASTE => {
            edit_paste(h);
            0
        }
        WM_CLEAR => {
            edit_delete(h, true);
            0
        }
        EM_GETSEL => with(|g| {
            let Some(st) = edit_state(g, h) else { return 0 };
            let (a, b) = (st.anchor.min(st.caret) as u32, st.anchor.max(st.caret) as u32);
            if wp != 0 {
                *(wp as *mut u32) = a;
            }
            if lp != 0 {
                *(lp as *mut u32) = b;
            }
            ((b << 16) | (a & 0xffff)) as isize
        }),
        EM_SETSEL => {
            with(|g| {
                let len = g.windows.get(&h).map_or(0, |w| w.text.chars().count());
                if let Some(mut st) = edit_state(g, h) {
                    let a = if wp as isize == -1 { len } else { wp.min(len) };
                    let b = if lp == -1 { len } else { (lp as usize).min(len) };
                    st.anchor = a;
                    st.caret = if wp as isize == -1 { a } else { b };
                    set_edit_state(g, h, st);
                    g.invalidate(h, None, true);
                }
            });
            edit_scroll_to_caret(h);
            1
        }
        EM_REPLACESEL => {
            let ansi = current_ansi();
            let text = if lp == 0 { String::new() } else if ansi { crate::runtime::strings::a_to_string(lp as *const u8) } else { crate::runtime::strings::w_ptr_to_string(lp as *const u16) };
            let ro = with(|g| edit_state(g, h).is_some_and(|s| s.readonly));
            if ro {
                with(|g| {
                    if let Some(mut st) = edit_state(g, h) {
                        st.readonly = false;
                        set_edit_state(g, h, st);
                    }
                });
            }
            edit_insert(h, &text);
            if ro {
                with(|g| {
                    if let Some(mut st) = edit_state(g, h) {
                        st.readonly = true;
                        set_edit_state(g, h, st);
                    }
                });
            }
            0
        }
        EM_LIMITTEXT => {
            with(|g| {
                if let Some(mut st) = edit_state(g, h) {
                    st.limit = if wp == 0 { 30000 } else { wp };
                    set_edit_state(g, h, st);
                }
            });
            0
        }
        EM_GETLIMITTEXT => with(|g| edit_state(g, h).map_or(0, |s| s.limit as isize)),
        EM_SETREADONLY => {
            with(|g| {
                if let Some(mut st) = edit_state(g, h) {
                    st.readonly = wp != 0;
                    set_edit_state(g, h, st);
                    g.invalidate(h, None, true);
                }
            });
            1
        }
        EM_SETPASSWORDCHAR => {
            with(|g| {
                if let Some(mut st) = edit_state(g, h) {
                    st.password = if wp == 0 { None } else { char::from_u32(wp as u32) };
                    set_edit_state(g, h, st);
                    g.invalidate(h, None, true);
                }
            });
            0
        }
        EM_GETMODIFY => with(|g| edit_state(g, h).is_some_and(|s| s.modified)) as isize,
        EM_SETMODIFY => {
            with(|g| {
                if let Some(mut st) = edit_state(g, h) {
                    st.modified = wp != 0;
                    set_edit_state(g, h, st);
                }
            });
            0
        }
        EM_GETLINECOUNT => with(|g| g.windows.get(&h).map_or(1, |w| w.text.matches('\n').count() as isize + 1)),
        EM_LINEINDEX => with(|g| {
            let chars: Vec<char> = g.windows.get(&h).map(|w| w.text.chars().collect()).unwrap_or_default();
            let line = if wp as isize == -1 { edit_state(g, h).map_or(0, |s| line_of(&chars, s.caret)) } else { wp };
            line_starts(&chars).get(line).map_or(-1, |v| *v as isize)
        }),
        EM_LINEFROMCHAR => with(|g| {
            let chars: Vec<char> = g.windows.get(&h).map(|w| w.text.chars().collect()).unwrap_or_default();
            let idx = if wp as isize == -1 { edit_state(g, h).map_or(0, |s| s.caret) } else { wp };
            line_of(&chars, idx) as isize
        }),
        0x0087 => 0x0083, // WM_GETDLGCODE: DLGC_WANTCHARS | DLGC_HASSETSEL | DLGC_WANTARROWS
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_helpers_split_on_newlines() {
        let c: Vec<char> = "ab\ncde\n".chars().collect();
        assert_eq!(line_starts(&c), vec![0, 3, 7]);
        assert_eq!(line_of(&c, 4), 1);
        assert_eq!(line_range(&c, 1), (3, 6));
        assert_eq!(line_range(&c, 2), (7, 7));
    }

    #[test]
    fn word_motion() {
        let c: Vec<char> = "foo  bar_baz qux".chars().collect();
        assert_eq!(word_right(&c, 0), 5);
        assert_eq!(word_right(&c, 5), 13);
        assert_eq!(word_left(&c, 13), 5);
        assert_eq!(word_left(&c, 5), 0);
    }

    #[test]
    fn password_display_hides_text_but_keeps_breaks() {
        let c: Vec<char> = "ab\ncd".chars().collect();
        assert_eq!(displayed(&c, Some('*')), "**\n**");
        assert_eq!(displayed(&c, None), "ab\ncd");
    }

    #[test]
    fn newline_normalisation() {
        assert_eq!(normalize_newlines("a\r\nb\rc\n"), "a\nb\nc\n");
    }
}
