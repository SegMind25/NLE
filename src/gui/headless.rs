//! A display that exists only in memory. Windows get ids and sizes, nothing is shown, and input
//! comes from a script (`NLE_GUI_SCRIPT`), which makes GUI programs testable without a screen:
//!
//! ```text
//! NLE_GUI=headless NLE_GUI_SCRIPT="wait 200; click 40 30; key a; snapshot out.png; close" nle app.exe
//! ```
//!
//! Steps are separated by `;` or newlines:
//! `wait MS`, `click X Y [left|right|middle]`, `dblclick X Y`, `move X Y`, `down X Y [button]`,
//! `up X Y [button]`, `key [ctrl+|shift+|alt+]NAME`, `type TEXT`, `resize W H`,
//! `snapshot PATH.png`, `close`, `exit [CODE]`, `print TEXT`.
//! Coordinates are client coordinates of the newest top-level window. The `s` variants (`smove`, `sclick`,
//! `sdblclick`, `sdown`, `sup`) take screen coordinates and act on whatever window is there, which is how a
//! script reaches menus and drop-down lists.

use super::backend::*;
use super::surface::{Rect, Surface};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
struct Step {
    wait_ms: u64,
    events: Vec<Event>,
}

struct HeadWin {
    visible: bool,
}

pub struct Headless {
    screen: (i32, i32),
    next: AtomicU64,
    windows: Mutex<HashMap<u64, HeadWin>>,
    script: Mutex<Script>,
    clipboard: Mutex<String>,
    pointer: Mutex<(i32, i32, u32)>,
}

struct Script {
    steps: VecDeque<Step>,
    due: Option<Instant>,
}

fn vk_for_name(name: &str) -> Option<(u32, Option<char>)> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "enter" | "return" => (0x0D, Some('\r')),
        "esc" | "escape" => (0x1B, Some('\u{1b}')),
        "tab" => (0x09, Some('\t')),
        "space" => (0x20, Some(' ')),
        "backspace" | "bs" => (0x08, Some('\u{8}')),
        "delete" | "del" => (0x2E, None),
        "insert" => (0x2D, None),
        "left" => (0x25, None),
        "up" => (0x26, None),
        "right" => (0x27, None),
        "down" => (0x28, None),
        "home" => (0x24, None),
        "end" => (0x23, None),
        "pageup" | "pgup" => (0x21, None),
        "pagedown" | "pgdn" => (0x22, None),
        s if s.len() >= 2 && s.starts_with('f') && s[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) => (0x70 + s[1..].parse::<u32>().unwrap() - 1, None),
        s if s.chars().count() == 1 => {
            let c = name.chars().next().unwrap();
            match c {
                'a'..='z' | 'A'..='Z' => (c.to_ascii_uppercase() as u32, Some(c)),
                '0'..='9' => (c as u32, Some(c)),
                '-' => (0xBD, Some(c)),
                '=' => (0xBB, Some(c)),
                ',' => (0xBC, Some(c)),
                '.' => (0xBE, Some(c)),
                '/' => (0xBF, Some(c)),
                ';' => (0xBA, Some(c)),
                '\'' => (0xDE, Some(c)),
                '[' => (0xDB, Some(c)),
                ']' => (0xDD, Some(c)),
                '\\' => (0xDC, Some(c)),
                '`' => (0xC0, Some(c)),
                other => (0, Some(other)),
            }
        }
        _ => return None,
    })
}

