//! TEB / PEB / KUSER_SHARED_DATA emulation and the guest-call trampoline.
//!
//! Windows x64 code reaches its thread block through `gs:`. Linux leaves `gs` free, so each
//! thread gets a TEB page and `arch_prctl(ARCH_SET_GS)` points `gs` at it. `fs` stays glibc's.

use crate::runtime::{self, strings};
use crate::win::*;
use std::arch::global_asm;
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

const ARCH_SET_GS: libc::c_long = 0x1001;
const TLS_ARRAY_SLOTS: usize = 256;
const ALT_STACK_SIZE: usize = 128 * 1024;
const TEB_ENVIRONMENT_POINTER: usize = 0x38;

static PEB: AtomicUsize = AtomicUsize::new(0);
static PROCESS_HEAP: AtomicUsize = AtomicUsize::new(0);

pub fn peb() -> usize {
    PEB.load(Ordering::SeqCst)
}

/// Token used as the `HANDLE` of the default process heap.
pub fn process_heap() -> usize {
    PROCESS_HEAP.load(Ordering::SeqCst)
}

unsafe fn map_zeroed(size: usize) -> *mut u8 {
    let p = libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
    if p == libc::MAP_FAILED {
        eprintln!("anyexe: out of memory allocating {size} bytes");
        libc::_exit(70);
    }
    p as *mut u8
}

unsafe fn put<T>(base: *mut u8, off: usize, v: T) {
    (base.add(off) as *mut T).write_unaligned(v);
}

fn leak_wide(s: &str) -> *mut u16 {
    let v = strings::to_wide_nul(s).into_boxed_slice();
    Box::leak(v).as_mut_ptr()
}

fn leak_env_block() -> *mut u16 {
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in runtime::ENV.lock().unwrap().iter() {
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    Box::leak(block.into_boxed_slice()).as_mut_ptr()
}

unsafe fn unicode_string(base: *mut u8, off: usize, s: &str) {
    let w = leak_wide(s);
    let len = (strings::wlen(w) * 2) as u16;
    put::<u16>(base, off, len);
    put::<u16>(base, off + 2, len + 2);
    put::<usize>(base, off + 8, w as usize);
}

/// Allocates the process-wide PEB. Call once, before the first thread block is created.
pub unsafe fn init_peb(image_base: usize) {
    let peb = map_zeroed(PEB_SIZE);
    let heap = map_zeroed(4096) as usize;
    PROCESS_HEAP.store(heap, Ordering::SeqCst);

    put::<usize>(peb, PEB_IMAGE_BASE, image_base);
    put::<usize>(peb, PEB_PROCESS_HEAP, heap);
    put::<u32>(peb, PEB_NUMBER_OF_PROCESSORS, runtime::cpu_count() as u32);
    put::<u32>(peb, PEB_OS_MAJOR, 10);
    put::<u32>(peb, PEB_OS_MINOR, 0);
    put::<u16>(peb, PEB_OS_BUILD, 19045);
    put::<u32>(peb, PEB_OS_PLATFORM, 2);

    // PEB_LDR_DATA with empty, self-linked module lists.
    let ldr = map_zeroed(4096);
    put::<u32>(ldr, 0, 0x58);
    put::<u8>(ldr, 4, 1);
    for off in [0x10usize, 0x20, 0x30] {
        put::<usize>(ldr, off, ldr as usize + off);
        put::<usize>(ldr, off + 8, ldr as usize + off);
    }
    put::<usize>(peb, PEB_LDR, ldr as usize);

    // RTL_USER_PROCESS_PARAMETERS
    let pp = map_zeroed(4096);
    let c = runtime::cfg();
    put::<u32>(pp, 0, 0x400);
    put::<u32>(pp, 4, 0x400);
    put::<u32>(pp, 8, 1);
    unicode_string(pp, 0x38, &format!("{}\\", runtime::paths::cwd_win().trim_end_matches('\\')));
    unicode_string(pp, 0x60, &c.exe_win);
    unicode_string(pp, 0x70, &runtime::command_line());
    put::<usize>(pp, 0x80, leak_env_block() as usize);
    put::<usize>(peb, PEB_PROCESS_PARAMETERS, pp as usize);

    PEB.store(peb as usize, Ordering::SeqCst);
}

/// Stack bounds `(high, low)` of the calling thread.
pub unsafe fn current_stack_bounds() -> (usize, usize) {
    let mut attr: libc::pthread_attr_t = std::mem::zeroed();
    if libc::pthread_getattr_np(libc::pthread_self(), &mut attr) == 0 {
        let mut addr: *mut libc::c_void = std::ptr::null_mut();
        let mut size: libc::size_t = 0;
        libc::pthread_attr_getstack(&attr, &mut addr, &mut size);
        libc::pthread_attr_destroy(&mut attr);
        if !addr.is_null() {
            return (addr as usize + size, addr as usize);
        }
    }
    let sp: usize;
    std::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack, preserves_flags));
    (sp + 0x10000, sp.saturating_sub(0x100000))
}

