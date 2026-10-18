//! Windows ABI types, constants and structure layouts (x64).
//!
//! Layouts follow the publicly documented Win32/NT x64 ABI. Everything is fixed-width:
//! Windows `LONG`/`DWORD` are 32 bits even though Linux `long` is 64.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::c_void;

pub type BOOL = i32;
pub type BYTE = u8;
pub type WORD = u16;
pub type WCHAR = u16;
pub type DWORD = u32;
pub type ULONG = u32;
pub type LONG = i32;
pub type UINT = u32;
pub type INT = i32;
pub type LONGLONG = i64;
pub type ULONGLONG = u64;
pub type DWORD64 = u64;
pub type SIZE_T = usize;
pub type ULONG_PTR = usize;
pub type NTSTATUS = i32;
pub type HANDLE = usize;
pub type HMODULE = usize;
pub type LPVOID = *mut c_void;
pub type LPCVOID = *const c_void;
pub type LPSTR = *mut u8;
pub type LPCSTR = *const u8;
pub type LPWSTR = *mut u16;
pub type LPCWSTR = *const u16;
pub type LPDWORD = *mut DWORD;
pub type FARPROC = usize;

pub const TRUE: BOOL = 1;
pub const FALSE: BOOL = 0;
pub const INVALID_HANDLE_VALUE: HANDLE = usize::MAX;
pub const INFINITE: DWORD = 0xffff_ffff;
pub const MAX_PATH: usize = 260;

pub const CURRENT_PROCESS: HANDLE = usize::MAX;
pub const CURRENT_THREAD: HANDLE = usize::MAX - 1;

// ---- error codes -------------------------------------------------------------------
pub const ERROR_SUCCESS: DWORD = 0;
pub const ERROR_INVALID_FUNCTION: DWORD = 1;
pub const ERROR_FILE_NOT_FOUND: DWORD = 2;
pub const ERROR_PATH_NOT_FOUND: DWORD = 3;
pub const ERROR_TOO_MANY_OPEN_FILES: DWORD = 4;
pub const ERROR_ACCESS_DENIED: DWORD = 5;
pub const ERROR_INVALID_HANDLE: DWORD = 6;
pub const ERROR_NOT_ENOUGH_MEMORY: DWORD = 8;
pub const ERROR_INVALID_DATA: DWORD = 13;
pub const ERROR_OUTOFMEMORY: DWORD = 14;
pub const ERROR_NOT_SAME_DEVICE: DWORD = 17;
pub const ERROR_NO_MORE_FILES: DWORD = 18;
pub const ERROR_SHARING_VIOLATION: DWORD = 32;
pub const ERROR_HANDLE_EOF: DWORD = 38;
pub const ERROR_NOT_SUPPORTED: DWORD = 50;
pub const ERROR_FILE_EXISTS: DWORD = 80;
pub const ERROR_INVALID_PARAMETER: DWORD = 87;
pub const ERROR_BROKEN_PIPE: DWORD = 109;
pub const ERROR_DISK_FULL: DWORD = 112;
pub const ERROR_CALL_NOT_IMPLEMENTED: DWORD = 120;
pub const ERROR_INSUFFICIENT_BUFFER: DWORD = 122;
pub const ERROR_INVALID_NAME: DWORD = 123;
pub const ERROR_MOD_NOT_FOUND: DWORD = 126;
pub const ERROR_PROC_NOT_FOUND: DWORD = 127;
pub const ERROR_DIR_NOT_EMPTY: DWORD = 145;
pub const ERROR_ALREADY_EXISTS: DWORD = 183;
pub const ERROR_ENVVAR_NOT_FOUND: DWORD = 203;
pub const ERROR_BAD_EXE_FORMAT: DWORD = 193;
pub const ERROR_DIRECTORY: DWORD = 267;
pub const ERROR_MORE_DATA: DWORD = 234;
pub const ERROR_NOT_OWNER: DWORD = 288;
pub const ERROR_TOO_MANY_POSTS: DWORD = 298;
pub const ERROR_OPERATION_ABORTED: DWORD = 995;
pub const ERROR_IO_PENDING: DWORD = 997;
pub const ERROR_NOACCESS: DWORD = 998;
pub const ERROR_NOT_FOUND: DWORD = 1168;
pub const ERROR_PRIVILEGE_NOT_HELD: DWORD = 1314;
pub const ERROR_TIMEOUT: DWORD = 1460;
pub const ERROR_NO_UNICODE_TRANSLATION: DWORD = 1113;
pub const ERROR_INVALID_FLAGS: DWORD = 1004;
pub const ERROR_ARITHMETIC_OVERFLOW: DWORD = 534;
pub const ERROR_INVALID_ADDRESS: DWORD = 487;
pub const ERROR_NO_MORE_ITEMS: DWORD = 259;
pub const ERROR_ALREADY_INITIALIZED: DWORD = 1247;