fn key_events(spec: &str) -> Result<Vec<Event>, String> {
    let mut parts: Vec<&str> = spec.split('+').collect();
    let name = parts.pop().unwrap_or("");
    let name = if name.is_empty() { "+" } else { name };
    let (vk, ch) = vk_for_name(name).ok_or_else(|| format!("unknown key `{name}`"))?;
    let mut mods: Vec<u32> = Vec::new();
    let mut ctrl = false;
    for m in &parts {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => {
                mods.push(0x11);
                ctrl = true;
            }
            "shift" => mods.push(0x10),
            "alt" => mods.push(0x12),
            other => return Err(format!("unknown modifier `{other}`")),
        }
    }
    let mut ev = Vec::new();
    for &m in &mods {
        ev.push(Event::Key { win: 0, down: true, vk: m, scan: 0, ch: None, ext: false, repeat: false });
    }
    let ch = if ctrl { None } else { ch };
    ev.push(Event::Key { win: 0, down: true, vk, scan: 0, ch, ext: matches!(vk, 0x21..=0x28 | 0x2D | 0x2E), repeat: false });
    ev.push(Event::Key { win: 0, down: false, vk, scan: 0, ch: None, ext: matches!(vk, 0x21..=0x28 | 0x2D | 0x2E), repeat: false });
    for &m in mods.iter().rev() {
        ev.push(Event::Key { win: 0, down: false, vk: m, scan: 0, ch: None, ext: false, repeat: false });
    }
    Ok(ev)
}

fn button_number(s: Option<&str>) -> u32 {
    match s {
        Some("right") => 3,
        Some("middle") => 2,
        _ => 1,
    }
}

/// Parses a script into steps; an error names the offending step.
fn parse_script(text: &str) -> Result<VecDeque<Step>, String> {
    let mut steps = VecDeque::new();
    for raw in text.split([';', '\n']) {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let (cmd, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(c, r)| (c, r.trim()));
        let args: Vec<&str> = rest.split_whitespace().collect();
        let num = |i: usize| -> Result<i32, String> { args.get(i).and_then(|v| v.parse().ok()).ok_or_else(|| format!("`{line}`: expected a number")) };
        let mut step = Step { wait_ms: 0, events: Vec::new() };
        match cmd {
            "wait" | "sleep" => step.wait_ms = num(0)? as u64,
            "move" => step.events.push(Event::Motion { win: 0, x: num(0)?, y: num(1)? }),
            "smove" => step.events.push(Event::Motion { win: SCREEN, x: num(0)?, y: num(1)? }),
            "sdown" | "sup" => step.events.push(Event::Button { win: SCREEN, button: button_number(args.get(2).copied()), down: cmd == "sdown", x: num(0)?, y: num(1)? }),
            "sclick" | "sdblclick" => {
                let (x, y, b) = (num(0)?, num(1)?, button_number(args.get(2).copied()));
                step.events.push(Event::Motion { win: SCREEN, x, y });
                for _ in 0..if cmd == "sdblclick" { 2 } else { 1 } {
                    step.events.push(Event::Button { win: SCREEN, button: b, down: true, x, y });
                    step.events.push(Event::Button { win: SCREEN, button: b, down: false, x, y });
                }
            }
            "down" | "up" => step.events.push(Event::Button { win: 0, button: button_number(args.get(2).copied()), down: cmd == "down", x: num(0)?, y: num(1)? }),
            "click" | "dblclick" => {
                let (x, y, b) = (num(0)?, num(1)?, button_number(args.get(2).copied()));
                step.events.push(Event::Motion { win: 0, x, y });
                for _ in 0..if cmd == "dblclick" { 2 } else { 1 } {
                    step.events.push(Event::Button { win: 0, button: b, down: true, x, y });
                    step.events.push(Event::Button { win: 0, button: b, down: false, x, y });
                }
            }
            "wheel" => step.events.push(Event::Wheel { win: 0, delta: num(0)?, x: num(1).unwrap_or(0), y: num(2).unwrap_or(0) }),
            "key" => step.events = key_events(rest)?,
            "type" => {
                for c in rest.chars() {
                    let (vk, _) = vk_for_name(&c.to_string()).unwrap_or((0, Some(c)));
                    step.events.push(Event::Key { win: 0, down: true, vk, scan: 0, ch: Some(c), ext: false, repeat: false });
                    step.events.push(Event::Key { win: 0, down: false, vk, scan: 0, ch: None, ext: false, repeat: false });
                }
            }
            "resize" => step.events.push(Event::Configure { win: 0, x: i32::MIN, y: i32::MIN, w: num(0)?, h: num(1)? }),
            "snapshot" => step.events.push(Event::Script(ScriptAction::Snapshot(rest.to_string()))),
            "close" => step.events.push(Event::Close { win: 0 }),
            "exit" => step.events.push(Event::Script(ScriptAction::Exit(args.first().and_then(|v| v.parse().ok()).unwrap_or(0)))),
            "print" => step.events.push(Event::Script(ScriptAction::Print(rest.to_string()))),
            other => return Err(format!("unknown script command `{other}`")),
        }
        steps.push_back(step);
    }
    Ok(steps)
}