/// Creates a TEB for the calling thread and points `gs` at it.
pub unsafe fn setup_thread() -> *mut u8 {
    let teb = map_zeroed(TEB_SIZE);
    let (hi, lo) = current_stack_bounds();
    put::<usize>(teb, TEB_STACK_BASE, hi);
    put::<usize>(teb, TEB_STACK_LIMIT, lo);
    put::<usize>(teb, TEB_SELF, teb as usize);
    put::<u64>(teb, TEB_PID, libc::getpid() as u64);
    put::<u64>(teb, TEB_TID, runtime::handles::current_tid() as u64);
    put::<usize>(teb, TEB_PEB, peb());
    let tls = libc::calloc(TLS_ARRAY_SLOTS, 8) as usize;
    put::<usize>(teb, TEB_TLS_POINTER, tls);
    libc::syscall(libc::SYS_arch_prctl, ARCH_SET_GS, teb as libc::c_long);
    let alt = libc::mmap(std::ptr::null_mut(), ALT_STACK_SIZE, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
    if alt != libc::MAP_FAILED {
        let ss = libc::stack_t { ss_sp: alt, ss_flags: 0, ss_size: ALT_STACK_SIZE };
        libc::sigaltstack(&ss, std::ptr::null_mut());
        put::<usize>(teb, TEB_ENVIRONMENT_POINTER, alt as usize);
    }
    teb
}

pub unsafe fn teardown_thread(teb: *mut u8) {
    let alt = (teb.add(TEB_ENVIRONMENT_POINTER) as *const usize).read();
    if alt != 0 {
        let ss = libc::stack_t { ss_sp: std::ptr::null_mut(), ss_flags: libc::SS_DISABLE, ss_size: 0 };
        libc::sigaltstack(&ss, std::ptr::null_mut());
        libc::munmap(alt as *mut libc::c_void, ALT_STACK_SIZE);
    }
    let tls = (teb.add(TEB_TLS_POINTER) as *const usize).read();
    if tls != 0 {
        for i in 0..TLS_ARRAY_SLOTS {
            let blk = *(tls as *const usize).add(i);
            if blk != 0 {
                libc::free(blk as *mut libc::c_void);
            }
        }
        libc::free(tls as *mut libc::c_void);
    }
    let fls = (teb.add(TEB_FLS_DATA) as *const usize).read();
    if fls != 0 {
        libc::free(fls as *mut libc::c_void);
    }
    libc::munmap(teb as *mut libc::c_void, TEB_SIZE);
}

pub const TLS_ARRAY_LEN: usize = TLS_ARRAY_SLOTS;

/// The `ThreadLocalStoragePointer` array of the calling thread (one slot per module with static TLS).
pub unsafe fn tls_array() -> *mut usize {
    (teb().add(TEB_TLS_POINTER) as *const usize).read() as *mut usize
}

// ---- KUSER_SHARED_DATA ----------------------------------------------------------------------
const UNIX_TO_FILETIME: u64 = 11_644_473_600;

fn boottime_100ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    ts.tv_sec as u64 * 10_000_000 + ts.tv_nsec as u64 / 100
}

pub fn filetime_now() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };
    (ts.tv_sec as u64 + UNIX_TO_FILETIME) * 10_000_000 + ts.tv_nsec as u64 / 100
}

unsafe fn write_ksystem_time(base: *mut u8, off: usize, v: u64) {
    let hi = (v >> 32) as i32;
    put::<i32>(base, off + 8, hi);
    put::<u32>(base, off, v as u32);
    put::<i32>(base, off + 4, hi);
}

unsafe fn kuser_update(base: *mut u8) {
    let t = boottime_100ns();
    write_ksystem_time(base, 0x08, t);
    write_ksystem_time(base, 0x14, filetime_now());
    put::<u64>(base, 0x320, t / 10_000);
}

