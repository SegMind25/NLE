//! Message queues and the pump: posting and sending messages, timers, turning display events
//! into messages, `GetMessage` / `PeekMessage`, dispatch, and the default window procedure.

use super::backend::{Cursor, Event, ScriptAction, SCREEN};
use super::surface::Rect;
use super::*;
use crate::runtime::handles::current_tid;
use std::cell::Cell;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub struct Timer {
    pub hwnd: usize,
    pub id: usize,
    pub interval: Duration,
    pub due: Instant,
    pub proc_: usize,
}

pub struct SentMsg {
    pub msg: Msg,
    pub ansi: bool,
    pub result: Arc<(Mutex<Option<isize>>, Condvar)>,
}

pub struct ThreadState {
    pub queue: VecDeque<Msg>,
    pub sent: VecDeque<SentMsg>,
    pub quit: Option<i32>,
    pub timers: Vec<Timer>,
    pub wake_fd: i32,
}

impl Drop for ThreadState {
    fn drop(&mut self) {
        if self.wake_fd >= 0 {
            unsafe { libc::close(self.wake_fd) };
        }
    }
}

thread_local! {
    /// Whether the string arguments of the message being handled are ANSI (set by the `...A`
    /// entry points and by the class a window was registered with).
    static ANSI: Cell<bool> = const { Cell::new(false) };
}

pub fn current_ansi() -> bool {
    ANSI.with(|a| a.get())
}

pub fn with_ansi<R>(ansi: bool, f: impl FnOnce() -> R) -> R {
    let prev = ANSI.with(|a| a.replace(ansi));
    let r = f();
    ANSI.with(|a| a.set(prev));
    r
}

pub type WndProc = unsafe extern "win64" fn(usize, u32, usize, isize) -> isize;

/// Calls a window procedure (guest code or one of ours).
pub unsafe fn call_proc(proc_: usize, hwnd: usize, msg: u32, wp: usize, lp: isize) -> isize {
    if proc_ == 0 {
        return 0;
    }
    if crate::runtime::cfg().trace.msg {
        let class = with(|g| g.windows.get(&hwnd).map(|w| w.class.name.clone())).unwrap_or_default();
        eprintln!("nle[msg]: {hwnd:#x} {class} {msg:#06x} wp={wp:#x} lp={lp:#x} proc={}", crate::shim::seh::describe_address(proc_));
    }
    let f: WndProc = std::mem::transmute(proc_);
    f(hwnd, msg, wp, lp)
}

