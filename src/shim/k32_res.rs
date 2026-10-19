//! kernel32: finding and loading the resources of a module (`FindResource`, `LoadResource`, ...).
//!
//! An `HRSRC` is the address of the resource's data entry inside the mapped image, and a loaded
//! resource is a pointer to its bytes, which is what Windows hands out for modules mapped as images.

use crate::gui::res::{self, Key};
use crate::win::*;

const ERROR_RESOURCE_DATA_NOT_FOUND: DWORD = 1812;
const ERROR_RESOURCE_TYPE_NOT_FOUND: DWORD = 1813;
const ERROR_RESOURCE_NAME_NOT_FOUND: DWORD = 1814;

fn find_resource(module: usize, name: usize, ty: usize, lang: u16, wide: bool) -> usize {
    let Some(m) = res::module_res(module) else {
        set_last_error(ERROR_RESOURCE_DATA_NOT_FOUND);
        return 0;
    };
    let (ty, name) = (Key::from_arg(ty, wide), Key::from_arg(name, wide));
    match res::find_in_module(&m, &ty, &name, lang) {
        Some(f) => m.base + f.entry,
        None => {
            let known = !res::names_in(m.image(), m.root(), &ty).is_empty();
            set_last_error(if known { ERROR_RESOURCE_NAME_NOT_FOUND } else { ERROR_RESOURCE_TYPE_NOT_FOUND });
            0
        }
    }
}

/// Reads the data entry an `HRSRC` points to, if it points into the module.
fn entry_of(module: usize, hrsrc: usize) -> Option<(usize, usize)> {
    let m = res::module_res(module)?;
    if hrsrc < m.base || hrsrc + 16 > m.base + m.size {
        return None;
    }
    let e = unsafe { std::slice::from_raw_parts(hrsrc as *const u8, 8) };
    let rva = u32::from_le_bytes(e[0..4].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(e[4..8].try_into().unwrap()) as usize;
    (rva.checked_add(size)? <= m.size).then_some((m.base + rva, size))
}

unsafe fn enum_names(module: usize, ty: usize, cb: usize, lparam: isize, wide: bool) -> BOOL {
    let m = res::module_res(module);
    if m.is_none() {
        set_last_error(ERROR_RESOURCE_DATA_NOT_FOUND);
        return FALSE;
    }
    type Cb = unsafe extern "win64" fn(usize, usize, usize, isize) -> BOOL;
    let f: Cb = std::mem::transmute(cb);
    let ty_key = Key::from_arg(ty, wide);
    for name in res::names(module, &ty_key) {
        // a numeric name is passed as MAKEINTRESOURCE, a string as a pointer in the caller's character set
        let (arg, _keep): (usize, Option<Vec<u8>>) = match &name {
            Key::Id(n) => (*n as usize, None),
            Key::Name(s) => {
                let bytes: Vec<u8> = if wide { s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect() } else { s.bytes().chain([0]).collect() };
                (bytes.as_ptr() as usize, Some(bytes))
            }
        };
        if f(module, ty, arg, lparam) == 0 {
            break;
        }
    }
    TRUE
}

crate::shims! { "kernel32";
    fn FindResourceW(module: HMODULE, name: usize, ty: usize) -> usize {
        find_resource(module, name, ty, 0, true)
    }
    fn FindResourceA(module: HMODULE, name: usize, ty: usize) -> usize {
        find_resource(module, name, ty, 0, false)
    }
    fn FindResourceExW(module: HMODULE, ty: usize, name: usize, lang: u16) -> usize {
        find_resource(module, name, ty, lang, true)
    }
    fn FindResourceExA(module: HMODULE, ty: usize, name: usize, lang: u16) -> usize {
        find_resource(module, name, ty, lang, false)
    }
    fn LoadResource(module: HMODULE, hrsrc: usize) -> usize {
        match entry_of(module, hrsrc) {
            Some((data, _)) => data,
            None => {
                set_last_error(ERROR_INVALID_PARAMETER);
                0
            }
        }
    }
    fn LockResource(hglobal: usize) -> usize {
        hglobal
    }
    fn SizeofResource(module: HMODULE, hrsrc: usize) -> DWORD {
        match entry_of(module, hrsrc) {
            Some((_, size)) => size as DWORD,
            None => {
                set_last_error(ERROR_INVALID_PARAMETER);
                0
            }
        }
    }
    fn FreeResource(_hglobal: usize) -> BOOL {
        FALSE
    }
    fn EnumResourceNamesW(module: HMODULE, ty: usize, cb: usize, lparam: isize) -> BOOL {
        enum_names(module, ty, cb, lparam, true)
    }
    fn EnumResourceNamesA(module: HMODULE, ty: usize, cb: usize, lparam: isize) -> BOOL {
        enum_names(module, ty, cb, lparam, false)
    }
}
