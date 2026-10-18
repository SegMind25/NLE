//! Process-wide runtime state shared by the loader and the Win32/CRT shims.

pub mod handles;
pub mod paths;
pub mod strings;
pub mod teb;
pub mod trap;

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Default, Debug)]
pub struct Trace {
    pub load: bool,
    pub bind: bool,
    pub api: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    /// Root of the emulated Windows directory tree (`drive_c/` lives below it).
    pub prefix: PathBuf,
    /// Host path of the main executable.
    pub exe_host: PathBuf,
    /// Windows-style path of the main executable, as the program sees it.
    pub exe_win: String,
    /// Arguments after the program name.
    pub args: Vec<String>,
    pub trace: Trace,
    /// Refuse to start when any import is unresolved.
    pub strict: bool,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn init(cfg: Config) {
    let _ = CONFIG.set(cfg);
}

pub fn cfg() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

pub fn trace_from_env() -> Trace {
    let mut t = Trace::default();
    if let Ok(v) = std::env::var("ANYEXE_TRACE") {
        for item in v.split(',') {
            match item.trim() {
                "load" => t.load = true,
                "bind" => t.bind = true,
                "api" => t.api = true,
                "all" => {
                    t.load = true;
                    t.bind = true;
                    t.api = true
                }
                _ => {}
            }
        }
    }
    t
}

#[macro_export]
macro_rules! trace_load {
    ($($arg:tt)*) => { if $crate::runtime::cfg().trace.load { eprintln!("anyexe[load]: {}", format_args!($($arg)*)); } };
}

#[macro_export]
macro_rules! trace_bind {
    ($($arg:tt)*) => { if $crate::runtime::cfg().trace.bind { eprintln!("anyexe[bind]: {}", format_args!($($arg)*)); } };
}

/// The environment block as the Windows program sees it (separate from the host's).
pub static ENV: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

pub fn user_name() -> String {
    std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_else(|_| "user".to_string())
}

pub fn host_name() -> String {
    let mut buf = [0u8; 256];
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len() - 1) } == 0;
    if ok {
        let n = buf.iter().position(|&b| b == 0).unwrap_or(0);
        let s = String::from_utf8_lossy(&buf[..n]).into_owned();
        if !s.is_empty() {
            return s;
        }
    }
    "localhost".to_string()
}

pub fn cpu_count() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

/// Builds the Windows-view environment from the host environment plus Windows defaults.
pub fn init_env() {
    let user = user_name();
    let mut env: Vec<(String, String)> = Vec::new();
    for (k, v) in std::env::vars() {
        if k == "PATH" || k.starts_with("ANYEXE_") || k.contains('=') || k.is_empty() {
            continue;
        }
        env.push((k, v));
    }
    let defaults: Vec<(String, String)> = vec![
        ("PATH".into(), "C:\\windows\\system32;C:\\windows".into()),
        ("SystemRoot".into(), "C:\\windows".into()),
        ("windir".into(), "C:\\windows".into()),
        ("SystemDrive".into(), "C:".into()),
        ("OS".into(), "Windows_NT".into()),
        ("COMSPEC".into(), "C:\\windows\\system32\\cmd.exe".into()),
        ("PATHEXT".into(), ".COM;.EXE;.BAT;.CMD".into()),
        ("PROCESSOR_ARCHITECTURE".into(), "AMD64".into()),
        ("PROCESSOR_IDENTIFIER".into(), "Intel64 Family 6 Model 85 Stepping 7, GenuineIntel".into()),
        ("NUMBER_OF_PROCESSORS".into(), cpu_count().to_string()),
        ("USERNAME".into(), user.clone()),
        ("COMPUTERNAME".into(), host_name().to_ascii_uppercase()),
        ("USERPROFILE".into(), format!("C:\\users\\{user}")),
        ("HOMEDRIVE".into(), "C:".into()),
        ("HOMEPATH".into(), format!("\\users\\{user}")),
        ("APPDATA".into(), format!("C:\\users\\{user}\\AppData\\Roaming")),
        ("LOCALAPPDATA".into(), format!("C:\\users\\{user}\\AppData\\Local")),
        ("TEMP".into(), format!("C:\\users\\{user}\\AppData\\Local\\Temp")),
        ("TMP".into(), format!("C:\\users\\{user}\\AppData\\Local\\Temp")),
        ("ProgramFiles".into(), "C:\\Program Files".into()),
        ("ProgramData".into(), "C:\\ProgramData".into()),
        ("ALLUSERSPROFILE".into(), "C:\\ProgramData".into()),
    ];
    for (k, v) in defaults {
        if k == "PATH" {
            env.push((k, v));
            continue;
        }
        if !env.iter().any(|(ek, _)| ek.eq_ignore_ascii_case(&k)) {
            env.push((k, v));
        }
    }
    *ENV.lock().unwrap() = env;
}

pub fn env_get(name: &str) -> Option<String> {
    ENV.lock().unwrap().iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
}

pub fn env_set(name: &str, value: Option<&str>) {
    let mut env = ENV.lock().unwrap();
    env.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    if let Some(v) = value {
        env.push((name.to_string(), v.to_string()));
    }
}

/// Quotes one argument following the Microsoft C runtime parsing rules.
pub fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '\n', '\x0b', '"']) {
        return a.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in a.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                out.push(c);
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// The command line string a Windows process sees (`GetCommandLineW`).
pub fn command_line() -> String {
    let c = cfg();
    let mut parts = vec![quote_arg(&c.exe_win)];
    parts.extend(c.args.iter().map(|a| quote_arg(a)));
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_matches_msvcrt_rules() {
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg("with space"), "\"with space\"");
        assert_eq!(quote_arg("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_arg("tail\\ x\\"), "\"tail\\ x\\\\\"");
        assert_eq!(quote_arg(""), "\"\"");
    }
}
