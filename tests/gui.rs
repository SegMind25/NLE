//! GUI programs from `examples/`, run two ways:
//!
//! * on the headless display, driven by `NLE_GUI_SCRIPT` (always available), and
//! * on a real X server (Xvfb), driven by `tests/gui/xdrive.c` with real X events (when Xvfb, a
//!   C compiler and the X11 headers are installed).
//!
//! MinGW-w64 is needed to build the programs; without it the tests print a notice and pass.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn nle() -> &'static str {
    env!("CARGO_BIN_EXE_nle")
}

fn tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

fn work_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("gui");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Builds the example programs once. `None` when MinGW is missing.
fn programs() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let gcc = tool("x86_64-w64-mingw32-gcc")?;
        let dir = work_dir().join("bin");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        let windres = tool("x86_64-w64-mingw32-windres");
        for name in ["window", "controls", "paint", "msgbox", "scroll", "lists", "menus", "dialogs", "common"] {
            let c = src.join(format!("{name}.c"));
            let gui = std::fs::read_to_string(&c).unwrap().contains("WinMain");
            let mut cmd = Command::new(&gcc);
            cmd.args(["-O1"]);
            if gui {
                cmd.arg("-mwindows");
            }
            cmd.arg("-o").arg(dir.join(format!("{name}.exe"))).arg(&c);
            // a program with resources has a .rc next to it; without windres it is left out
            if src.join(format!("{name}.rc")).exists() {
                let Some(windres) = &windres else { continue };
                let obj = dir.join(format!("{name}_rc.o"));
                let out = Command::new(windres).current_dir(&src).arg(format!("{name}.rc")).args(["-O", "coff", "-o"]).arg(&obj).output().expect("windres did not start");
                assert!(out.status.success(), "windres {name}: {}", String::from_utf8_lossy(&out.stderr));
                cmd.arg(&obj);
            }
            cmd.args(["-lgdi32", "-luser32", "-lcomctl32"]);
            let out = cmd.output().expect("compiler did not start");
            assert!(out.status.success(), "compiling {name}: {}", String::from_utf8_lossy(&out.stderr));
        }
        // programs that exist only for the tests
        let c = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gui/t_sbctl.c");
        let out = Command::new(&gcc).arg("-O1").arg("-o").arg(dir.join("t_sbctl.exe")).arg(&c).arg("-luser32").output().expect("compiler did not start");
        assert!(out.status.success(), "compiling t_sbctl: {}", String::from_utf8_lossy(&out.stderr));
        let c = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gui/t_comctl.c");
        let out = Command::new(&gcc).arg("-O1").arg("-o").arg(dir.join("t_comctl.exe")).arg(&c).args(["-lgdi32", "-luser32", "-lcomctl32"]).output().expect("compiler did not start");
        assert!(out.status.success(), "compiling t_comctl: {}", String::from_utf8_lossy(&out.stderr));
        Some(dir)
    })
    .as_ref()
}

// ---- PNG snapshots (our writer uses stored deflate blocks, so reading them back is simple) -----------

struct Image {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

impl Image {
    fn read(path: &Path) -> Image {
        let data = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(&data[..8], b"\x89PNG\r\n\x1a\n");
        let (mut pos, mut w, mut h, mut idat) = (8, 0usize, 0usize, Vec::new());
        while pos + 8 <= data.len() {
            let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            let kind = &data[pos + 4..pos + 8];
            let body = &data[pos + 8..pos + 8 + len];
            match kind {
                b"IHDR" => {
                    w = u32::from_be_bytes(body[0..4].try_into().unwrap()) as usize;
                    h = u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize;
                }
                b"IDAT" => idat.extend_from_slice(body),
                _ => {}
            }
            pos += 12 + len;
        }
        let mut raw = Vec::new();
        let mut i = 2; // zlib header
        loop {
            let last = idat[i] & 1 == 1;
            assert_eq!(idat[i] >> 1 & 3, 0, "only stored blocks are expected");
            let n = u16::from_le_bytes([idat[i + 1], idat[i + 2]]) as usize;
            raw.extend_from_slice(&idat[i + 5..i + 5 + n]);
            i += 5 + n;
            if last {
                break;
            }
        }
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            assert_eq!(raw[y * (w * 3 + 1)], 0, "filter type");
            rgb.extend_from_slice(&raw[y * (w * 3 + 1) + 1..(y + 1) * (w * 3 + 1)]);
        }
        Image { w, h, rgb }
    }

