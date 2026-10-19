//! `MessageBox`: a modal dialog built from the same windows and controls a program would use,
//! with its own message loop until a button is chosen.

use super::api::{self, send};
use super::controls::Ctl;
use super::gdi::sys_color;
use super::msg::{self, def_window_proc, Filter, Next};
use super::surface::*;
use super::wm::{Class, ClassRef, CreateArgs};
use super::*;

const MB_TYPEMASK: u32 = 0xF;
const MB_ICONMASK: u32 = 0xF0;
const MB_DEFMASK: u32 = 0xF00;

pub const IDOK: i32 = 1;
pub const IDCANCEL: i32 = 2;
const IDABORT: i32 = 3;
const IDRETRY: i32 = 4;
const IDIGNORE: i32 = 5;
const IDYES: i32 = 6;
const IDNO: i32 = 7;
const IDTRYAGAIN: i32 = 10;
const IDCONTINUE: i32 = 11;

fn buttons_for(flags: u32) -> Vec<(i32, &'static str)> {
    match flags & MB_TYPEMASK {
        1 => vec![(IDOK, "OK"), (IDCANCEL, "Cancel")],
        2 => vec![(IDABORT, "&Abort"), (IDRETRY, "&Retry"), (IDIGNORE, "&Ignore")],
        3 => vec![(IDYES, "&Yes"), (IDNO, "&No"), (IDCANCEL, "Cancel")],
        4 => vec![(IDYES, "&Yes"), (IDNO, "&No")],
        5 => vec![(IDRETRY, "&Retry"), (IDCANCEL, "Cancel")],
        6 => vec![(IDCANCEL, "Cancel"), (IDTRYAGAIN, "&Try Again"), (IDCONTINUE, "&Continue")],
        _ => vec![(IDOK, "OK")],
    }
}

/// What the close button / Escape means for this set of buttons, if anything.
fn cancel_id(buttons: &[(i32, &str)]) -> Option<i32> {
    if buttons.iter().any(|b| b.0 == IDCANCEL) {
        Some(IDCANCEL)
    } else if buttons.len() == 1 {
        Some(buttons[0].0)
    } else {
        None
    }
}

fn default_index(flags: u32, count: usize) -> usize {
    let i = ((flags & MB_DEFMASK) >> 8) as usize;
    if i < count {
        i
    } else {
        0
    }
}

pub fn register_class(g: &mut Gui) {
    if g.find_class(&ClassRef::Name("#32770".into())).is_none() {
        g.register_class(Class { name: "#32770".into(), atom: 0, style: 0, wndproc: dialog_proc as *const () as usize, cls_extra: 0, wnd_extra: 0, hinstance: 0, icon: 0, cursor: 0, background: 16, menu_name: None, unicode: true, native: true });
    }
}

fn set_result(g: &mut Gui, h: usize, id: i32) {
    if let Some(w) = g.windows.get_mut(&h) {
        w.props.insert("result".into(), id as usize);
    }
}

fn icon_kind(flags: u32) -> u32 {
    match flags & MB_ICONMASK {
        0x10 => 1,
        0x20 => 2,
        0x30 => 3,
        0x40 => 4,
        _ => 0,
    }
}

/// The icon in the message box: a coloured disc or triangle with its symbol.
fn draw_icon(g: &mut Gui, hdc: usize, kind: u32, x: i32, y: i32) {
    g.draw(hdc, |cv, v| {
        let none = Pen { style: PenStyle::Null, width: 1, color: 0 };
        let white = 0xFFFFFF;
        match kind {
            1 => {
                cv.ellipse(Rect::new(x, y, x + 32, y + 32), &none, &Brush::Solid(0xE81123));
                let pen = Pen { style: PenStyle::Solid, width: 3, color: white };
                cv.line(x + 10, y + 10, x + 22, y + 22, &pen);
                cv.line(x + 22, y + 10, x + 10, y + 22, &pen);
            }
            3 => {
                cv.polygon(&[(x + 16, y + 1), (x + 31, y + 29), (x + 1, y + 29)], &Pen { style: PenStyle::Solid, width: 1, color: 0x7A6200 }, &Brush::Solid(0xFFD42A), false);
                cv.fill_rect(Rect::new(x + 15, y + 10, x + 17, y + 20), 0x222222);
                cv.fill_rect(Rect::new(x + 15, y + 22, x + 17, y + 25), 0x222222);
            }
            2 | 4 => {
                cv.ellipse(Rect::new(x, y, x + 32, y + 32), &none, &Brush::Solid(0x0078D7));
                if kind == 4 {
                    cv.fill_rect(Rect::new(x + 15, y + 7, x + 18, y + 10), white);
                    cv.fill_rect(Rect::new(x + 15, y + 13, x + 18, y + 25), white);
                } else {
                    let big = crate::gui::font::Sized::new(v.font.face.as_ref().map_or(crate::gui::font::Family::Sans, |_| crate::gui::font::Family::Sans), true, false, 24.0);
                    let w = big.text_width("?");
                    big.draw(cv, x + (32 - w) / 2, y + 2, "?", white, None);
                }
            }
            _ => {}
        }
    });
}

