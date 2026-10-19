//! The COMBOBOX window class: an edit box (or, for the drop-down list style, a plain selection
//! field) with a button that opens a list of choices.
//!
//! A combo box is made of other windows, as on Windows: a child EDIT for the text, and a LISTBOX that
//! is a child for the simple style and a popup window under the box for the drop-down styles. The
//! list holds the items; the combo box translates its `CB_*` messages into the list box's `LB_*`.

use super::api;
use super::controls::{font_of, notify_parent, Ctl};
use super::ctl_list::*;
use super::msg::{current_ansi, def_window_proc};
use super::surface::*;
use super::wm::{ClassRef, CreateArgs};
use super::*;

pub const CB_GETEDITSEL: u32 = 0x140;
pub const CB_LIMITTEXT: u32 = 0x141;
pub const CB_SETEDITSEL: u32 = 0x142;
pub const CB_ADDSTRING: u32 = 0x143;
pub const CB_DELETESTRING: u32 = 0x144;
pub const CB_GETCOUNT: u32 = 0x146;
pub const CB_GETCURSEL: u32 = 0x147;
pub const CB_GETLBTEXT: u32 = 0x148;
pub const CB_GETLBTEXTLEN: u32 = 0x149;
pub const CB_INSERTSTRING: u32 = 0x14A;
pub const CB_RESETCONTENT: u32 = 0x14B;
pub const CB_FINDSTRING: u32 = 0x14C;
pub const CB_SELECTSTRING: u32 = 0x14D;
pub const CB_SETCURSEL: u32 = 0x14E;
pub const CB_SHOWDROPDOWN: u32 = 0x14F;
pub const CB_GETITEMDATA: u32 = 0x150;
pub const CB_SETITEMDATA: u32 = 0x151;
pub const CB_GETDROPPEDCONTROLRECT: u32 = 0x152;
pub const CB_SETITEMHEIGHT: u32 = 0x153;
pub const CB_GETITEMHEIGHT: u32 = 0x154;
pub const CB_SETEXTENDEDUI: u32 = 0x155;
pub const CB_GETEXTENDEDUI: u32 = 0x156;
pub const CB_GETDROPPEDSTATE: u32 = 0x157;
pub const CB_FINDSTRINGEXACT: u32 = 0x158;
pub const CB_SETLOCALE: u32 = 0x159;
pub const CB_GETLOCALE: u32 = 0x15A;
pub const CB_GETTOPINDEX: u32 = 0x15B;
pub const CB_SETTOPINDEX: u32 = 0x15C;
pub const CB_GETHORIZONTALEXTENT: u32 = 0x15D;
pub const CB_SETHORIZONTALEXTENT: u32 = 0x15E;
pub const CB_GETDROPPEDWIDTH: u32 = 0x15F;
pub const CB_SETDROPPEDWIDTH: u32 = 0x160;
pub const CB_INITSTORAGE: u32 = 0x161;
pub const CB_SETMINVISIBLE: u32 = 0x1701;
pub const CB_GETMINVISIBLE: u32 = 0x1702;

const CBN_SELCHANGE: u32 = 1;
const CBN_DBLCLK: u32 = 2;
const CBN_SETFOCUS: u32 = 3;
const CBN_KILLFOCUS: u32 = 4;
const CBN_EDITCHANGE: u32 = 5;
const CBN_EDITUPDATE: u32 = 6;
const CBN_DROPDOWN: u32 = 7;
const CBN_CLOSEUP: u32 = 8;
const CBN_SELENDOK: u32 = 9;
const CBN_SELENDCANCEL: u32 = 10;

const CBS_SIMPLE: u32 = 1;
const CBS_DROPDOWNLIST: u32 = 3;
const CBS_AUTOHSCROLL: u32 = 0x40;
const CBS_UPPERCASE: u32 = 0x2000;
const CBS_LOWERCASE: u32 = 0x4000;

const EDIT_ID: usize = 1001;
const LIST_ID: usize = 1000;
const BTN_W: i32 = 17;

#[derive(Clone, Default)]
pub struct ComboState {
    pub edit: usize,
    pub list: usize,
    pub dropped: bool,
    pub drop_h: i32,
    pub drop_w: i32,
    pub closed_h: i32,
    pub hot: bool,
    pub ext_ui: bool,
    pub min_visible: i32,
    pub vscroll: bool,
    /// The selection when the list was opened; hovering moves the list's highlight but not this.
    pub before: i32,
}