    fn from_ppm(path: &Path) -> Image {
        let data = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut it = data.splitn(4, |b| *b == b'\n');
        assert_eq!(it.next().unwrap(), b"P6");
        let dims = String::from_utf8_lossy(it.next().unwrap()).to_string();
        let (w, h) = dims.split_once(' ').unwrap();
        it.next();
        Image { w: w.parse().unwrap(), h: h.parse().unwrap(), rgb: it.next().unwrap().to_vec() }
    }

    fn px(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let o = (y * self.w + x) * 3;
        (self.rgb[o], self.rgb[o + 1], self.rgb[o + 2])
    }

    /// Pixels in the rectangle that are noticeably darker than mid-grey.
    fn dark_in(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> usize {
        (y0..y1.min(self.h)).flat_map(|y| (x0..x1.min(self.w)).map(move |x| (x, y))).filter(|&(x, y)| {
            let p = self.px(x, y);
            (p.0 as u32 + p.1 as u32 + p.2 as u32) / 3 < 110
        }).count()
    }

    fn differs_in(&self, other: &Image, x0: usize, y0: usize, x1: usize, y1: usize) -> usize {
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| self.px(x, y) != other.px(x, y)).count()
    }

    fn near(&self, x: usize, y: usize, want: (u8, u8, u8), radius: usize) -> bool {
        (y.saturating_sub(radius)..=y + radius).any(|yy| (x.saturating_sub(radius)..=x + radius).any(|xx| xx < self.w && yy < self.h && self.px(xx, yy) == want))
    }
}

fn run_headless(exe: &str, script: &str) -> std::process::Output {
    run_headless_args(exe, &[], script)
}

