//! Reimplemented Windows libraries.
//!
//! Each module declares its functions with `shims!`; this file collects the tables for the registry.

use crate::registry::Shim;

pub mod crt_math;
pub mod crt_printf;
pub mod crt_setjmp;
pub mod crt_startup;
pub mod crt_stdio;
pub mod crt_stdlib;
pub mod gdi32;
pub mod crt_string;
pub mod k32_console;
pub mod k32_file;
pub mod k32_mem;
pub mod k32_proc;
pub mod k32_spawn;
pub mod k32_str;
pub mod k32_sync;
pub mod k32_thread;
pub mod k32_time;
pub mod misc;
pub mod ntdll;
pub mod ntdll_file;
pub mod seh;
pub mod user32;
pub mod winreg;
pub mod ws2_32;

pub fn tables() -> Vec<&'static [Shim]> {
    vec![
        k32_console::SHIMS,
        k32_file::SHIMS,
        k32_mem::SHIMS,
        k32_proc::SHIMS,
        k32_spawn::SPAWN_SHIMS,
        k32_spawn::WS2_SPAWN_SHIMS,
        k32_str::SHIMS,
        k32_sync::SHIMS,
        k32_thread::SHIMS,
        k32_time::SHIMS,
        seh::SHIMS,
        seh::NT_SHIMS,
        seh::NT_EXTRA,
        seh::K32_EXTRA,
        seh::CRT_EXTRA,
        seh::NT_EXTRA2,
        ntdll::SHIMS,
        ntdll::EXTRA,
        ntdll_file::NTFILE_SHIMS,
        ntdll_file::IOCP_SHIMS,
        misc::BCRYPT_SHIMS,
        misc::ADVAPI_SHIMS,
        winreg::REG_SHIMS,
        winreg::SHREG_SHIMS,
        misc::USERENV_SHIMS,
        misc::SHELL_SHIMS,
        misc::OLE_SHIMS,
        misc::OLEAUT_SHIMS,
        misc::USER_SHIMS,
        misc::USER_EXTRA,
        misc::SHLWAPI_SHIMS,
        misc::VERSION_SHIMS,
        misc::PSAPI_SHIMS,
        misc::PSAPI_K32,
        misc::WINMM_SHIMS,
        ws2_32::WS2_SHIMS,
        user32::USER32_SHIMS,
        gdi32::GDI32_SHIMS,
        gdi32::MSIMG32_SHIMS,
        crt_math::SHIMS,
        crt_startup::SHIMS,
        crt_startup::DATA,
        crt_setjmp::EXTRA,
        crt_stdio::SHIMS,
        crt_stdio::EXTRA,
        crt_stdlib::SHIMS,
        crt_string::SHIMS,
    ]
}
