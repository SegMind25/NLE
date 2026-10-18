//! Compatibility analysis: which of an executable's imports the native layer implements,
//! and which backend should run it.

use crate::pe::{self, Import, ImportName, Layout, Pe};
use crate::registry;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Every import is implemented natively.
    Native,
    /// Some imports are missing; may still run if they are never called.
    Partial,
    /// Cannot be run by the native layer at all (32-bit, .NET, ARM64, DLL...).
    Unsupported,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Native => "native",
            Verdict::Partial => "partial",
            Verdict::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Resolved,
    Unresolved,
    /// A DLL shipped with the application (found next to the executable).
    Bundled,
    /// Not a Windows system DLL and not found next to the executable.
    MissingDll,
}

#[derive(Clone, Debug)]
pub struct ImportReport {
    pub dll: String,
    pub name: String,
    pub delay: bool,
    pub status: Status,
}

#[derive(Clone, Debug, Default)]
pub struct LibSummary {
    pub resolved: usize,
    pub unresolved: usize,
}

#[derive(Debug)]
pub struct Report {
    pub arch: &'static str,
    pub subsystem: &'static str,
    pub image_base: u64,
    pub entry_rva: u32,
    pub is_dll: bool,
    pub is_clr: bool,
    pub native_arch: bool,
    pub has_tls: bool,
    pub has_exceptions: bool,
    pub has_relocs: bool,
    pub sections: Vec<(String, u32, u32, u32)>,
    pub imports: Vec<ImportReport>,
    pub libs: BTreeMap<String, LibSummary>,
    pub bundled: Vec<PathBuf>,
    pub notes: Vec<String>,
    pub verdict: Verdict,
}

impl Report {
    pub fn unresolved(&self) -> Vec<&ImportReport> {
        self.imports.iter().filter(|i| i.status == Status::Unresolved || i.status == Status::MissingDll).collect()
    }
}

/// Case-insensitive file lookup in a single directory (Windows file systems are case-insensitive).
pub fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }
    let lower = name.to_ascii_lowercase();
    std::fs::read_dir(dir).ok()?.flatten().find(|e| e.file_name().to_string_lossy().to_ascii_lowercase() == lower).map(|e| e.path())
}

fn import_status(imp: &Import, exe_dir: Option<&Path>, bundled: &mut Vec<PathBuf>) -> Status {
    if registry::is_system_dll(&imp.dll) {
        let ok = match &imp.name {
            ImportName::Name { name, .. } => registry::resolve(&imp.dll, name).is_some(),
            ImportName::Ordinal(_) => false,
        };
        return if ok { Status::Resolved } else { Status::Unresolved };
    }
    if let Some(dir) = exe_dir {
        if let Some(p) = find_ci(dir, &imp.dll) {
            if !bundled.contains(&p) {
                bundled.push(p);
            }
            return Status::Bundled;
        }
    }
    Status::MissingDll
}

fn collect_imports(
    pe: &Pe,
    exe_dir: Option<&Path>,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<ImportReport>,
    bundled: &mut Vec<PathBuf>,
    notes: &mut Vec<String>,
) {
    let before = bundled.len();
    for imp in pe.imports() {
        let status = import_status(&imp, exe_dir, bundled);
        let name = match &imp.name {
            ImportName::Name { name, .. } => name.clone(),
            ImportName::Ordinal(o) => format!("#{o}"),
        };
        if status != Status::Bundled {
            out.push(ImportReport { dll: imp.dll.clone(), name, delay: imp.delay, status });
        }
    }
    let new: Vec<PathBuf> = bundled[before..].to_vec();
    for path in new {
        if !seen.insert(path.clone()) {
            continue;
        }
        match std::fs::read(&path) {
            Ok(bytes) => match Pe::parse(&bytes, Layout::File) {
                Ok(dep) => {
                    if dep.machine != pe::MACHINE_AMD64 || !dep.pe32plus {
                        notes.push(format!("bundled {} is not a 64-bit x86 PE", path.display()));
                    }
                    collect_imports(&dep, exe_dir, seen, out, bundled, notes);
                }
                Err(e) => notes.push(format!("bundled {}: {e}", path.display())),
            },
            Err(e) => notes.push(format!("bundled {}: {e}", path.display())),
        }
    }
}