fn state(g: &Gui, h: usize) -> Option<&ComboState> {
    match &g.windows.get(&h)?.ctl {
        Ctl::Combo(c) => Some(c),
        _ => None,
    }
}

fn state_mut(g: &mut Gui, h: usize) -> Option<&mut ComboState> {
    match &mut g.windows.get_mut(&h)?.ctl {
        Ctl::Combo(c) => Some(c),
        _ => None,
    }
}

fn kind(g: &Gui, h: usize) -> u32 {
    g.windows.get(&h).map_or(0, |w| w.style & 3)
}

fn selection_height(g: &Gui, h: usize) -> i32 {
    g.sized_font(font_of(g, h)).height().max(8) + 8
}

/// The list box behind the combo box (or 0).
fn list_of(g: &Gui, h: usize) -> usize {
    state(g, h).map_or(0, |s| s.list)
}

unsafe fn lb(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    let list = with(|g| list_of(g, h));
    if list == 0 {
        return LB_ERR;
    }
    api::send(list, msg, wp, lp)
}

unsafe fn set_edit_text(h: usize, text: &str) {
    let edit = with(|g| state(g, h).map_or(0, |s| s.edit));
    if edit != 0 {
        let w = crate::runtime::strings::to_wide_nul(text);
        api::send_ansi(edit, WM_SETTEXT, 0, w.as_ptr() as isize, false);
    }
}

/// The text a closed combo box shows: the edit's, or the selected item's.
unsafe fn current_text(h: usize) -> String {
    let (edit, k) = with(|g| (state(g, h).map_or(0, |s| s.edit), kind(g, h)));
    if edit != 0 && k != CBS_DROPDOWNLIST {
        return with(|g| g.windows.get(&edit).map(|w| w.text.clone())).unwrap_or_default();
    }
    with(|g| {
        let list = list_of(g, h);
        g.windows.get(&list).and_then(|w| match &w.ctl {
            Ctl::List(l) if l.cur >= 0 => l.items.get(l.cur as usize).map(|i| i.text.clone()),
            _ => None,
        })
    })
    .unwrap_or_default()
}

/// Positions the edit and (for the simple style) the list inside the combo box.
fn layout(g: &mut Gui, h: usize) {
    let Some(w) = g.windows.get(&h) else { return };
    let (cw, ch) = w.client_size();
    let Some(st) = state(g, h).cloned() else { return };
    let k = kind(g, h);
    if st.edit != 0 {
        let right = if k == CBS_SIMPLE { cw - 2 } else { cw - BTN_W - 1 };
        g.set_rect(st.edit, Rect::new(2, 2, right.max(3), (st.closed_h - 2).max(3)));
    }
    if k == CBS_SIMPLE && st.list != 0 {
        g.set_rect(st.list, Rect::new(0, st.closed_h, cw, ch.max(st.closed_h + 1)));
    }
}

unsafe fn create_children(h: usize) {
    let (style, k, cw, ch, closed_h, rect) = with(|g| {
        let w = g.windows.get(&h).unwrap();
        let (cw, ch) = w.client_size();
        (w.style, w.style & 3, cw, ch, selection_height(g, h), w.rect)
    });
    // a drop-down box is only as tall as its selection field; the rest of the height it was given is for the list
    let drop_h = (rect.height() - closed_h).max(0);
    with(|g| {
        if let Some(st) = state_mut(g, h) {
            st.closed_h = closed_h;
            st.drop_h = if k == CBS_SIMPLE { 0 } else { drop_h };
            st.min_visible = 30;
        }
        if k != CBS_SIMPLE {
            g.set_rect(h, Rect::new(rect.l, rect.t, rect.r, rect.t + closed_h));
        }
    });
    let ch = if k == CBS_SIMPLE { ch } else { closed_h };
    let mk = |class: &str, style: u32, ex: u32, parent: usize, id: usize, r: Rect| {
        api::create_window_ex(CreateArgs {
            ex_style: ex,
            class: ClassRef::Name(class.into()),
            title: String::new(),
            style,
            x: r.l,
            y: r.t,
            w: r.width(),
            h: r.height(),
            parent,
            menu: id,
            hinstance: 0,
            param: 0,
            ansi: false,
        })
    };
    let mut edit = 0;
    if k != CBS_DROPDOWNLIST {
        let mut es = WS_CHILD | WS_VISIBLE | 0x80; // ES_AUTOHSCROLL
        if style & CBS_UPPERCASE != 0 {
            es |= 8;
        }
        if style & CBS_LOWERCASE != 0 {
            es |= 0x10;
        }
        let _ = CBS_AUTOHSCROLL;
        let right = if k == CBS_SIMPLE { cw - 2 } else { cw - BTN_W - 1 };
        edit = mk("EDIT", es, 0, h, EDIT_ID, Rect::new(2, 2, right.max(3), (closed_h - 2).max(3)));
    }
    // CBS_SORT, CBS_HASSTRINGS and the owner-draw bits map onto the list box's
    let vscroll = with(|g| state(g, h).is_some_and(|s| s.vscroll));
    let mut ls = LBS_NOTIFY_BIT | WS_BORDER | if vscroll { WS_VSCROLL } else { 0 };
    if style & 0x100 != 0 {
        ls |= 2;
    }
    if style & 0x200 != 0 || style & 0x30 == 0 {
        ls |= 0x40;
    }
    ls |= style & 0x30;
    let list = if k == CBS_SIMPLE {
        mk("LISTBOX", ls | WS_CHILD | WS_VISIBLE, 0, h, LIST_ID, Rect::new(0, closed_h, cw, ch.max(closed_h + 1)))
    } else {
        mk("LISTBOX", ls | WS_POPUP | LBS_COMBOBOX, WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, h, LIST_ID, Rect::new(0, 0, cw.max(20), 100))
    };
    let font = with(|g| g.windows.get(&h).map_or(0, |w| w.font));
    with(|g| {
        if let Some(st) = state_mut(g, h) {
            st.edit = edit;
            st.list = list;
        }
        layout(g, h);
    });
    if font != 0 {
        for c in [edit, list] {
            if c != 0 {
                api::send(c, WM_SETFONT, font, 0);
            }
        }
    }
}