unsafe fn paint_dialog(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return };
        let (cw, ch) = w.client_size();
        let text = w.props.get("text_id").and_then(|_| g.windows.get(&h)).map(|w| w.text.clone()).unwrap_or_default();
        let _ = text;
        let body = w.props.get("body_h").copied().unwrap_or(ch as usize) as i32;
        let kind = w.props.get("icon").copied().unwrap_or(0) as u32;
        let message = MESSAGES.lock().unwrap_or_else(|e| e.into_inner()).get(&h).cloned().unwrap_or_default();
        let f = g.stock_object(17);
        g.select_object(hdc, f);
        g.fill_rect(hdc, Rect::new(0, 0, cw, body), &Brush::Solid(0xFFFFFF));
        g.fill_rect(hdc, Rect::new(0, body, cw, ch), &Brush::Solid(sys_color(15)));
        let mut tx = 20;
        if kind != 0 {
            draw_icon(g, hdc, kind, 20, 20);
            tx = 20 + 32 + 16;
        }
        if let Some(d) = g.dc_mut(hdc) {
            d.bk_mode = 1;
            d.text_color = 0;
        }
        let mut r = Rect::new(tx, 22, cw - 20, body - 10);
        g.draw_text(hdc, &message, &mut r, 0x10 | 0x800);
    });
    api::end_paint(h, ps.as_ptr());
}

static MESSAGES: std::sync::Mutex<Option<std::collections::HashMap<usize, String>>> = std::sync::Mutex::new(None);

trait MapOpt {
    fn get(&self, k: &usize) -> Option<&String>;
}

impl MapOpt for Option<std::collections::HashMap<usize, String>> {
    fn get(&self, k: &usize) -> Option<&String> {
        self.as_ref().and_then(|m| m.get(k))
    }
}

pub unsafe extern "win64" fn dialog_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_PAINT => {
            paint_dialog(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_COMMAND => {
            let id = loword(wp);
            if id != 0 {
                with(|g| set_result(g, h, id));
            }
            0
        }
        WM_CLOSE => {
            let cancel = with(|g| g.windows.get(&h).and_then(|w| w.props.get("cancel").copied()));
            if let Some(id) = cancel {
                with(|g| set_result(g, h, id as i32));
            }
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            // buttons sit on the grey strip
            with(|g| g.sys_brush(15) as isize)
        }
        _ => def_window_proc(h, msg, wp, lp, false),
    }
}

