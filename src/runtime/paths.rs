//! Windows <-> host path translation.
//!
//! `C:` lives in `<prefix>/drive_c`, other drive letters in `<prefix>/drive_<x>`, and `Z:` is the
//! host root (the same convention Wine uses), so any Linux file is reachable as `Z:\...`.
//! Windows file systems are case-insensitive: lookups that miss fall back to a per-component
//! case-insensitive directory scan.

use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

static PREFIX: OnceLock<PathBuf> = OnceLock::new();

pub fn default_prefix() -> PathBuf {
    if let Ok(p) = std::env::var("NLE_PREFIX") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(x) = std::env::var("XDG_DATA_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("nle/prefix");
        }
    }
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h).join(".local/share/nle/prefix"),
        _ => PathBuf::from(format!("/tmp/nle-prefix-{}", unsafe { libc::getuid() })),
    }
}

/// Creates the standard Windows directory skeleton. Falls back to /tmp if `prefix` is not writable.
pub fn init_prefix(prefix: PathBuf) -> PathBuf {
    let user = super::user_name();
    let make = |root: &Path| -> std::io::Result<()> {
        let c = root.join("drive_c");
        for d in [
            "windows/system32",
            "windows/temp",
            "Program Files",
            "Program Files (x86)",
            "ProgramData",
        ] {
            std::fs::create_dir_all(c.join(d))?;
        }
        for d in ["AppData/Local/Temp", "AppData/Roaming", "Documents", "Desktop"] {
            std::fs::create_dir_all(c.join("users").join(&user).join(d))?;
        }
        Ok(())
    };
    let chosen = if make(&prefix).is_ok() {
        prefix
    } else {
        let fallback = PathBuf::from(format!("/tmp/nle-prefix-{}", unsafe { libc::getuid() }));
        let _ = make(&fallback);
        fallback
    };
    PREFIX.get_or_init(|| chosen).clone()
}

pub fn prefix() -> &'static Path {
    PREFIX.get_or_init(default_prefix)
}

fn drive_dir(letter: char) -> PathBuf {
    match letter.to_ascii_lowercase() {
        'z' => PathBuf::from("/"),
        l => prefix().join(format!("drive_{l}")),
    }
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

fn drive_of_host(p: &Path) -> char {
    for l in 'a'..='y' {
        if p.starts_with(drive_dir(l)) && l != 'z' {
            return l;
        }
    }
    'z'
}

fn strip_device_prefix(s: &str) -> &str {
    for pre in ["\\\\?\\", "\\\\.\\", "\\??\\", "//?/", "//./"] {
        if let Some(r) = s.strip_prefix(pre) {
            return r;
        }
    }
    s
}

pub fn has_drive(s: &str) -> Option<char> {
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        Some(b[0].to_ascii_lowercase() as char)
    } else {
        None
    }
}

fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// Converts a Windows path into a host path string (no case-insensitive lookup).
pub fn win_to_host_str(win: &str) -> String {
    let s = strip_device_prefix(win);
    let slashes = |t: &str| t.replace('\\', "/");
    if let Some(letter) = has_drive(s) {
        let rest = &s[2..];
        let base = drive_dir(letter);
        if rest.is_empty() || rest.starts_with(is_sep) {
            let r = slashes(rest);
            let r = r.trim_start_matches('/');
            return if r.is_empty() { base.to_string_lossy().into_owned() } else { base.join(r).to_string_lossy().into_owned() };
        }
        if drive_of_host(&cwd()) == letter {
            return slashes(rest);
        }
        return base.join(slashes(rest)).to_string_lossy().into_owned();
    }
    if s.starts_with(is_sep) {
        let base = drive_dir(drive_of_host(&cwd()));
        let r = slashes(s);
        let r = r.trim_start_matches('/');
        return if r.is_empty() { base.to_string_lossy().into_owned() } else { base.join(r).to_string_lossy().into_owned() };
    }
    if s.is_empty() {
        return ".".to_string();
    }
    slashes(s)
}

pub fn host_to_win(host: &Path) -> String {
    let abs = if host.is_absolute() { host.to_path_buf() } else { cwd().join(host) };
    let letter = drive_of_host(&abs);
    let rel = if letter == 'z' { abs.strip_prefix("/").unwrap_or(&abs).to_path_buf() } else { abs.strip_prefix(drive_dir(letter)).unwrap_or(&abs).to_path_buf() };
    let mut out = format!("{}:", letter.to_ascii_uppercase());
    let mut any = false;
    for c in rel.components() {
        if let Component::Normal(n) = c {
            out.push('\\');
            out.push_str(&n.to_string_lossy());
            any = true;
        }
    }
    if !any {
        out.push('\\');
    }
    out
}

pub fn cwd_win() -> String {
    host_to_win(&cwd())
}

pub fn is_absolute_win(s: &str) -> bool {
    let s = strip_device_prefix(s);
    matches!(has_drive(s), Some(_) if s[2..].starts_with(is_sep)) || s.starts_with("\\\\")
}