impl Gui {
    pub fn thread_state(&mut self, tid: u32) -> &mut ThreadState {
        self.threads.entry(tid).or_insert_with(|| ThreadState {
            queue: VecDeque::new(),
            sent: VecDeque::new(),
            quit: None,
            timers: Vec::new(),
            wake_fd: unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) },
        })
    }

    pub fn wake(&mut self, tid: u32) {
        let fd = self.thread_state(tid).wake_fd;
        if fd >= 0 {
            let one = 1u64;
            unsafe { libc::write(fd, &one as *const u64 as *const libc::c_void, 8) };
        }
    }

    pub fn post_to_thread(&mut self, tid: u32, m: Msg) {
        let t = self.thread_state(tid);
        t.queue.push_back(m);
        // a runaway producer must not eat all memory
        while t.queue.len() > 10_000 {
            t.queue.pop_front();
        }
        self.wake(tid);
    }

    pub fn post_message(&mut self, hwnd: usize, message: u32, wparam: usize, lparam: isize) -> bool {
        let tid = if hwnd == 0 {
            current_tid()
        } else {
            match self.windows.get(&hwnd) {
                Some(w) => w.thread,
                None => return false,
            }
        };
        let (x, y) = self.pointer;
        let time = self.tick();
        self.post_to_thread(tid, Msg { hwnd, message, wparam, lparam, time, pt: (x, y) });
        true
    }

    /// The window script input goes to: the newest visible top-level window.
    pub fn main_window(&self) -> usize {
        self.windows.values().filter(|w| w.parent == 0 && w.visible_flag()).map(|w| w.hwnd).max().unwrap_or(0)
    }

    fn resolve_native(&self, win: u64) -> usize {
        if win == 0 {
            self.main_window()
        } else {
            self.native_to_hwnd.get(&win).copied().unwrap_or(0)
        }
    }

    fn mouse_flags(&self) -> usize {
        let mut f = 0;
        if self.buttons & 1 != 0 {
            f |= 0x0001;
        }
        if self.buttons & 2 != 0 {
            f |= 0x0002;
        }
        if self.buttons & 4 != 0 {
            f |= 0x0010;
        }
        if self.keys[0x10] & 0x80 != 0 {
            f |= 0x0004;
        }
        if self.keys[0x11] & 0x80 != 0 {
            f |= 0x0008;
        }
        f
    }

    /// Turns display events into messages. Anything that has to call a window procedure is
    /// returned in `sends`, for the caller to do once the lock is released.
    pub fn process_events(&mut self, events: Vec<Event>) -> Out {
        let mut out = Out::default();
        let now = self.tick();
        for ev in events {
            let ev = self.localize(ev);
            if self.menu_mode.is_some() {
                match &ev {
                    Event::Button { win, x, y, .. } | Event::Motion { win, x, y } => {
                        let top = self.resolve_native(*win);
                        let (bx, by) = self.top_client_screen(top);
                        let (sx, sy) = (bx + x, by + y);
                        self.pointer = (sx, sy);
                        if self.menu_mouse(&ev, sx, sy, top, &mut out) {
                            continue;
                        }
                    }
                    Event::Key { down, vk, ch, .. } => {
                        let (down, vk, ch) = (*down, *vk, *ch);
                        self.set_key(vk, down);
                        if !down || self.menu_key(vk, ch, &mut out) {
                            continue;
                        }
                    }
                    Event::Wheel { .. } => continue,
                    Event::Focus { gained: false, .. } => self.menu_end(None),
                    _ => {}
                }
            }
            match ev {
                Event::Expose { win, rect } => {
                    let h = self.resolve_native(win);
                    if let (Some(b), Some(w)) = (self.backend.clone(), self.windows.get(&h)) {
                        if let Some(s) = w.surface.as_ref() {
                            b.present(w.native, s, rect.intersect(&s.bounds()));
                        }
                    }
                }
                Event::Configure { win, x, y, w, h } => {
                    let hwnd = self.resolve_native(win);
                    let Some(wnd) = self.windows.get(&hwnd) else { continue };
                    let f = wnd.frame();
                    let native = wnd.native_size();
                    let old = wnd.rect;
                    let (nl, nt) = if x == i32::MIN { (old.l, old.t) } else { (x - f.l, y - f.t) };
                    let new = Rect::new(nl, nt, nl + w + f.l + f.r, nt + h + f.t + f.b);
                    if new == old {
                        continue;
                    }
                    let resized = native != (w, h);
                    let moved = (old.l, old.t) != (new.l, new.t);
                    let from_script = x == i32::MIN;
                    if from_script {
                        self.set_rect(hwnd, new);
                    } else {
                        self.set_rect_quiet(hwnd, new);
                    }
                    let tid = self.windows.get(&hwnd).map_or(0, |w| w.thread);
                    if resized {
                        let (cw, ch) = self.windows.get(&hwnd).map_or((w, h), |w| w.client_size());
                        self.post_to_thread(tid, Msg { hwnd, message: WM_SIZE, wparam: 0, lparam: make_lparam(cw, ch), time: now, pt: self.pointer });
                    }
                    if moved {
                        let (mx, my) = self.client_to_screen(hwnd, 0, 0);
                        self.post_to_thread(tid, Msg { hwnd, message: WM_MOVE, wparam: 0, lparam: make_lparam(mx, my), time: now, pt: self.pointer });
                    }
                }
                Event::Close { win } => {
                    let h = self.resolve_native(win);
                    if h != 0 {
                        self.post_message(h, WM_CLOSE, 0, 0);
                    }
                }
                Event::Key { win, down, vk, scan, ch, ext, repeat } => {
                    let top = self.resolve_native(win);
                    // the key is for the window the display delivered it to: the focus decides only inside that
                    // window (a dialog that is still opening has not taken the focus from its owner yet, and keys
                    // sent to the dialog must not end up in the owner); pop-ups never take the focus, so keys that
                    // reach one belong to the focus window
                    let focus_here = self.focus != 0 && self.windows.contains_key(&self.focus) && (top == 0 || self.top_level_of(self.focus) == top || !self.activatable(top));
                    let target = if focus_here { self.focus } else { top };
                    if target == 0 {
                        continue;
                    }
                    let was_down = self.keys[(vk & 0xff) as usize] & 0x80 != 0;
                    self.set_key(vk, down);
                    if matches!(vk, 0x12 | 0xA4 | 0xA5) {
                        if down && !was_down {
                            self.alt_alone = true;
                        }
                    } else if down {
                        self.alt_alone = false;
                    }
                    let alt = self.keys[0x12] & 0x80 != 0 && self.keys[0x11] & 0x80 == 0;
                    let sys = alt || vk == 0x12 || vk == 0x79;
                    let message = match (down, sys) {
                        (true, false) => WM_KEYDOWN,
                        (true, true) => WM_SYSKEYDOWN,
                        (false, false) => WM_KEYUP,
                        (false, true) => WM_SYSKEYUP,
                    };
                    // bits 25 and 26 are reserved in a key message's lParam; they carry Shift and Ctrl as they were
                    // when the key went down, so accelerators see the state of that moment
                    let shift = self.keys[0x10] & 0x80 != 0;
                    let ctrl = self.keys[0x11] & 0x80 != 0;
                    let lparam = 1 | (scan as isize & 0xff) << 16 | (ext as isize) << 24 | (shift as isize) << 25 | (ctrl as isize) << 26 | (alt as isize) << 29 | ((was_down || repeat) as isize) << 30 | (!down as isize) << 31;
                    if down {
                        if let Some(c) = ch {
                            if (c as u32) < 0x10000 {
                                self.key_chars.push_back((vk, c as u16));
                                while self.key_chars.len() > 64 {
                                    self.key_chars.pop_front();
                                }
                            }
                        }
                    }
                    let tid = self.windows.get(&target).map_or(0, |w| w.thread);
                    self.post_to_thread(tid, Msg { hwnd: target, message, wparam: vk as usize, lparam: lparam as i32 as isize, time: now, pt: self.pointer });
                }
                Event::Button { win, button, down, x, y } => {
                    let top = self.resolve_native(win);
                    if top == 0 {
                        continue;
                    }
                    let (h, lx, ly) = self.route_mouse(top, x, y);
                    self.pointer = self.client_to_screen(h, lx, ly);
                    if self.nc_button(h, lx, ly, button, down, &mut out) {
                        if down && self.activatable(top) {
                            out.activate = Some(top);
                        }
                        continue;
                    }
                    let bit = match button {
                        3 => 2,
                        2 => 4,
                        _ => 1,
                    };
                    let (base_down, base_up, dbl) = match button {
                        3 => (WM_RBUTTONDOWN, WM_RBUTTONUP, WM_RBUTTONDBLCLK),
                        2 => (WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MBUTTONDBLCLK),
                        _ => (WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDBLCLK),
                    };
                    let message = if down {
                        let is_dbl = self.windows.get(&h).is_some_and(|w| w.class.style & 0x8 != 0)
                            && self.last_click.is_some_and(|(t, hw, b, p)| hw == h && b == button && now.wrapping_sub(t) <= 500 && (p.0 - x).abs() <= 4 && (p.1 - y).abs() <= 4);
                        self.last_click = if is_dbl { None } else { Some((now, h, button, (x, y))) };
                        self.buttons |= bit;
                        if is_dbl {
                            dbl
                        } else {
                            base_down
                        }
                    } else {
                        self.buttons &= !bit;
                        base_up
                    };
                    if down {
                        let t = self.top_level_of(h);
                        if self.activatable(t) {
                            out.activate = Some(t);
                        }
                    }
                    let tid = self.windows.get(&h).map_or(0, |w| w.thread);
                    let flags = self.mouse_flags();
                    self.post_to_thread(tid, Msg { hwnd: h, message, wparam: flags, lparam: make_lparam(lx, ly), time: now, pt: self.client_to_screen(h, lx, ly) });
                }
                Event::Motion { win, x, y } => {
                    let top = self.resolve_native(win);
                    if top == 0 {
                        continue;
                    }
                    let (h, lx, ly) = self.route_mouse(top, x, y);
                    let scr = self.client_to_screen(h, lx, ly);
                    let moved = self.pointer != scr;
                    self.pointer = scr;
                    if !moved {
                        continue;
                    }
                    if self.hover != h {
                        let old = std::mem::replace(&mut self.hover, h);
                        if self.windows.get_mut(&old).is_some_and(|w| std::mem::replace(&mut w.track_leave, false)) {
                            out.sends.push(Msg { hwnd: old, message: WM_MOUSELEAVE, wparam: 0, lparam: 0, time: now, pt: scr });
                        }
                    }
                    if self.nc_motion(h, lx, ly, &mut out) {
                        continue;
                    }
                    let tid = self.windows.get(&h).map_or(0, |w| w.thread);
                    let flags = self.mouse_flags();
                    out.sends.push(Msg { hwnd: h, message: WM_SETCURSOR, wparam: h, lparam: make_lparam(1, WM_MOUSEMOVE as i32), time: now, pt: scr });
                    self.post_to_thread(tid, Msg { hwnd: h, message: WM_MOUSEMOVE, wparam: flags, lparam: make_lparam(lx, ly), time: now, pt: scr });
                }
                Event::Wheel { win, delta, x, y } => {
                    let top = self.resolve_native(win);
                    let target = if self.focus != 0 { self.focus } else { top };
                    if target == 0 {
                        continue;
                    }
                    let (sx, sy) = self.client_to_screen(top, x, y);
                    let tid = self.windows.get(&target).map_or(0, |w| w.thread);
                    let flags = self.mouse_flags();
                    self.post_to_thread(tid, Msg { hwnd: target, message: WM_MOUSEWHEEL, wparam: (flags & 0xffff) | ((delta as i16 as u16 as usize) << 16), lparam: make_lparam(sx, sy), time: now, pt: (sx, sy) });
                }
                Event::Focus { win, gained } => {
                    let top = self.resolve_native(win);
                    if top == 0 {
                        continue;
                    }
                    if gained {
                        if self.activatable(top) {
                            out.activate = Some(top);
                        }
                    } else if self.active == top {
                        out.deactivate = Some(top);
                    }
                }
                Event::Script(ScriptAction::Snapshot(path)) => {
                    let h = self.main_window();
                    if crate::runtime::cfg().trace.api {
                        for c in self.subtree(h) {
                            if let Some(w) = self.windows.get(&c) {
                                let r = w.rect;
                                eprintln!("nle[gui]: window {c:#x} {:<18} ({},{} {}x{}) {}{} {:?} style={:#x} update={:?} parent={:#x}", w.class.name, r.l, r.t, r.width(), r.height(), if w.visible_flag() { "visible" } else { "hidden" }, if w.parent == 0 { " top" } else { "" }, w.text, w.style, w.update, w.parent);
                            }
                        }
                    }
                    if let Some(s) = self.windows.get(&h).and_then(|w| w.surface.as_ref()) {
                        let mut rgb = Vec::with_capacity((s.w * s.h * 3) as usize);
                        for y in 0..s.h {
                            for x in 0..s.w {
                                let c = s.get(x, y);
                                rgb.extend([(c >> 16) as u8, (c >> 8) as u8, c as u8]);
                            }
                        }
                        let png = super::png::encode_rgb(s.w as u32, s.h as u32, &rgb);
                        if let Err(e) = std::fs::write(&path, png) {
                            eprintln!("nle: snapshot {path}: {e}");
                        }
                    }
                }
                Event::Script(ScriptAction::Exit(code)) => out.exit = Some(code),
                Event::Script(ScriptAction::Print(t)) => println!("{t}"),
            }
        }
        out
    }

    /// Menus and drop-down lists (`WS_EX_NOACTIVATE`) never take the focus away from the window they serve.
    pub fn activatable(&self, top: usize) -> bool {
        self.windows.get(&top).is_none_or(|w| w.ex_style & WS_EX_NOACTIVATE == 0)
    }

    /// The top-level window at a screen point, newest first (so popups win over what they cover).
    pub fn top_at_screen(&self, x: i32, y: i32) -> Option<usize> {
        self.windows
            .values()
            .filter(|w| w.parent == 0 && w.visible_flag() && !w.minimized && w.native != 0)
            .filter(|w| {
                let (sx, sy) = self.top_client_screen(w.hwnd);
                let (nw, nh) = w.native_size();
                x >= sx && y >= sy && x < sx + nw && y < sy + nh
            })
            .map(|w| w.hwnd)
            .max()
    }

    /// Events whose `win` is `SCREEN` carry screen coordinates; this turns them into ordinary window events.
    fn localize(&self, ev: Event) -> Event {
        let fix = |x: i32, y: i32| -> (u64, i32, i32) {
            match self.top_at_screen(x, y).or_else(|| Some(self.main_window()).filter(|h| *h != 0)) {
                Some(top) => {
                    let (sx, sy) = self.top_client_screen(top);
                    (self.windows.get(&top).map_or(0, |w| w.native), x - sx, y - sy)
                }
                None => (0, x, y),
            }
        };
        match ev {
            Event::Button { win: SCREEN, button, down, x, y } => {
                let (win, x, y) = fix(x, y);
                Event::Button { win, button, down, x, y }
            }
            Event::Motion { win: SCREEN, x, y } => {
                let (win, x, y) = fix(x, y);
                Event::Motion { win, x, y }
            }
            Event::Wheel { win: SCREEN, delta, x, y } => {
                let (win, x, y) = fix(x, y);
                Event::Wheel { win, delta, x, y }
            }
            other => other,
        }
    }

    fn set_key(&mut self, vk: u32, down: bool) {
        let i = (vk & 0xff) as usize;
        let was = self.keys[i] & 0x80 != 0;
        self.keys[i] = if down { 0x80 } else { 0 };
        if down && !was {
            self.toggles[i] = !self.toggles[i];
        }
        // the generic modifier keys follow their left/right variants
        for (generic, l, r) in [(0x10usize, 0xA0usize, 0xA1usize), (0x11, 0xA2, 0xA3), (0x12, 0xA4, 0xA5)] {
            if i == generic {
                self.keys[l] = self.keys[generic];
            }
            if i == l || i == r {
                self.keys[generic] = if self.keys[l] & 0x80 != 0 || self.keys[r] & 0x80 != 0 { 0x80 } else { 0 };
            }
        }
    }

    /// Where a mouse event at (x, y) in `top`'s client area goes: the capture window, or whatever is under it.
    fn route_mouse(&self, top: usize, x: i32, y: i32) -> (usize, i32, i32) {
        if self.capture != 0 && self.windows.contains_key(&self.capture) {
            let (ox, oy) = self.client_origin_in_top(self.capture);
            let ctop = self.top_level_of(self.capture);
            if ctop == top {
                return (self.capture, x - ox, y - oy);
            }
        }
        self.hit_test(top, x, y)
    }

    /// `TrackMouseEvent(TME_LEAVE)`: the window gets `WM_MOUSELEAVE` when the pointer moves to another window.
    pub fn track_leave(&mut self, h: usize) {
        if let Some(w) = self.windows.get_mut(&h) {
            w.track_leave = true;
        }
    }

    /// Like `set_rect` but does not tell the display (the display is where the change came from).
    pub fn set_rect_quiet(&mut self, h: usize, r: Rect) {
        let backend = self.backend.take();
        self.set_rect(h, r);
        self.backend = backend;
    }
}