/// Dialog keyboard handling: Enter, Escape, Tab, arrows and the underlined letters.
unsafe fn dialog_key(dlg: usize, vk: usize, buttons: &[(i32, &str)], default: usize) -> bool {
    let kids: Vec<usize> = with(|g| g.windows.get(&dlg).map(|w| w.children.clone()).unwrap_or_default());
    let focus = with(|g| g.focus);
    let pos = kids.iter().position(|k| *k == focus);
    match vk {
        0x0D => {
            let target = pos.unwrap_or(default);
            if let Some(k) = kids.get(target) {
                send(*k, 0xF5, 0, 0);
            }
            true
        }
        0x1B => {
            if let Some(id) = cancel_id(buttons) {
                with(|g| set_result(g, dlg, id));
            }
            true
        }
        0x09 | 0x25 | 0x27 => {
            let back = vk == 0x25 || with(|g| g.keys[0x10] & 0x80 != 0 && vk == 0x09);
            let n = kids.len();
            let next = match pos {
                Some(p) => if back { (p + n - 1) % n } else { (p + 1) % n },
                None => 0,
            };
            if let Some(k) = kids.get(next) {
                api::set_focus(*k);
            }
            true
        }
        c if (0x41..=0x5A).contains(&c) => {
            for (i, (_, label)) in buttons.iter().enumerate() {
                if let Some(pos) = label.find('&') {
                    if label[pos + 1..].chars().next().is_some_and(|ch| ch.to_ascii_uppercase() as usize == c) {
                        if let Some(k) = kids.get(i) {
                            send(*k, 0xF5, 0, 0);
                        }
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

pub unsafe fn message_box(owner: usize, text: &str, caption: &str, flags: u32) -> i32 {
    let buttons = buttons_for(flags);
    let default = default_index(flags, buttons.len());
    // Without a display (or a script to drive one) a message box prints itself and answers.
    let script = std::env::var("NLE_GUI_SCRIPT").map(|s| !s.trim().is_empty()).unwrap_or(false);
    let headless_auto = std::env::var("NLE_GUI").is_ok_and(|v| v.eq_ignore_ascii_case("headless")) && !script;
    let has_display = !headless_auto && with(|g| g.ensure_backend().is_some());
    if !has_display {
        libc::fflush(std::ptr::null_mut());
        eprintln!("[{caption}] {text}");
        return buttons.get(default).map_or(IDOK, |b| b.0);
    }
    with(register_class);

    // measure the text with the dialog font
    let kind = icon_kind(flags);
    let (text_w, text_h) = with(|g| {
        let dc = g.new_dc(super::gdi::Target::Memory, 0, false);
        let f = g.stock_object(17);
        g.select_object(dc, f);
        let max_w = 330;
        let mut r = Rect::new(0, 0, max_w, 0);
        g.draw_text(dc, text, &mut r, 0x10 | 0x400 | 0x800);
        let mut one = Rect::new(0, 0, 10000, 0);
        g.draw_text(dc, text, &mut one, 0x20 | 0x400 | 0x800);
        g.delete_dc(dc);
        (one.width().min(max_w).max(r.width().min(max_w)), r.height().max(16))
    });
    let tx = if kind != 0 { 20 + 32 + 16 } else { 20 };
    let body_h = (text_h + 22 + 14).max(if kind != 0 { 32 + 40 } else { 0 }).max(56);
    let btn_w = 80;
    let strip_h = 50;
    let buttons_w = buttons.len() as i32 * btn_w + (buttons.len() as i32 - 1) * 8;
    let client_w = (tx + text_w + 22).max(buttons_w + 24).max(180);
    let client_h = body_h + strip_h;
    let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
    let ex = WS_EX_DLGMODALFRAME | WS_EX_TOPMOST;
    let inset = super::wm::insets(style, ex, false, true);
    let (ow, oh) = (client_w + inset.l + inset.r, client_h + inset.t + inset.b);
    let (sw, sh) = with(|g| g.screen_size());
    let owner_rect = with(|g| g.windows.contains_key(&owner).then(|| g.window_rect_screen(owner)));
    let (cx, cy) = match owner_rect {
        Some(r) => ((r.l + r.r) / 2, (r.t + r.b) / 2),
        None => (sw / 2, sh / 2),
    };
    let (x, y) = ((cx - ow / 2).clamp(0, (sw - ow).max(0)), (cy - oh / 2).clamp(0, (sh - oh).max(0)));
    let dlg = api::create_window_ex(CreateArgs { ex_style: ex, class: ClassRef::Name("#32770".into()), title: caption.to_string(), style, x, y, w: ow, h: oh, parent: owner, menu: 0, hinstance: 0, param: 0, ansi: false });
    if dlg == 0 {
        eprintln!("[{caption}] {text}");
        return buttons.get(default).map_or(IDOK, |b| b.0);
    }
    with(|g| {
        let mut m = MESSAGES.lock().unwrap_or_else(|e| e.into_inner());
        m.get_or_insert_with(Default::default).insert(dlg, text.to_string());
        if let Some(w) = g.windows.get_mut(&dlg) {
            w.props.insert("body_h".into(), body_h as usize);
            w.props.insert("icon".into(), kind as usize);
            if let Some(c) = cancel_id(&buttons) {
                w.props.insert("cancel".into(), c as usize);
            }
        }
    });
    // buttons, right-aligned on the grey strip
    let mut bx = client_w - 12 - buttons_w;
    let font = with(|g| g.stock_object(17));
    for (i, (id, label)) in buttons.iter().enumerate() {
        let st = WS_CHILD | WS_VISIBLE | WS_TABSTOP | if i == default { 1 } else { 0 };
        let b = api::create_window_ex(CreateArgs { ex_style: 0, class: ClassRef::Name("BUTTON".into()), title: label.to_string(), style: st, x: bx, y: body_h + 14, w: btn_w, h: 24, parent: dlg, menu: *id as usize, hinstance: 0, param: 0, ansi: false });
        send(b, 0x30, font, 0);
        bx += btn_w + 8;
    }
    // modal: the owner stays disabled until the box is answered
    let owner_was_enabled = with(|g| g.windows.get(&owner).is_some_and(|w| w.style & WS_DISABLED == 0));
    if owner != 0 && owner_was_enabled {
        with(|g| {
            if let Some(w) = g.windows.get_mut(&owner) {
                w.style |= WS_DISABLED;
            }
        });
    }
    api::show_window(dlg, SW_SHOWNORMAL);
    let kids: Vec<usize> = with(|g| g.windows.get(&dlg).map(|w| w.children.clone()).unwrap_or_default());
    if let Some(k) = kids.get(default) {
        api::set_focus(*k);
    }
    let result = loop {
        if let Some(r) = with(|g| g.windows.get(&dlg).and_then(|w| w.props.get("result").copied())) {
            break r as i32;
        }
        match msg::next_message(&Filter { hwnd: 0, min: 0, max: 0 }, true, true) {
            Next::Msg(m) => {
                if m.message == WM_KEYDOWN && with(|g| g.is_descendant(m.hwnd, dlg)) && dialog_key(dlg, m.wparam, &buttons, default) {
                    continue;
                }
                msg::translate(&m);
                msg::dispatch(&m);
            }
            Next::Quit(code) => {
                // a quit message posted while the box was open still has to reach the program
                with(|g| {
                    let tid = crate::runtime::handles::current_tid();
                    g.thread_state(tid).quit = Some(code);
                });
                break cancel_id(&buttons).unwrap_or(IDOK);
            }
            Next::None => {}
        }
        if !with(|g| g.windows.contains_key(&dlg)) {
            break cancel_id(&buttons).unwrap_or(IDOK);
        }
    };
    if owner != 0 && owner_was_enabled {
        with(|g| {
            if let Some(w) = g.windows.get_mut(&owner) {
                w.style &= !WS_DISABLED;
            }
        });
    }
    api::destroy_window(dlg);
    with(|_| {
        if let Some(m) = MESSAGES.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            m.remove(&dlg);
        }
    });
    if owner != 0 {
        api::activate_window(owner);
    }
    let _ = Ctl::None;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_sets_match_the_documented_flags() {
        assert_eq!(buttons_for(0).iter().map(|b| b.0).collect::<Vec<_>>(), vec![IDOK]);
        assert_eq!(buttons_for(4).iter().map(|b| b.0).collect::<Vec<_>>(), vec![IDYES, IDNO]);
        assert_eq!(buttons_for(3).len(), 3);
        assert_eq!(buttons_for(6).iter().map(|b| b.0).collect::<Vec<_>>(), vec![IDCANCEL, IDTRYAGAIN, IDCONTINUE]);
    }

    #[test]
    fn closing_means_cancel_only_when_it_makes_sense() {
        assert_eq!(cancel_id(&buttons_for(1)), Some(IDCANCEL));
        assert_eq!(cancel_id(&buttons_for(0)), Some(IDOK));
        assert_eq!(cancel_id(&buttons_for(4)), None, "Yes/No has no way out but a choice");
    }

    #[test]
    fn default_button_flag_picks_the_index() {
        assert_eq!(default_index(0x000, 3), 0);
        assert_eq!(default_index(0x100, 3), 1);
        assert_eq!(default_index(0x300, 3), 0, "out of range falls back to the first");
    }
}
