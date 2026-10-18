//! Registry of reimplemented Windows library functions.
//!
//! Every shim module declares its functions with the [`shims!`](crate::shims) macro, which
//! emits both the `extern "win64"` function and a table entry. The loader resolves a PE
//! import `(dll, name)` to a native address through [`resolve`].

use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub struct Shim {
    pub family: &'static str,
    pub name: &'static str,
    pub addr: *const (),
}

unsafe impl Sync for Shim {}
unsafe impl Send for Shim {}

/// Declares Windows API functions and registers them.
///
/// ```ignore
/// shims! { "kernel32";
///     fn GetCurrentProcessId() -> DWORD { ... }
///     fn cxx_new as "??2@YAPEAX_K@Z"(n: usize) -> *mut c_void { ... }
/// }
/// ```
#[macro_export]
macro_rules! shims {
    ($family:literal; $($rest:tt)*) => {
        $crate::shims!($family => SHIMS; $($rest)*);
    };
    ($family:literal => $table:ident; $( $(#[$meta:meta])* fn $name:ident $(as $exp:literal)? ( $($arg:ident : $ty:ty),* $(,)? ) $(-> $ret:ty)? $body:block )* ) => {
        $(
            $(#[$meta])*
            #[allow(non_snake_case, clippy::missing_safety_doc, clippy::not_unsafe_ptr_arg_deref)]
            pub unsafe extern "win64" fn $name($($arg: $ty),*) $(-> $ret)? $body
        )*
        pub static $table: &[$crate::registry::Shim] = &[
            $( $crate::registry::Shim {
                family: $family,
                name: $crate::shim_name!($name $(, $exp)?),
                addr: $name as *const (),
            }, )*
        ];
    };
}

#[macro_export]
macro_rules! shim_name {
    ($n:ident) => {
        stringify!($n)
    };
    ($n:ident, $e:literal) => {
        $e
    };
}

/// Extra table entries for functions that cannot be written with `shims!`
/// (naked variadic thunks, aliases, data exports).
#[macro_export]
macro_rules! shim_extra {
    ($family:literal; $( $name:literal => $addr:expr ),* $(,)? ) => {
        $crate::shim_extra!($family => EXTRA; $( $name => $addr ),*);
    };
    ($family:literal => $table:ident; $( $name:literal => $addr:expr ),* $(,)? ) => {
        pub static $table: &[$crate::registry::Shim] = &[
            $( $crate::registry::Shim { family: $family, name: $name, addr: $addr as *const () }, )*
        ];
    };
}

struct Index {
    by_family: HashMap<&'static str, HashMap<&'static str, usize>>,
}

fn index() -> &'static Index {
    static IDX: OnceLock<Index> = OnceLock::new();
    IDX.get_or_init(|| {
        let mut by_family: HashMap<&'static str, HashMap<&'static str, usize>> = HashMap::new();
        for table in crate::shim::tables() {
            for s in table.iter() {
                by_family.entry(s.family).or_default().insert(s.name, s.addr as usize);
            }
        }
        Index { by_family }
    })
}

/// Maps a DLL name from an import table to the family of shims that implements it.
pub fn family_of(dll: &str) -> Option<&'static str> {
    let mut d = dll.to_ascii_lowercase();
    if let Some(stripped) = d.strip_suffix(".dll") {
        d = stripped.to_string();
    }
    if d.starts_with("api-ms-win-crt-") || d.starts_with("api-ms-win-crt_") {
        return Some("crt");
    }
    if d.starts_with("api-ms-win-") || d.starts_with("ext-ms-win-") || d.starts_with("api-ms-onecore") {
        return Some("kernel32");
    }
    let fam = match d.as_str() {
        "kernel32" | "kernelbase" | "kernel.appcore" => "kernel32",
        "ntdll" => "ntdll",
        "msvcrt" | "ucrtbase" | "msvcr70" | "msvcr71" | "msvcr80" | "msvcr90" | "msvcr100" | "msvcr110" | "msvcr120"
        | "vcruntime140" | "vcruntime140_1" | "vcruntime140d" | "ucrtbased" => "crt",
        "user32" => "user32",
        "gdi32" | "gdi32full" => "gdi32",
        "advapi32" | "sechost" => "advapi32",
        "shell32" => "shell32",
        "shlwapi" => "shlwapi",
        "ole32" => "ole32",
        "oleaut32" => "oleaut32",
        "ws2_32" | "wsock32" => "ws2_32",
        "bcrypt" | "bcryptprimitives" => "bcrypt",
        "winmm" => "winmm",
        "version" => "version",
        "psapi" => "psapi",
        "userenv" => "userenv",
        "iphlpapi" => "iphlpapi",
        "crypt32" => "crypt32",
        "secur32" | "sspicli" => "secur32",
        "comctl32" => "comctl32",
        "comdlg32" => "comdlg32",
        "dbghelp" => "dbghelp",
        "shcore" => "shcore",
        "imm32" => "imm32",
        "setupapi" => "setupapi",
        "wintrust" => "wintrust",
        "rpcrt4" => "rpcrt4",
        "mswsock" => "mswsock",
        "netapi32" => "netapi32",
        "cfgmgr32" => "cfgmgr32",
        "dwmapi" => "dwmapi",
        "uxtheme" => "uxtheme",
        "winhttp" => "winhttp",
        "wininet" => "wininet",
        "d3d9" | "d3d11" | "d3d12" | "dxgi" | "opengl32" | "glu32" | "dinput8" | "xinput1_4" | "xinput1_3"
        | "xaudio2_9" | "dsound" => "graphics",
        _ => return None,
    };
    Some(fam)
}

/// True for DLLs that belong to Windows itself (as opposed to ones shipped with an app).
pub fn is_system_dll(dll: &str) -> bool {
    family_of(dll).is_some()
}

fn fallbacks(family: &str) -> &'static [&'static str] {
    match family {
        "kernel32" => &["ntdll", "advapi32", "bcrypt", "shlwapi"],
        "ntdll" => &["kernel32"],
        _ => &[],
    }
}

/// Resolves `dll!name` to the address of a native implementation.
pub fn resolve(dll: &str, name: &str) -> Option<usize> {
    let fam = family_of(dll)?;
    resolve_in_family(fam, name)
}

pub fn resolve_in_family(fam: &str, name: &str) -> Option<usize> {
    let idx = index();
    if let Some(a) = idx.by_family.get(fam).and_then(|m| m.get(name)) {
        return Some(*a);
    }
    for f in fallbacks(fam) {
        if let Some(a) = idx.by_family.get(f).and_then(|m| m.get(name)) {
            return Some(*a);
        }
    }
    None
}

/// All registered functions as `(family, name)`, sorted. Used by the coverage report.
pub fn all_names() -> Vec<(&'static str, &'static str)> {
    let mut v: Vec<_> = index().by_family.iter().flat_map(|(f, m)| m.keys().map(move |n| (*f, *n))).collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dll_names_map_to_families() {
        assert_eq!(family_of("KERNEL32.dll"), Some("kernel32"));
        assert_eq!(family_of("api-ms-win-core-synch-l1-2-0.dll"), Some("kernel32"));
        assert_eq!(family_of("api-ms-win-crt-stdio-l1-1-0.dll"), Some("crt"));
        assert_eq!(family_of("MSVCRT.DLL"), Some("crt"));
        assert_eq!(family_of("my_private_lib.dll"), None);
    }
}