/// `GetFullPathName` semantics: absolute, `.`/`..` removed, backslash separators.
pub fn full_path(win: &str) -> String {
    if win.starts_with("\\\\?\\") {
        return win.to_string();
    }
    let s = strip_device_prefix(win);
    let cwd = cwd_win();
    let (drive, rest): (char, String) = if let Some(l) = has_drive(s) {
        let r = &s[2..];
        if r.starts_with(is_sep) {
            (l, r.to_string())
        } else if has_drive(&cwd) == Some(l) {
            (l, format!("{}\\{}", &cwd[2..], r))
        } else {
            (l, format!("\\{r}"))
        }
    } else if s.starts_with(is_sep) {
        (has_drive(&cwd).unwrap_or('c'), s.to_string())
    } else {
        (has_drive(&cwd).unwrap_or('c'), format!("{}\\{}", &cwd[2..], s))
    };
    let mut stack: Vec<&str> = Vec::new();
    for comp in rest.split(is_sep) {
        match comp {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            c => stack.push(c),
        }
    }
    let mut out = format!("{}:", drive.to_ascii_uppercase());
    if stack.is_empty() {
        out.push('\\');
    }
    for c in stack {
        out.push('\\');
        out.push_str(c);
    }
    if rest.ends_with(is_sep) && !out.ends_with('\\') {
        out.push('\\');
    }
    out
}

/// Resolves each path component case-insensitively against the host file system.
pub fn ci_resolve(p: &Path) -> PathBuf {
    if p.symlink_metadata().is_ok() {
        return p.to_path_buf();
    }
    let mut out = if p.is_absolute() { PathBuf::from("/") } else { PathBuf::new() };
    for comp in p.components() {
        match comp {
            Component::RootDir | Component::Prefix(_) => {}
            Component::CurDir => out.push("."),
            Component::ParentDir => out.push(".."),
            Component::Normal(name) => {
                let cand = out.join(name);
                if cand.symlink_metadata().is_ok() {
                    out = cand;
                    continue;
                }
                let dir = if out.as_os_str().is_empty() { Path::new(".") } else { out.as_path() };
                let lname = name.to_string_lossy().to_lowercase();
                let found = std::fs::read_dir(dir)
                    .ok()
                    .and_then(|rd| rd.flatten().find(|e| e.file_name().to_string_lossy().to_lowercase() == lname).map(|e| e.file_name()));
                match found {
                    Some(f) => out.push(f),
                    None => out.push(name),
                }
            }
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Windows path -> existing-or-creatable host path.
pub fn resolve(win: &str) -> PathBuf {
    let p = ci_resolve(Path::new(&win_to_host_str(win)));
    let cfg = crate::runtime::cfg();
    match &cfg.elf_alias {
        Some(elf) if p == cfg.exe_host && p.symlink_metadata().is_err() => elf.clone(),
        _ => p,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Device {
    Null,
    Con,
    ConIn,
    ConOut,
}

pub fn device(win: &str) -> Option<Device> {
    let s = strip_device_prefix(win);
    let name = if std::ptr::eq(s.as_ptr(), win.as_ptr()) {
        s.rsplit(is_sep).next().unwrap_or(s)
    } else {
        s
    };
    let name = name.split('.').next().unwrap_or(name);
    match name.to_ascii_uppercase().as_str() {
        "NUL" => Some(Device::Null),
        "CON" => Some(Device::Con),
        "CONIN$" => Some(Device::ConIn),
        "CONOUT$" => Some(Device::ConOut),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() {
        let _ = PREFIX.set(PathBuf::from("/tmp/nle-test-prefix"));
    }

    #[test]
    fn drive_mapping() {
        setup();
        assert_eq!(win_to_host_str("C:\\Windows\\System32"), "/tmp/nle-test-prefix/drive_c/Windows/System32");
        assert_eq!(win_to_host_str("Z:\\usr\\bin"), "/usr/bin");
        assert_eq!(win_to_host_str("Z:\\"), "/");
        assert_eq!(win_to_host_str("\\\\?\\Z:\\tmp"), "/tmp");
        assert_eq!(win_to_host_str("relative\\dir\\f.txt"), "relative/dir/f.txt");
    }

    #[test]
    fn host_to_windows() {
        setup();
        assert_eq!(host_to_win(Path::new("/usr/bin")), "Z:\\usr\\bin");
        assert_eq!(host_to_win(Path::new("/")), "Z:\\");
        assert_eq!(host_to_win(Path::new("/tmp/nle-test-prefix/drive_c/users/x")), "C:\\users\\x");
    }

    #[test]
    fn full_path_normalises() {
        setup();
        assert_eq!(full_path("C:\\a\\b\\..\\c\\.\\d"), "C:\\a\\c\\d");
        assert_eq!(full_path("C:/a//b"), "C:\\a\\b");
        assert_eq!(full_path("C:\\"), "C:\\");
        assert_eq!(full_path("\\\\?\\C:\\a\\..\\b"), "\\\\?\\C:\\a\\..\\b");
    }

    #[test]
    fn devices() {
        assert_eq!(device("NUL"), Some(Device::Null));
        assert_eq!(device("\\\\.\\CONOUT$"), Some(Device::ConOut));
        assert_eq!(device("nul.txt"), Some(Device::Null));
        assert_eq!(device("C:\\data\\file.txt"), None);
    }

    #[test]
    fn case_insensitive_lookup() {
        let dir = std::env::temp_dir().join(format!("nle-ci-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("SubDir")).unwrap();
        std::fs::write(dir.join("SubDir/Hello.TXT"), b"x").unwrap();
        let got = ci_resolve(&dir.join("subdir/hello.txt"));
        assert_eq!(got, dir.join("SubDir/Hello.TXT"));
        let missing = ci_resolve(&dir.join("subdir/new.txt"));
        assert_eq!(missing, dir.join("SubDir/new.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
