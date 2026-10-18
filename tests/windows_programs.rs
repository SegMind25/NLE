//! Builds the C/C++ programs in `tests/exe` with MinGW-w64 and runs them twice: through
//! `anyexe run` and as native ELF files produced by `anyexe relink`.
//!
//! Every program prints `ALL OK (N checks)` and exits 0 when its checks pass. When MinGW is not
//! installed the tests print a notice and pass, so the suite stays usable on any machine; CI
//! installs MinGW and therefore runs them for real.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

fn anyexe() -> &'static str {
    env!("CARGO_BIN_EXE_anyexe")
}

fn tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

struct Built {
    dir: PathBuf,
}

/// `cargo test` rebuilds the library for the tests but not the `libanyexe.so` that `relink` copies
/// next to its output, so refresh it once; a stale copy would test yesterday's runtime.
fn refresh_runtime_library() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut cmd = Command::new(cargo);
        cmd.args(["build", "--lib"]).current_dir(env!("CARGO_MANIFEST_DIR"));
        if anyexe().contains("/release/") {
            cmd.arg("--release");
        }
        match cmd.output() {
            Ok(o) if o.status.success() => {}
            Ok(o) => eprintln!("warning: could not refresh libanyexe.so: {}", String::from_utf8_lossy(&o.stderr)),
            Err(e) => eprintln!("warning: could not run cargo to refresh libanyexe.so: {e}"),
        }
    });
}

/// Compiles every test program once per test run. `None` when MinGW is missing.
fn built() -> Option<&'static Built> {
    static B: OnceLock<Option<Built>> = OnceLock::new();
    B.get_or_init(|| {
        let gcc = tool("x86_64-w64-mingw32-gcc")?;
        let gxx = tool("x86_64-w64-mingw32-g++");
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/exe");
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("winexe");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().expect("compiler did not start");
            assert!(out.status.success(), "compile failed:\n{}", String::from_utf8_lossy(&out.stderr));
        };
        for t in ["t_threads", "t_files", "t_memory", "t_system", "t_unicode", "t_console", "t_process"] {
            run(Command::new(&gcc).args(["-O1", "-o"]).arg(dir.join(format!("{t}.exe"))).arg(src.join(format!("{t}.c"))));
        }
        run(Command::new(&gcc).args(["-O1", "-o"]).arg(dir.join("t_net.exe")).arg(src.join("t_net.c")).arg("-lws2_32"));
        run(Command::new(&gcc).args(["-O1", "-o"]).arg(dir.join("t_registry.exe")).arg(src.join("t_registry.c")).arg("-lshlwapi"));
        run(Command::new(&gcc)
            .args(["-O1", "-shared", "-static-libgcc", "-o"])
            .arg(dir.join("helper.dll"))
            .arg(src.join("helper.c"))
            .arg(format!("-Wl,--out-implib,{}", dir.join("libhelper.dll.a").display())));
        run(Command::new(&gcc).args(["-O1", "-o"]).arg(dir.join("t_dll.exe")).arg(src.join("t_dll.c")).arg("-L").arg(&dir).arg("-lhelper"));
        if let Some(gxx) = gxx {
            run(Command::new(gxx).args(["-O1", "-fexceptions", "-static", "-o"]).arg(dir.join("t_seh.exe")).arg(src.join("t_seh.cpp")));
        }
        Some(Built { dir })
    })
    .as_ref()
}

struct Case {
    name: &'static str,
    args: &'static [&'static str],
    stdin: &'static str,
}

const fn case(name: &'static str) -> Case {
    Case { name, args: &[], stdin: "" }
}

