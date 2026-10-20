//! ntdll: context capture, version info, status mapping, heap, memory helpers and the small set of
//! `Nt*` system calls user programs reach directly. kernel32 imports fall back to this table.

use crate::runtime::handles::{self, Kind};
use crate::runtime::strings;
use crate::runtime::teb;
use crate::win::*;
use std::arch::naked_asm;
use std::ffi::c_void;
use std::sync::atomic::Ordering;

#[unsafe(naked)]
pub unsafe extern "win64" fn rtl_capture_context(_ctx: *mut CONTEXT) {
    naked_asm!(
        "mov [rcx+0x78], rax",
        "mov [rcx+0x80], rcx",
        "mov [rcx+0x88], rdx",
        "mov [rcx+0x90], rbx",
        "lea rax, [rsp+8]",
        "mov [rcx+0x98], rax",
        "mov [rcx+0xa0], rbp",
        "mov [rcx+0xa8], rsi",
        "mov [rcx+0xb0], rdi",
        "mov [rcx+0xb8], r8",
        "mov [rcx+0xc0], r9",
        "mov [rcx+0xc8], r10",
        "mov [rcx+0xd0], r11",
        "mov [rcx+0xd8], r12",
        "mov [rcx+0xe0], r13",
        "mov [rcx+0xe8], r14",
        "mov [rcx+0xf0], r15",
        "mov rax, [rsp]",
        "mov [rcx+0xf8], rax",
        "mov dword ptr [rcx+0x30], 0x10001f",
        "pushfq",
        "pop rax",
        "mov [rcx+0x44], eax",
        "mov ax, cs",
        "mov [rcx+0x38], ax",
        "mov ax, ds",
        "mov [rcx+0x3a], ax",
        "mov ax, es",
        "mov [rcx+0x3c], ax",
        "mov ax, fs",
        "mov [rcx+0x3e], ax",
        "mov ax, gs",
        "mov [rcx+0x40], ax",
        "mov ax, ss",
        "mov [rcx+0x42], ax",
        "stmxcsr [rcx+0x34]",
        "movups [rcx+0x1a0], xmm0",
        "movups [rcx+0x1b0], xmm1",
        "movups [rcx+0x1c0], xmm2",
        "movups [rcx+0x1d0], xmm3",
        "movups [rcx+0x1e0], xmm4",
        "movups [rcx+0x1f0], xmm5",
        "movups [rcx+0x200], xmm6",
        "movups [rcx+0x210], xmm7",
        "movups [rcx+0x220], xmm8",
        "movups [rcx+0x230], xmm9",
        "movups [rcx+0x240], xmm10",
        "movups [rcx+0x250], xmm11",
        "movups [rcx+0x260], xmm12",
        "movups [rcx+0x270], xmm13",
        "movups [rcx+0x280], xmm14",
        "movups [rcx+0x290], xmm15",
        "ret",
    );
}

pub unsafe fn capture_context(ctx: *mut CONTEXT) {
    rtl_capture_context(ctx);
}

/// NTSTATUS and Win32 error pairs; the first pair for a Win32 error is the status it comes back as.
const NT_DOS: &[(u32, DWORD)] = &[
    (0, ERROR_SUCCESS),
    (0xc000_0022, ERROR_ACCESS_DENIED),
    (0xc000_0034, ERROR_FILE_NOT_FOUND),
    (0xc000_000f, ERROR_FILE_NOT_FOUND),
    (0xc000_003a, ERROR_PATH_NOT_FOUND),
    (0xc000_0035, ERROR_ALREADY_EXISTS),
    (0xc000_0033, 123),
    (0xc000_0043, 32),
    (0xc000_0056, ERROR_ACCESS_DENIED),
    (0xc000_00ba, ERROR_ACCESS_DENIED),
    (0xc000_0101, ERROR_DIR_NOT_EMPTY),
    (0xc000_0103, 267),
    (0xc000_00d4, 17),
    (0xc000_007f, 112),
    (0xc000_0011, 38),
    (0xc000_014b, 109),
    (0xc000_0120, 995),
    (0xc000_0005, 998),
    (0xc000_000d, ERROR_INVALID_PARAMETER),
    (0xc000_0008, ERROR_INVALID_HANDLE),
    (0xc000_0017, ERROR_NOT_ENOUGH_MEMORY),
    (0xc000_0023, ERROR_INSUFFICIENT_BUFFER),
    (0xc000_00bb, ERROR_NOT_SUPPORTED),
    (0xc000_0002, ERROR_CALL_NOT_IMPLEMENTED),
    (0xc000_0135, ERROR_MOD_NOT_FOUND),
    (0xc000_0139, ERROR_PROC_NOT_FOUND),
    (0xc000_0001, 31),
    (0x8000_0005, 234),
    (0x8000_0006, 18),
    (0x102, WAIT_TIMEOUT),
];