pub fn analyze(pe: &Pe, exe_path: Option<&Path>) -> Report {
    let exe_dir = exe_path.and_then(|p| p.parent()).map(|d| if d.as_os_str().is_empty() { Path::new(".") } else { d });
    let mut imports = Vec::new();
    let mut bundled = Vec::new();
    let mut notes = Vec::new();
    let mut seen = HashSet::new();
    collect_imports(pe, exe_dir, &mut seen, &mut imports, &mut bundled, &mut notes);

    let mut libs: BTreeMap<String, LibSummary> = BTreeMap::new();
    for i in &imports {
        let e = libs.entry(i.dll.to_ascii_lowercase()).or_default();
        if i.status == Status::Resolved {
            e.resolved += 1;
        } else {
            e.unresolved += 1;
        }
    }

    let native_arch = pe.machine == pe::MACHINE_AMD64 && pe.pe32plus;
    let mut verdict = Verdict::Native;
    if !native_arch {
        verdict = Verdict::Unsupported;
        notes.push(format!(
            "{} executables cannot run in a native x86-64 process (32-bit code needs Wine's WoW64 layer)",
            pe.arch_name()
        ));
    }
    if pe.is_clr() {
        verdict = Verdict::Unsupported;
        notes.push(".NET managed executable: needs a CLR (Mono or dotnet), not a Win32 compatibility layer".into());
    }
    if pe.is_dll() {
        verdict = Verdict::Unsupported;
        notes.push("this is a DLL, not a program; it can only be loaded by an executable".into());
    }
    if native_arch && pe.section_align < 0x1000 {
        verdict = Verdict::Unsupported;
        notes.push("section alignment below the page size is not supported".into());
    }
    let unresolved = imports.iter().filter(|i| i.status == Status::Unresolved || i.status == Status::MissingDll).count();
    if verdict == Verdict::Native && unresolved > 0 {
        verdict = Verdict::Partial;
    }
    for i in &imports {
        if i.status == Status::MissingDll {
            notes.push(format!("{} is not a Windows system DLL and was not found next to the executable", i.dll));
            break;
        }
    }
    let mut seen_missing: HashSet<String> = HashSet::new();
    for i in &imports {
        if i.status == Status::MissingDll && seen_missing.insert(i.dll.to_ascii_lowercase()) {
            notes.push(format!("missing DLL: {}", i.dll));
        }
    }
    notes.dedup();

    Report {
        arch: pe.arch_name(),
        subsystem: pe.subsystem_name(),
        image_base: pe.image_base,
        entry_rva: pe.entry_rva,
        is_dll: pe.is_dll(),
        is_clr: pe.is_clr(),
        native_arch,
        has_tls: pe.tls().is_some(),
        has_exceptions: pe.dirs[pe::DIR_EXCEPTION].rva != 0,
        has_relocs: pe.has_relocs(),
        sections: pe.sections.iter().map(|s| (s.name.clone(), s.vaddr, s.vsize, s.chars)).collect(),
        imports,
        libs,
        bundled,
        notes,
        verdict,
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn to_json(path: &str, r: &Report) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!("  \"file\": {},\n", json_str(path)));
    s.push_str(&format!("  \"arch\": {},\n", json_str(r.arch)));
    s.push_str(&format!("  \"subsystem\": {},\n", json_str(r.subsystem)));
    s.push_str(&format!("  \"image_base\": \"0x{:x}\",\n", r.image_base));
    s.push_str(&format!("  \"verdict\": {},\n", json_str(r.verdict.as_str())));
    s.push_str("  \"libraries\": {\n");
    let n = r.libs.len();
    for (i, (dll, l)) in r.libs.iter().enumerate() {
        s.push_str(&format!(
            "    {}: {{\"resolved\": {}, \"unresolved\": {}}}{}\n",
            json_str(dll),
            l.resolved,
            l.unresolved,
            if i + 1 < n { "," } else { "" }
        ));
    }
    s.push_str("  },\n  \"unresolved\": [");
    let un = r.unresolved();
    for (i, u) in un.iter().enumerate() {
        s.push_str(&format!("{}{}", if i > 0 { ", " } else { "" }, json_str(&format!("{}!{}", u.dll, u.name))));
    }
    s.push_str("],\n  \"notes\": [");
    for (i, nn) in r.notes.iter().enumerate() {
        s.push_str(&format!("{}{}", if i > 0 { ", " } else { "" }, json_str(nn)));
    }
    s.push_str("]\n}\n");
    s
}

pub fn print_report(path: &str, r: &Report, verbose: bool) {
    println!("{path}");
    println!("  format      PE32+ {} ({})", r.arch, r.subsystem);
    println!("  image base  0x{:x}, entry RVA 0x{:x}", r.image_base, r.entry_rva);
    let mut feats = Vec::new();
    if r.has_tls {
        feats.push("TLS");
    }
    if r.has_exceptions {
        feats.push("SEH tables");
    }
    if r.has_relocs {
        feats.push("relocatable");
    }
    if !feats.is_empty() {
        println!("  features    {}", feats.join(", "));
    }
    println!("  sections    {}", r.sections.iter().map(|s| s.0.as_str()).collect::<Vec<_>>().join(" "));
    println!("  imports");
    for (dll, l) in &r.libs {
        let total = l.resolved + l.unresolved;
        let mark = if l.unresolved == 0 { "ok " } else { "!! " };
        println!("    {mark}{dll:<34} {}/{} implemented", l.resolved, total);
    }
    if !r.bundled.is_empty() {
        for b in &r.bundled {
            println!("    ok {:<34} bundled with the application", b.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
    }
    let un = r.unresolved();
    if !un.is_empty() {
        println!("  not implemented ({}):", un.len());
        let limit = if verbose { usize::MAX } else { 25 };
        for u in un.iter().take(limit) {
            println!("    {}!{}{}", u.dll, u.name, if u.delay { "  (delay-load)" } else { "" });
        }
        if un.len() > limit {
            println!("    ... and {} more (use --verbose)", un.len() - limit);
        }
    }
    for n in &r.notes {
        println!("  note: {n}");
    }
    let summary = match r.verdict {
        Verdict::Native => "native: every import is implemented, runs without Wine",
        Verdict::Partial => "partial: runs natively until it calls a missing API; Wine recommended if installed",
        Verdict::Unsupported => "unsupported by the native layer: use the Wine backend",
    };
    println!("  verdict     {summary}");
}