#[derive(Default)]
pub struct Out {
    pub sends: Vec<Msg>,
    pub activate: Option<usize>,
    pub deactivate: Option<usize>,
    pub exit: Option<i32>,
    pub menu_ops: Vec<super::menu::MenuOp>,
}

fn drain_wake(fd: i32) {
    if fd >= 0 {
        let mut buf = 0u64;
        unsafe { libc::read(fd, &mut buf as *mut u64 as *mut libc::c_void, 8) };
    }
}

/// Handles what `process_events` could not do under the lock.
pub unsafe fn finish(out: Out) {
    for m in out.sends {
        super::api::send(m.hwnd, m.message, m.wparam, m.lparam);
    }
    for op in out.menu_ops {
        with(|g| g.menu_apply(op));
    }
    if let Some(t) = out.activate {
        super::api::activate_window(t);
    }
    if let Some(t) = out.deactivate {
        super::api::deactivate_window(t);
    }
    if let Some(code) = out.exit {
        crate::loader::exit_process(code as u32);
    }
}

pub struct Filter {
    pub hwnd: usize,
    pub min: u32,
    pub max: u32,
}

pub enum Next {
    Msg(Msg),
    Quit(i32),
    None,
}

impl Filter {
    fn allows(&self, g: &Gui, m: &Msg) -> bool {
        let in_range = (self.min == 0 && self.max == 0) || (m.message >= self.min && m.message <= self.max);
        let for_window = self.hwnd == 0 || m.hwnd == self.hwnd || (self.hwnd != usize::MAX && g.is_descendant(m.hwnd, self.hwnd)) || (self.hwnd == usize::MAX && m.hwnd == 0);
        in_range && for_window
    }