fn exec(mut cmd: Command, stdin: &str) -> (bool, String) {
    use std::io::Write;
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("spawn");
    let mut input = child.stdin.take().unwrap();
    let data = stdin.to_string();
    let writer = std::thread::spawn(move || {
        let _ = input.write_all(data.as_bytes());
    });
    let out = child.wait_with_output().unwrap();
    let _ = writer.join();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

fn check_run(c: &Case) {
    let Some(b) = built() else {
        eprintln!("skipping {}: MinGW-w64 (x86_64-w64-mingw32-gcc) is not installed", c.name);
        return;
    };
    let mut cmd = Command::new(anyexe());
    cmd.arg("run").arg(b.dir.join(format!("{}.exe", c.name))).args(c.args).current_dir(&b.dir);
    cmd.env("ANYEXE_PREFIX", b.dir.join(format!("prefix-run-{}", c.name)));
    let (ok, text) = exec(cmd, c.stdin);
    assert!(ok && text.contains("ALL OK"), "{} failed under `anyexe run`:\n{text}", c.name);
}

fn check_relinked(c: &Case) {
    let Some(b) = built() else {
        eprintln!("skipping {}: MinGW-w64 (x86_64-w64-mingw32-gcc) is not installed", c.name);
        return;
    };
    refresh_runtime_library();
    let out = b.dir.join(format!("relinked-{}", c.name));
    let _ = std::fs::remove_dir_all(&out);
    let status = Command::new(anyexe())
        .arg("relink")
        .arg(b.dir.join(format!("{}.exe", c.name)))
        .arg("-o")
        .arg(&out)
        .output()
        .expect("relink did not start");
    assert!(status.status.success(), "relink failed: {}", String::from_utf8_lossy(&status.stderr));
    if !out.join("libanyexe.so").exists() {
        eprintln!("skipping the relinked {}: libanyexe.so was not built next to the anyexe binary (run `cargo build` first)", c.name);
        return;
    }
    let elf = out.join(c.name);
    assert!(std::fs::read(&elf).unwrap().starts_with(b"\x7fELF"), "relink must produce an ELF file");
    let mut cmd = Command::new(&elf);
    cmd.args(c.args).current_dir(&out);
    cmd.env("ANYEXE_PREFIX", b.dir.join(format!("prefix-relinked-{}", c.name)));
    let (ok, text) = exec(cmd, c.stdin);
    assert!(ok && text.contains("ALL OK"), "{} failed as a relinked ELF:\n{text}", c.name);
}

macro_rules! program_tests {
    ($( $test:ident => $case:expr ;)*) => {
        mod run {
            use super::*;
            $( #[test] fn $test() { check_run(&$case); } )*
        }
        mod relinked {
            use super::*;
            $( #[test] fn $test() { check_relinked(&$case); } )*
        }
    };
}

program_tests! {
    threads_and_sync => case("t_threads");
    files_and_paths => case("t_files");
    memory => case("t_memory");
    system_info => Case { name: "t_system", args: &["first arg", "second"], stdin: "" };
    unicode => case("t_unicode");
    exceptions => case("t_seh");
    dll_loading => case("t_dll");
    console_input => Case { name: "t_console", args: &[], stdin: "hello\nworld\n10 20\n" };
    sockets => case("t_net");
    child_processes => case("t_process");
    registry => case("t_registry");
}

#[test]
fn relinked_hello_returns_its_exit_code() {
    let Some(gcc) = tool("x86_64-w64-mingw32-gcc") else {
        eprintln!("skipping: MinGW-w64 is not installed");
        return;
    };
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hello");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("hello.c");
    std::fs::write(&src, "#include <stdio.h>\nint main(int argc, char **argv) { printf(\"hello %d\\n\", argc); return argc; }\n").unwrap();
    let exe = dir.join("hello.exe");
    assert!(Command::new(gcc).args(["-O1", "-o"]).arg(&exe).arg(&src).status().unwrap().success());
    let direct = Command::new(anyexe()).arg("run").arg(&exe).args(["a", "b"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&direct.stdout), "hello 3\n");
    assert_eq!(direct.status.code(), Some(3));

    refresh_runtime_library();
    let out = dir.join("out");
    assert!(Command::new(anyexe()).arg("relink").arg(&exe).arg("-o").arg(&out).output().unwrap().status.success());
    if !out.join("libanyexe.so").exists() {
        return;
    }
    let elf = Command::new(out.join("hello")).args(["a", "b"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&elf.stdout), "hello 3\n");
    assert_eq!(elf.status.code(), Some(3));
}

#[test]
fn inspect_reports_a_native_verdict() {
    let Some(b) = built() else {
        eprintln!("skipping: MinGW-w64 is not installed");
        return;
    };
    let out = Command::new(anyexe()).arg("inspect").arg(b.dir.join("t_files.exe")).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("verdict     native"), "{text}");
    let json = Command::new(anyexe()).args(["inspect", "--json"]).arg(b.dir.join("t_files.exe")).output().unwrap();
    assert!(String::from_utf8_lossy(&json.stdout).trim_start().starts_with('{'));
}

#[test]
fn rejects_files_that_are_not_windows_executables() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let bogus = dir.join("not_a_program.exe");
    std::fs::write(&bogus, b"this is not a PE file").unwrap();
    let out = Command::new(anyexe()).arg("run").arg(&bogus).output().unwrap();
    assert_eq!(out.status.code(), Some(126));
    let missing = Command::new(anyexe()).arg("run").arg(dir.join("missing.exe")).output().unwrap();
    assert_eq!(missing.status.code(), Some(127));
}

/// Go and Rust programs bring their own runtimes (no C library). They are cross-compiled when the
/// toolchains are present.
fn cross_check(name: &str, exe: &Path, expect_stdout: &str, expect_code: i32) {
    let direct = Command::new(anyexe()).arg("run").arg(exe).args(["a", "b"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&direct.stdout), expect_stdout, "{name} under `anyexe run`: {}", String::from_utf8_lossy(&direct.stderr));
    assert_eq!(direct.status.code(), Some(expect_code), "{name} exit code under `anyexe run`");
    refresh_runtime_library();
    let out = exe.with_file_name(format!("{name}-linux"));
    let _ = std::fs::remove_dir_all(&out);
    let relink = Command::new(anyexe()).arg("relink").arg(exe).arg("-o").arg(&out).output().unwrap();
    assert!(relink.status.success(), "{}", String::from_utf8_lossy(&relink.stderr));
    if !out.join("libanyexe.so").exists() {
        return;
    }
    let stem = exe.file_stem().unwrap();
    let elf = Command::new(out.join(stem)).args(["a", "b"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&elf.stdout), expect_stdout, "{name} as ELF: {}", String::from_utf8_lossy(&elf.stderr));
    assert_eq!(elf.status.code(), Some(expect_code), "{name} exit code as ELF");
}

#[test]
fn go_program() {
    let Some(go) = tool("go") else {
        eprintln!("skipping: Go is not installed");
        return;
    };
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lang-go");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("go_hello.exe");
    let st = Command::new(go)
        .args(["build", "-o"])
        .arg(&exe)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lang/hello.go"))
        .env("GOOS", "windows")
        .env("GOARCH", "amd64")
        .env("CGO_ENABLED", "0")
        .env("GOCACHE", dir.join("cache"))
        .env("GOFLAGS", "-mod=mod")
        .status()
        .unwrap();
    assert!(st.success());
    cross_check("go_hello", &exe, "hello from go windows amd64 3\n", 7);
}

#[test]
fn rust_program() {
    let Some(rustc) = tool("rustc") else {
        eprintln!("skipping: rustc is not installed");
        return;
    };
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lang-rust");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("rs_hello.exe");
    let out = Command::new(rustc)
        .args(["--target", "x86_64-pc-windows-gnu", "-O", "-o"])
        .arg(&exe)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lang/hello.rs"))
        .output()
        .unwrap();
    if !out.status.success() {
        eprintln!("skipping: the x86_64-pc-windows-gnu target is not usable here: {}", String::from_utf8_lossy(&out.stderr));
        return;
    }
    cross_check("rs_hello", &exe, "hello from rust, args=3\n", 5);
}