// ---- NTSTATUS ----------------------------------------------------------------------
pub const STATUS_SUCCESS: NTSTATUS = 0;
pub const STATUS_TIMEOUT: NTSTATUS = 0x102;
pub const STATUS_PENDING: NTSTATUS = 0x103;
pub const STATUS_BREAKPOINT: u32 = 0x8000_0003;
pub const STATUS_SINGLE_STEP: u32 = 0x8000_0004;
pub const STATUS_UNSUCCESSFUL: NTSTATUS = 0xc000_0001u32 as i32;
pub const STATUS_NOT_IMPLEMENTED: NTSTATUS = 0xc000_0002u32 as i32;
pub const STATUS_INFO_LENGTH_MISMATCH: NTSTATUS = 0xc000_0004u32 as i32;
pub const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
pub const STATUS_INVALID_HANDLE: NTSTATUS = 0xc000_0008u32 as i32;
pub const STATUS_INVALID_PARAMETER: NTSTATUS = 0xc000_000du32 as i32;
pub const STATUS_NO_MEMORY: NTSTATUS = 0xc000_0017u32 as i32;
pub const STATUS_ILLEGAL_INSTRUCTION: u32 = 0xc000_001d;
pub const STATUS_ACCESS_DENIED: NTSTATUS = 0xc000_0022u32 as i32;
pub const STATUS_BUFFER_TOO_SMALL: NTSTATUS = 0xc000_0023u32 as i32;
pub const STATUS_NONCONTINUABLE_EXCEPTION: u32 = 0xc000_0025;
pub const STATUS_INVALID_DISPOSITION: u32 = 0xc000_0026;
pub const STATUS_UNWIND: u32 = 0xc000_0027;
pub const STATUS_BAD_STACK: u32 = 0xc000_0028;
pub const STATUS_INVALID_UNWIND_TARGET: u32 = 0xc000_0029;
pub const STATUS_OBJECT_NAME_NOT_FOUND: NTSTATUS = 0xc000_0034u32 as i32;
pub const STATUS_INTEGER_DIVIDE_BY_ZERO: u32 = 0xc000_0094;
pub const STATUS_INTEGER_OVERFLOW: u32 = 0xc000_0095;
pub const STATUS_FLOAT_DIVIDE_BY_ZERO: u32 = 0xc000_008e;
pub const STATUS_FLOAT_INVALID_OPERATION: u32 = 0xc000_0090;
pub const STATUS_NOT_SUPPORTED: NTSTATUS = 0xc000_00bbu32 as i32;
pub const STATUS_STACK_OVERFLOW: u32 = 0xc000_00fd;
pub const STATUS_DLL_NOT_FOUND: NTSTATUS = 0xc000_0135u32 as i32;
pub const STATUS_ENTRYPOINT_NOT_FOUND: NTSTATUS = 0xc000_0139u32 as i32;
pub const STATUS_GCC_EXCEPTION: u32 = 0x2047_4343;
pub const STATUS_CXX_EXCEPTION: u32 = 0xe06d_7363;