    fn range_has(&self, msg: u32) -> bool {
        (self.min == 0 && self.max == 0) || (msg >= self.min && msg <= self.max)
    }
}

/// The heart of `GetMessage` / `PeekMessage` / `WaitMessage`.
pub unsafe fn next_message(f: &Filter, remove: bool, block: bool) -> Next {
    let tid = current_tid();
    loop {
        // messages sent by other threads are answered first
        let sent = with(|g| g.thread_state(tid).sent.pop_front());
        if let Some(s) = sent {
            let r = send_local(s.msg.hwnd, s.msg.message, s.msg.wparam, s.msg.lparam, s.ansi);
            let (lock, cv) = &*s.result;
            *lock.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
            cv.notify_all();
            continue;
        }
        // input that has arrived, without waiting for it
        let (backend, wake) = with(|g| (g.backend.clone(), g.thread_state(tid).wake_fd));
        drain_wake(wake);
        if let Some(b) = &backend {
            let evs = b.poll(wake, 0);
            if !evs.is_empty() {
                let out = with(|g| g.process_events(evs));
                finish(out);
                continue;
            }
        }
        let now = Instant::now();
        let found: Option<Next> = with(|g| {
            let idx = {
                let t = g.threads.get(&tid)?;
                t.queue.iter().position(|m| f.allows(g, m))
            };
            if let Some(i) = idx {
                let t = g.threads.get_mut(&tid)?;
                let mut m = if remove { t.queue.remove(i)? } else { t.queue[i] };
                g.retarget_key(&mut m);
                return Some(Next::Msg(m));
            }
            let quit = g.threads.get(&tid).and_then(|t| t.quit);
            if let Some(code) = quit {
                if f.range_has(WM_QUIT) {
                    if remove {
                        g.thread_state(tid).quit = None;
                    }
                    return Some(Next::Quit(code));
                }
            }
            if f.range_has(WM_PAINT) {
                if let Some(h) = g.next_paint_window(tid) {
                    if f.hwnd == 0 || h == f.hwnd || g.is_descendant(h, f.hwnd) {
                        return Some(Next::Msg(Msg { hwnd: h, message: WM_PAINT, wparam: 0, lparam: 0, time: g.tick(), pt: g.pointer }));
                    }
                }
            }
            if f.range_has(WM_TIMER) {
                let tick = g.tick();
                let pointer = g.pointer;
                if let Some(t) = g.threads.get_mut(&tid) {
                    if let Some(tm) = t.timers.iter_mut().find(|tm| tm.due <= now && (f.hwnd == 0 || tm.hwnd == f.hwnd)) {
                        tm.due = now + tm.interval;
                        return Some(Next::Msg(Msg { hwnd: tm.hwnd, message: WM_TIMER, wparam: tm.id, lparam: tm.proc_ as isize, time: tick, pt: pointer }));
                    }
                }
            }
            None
        });
        if let Some(n) = found {
            return n;
        }
        if !block {
            return Next::None;
        }
        // nothing to do: put pixels on screen, then sleep until input, a post, or the next timer
        let (backend, timeout) = with(|g| {
            g.flush();
            let next_due = g.threads.get(&tid).and_then(|t| t.timers.iter().map(|tm| tm.due).min());
            let timeout = next_due.map_or(-1, |d| d.saturating_duration_since(Instant::now()).as_millis().min(i32::MAX as u128) as i32 + 1);
            (g.backend.clone(), timeout)
        });
        let evs = match &backend {
            Some(b) => b.poll(wake, timeout),
            None => {
                let mut pfd = libc::pollfd { fd: wake, events: libc::POLLIN, revents: 0 };
                libc::poll(&mut pfd, 1, timeout);
                Vec::new()
            }
        };
        if !evs.is_empty() {
            let out = with(|g| g.process_events(evs));
            finish(out);
        }
    }
}

