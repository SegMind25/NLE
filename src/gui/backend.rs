//! What the window system needs from a display: windows to put pixels in, and input coming back.
//! `x11` talks to a real X server; `headless` keeps everything in memory and can be scripted.

use super::surface::{Rect, Surface};
use std::sync::Arc;

/// What a new top-level window should look like.
#[derive(Clone, Debug, Default)]
pub struct WinSpec {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub w: i32,
    pub h: i32,
    pub title: String,
    pub resizable: bool,
    pub decorated: bool,
    pub tool: bool,
    pub topmost: bool,
    /// A menu or drop-down list: shown without the window manager's involvement and without taking focus.
    pub popup: bool,
    /// Native id of the owner, so dialogs stay in front of it.
    pub transient_for: Option<u64>,
}

/// In an event's `win`: the coordinates are screen coordinates and the window system finds the window under them.
pub const SCREEN: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    IBeam,
    Wait,
    Cross,
    Hand,
    SizeAll,
    SizeWE,
    SizeNS,
    SizeNWSE,
    SizeNESW,
    No,
}

/// Something a headless script asks the window system to do besides sending input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptAction {
    Snapshot(String),
    Exit(i32),
    Print(String),
}

#[derive(Clone, Debug)]
pub enum Event {
    Expose { win: u64, rect: Rect },
    /// The client area moved or changed size (position in screen coordinates).
    Configure { win: u64, x: i32, y: i32, w: i32, h: i32 },
    Close { win: u64 },
    /// `vk` is the Windows virtual-key code; `ch` the character the key produced, if any;
    /// `ext` marks extended keys (arrows, navigation block, right Ctrl/Alt).
    Key { win: u64, down: bool, vk: u32, scan: u32, ch: Option<char>, ext: bool, repeat: bool },
    /// `button`: 1 left, 2 middle, 3 right. Coordinates are client coordinates.
    Button { win: u64, button: u32, down: bool, x: i32, y: i32 },
    Motion { win: u64, x: i32, y: i32 },
    Wheel { win: u64, delta: i32, x: i32, y: i32 },
    Focus { win: u64, gained: bool },
    Script(ScriptAction),
}

pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    fn screen_size(&self) -> (i32, i32);
    fn create_window(&self, spec: &WinSpec) -> u64;
    fn destroy_window(&self, native: u64);
    fn show(&self, native: u64, visible: bool);
    fn set_title(&self, native: u64, title: &str);
    fn move_resize(&self, native: u64, x: i32, y: i32, w: i32, h: i32);
    fn set_resizable(&self, native: u64, resizable: bool, w: i32, h: i32);
    /// Shows `rect` of the surface in the window.
    fn present(&self, native: u64, surf: &Surface, rect: Rect);
    fn set_cursor(&self, native: u64, cursor: Cursor);
    fn focus(&self, native: u64);
    fn raise(&self, native: u64);
    fn grab_pointer(&self, native: u64, grab: bool);
    /// Pointer position in screen coordinates and the button mask (bit 0 left, 1 right, 2 middle).
    fn pointer(&self) -> (i32, i32, u32);
    fn bell(&self);
    /// Waits up to `timeout_ms` (-1 forever) for input or for `wake_fd` to become readable.
    fn poll(&self, wake_fd: i32, timeout_ms: i32) -> Vec<Event>;
    /// Windows can ask for the clipboard text; backends without one keep it in memory.
    fn set_clipboard(&self, text: &str);
    fn get_clipboard(&self) -> String;
}

/// Picks and starts a backend according to `NLE_GUI`: `x11`, `headless`, or `auto` (the default:
/// X11 when a display is set, otherwise an error explaining what to do).
pub fn start() -> Result<Arc<dyn Backend>, String> {
    let choice = std::env::var("NLE_GUI").unwrap_or_else(|_| "auto".into()).to_ascii_lowercase();
    match choice.as_str() {
        "headless" | "none" | "offscreen" => Ok(Arc::new(super::headless::Headless::from_env())),
        "x11" => super::x11::X11::open().map(|b| Arc::new(b) as Arc<dyn Backend>),
        _ => {
            if std::env::var_os("DISPLAY").is_some() {
                super::x11::X11::open().map(|b| Arc::new(b) as Arc<dyn Backend>)
            } else {
                Err("there is no display to open a window on (DISPLAY is not set); start an X server or use NLE_GUI=headless".into())
            }
        }
    }
}