const LBS_NOTIFY_BIT: u32 = 1;

fn dropped(g: &Gui, h: usize) -> bool {
    state(g, h).is_some_and(|s| s.dropped)
}

unsafe fn open_dropdown(h: usize) {
    let ready = with(|g| kind(g, h) != CBS_SIMPLE && state(g, h).is_some_and(|s| !s.dropped && s.list != 0) && g.is_enabled(h));
    if !ready {
        return;
    }
    notify_parent(h, CBN_DROPDOWN);
    let list = with(|g| list_of(g, h));
    let current = current_text(h);
    let before = lb(h, LB_GETCURSEL, 0, 0) as i32;
    with(|g| {
        if let Some(st) = state_mut(g, h) {
            st.before = before;
        }
    });
    with(|g| {
        let Some(st) = state(g, h).cloned() else { return };
        let (cx, cy) = g.client_to_screen(h, 0, 0);
        let (cw, ch) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
        let (ih, count) = match g.windows.get(&list).map(|w| &w.ctl) {
            Some(Ctl::List(l)) => (if l.item_h > 0 { l.item_h } else { g.sized_font(font_of(g, list)).height().max(8) }, l.items.len() as i32),
            _ => (16, 0),
        };
        let fit = (st.drop_h / ih.max(1)).max(st.min_visible.min(8));
        let rows = count.min(fit).max(1);
        let height = rows * ih + 2;
        let width = if st.drop_w > 0 { st.drop_w } else { cw };
        let (sw, sh) = g.screen_size();
        let mut y = cy + ch;
        if y + height > sh && cy - height >= 0 {
            y = cy - height;
        }
        let x = cx.min((sw - width).max(0));
        g.set_rect(list, Rect::new(x, y, x + width, y + height));
        // the list starts on the current choice
        let sel = match g.windows.get(&list).map(|w| &w.ctl) {
            Some(Ctl::List(l)) => l.items.iter().position(|i| i.text == current),
            _ => None,
        };
        if let Some(Ctl::List(l)) = g.windows.get_mut(&list).map(|w| &mut w.ctl) {
            l.cur = sel.map_or(-1, |i| i as i32);
            for it in &mut l.items {
                it.sel = false;
            }
            if let Some(i) = sel {
                l.items[i].sel = true;
                l.top = i.saturating_sub(rows as usize / 2);
            } else {
                l.top = 0;
            }
        }
    });
    api::show_window(list, SW_SHOWNOACTIVATE);
    with(|g| {
        g.bring_to_top(list);
        if let Some(st) = state_mut(g, h) {
            st.dropped = true;
        }
        if let (Some(b), Some(w)) = (g.backend.clone(), g.windows.get(&list)) {
            b.grab_pointer(w.native, true);
        }
        g.invalidate(h, None, false);
        g.invalidate(list, None, true);
    });
}