/// `SendMessage` to a window of the calling thread, or marshalled to the owner thread.
pub unsafe fn send_message(hwnd: usize, msg: u32, wp: usize, lp: isize, ansi: bool) -> isize {
    let me = current_tid();
    let owner = with(|g| g.windows.get(&hwnd).map(|w| w.thread));
    match owner {
        None => 0,
        Some(t) if t == me => send_local(hwnd, msg, wp, lp, ansi),
        Some(t) => {
            let result = Arc::new((Mutex::new(None), Condvar::new()));
            with(|g| {
                let time = g.tick();
                g.thread_state(t).sent.push_back(SentMsg { msg: Msg { hwnd, message: msg, wparam: wp, lparam: lp, time, pt: (0, 0) }, ansi, result: result.clone() });
                g.wake(t);
            });
            let (lock, cv) = &*result;
            let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
            while g.is_none() {
                let (guard, timeout) = cv.wait_timeout(g, Duration::from_secs(5)).unwrap_or_else(|e| e.into_inner());
                g = guard;
                if timeout.timed_out() && g.is_none() {
                    // the owner is not pumping messages; give up like a hung window would
                    return 0;
                }
            }
            g.unwrap_or(0)
        }
    }
}

unsafe fn send_local(hwnd: usize, msg: u32, wp: usize, lp: isize, ansi: bool) -> isize {
    let Some((proc_, unicode)) = with(|g| g.windows.get(&hwnd).map(|w| (w.wndproc, w.unicode))) else { return 0 };
    // the window's own procedure sees strings in the encoding of its class, unless the caller is
    // an ANSI entry point talking to a Unicode window, in which case the strings are converted
    let _ = unicode;
    if msg == WM_PAINT && wp != 0 {
        return super::api::with_print_dc(hwnd, wp, || with_ansi(ansi, || call_proc(proc_, hwnd, msg, wp, lp)));
    }
    with_ansi(ansi, || call_proc(proc_, hwnd, msg, wp, lp))
}