fn run_headless_args(exe: &str, args: &[&str], script: &str) -> std::process::Output {
    let mut child = Command::new(nle())
        .arg("run")
        .arg(programs().unwrap().join(format!("{exe}.exe")))
        .args(args)
        .env("NLE_GUI", "headless")
        .env("NLE_GUI_SCRIPT", script)
        .env("NLE_PREFIX", work_dir().join(format!("prefix-{exe}")))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().ok();
            let out = child.wait_with_output().unwrap();
            panic!("{exe} did not finish; stderr: {}", String::from_utf8_lossy(&out.stderr));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

macro_rules! need_mingw {
    () => {
        if programs().is_none() {
            eprintln!("skipping: MinGW-w64 is not installed");
            return;
        }
    };
}

macro_rules! need_exe {
    ($name:literal) => {
        need_mingw!();
        if !programs().unwrap().join(concat!($name, ".exe")).exists() {
            eprintln!("skipping: {} needs windres", $name);
            return;
        }
    };
}

fn shot(name: &str) -> PathBuf {
    work_dir().join(format!("{name}.png"))
}

// ---- headless ---------------------------------------------------------------------------------------------

#[test]
fn messages_sent_from_another_thread_are_answered_by_the_window_thread() {
    need_mingw!();
    let gcc = tool("x86_64-w64-mingw32-gcc").unwrap();
    let exe = work_dir().join("t_threads.exe");
    let c = Command::new(gcc).arg("-O1").arg("-o").arg(&exe).arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gui/t_threads.c")).args(["-luser32"]).output().unwrap();
    assert!(c.status.success(), "{}", String::from_utf8_lossy(&c.stderr));
    let out = Command::new(nle()).arg("run").arg(&exe).env("NLE_GUI", "headless").env_remove("NLE_GUI_SCRIPT").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("ALL OK"), "{text}{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn window_program_draws_its_text_and_closes() {
    need_mingw!();
    let png = shot("window");
    let out = run_headless("window", &format!("wait 100; snapshot {}; close", png.display()));
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let img = Image::read(&png);
    // WS_OVERLAPPEDWINDOW 480 x 240 has a 464 x 201 client area, as on Windows 10
    assert_eq!((img.w, img.h), (464, 201));
    assert!(img.dark_in(15, 15, 230, 45) > 40, "the greeting should be drawn at (20, 20)");
    assert_eq!(img.px(300, 150), (255, 255, 255));
}

#[test]
fn form_controls_react_to_clicks_and_typing() {
    need_mingw!();
    let (before, after) = (shot("controls-before"), shot("controls-after"));
    let out = run_headless(
        "controls",
        &format!("wait 100; snapshot {}; click 130 25; type Ada; click 340 26; snapshot {}; close", before.display(), after.display()),
    );
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let (a, b) = (Image::read(&before), Image::read(&after));
    assert!(b.dark_in(102, 16, 280, 36) > 25, "the typed name shows in the text box");
    assert!(a.differs_in(&b, 16, 112, 380, 140) > 100, "the greeting label changed");
    // the default button keeps its blue border
    assert_eq!(b.px(300, 20), (0, 120, 215));
}

fn lines(out: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

#[test]
fn window_scroll_bars_send_scroll_messages_and_move_the_thumb() {
    need_mingw!();
    let png = shot("scroll");
    // 320 x 200 with both bars: the vertical bar is x 303..320, the horizontal one y 183..200
    let script = format!("wait 200; click 311 175; click 311 100; down 311 40; move 311 90; up 311 90; click 295 191; wait 100; snapshot {}; close", png.display());
    let out = run_headless_args("scroll", &["--log"], &script);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let got = lines(&out);
    // arrow down, page down, then a thumb drag (track, position, end), then the horizontal arrow
    assert_eq!(&got[..4], ["V 1 1", "V 8 1", "V 3 12", "V 8 12"], "{got:?}");
    assert!(got.iter().any(|l| l.starts_with("V 5 ")), "dragging the thumb tracks: {got:?}");
    assert!(got.iter().any(|l| l.starts_with("V 4 ")), "releasing it reports the position: {got:?}");
    assert!(got.contains(&"H 1 1".to_string()), "{got:?}");
    let img = Image::read(&png);
    assert_eq!((img.w, img.h), (320, 200));
    assert_eq!(img.px(311, 140), (240, 240, 240), "the track is light grey");
    assert!(img.near(311, 92, (205, 205, 205), 14), "the thumb sits where it was dragged");
    assert_eq!(img.px(311, 190), (240, 240, 240), "the corner between the bars");
}

#[test]
fn menu_bar_menus_accelerators_and_checks() {
    need_mingw!();
    let png = shot("menus");
    // the client area starts at screen (103, 126) and the 20 px menu bar is its top; File is the first item
    let script = format!(
        "wait 200; snapshot {}; key ctrl+n; key f5; sclick 113 136; wait 100; sclick 143 155; wait 100; \
         key alt+e; key down; key down; key down; key enter; wait 50; \
         key alt+v; key enter; key alt+v; key down; key right; key down; key enter; wait 50; close",
        png.display()
    );
    let out = run_headless_args("menus", &["--log"], &script);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let want = [
        "ACCEL 101 New", // Ctrl+N and F5 come from the accelerator table
        "ACCEL 113 Refresh",
        "INIT", // opening File by mouse tells the window (WM_INITMENUPOPUP)
        "CMD 101 New",
        "INIT", // Alt+E opens Edit; Undo is first, Cut is grayed but selectable, Copy, then Paste
        "CMD 107 Paste",
        "INIT",
        "CMD 108 Word wrap",
        "INIT", // View, down to Zoom, right into its submenu, down to 150%
        "INIT",
        "CMD 110 150%",
    ];
    assert_eq!(lines(&out), want);
    let img = Image::read(&png);
    // 360 x 120 of client area plus the menu bar above it
    assert_eq!((img.w, img.h), (360, 140));
    assert_eq!(img.px(300, 5), (240, 240, 240), "the menu bar is light grey");
    assert!(img.dark_in(6, 3, 36, 17) > 8, "File is written on the bar");
    assert_eq!(img.px(300, 80), (255, 255, 255), "the client area below is the window colour");
}

#[test]
fn context_menu_returns_the_command_or_zero_when_cancelled() {
    need_mingw!();
    let script = "wait 200; sclick 250 190 right; wait 100; key down; key down; key enter; wait 100; \
                  sclick 250 190 right; wait 100; sclick 700 600; wait 100; \
                  sclick 250 190 right; wait 100; key escape; wait 50; key alt+f; key x; wait 100";
    let out = run_headless_args("menus", &["--log"], script);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    // Paste by keyboard; a click elsewhere and Esc cancel; Alt+F, X is File > Exit
    assert_eq!(lines(&out), ["INIT", "POPUP 107 Paste", "INIT", "POPUP 0 ?", "INIT", "POPUP 0 ?", "INIT"]);
}

#[test]
fn dialog_from_resources_is_filled_navigated_and_answered_with_the_keyboard() {
    need_exe!("dialogs");
    let png = shot("dialogs-main");
    // Ctrl+S opens Settings; type a name, Tab to the check box and toggle it, Tab to the radio group and
    // press Down, Tab to the list and Down, Tab to the combo box and Down, Enter presses the default button.
    // The second time Alt+R, Alt+N (a label: the focus goes to the edit), a name, Alt+S and Enter; the third
    // time Esc cancels; Alt+H, A opens About and Enter closes it.
    let script = format!(
        "wait 200; snapshot {}; key ctrl+s; wait 100; type Ada; key tab; key space; key tab; key down; key tab; key down; key tab; key down; key enter; wait 100; \
         key ctrl+s; wait 100; key alt+r; key alt+n; type Bob; key alt+s; key enter; wait 100; \
         key ctrl+s; wait 100; key escape; wait 100; key alt+h; key a; wait 100; key enter; wait 100; close",
        png.display()
    );
    let out = run_headless_args("dialogs", &["--log"], &script);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        lines(&out),
        [
            "SETTINGS name=Ada shout=1 colour=Green list=Beta combo=Two",
            "SETTINGS name=Bob shout=0 colour=Red list=Beta combo=Two",
            "CANCEL",
            "ABOUT closed (NLE dialogs example)",
        ]
    );
    // the menu came from the MENU resource and the icon from the ICON resource
    let img = Image::read(&png);
    assert_eq!((img.w, img.h), (420, 100), "420 x 80 of client area and the menu bar");
    assert!(img.dark_in(6, 3, 60, 17) > 12, "the menu bar shows File and Help");
    assert!(img.near(22, 40, (0, 120, 215), 8), "the blue application icon is drawn");
}

#[test]
fn x11_dialog_opens_from_the_menu_and_answers_to_the_keyboard() {
    need_exe!("dialogs");
    if start_xvfb().is_none() || xdrive().is_none() || !xdrive_has_xtest() {
        eprintln!("skipping: needs Xvfb, the X11 headers and libXtst");
        return;
    }
    let ppm = work_dir().join("x-dialog.ppm");
    // File > Settings with the pointer; then the dialog window gets the keys
    let steps = ["fclick", "10", "10", "sleep", "200", "fclick", "40", "34", "sleep", "300", "title", "Settings", "wait", "5000", "type", "Eve", "key", "Tab", "key", "space", "sleep", "100", "shot", ppm.to_str().unwrap(), "key", "Return", "sleep", "200", "title", "NLE dialogs", "wait", "3000", "close"];
    let (code, out) = on_x_with("dialogs", &["--log"], "NLE dialogs", &steps).expect("ran");
    assert_eq!(code, 0);
    assert_eq!(out.lines().map(str::trim).collect::<Vec<_>>(), ["SETTINGS name=Eve shout=1 colour=Red list=Alpha combo=One"], "{out}");
    // the dialog is a real window with its controls drawn: its own size, and text in it
    let img = Image::from_ppm(&ppm);
    assert!(img.w > 200 && img.h > 100, "{}x{}", img.w, img.h);
    assert!(img.dark_in(40, 5, 160, 30) > 20, "the name that was typed is in the edit control");
}

#[test]
fn scroll_bar_controls_report_to_their_parent() {
    need_mingw!();
    // a vertical bar at (170, 10) 17 x 120 and a horizontal one at (10, 140) 150 x 17; range 0..109 with a page of 10
    let script = "wait 200; click 178 122; click 178 80; down 178 42; move 178 70; move 178 90; up 178 90; click 150 148; wait 50; close";
    let out = run_headless("t_sbctl", script);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let got = lines(&out);
    assert_eq!(got[0], "range 0 100 pos 0", "{got:?}");
    assert_eq!(&got[1..5], ["V 1 1", "V 8 1", "V 3 11", "V 8 11"], "{got:?}");
    let tracks: Vec<i32> = got.iter().filter_map(|l| l.strip_prefix("V 5 ")?.parse().ok()).collect();
    assert!(tracks.len() >= 2 && tracks.windows(2).all(|w| w[1] >= w[0]), "dragging down tracks increasing positions: {got:?}");
    assert!(got.iter().any(|l| l.starts_with("V 4 ")) && got.contains(&"H 1 1".to_string()), "{got:?}");
}

#[test]
fn list_and_combo_boxes_select_and_notify() {
    need_mingw!();
    // the window's client area starts at screen (103, 126); the drop-down opens just under the combo box
    let script = "wait 200; click 20 20; key down; key down; \
                  click 330 20; sclick 340 165; \
                  click 330 20; key down; key down; key enter; \
                  click 330 20; sclick 700 500; \
                  click 330 60; type hello; \
                  click 20 150; click 20 165; close";
    let out = run_headless_args("lists", &["--log"], script);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let got = lines(&out);
    let want = [
        "LB 0 Apple", // the sorted list starts with Apple
        "LB 1 Banana",
        "LB 2 Cherry",
        "CB 104 open",
        "CB 104 select 0 Cherry", // the combo box keeps its items in the order they were added
        "CB 104 closed",
        "CB 104 open",
        "CB 104 select 2 Elderberry",
        "CB 104 closed",
        "CB 104 open",
        "CB 104 closed", // a click outside the list closes it without selecting
    ];
    assert_eq!(&got[..want.len()], want, "{got:?}");
    assert!(got.contains(&"CB 105 text hello".to_string()), "typing into the editable combo box: {got:?}");
    assert!(got.contains(&"MULTI 1: 1".to_string()), "{got:?}");
    assert!(got.contains(&"MULTI 1: 2".to_string()) || got.iter().any(|l| l.starts_with("MULTI 1:")), "{got:?}");
}

#[test]
fn common_controls_toolbar_list_tree_tabs_and_spin_notify_the_program() {
    need_mingw!();
    // tool bar buttons are 23 pixels wide; the list rows start at y=86 and are 18 high; the tree's labels sit at x=60
    let script = "wait 300; click 11 13; click 88 13; click 134 13; click 165 13; click 165 13; \
                  click 100 86; click 100 122; key down; click 200 66; dblclick 100 104; \
                  click 65 42; click 41 83; click 60 137; key down; key left; key left; key right; \
                  click 110 42; click 200 98; click 162 152; click 162 163; close";
    let out = run_headless_args("common", &["--log"], script);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let want = [
        "TV expand Computer",
        "TB 1001", // New
        "TB 1004", // Cut; the disabled Paste button says nothing
        "TB 1007 on",
        "TB 1007 off",
        "LV select 0 readme.txt",
        "LV select 2 photo.png",
        "LV select 3 setup.exe", // the Down key
        "LV column 1",
        "LV select 1 notes.txt",
        "LV open 1",
        "TAB 1",
        "TV expand Documents", // the + button opens the item without selecting it
        "TV select Pictures",
    ];
    let got = lines(&out);
    assert_eq!(&got[..want.len()], want, "{got:?}");
    // Down moves to Music; Left on a leaf goes to its parent (Computer) and Left again collapses it; Right opens it again
    let tree: Vec<_> = got.iter().filter(|l| l.starts_with("TV ")).skip(3).cloned().collect();
    assert_eq!(tree, ["TV select Music", "TV select Computer", "TV collapse Computer", "TV expand Computer"], "{got:?}");
    assert!(got.contains(&"TAB 2".to_string()), "{got:?}");
    assert!(got.contains(&"TRACK 45".to_string()), "a click in the track bar's channel pages the thumb: {got:?}");
    let tail: Vec<_> = got.iter().rev().take(2).rev().cloned().collect();
    assert_eq!(tail, ["SPIN 6", "SPIN 5"], "the spin arrows step the number: {got:?}");
}

#[test]
fn common_controls_answer_their_messages() {
    need_mingw!();
    // the last check is a TaskDialog, which the script answers with Enter
    let out = run_headless("t_comctl", "wait 100; key enter");
    let got = lines(&out);
    let failed: Vec<_> = got.iter().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failed.is_empty(), "{failed:?}\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(got.last().map(String::as_str), Some("DONE 0"), "{got:?}");
    assert!(got.iter().filter(|l| l.starts_with("ok")).count() > 75, "{} checks ran", got.len());
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn switching_pages_repaints_only_what_changed_and_keeps_the_tool_bar() {
    need_mingw!();
    let (before, after) = (shot("common-files"), shot("common-tree"));
    run_headless("common", &format!("wait 300; snapshot {}; click 65 42; wait 100; snapshot {}; close", before.display(), after.display()));
    let (a, b) = (Image::read(&before), Image::read(&after));
    assert!(a.dark_in(0, 0, 180, 26) > 100, "the tool bar draws its icons");
    assert_eq!(b.differs_in(&a, 0, 0, 520, 26), 0, "the tool bar is untouched by the page switch");
    assert_eq!(b.differs_in(&a, 0, 336, 330, 360) > 0, true, "the status text changed");
    assert!(b.differs_in(&a, 10, 60, 500, 300) > 500, "the tree replaced the list");
}

#[test]
fn radio_buttons_and_check_boxes_toggle() {
    need_mingw!();
    let (before, after) = (shot("radio-before"), shot("radio-after"));
    run_headless("controls", &format!("wait 100; snapshot {}; click 108 90; click 108 62; snapshot {}; close", before.display(), after.display()));
    let (a, b) = (Image::read(&before), Image::read(&after));
    // the red radio gets its dot, the check box its tick, and blue loses the dot
    assert!(b.differs_in(&a, 100, 80, 114, 100) > 10);
    assert!(b.differs_in(&a, 100, 52, 114, 72) > 10);
    assert!(b.differs_in(&a, 240, 80, 254, 100) > 10);
}

#[test]
fn drawing_with_the_mouse_uses_the_chosen_colour() {
    need_mingw!();
    let png = shot("paint");
    let out = run_headless("paint", &format!("wait 100; down 50 100; move 150 150; move 250 200; up 250 200; key 2; down 80 300; move 240 310; move 400 280; up 400 280; wait 50; snapshot {}; close", png.display()));
    // the program exits with the number of strokes it saw
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    let img = Image::read(&png);
    assert!(img.near(150, 150, (0, 120, 215), 3), "the first stroke is blue");
    assert!(img.near(240, 310, (232, 17, 35), 3), "after pressing 2 the stroke is red");
    assert_eq!(img.px(600, 380), (255, 255, 255), "the rest of the canvas is untouched");
}

#[test]
fn clearing_the_canvas_resets_the_stroke_count() {
    need_mingw!();
    let out = run_headless("paint", "wait 100; down 50 100; move 150 150; up 150 150; key c; key escape");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn message_box_returns_the_chosen_button() {
    need_mingw!();
    let out = run_headless("msgbox", "wait 100; key y; wait 100; key enter");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "answer: yes");
    assert_eq!(out.status.code(), Some(0));
    let out = run_headless("msgbox", "wait 100; key n; wait 100; key enter");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "answer: no");
    assert_eq!(out.status.code(), Some(1));
    // Enter takes the default button, and clicking a button works too
    let out = run_headless("msgbox", "wait 100; key enter; wait 100; key enter");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "answer: yes");
}

