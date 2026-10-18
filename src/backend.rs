//! Choosing between the native layer and Wine.

use crate::analyze::{Report, Verdict};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    Auto,
    Native,
    Wine,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Backend> {
        match s {
            "auto" => Some(Backend::Auto),
            "native" => Some(Backend::Native),
            "wine" => Some(Backend::Wine),
            _ => None,
        }
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Locates a Wine launcher.
pub fn find_wine() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ANYEXE_WINE") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    ["wine64", "wine", "wine-stable", "wine-development"].iter().find_map(|n| which(n))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    Native,
    Wine(&'static str),
    NativeWithWarning(&'static str),
    Refuse(&'static str),
}

/// Decides what to do for `--backend auto`.
pub fn choose(report: &Report, wine_available: bool) -> Choice {
    match report.verdict {
        Verdict::Native => Choice::Native,
        Verdict::Partial => {
            if wine_available {
                Choice::Wine("some imports are not implemented natively, using Wine")
            } else {
                Choice::NativeWithWarning("some imports are not implemented natively and Wine is not installed; the program stops if it calls one")
            }
        }
        Verdict::Unsupported => {
            if wine_available {
                Choice::Wine("the native layer cannot run this kind of executable, using Wine")
            } else {
                Choice::Refuse("the native layer cannot run this executable and Wine is not installed")
            }
        }
    }
}

/// Replaces this process with `wine <exe> args...`.
pub fn exec_wine(wine: &Path, exe: &Path, args: &[String], prefix: Option<&Path>) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(wine);
    cmd.arg(exe).args(args);
    if let Some(p) = prefix {
        cmd.env("WINEPREFIX", p);
    }
    cmd.exec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::Report;
    use std::collections::BTreeMap;

    fn report(v: Verdict) -> Report {
        Report {
            arch: "x86-64",
            subsystem: "windows console",
            image_base: 0,
            entry_rva: 0,
            is_dll: false,
            is_clr: false,
            native_arch: true,
            has_tls: false,
            has_exceptions: false,
            has_relocs: false,
            sections: vec![],
            imports: vec![],
            libs: BTreeMap::new(),
            bundled: vec![],
            notes: vec![],
            verdict: v,
        }
    }

    #[test]
    fn auto_policy() {
        assert_eq!(choose(&report(Verdict::Native), true), Choice::Native);
        assert!(matches!(choose(&report(Verdict::Partial), true), Choice::Wine(_)));
        assert!(matches!(choose(&report(Verdict::Partial), false), Choice::NativeWithWarning(_)));
        assert!(matches!(choose(&report(Verdict::Unsupported), true), Choice::Wine(_)));
        assert!(matches!(choose(&report(Verdict::Unsupported), false), Choice::Refuse(_)));
    }
}