pub unsafe fn dispatch(m: &Msg) -> isize {
    if m.message == WM_TIMER && m.lparam != 0 {
        type TimerProc = unsafe extern "win64" fn(usize, u32, usize, u32);
        let f: TimerProc = std::mem::transmute(m.lparam as usize);
        f(m.hwnd, WM_TIMER, m.wparam, m.time);
        return 0;
    }
    let Some((proc_, unicode)) = with(|g| g.windows.get(&m.hwnd).map(|w| (w.wndproc, w.unicode))) else { return 0 };
    with_ansi(!unicode, || call_proc(proc_, m.hwnd, m.message, m.wparam, m.lparam))
}

impl Gui {
    /// Queues the character message of a key press. It goes ahead of the keys that came in after the press: on
    /// Windows those are still waiting in the system input queue while the character is a posted message, so "E v e"
    /// followed by Tab is seen as three characters and then the Tab, whatever the program was doing meanwhile.
    pub fn post_char(&mut self, hwnd: usize, message: u32, wparam: usize, lparam: isize) {
        let Some(tid) = self.windows.get(&hwnd).map(|w| w.thread) else { return };
        let (x, y) = self.pointer;
        let time = self.tick();
        let m = Msg { hwnd, message, wparam, lparam, time, pt: (x, y) };
        let t = self.thread_state(tid);
        let at = t.queue.iter().position(|q| matches!(q.message, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR)).unwrap_or(t.queue.len());
        t.queue.insert(at, m);
        self.wake(tid);
    }

