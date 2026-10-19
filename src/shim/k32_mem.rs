//! kernel32: virtual memory, heaps, global/local memory and file mappings.

use crate::runtime::handles::{self, Kind, MappingObj};
use crate::runtime::{strings, teb};
use crate::win::*;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

const GRANULARITY: usize = 0x10000;
const PAGE: usize = 4096;

static ALLOCS: Mutex<BTreeMap<usize, usize>> = Mutex::new(BTreeMap::new());
static VIEWS: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);

fn round_up(v: usize, to: usize) -> usize {
    (v + to - 1) & !(to - 1)
}

fn round_down(v: usize, to: usize) -> usize {
    v & !(to - 1)
}

pub fn prot_from_win(p: DWORD) -> i32 {
    if p & PAGE_GUARD != 0 {
        return libc::PROT_NONE;
    }
    match p & 0xff {
        PAGE_NOACCESS => libc::PROT_NONE,
        PAGE_READONLY => libc::PROT_READ,
        PAGE_READWRITE | PAGE_WRITECOPY => libc::PROT_READ | libc::PROT_WRITE,
        PAGE_EXECUTE => libc::PROT_EXEC | libc::PROT_READ,
        PAGE_EXECUTE_READ => libc::PROT_READ | libc::PROT_EXEC,
        PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY => libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
        _ => libc::PROT_READ | libc::PROT_WRITE,
    }
}

pub fn prot_to_win(r: bool, w: bool, x: bool) -> DWORD {
    match (r, w, x) {
        (false, false, false) => PAGE_NOACCESS,
        (true, false, false) => PAGE_READONLY,
        (_, true, false) => PAGE_READWRITE,
        (false, false, true) => PAGE_EXECUTE,
        (true, false, true) => PAGE_EXECUTE_READ,
        (_, true, true) => PAGE_EXECUTE_READWRITE,
    }
}

#[derive(Clone, Debug)]
pub struct Vma {
    pub start: usize,
    pub end: usize,
    pub r: bool,
    pub w: bool,
    pub x: bool,
    pub file_backed: bool,
}

fn read_maps() -> Vec<Vma> {
    let text = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let range = it.next()?;
            let perms = it.next()?.as_bytes();
            let _off = it.next()?;
            let _dev = it.next()?;
            let inode = it.next()?;
            let (s, e) = range.split_once('-')?;
            Some(Vma {
                start: usize::from_str_radix(s, 16).ok()?,
                end: usize::from_str_radix(e, 16).ok()?,
                r: perms[0] == b'r',
                w: perms[1] == b'w',
                x: perms[2] == b'x',
                file_backed: inode != "0",
            })
        })
        .collect()
}

/// Mapping that contains `addr`, plus the start of the next mapping above it (for free regions).
pub fn find_vma(addr: usize) -> (Option<Vma>, usize) {
    let maps = read_maps();
    let mut next = 0x7fff_ffff_0000usize;
    for v in &maps {
        if addr >= v.start && addr < v.end {
            return (Some(v.clone()), next);
        }
        if v.start > addr && v.start < next {
            next = v.start;
        }
    }
    (None, next)
}

fn track(base: usize, size: usize) {
    ALLOCS.lock().unwrap().insert(base, size);
}

unsafe fn alloc_region(len: usize, prot: i32) -> usize {
    let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE;
    let over = len + GRANULARITY;
    let p = libc::mmap(std::ptr::null_mut(), over, libc::PROT_NONE, flags, -1, 0);
    if p == libc::MAP_FAILED {
        return 0;
    }
    let raw = p as usize;
    let base = round_up(raw, GRANULARITY);
    if base > raw {
        libc::munmap(raw as *mut libc::c_void, base - raw);
    }
    let tail = raw + over - (base + len);
    if tail > 0 {
        libc::munmap((base + len) as *mut libc::c_void, tail);
    }
    if prot != libc::PROT_NONE && libc::mprotect(base as *mut libc::c_void, len, prot) != 0 {
        libc::munmap(base as *mut libc::c_void, len);
        return 0;
    }
    base
}

