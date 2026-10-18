//! Process start-up for the native backend.

use crate::loader::{self, LoadError};
use crate::runtime::{self, handles, paths, Config, Trace};
use crate::shim::crt_startup;
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
pub struct RunOptions {
    pub prefix: Option<PathBuf>,
    pub strict: bool,
    pub trace: Option<Trace>,
}

fn setup(exe_host: PathBuf, args: Vec<String>, opts: &RunOptions, elf_alias: Option<PathBuf>) -> Result<PathBuf, LoadError> {
    let prefix = paths::init_prefix(opts.prefix.clone().unwrap_or_else(paths::default_prefix));
    runtime::init(Config {
        prefix,
        exe_win: paths::host_to_win(&exe_host),
        exe_host: exe_host.clone(),
        args,
        trace: opts.trace.unwrap_or_else(runtime::trace_from_env),
        strict: opts.strict,
        elf_alias,
    });
    runtime::init_env();
    handles::init_std_handles();
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    crt_startup::init_data();
    crate::shim::seh::install_fault_handlers();
    Ok(exe_host)
}

/// Loads `exe` into this process and runs it. Only returns on a load error; on success the process
/// ends when the program exits.
pub fn run_native(exe: &Path, args: Vec<String>, opts: &RunOptions) -> LoadError {
    let canon = match std::fs::canonicalize(exe) {
        Ok(p) => p,
        Err(e) => return LoadError::Io(format!("{}: {e}", exe.display())),
    };
    let exe_host = match setup(canon, args, opts, None) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match loader::load_main(&exe_host) {
        Ok((main, init)) => unsafe { loader::run_main(main, init) },
        Err(e) => e,
    }
}

/// Entry point of executables produced by `anyexe relink`. The ELF's entry stub hands control to
/// `__libc_start_main`, which calls this as `main`. The kernel has already mapped the PE image.
#[no_mangle]
pub unsafe extern "C" fn anyexe_relinked_main(argc: i32, argv: *const *const libc::c_char, _envp: *const *const libc::c_char) -> i32 {
    use std::ffi::CStr;
    let Some((base, _size, name)) = crate::relink::find_note() else {
        eprintln!("anyexe: this file was not produced by `anyexe relink`");
        return 127;
    };
    let args: Vec<String> = (1..argc.max(0) as usize).map(|i| CStr::from_ptr(*argv.add(i)).to_string_lossy().into_owned()).collect();
    let elf = std::fs::read_link("/proc/self/exe").unwrap_or_else(|_| PathBuf::from(&name));
    let exe_host = elf.with_file_name(&name);
    let opts = RunOptions::default();
    let alias = (exe_host.symlink_metadata().is_err()).then(|| elf.clone());
    if let Err(e) = setup(exe_host.clone(), args, &opts, alias) {
        eprintln!("anyexe: {e}");
        return 126;
    }
    match loader::attach_mapped(base as usize, &exe_host) {
        Ok((main, init)) => loader::run_main(main, init),
        Err(e) => {
            eprintln!("anyexe: {}: {e}", exe_host.display());
            126
        }
    }
}