// ---- memory ------------------------------------------------------------------------
pub const MEM_COMMIT: DWORD = 0x1000;
pub const MEM_RESERVE: DWORD = 0x2000;
pub const MEM_DECOMMIT: DWORD = 0x4000;
pub const MEM_RELEASE: DWORD = 0x8000;
pub const MEM_FREE: DWORD = 0x10000;
pub const MEM_PRIVATE: DWORD = 0x20000;
pub const MEM_MAPPED: DWORD = 0x40000;
pub const MEM_RESET: DWORD = 0x80000;
pub const MEM_TOP_DOWN: DWORD = 0x100000;
pub const MEM_IMAGE: DWORD = 0x1000000;

pub const PAGE_NOACCESS: DWORD = 0x01;
pub const PAGE_READONLY: DWORD = 0x02;
pub const PAGE_READWRITE: DWORD = 0x04;
pub const PAGE_WRITECOPY: DWORD = 0x08;
pub const PAGE_EXECUTE: DWORD = 0x10;
pub const PAGE_EXECUTE_READ: DWORD = 0x20;
pub const PAGE_EXECUTE_READWRITE: DWORD = 0x40;
pub const PAGE_EXECUTE_WRITECOPY: DWORD = 0x80;
pub const PAGE_GUARD: DWORD = 0x100;

pub const HEAP_ZERO_MEMORY: DWORD = 0x8;
pub const HEAP_REALLOC_IN_PLACE_ONLY: DWORD = 0x10;
pub const HEAP_GENERATE_EXCEPTIONS: DWORD = 0x4;

// ---- files -------------------------------------------------------------------------
pub const GENERIC_READ: DWORD = 0x8000_0000;
pub const GENERIC_WRITE: DWORD = 0x4000_0000;
pub const GENERIC_EXECUTE: DWORD = 0x2000_0000;
pub const GENERIC_ALL: DWORD = 0x1000_0000;
pub const FILE_APPEND_DATA: DWORD = 0x4;
pub const DELETE: DWORD = 0x10000;

pub const FILE_SHARE_READ: DWORD = 1;
pub const FILE_SHARE_WRITE: DWORD = 2;
pub const FILE_SHARE_DELETE: DWORD = 4;

pub const CREATE_NEW: DWORD = 1;
pub const CREATE_ALWAYS: DWORD = 2;
pub const OPEN_EXISTING: DWORD = 3;
pub const OPEN_ALWAYS: DWORD = 4;
pub const TRUNCATE_EXISTING: DWORD = 5;

pub const FILE_ATTRIBUTE_READONLY: DWORD = 0x1;
pub const FILE_ATTRIBUTE_HIDDEN: DWORD = 0x2;
pub const FILE_ATTRIBUTE_SYSTEM: DWORD = 0x4;
pub const FILE_ATTRIBUTE_DIRECTORY: DWORD = 0x10;
pub const FILE_ATTRIBUTE_ARCHIVE: DWORD = 0x20;
pub const FILE_ATTRIBUTE_DEVICE: DWORD = 0x40;
pub const FILE_ATTRIBUTE_NORMAL: DWORD = 0x80;
pub const FILE_ATTRIBUTE_REPARSE_POINT: DWORD = 0x400;
pub const INVALID_FILE_ATTRIBUTES: DWORD = 0xffff_ffff;

pub const FILE_FLAG_DELETE_ON_CLOSE: DWORD = 0x0400_0000;
pub const FILE_FLAG_BACKUP_SEMANTICS: DWORD = 0x0200_0000;
pub const FILE_FLAG_OVERLAPPED: DWORD = 0x4000_0000;

pub const FILE_BEGIN: DWORD = 0;
pub const FILE_CURRENT: DWORD = 1;
pub const FILE_END: DWORD = 2;

pub const FILE_TYPE_UNKNOWN: DWORD = 0;
pub const FILE_TYPE_DISK: DWORD = 1;
pub const FILE_TYPE_CHAR: DWORD = 2;
pub const FILE_TYPE_PIPE: DWORD = 3;

