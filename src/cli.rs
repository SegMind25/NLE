//! Command line interface.

use crate::analyze::{self, Verdict};
use crate::backend::{self, Backend, Choice};
use crate::launch::{self, RunOptions};
use crate::pe::{Layout, Pe};
use crate::relink;
use crate::registry;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
NLE: run Windows .exe files as native Linux programs (no emulation).

usage:
  nle [run] [options] <app.exe> [args...]   run a Windows program
  nle inspect [--json] [-v] <app.exe>       compatibility report
  nle relink <app.exe> [-o DIR] [options]   convert to a native Linux ELF
  nle coverage [--markdown|--json] [LIB]    list the implemented Windows APIs
  nle doctor                                check this system
  nle binfmt [status|install|remove]        let the kernel run .exe files directly
  nle --version | --help

run options:
  --backend auto|native|wine   auto (default) uses the native layer, or Wine when the
                               program needs APIs we do not implement and Wine exists
  --prefix DIR                 root of the emulated C:\\ drive
  --strict                     refuse to start if any import is unimplemented
  -v                           explain which backend was chosen

relink options:
  -o DIR                 output directory (default: <name>-linux)
  --name NAME            name of the produced executable (default: the exe's stem)
  --keep-original        also copy the original .exe next to the result
  --no-copy-runtime      do not copy libnle.so (use --rpath to point at it)
  --rpath DIR            DT_RUNPATH for libnle.so (default: $ORIGIN)
  --strict               fail if any import is unimplemented

environment:
  NLE_PREFIX   emulated Windows directory tree     NLE_WINE   path to wine
  NLE_TRACE    comma list of: load, bind, api, all (debug logging)
";

fn load_pe(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}"))
}

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(first) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    match first.as_str() {
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        "-V" | "--version" | "version" => {
            println!("nle {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "run" => cmd_run(&args[1..]),
        "inspect" => cmd_inspect(&args[1..]),
        "relink" => cmd_relink(&args[1..]),
        "coverage" => cmd_coverage(&args[1..]),
        "doctor" => cmd_doctor(),
        "binfmt" => cmd_binfmt(&args[1..]),
        _ => cmd_run(&args),
    }
}

fn cmd_run(args: &[String]) -> ExitCode {
    let mut opts = RunOptions::default();
    let mut backend = Backend::Auto;
    let mut verbose = false;
    let mut rest = args;
    while let Some(a) = rest.first() {
        match a.as_str() {
            "--strict" => opts.strict = true,
            "-v" | "--verbose" => verbose = true,
            "--backend" if rest.len() > 1 => {
                let Some(b) = Backend::parse(&rest[1]) else {
                    eprintln!("nle: unknown backend `{}` (use auto, native or wine)", rest[1]);
                    return ExitCode::from(2);
                };
                backend = b;
                rest = &rest[1..];
            }
            "--prefix" if rest.len() > 1 => {
                opts.prefix = Some(PathBuf::from(&rest[1]));
                rest = &rest[1..];
            }
            _ => break,
        }
        rest = &rest[1..];
    }
    let Some(exe) = rest.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let prog_args = rest[1..].to_vec();
    let bytes = match load_pe(exe) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("nle: {e}");
            return ExitCode::from(127);
        }
    };
    let pe = match Pe::parse(&bytes, Layout::File) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("nle: {exe}: {e}");
            return ExitCode::from(126);
        }
    };
    let report = analyze::analyze(&pe, Some(Path::new(exe)));
    let wine = backend::find_wine();
    let chosen = match backend {
        Backend::Native => Choice::Native,
        Backend::Wine => Choice::Wine("requested with --backend wine"),
        Backend::Auto => backend::choose(&report, wine.is_some()),
    };
    match chosen {
        Choice::Refuse(why) => {
            eprintln!("nle: {exe}: {why}");
            for n in &report.notes {
                eprintln!("  note: {n}");
            }
            ExitCode::from(126)
        }
        Choice::Wine(why) => {
            let Some(w) = wine else {
                eprintln!("nle: Wine was requested but is not installed (set NLE_WINE to its path)");
                return ExitCode::from(127);
            };
            if verbose || backend == Backend::Auto {
                eprintln!("nle: {why}");
            }
            let e = backend::exec_wine(&w, Path::new(exe), &prog_args, opts.prefix.as_deref());
            eprintln!("nle: cannot start {}: {e}", w.display());
            ExitCode::from(127)
        }
        Choice::Native | Choice::NativeWithWarning(_) => {
            if let Choice::NativeWithWarning(w) = chosen {
                eprintln!("nle: warning: {w}");
            } else if verbose {
                eprintln!("nle: running natively ({} imports implemented)", report.imports.len());
            }
            if report.verdict == Verdict::Unsupported && backend == Backend::Native {
                eprintln!("nle: warning: this executable is outside what the native layer supports");
                for n in &report.notes {
                    eprintln!("  note: {n}");
                }
            }
            drop(bytes);
            let err = launch::run_native(Path::new(exe), prog_args, &opts);
            eprintln!("nle: {exe}: {err}");
            ExitCode::from(126)
        }
    }
}