pub(crate) fn nt_to_dos(status: u32) -> DWORD {
    // a Win32 error wrapped into a status (FACILITY_NTWIN32), as programs do to carry one through an OVERLAPPED
    if status & 0xffff_0000 == 0x8007_0000 {
        return status & 0xffff;
    }
    NT_DOS.iter().find(|p| p.0 == status).map_or_else(
        || {
            if crate::runtime::TRACE_ERR.load(std::sync::atomic::Ordering::Relaxed) {
                let bt = std::backtrace::Backtrace::force_capture().to_string();
                let who: Vec<&str> = bt.lines().filter(|l| l.contains("nle::shim::")).take(3).map(|l| l.trim()).collect();
                eprintln!("nle[err]: NTSTATUS {status:#010x} has no Win32 error ({})", who.join(" <- "));
            }
            ERROR_INVALID_FUNCTION
        },
        |p| p.1,
    )
}

/// The status a Win32 error stands for.
pub(crate) fn dos_to_nt(error: DWORD) -> u32 {
    NT_DOS.iter().find(|p| p.1 == error).map_or(0xc000_0001, |p| p.0)
}

crate::shims! { "ntdll";
    fn RtlGetVersion(v: *mut OSVERSIONINFOEXW) -> NTSTATUS {
        (*v).major = 10;
        (*v).minor = 0;
        (*v).build = 19045;
        (*v).platform = 2;
        if (*v).size as usize >= std::mem::size_of::<OSVERSIONINFOEXW>() {
            (*v).product_type = 1;
        }
        STATUS_SUCCESS
    }
    fn RtlGetNtVersionNumbers(major: *mut DWORD, minor: *mut DWORD, build: *mut DWORD) {
        if !major.is_null() {
            *major = 10;
        }
        if !minor.is_null() {
            *minor = 0;
        }
        if !build.is_null() {
            *build = 0xf000_4a65;
        }
    }
    fn RtlNtStatusToDosError(status: u32) -> DWORD {
        nt_to_dos(status)
    }
    fn RtlGetCurrentPeb() -> usize {
        teb::peb()
    }
    fn RtlGetLastWin32Error() -> DWORD {
        last_error()
    }
    fn RtlSetLastWin32Error(e: DWORD) {
        set_last_error(e);
    }
    fn RtlIsProcessorFeaturePresent(f: DWORD) -> u8 {
        (f < 64 && teb::processor_features()[f as usize]) as u8
    }
    fn RtlExitUserProcess(code: u32) -> ! {
        crate::loader::exit_process(code)
    }
    fn RtlExitUserThread(code: u32) -> ! {
        crate::shim::k32_thread::ExitThread(code)
    }

    fn RtlMoveMemory(dst: *mut u8, src: *const u8, n: usize) {
        std::ptr::copy(src, dst, n);
    }
    fn RtlCopyMemory(dst: *mut u8, src: *const u8, n: usize) {
        std::ptr::copy(src, dst, n);
    }
    fn RtlFillMemory(dst: *mut u8, n: usize, v: u8) {
        std::ptr::write_bytes(dst, v, n);
    }
    fn RtlZeroMemory(dst: *mut u8, n: usize) {
        std::ptr::write_bytes(dst, 0, n);
    }
    fn RtlSecureZeroMemory(dst: *mut u8, n: usize) -> *mut u8 {
        std::ptr::write_bytes(dst, 0, n);
        dst
    }
    fn RtlCompareMemory(a: *const u8, b: *const u8, n: usize) -> usize {
        let mut i = 0;
        while i < n && *a.add(i) == *b.add(i) {
            i += 1;
        }
        i
    }
    fn RtlInitUnicodeString(dst: *mut UNICODE_STRING, src: LPCWSTR) {
        let n = strings::wlen(src) * 2;
        (*dst).length = n as u16;
        (*dst).maximum_length = if src.is_null() { 0 } else { n as u16 + 2 };
        (*dst).buffer = src as *mut u16;
    }

    fn RtlCreateHeap(_flags: ULONG, _base: usize, _reserve: usize, _commit: usize, _lock: usize, _params: usize) -> HANDLE {
        crate::shim::k32_mem::HeapCreate(0, 0, 0)
    }
    fn RtlDestroyHeap(_h: HANDLE) -> HANDLE {
        0
    }
    fn RtlAllocateHeap(h: HANDLE, flags: ULONG, size: usize) -> *mut u8 {
        crate::shim::k32_mem::HeapAlloc(h, flags, size)
    }
    fn RtlReAllocateHeap(h: HANDLE, flags: ULONG, p: *mut u8, size: usize) -> *mut u8 {
        crate::shim::k32_mem::HeapReAlloc(h, flags, p, size)
    }
    fn RtlFreeHeap(h: HANDLE, flags: ULONG, p: *mut u8) -> u8 {
        crate::shim::k32_mem::HeapFree(h, flags, p) as u8
    }
    fn RtlSizeHeap(h: HANDLE, flags: ULONG, p: *const u8) -> usize {
        crate::shim::k32_mem::HeapSize(h, flags, p)
    }

    fn NtClose(h: HANDLE) -> NTSTATUS {
        if handles::close(h) { STATUS_SUCCESS } else { STATUS_INVALID_HANDLE }
    }
    fn NtYieldExecution() -> NTSTATUS {
        libc::sched_yield();
        STATUS_SUCCESS
    }
    fn NtDelayExecution(_alertable: u8, interval: *const i64) -> NTSTATUS {
        let t = *interval;
        let ns: u64 = if t < 0 { (-(t as i128) * 100) as u64 } else { 0 };
        std::thread::sleep(std::time::Duration::from_nanos(ns));
        STATUS_SUCCESS
    }
    fn NtQuerySystemTime(out: *mut u64) -> NTSTATUS {
        *out = teb::filetime_now();
        STATUS_SUCCESS
    }
    fn NtQueryPerformanceCounter(counter: *mut i64, freq: *mut i64) -> NTSTATUS {
        *counter = (handles::now_ns() / 100) as i64;
        if !freq.is_null() {
            *freq = 10_000_000;
        }
        STATUS_SUCCESS
    }
    fn NtTerminateProcess(h: HANDLE, status: u32) -> NTSTATUS {
        if h == CURRENT_PROCESS || h == 0 {
            crate::loader::exit_process(status);
        }
        STATUS_INVALID_HANDLE
    }
    fn NtWaitForSingleObject(h: HANDLE, _alertable: u8, timeout: *const i64) -> NTSTATUS {
        let ms = if timeout.is_null() {
            INFINITE
        } else if *timeout < 0 {
            ((-(*timeout as i128)) / 10_000) as u32
        } else {
            0
        };
        match crate::shim::k32_sync::WaitForSingleObject(h, ms) {
            WAIT_OBJECT_0 => STATUS_SUCCESS,
            WAIT_TIMEOUT => STATUS_TIMEOUT,
            _ => STATUS_INVALID_HANDLE,
        }
    }
    fn NtQueryInformationProcess(_h: HANDLE, class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> NTSTATUS {
        match class {
            0 if len >= 48 => {
                std::ptr::write_bytes(buf, 0, 48);
                *(buf.add(8) as *mut usize) = teb::peb();
                *(buf.add(32) as *mut usize) = libc::getpid() as usize;
                if !ret.is_null() {
                    *ret = 48;
                }
                STATUS_SUCCESS
            }
            7 | 30 | 31 if len >= 8 => {
                std::ptr::write_bytes(buf, 0, len as usize);
                if !ret.is_null() {
                    *ret = 8;
                }
                STATUS_SUCCESS
            }
            _ => STATUS_INFO_LENGTH_MISMATCH,
        }
    }
    fn NtSetInformationThread(_h: HANDLE, _class: u32, _buf: *const c_void, _len: u32) -> NTSTATUS {
        STATUS_SUCCESS
    }
    fn NtSetInformationProcess(_h: HANDLE, _class: u32, _buf: *const c_void, _len: u32) -> NTSTATUS {
        STATUS_SUCCESS
    }
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> NTSTATUS {
        match class {
            0 if len >= 64 => {
                std::ptr::write_bytes(buf, 0, 64);
                *(buf.add(8) as *mut u32) = 4096;
                *(buf.add(12) as *mut u32) = 4096;
                *(buf.add(40) as *mut u32) = 4096;
                *(buf.add(44) as *mut usize) = 0x10000;
                *(buf.add(52) as *mut u8) = crate::runtime::cpu_count() as u8;
                if !ret.is_null() {
                    *ret = 64;
                }
                STATUS_SUCCESS
            }
            // SystemProcessorPerformanceInformation: per processor idle, kernel (idle included), user, DPC and interrupt
            // time in 100 ns units, from /proc/stat's jiffies
            8 => {
                let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
                let mut out: Vec<u8> = Vec::new();
                for line in stat.lines() {
                    let mut it = line.split_whitespace();
                    let Some(name) = it.next() else { continue };
                    if !name.starts_with("cpu") || name == "cpu" {
                        continue;
                    }
                    let v: Vec<i64> = it.take(7).filter_map(|x| x.parse().ok()).collect();
                    if v.len() < 7 {
                        continue;
                    }
                    let unit = 100_000i64;
                    let idle = (v[3] + v[4]) * unit;
                    let user = (v[0] + v[1]) * unit;
                    let kernel = (v[2] + v[3] + v[4] + v[5] + v[6]) * unit;
                    for t in [idle, kernel, user, v[6] * unit, v[5] * unit, 0] {
                        out.extend_from_slice(&t.to_le_bytes());
                    }
                }
                if !ret.is_null() {
                    *ret = out.len() as u32;
                }
                if (len as usize) < out.len() {
                    return STATUS_INFO_LENGTH_MISMATCH;
                }
                std::ptr::copy_nonoverlapping(out.as_ptr(), buf, out.len());
                STATUS_SUCCESS
            }
            // SystemTimeOfDayInformation: boot time, current time, zero bias
            3 if len >= 48 => {
                std::ptr::write_bytes(buf, 0, 48);
                let now = teb::filetime_now();
                let up = std::fs::read_to_string("/proc/uptime").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok())).unwrap_or(0.0);
                *(buf as *mut u64) = now.saturating_sub((up * 1e7) as u64);
                *(buf.add(8) as *mut u64) = now;
                if !ret.is_null() {
                    *ret = 48;
                }
                STATUS_SUCCESS
            }
            _ => STATUS_NOT_IMPLEMENTED,
        }
    }
}

crate::shim_extra!("ntdll"; "RtlCaptureContext" => rtl_capture_context);

#[allow(dead_code)]
fn _unused() {
    let _ = Ordering::SeqCst;
    let _: Option<Kind> = None;
}