unsafe fn virtual_alloc(addr: usize, size: usize, ty: DWORD, protect: DWORD) -> usize {
    if size == 0 {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let prot = prot_from_win(protect);
    let reserve = ty & MEM_RESERVE != 0;
    let commit = ty & MEM_COMMIT != 0;
    if ty & MEM_RESET != 0 {
        libc::madvise(round_down(addr, PAGE) as *mut libc::c_void, round_up(size, PAGE), libc::MADV_FREE);
        return addr;
    }
    if !reserve && !commit {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    if addr == 0 || reserve {
        let want = if commit { prot } else { libc::PROT_NONE };
        if addr == 0 {
            let len = round_up(size, PAGE);
            let base = alloc_region(len, want);
            if base == 0 {
                set_last_error(ERROR_NOT_ENOUGH_MEMORY);
                return 0;
            }
            track(base, len);
            return base;
        }
        let base = round_down(addr, GRANULARITY);
        let len = round_up(addr - base + size, PAGE);
        let p = libc::mmap(
            base as *mut libc::c_void,
            len,
            want,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE | libc::MAP_FIXED_NOREPLACE,
            -1,
            0,
        );
        if p == libc::MAP_FAILED || p as usize != base {
            if p != libc::MAP_FAILED {
                libc::munmap(p, len);
            }
            set_last_error(ERROR_INVALID_ADDRESS);
            return 0;
        }
        track(base, len);
        return base;
    }
    let start = round_down(addr, PAGE);
    let end = round_up(addr + size, PAGE);
    if libc::mprotect(start as *mut libc::c_void, end - start, prot) != 0 {
        set_last_error(ERROR_INVALID_ADDRESS);
        return 0;
    }
    start
}

unsafe fn virtual_free(addr: usize, size: usize, ty: DWORD) -> BOOL {
    if ty & MEM_RELEASE != 0 {
        if size != 0 {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        let len = ALLOCS.lock().unwrap().remove(&addr);
        return match len {
            Some(l) => {
                libc::munmap(addr as *mut libc::c_void, l);
                TRUE
            }
            None => {
                set_last_error(ERROR_INVALID_PARAMETER);
                FALSE
            }
        };
    }
    if ty & MEM_DECOMMIT != 0 {
        let start = round_down(addr, PAGE);
        let len = round_up(addr + size.max(1), PAGE) - start;
        let p = libc::mmap(
            start as *mut libc::c_void,
            len,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE | libc::MAP_FIXED,
            -1,
            0,
        );
        return if p == libc::MAP_FAILED {
            set_last_error(ERROR_INVALID_ADDRESS);
            FALSE
        } else {
            TRUE
        };
    }
    set_last_error(ERROR_INVALID_PARAMETER);
    FALSE
}

unsafe fn virtual_protect(addr: usize, size: usize, new: DWORD, old: *mut DWORD) -> BOOL {
    let start = round_down(addr, PAGE);
    let len = round_up(addr + size.max(1), PAGE) - start;
    let (vma, _) = find_vma(start);
    let Some(v) = vma else {
        set_last_error(ERROR_INVALID_ADDRESS);
        return FALSE;
    };
    if !old.is_null() {
        *old = prot_to_win(v.r, v.w, v.x);
    }
    if libc::mprotect(start as *mut libc::c_void, len, prot_from_win(new)) != 0 {
        set_last_error(ERROR_INVALID_ADDRESS);
        return FALSE;
    }
    TRUE
}

unsafe fn virtual_query(addr: usize, mbi: *mut MEMORY_BASIC_INFORMATION, len: usize) -> usize {
    if mbi.is_null() || len < std::mem::size_of::<MEMORY_BASIC_INFORMATION>() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let page = round_down(addr, PAGE);
    let (vma, next) = find_vma(page);
    let m = &mut *mbi;
    *m = std::mem::zeroed();
    m.base_address = page;
    match vma {
        None => {
            m.region_size = next.saturating_sub(page);
            m.state = MEM_FREE;
            m.protect = PAGE_NOACCESS;
        }
        Some(v) => {
            m.region_size = v.end - page;
            let committed = v.r || v.w || v.x;
            m.state = if committed { MEM_COMMIT } else { MEM_RESERVE };
            m.protect = if committed { prot_to_win(v.r, v.w, v.x) } else { 0 };
            m.allocation_protect = m.protect.max(PAGE_READWRITE);
            m.type_ = if crate::loader::module_from_addr(page).is_some() {
                MEM_IMAGE
            } else if v.file_backed {
                MEM_MAPPED
            } else {
                MEM_PRIVATE
            };
            m.allocation_base = match crate::loader::module_from_addr(page) {
                Some(md) => md.base,
                None => ALLOCS.lock().unwrap().range(..=page).next_back().filter(|(b, l)| page < **b + **l).map(|(b, _)| *b).unwrap_or(v.start),
            };
        }
    }
    std::mem::size_of::<MEMORY_BASIC_INFORMATION>()
}

fn is_self(h: HANDLE) -> bool {
    h == CURRENT_PROCESS || h == 0
}

crate::shims! { "kernel32";
    fn VirtualAlloc(addr: usize, size: usize, ty: DWORD, protect: DWORD) -> usize {
        virtual_alloc(addr, size, ty, protect)
    }
    fn VirtualAllocEx(process: HANDLE, addr: usize, size: usize, ty: DWORD, protect: DWORD) -> usize {
        if !is_self(process) {
            set_last_error(ERROR_NOT_SUPPORTED);
            return 0;
        }
        virtual_alloc(addr, size, ty, protect)
    }
    fn VirtualAllocExNuma(process: HANDLE, addr: usize, size: usize, ty: DWORD, protect: DWORD, _node: DWORD) -> usize {
        VirtualAllocEx(process, addr, size, ty, protect)
    }
    fn VirtualFree(addr: usize, size: usize, ty: DWORD) -> BOOL {
        virtual_free(addr, size, ty)
    }
    fn VirtualFreeEx(process: HANDLE, addr: usize, size: usize, ty: DWORD) -> BOOL {
        if !is_self(process) {
            set_last_error(ERROR_NOT_SUPPORTED);
            return FALSE;
        }
        virtual_free(addr, size, ty)
    }
    fn VirtualProtect(addr: usize, size: usize, new: DWORD, old: *mut DWORD) -> BOOL {
        virtual_protect(addr, size, new, old)
    }
    fn VirtualProtectEx(process: HANDLE, addr: usize, size: usize, new: DWORD, old: *mut DWORD) -> BOOL {
        if !is_self(process) {
            set_last_error(ERROR_NOT_SUPPORTED);
            return FALSE;
        }
        virtual_protect(addr, size, new, old)
    }
    fn VirtualQuery(addr: usize, mbi: *mut MEMORY_BASIC_INFORMATION, len: usize) -> usize {
        virtual_query(addr, mbi, len)
    }
    fn VirtualQueryEx(process: HANDLE, addr: usize, mbi: *mut MEMORY_BASIC_INFORMATION, len: usize) -> usize {
        if !is_self(process) {
            set_last_error(ERROR_NOT_SUPPORTED);
            return 0;
        }
        virtual_query(addr, mbi, len)
    }
    fn VirtualLock(addr: usize, size: usize) -> BOOL {
        libc::mlock(addr as *const libc::c_void, size);
        TRUE
    }
    fn VirtualUnlock(addr: usize, size: usize) -> BOOL {
        libc::munlock(addr as *const libc::c_void, size);
        TRUE
    }
    fn FlushInstructionCache(_process: HANDLE, _addr: usize, _size: usize) -> BOOL {
        TRUE
    }
    fn GetLargePageMinimum() -> usize {
        2 * 1024 * 1024
    }

    fn GetProcessHeap() -> HANDLE {
        teb::process_heap()
    }
    fn GetProcessHeaps(n: DWORD, out: *mut HANDLE) -> DWORD {
        if n >= 1 && !out.is_null() {
            *out = teb::process_heap();
        }
        1
    }
    fn HeapCreate(_options: DWORD, _initial: usize, _max: usize) -> HANDLE {
        let token = libc::mmap(std::ptr::null_mut(), PAGE, libc::PROT_NONE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE, -1, 0);
        token as HANDLE
    }
    fn HeapDestroy(_heap: HANDLE) -> BOOL {
        TRUE
    }
    fn HeapAlloc(_heap: HANDLE, flags: DWORD, size: usize) -> *mut u8 {
        let p = if flags & HEAP_ZERO_MEMORY != 0 { libc::calloc(1, size.max(1)) } else { libc::malloc(size.max(1)) };
        if p.is_null() {
            set_last_error(ERROR_NOT_ENOUGH_MEMORY);
        }
        p as *mut u8
    }
    fn HeapReAlloc(_heap: HANDLE, flags: DWORD, ptr: *mut u8, size: usize) -> *mut u8 {
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        let old = libc::malloc_usable_size(ptr as *mut libc::c_void);
        if flags & HEAP_REALLOC_IN_PLACE_ONLY != 0 && size > old {
            return std::ptr::null_mut();
        }
        let p = libc::realloc(ptr as *mut libc::c_void, size.max(1)) as *mut u8;
        if p.is_null() {
            set_last_error(ERROR_NOT_ENOUGH_MEMORY);
            return p;
        }
        if flags & HEAP_ZERO_MEMORY != 0 && size > old {
            std::ptr::write_bytes(p.add(old), 0, size - old);
        }
        p
    }
    fn HeapFree(_heap: HANDLE, _flags: DWORD, ptr: *mut u8) -> BOOL {
        if !ptr.is_null() {
            libc::free(ptr as *mut libc::c_void);
        }
        TRUE
    }
    fn HeapSize(_heap: HANDLE, _flags: DWORD, ptr: *const u8) -> usize {
        if ptr.is_null() {
            return usize::MAX;
        }
        libc::malloc_usable_size(ptr as *mut libc::c_void)
    }
    fn HeapValidate(_heap: HANDLE, _flags: DWORD, _ptr: *const u8) -> BOOL {
        TRUE
    }
    fn HeapCompact(_heap: HANDLE, _flags: DWORD) -> usize {
        0
    }
    fn HeapLock(_heap: HANDLE) -> BOOL {
        TRUE
    }
    fn HeapUnlock(_heap: HANDLE) -> BOOL {
        TRUE
    }
    fn HeapSetInformation(_heap: HANDLE, _class: i32, _info: *mut u8, _len: usize) -> BOOL {
        TRUE
    }
    fn HeapQueryInformation(_heap: HANDLE, _class: i32, info: *mut u32, len: usize, ret: *mut usize) -> BOOL {
        if !ret.is_null() {
            *ret = 4;
        }
        if !info.is_null() && len >= 4 {
            *info = 0;
        }
        TRUE
    }

    fn GlobalAlloc(flags: UINT, size: usize) -> *mut u8 {
        let p = if flags & 0x40 != 0 { libc::calloc(1, size.max(1)) } else { libc::malloc(size.max(1)) };
        if p.is_null() {
            set_last_error(ERROR_NOT_ENOUGH_MEMORY);
        }
        p as *mut u8
    }
    fn GlobalReAlloc(ptr: *mut u8, size: usize, flags: UINT) -> *mut u8 {
        let old = libc::malloc_usable_size(ptr as *mut libc::c_void);
        let p = libc::realloc(ptr as *mut libc::c_void, size.max(1)) as *mut u8;
        if !p.is_null() && flags & 0x40 != 0 && size > old {
            std::ptr::write_bytes(p.add(old), 0, size - old);
        }
        p
    }
    fn GlobalFree(ptr: *mut u8) -> *mut u8 {
        libc::free(ptr as *mut libc::c_void);
        std::ptr::null_mut()
    }
    fn GlobalLock(ptr: *mut u8) -> *mut u8 {
        ptr
    }
    fn GlobalUnlock(_ptr: *mut u8) -> BOOL {
        FALSE
    }
    fn GlobalHandle(ptr: *mut u8) -> *mut u8 {
        ptr
    }
    fn GlobalSize(ptr: *mut u8) -> usize {
        libc::malloc_usable_size(ptr as *mut libc::c_void)
    }
    fn GlobalFlags(_ptr: *mut u8) -> UINT {
        0
    }
    fn LocalAlloc(flags: UINT, size: usize) -> *mut u8 {
        GlobalAlloc(flags, size)
    }
    fn LocalReAlloc(ptr: *mut u8, size: usize, flags: UINT) -> *mut u8 {
        GlobalReAlloc(ptr, size, flags)
    }
    fn LocalFree(ptr: *mut u8) -> *mut u8 {
        GlobalFree(ptr)
    }
    fn LocalLock(ptr: *mut u8) -> *mut u8 {
        ptr
    }
    fn LocalUnlock(_ptr: *mut u8) -> BOOL {
        FALSE
    }
    fn LocalHandle(ptr: *mut u8) -> *mut u8 {
        ptr
    }
    fn LocalSize(ptr: *mut u8) -> usize {
        libc::malloc_usable_size(ptr as *mut libc::c_void)
    }
    fn LocalFlags(_ptr: *mut u8) -> UINT {
        0
    }

    fn IsBadReadPtr(ptr: *const u8, size: usize) -> BOOL {
        probe(ptr as usize, size, false)
    }
    fn IsBadWritePtr(ptr: *mut u8, size: usize) -> BOOL {
        probe(ptr as usize, size, true)
    }
    fn IsBadCodePtr(ptr: usize) -> BOOL {
        probe(ptr, 1, false)
    }

    fn CreateFileMappingW(file: HANDLE, _attrs: usize, protect: DWORD, size_high: DWORD, size_low: DWORD, name: LPCWSTR) -> HANDLE {
        let n = if name.is_null() { None } else { Some(strings::w_ptr_to_string(name)) };
        create_mapping(file, protect, ((size_high as u64) << 32) | size_low as u64, n)
    }
    fn CreateFileMappingA(file: HANDLE, _attrs: usize, protect: DWORD, size_high: DWORD, size_low: DWORD, name: LPCSTR) -> HANDLE {
        let n = if name.is_null() { None } else { Some(strings::a_to_string(name)) };
        create_mapping(file, protect, ((size_high as u64) << 32) | size_low as u64, n)
    }
    fn CreateFileMappingFromApp(file: HANDLE, attrs: usize, protect: DWORD, size: u64, name: LPCWSTR) -> HANDLE {
        CreateFileMappingW(file, attrs, protect, (size >> 32) as u32, size as u32, name)
    }
    fn OpenFileMappingW(_access: DWORD, _inherit: BOOL, name: LPCWSTR) -> HANDLE {
        let n = strings::w_ptr_to_string(name);
        match crate::shim::k32_sync::named_get(&n) {
            Some(o) => handles::insert_arc(o),
            None => {
                set_last_error(ERROR_FILE_NOT_FOUND);
                0
            }
        }
    }
    fn OpenFileMappingA(access: DWORD, inherit: BOOL, name: LPCSTR) -> HANDLE {
        let w = strings::to_wide_nul(&strings::a_to_string(name));
        OpenFileMappingW(access, inherit, w.as_ptr())
    }
    fn MapViewOfFile(mapping: HANDLE, access: DWORD, off_high: DWORD, off_low: DWORD, len: usize) -> usize {
        map_view(mapping, access, ((off_high as u64) << 32) | off_low as u64, len, 0)
    }
    fn MapViewOfFileEx(mapping: HANDLE, access: DWORD, off_high: DWORD, off_low: DWORD, len: usize, base: usize) -> usize {
        map_view(mapping, access, ((off_high as u64) << 32) | off_low as u64, len, base)
    }
    fn MapViewOfFileFromApp(mapping: HANDLE, access: DWORD, offset: u64, len: usize) -> usize {
        map_view(mapping, access, offset, len, 0)
    }
    fn UnmapViewOfFile(addr: usize) -> BOOL {
        let len = VIEWS.lock().unwrap().as_mut().and_then(|m| m.remove(&addr));
        match len {
            Some(l) => {
                libc::munmap(addr as *mut libc::c_void, l);
                TRUE
            }
            None => {
                set_last_error(ERROR_INVALID_ADDRESS);
                FALSE
            }
        }
    }
    fn FlushViewOfFile(addr: usize, len: usize) -> BOOL {
        libc::msync(round_down(addr, PAGE) as *mut libc::c_void, len.max(1), libc::MS_ASYNC);
        TRUE
    }
}

fn probe(addr: usize, size: usize, write: bool) -> BOOL {
    if size == 0 {
        return FALSE;
    }
    let end = addr + size;
    let mut cur = round_down(addr, PAGE);
    while cur < end {
        match find_vma(cur) {
            (Some(v), _) if (if write { v.w } else { v.r }) => cur = v.end,
            _ => return TRUE,
        }
    }
    FALSE
}

unsafe fn create_mapping(file: HANDLE, protect: DWORD, size: u64, name: Option<String>) -> HANDLE {
    if let Some(n) = &name {
        if let Some(existing) = crate::shim::k32_sync::named_get(n) {
            if matches!(&*existing, Kind::Mapping(_)) {
                set_last_error(ERROR_ALREADY_EXISTS);
                return handles::insert_arc(existing);
            }
        }
    }
    let (fd, owns, size) = if file == INVALID_HANDLE_VALUE {
        if size == 0 {
            set_last_error(ERROR_INVALID_PARAMETER);
            return 0;
        }
        let fd = libc::memfd_create(b"nle-map\0".as_ptr() as *const libc::c_char, 0);
        if fd < 0 || libc::ftruncate(fd, size as libc::off_t) != 0 {
            set_last_error(ERROR_NOT_ENOUGH_MEMORY);
            return 0;
        }
        (fd, true, size)
    } else {
        let Some(k) = handles::get(file) else {
            set_last_error(ERROR_INVALID_HANDLE);
            return 0;
        };
        let Kind::File(f) = &*k else {
            set_last_error(ERROR_INVALID_HANDLE);
            return 0;
        };
        let dupfd = libc::dup(f.fd());
        let mut st: libc::stat = std::mem::zeroed();
        libc::fstat(dupfd, &mut st);
        let real = if size == 0 { st.st_size as u64 } else { size };
        if real > st.st_size as u64 && protect & 0xcc != 0 {
            libc::ftruncate(dupfd, real as libc::off_t);
        }
        (dupfd, true, real)
    };
    let obj = std::sync::Arc::new(Kind::Mapping(MappingObj { fd, owns_fd: owns, size, protect }));
    if let Some(n) = name {
        crate::shim::k32_sync::named_put(&n, obj.clone());
    }
    handles::insert_arc(obj)
}

unsafe fn map_view(mapping: HANDLE, access: DWORD, offset: u64, len: usize, base: usize) -> usize {
    let Some(k) = handles::get(mapping) else {
        set_last_error(ERROR_INVALID_HANDLE);
        return 0;
    };
    let Kind::Mapping(m) = &*k else {
        set_last_error(ERROR_INVALID_HANDLE);
        return 0;
    };
    let len = if len == 0 { (m.size - offset.min(m.size)) as usize } else { len };
    if len == 0 {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let mut prot = libc::PROT_READ;
    if access & 2 != 0 {
        prot |= libc::PROT_WRITE;
    }
    if access & 0x20 != 0 {
        prot |= libc::PROT_EXEC;
    }
    let copy = access & 1 != 0 && access & 2 == 0;
    if copy {
        prot |= libc::PROT_WRITE;
    }
    let mut flags = if copy { libc::MAP_PRIVATE } else { libc::MAP_SHARED };
    if base != 0 {
        flags |= libc::MAP_FIXED_NOREPLACE;
    }
    let p = libc::mmap(base as *mut libc::c_void, len, prot, flags, m.fd, offset as libc::off_t);
    if p == libc::MAP_FAILED {
        set_last_error(strings::errno_to_win(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)));
        return 0;
    }
    VIEWS.lock().unwrap().get_or_insert_with(HashMap::new).insert(p as usize, len);
    p as usize
}