fn cmd_inspect(args: &[String]) -> ExitCode {
    let (mut json, mut verbose, mut path) = (false, false, None);
    for a in args {
        match a.as_str() {
            "--json" => json = true,
            "-v" | "--verbose" => verbose = true,
            p => path = Some(p.to_string()),
        }
    }
    let Some(path) = path else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let bytes = match load_pe(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("nle: {e}");
            return ExitCode::from(1);
        }
    };
    let pe = match Pe::parse(&bytes, Layout::File) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("nle: {path}: {e}");
            return ExitCode::from(1);
        }
    };
    let report = analyze::analyze(&pe, Some(Path::new(&path)));
    if json {
        print!("{}", analyze::to_json(&path, &report));
    } else {
        analyze::print_report(&path, &report, verbose);
    }
    ExitCode::SUCCESS
}

/// Locates `libnle.so` for the relinker.
pub fn runtime_lib() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NLE_LIB") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("libnle.so"), dir.join("../lib/nle/libnle.so"), dir.join("../lib/libnle.so")]
        .into_iter()
        .find(|p| p.is_file())
}

fn cmd_relink(args: &[String]) -> ExitCode {
    let (mut out, mut name, mut keep, mut copy_rt, mut rpath, mut strict) = (None::<PathBuf>, None::<String>, false, true, "$ORIGIN".to_string(), false);
    let mut input = None::<String>;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" if i + 1 < args.len() => {
                out = Some(PathBuf::from(&args[i + 1]));
                i += 1;
            }
            "--name" if i + 1 < args.len() => {
                name = Some(args[i + 1].clone());
                i += 1;
            }
            "--rpath" if i + 1 < args.len() => {
                rpath = args[i + 1].clone();
                i += 1;
            }
            "--keep-original" => keep = true,
            "--no-copy-runtime" => copy_rt = false,
            "--strict" => strict = true,
            p if !p.starts_with('-') => input = Some(p.to_string()),
            other => {
                eprintln!("nle: unknown option `{other}`");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let Some(input) = input else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let bytes = match load_pe(&input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("nle: {e}");
            return ExitCode::from(1);
        }
    };
    let pe = match relink::parse_for_relink(&bytes) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("nle: {input}: {e}");
            return ExitCode::from(1);
        }
    };
    let report = analyze::analyze(&pe, Some(Path::new(&input)));
    let unresolved = report.unresolved();
    if strict && !unresolved.is_empty() {
        eprintln!("nle: {} imports are not implemented:", unresolved.len());
        for u in unresolved.iter().take(20) {
            eprintln!("  {}!{}", u.dll, u.name);
        }
        return ExitCode::from(1);
    }
    let in_path = Path::new(&input);
    let stem = in_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "app".into());
    let exe_name = name.unwrap_or_else(|| stem.clone());
    let out_dir = out.unwrap_or_else(|| PathBuf::from(format!("{exe_name}-linux")));
    let self_exe = std::env::current_exe().unwrap_or_default();
    let interp = relink::interpreter_of(&self_exe).unwrap_or_else(|| "/lib64/ld-linux-x86-64.so.2".to_string());
    let original = in_path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| format!("{stem}.exe"));
    let elf = match relink::build(&pe, &bytes, &interp, &relink::Options { runpath: rpath.clone(), original_name: original.clone() }) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("nle: {input}: {e}");
            return ExitCode::from(1);
        }
    };
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("nle: {}: {e}", out_dir.display());
        return ExitCode::from(1);
    }
    let target = out_dir.join(&exe_name);
    if let Err(e) = write_executable(&target, &elf) {
        eprintln!("nle: {}: {e}", target.display());
        return ExitCode::from(1);
    }
    println!("{} {}  ({} KiB native ELF)", crate::ui::green("created  "), target.display(), elf.len() / 1024);
    if copy_rt {
        match runtime_lib() {
            Some(lib) => {
                let dst = out_dir.join("libnle.so");
                match std::fs::copy(&lib, &dst) {
                    Ok(_) => println!("{} {}", crate::ui::cyan("copied   "), dst.display()),
                    Err(e) => eprintln!("nle: cannot copy runtime library: {e}"),
                }
            }
            None => eprintln!("nle: warning: libnle.so not found next to the nle binary; set NLE_LIB or pass --rpath"),
        }
    }
    let spawns = report.imports.iter().any(|i| matches!(i.name.as_str(), "CreateProcessA" | "CreateProcessW" | "WinExec" | "ShellExecuteW" | "ShellExecuteExW"));
    if spawns && copy_rt && !self_exe.as_os_str().is_empty() {
        // Windows children are started through the launcher, which is searched for next to libnle.so.
        let dst = out_dir.join("nle");
        match std::fs::copy(&self_exe, &dst) {
            Ok(_) => println!("{} {}  (starts Windows child processes)", crate::ui::cyan("copied   "), dst.display()),
            Err(e) => eprintln!("nle: cannot copy the launcher: {e}"),
        }
    }
    for dll in &report.bundled {
        if let Some(n) = dll.file_name() {
            let dst = out_dir.join(n);
            if std::fs::copy(dll, &dst).is_ok() {
                println!("{} {}", crate::ui::cyan("copied   "), dst.display());
            }
        }
    }
    if keep {
        let dst = out_dir.join(&original);
        if std::fs::copy(in_path, &dst).is_ok() {
            println!("{} {}", crate::ui::cyan("copied   "), dst.display());
        }
    }
    let total = report.imports.len();
    println!("{} {} of {} implemented natively", crate::ui::dim("imports  "), total - unresolved.len(), total);
    if !unresolved.is_empty() {
        println!("          {} missing (the program stops with a clear message if it calls one); see `nle inspect`", unresolved.len());
    }
    println!("{} {}", crate::ui::bold("run it   "), target.display());
    ExitCode::SUCCESS
}