    /// Keyboard messages go to the window that has the focus when the program fetches them, not when the key arrived:
    /// a program that is busy while "Tab" and then "Space" come in must see the Space on the control the Tab moved to.
    pub fn retarget_key(&self, m: &mut Msg) {
        let key = matches!(m.message, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR);
        if !key || self.focus == 0 || self.focus == m.hwnd || !self.windows.contains_key(&self.focus) || !self.windows.contains_key(&m.hwnd) {
            return;
        }
        if self.top_level_of(self.focus) == self.top_level_of(m.hwnd) {
            m.hwnd = self.focus;
        }
    }
}

/// `TranslateMessage`: a key press becomes a character message when the key produced one.
pub fn translate(m: &Msg) -> bool {
    if m.message != WM_KEYDOWN && m.message != WM_SYSKEYDOWN {
        return false;
    }
    with(|g| {
        let vk = m.wparam as u32;
        let idx = g.key_chars.iter().position(|(v, _)| *v == vk);
        let Some(i) = idx else { return false };
        let (_, ch) = g.key_chars.remove(i).unwrap();
        let ctrl = g.keys[0x11] & 0x80 != 0;
        let ch = if ctrl && (ch as u8).is_ascii_alphabetic() { (ch as u8 & 0x1f) as u16 } else { ch };
        let message = if m.message == WM_SYSKEYDOWN { WM_SYSCHAR } else { WM_CHAR };
        g.post_char(m.hwnd, message, ch as usize, m.lparam);
        true
    })
}

pub fn set_timer(hwnd: usize, id: usize, ms: u32, proc_: usize) -> usize {
    let tid = current_tid();
    with(|g| {
        let interval = Duration::from_millis(ms.max(10) as u64);
        let id = if hwnd == 0 && id == 0 {
            let used: Vec<usize> = g.thread_state(tid).timers.iter().map(|t| t.id).collect();
            (1..).find(|i| !used.contains(i)).unwrap_or(1)
        } else {
            id
        };
        let t = g.thread_state(tid);
        t.timers.retain(|tm| !(tm.hwnd == hwnd && tm.id == id));
        t.timers.push(Timer { hwnd, id, interval, due: Instant::now() + interval, proc_ });
        g.wake(tid);
        id
    })
}

pub fn kill_timer(hwnd: usize, id: usize) -> bool {
    let tid = current_tid();
    with(|g| {
        let t = g.thread_state(tid);
        let n = t.timers.len();
        t.timers.retain(|tm| !(tm.hwnd == hwnd && tm.id == id));
        t.timers.len() != n
    })
}

impl Gui {
    /// Applies a window class cursor for `WM_SETCURSOR` (the real change happens in the backend).
    pub fn set_cursor_for(&mut self, hwnd: usize, cursor: Cursor) {
        let top = self.top_level_of(hwnd);
        if let (Some(b), Some(w)) = (self.backend.clone(), self.windows.get_mut(&top)) {
            if w.cursor != cursor && w.native != 0 {
                w.cursor = cursor;
                b.set_cursor(w.native, cursor);
            }
        }
    }
}