/// Maps the shared-data page at its fixed Windows address. Returns false if the address is taken.
pub unsafe fn init_kuser() -> bool {
    let p = libc::mmap(
        KUSER_SHARED_DATA as *mut libc::c_void,
        4096,
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED_NOREPLACE,
        -1,
        0,
    );
    if p as usize != KUSER_SHARED_DATA {
        return false;
    }
    let b = p as *mut u8;
    put::<u32>(b, 0x004, 1 << 24);
    put::<u16>(b, 0x02c, 0x014c);
    put::<u16>(b, 0x02e, 0x8664);
    let root = strings::to_wide_nul("C:\\windows");
    std::ptr::copy_nonoverlapping(root.as_ptr(), b.add(0x30) as *mut u16, root.len());
    put::<u32>(b, 0x260, 19045);
    put::<u32>(b, 0x264, 1);
    put::<u8>(b, 0x268, 1);
    put::<u16>(b, 0x26a, 9);
    put::<u32>(b, 0x26c, 10);
    put::<u32>(b, 0x270, 0);
    for (i, present) in processor_features().iter().enumerate() {
        put::<u8>(b, 0x274 + i, *present as u8);
    }
    put::<u64>(b, 0x2c8, 10_000_000);
    put::<u32>(b, 0x3c0, runtime::cpu_count() as u32);
    kuser_update(b);
    true
}

/// Keeps the clock fields of KUSER_SHARED_DATA current (some runtimes read time from there directly).
pub fn start_kuser_updater() {
    let _ = std::thread::Builder::new().name("anyexe-kuser".into()).spawn(|| loop {
        unsafe { kuser_update(KUSER_SHARED_DATA as *mut u8) };
        std::thread::sleep(std::time::Duration::from_millis(1));
    });
}

/// `PF_*` indexes as used by `IsProcessorFeaturePresent`.
pub fn processor_features() -> [bool; 64] {
    let mut f = [false; 64];
    f[2] = true;
    f[3] = is_x86_feature_detected!("mmx");
    f[6] = is_x86_feature_detected!("sse");
    f[8] = true;
    f[9] = true;
    f[10] = is_x86_feature_detected!("sse2");
    f[12] = true;
    f[13] = is_x86_feature_detected!("sse3");
    f[14] = is_x86_feature_detected!("cmpxchg16b");
    f[17] = is_x86_feature_detected!("xsave");
    f[20] = true;
    f[22] = true;
    f[23] = true;
    f[36] = is_x86_feature_detected!("ssse3");
    f[37] = is_x86_feature_detected!("sse4.1");
    f[38] = is_x86_feature_detected!("sse4.2");
    f[39] = is_x86_feature_detected!("avx");
    f[40] = is_x86_feature_detected!("avx2");
    f[41] = is_x86_feature_detected!("avx512f");
    f
}

// ---- calling into guest code ------------------------------------------------------------------
global_asm!(
    r#"
    .globl anyexe_call_guest
    .hidden anyexe_call_guest
    .type anyexe_call_guest,@function
anyexe_call_guest:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rdx], rsp
    sub rsp, 40
    mov rcx, rsi
    call rdi
    add rsp, 40
    .globl anyexe_guest_epilogue
    .hidden anyexe_guest_epilogue
anyexe_guest_epilogue:
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret
    .size anyexe_call_guest, .-anyexe_call_guest

    .globl anyexe_escape
    .hidden anyexe_escape
    .type anyexe_escape,@function
anyexe_escape:
    mov rsp, rdi
    mov eax, esi
    jmp anyexe_guest_epilogue
    .size anyexe_escape, .-anyexe_escape
"#
);

extern "C" {
    fn anyexe_call_guest(f: usize, arg: usize, slot: *mut usize) -> u32;
    fn anyexe_escape(saved_sp: usize, code: u32) -> !;
}

thread_local! {
    static ESCAPE_SP: Cell<usize> = const { Cell::new(0) };
}

/// Calls a Windows-ABI `fn(arg) -> u32` as the thread's outermost guest frame.
/// [`escape_to_top`] unwinds back to here, returning its exit code.
pub unsafe fn call_guest_top(f: usize, arg: usize) -> u32 {
    // The trampoline stores the stack pointer to restore into `slot`; the thread-local keeps
    // the slot's address so `escape_to_top` can find it from any depth.
    let mut slot = 0usize;
    let prev = ESCAPE_SP.with(|s| s.replace(&mut slot as *mut usize as usize));
    let code = anyexe_call_guest(f, arg, &mut slot);
    ESCAPE_SP.with(|s| s.set(prev));
    code
}

/// Abandons all frames above the thread's outermost guest call and makes it return `code`.
pub unsafe fn escape_to_top(code: u32) -> ! {
    let slot_addr = ESCAPE_SP.with(|s| s.get());
    if slot_addr == 0 {
        libc::_exit(code as i32 & 0xff);
    }
    let saved = *(slot_addr as *const usize);
    anyexe_escape(saved, code)
}