fn write_executable(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, data)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

fn cmd_coverage(args: &[String]) -> ExitCode {
    let (mut markdown, mut json, mut only) = (false, false, None::<String>);
    for a in args {
        match a.as_str() {
            "--markdown" => markdown = true,
            "--json" => json = true,
            lib => only = Some(lib.to_ascii_lowercase()),
        }
    }
    let all = registry::all_names();
    let mut families: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for (f, n) in &all {
        families.entry(f).or_default().push(n);
    }
    if json {
        let mut s = String::from("{\n");
        let n = families.len();
        for (i, (f, names)) in families.iter().enumerate() {
            s.push_str(&format!("  \"{f}\": {}{}\n", names.len(), if i + 1 < n { "," } else { "" }));
        }
        s.push_str("}\n");
        print!("{s}");
        return ExitCode::SUCCESS;
    }
    if markdown {
        println!("# Implemented Windows APIs\n");
        println!("Generated by `nle coverage --markdown`. {} functions in {} libraries.\n", all.len(), families.len());
        println!("| library | functions |\n| --- | ---: |");
        for (f, names) in &families {
            println!("| {f} | {} |", names.len());
        }
        for (f, names) in &families {
            println!("\n## {f} ({})\n", names.len());
            println!("{}", names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(" "));
        }
        return ExitCode::SUCCESS;
    }
    match only {
        Some(lib) => {
            let lib = registry::family_of(&lib).unwrap_or("unknown");
            match families.get(lib) {
                Some(names) => {
                    for n in names {
                        println!("{n}");
                    }
                }
                None => {
                    eprintln!("nle: no library named `{lib}`");
                    return ExitCode::from(1);
                }
            }
        }
        None => {
            println!("{:<14} {:>9}", "library", "functions");
            for (f, names) in &families {
                println!("{f:<14} {:>9}", names.len());
            }
            println!("{:<14} {:>9}", "total", all.len());
        }
    }
    ExitCode::SUCCESS
}

fn report_line(mark: &str, label: &str, detail: &str) {
    println!("  [{mark}] {label}{}", if detail.is_empty() { String::new() } else { format!(": {detail}") });
}

/// A required capability: `!!` when it is missing.
fn check(ok: bool, label: &str, detail: &str) -> bool {
    report_line(&if ok { crate::ui::green("ok") } else { crate::ui::red("!!") }, label, detail);
    ok
}

/// An optional capability: `--` when it is missing.
fn optional(ok: bool, label: &str, detail: &str) {
    report_line(&if ok { crate::ui::green("ok") } else { crate::ui::dim("--") }, label, detail);
}