unsafe fn close_dropdown(h: usize, commit: Option<usize>) {
    let list = with(|g| if dropped(g, h) { list_of(g, h) } else { 0 });
    if list == 0 {
        return;
    }
    with(|g| {
        if let Some(st) = state_mut(g, h) {
            st.dropped = false;
        }
        if let (Some(b), Some(w)) = (g.backend.clone(), g.windows.get(&list)) {
            b.grab_pointer(w.native, false);
        }
        if g.capture == list {
            g.capture = 0;
        }
    });
    api::show_window(list, SW_HIDE);
    match commit {
        Some(i) => {
            let before = with(|g| state(g, h).map_or(-1, |s| s.before));
            select_index(h, i as i32);
            if before != i as i32 {
                notify_parent(h, CBN_SELCHANGE);
            }
            notify_parent(h, CBN_SELENDOK);
        }
        None => notify_parent(h, CBN_SELENDCANCEL),
    }
    notify_parent(h, CBN_CLOSEUP);
    with(|g| g.invalidate(h, None, false));
}

/// Selects item `i` of the list and shows its text.
unsafe fn select_index(h: usize, i: i32) {
    lb(h, LB_SETCURSEL, i as usize, 0);
    if i >= 0 {
        let text = with(|g| {
            g.windows.get(&list_of(g, h)).and_then(|w| match &w.ctl {
                Ctl::List(l) => l.items.get(i as usize).map(|it| it.text.clone()),
                _ => None,
            })
        });
        if let Some(t) = text {
            set_edit_text(h, &t);
            let edit = with(|g| state(g, h).map_or(0, |s| s.edit));
            if edit != 0 {
                api::send(edit, 0xB1, 0, -1); // EM_SETSEL 0, -1
            }
        }
    } else {
        set_edit_text(h, "");
    }
    with(|g| g.invalidate(h, None, false));
}

unsafe fn step_selection(h: usize, delta: i32) {
    let (cur, count) = (lb(h, LB_GETCURSEL, 0, 0) as i32, lb(h, LB_GETCOUNT, 0, 0) as i32);
    if count <= 0 {
        return;
    }
    let target = if cur < 0 { if delta > 0 { 0 } else { count - 1 } } else { (cur + delta).clamp(0, count - 1) };
    if target != cur {
        select_index(h, target);
        notify_parent(h, CBN_SELCHANGE);
    }
}

/// Moves the highlight in an open list.
unsafe fn move_highlight(h: usize, delta: i32) {
    let list = with(|g| list_of(g, h));
    let (cur, count) = (lb(h, LB_GETCURSEL, 0, 0) as i32, lb(h, LB_GETCOUNT, 0, 0) as i32);
    if count <= 0 {
        return;
    }
    let target = (if cur < 0 { if delta > 0 { 0 } else { count - 1 } } else { cur + delta }).clamp(0, count - 1);
    api::send(list, LB_SETCURSEL, target as usize, 0);
}

fn paint_closed(_h: usize, hdc: usize, info: &(i32, i32, u32, bool, bool, bool, bool, String, usize)) {
    let (cw, closed_h, k, enabled, focused, hot, open, text, font) = info.clone();
    with(|g| {
        g.select_object(hdc, font);
        let (face, border) = if !enabled {
            (0xCCCCCC, 0xBFBFBF)
        } else if open || focused {
            (if k == CBS_DROPDOWNLIST { 0xE1E1E1 } else { 0xFFFFFF }, 0x0078D7)
        } else if hot {
            (if k == CBS_DROPDOWNLIST { 0xE5F1FB } else { 0xFFFFFF }, 0x7EB4EA)
        } else {
            (if k == CBS_DROPDOWNLIST { 0xE1E1E1 } else { 0xFFFFFF }, 0xADADAD)
        };
        let area = Rect::new(0, 0, cw, closed_h);
        g.draw(hdc, |cv, v| {
            cv.fill_rect(area, face);
            cv.frame_rect(area, border, 1);
            // the button
            let btn = Rect::new(cw - BTN_W, 1, cw - 1, closed_h - 1);
            if k != CBS_DROPDOWNLIST && k != CBS_SIMPLE {
                let bg = if !enabled { 0xCCCCCC } else if open { 0xCCE4F7 } else if hot { 0xE5F1FB } else { 0xE1E1E1 };
                cv.fill_rect(btn, bg);
                cv.fill_rect(Rect::new(btn.l - 1, 1, btn.l, closed_h - 1), border);
            }
            if k != CBS_SIMPLE {
                let (mx, my) = ((btn.l + btn.r) / 2, (btn.t + btn.b) / 2);
                let ink = if enabled { 0x606060 } else { 0x838383 };
                let pen = Pen { style: PenStyle::Solid, width: 1, color: ink };
                cv.line(mx - 4, my - 1, mx, my + 3, &pen);
                cv.line(mx, my + 3, mx + 5, my - 2, &pen);
                cv.line(mx - 4, my - 2, mx, my + 2, &pen);
                cv.line(mx, my + 2, mx + 4, my - 2, &pen);
            }
            if k == CBS_DROPDOWNLIST {
                let ty = (closed_h - v.font.height()) / 2;
                let (fg, back) = if enabled && focused && !open { (0xFFFFFF, Some(sys_color_hl())) } else if enabled { (0, None) } else { (0x838383, None) };
                if let Some(b) = back {
                    cv.fill_rect(Rect::new(3, 3, cw - BTN_W - 1, closed_h - 3), b);
                }
                v.font.draw(cv, 6, ty, &text, fg, None);
            }
        });
    });
}