pub const STD_INPUT_HANDLE: DWORD = 0xffff_fff6;
pub const STD_OUTPUT_HANDLE: DWORD = 0xffff_fff5;
pub const STD_ERROR_HANDLE: DWORD = 0xffff_fff4;

pub const WAIT_OBJECT_0: DWORD = 0;
pub const WAIT_ABANDONED: DWORD = 0x80;
pub const WAIT_TIMEOUT: DWORD = 0x102;
pub const WAIT_FAILED: DWORD = 0xffff_ffff;

pub const CREATE_SUSPENDED: DWORD = 4;

pub const DLL_PROCESS_DETACH: DWORD = 0;
pub const DLL_PROCESS_ATTACH: DWORD = 1;
pub const DLL_THREAD_ATTACH: DWORD = 2;
pub const DLL_THREAD_DETACH: DWORD = 3;

pub const CP_ACP: UINT = 0;
pub const CP_OEMCP: UINT = 1;
pub const CP_UTF8: UINT = 65001;

// ---- structures --------------------------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct FILETIME {
    pub low: DWORD,
    pub high: DWORD,
}

impl FILETIME {
    pub fn from_u64(v: u64) -> Self {
        FILETIME { low: v as u32, high: (v >> 32) as u32 }
    }
    pub fn to_u64(self) -> u64 {
        (self.high as u64) << 32 | self.low as u64
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SYSTEMTIME {
    pub year: WORD,
    pub month: WORD,
    pub day_of_week: WORD,
    pub day: WORD,
    pub hour: WORD,
    pub minute: WORD,
    pub second: WORD,
    pub milliseconds: WORD,
}

#[repr(C)]
pub struct SYSTEM_INFO {
    pub processor_architecture: WORD,
    pub reserved: WORD,
    pub page_size: DWORD,
    pub min_app_address: usize,
    pub max_app_address: usize,
    pub active_processor_mask: usize,
    pub number_of_processors: DWORD,
    pub processor_type: DWORD,
    pub allocation_granularity: DWORD,
    pub processor_level: WORD,
    pub processor_revision: WORD,
}

#[repr(C)]
pub struct OVERLAPPED {
    pub internal: usize,
    pub internal_high: usize,
    pub offset: DWORD,
    pub offset_high: DWORD,
    pub h_event: HANDLE,
}

#[repr(C)]
pub struct CRITICAL_SECTION {
    pub debug_info: *mut c_void,
    pub lock_count: LONG,
    pub recursion_count: LONG,
    pub owning_thread: HANDLE,
    pub lock_semaphore: HANDLE,
    pub spin_count: usize,
}

#[repr(C)]
pub struct SECURITY_ATTRIBUTES {
    pub length: DWORD,
    pub descriptor: *mut c_void,
    pub inherit: BOOL,
}

#[repr(C)]
pub struct MEMORY_BASIC_INFORMATION {
    pub base_address: usize,
    pub allocation_base: usize,
    pub allocation_protect: DWORD,
    pub partition_id: WORD,
    pub region_size: usize,
    pub state: DWORD,
    pub protect: DWORD,
    pub type_: DWORD,
}

#[repr(C)]
pub struct MEMORYSTATUSEX {
    pub length: DWORD,
    pub memory_load: DWORD,
    pub total_phys: u64,
    pub avail_phys: u64,
    pub total_page_file: u64,
    pub avail_page_file: u64,
    pub total_virtual: u64,
    pub avail_virtual: u64,
    pub avail_extended_virtual: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WIN32_FIND_DATAW {
    pub attributes: DWORD,
    pub creation_time: FILETIME,
    pub last_access_time: FILETIME,
    pub last_write_time: FILETIME,
    pub size_high: DWORD,
    pub size_low: DWORD,
    pub reserved0: DWORD,
    pub reserved1: DWORD,
    pub file_name: [WCHAR; MAX_PATH],
    pub alternate_file_name: [WCHAR; 14],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WIN32_FIND_DATAA {
    pub attributes: DWORD,
    pub creation_time: FILETIME,
    pub last_access_time: FILETIME,
    pub last_write_time: FILETIME,
    pub size_high: DWORD,
    pub size_low: DWORD,
    pub reserved0: DWORD,
    pub reserved1: DWORD,
    pub file_name: [u8; MAX_PATH],
    pub alternate_file_name: [u8; 14],
}

#[repr(C)]
pub struct BY_HANDLE_FILE_INFORMATION {
    pub attributes: DWORD,
    pub creation_time: FILETIME,
    pub last_access_time: FILETIME,
    pub last_write_time: FILETIME,
    pub volume_serial: DWORD,
    pub size_high: DWORD,
    pub size_low: DWORD,
    pub nlinks: DWORD,
    pub index_high: DWORD,
    pub index_low: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct WIN32_FILE_ATTRIBUTE_DATA {
    pub attributes: DWORD,
    pub creation_time: FILETIME,
    pub last_access_time: FILETIME,
    pub last_write_time: FILETIME,
    pub size_high: DWORD,
    pub size_low: DWORD,
}

#[repr(C)]
pub struct UNICODE_STRING {
    pub length: u16,
    pub maximum_length: u16,
    pub buffer: *mut u16,
}

#[repr(C)]
pub struct STARTUPINFOW {
    pub cb: DWORD,
    pub reserved: LPWSTR,
    pub desktop: LPWSTR,
    pub title: LPWSTR,
    pub x: DWORD,
    pub y: DWORD,
    pub x_size: DWORD,
    pub y_size: DWORD,
    pub x_count_chars: DWORD,
    pub y_count_chars: DWORD,
    pub fill_attribute: DWORD,
    pub flags: DWORD,
    pub show_window: WORD,
    pub cb_reserved2: WORD,
    pub reserved2: *mut u8,
    pub std_input: HANDLE,
    pub std_output: HANDLE,
    pub std_error: HANDLE,
}

#[repr(C)]
pub struct STARTUPINFOA {
    pub cb: DWORD,
    pub reserved: LPSTR,
    pub desktop: LPSTR,
    pub title: LPSTR,
    pub x: DWORD,
    pub y: DWORD,
    pub x_size: DWORD,
    pub y_size: DWORD,
    pub x_count_chars: DWORD,
    pub y_count_chars: DWORD,
    pub fill_attribute: DWORD,
    pub flags: DWORD,
    pub show_window: WORD,
    pub cb_reserved2: WORD,
    pub reserved2: *mut u8,
    pub std_input: HANDLE,
    pub std_output: HANDLE,
    pub std_error: HANDLE,
}

#[repr(C)]
pub struct PROCESS_INFORMATION {
    pub process: HANDLE,
    pub thread: HANDLE,
    pub process_id: DWORD,
    pub thread_id: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct COORD {
    pub x: i16,
    pub y: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SMALL_RECT {
    pub left: i16,
    pub top: i16,
    pub right: i16,
    pub bottom: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CONSOLE_SCREEN_BUFFER_INFO {
    pub size: COORD,
    pub cursor: COORD,
    pub attributes: WORD,
    pub window: SMALL_RECT,
    pub max_window_size: COORD,
}

#[repr(C)]
pub struct OSVERSIONINFOW {
    pub size: DWORD,
    pub major: DWORD,
    pub minor: DWORD,
    pub build: DWORD,
    pub platform: DWORD,
    pub csd: [WCHAR; 128],
}

#[repr(C)]
pub struct OSVERSIONINFOEXW {
    pub size: DWORD,
    pub major: DWORD,
    pub minor: DWORD,
    pub build: DWORD,
    pub platform: DWORD,
    pub csd: [WCHAR; 128],
    pub sp_major: WORD,
    pub sp_minor: WORD,
    pub suite_mask: WORD,
    pub product_type: u8,
    pub reserved: u8,
}

#[repr(C)]
pub struct TIME_ZONE_INFORMATION {
    pub bias: LONG,
    pub standard_name: [WCHAR; 32],
    pub standard_date: SYSTEMTIME,
    pub standard_bias: LONG,
    pub daylight_name: [WCHAR; 32],
    pub daylight_date: SYSTEMTIME,
    pub daylight_bias: LONG,
}

// ---- x64 exception structures --------------------------------------------------------
#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
pub struct M128A {
    pub low: u64,
    pub high: i64,
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct CONTEXT {
    pub p1_home: u64,
    pub p2_home: u64,
    pub p3_home: u64,
    pub p4_home: u64,
    pub p5_home: u64,
    pub p6_home: u64,
    pub context_flags: DWORD,
    pub mxcsr: DWORD,
    pub seg_cs: WORD,
    pub seg_ds: WORD,
    pub seg_es: WORD,
    pub seg_fs: WORD,
    pub seg_gs: WORD,
    pub seg_ss: WORD,
    pub eflags: DWORD,
    pub dr0: u64,
    pub dr1: u64,
    pub dr2: u64,
    pub dr3: u64,
    pub dr6: u64,
    pub dr7: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rbx: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    /// XMM_SAVE_AREA32 (512 bytes); xmm0..15 start at offset 0xa0 inside.
    pub flt_save: [u8; 512],
    pub vector_register: [M128A; 26],
    pub vector_control: u64,
    pub debug_control: u64,
    pub last_branch_to_rip: u64,
    pub last_branch_from_rip: u64,
    pub last_exception_to_rip: u64,
    pub last_exception_from_rip: u64,
}

pub const CONTEXT_AMD64: DWORD = 0x0010_0000;
pub const CONTEXT_CONTROL: DWORD = CONTEXT_AMD64 | 0x1;
pub const CONTEXT_INTEGER: DWORD = CONTEXT_AMD64 | 0x2;
pub const CONTEXT_SEGMENTS: DWORD = CONTEXT_AMD64 | 0x4;
pub const CONTEXT_FLOATING_POINT: DWORD = CONTEXT_AMD64 | 0x8;
pub const CONTEXT_FULL: DWORD = CONTEXT_CONTROL | CONTEXT_INTEGER | CONTEXT_FLOATING_POINT;
pub const CONTEXT_ALL: DWORD = CONTEXT_FULL | CONTEXT_SEGMENTS;

impl CONTEXT {
    pub fn zeroed() -> CONTEXT {
        unsafe { std::mem::zeroed() }
    }
    pub fn xmm(&self, i: usize) -> &[u8] {
        &self.flt_save[0xa0 + i * 16..0xa0 + i * 16 + 16]
    }
    pub fn xmm_mut(&mut self, i: usize) -> &mut [u8] {
        &mut self.flt_save[0xa0 + i * 16..0xa0 + i * 16 + 16]
    }
}

pub const EXCEPTION_MAXIMUM_PARAMETERS: usize = 15;

#[repr(C)]
pub struct EXCEPTION_RECORD {
    pub code: DWORD,
    pub flags: DWORD,
    pub record: *mut EXCEPTION_RECORD,
    pub address: *mut c_void,
    pub number_parameters: DWORD,
    pub information: [usize; EXCEPTION_MAXIMUM_PARAMETERS],
}

#[repr(C)]
pub struct EXCEPTION_POINTERS {
    pub record: *mut EXCEPTION_RECORD,
    pub context: *mut CONTEXT,
}

pub const EXCEPTION_NONCONTINUABLE: DWORD = 0x1;
pub const EXCEPTION_UNWINDING: DWORD = 0x2;
pub const EXCEPTION_EXIT_UNWIND: DWORD = 0x4;
pub const EXCEPTION_STACK_INVALID: DWORD = 0x8;
pub const EXCEPTION_NESTED_CALL: DWORD = 0x10;
pub const EXCEPTION_TARGET_UNWIND: DWORD = 0x20;
pub const EXCEPTION_COLLIDED_UNWIND: DWORD = 0x40;

pub const EXCEPTION_EXECUTE_HANDLER: i32 = 1;
pub const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
pub const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RUNTIME_FUNCTION {
    pub begin: DWORD,
    pub end: DWORD,
    pub unwind_info: DWORD,
}

// ---- TEB / PEB ------------------------------------------------------------------------
pub const TEB_SIZE: usize = 0x2000;
pub const TEB_STACK_BASE: usize = 0x08;
pub const TEB_STACK_LIMIT: usize = 0x10;
pub const TEB_ARBITRARY_USER_PTR: usize = 0x28;
pub const TEB_SELF: usize = 0x30;
pub const TEB_PID: usize = 0x40;
pub const TEB_TID: usize = 0x48;
pub const TEB_TLS_POINTER: usize = 0x58;
pub const TEB_PEB: usize = 0x60;
pub const TEB_LAST_ERROR: usize = 0x68;
pub const TEB_TLS_SLOTS: usize = 0x1480;
pub const TEB_FLS_DATA: usize = 0x17c8;

pub const PEB_IMAGE_BASE: usize = 0x10;
pub const PEB_LDR: usize = 0x18;
pub const PEB_PROCESS_PARAMETERS: usize = 0x20;
pub const PEB_PROCESS_HEAP: usize = 0x30;
pub const PEB_NUMBER_OF_PROCESSORS: usize = 0xb8;
pub const PEB_OS_MAJOR: usize = 0x118;
pub const PEB_OS_MINOR: usize = 0x11c;
pub const PEB_OS_BUILD: usize = 0x120;
pub const PEB_OS_PLATFORM: usize = 0x124;
pub const PEB_SIZE: usize = 0x800;

pub const KUSER_SHARED_DATA: usize = 0x7ffe_0000;

pub const TLS_MINIMUM_AVAILABLE: usize = 64;

/// Reads the current thread's TEB pointer (the guest sets `gs` base to it).
#[inline(always)]
pub fn teb() -> *mut u8 {
    let t: usize;
    unsafe { std::arch::asm!("mov {}, gs:[0x30]", out(reg) t, options(nostack, preserves_flags, readonly)) };
    t as *mut u8
}

#[inline(always)]
pub fn last_error() -> DWORD {
    unsafe { (teb().add(TEB_LAST_ERROR) as *const DWORD).read() }
}

#[inline(always)]
pub fn set_last_error(e: DWORD) {
    unsafe { (teb().add(TEB_LAST_ERROR) as *mut DWORD).write(e) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn context_layout_matches_x64_abi() {
        assert_eq!(size_of::<CONTEXT>(), 0x4d0);
        assert_eq!(offset_of!(CONTEXT, context_flags), 0x30);
        assert_eq!(offset_of!(CONTEXT, mxcsr), 0x34);
        assert_eq!(offset_of!(CONTEXT, eflags), 0x44);
        assert_eq!(offset_of!(CONTEXT, rax), 0x78);
        assert_eq!(offset_of!(CONTEXT, rsp), 0x98);
        assert_eq!(offset_of!(CONTEXT, rip), 0xf8);
        assert_eq!(offset_of!(CONTEXT, flt_save), 0x100);
        assert_eq!(offset_of!(CONTEXT, vector_register), 0x300);
    }

    #[test]
    fn struct_sizes() {
        assert_eq!(size_of::<EXCEPTION_RECORD>(), 152);
        assert_eq!(size_of::<CRITICAL_SECTION>(), 40);
        assert_eq!(size_of::<SYSTEM_INFO>(), 48);
        assert_eq!(size_of::<WIN32_FIND_DATAW>(), 592);
        assert_eq!(size_of::<WIN32_FIND_DATAA>(), 320);
        assert_eq!(size_of::<MEMORY_BASIC_INFORMATION>(), 48);
        assert_eq!(size_of::<MEMORYSTATUSEX>(), 64);
        assert_eq!(size_of::<STARTUPINFOW>(), 104);
        assert_eq!(size_of::<OVERLAPPED>(), 32);
        assert_eq!(size_of::<BY_HANDLE_FILE_INFORMATION>(), 52);
        assert_eq!(size_of::<TIME_ZONE_INFORMATION>(), 172);
        assert_eq!(size_of::<OSVERSIONINFOEXW>(), 284);
    }
}