fn cmd_doctor() -> ExitCode {
    println!("NLE {} on {}", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH);
    let mut all_ok = true;
    all_ok &= check(cfg!(target_arch = "x86_64"), "x86-64 CPU", "Windows x64 code runs natively, so the host must be x86-64");
    unsafe {
        let p = libc::mmap(0x1_4000_0000usize as *mut _, 0x10000, libc::PROT_NONE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED_NOREPLACE, -1, 0);
        let ok = p as usize == 0x1_4000_0000;
        if ok {
            libc::munmap(p, 0x10000);
        }
        all_ok &= check(ok, "address 0x140000000 is mappable", "the usual Windows executable base");
        let k = libc::mmap(0x7ffe_0000usize as *mut _, 4096, libc::PROT_NONE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED_NOREPLACE, -1, 0);
        let ok = k as usize == 0x7ffe_0000;
        if ok {
            libc::munmap(k, 4096);
        }
        optional(ok, "address 0x7FFE0000 is mappable", "KUSER_SHARED_DATA, needed by some runtimes (Go)");
        let t = std::thread::spawn(|| {
            let mem = libc::mmap(std::ptr::null_mut(), 4096, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) as *mut u64;
            *mem.add(6) = mem as u64;
            let r = libc::syscall(libc::SYS_arch_prctl, 0x1001 as libc::c_long, mem as libc::c_long);
            let mut got: u64;
            std::arch::asm!("mov {}, gs:[0x30]", out(reg) got);
            libc::syscall(libc::SYS_arch_prctl, 0x1001 as libc::c_long, 0 as libc::c_long);
            r == 0 && got == mem as u64
        });
        all_ok &= check(t.join().unwrap_or(false), "thread block via the gs register", "how Windows code finds its TEB");
        let (cs, ss): (u16, u16);
        std::arch::asm!("mov {0:x}, cs", "mov {1:x}, ss", out(reg) cs, out(reg) ss);
        all_ok &= check(cs == 0x33 && ss == 0x2b, "user segment selectors", &format!("cs={cs:#x} ss={ss:#x}; exception recovery assumes 0x33/0x2b"));
    }
    let maps = std::fs::read_to_string("/proc/self/maps").is_ok();
    all_ok &= check(maps, "/proc/self/maps readable", "used by VirtualQuery");
    let prefix = crate::runtime::paths::default_prefix();
    let writable = std::fs::create_dir_all(&prefix).is_ok();
    all_ok &= check(writable, "Windows prefix directory", &prefix.display().to_string());
    match runtime_lib() {
        Some(p) => {
            optional(true, "libnle.so (needed by `relink`)", &p.display().to_string());
        }
        None => {
            optional(false, "libnle.so (needed by `relink`)", "not found next to the nle binary");
        }
    }
    match backend::find_wine() {
        Some(w) => {
            optional(true, "Wine fallback", &w.display().to_string());
        }
        None => {
            optional(false, "Wine fallback", "not installed; only matters for programs the native layer cannot run");
        }
    }
    let bf = std::fs::read_to_string("/proc/sys/fs/binfmt_misc/nle").is_ok();
    optional(bf, "binfmt_misc handler for .exe", if bf { "registered" } else { "not registered (optional, see `nle binfmt`)" });
    println!("{}", if all_ok { "everything needed for native execution is available" } else { "some required checks failed" });
    if all_ok { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

fn cmd_binfmt(args: &[String]) -> ExitCode {
    let register = "/proc/sys/fs/binfmt_misc/register";
    let entry = "/proc/sys/fs/binfmt_misc/nle";
    let sub = args.first().map(String::as_str).unwrap_or("status");
    let me = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/usr/local/bin/nle"));
    match sub {
        "status" => {
            match std::fs::read_to_string(entry) {
                Ok(t) => print!("registered:\n{t}"),
                Err(_) => println!("not registered. `sudo nle binfmt install` lets you run ./program.exe directly."),
            }
            ExitCode::SUCCESS
        }
        "install" => {
            let line = format!(":nle:M::MZ::{}:", me.display());
            if std::fs::metadata("/proc/sys/fs/binfmt_misc/register").is_err() {
                println!("binfmt_misc is not mounted. Run: sudo mount -t binfmt_misc binfmt_misc /proc/sys/fs/binfmt_misc");
                return ExitCode::from(1);
            }
            match std::fs::write(register, line.as_bytes()) {
                Ok(()) => {
                    println!("registered: the kernel now runs Windows executables through {}", me.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("nle: cannot register ({e}). Run as root, for example:");
                    eprintln!("  sudo sh -c 'echo \"{line}\" > {register}'");
                    ExitCode::from(1)
                }
            }
        }
        "remove" => match std::fs::write(entry, b"-1") {
            Ok(()) => {
                println!("binfmt handler removed");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("nle: cannot remove ({e}). Run as root: sudo sh -c 'echo -1 > {entry}'");
                ExitCode::from(1)
            }
        },
        other => {
            eprintln!("nle: unknown binfmt command `{other}` (status, install, remove)");
            ExitCode::from(2)
        }
    }
}