#[test]
fn message_box_without_a_script_answers_with_the_default() {
    need_mingw!();
    let out = Command::new(nle())
        .arg("run")
        .arg(programs().unwrap().join("msgbox.exe"))
        .env("NLE_GUI", "headless")
        .env_remove("NLE_GUI_SCRIPT")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "answer: yes");
    assert!(String::from_utf8_lossy(&out.stderr).contains("Do you want NLE to run this program?"), "the box is printed instead");
}

#[test]
fn a_window_program_without_a_display_stops_with_an_explanation() {
    need_mingw!();
    let out = Command::new(nle()).arg("run").arg(programs().unwrap().join("window.exe")).env_remove("DISPLAY").env_remove("NLE_GUI").output().unwrap();
    assert_eq!(out.status.code(), Some(70));
    assert!(String::from_utf8_lossy(&out.stderr).contains("DISPLAY"), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_relinked_gui_program_runs_the_same() {
    need_mingw!();
    let dir = work_dir().join("relinked");
    let _ = std::fs::remove_dir_all(&dir);
    let r = Command::new(nle()).arg("relink").arg(programs().unwrap().join("window.exe")).arg("-o").arg(&dir).output().unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    if !dir.join("libnle.so").exists() {
        return;
    }
    let png = shot("relinked");
    let out = Command::new(dir.join("window")).env("NLE_GUI", "headless").env("NLE_GUI_SCRIPT", format!("wait 100; snapshot {}; close", png.display())).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(Image::read(&png).dark_in(15, 15, 230, 45) > 40);
}

#[test]
fn inspect_reports_gui_programs_as_native() {
    need_mingw!();
    let out = Command::new(nle()).arg("inspect").arg(programs().unwrap().join("controls.exe")).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("verdict     native"), "{text}");
    assert!(text.contains("window"), "the report mentions that it opens windows: {text}");
}

// ---- a real X server -------------------------------------------------------------------------------------------

struct Xvfb {
    child: Child,
    display: String,
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_xvfb() -> Option<Xvfb> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let xvfb = tool("Xvfb")?;
    for _ in 0..20 {
        let n = 100 + (std::process::id() % 400) + NEXT.fetch_add(7, Ordering::SeqCst);
        let socket = format!("/tmp/.X11-unix/X{n}");
        if Path::new(&socket).exists() {
            continue;
        }
        let mut child = Command::new(&xvfb).arg(format!(":{n}")).args(["-screen", "0", "1024x768x24", "-nolisten", "tcp", "-noreset"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(8) {
            if Path::new(&socket).exists() {
                return Some(Xvfb { child, display: format!(":{n}") });
            }
            if child.try_wait().ok()?.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        let _ = child.kill();
    }
    None
}

/// The X test driver, built with real pointer injection (XTEST) when libXtst is there.
fn xdrive() -> Option<&'static PathBuf> {
    static X: OnceLock<Option<(PathBuf, bool)>> = OnceLock::new();
    X.get_or_init(|| {
        let cc = tool("cc").or_else(|| tool("gcc"))?;
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gui/xdrive.c");
        let out = work_dir().join("xdrive");
        let with_xtest = Command::new(&cc).arg("-O1").arg("-o").arg(&out).arg(&src).args(["-lX11", "-l:libXtst.so.6"]).output().ok()?.status.success();
        if with_xtest {
            return Some((out, true));
        }
        let ok = Command::new(&cc).arg("-O1").arg("-DNO_XTEST").arg("-o").arg(&out).arg(&src).arg("-lX11").output().ok()?.status.success();
        ok.then_some((out, false))
    })
    .as_ref()
    .map(|(p, _)| p)
}

fn xdrive_has_xtest() -> bool {
    xdrive().is_some() && Command::new(xdrive().unwrap()).arg("x").arg("sleep").arg("0").arg("fclick").arg("0").arg("0").output().map(|o| o.status.code() != Some(5)).unwrap_or(false)
}

/// Runs a program on a fresh X server while `xdrive` plays the user; returns the program's exit code.
fn on_x(exe: &str, title: &str, steps: &[&str]) -> Option<i32> {
    on_x_with(exe, &[], title, steps).map(|(code, _)| code)
}

/// Like `on_x`, with program arguments, and returning what the program printed.
fn on_x_with(exe: &str, args: &[&str], title: &str, steps: &[&str]) -> Option<(i32, String)> {
    let x = start_xvfb()?;
    let drive = xdrive()?;
    let mut prog = Command::new(nle()).arg("run").arg(programs()?.join(format!("{exe}.exe"))).args(args).env("DISPLAY", &x.display).env_remove("NLE_GUI").env("NLE_PREFIX", work_dir().join(format!("xprefix-{exe}"))).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let mut cmd = Command::new(drive);
    cmd.env("DISPLAY", &x.display).arg(title).arg("wait").arg("8000").args(steps);
    let driven = cmd.output().ok()?;
    assert!(driven.status.success(), "xdrive: {}", String::from_utf8_lossy(&driven.stderr));
    let start = Instant::now();
    loop {
        if let Some(st) = prog.try_wait().ok()? {
            let mut text = String::new();
            if let Some(mut o) = prog.stdout.take() {
                std::io::Read::read_to_string(&mut o, &mut text).ok();
            }
            return st.code().map(|c| (c, text));
        }
        if start.elapsed() > Duration::from_secs(20) {
            prog.kill().ok();
            panic!("{exe} did not exit after its window was closed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

macro_rules! need_x {
    () => {
        need_mingw!();
        if start_xvfb().is_none() || xdrive().is_none() {
            eprintln!("skipping: needs Xvfb and a C compiler with the X11 headers");
            return;
        }
    };
}

#[test]
fn x11_window_shows_pixels_and_closes_with_its_close_button() {
    need_x!();
    let ppm = work_dir().join("x-window.ppm");
    let code = on_x("window", "NLE demo", &["shot", ppm.to_str().unwrap(), "close"]).expect("ran");
    assert_eq!(code, 0);
    let img = Image::from_ppm(&ppm);
    assert_eq!((img.w, img.h), (464, 201));
    assert!(img.dark_in(15, 15, 230, 45) > 40, "the text reached the X window");
}

#[test]
fn x11_keyboard_and_mouse_reach_the_controls() {
    need_x!();
    let ppm = work_dir().join("x-controls.ppm");
    let code = on_x("controls", "NLE controls", &["click", "130", "25", "type", "Ada", "click", "340", "26", "sleep", "200", "shot", ppm.to_str().unwrap(), "close"]).expect("ran");
    assert_eq!(code, 0);
    let img = Image::from_ppm(&ppm);
    assert!(img.dark_in(102, 16, 280, 36) > 25, "typed text appears in the box");
    assert!(img.dark_in(16, 112, 380, 140) > 40, "the greeting appears");
}

#[test]
fn x11_mouse_drag_draws_and_the_program_counts_strokes() {
    need_x!();
    let ppm = work_dir().join("x-paint.ppm");
    let code = on_x("paint", "NLE paint", &["drag", "50", "100", "300", "200", "key", "2", "drag", "80", "300", "400", "280", "sleep", "100", "shot", ppm.to_str().unwrap(), "close"]).expect("ran");
    assert_eq!(code, 2);
    let img = Image::from_ppm(&ppm);
    assert!(img.near(175, 150, (0, 120, 215), 3), "the blue stroke is on screen");
    assert!(img.near(240, 290, (232, 17, 35), 6), "the red stroke is on screen");
}

#[test]
fn x11_combo_box_drop_down_opens_as_a_popup_and_selects_with_the_mouse() {
    need_x!();
    if !xdrive_has_xtest() {
        eprintln!("skipping: needs libXtst for real pointer events");
        return;
    }
    let ppm = work_dir().join("x-combo.ppm");
    let steps = ["fclick", "330", "20", "sleep", "300", "rshot", ppm.to_str().unwrap(), "fclick", "250", "42", "sleep", "300", "close"];
    let (code, out) = on_x_with("lists", &["--log"], "NLE lists", &steps).expect("ran");
    assert_eq!(code, 0);
    let got: Vec<&str> = out.lines().map(str::trim).collect();
    assert_eq!(got, ["CB 104 open", "CB 104 select 0 Cherry", "CB 104 closed"], "{out}");
    // the list is a window of its own over the form: its first row is on screen below the combo box
    let img = Image::from_ppm(&ppm);
    assert!(img.dark_in(290, 162, 380, 174) > 15, "the drop-down list is drawn on the screen");
}

#[test]
fn x11_common_controls_switch_pages_and_expand_the_tree() {
    need_x!();
    if !xdrive_has_xtest() {
        eprintln!("skipping: needs libXtst for real pointer events");
        return;
    }
    let ppm = work_dir().join("x-common.ppm");
    // the Tree tab, then the + button beside "Documents"
    let steps = ["fclick", "65", "42", "sleep", "200", "fclick", "41", "83", "sleep", "200", "rshot", ppm.to_str().unwrap(), "close"];
    let (code, out) = on_x_with("common", &["--log"], "NLE common controls", &steps).expect("ran");
    assert_eq!(code, 0);
    let got: Vec<&str> = out.lines().map(str::trim).collect();
    assert_eq!(got, ["TV expand Computer", "TAB 1", "TV expand Documents"], "{out}");
    let img = Image::from_ppm(&ppm);
    assert!(img.dark_in(0, 0, 180, 26) > 100, "the tool bar is drawn");
    assert!(img.dark_in(30, 60, 140, 200) > 100, "the tree's labels are drawn");
}

#[test]
fn x11_menus_open_with_the_pointer_and_the_keyboard() {
    need_x!();
    if !xdrive_has_xtest() {
        eprintln!("skipping: needs libXtst for real pointer events");
        return;
    }
    let ppm = work_dir().join("x-menu.ppm");
    // the window holds the menu bar in its top 20 rows: click File, then New; F10, V opens View; Enter toggles Word wrap
    let steps = ["fclick", "10", "10", "sleep", "200", "rshot", ppm.to_str().unwrap(), "fclick", "40", "32", "sleep", "200", "key", "F10", "sleep", "100", "key", "v", "sleep", "100", "key", "Return", "sleep", "200", "close"];
    let (code, out) = on_x_with("menus", &["--log"], "NLE menus", &steps).expect("ran");
    assert_eq!(code, 0);
    let got: Vec<&str> = out.lines().map(str::trim).collect();
    assert_eq!(got, ["INIT", "CMD 101 New", "INIT", "CMD 108 Word wrap"], "{out}");
    // the open menu is a window of its own, drawn below the bar
    let img = Image::from_ppm(&ppm);
    assert!(img.dark_in(110, 150, 160, 168) > 15, "the File menu's first item is on the screen");
}