impl Headless {
    pub fn from_env() -> Headless {
        let screen = std::env::var("NLE_GUI_SCREEN")
            .ok()
            .and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))))
            .unwrap_or((1280, 800));
        let script = std::env::var("NLE_GUI_SCRIPT").unwrap_or_default();
        Headless::with_script(screen, &script)
    }

    pub fn with_script(screen: (i32, i32), script: &str) -> Headless {
        let steps = match parse_script(script) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("nle: NLE_GUI_SCRIPT: {e}");
                VecDeque::new()
            }
        };
        Headless {
            screen,
            next: AtomicU64::new(1),
            windows: Mutex::new(HashMap::new()),
            script: Mutex::new(Script { steps, due: None }),
            clipboard: Mutex::new(String::new()),
            pointer: Mutex::new((0, 0, 0)),
        }
    }

    fn any_visible(&self) -> bool {
        self.windows.lock().unwrap_or_else(|e| e.into_inner()).values().any(|w| w.visible)
    }
}

impl Backend for Headless {
    fn name(&self) -> &'static str {
        "headless"
    }
    fn screen_size(&self) -> (i32, i32) {
        self.screen
    }
    fn create_window(&self, _spec: &WinSpec) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        self.windows.lock().unwrap_or_else(|e| e.into_inner()).insert(id, HeadWin { visible: false });
        id
    }
    fn destroy_window(&self, native: u64) {
        self.windows.lock().unwrap_or_else(|e| e.into_inner()).remove(&native);
    }
    fn show(&self, native: u64, visible: bool) {
        if let Some(w) = self.windows.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&native) {
            w.visible = visible;
        }
    }
    fn set_title(&self, _native: u64, _title: &str) {}
    fn move_resize(&self, _native: u64, _x: i32, _y: i32, _w: i32, _h: i32) {}
    fn set_resizable(&self, _native: u64, _resizable: bool, _w: i32, _h: i32) {}
    fn present(&self, _native: u64, _surf: &Surface, _rect: Rect) {}
    fn set_cursor(&self, _native: u64, _cursor: Cursor) {}
    fn focus(&self, _native: u64) {}
    fn raise(&self, _native: u64) {}
    fn grab_pointer(&self, _native: u64, _grab: bool) {}
    fn pointer(&self) -> (i32, i32, u32) {
        *self.pointer.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn bell(&self) {}
    fn set_clipboard(&self, text: &str) {
        *self.clipboard.lock().unwrap_or_else(|e| e.into_inner()) = text.to_string();
    }
    fn get_clipboard(&self) -> String {
        self.clipboard.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn poll(&self, wake_fd: i32, timeout_ms: i32) -> Vec<Event> {
        // The script is the "user": it acts only when the program is idle (blocked waiting for
        // input), so every step sees the result of the previous one, already painted.
        if timeout_ms == 0 {
            return Vec::new();
        }
        let deadline = if timeout_ms < 0 { None } else { Some(Instant::now() + Duration::from_millis(timeout_ms as u64)) };
        loop {
            // Script steps only run once a window is on screen, like a user would act on one.
            let mut wait_for: Option<Duration> = None;
            {
                let mut s = self.script.lock().unwrap_or_else(|e| e.into_inner());
                if self.any_visible() {
                    if let Some(step) = s.steps.front().cloned() {
                        let due = *s.due.get_or_insert_with(|| Instant::now() + Duration::from_millis(step.wait_ms));
                        if Instant::now() >= due {
                            s.steps.pop_front();
                            s.due = None;
                            if !step.events.is_empty() {
                                for e in &step.events {
                                    match e {
                                        Event::Motion { x, y, .. } => *self.pointer.lock().unwrap_or_else(|e| e.into_inner()) = (*x, *y, 0),
                                        Event::Button { x, y, down, .. } => *self.pointer.lock().unwrap_or_else(|e| e.into_inner()) = (*x, *y, if *down { 1 } else { 0 }),
                                        _ => {}
                                    }
                                }
                                return step.events;
                            }
                            continue;
                        }
                        wait_for = Some(due - Instant::now());
                    }
                }
            }
            let mut ms = match (deadline, wait_for) {
                (Some(d), Some(w)) => d.saturating_duration_since(Instant::now()).min(w).as_millis() as i32,
                (Some(d), None) => d.saturating_duration_since(Instant::now()).as_millis() as i32,
                (None, Some(w)) => w.as_millis() as i32 + 1,
                (None, None) => -1,
            };
            // Without a script step coming, a blocked pump still wakes now and then so a script
            // step enabled by a window appearing on another thread is not missed.
            if ms < 0 || ms > 50 {
                ms = if deadline.is_none() && wait_for.is_none() { 50 } else { ms.min(50) };
            }
            let mut pfd = libc::pollfd { fd: wake_fd, events: libc::POLLIN, revents: 0 };
            let n = unsafe { libc::poll(&mut pfd, if wake_fd >= 0 { 1 } else { 0 }, ms.max(0)) };
            if n > 0 {
                return Vec::new();
            }
            if let Some(d) = deadline {
                if Instant::now() >= d {
                    return Vec::new();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_parses_into_steps() {
        let s = parse_script("wait 10; click 5 6; key ctrl+a; type hi; snapshot /tmp/x.png; close").unwrap();
        assert_eq!(s.len(), 6);
        assert_eq!(s[0].wait_ms, 10);
        assert_eq!(s[1].events.len(), 3, "move, press, release");
        assert_eq!(s[2].events.len(), 4, "ctrl down, a down, a up, ctrl up");
        assert_eq!(s[3].events.len(), 4);
        assert!(matches!(s[4].events[0], Event::Script(ScriptAction::Snapshot(_))));
    }

    #[test]
    fn script_errors_name_the_problem() {
        assert!(parse_script("frobnicate 1").unwrap_err().contains("frobnicate"));
        assert!(parse_script("click x y").is_err());
        assert!(parse_script("key hyper+a").unwrap_err().contains("hyper"));
    }

    #[test]
    fn letter_keys_map_to_virtual_keys() {
        assert_eq!(vk_for_name("a"), Some((0x41, Some('a'))));
        assert_eq!(vk_for_name("F5"), Some((0x74, None)));
        assert_eq!(vk_for_name("enter").unwrap().0, 0x0D);
        assert_eq!(vk_for_name("nonsense"), None);
    }

    #[test]
    fn steps_wait_for_a_visible_window() {
        let h = Headless::with_script((800, 600), "close");
        assert!(h.poll(-1, 5).is_empty(), "no window yet, so nothing happens");
        assert!(h.poll(-1, 0).is_empty(), "a non-blocking probe never advances the script");
        let w = h.create_window(&WinSpec::default());
        assert!(h.poll(-1, 5).is_empty());
        h.show(w, true);
        let ev = h.poll(-1, 200);
        assert!(matches!(ev.first(), Some(Event::Close { .. })));
    }
}