/// The default window procedure.
pub unsafe fn def_window_proc(hwnd: usize, msg: u32, wp: usize, lp: isize, ansi: bool) -> isize {
    use super::api;
    match msg {
        WM_NCCREATE => 1,
        WM_CLOSE => {
            api::destroy_window(hwnd);
            0
        }
        WM_PAINT => {
            let mut ps = [0u8; 72];
            api::begin_paint(hwnd, ps.as_mut_ptr());
            api::end_paint(hwnd, ps.as_ptr());
            0
        }
        // WM_PRINT: wParam is the context to draw into, lParam says what: the background, the client area
        0x0317 => {
            if lp & 0x8 != 0 {
                send_message(hwnd, WM_ERASEBKGND, wp, 0, ansi);
            }
            if lp & 0x4 != 0 {
                send_message(hwnd, 0x0318, wp, lp, ansi);
            }
            0
        }
        // WM_PRINTCLIENT: paint the client area into the context in wParam
        0x0318 => {
            // a built-in control paints itself from its own procedure (a subclass that double buffers WM_PAINT
            // must not see this one again); any other window gets a WM_PAINT carrying the context
            match with(|g| g.windows.get(&hwnd).filter(|w| w.class.native && w.class.wndproc != 0).map(|w| w.class.wndproc)) {
                Some(native) => {
                    super::api::with_print_dc(hwnd, wp, || call_proc(native, hwnd, WM_PAINT, wp, 0));
                }
                None => {
                    send_message(hwnd, WM_PAINT, wp, 0, ansi);
                }
            }
            0
        }
        WM_ERASEBKGND => with(|g| {
            let Some(w) = g.windows.get(&hwnd) else { return 0 };
            let brush = g.brush_arg(w.class.background);
            if matches!(brush, super::surface::Brush::Null) {
                return 0;
            }
            let (cw, ch) = w.client_size();
            g.fill_rect(wp, Rect::new(0, 0, cw, ch), &brush);
            1
        }),
        WM_SETTEXT => {
            let text = if lp == 0 { String::new() } else if ansi { crate::runtime::strings::a_to_string(lp as *const u8) } else { crate::runtime::strings::w_ptr_to_string(lp as *const u16) };
            api::set_window_text(hwnd, &text);
            1
        }
        WM_GETTEXT => {
            let text = with(|g| g.windows.get(&hwnd).map(|w| w.text.clone())).unwrap_or_default();
            if lp == 0 || wp == 0 {
                return 0;
            }
            if ansi {
                crate::runtime::strings::return_a(lp as *mut u8, wp as u32, &text) as isize
            } else {
                crate::runtime::strings::return_w(lp as *mut u16, wp as u32, &text) as isize
            }
        }
        WM_GETTEXTLENGTH => with(|g| g.windows.get(&hwnd).map(|w| if ansi { w.text.len() } else { w.text.encode_utf16().count() } as isize)).unwrap_or(0),
        WM_SETCURSOR => {
            if lo_signed(lp) == 1 {
                let cursor = with(|g| g.windows.get(&hwnd).map(|w| w.class.cursor)).map_or(Cursor::Arrow, super::api::cursor_kind);
                with(|g| g.set_cursor_for(hwnd, cursor));
            }
            0
        }
        0x0205 => {
            // WM_RBUTTONUP: the default procedure turns it into a context-menu request
            let (x, y) = with(|g| g.client_to_screen(hwnd, lo_signed(lp), hi_signed(lp)));
            api::send(hwnd, 0x007B, hwnd, make_lparam(x, y));
            0
        }
        WM_MOUSEWHEEL => {
            // unhandled wheel input goes to the parent
            let parent = with(|g| g.windows.get(&hwnd).map_or(0, |w| w.parent));
            if parent != 0 {
                api::send(parent, msg, wp, lp)
            } else {
                0
            }
        }
        WM_SYSKEYDOWN => {
            if wp == 0x79 {
                api::send(hwnd, WM_SYSCOMMAND, super::menu::SC_KEYMENU, 0);
            } else if wp == 0x73 && lp & (1 << 29) != 0 {
                api::send(hwnd, WM_SYSCOMMAND, 0xF060, 0);
            }
            0
        }
        WM_SYSKEYUP => {
            if matches!(wp, 0x12 | 0xA4 | 0xA5) && with(|g| std::mem::take(&mut g.alt_alone)) {
                api::send(hwnd, WM_SYSCOMMAND, super::menu::SC_KEYMENU, 0);
            }
            0
        }
        WM_SYSCHAR => {
            if wp != 0x20 {
                api::send(hwnd, WM_SYSCOMMAND, super::menu::SC_KEYMENU, wp as isize);
            }
            0
        }
        WM_TIMER if wp == super::nc::SB_REPEAT_TIMER => {
            let out = with(|g| g.nc_repeat(hwnd));
            finish(out);
            0
        }
        WM_NCHITTEST => 1,
        WM_MOUSEACTIVATE => 1,
        WM_NCACTIVATE => 1,
        WM_SYSCOMMAND => {
            match wp & 0xFFF0 {
                0xF060 => {
                    with(|g| g.post_message(hwnd, WM_CLOSE, 0, 0));
                }
                0xF020 => {
                    api::show_window(hwnd, SW_MINIMIZE);
                }
                0xF030 => {
                    api::show_window(hwnd, SW_SHOWMAXIMIZED);
                }
                0xF120 => {
                    api::show_window(hwnd, SW_RESTORE);
                }
                0xF100 => super::menu::key_start(hwnd, char::from_u32(lp as u32).filter(|_| lp != 0)),
                _ => {}
            }
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORDLG | WM_CTLCOLOREDIT | WM_CTLCOLORMSGBOX => with(|g| {
            let (face, text) = (super::gdi::sys_color(15), super::gdi::sys_color(8));
            if msg == WM_CTLCOLOREDIT {
                if let Some(d) = g.dc_mut(wp) {
                    d.bk_color = 0x00ff_ffff;
                    d.text_color = 0;
                }
                return g.sys_brush(5) as isize;
            }
            if let Some(d) = g.dc_mut(wp) {
                d.bk_color = super::surface::to_colorref(face);
                d.text_color = super::surface::to_colorref(text);
            }
            g.sys_brush(15) as isize
        }),
        _ => 0,
    }
}

impl Gui {
    /// A cached stock-like brush for a system colour index (`GetSysColorBrush`).
    pub fn sys_brush(&mut self, index: i32) -> usize {
        let key = 1000 + index;
        if let Some(h) = self.stock.get(&key) {
            return *h;
        }
        let h = self.add_object(super::gdi::Object::Brush(super::surface::Brush::Solid(super::gdi::sys_color(index))));
        self.stock.insert(key, h);
        h
    }
}
