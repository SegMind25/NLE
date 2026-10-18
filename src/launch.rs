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

fn setup(exe: &Path, args: Vec<String>, opts: &RunOptions) -> Result<PathBuf, LoadError> {
    let exe_host = std::fs::canonicalize(exe).map_err(|e| LoadError::Io(format!("{}: {e}", exe.display())))?;
    let prefix = paths::init_prefix(opts.prefix.clone().unwrap_or_else(paths::default_prefix));
    runtime::init(Config {
        prefix,
        exe_win: paths::host_to_win(&exe_host),
        exe_host: exe_host.clone(),
        args,
        trace: opts.trace.unwrap_or_else(runtime::trace_from_env),
        strict: opts.strict,
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
    let exe_host = match setup(exe, args, opts) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match loader::load_main(&exe_host) {
        Ok((main, init)) => unsafe { loader::run_main(main, init) },
        Err(e) => e,
    }
}