fn sys_color_hl() -> u32 {
    super::gdi::sys_color(13)
}

unsafe fn paint_combo(h: usize) {
    let mut ps = [0u8; 72];
    let hdc = api::begin_paint(h, ps.as_mut_ptr());
    if hdc == 0 {
        return;
    }
    let text = current_text(h);
    let info = with(|g| {
        let w = g.windows.get(&h)?;
        let (cw, _) = w.client_size();
        let st = state(g, h)?;
        let focus = g.focus;
        let focused = focus == h || (st.edit != 0 && focus == st.edit);
        Some((cw, st.closed_h, w.style & 3, g.is_enabled(h), focused, st.hot, st.dropped, text, font_of(g, h)))
    });
    if let Some(info) = info {
        paint_combo_bg(h, hdc, &info);
        paint_closed(h, hdc, &info);
    }
    api::end_paint(h, ps.as_ptr());
}

/// Clears the parts of the client area the combo box does not draw (below the selection field).
fn paint_combo_bg(h: usize, hdc: usize, info: &(i32, i32, u32, bool, bool, bool, bool, String, usize)) {
    with(|g| {
        let Some(w) = g.windows.get(&h) else { return };
        let (cw, ch) = w.client_size();
        let _ = info;
        g.fill_rect(hdc, Rect::new(0, 0, cw, ch), &Brush::Solid(super::gdi::sys_color(15)));
    });
}

pub unsafe extern "win64" fn combobox_proc(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    // the scroll bar bit is for the drop-down list, not for the box itself
                    let vscroll = w.style & WS_VSCROLL != 0;
                    w.style &= !(WS_VSCROLL | WS_HSCROLL);
                    w.ctl = Ctl::Combo(Box::new(ComboState { vscroll, ..Default::default() }));
                }
            });
            1
        }
        WM_CREATE => {
            create_children(h);
            0
        }
        WM_DESTROY => {
            let list = with(|g| list_of(g, h));
            if with(|g| kind(g, h)) != CBS_SIMPLE && list != 0 && with(|g| g.windows.contains_key(&list)) {
                with(|g| {
                    if let (Some(b), Some(w)) = (g.backend.clone(), g.windows.get(&list)) {
                        b.grab_pointer(w.native, false);
                    }
                });
                api::destroy_window(list);
            }
            0
        }
        WM_SIZE => {
            // a drop-down box keeps only its selection field; extra height is the list's
            let resize = with(|g| {
                let (k, st) = (kind(g, h), state(g, h).cloned()?);
                let r = g.windows.get(&h)?.rect;
                if k != CBS_SIMPLE && st.closed_h > 0 && r.height() != st.closed_h {
                    let extra = r.height() - st.closed_h;
                    if let Some(s) = state_mut(g, h) {
                        if extra > 0 {
                            s.drop_h = extra;
                        }
                    }
                    g.set_rect(h, Rect::new(r.l, r.t, r.r, r.t + st.closed_h));
                }
                layout(g, h);
                Some(())
            });
            let _ = resize;
            0
        }
        WM_PAINT => {
            paint_combo(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETFONT => {
            let (edit, list) = with(|g| {
                if let Some(w) = g.windows.get_mut(&h) {
                    w.font = wp;
                }
                (state(g, h).map_or(0, |s| s.edit), list_of(g, h))
            });
            for c in [edit, list] {
                if c != 0 {
                    api::send(c, WM_SETFONT, wp, lp);
                }
            }
            with(|g| {
                let ch = selection_height(g, h);
                let k = kind(g, h);
                if let Some(st) = state_mut(g, h) {
                    st.closed_h = ch;
                }
                if k != CBS_SIMPLE {
                    if let Some(r) = g.windows.get(&h).map(|w| w.rect) {
                        g.set_rect(h, Rect::new(r.l, r.t, r.r, r.t + ch));
                    }
                }
                layout(g, h);
                g.invalidate(h, None, true);
            });
            0
        }
        WM_GETFONT => with(|g| g.windows.get(&h).map_or(0, |w| w.font)) as isize,
        WM_ENABLE => {
            let (edit, list) = with(|g| (state(g, h).map_or(0, |s| s.edit), list_of(g, h)));
            for c in [edit, list] {
                if c != 0 {
                    with(|g| {
                        if let Some(w) = g.windows.get_mut(&c) {
                            if wp != 0 {
                                w.style &= !WS_DISABLED;
                            } else {
                                w.style |= WS_DISABLED;
                            }
                        }
                    });
                }
            }
            with(|g| g.invalidate(h, None, true));
            0
        }
        WM_SETFOCUS => {
            let edit = with(|g| state(g, h).map_or(0, |s| s.edit));
            if edit != 0 {
                api::set_focus(edit);
                api::send(edit, 0xB1, 0, -1);
            }
            with(|g| g.invalidate(h, None, false));
            notify_parent(h, CBN_SETFOCUS);
            0
        }
        WM_KILLFOCUS => {
            close_dropdown(h, None);
            with(|g| g.invalidate(h, None, false));
            notify_parent(h, CBN_KILLFOCUS);
            0
        }
        WM_GETDLGCODE => 0x0081, // DLGC_WANTARROWS | DLGC_WANTCHARS
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC => {
            let parent = with(|g| g.windows.get(&h).map_or(0, |w| w.parent));
            if parent != 0 {
                api::send(parent, msg, wp, lp)
            } else {
                def_window_proc(h, msg, wp, lp, current_ansi())
            }
        }
        WM_SETTEXT => {
            let k = with(|g| kind(g, h));
            if k == CBS_DROPDOWNLIST {
                return 0;
            }
            let edit = with(|g| state(g, h).map_or(0, |s| s.edit));
            api::send(edit, WM_SETTEXT, wp, lp)
        }
        WM_GETTEXT | WM_GETTEXTLENGTH => {
            let (edit, k) = with(|g| (state(g, h).map_or(0, |s| s.edit), kind(g, h)));
            if edit != 0 && k != CBS_DROPDOWNLIST {
                return api::send(edit, msg, wp, lp);
            }
            let text = current_text(h);
            if msg == WM_GETTEXTLENGTH {
                return text.chars().count() as isize;
            }
            if lp == 0 || wp == 0 {
                return 0;
            }
            if current_ansi() {
                crate::runtime::strings::return_a(lp as *mut u8, wp as u32, &text) as isize
            } else {
                crate::runtime::strings::return_w(lp as *mut u16, wp as u32, &text) as isize
            }
        }
        WM_MOUSEMOVE => {
            let hot = with(|g| {
                let st = state(g, h)?;
                let inside = lo_signed(lp) >= 0 && hi_signed(lp) >= 0 && hi_signed(lp) < st.closed_h;
                let changed = st.hot != inside;
                if let Some(s) = state_mut(g, h) {
                    s.hot = inside;
                }
                if inside {
                    g.track_leave(h);
                }
                changed.then_some(())
            });
            if hot.is_some() {
                with(|g| g.invalidate(h, None, false));
            }
            0
        }
        WM_MOUSELEAVE => {
            with(|g| {
                if let Some(s) = state_mut(g, h) {
                    s.hot = false;
                }
                g.invalidate(h, None, false);
            });
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (k, cw, closed_h) = with(|g| (kind(g, h), g.windows.get(&h).map_or(0, |w| w.client_size().0), state(g, h).map_or(0, |s| s.closed_h)));
            let (x, y) = (lo_signed(lp), hi_signed(lp));
            if k == CBS_SIMPLE || y >= closed_h || !with(|g| g.is_enabled(h)) {
                return 0;
            }
            if k != CBS_DROPDOWNLIST && x < cw - BTN_W {
                return 0;
            }
            let edit = with(|g| state(g, h).map_or(0, |s| s.edit));
            api::set_focus(if edit != 0 { edit } else { h });
            if with(|g| dropped(g, h)) {
                close_dropdown(h, None);
            } else {
                open_dropdown(h);
            }
            0
        }
        WM_KEYDOWN => {
            let alt = lp & (1 << 29) != 0;
            let open = with(|g| dropped(g, h));
            match wp as u32 {
                0x73 => {
                    if open {
                        close_dropdown(h, None);
                    } else {
                        open_dropdown(h);
                    }
                }
                0x28 if alt => {
                    if open {
                        close_dropdown(h, None);
                    } else {
                        open_dropdown(h);
                    }
                }
                0x26 | 0x28 | 0x21 | 0x22 | 0x24 | 0x23 if open => {
                    let rows = with(|g| page_rows_of_list(g, h));
                    let delta = match wp as u32 {
                        0x26 => -1,
                        0x28 => 1,
                        0x21 => -rows,
                        0x22 => rows,
                        0x24 => -100_000,
                        _ => 100_000,
                    };
                    move_highlight(h, delta);
                }
                0x0D if open => {
                    let i = lb(h, LB_GETCURSEL, 0, 0);
                    close_dropdown(h, if i >= 0 { Some(i as usize) } else { None });
                }
                0x1B if open => close_dropdown(h, None),
                0x26 | 0x25 => step_selection(h, -1),
                0x28 | 0x27 => step_selection(h, 1),
                0x21 => step_selection(h, -3),
                0x22 => step_selection(h, 3),
                _ => {}
            }
            0
        }
        WM_CHAR => {
            // a drop-down list has no edit to type into: letters jump to the next matching item
            if with(|g| kind(g, h)) == CBS_DROPDOWNLIST {
                let c = char::from_u32(wp as u32).unwrap_or('\0');
                if !c.is_control() {
                    let cur = lb(h, LB_GETCURSEL, 0, 0);
                    let s = c.to_string();
                    let w = crate::runtime::strings::to_wide_nul(&s);
                    let found = api::send_ansi(with(|g| list_of(g, h)), LB_FINDSTRING, cur as usize, w.as_ptr() as isize, false);
                    if found >= 0 && found != cur {
                        select_index(h, found as i32);
                        notify_parent(h, CBN_SELCHANGE);
                    }
                }
            }
            0
        }
        WM_COMMAND => {
            let (id, code, from) = (loword(wp) as usize, hiword(wp) as u32, lp as usize);
            let (edit, list) = with(|g| (state(g, h).map_or(0, |s| s.edit), list_of(g, h)));
            if from != 0 && from == edit {
                match code {
                    0x300 => notify_parent(h, CBN_EDITCHANGE),
                    0x400 => notify_parent(h, CBN_EDITUPDATE),
                    0x100 => notify_parent(h, CBN_SETFOCUS),
                    0x200 => {
                        close_dropdown(h, None);
                        notify_parent(h, CBN_KILLFOCUS);
                    }
                    _ => {}
                }
            } else if from != 0 && from == list && id == LIST_ID {
                match code {
                    LBN_SELCHANGE => {
                        let i = lb(h, LB_GETCURSEL, 0, 0);
                        if i >= 0 {
                            select_index(h, i as i32);
                        }
                        notify_parent(h, CBN_SELCHANGE);
                    }
                    LBN_DBLCLK => notify_parent(h, CBN_DBLCLK),
                    _ => {}
                }
            }
            0
        }
        LBC_COMMIT => {
            close_dropdown(h, Some(lp as usize));
            0
        }
        LBC_CANCEL => {
            close_dropdown(h, None);
            0
        }
        // ---- CB_* ---------------------------------------------------------------------------------
        CB_ADDSTRING => lb(h, LB_ADDSTRING, wp, lp),
        CB_INSERTSTRING => lb(h, LB_INSERTSTRING, wp, lp),
        CB_DELETESTRING => lb(h, LB_DELETESTRING, wp, lp),
        CB_RESETCONTENT => {
            let r = lb(h, LB_RESETCONTENT, 0, 0);
            set_edit_text(h, "");
            with(|g| g.invalidate(h, None, true));
            r
        }
        CB_GETCOUNT => lb(h, LB_GETCOUNT, 0, 0),
        CB_GETCURSEL => lb(h, LB_GETCURSEL, 0, 0),
        CB_SETCURSEL => {
            select_index(h, wp as i32);
            if wp as i32 >= 0 { lb(h, LB_GETCURSEL, 0, 0) } else { LB_ERR }
        }
        CB_GETLBTEXT => lb(h, LB_GETTEXT, wp, lp),
        CB_GETLBTEXTLEN => lb(h, LB_GETTEXTLEN, wp, 0),
        CB_GETITEMDATA => lb(h, LB_GETITEMDATA, wp, 0),
        CB_SETITEMDATA => lb(h, LB_SETITEMDATA, wp, lp),
        CB_FINDSTRING => lb(h, LB_FINDSTRING, wp, lp),
        CB_FINDSTRINGEXACT => lb(h, LB_FINDSTRINGEXACT, wp, lp),
        CB_SELECTSTRING => {
            let r = lb(h, LB_FINDSTRING, wp, lp);
            if r >= 0 {
                select_index(h, r as i32);
            }
            r
        }
        CB_SHOWDROPDOWN => {
            if wp != 0 {
                open_dropdown(h);
            } else {
                close_dropdown(h, None);
            }
            1
        }
        CB_GETDROPPEDSTATE => with(|g| dropped(g, h)) as isize,
        CB_GETDROPPEDCONTROLRECT => {
            if lp != 0 {
                with(|g| {
                    let (x, y) = g.client_to_screen(h, 0, 0);
                    let (cw, _) = g.windows.get(&h).map_or((0, 0), |w| w.client_size());
                    let st = state(g, h).cloned().unwrap_or_default();
                    let r = lp as *mut i32;
                    *r = x;
                    *r.add(1) = y;
                    *r.add(2) = x + if st.drop_w > 0 { st.drop_w.max(cw) } else { cw };
                    *r.add(3) = y + st.closed_h + st.drop_h;
                });
            }
            1
        }
        CB_SETITEMHEIGHT => {
            if wp as i32 == -1 {
                with(|g| {
                    if let Some(st) = state_mut(g, h) {
                        st.closed_h = lp as i32 + 6;
                    }
                    layout(g, h);
                    g.invalidate(h, None, true);
                });
                0
            } else {
                lb(h, LB_SETITEMHEIGHT, wp, lp)
            }
        }
        CB_GETITEMHEIGHT => {
            if wp as i32 == -1 {
                with(|g| state(g, h).map_or(0, |s| s.closed_h as isize - 6))
            } else {
                lb(h, LB_GETITEMHEIGHT, wp, 0)
            }
        }
        CB_SETEXTENDEDUI => with(|g| {
            if let Some(s) = state_mut(g, h) {
                s.ext_ui = wp != 0;
            }
            0
        }),
        CB_GETEXTENDEDUI => with(|g| state(g, h).is_some_and(|s| s.ext_ui)) as isize,
        CB_SETDROPPEDWIDTH => with(|g| {
            if let Some(s) = state_mut(g, h) {
                s.drop_w = wp as i32;
            }
            wp as isize
        }),
        CB_GETDROPPEDWIDTH => with(|g| state(g, h).map_or(0, |s| s.drop_w as isize)),
        CB_SETMINVISIBLE => with(|g| {
            if let Some(s) = state_mut(g, h) {
                s.min_visible = wp as i32;
            }
            1
        }),
        CB_GETMINVISIBLE => with(|g| state(g, h).map_or(0, |s| s.min_visible as isize)),
        CB_GETTOPINDEX => lb(h, LB_GETTOPINDEX, 0, 0),
        CB_SETTOPINDEX => lb(h, LB_SETTOPINDEX, wp, 0),
        CB_GETHORIZONTALEXTENT => lb(h, LB_GETHORIZONTALEXTENT, 0, 0),
        CB_SETHORIZONTALEXTENT => lb(h, LB_SETHORIZONTALEXTENT, wp, 0),
        CB_INITSTORAGE => wp as isize,
        CB_SETLOCALE | CB_GETLOCALE => 0x409,
        CB_LIMITTEXT => {
            let edit = with(|g| state(g, h).map_or(0, |s| s.edit));
            api::send(edit, 0xC5, wp, 0);
            1
        }
        CB_GETEDITSEL => {
            let edit = with(|g| state(g, h).map_or(0, |s| s.edit));
            api::send(edit, 0xB0, wp, lp)
        }
        CB_SETEDITSEL => {
            let edit = with(|g| state(g, h).map_or(0, |s| s.edit));
            api::send(edit, 0xB1, loword(lp as usize) as usize, hiword(lp as usize) as isize)
        }
        _ => def_window_proc(h, msg, wp, lp, current_ansi()),
    }
}

fn page_rows_of_list(g: &Gui, h: usize) -> i32 {
    let list = list_of(g, h);
    let ch = g.windows.get(&list).map_or(0, |w| w.client_size().1);
    let ih = g.sized_font(font_of(g, list)).height().max(8);
    (ch / ih).max(1)
}
