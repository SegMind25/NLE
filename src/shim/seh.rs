//! Structured exception handling for Windows x64.
//!
//! * `RtlLookupFunctionEntry` / `RtlVirtualUnwind` interpret `.pdata` and unwind info exactly as the
//!   Windows unwinder does, so MinGW C++ exceptions (`__gxx_personality_seh0` in libgcc) and MSVC
//!   `__try`/`__except` (`__C_specific_handler`) work unmodified.
//! * `RaiseException` dispatches in two phases: vectored handlers, then a walk of the frames asking each
//!   language handler to accept the exception; the handler then calls `RtlUnwindEx` to run the second phase.
//! * Hardware faults (SIGSEGV, SIGFPE, ...) become Windows exceptions through a signal handler.

use crate::loader;
use crate::runtime::strings;
use crate::win::*;
use std::arch::naked_asm;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

pub static UNHANDLED_FILTER: AtomicUsize = AtomicUsize::new(0);

type ExceptionRoutine = unsafe extern "win64" fn(*mut EXCEPTION_RECORD, u64, *mut CONTEXT, *mut DispatcherContext) -> i32;
type VectoredHandler = unsafe extern "win64" fn(*mut EXCEPTION_POINTERS) -> i32;
type TopFilter = unsafe extern "win64" fn(*mut EXCEPTION_POINTERS) -> i32;

const FPE_INTDIV: i32 = 1;
const FPE_INTOVF: i32 = 2;
const FPE_FLTDIV: i32 = 3;

const DISP_CONTINUE_EXECUTION: i32 = 0;
const DISP_CONTINUE_SEARCH: i32 = 1;
const DISP_NESTED: i32 = 2;
const DISP_COLLIDED: i32 = 3;

const UNW_FLAG_EHANDLER: u8 = 1;
const UNW_FLAG_UHANDLER: u8 = 2;
const UNW_FLAG_CHAININFO: u8 = 4;

#[repr(C)]
pub struct DispatcherContext {
    control_pc: u64,
    image_base: u64,
    function_entry: *const RUNTIME_FUNCTION,
    establisher_frame: u64,
    target_ip: u64,
    context_record: *mut CONTEXT,
    language_handler: usize,
    handler_data: *mut c_void,
    history_table: *mut c_void,
    scope_index: u32,
    fill0: u32,
}

struct DynTable {
    base: u64,
    table: usize,
    count: u32,
}

static VEH: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// What the runtime is in the middle of, innermost last. The Windows unwinder keeps these on the real
/// stack (dispatcher and handler frames); ours are Rust frames without unwind data, so a nested raise
/// from inside a handler needs this record to continue its frame walk where the handler was entered.
#[derive(Clone, Copy)]
enum Kind {
    /// An exception being dispatched; holds the context at the raise point.
    Dispatch(*mut CONTEXT),
    /// A language handler running for the frame with this (virtually unwound) context.
    Handler(*const CONTEXT),
}

struct Inflight {
    /// Address of a local in the Rust frame that pushed the entry. A restore to stack address R
    /// abandons every Rust frame below R, so entries with a smaller marker are dead.
    marker: usize,
    kind: Kind,
}

thread_local! {
    static INFLIGHT: std::cell::RefCell<Vec<Inflight>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn push_inflight(marker: *const u8, kind: Kind) -> usize {
    INFLIGHT.with(|v| {
        let mut v = v.borrow_mut();
        v.push(Inflight { marker: marker as usize, kind });
        v.len() - 1
    })
}

fn truncate_inflight(len: usize) {
    INFLIGHT.with(|v| v.borrow_mut().truncate(len));
}

fn inflight_len() -> usize {
    INFLIGHT.with(|v| v.borrow().len())
}

fn innermost_dispatch() -> Option<*mut CONTEXT> {
    INFLIGHT.with(|v| {
        v.borrow().iter().rev().find_map(|e| match e.kind {
            Kind::Dispatch(c) => Some(c),
            _ => None,
        })
    })
}

/// The nearest handler entry below `below`: where a frame walk continues after it runs off the
/// end of the PE frames into runtime code.
fn splice_target(below: usize) -> Option<(usize, CONTEXT)> {
    INFLIGHT.with(|v| {
        let v = v.borrow();
        (0..below.min(v.len())).rev().find_map(|i| match v[i].kind {
            Kind::Handler(c) => Some((i, unsafe { *c })),
            _ => None,
        })
    })
}

fn drop_dead_entries(rsp: u64) {
    INFLIGHT.with(|v| v.borrow_mut().retain(|e| e.marker as u64 >= rsp));
}

/// Resumes execution at `ctx`, abandoning the runtime frames below it.
unsafe fn restore(ctx: *const CONTEXT, rec: *const EXCEPTION_RECORD) -> ! {
    drop_dead_entries((*ctx).rsp);
    rtl_restore_context(ctx, rec);
    libc::_exit(3)
}

fn in_pe_module(pc: u64) -> bool {
    loader::module_from_addr(pc as usize).is_some()
}

static DYN_TABLES: Mutex<Vec<DynTable>> = Mutex::new(Vec::new());

fn ctx_regs(ctx: &mut CONTEXT) -> *mut u64 {
    &mut ctx.rax as *mut u64
}

unsafe fn reg(ctx: &mut CONTEXT, i: u8) -> &mut u64 {
    &mut *ctx_regs(ctx).add(i as usize)
}

// ---- function table lookup ------------------------------------------------------------------------------

unsafe fn lookup_table(base: u64, table: *const RUNTIME_FUNCTION, count: usize, pc: u64) -> *const RUNTIME_FUNCTION {
    let rva = (pc - base) as u32;
    let (mut lo, mut hi) = (0usize, count);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let f = &*table.add(mid);
        if rva < f.begin {
            hi = mid;
        } else if rva >= f.end {
            lo = mid + 1;
        } else {
            return table.add(mid);
        }
    }
    std::ptr::null()
}

pub unsafe fn lookup_function_entry(pc: u64) -> Option<(*const RUNTIME_FUNCTION, u64)> {
    if let Some(m) = loader::module_from_addr(pc as usize) {
        if m.pdata.0 != 0 {
            let n = m.pdata.1 / std::mem::size_of::<RUNTIME_FUNCTION>();
            let f = lookup_table(m.base as u64, m.pdata.0 as *const RUNTIME_FUNCTION, n, pc);
            if !f.is_null() {
                return Some((f, m.base as u64));
            }
        }
    }
    let tables = DYN_TABLES.lock().unwrap();
    for t in tables.iter() {
        if pc >= t.base {
            let f = lookup_table(t.base, t.table as *const RUNTIME_FUNCTION, t.count as usize, pc);
            if !f.is_null() {
                return Some((f, t.base));
            }
        }
    }
    None
}

// ---- unwinding ----------------------------------------------------------------------------------------------

/// Decides whether `pc` sits in a function epilogue: optional `add rsp, imm` / `lea rsp, [reg+disp]`,
/// then pops, then `ret` or a tail-call `jmp` that leaves the function `[begin, end)`.
unsafe fn is_epilogue(pc: *const u8, begin: u64, end: u64) -> bool {
    let b = |i: usize| *pc.add(i);
    let mut q = 0usize;
    match (b(0), b(1), b(2)) {
        (0x48, 0x83, 0xc4) => q += 4,
        (0x48, 0x81, 0xc4) => q += 7,
        (0x48, 0x8d, 0x65) => q += 4,
        (0x48, 0x8d, 0xa5) => q += 7,
        (0x48, 0x8d, 0x64) if b(3) == 0x24 => q += 5,
        (0x48, 0x8d, 0xa4) if b(3) == 0x24 => q += 8,
        _ => {}
    }
    loop {
        if b(q) & 0xf8 == 0x58 {
            q += 1;
        } else if b(q) == 0x41 && b(q + 1) & 0xf8 == 0x58 {
            q += 2;
        } else {
            break;
        }
    }
    let t = b(q);
    if t == 0xc3 || t == 0xc2 || (t == 0xf3 && b(q + 1) == 0xc3) {
        return true;
    }
    if q == 0 {
        return false;
    }
    let at = pc as u64 + q as u64;
    let outside = |target: u64| target < begin || target >= end;
    match t {
        0xeb => outside((at + 2).wrapping_add(b(q + 1) as i8 as i64 as u64)),
        0xe9 => outside((at + 5).wrapping_add((pc.add(q + 1) as *const i32).read_unaligned() as i64 as u64)),
        0xff => b(q + 1) == 0x25 || b(q + 1) & 0xf8 == 0xe0,
        0x48 => b(q + 1) == 0xff && (b(q + 2) == 0x25 || b(q + 2) & 0xf8 == 0xe0),
        _ => false,
    }
}

/// If `pc` is inside a function epilogue, applies the remaining epilogue instructions to `ctx`.
unsafe fn unwind_epilogue(pc: u64, ctx: &mut CONTEXT, begin: u64, end: u64) -> bool {
    let p = pc as *const u8;
    if !is_epilogue(p, begin, end) {
        return false;
    }
    let mut q = 0usize;
    let b = |i: usize| *p.add(i);
    match (b(0), b(1), b(2)) {
        (0x48, 0x83, 0xc4) => {
            ctx.rsp = ctx.rsp.wrapping_add(b(3) as i8 as i64 as u64);
            q += 4;
        }
        (0x48, 0x81, 0xc4) => {
            ctx.rsp = ctx.rsp.wrapping_add((p.add(3) as *const i32).read_unaligned() as i64 as u64);
            q += 7;
        }
        (0x48, 0x8d, 0x65) => {
            ctx.rsp = ctx.rbp.wrapping_add(b(3) as i8 as i64 as u64);
            q += 4;
        }
        (0x48, 0x8d, 0xa5) => {
            ctx.rsp = ctx.rbp.wrapping_add((p.add(3) as *const i32).read_unaligned() as i64 as u64);
            q += 7;
        }
        (0x48, 0x8d, 0x64) if b(3) == 0x24 => {
            ctx.rsp = ctx.rsp.wrapping_add(b(4) as i8 as i64 as u64);
            q += 5;
        }
        (0x48, 0x8d, 0xa4) if b(3) == 0x24 => {
            ctx.rsp = ctx.rsp.wrapping_add((p.add(4) as *const i32).read_unaligned() as i64 as u64);
            q += 8;
        }
        _ => {}
    }
    loop {
        let (r, len) = if b(q) & 0xf8 == 0x58 {
            (b(q) & 7, 1)
        } else if b(q) == 0x41 && b(q + 1) & 0xf8 == 0x58 {
            ((b(q + 1) & 7) + 8, 2)
        } else {
            break;
        };
        *reg(ctx, r) = *(ctx.rsp as *const u64);
        ctx.rsp += 8;
        q += len;
    }
    ctx.rip = *(ctx.rsp as *const u64);
    ctx.rsp += 8;
    true
}

/// Result of unwinding one frame.
pub struct Unwound {
    pub handler: usize,
    pub handler_data: *mut c_void,
    pub frame: u64,
}

pub unsafe fn virtual_unwind(handler_type: u8, base: u64, pc: u64, func: *const RUNTIME_FUNCTION, ctx: &mut CONTEXT) -> Unwound {
    let mut out = Unwound { handler: 0, handler_data: std::ptr::null_mut(), frame: ctx.rsp };
    let f = &*func;
    let mut offset = pc.wrapping_sub(base + f.begin as u64) as u32;
    if offset < (f.end - f.begin) && unwind_epilogue(pc, ctx, base + f.begin as u64, base + f.end as u64) {
        out.frame = ctx.rsp;
        return out;
    }
    let mut cur = func;
    let mut first = true;
    loop {
        let info = (base + (*cur).unwind_info as u64) as *const u8;
        let flags = *info >> 3;
        let prolog = *info.add(1) as u32;
        let count = *info.add(2) as usize;
        let frame_reg = *info.add(3) & 0xf;
        let frame_off = (*info.add(3) >> 4) as u64 * 16;
        let codes = info.add(4);
        if first {
            out.frame = if frame_reg != 0 && offset >= prolog { *reg(ctx, frame_reg) - frame_off } else { ctx.rsp };
        }
        let mut i = 0usize;
        while i < count {
            let code_off = *codes.add(i * 2) as u32;
            let op = *codes.add(i * 2 + 1) & 0xf;
            let op_info = *codes.add(i * 2 + 1) >> 4;
            let slot = |n: usize| (codes.add((i + n) * 2) as *const u16).read_unaligned() as u64;
            let slot32 = || (codes.add((i + 1) * 2) as *const u32).read_unaligned() as u64;
            let applies = offset >= code_off;
            let used = match op {
                0 => {
                    if applies {
                        *reg(ctx, op_info) = *(ctx.rsp as *const u64);
                        ctx.rsp += 8;
                    }
                    1
                }
                1 => {
                    if op_info == 0 {
                        if applies {
                            ctx.rsp += slot(1) * 8;
                        }
                        2
                    } else {
                        if applies {
                            ctx.rsp += slot32();
                        }
                        3
                    }
                }
                2 => {
                    if applies {
                        ctx.rsp += op_info as u64 * 8 + 8;
                    }
                    1
                }
                3 => {
                    if applies {
                        ctx.rsp = *reg(ctx, frame_reg) - frame_off;
                    }
                    1
                }
                4 => {
                    if applies {
                        *reg(ctx, op_info) = *((ctx.rsp + slot(1) * 8) as *const u64);
                    }
                    2
                }
                5 => {
                    if applies {
                        *reg(ctx, op_info) = *((ctx.rsp + slot32()) as *const u64);
                    }
                    3
                }
                6 => 2,
                7 => 3,
                8 => {
                    if applies {
                        let src = (ctx.rsp + slot(1) * 16) as *const u8;
                        std::ptr::copy_nonoverlapping(src, ctx.xmm_mut(op_info as usize).as_mut_ptr(), 16);
                    }
                    2
                }
                9 => {
                    if applies {
                        let src = (ctx.rsp + slot32()) as *const u8;
                        std::ptr::copy_nonoverlapping(src, ctx.xmm_mut(op_info as usize).as_mut_ptr(), 16);
                    }
                    3
                }
                10 => {
                    if applies {
                        let at = ctx.rsp + if op_info != 0 { 8 } else { 0 };
                        ctx.rip = *(at as *const u64);
                        ctx.rsp = *((at + 24) as *const u64);
                        return out;
                    }
                    1
                }
                _ => 1,
            };
            i += used;
        }
        let tail = (count + 1) & !1;
        let after = codes.add(tail * 2);
        if flags & UNW_FLAG_CHAININFO != 0 {
            cur = after as *const RUNTIME_FUNCTION;
            offset = u32::MAX;
            first = false;
            continue;
        }
        if flags & handler_type != 0 {
            out.handler = (base + (after as *const u32).read_unaligned() as u64) as usize;
            out.handler_data = after.add(4) as *mut c_void;
        }
        break;
    }
    ctx.rip = *(ctx.rsp as *const u64);
    ctx.rsp += 8;
    out
}

// ---- restoring a context -------------------------------------------------------------------------------------

#[unsafe(naked)]
pub unsafe extern "win64" fn rtl_restore_context(_ctx: *const CONTEXT, _rec: *const EXCEPTION_RECORD) {
    naked_asm!(
        "mov r11, rcx",
        "ldmxcsr [r11+0x34]",
        "movups xmm0, [r11+0x1a0]",
        "movups xmm1, [r11+0x1b0]",
        "movups xmm2, [r11+0x1c0]",
        "movups xmm3, [r11+0x1d0]",
        "movups xmm4, [r11+0x1e0]",
        "movups xmm5, [r11+0x1f0]",
        "movups xmm6, [r11+0x200]",
        "movups xmm7, [r11+0x210]",
        "movups xmm8, [r11+0x220]",
        "movups xmm9, [r11+0x230]",
        "movups xmm10, [r11+0x240]",
        "movups xmm11, [r11+0x250]",
        "movups xmm12, [r11+0x260]",
        "movups xmm13, [r11+0x270]",
        "movups xmm14, [r11+0x280]",
        "movups xmm15, [r11+0x290]",
        "mov rax, [r11+0x98]",
        "sub rax, 40",
        "mov rdx, [r11+0xf8]",
        "mov [rax], rdx",
        "mov qword ptr [rax+8], 0x33",
        "mov rdx, [r11+0x44]",
        "or rdx, 0x200",
        "mov [rax+16], rdx",
        "mov rdx, [r11+0x98]",
        "mov [rax+24], rdx",
        "mov qword ptr [rax+32], 0x2b",
        "mov rsp, rax",
        "mov rbx, [r11+0x90]",
        "mov rbp, [r11+0xa0]",
        "mov rsi, [r11+0xa8]",
        "mov rdi, [r11+0xb0]",
        "mov r8, [r11+0xb8]",
        "mov r9, [r11+0xc0]",
        "mov r10, [r11+0xc8]",
        "mov r12, [r11+0xd8]",
        "mov r13, [r11+0xe0]",
        "mov r14, [r11+0xe8]",
        "mov r15, [r11+0xf0]",
        "mov rcx, [r11+0x80]",
        "mov rdx, [r11+0x88]",
        "mov rax, [r11+0x78]",
        "mov r11, [r11+0xd0]",
        "iretq",
    );
}

// ---- exception dispatch ----------------------------------------------------------------------------------------

pub enum Dispatch {
    Continue,
    Unhandled,
}

unsafe fn run_vectored(rec: *mut EXCEPTION_RECORD, ctx: *mut CONTEXT) -> bool {
    let handlers: Vec<usize> = VEH.lock().unwrap().iter().map(|h| h.1).collect();
    let mut ptrs = EXCEPTION_POINTERS { record: rec, context: ctx };
    for h in handlers {
        let f: VectoredHandler = std::mem::transmute(h);
        if f(&mut ptrs) == EXCEPTION_CONTINUE_EXECUTION {
            return true;
        }
    }
    false
}

pub unsafe fn dispatch_exception(rec: *mut EXCEPTION_RECORD, ctx: *mut CONTEXT) -> Dispatch {
    let marker = 0u8;
    let depth = inflight_len();
    push_inflight(&marker, Kind::Dispatch(ctx));
    let r = dispatch_inner(rec, ctx);
    truncate_inflight(depth);
    r
}

unsafe fn dispatch_inner(rec: *mut EXCEPTION_RECORD, ctx: *mut CONTEXT) -> Dispatch {
    if run_vectored(rec, ctx) {
        return Dispatch::Continue;
    }
    if crate::runtime::cfg().trace.api {
        let r = &*rec;
        eprintln!("nle[seh]: raise code={:#010x} flags={:#x} at={} params={:x?}", r.code, r.flags, describe_address((*ctx).rip as usize), &r.information[..(r.number_parameters as usize).min(EXCEPTION_MAXIMUM_PARAMETERS)]);
    }
    let mut c: CONTEXT = *ctx;
    let mut base = inflight_len() - 1;
    for _ in 0..8192 {
        let pc = c.rip;
        let Some((func, image)) = lookup_function_entry(pc) else {
            if !in_pe_module(pc) {
                match splice_target(base) {
                    Some((idx, saved)) => {
                        c = saved;
                        base = idx;
                        continue;
                    }
                    None => break,
                }
            }
            if c.rsp == 0 || c.rsp % 8 != 0 {
                break;
            }
            let ret = *(c.rsp as *const u64);
            if ret == 0 || ret < 0x10000 {
                break;
            }
            c.rip = ret;
            c.rsp += 8;
            continue;
        };
        let mut next = c;
        let u = virtual_unwind(UNW_FLAG_EHANDLER, image, pc, func, &mut next);
        if crate::runtime::cfg().trace.api {
            eprintln!(
                "nle[seh]: search pc={} func=[{:#x},{:#x}) frame={:#x} handler={} -> ret={}",
                describe_address(pc as usize),
                (*func).begin,
                (*func).end,
                u.frame,
                if u.handler != 0 { describe_address(u.handler) } else { "-".into() },
                describe_address(next.rip as usize)
            );
        }
        if u.handler != 0 {
            let mut dc = DispatcherContext {
                control_pc: pc,
                image_base: image,
                function_entry: func,
                establisher_frame: u.frame,
                target_ip: 0,
                context_record: &mut c,
                language_handler: u.handler,
                handler_data: u.handler_data,
                history_table: std::ptr::null_mut(),
                scope_index: 0,
                fill0: 0,
            };
            let h: ExceptionRoutine = std::mem::transmute(u.handler);
            let marker = 0u8;
            let depth = inflight_len();
            push_inflight(&marker, Kind::Handler(&c));
            let disp = h(rec, u.frame, ctx, &mut dc);
            truncate_inflight(depth);
            if crate::runtime::cfg().trace.api {
                eprintln!("nle[seh]:   handler returned {disp} (code {:#x}, flags {:#x})", (*rec).code, (*rec).flags);
            }
            match disp {
                DISP_CONTINUE_EXECUTION => {
                    if (*rec).flags & EXCEPTION_NONCONTINUABLE != 0 {
                        raise_noncontinuable(rec);
                    }
                    return Dispatch::Continue;
                }
                DISP_CONTINUE_SEARCH | DISP_NESTED | DISP_COLLIDED => {}
                _ => raise_simple(STATUS_INVALID_DISPOSITION, EXCEPTION_NONCONTINUABLE),
            }
        }
        if next.rip == 0 || (next.rip == c.rip && next.rsp == c.rsp) || next.rsp < c.rsp {
            break;
        }
        c = next;
    }
    Dispatch::Unhandled
}

unsafe fn raise_simple(code: u32, flags: DWORD) -> ! {
    let mut rec: EXCEPTION_RECORD = std::mem::zeroed();
    rec.code = code;
    rec.flags = flags;
    let mut ctx = CONTEXT::zeroed();
    crate::shim::ntdll::capture_context(&mut ctx);
    finish_dispatch(&mut rec, &mut ctx)
}

unsafe fn raise_noncontinuable(rec: *mut EXCEPTION_RECORD) -> ! {
    let mut nr: EXCEPTION_RECORD = std::mem::zeroed();
    nr.code = STATUS_NONCONTINUABLE_EXCEPTION;
    nr.flags = EXCEPTION_NONCONTINUABLE;
    nr.record = rec;
    nr.address = (*rec).address;
    let mut ctx = CONTEXT::zeroed();
    crate::shim::ntdll::capture_context(&mut ctx);
    finish_dispatch(&mut nr, &mut ctx)
}

fn describe_address(addr: usize) -> String {
    match loader::module_from_addr(addr) {
        Some(m) => format!("{}+0x{:x}", m.name, addr - m.base),
        None => format!("0x{addr:x}"),
    }
}

/// Dispatches; on `Continue` resumes at `ctx`, otherwise runs the top-level filter and terminates.
pub unsafe fn finish_dispatch(rec: *mut EXCEPTION_RECORD, ctx: *mut CONTEXT) -> ! {
    match dispatch_exception(rec, ctx) {
        Dispatch::Continue => restore(ctx, rec),
        Dispatch::Unhandled => unhandled(rec, ctx),
    }
    libc::_exit(3)
}

unsafe fn unhandled(rec: *mut EXCEPTION_RECORD, ctx: *mut CONTEXT) {
    let f = UNHANDLED_FILTER.load(Ordering::SeqCst);
    if f != 0 {
        let mut ptrs = EXCEPTION_POINTERS { record: rec, context: ctx };
        let filter: TopFilter = std::mem::transmute(f);
        if filter(&mut ptrs) == EXCEPTION_CONTINUE_EXECUTION {
            restore(ctx, rec);
        }
    }
    libc::fflush(std::ptr::null_mut());
    let code = (*rec).code;
    eprintln!("nle: unhandled exception 0x{:08X} at {}", code, describe_address((*ctx).rip as usize));
    if (*ctx).rip < 0x10000 && (*ctx).rsp != 0 {
        let ret = *((*ctx).rsp as *const usize);
        eprintln!("        jumped to a null pointer; the caller is {}", describe_address(ret));
    }
    if code == STATUS_ACCESS_VIOLATION && (*rec).number_parameters >= 2 {
        let kind = match (*rec).information[0] {
            0 => "reading",
            1 => "writing",
            _ => "executing",
        };
        eprintln!("        access violation {kind} address 0x{:x}", (*rec).information[1]);
    }
    let sig = match code {
        STATUS_ACCESS_VIOLATION | STATUS_STACK_OVERFLOW => Some(libc::SIGSEGV),
        STATUS_ILLEGAL_INSTRUCTION => Some(libc::SIGILL),
        STATUS_INTEGER_DIVIDE_BY_ZERO | STATUS_FLOAT_DIVIDE_BY_ZERO | STATUS_INTEGER_OVERFLOW => Some(libc::SIGFPE),
        STATUS_BREAKPOINT => Some(libc::SIGTRAP),
        _ => None,
    };
    if let Some(s) = sig {
        libc::signal(s, libc::SIG_DFL);
        libc::raise(s);
    }
    libc::_exit(3)
}

unsafe fn unwind_ex(target_frame: u64, target_ip: u64, rec: *mut EXCEPTION_RECORD, ret: u64, orig: *mut CONTEXT) -> ! {
    let mut local_rec: EXCEPTION_RECORD = std::mem::zeroed();
    let rec = if rec.is_null() {
        local_rec.code = STATUS_UNWIND;
        local_rec.flags = if target_frame == 0 { EXCEPTION_EXIT_UNWIND } else { 0 };
        &mut local_rec as *mut EXCEPTION_RECORD
    } else {
        rec
    };
    (*rec).flags |= EXCEPTION_UNWINDING;
    if target_frame == 0 {
        (*rec).flags |= EXCEPTION_EXIT_UNWIND;
    }
    // Inside a dispatch the walk starts at its raise point: the frames of the dispatcher itself carry
    // no unwind data. Outside of one, the caller supplies an initialised context.
    let dispatch = innermost_dispatch();
    if crate::runtime::cfg().trace.api {
        eprintln!(
            "nle[seh]: RtlUnwindEx target_frame={target_frame:#x} target_ip={} dispatch={}",
            describe_address(target_ip as usize),
            dispatch.is_some()
        );
    }
    let mut ctx: CONTEXT = match dispatch {
        Some(p) => *p,
        None => *orig,
    };
    let own_marker = 0u8;
    let depth = inflight_len();
    let mut base = depth;
    for _ in 0..16384 {
        let pc = ctx.rip;
        let Some((func, image)) = lookup_function_entry(pc) else {
            if !in_pe_module(pc) {
                match splice_target(base) {
                    Some((idx, saved)) => {
                        ctx = saved;
                        base = idx;
                        continue;
                    }
                    None => break,
                }
            }
            if ctx.rsp == 0 {
                break;
            }
            ctx.rip = *(ctx.rsp as *const u64);
            ctx.rsp += 8;
            if ctx.rip == 0 {
                break;
            }
            continue;
        };
        let mut next = ctx;
        let u = virtual_unwind(UNW_FLAG_UHANDLER, image, pc, func, &mut next);
        if crate::runtime::cfg().trace.api {
            eprintln!(
                "nle[seh]:   unwind pc={} frame={:#x} handler={}",
                describe_address(pc as usize),
                u.frame,
                if u.handler != 0 { describe_address(u.handler) } else { "-".into() }
            );
        }
        if target_frame != 0 && u.frame == target_frame {
            (*rec).flags |= EXCEPTION_TARGET_UNWIND;
        }
        if u.handler != 0 {
            let mut dc = DispatcherContext {
                control_pc: pc,
                image_base: image,
                function_entry: func,
                establisher_frame: u.frame,
                target_ip,
                context_record: &mut ctx,
                language_handler: u.handler,
                handler_data: u.handler_data,
                history_table: std::ptr::null_mut(),
                scope_index: 0,
                fill0: 0,
            };
            let h: ExceptionRoutine = std::mem::transmute(u.handler);
            let hdepth = inflight_len();
            push_inflight(&own_marker, Kind::Handler(&ctx));
            let d = h(rec, u.frame, &mut ctx, &mut dc);
            truncate_inflight(hdepth);
            if d != DISP_CONTINUE_SEARCH && d != DISP_COLLIDED {
                raise_simple(STATUS_INVALID_DISPOSITION, EXCEPTION_NONCONTINUABLE);
            }
        }
        if target_frame != 0 && u.frame == target_frame {
            break;
        }
        ctx = next;
        if ctx.rip == 0 {
            break;
        }
    }
    ctx.rax = ret;
    ctx.context_flags = CONTEXT_FULL;
    if (*rec).code == STATUS_UNWIND_CONSOLIDATE && (*rec).number_parameters >= 1 && (*rec).information[0] != 0 {
        // The Visual C++ runtime runs the catch block from this callback, after the frames below are unwound,
        // and gets the address to continue at from it. A raise inside the block continues its frame walk
        // from the target frame, as if the callback had been called from there.
        let callback: unsafe extern "win64" fn(*mut EXCEPTION_RECORD) -> u64 = std::mem::transmute((*rec).information[0]);
        let hdepth = inflight_len();
        push_inflight(&own_marker, Kind::Handler(&ctx));
        let resume = callback(rec);
        truncate_inflight(hdepth);
        ctx.rip = resume;
    } else if target_ip != 0 {
        ctx.rip = target_ip;
    }
    restore(&ctx, rec)
}

// ---- entry points that need the caller's register state ---------------------------------------------------------

unsafe extern "win64" fn raise_exception_impl(code: u32, flags: u32, nargs: u32, args: *const usize, ctx: *mut CONTEXT) {
    let mut rec: EXCEPTION_RECORD = std::mem::zeroed();
    rec.code = code;
    rec.flags = flags & EXCEPTION_NONCONTINUABLE;
    rec.address = (*ctx).rip as *mut c_void;
    let n = (nargs as usize).min(EXCEPTION_MAXIMUM_PARAMETERS);
    rec.number_parameters = n as u32;
    if !args.is_null() {
        for i in 0..n {
            rec.information[i] = *args.add(i);
        }
    }
    (*ctx).context_flags = CONTEXT_FULL;
    finish_dispatch(&mut rec, ctx)
}

unsafe extern "win64" fn rtl_raise_exception_impl(rec: *mut EXCEPTION_RECORD, _a: usize, _b: usize, _c: usize, ctx: *mut CONTEXT) {
    if (*rec).address.is_null() {
        (*rec).address = (*ctx).rip as *mut c_void;
    }
    (*ctx).context_flags = CONTEXT_FULL;
    finish_dispatch(rec, ctx)
}

unsafe extern "win64" fn rtl_unwind_impl(target_frame: u64, target_ip: u64, rec: *mut EXCEPTION_RECORD, ret: u64, ctx: *mut CONTEXT) {
    unwind_ex(target_frame, target_ip, rec, ret, ctx)
}

macro_rules! capture_thunk {
    ($name:ident, $target:path) => {
        #[unsafe(naked)]
        pub unsafe extern "win64" fn $name() {
            naked_asm!(
                "sub rsp, 0x508",
                "mov [rsp+0x30+0x78], rax",
                "mov [rsp+0x30+0x80], rcx",
                "mov [rsp+0x30+0x88], rdx",
                "mov [rsp+0x30+0x90], rbx",
                "lea rax, [rsp+0x508+8]",
                "mov [rsp+0x30+0x98], rax",
                "mov [rsp+0x30+0xa0], rbp",
                "mov [rsp+0x30+0xa8], rsi",
                "mov [rsp+0x30+0xb0], rdi",
                "mov [rsp+0x30+0xb8], r8",
                "mov [rsp+0x30+0xc0], r9",
                "mov [rsp+0x30+0xc8], r10",
                "mov [rsp+0x30+0xd0], r11",
                "mov [rsp+0x30+0xd8], r12",
                "mov [rsp+0x30+0xe0], r13",
                "mov [rsp+0x30+0xe8], r14",
                "mov [rsp+0x30+0xf0], r15",
                "mov rax, [rsp+0x508]",
                "mov [rsp+0x30+0xf8], rax",
                "pushfq",
                "pop rax",
                "mov [rsp+0x30+0x44], eax",
                "stmxcsr [rsp+0x30+0x34]",
                "movups [rsp+0x30+0x1a0], xmm0",
                "movups [rsp+0x30+0x1b0], xmm1",
                "movups [rsp+0x30+0x1c0], xmm2",
                "movups [rsp+0x30+0x1d0], xmm3",
                "movups [rsp+0x30+0x1e0], xmm4",
                "movups [rsp+0x30+0x1f0], xmm5",
                "movups [rsp+0x30+0x200], xmm6",
                "movups [rsp+0x30+0x210], xmm7",
                "movups [rsp+0x30+0x220], xmm8",
                "movups [rsp+0x30+0x230], xmm9",
                "movups [rsp+0x30+0x240], xmm10",
                "movups [rsp+0x30+0x250], xmm11",
                "movups [rsp+0x30+0x260], xmm12",
                "movups [rsp+0x30+0x270], xmm13",
                "movups [rsp+0x30+0x280], xmm14",
                "movups [rsp+0x30+0x290], xmm15",
                "lea rax, [rsp+0x30]",
                "mov [rsp+0x20], rax",
                "call {t}",
                "ud2",
                t = sym $target,
            );
        }
    };
}

capture_thunk!(raise_exception, raise_exception_impl);
capture_thunk!(rtl_raise_exception, rtl_raise_exception_impl);
capture_thunk!(rtl_unwind, rtl_unwind_impl);

// ---- C scope tables (__try / __except / __finally) -------------------------------------------------------------

#[repr(C)]
struct ScopeRecord {
    begin: u32,
    end: u32,
    handler: u32,
    target: u32,
}

type FilterFn = unsafe extern "win64" fn(*mut EXCEPTION_POINTERS, u64) -> i32;
type TermFn = unsafe extern "win64" fn(i32, u64);

unsafe extern "win64" fn c_specific_handler(rec: *mut EXCEPTION_RECORD, frame: u64, ctx: *mut CONTEXT, dc: *mut DispatcherContext) -> i32 {
    let d = &*dc;
    let table = d.handler_data as *const u32;
    let count = *table as usize;
    let recs = table.add(1) as *const ScopeRecord;
    let pc_rva = (d.control_pc - d.image_base) as u32;
    let unwinding = (*rec).flags & (EXCEPTION_UNWINDING | EXCEPTION_EXIT_UNWIND) != 0;
    for i in 0..count {
        let s = &*recs.add(i);
        if pc_rva < s.begin || pc_rva >= s.end {
            continue;
        }
        if unwinding {
            if (*rec).flags & EXCEPTION_TARGET_UNWIND != 0 && d.target_ip == d.image_base + s.target as u64 && s.target != 0 {
                return DISP_CONTINUE_SEARCH;
            }
            if s.target == 0 && s.handler != 0 {
                let f: TermFn = std::mem::transmute(d.image_base as usize + s.handler as usize);
                f(1, frame);
            }
            continue;
        }
        if s.target == 0 {
            continue;
        }
        let verdict = if s.handler == 1 {
            EXCEPTION_EXECUTE_HANDLER
        } else {
            let mut ptrs = EXCEPTION_POINTERS { record: rec, context: ctx };
            let f: FilterFn = std::mem::transmute(d.image_base as usize + s.handler as usize);
            f(&mut ptrs, frame)
        };
        match verdict {
            EXCEPTION_CONTINUE_EXECUTION => return DISP_CONTINUE_EXECUTION,
            EXCEPTION_EXECUTE_HANDLER => {
                unwind_ex(frame, d.image_base + s.target as u64, rec, (*rec).code as u64, ctx);
            }
            _ => {}
        }
    }
    DISP_CONTINUE_SEARCH
}

// ---- hardware faults -> exceptions --------------------------------------------------------------------------------

unsafe fn context_from_ucontext(uc: *mut libc::ucontext_t, ctx: &mut CONTEXT) {
    let g = &(*uc).uc_mcontext.gregs;
    ctx.context_flags = CONTEXT_FULL;
    ctx.rax = g[libc::REG_RAX as usize] as u64;
    ctx.rcx = g[libc::REG_RCX as usize] as u64;
    ctx.rdx = g[libc::REG_RDX as usize] as u64;
    ctx.rbx = g[libc::REG_RBX as usize] as u64;
    ctx.rsp = g[libc::REG_RSP as usize] as u64;
    ctx.rbp = g[libc::REG_RBP as usize] as u64;
    ctx.rsi = g[libc::REG_RSI as usize] as u64;
    ctx.rdi = g[libc::REG_RDI as usize] as u64;
    ctx.r8 = g[libc::REG_R8 as usize] as u64;
    ctx.r9 = g[libc::REG_R9 as usize] as u64;
    ctx.r10 = g[libc::REG_R10 as usize] as u64;
    ctx.r11 = g[libc::REG_R11 as usize] as u64;
    ctx.r12 = g[libc::REG_R12 as usize] as u64;
    ctx.r13 = g[libc::REG_R13 as usize] as u64;
    ctx.r14 = g[libc::REG_R14 as usize] as u64;
    ctx.r15 = g[libc::REG_R15 as usize] as u64;
    ctx.rip = g[libc::REG_RIP as usize] as u64;
    ctx.eflags = g[libc::REG_EFL as usize] as u32;
    ctx.seg_cs = 0x33;
    ctx.seg_ss = 0x2b;
    let fp = (*uc).uc_mcontext.fpregs;
    if !fp.is_null() {
        ctx.mxcsr = (*fp).mxcsr;
        for i in 0..16 {
            let src = &(*fp)._xmm[i] as *const _ as *const u8;
            std::ptr::copy_nonoverlapping(src, ctx.xmm_mut(i).as_mut_ptr(), 16);
        }
    }
}

unsafe fn ucontext_from_context(uc: *mut libc::ucontext_t, ctx: &CONTEXT) {
    let g = &mut (*uc).uc_mcontext.gregs;
    g[libc::REG_RAX as usize] = ctx.rax as i64;
    g[libc::REG_RCX as usize] = ctx.rcx as i64;
    g[libc::REG_RDX as usize] = ctx.rdx as i64;
    g[libc::REG_RBX as usize] = ctx.rbx as i64;
    g[libc::REG_RSP as usize] = ctx.rsp as i64;
    g[libc::REG_RBP as usize] = ctx.rbp as i64;
    g[libc::REG_RSI as usize] = ctx.rsi as i64;
    g[libc::REG_RDI as usize] = ctx.rdi as i64;
    g[libc::REG_R8 as usize] = ctx.r8 as i64;
    g[libc::REG_R9 as usize] = ctx.r9 as i64;
    g[libc::REG_R10 as usize] = ctx.r10 as i64;
    g[libc::REG_R11 as usize] = ctx.r11 as i64;
    g[libc::REG_R12 as usize] = ctx.r12 as i64;
    g[libc::REG_R13 as usize] = ctx.r13 as i64;
    g[libc::REG_R14 as usize] = ctx.r14 as i64;
    g[libc::REG_R15 as usize] = ctx.r15 as i64;
    g[libc::REG_RIP as usize] = ctx.rip as i64;
    let fp = (*uc).uc_mcontext.fpregs;
    if !fp.is_null() {
        for i in 0..16 {
            let dst = &mut (*fp)._xmm[i] as *mut _ as *mut u8;
            std::ptr::copy_nonoverlapping(ctx.xmm(i).as_ptr(), dst, 16);
        }
    }
}

fn has_teb() -> bool {
    let mut base: u64 = 0;
    unsafe { libc::syscall(libc::SYS_arch_prctl, 0x1004 as libc::c_long, &mut base as *mut u64) };
    base != 0
}

extern "C" fn fault_handler(sig: libc::c_int, info: *mut libc::siginfo_t, uctx: *mut c_void) {
    unsafe {
        if !has_teb() {
            libc::signal(sig, libc::SIG_DFL);
            return;
        }
        let uc = uctx as *mut libc::ucontext_t;
        let mut ctx = CONTEXT::zeroed();
        context_from_ucontext(uc, &mut ctx);
        let mut rec: EXCEPTION_RECORD = std::mem::zeroed();
        rec.address = ctx.rip as *mut c_void;
        let code = (*info).si_code;
        match sig {
            libc::SIGSEGV | libc::SIGBUS => {
                let addr = (*info).si_addr() as usize;
                let err = (*uc).uc_mcontext.gregs[libc::REG_ERR as usize] as u64;
                rec.code = if is_stack_overflow(addr, ctx.rsp as usize) { STATUS_STACK_OVERFLOW } else { STATUS_ACCESS_VIOLATION };
                rec.number_parameters = 2;
                rec.information[0] = if err & 0x10 != 0 { 8 } else if err & 2 != 0 { 1 } else { 0 };
                rec.information[1] = addr;
            }
            libc::SIGFPE => {
                rec.code = match code {
                    FPE_INTDIV => STATUS_INTEGER_DIVIDE_BY_ZERO,
                    FPE_INTOVF => STATUS_INTEGER_OVERFLOW,
                    FPE_FLTDIV => STATUS_FLOAT_DIVIDE_BY_ZERO,
                    _ => STATUS_FLOAT_INVALID_OPERATION,
                };
            }
            libc::SIGILL => rec.code = STATUS_ILLEGAL_INSTRUCTION,
            libc::SIGTRAP => {
                rec.code = STATUS_BREAKPOINT;
                rec.address = (ctx.rip - 1) as *mut c_void;
            }
            _ => rec.code = STATUS_ACCESS_VIOLATION,
        }
        let rec_ptr: *mut EXCEPTION_RECORD = &mut rec;
        let ctx_ptr: *mut CONTEXT = &mut ctx;
        match dispatch_exception(rec_ptr, ctx_ptr) {
            Dispatch::Continue => {
                ucontext_from_context(uc, &ctx);
            }
            Dispatch::Unhandled => unhandled(rec_ptr, ctx_ptr),
        }
    }
}

fn is_stack_overflow(addr: usize, rsp: usize) -> bool {
    unsafe {
        let low = (crate::win::teb().add(TEB_STACK_LIMIT) as *const usize).read();
        addr < low.saturating_add(0x10000) && addr + 0x100000 >= low && rsp.abs_diff(addr) < 0x20000
    }
}

pub fn install_fault_handlers() {
    unsafe {
        for sig in [libc::SIGSEGV, libc::SIGBUS, libc::SIGFPE, libc::SIGILL, libc::SIGTRAP] {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = fault_handler as *const () as usize;
            sa.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER | libc::SA_ONSTACK;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

/// Gives the calling thread an alternate signal stack so stack overflows can be reported.
pub unsafe fn install_alt_stack() {
    const SIZE: usize = 128 * 1024;
    let p = libc::mmap(std::ptr::null_mut(), SIZE, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
    if p == libc::MAP_FAILED {
        return;
    }
    let ss = libc::stack_t { ss_sp: p, ss_flags: 0, ss_size: SIZE };
    libc::sigaltstack(&ss, std::ptr::null_mut());
}

// ---- exported functions --------------------------------------------------------------------------------------------

crate::shims! { "kernel32";
    fn SetUnhandledExceptionFilter(f: usize) -> usize {
        UNHANDLED_FILTER.swap(f, Ordering::SeqCst)
    }
    fn UnhandledExceptionFilter(info: *mut EXCEPTION_POINTERS) -> i32 {
        let f = UNHANDLED_FILTER.load(Ordering::SeqCst);
        if f != 0 {
            let filter: TopFilter = std::mem::transmute(f);
            return filter(info);
        }
        EXCEPTION_EXECUTE_HANDLER
    }
    fn AddVectoredExceptionHandler(first: ULONG, handler: usize) -> usize {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        let id = NEXT.fetch_add(1, Ordering::SeqCst) * 16;
        let mut v = VEH.lock().unwrap();
        if first != 0 {
            v.insert(0, (id, handler));
        } else {
            v.push((id, handler));
        }
        id
    }
    fn RemoveVectoredExceptionHandler(id: usize) -> ULONG {
        let mut v = VEH.lock().unwrap();
        let n = v.len();
        v.retain(|h| h.0 != id);
        (v.len() != n) as ULONG
    }
    fn AddVectoredContinueHandler(_first: ULONG, _handler: usize) -> usize {
        1
    }
    fn RemoveVectoredContinueHandler(_id: usize) -> ULONG {
        1
    }
}

crate::shims! { "ntdll" => NT_SHIMS;
    fn RtlLookupFunctionEntry(pc: u64, image_base: *mut u64, _history: *mut c_void) -> *const RUNTIME_FUNCTION {
        match lookup_function_entry(pc) {
            Some((f, base)) => {
                if !image_base.is_null() {
                    *image_base = base;
                }
                f
            }
            None => {
                if !image_base.is_null() {
                    *image_base = 0;
                }
                std::ptr::null()
            }
        }
    }
    fn RtlVirtualUnwind(handler_type: DWORD, base: u64, pc: u64, func: *const RUNTIME_FUNCTION, ctx: *mut CONTEXT, handler_data: *mut *mut c_void, frame: *mut u64, _ptrs: *mut c_void) -> usize {
        let u = virtual_unwind(handler_type as u8, base, pc, func, &mut *ctx);
        if !handler_data.is_null() {
            *handler_data = u.handler_data;
        }
        if !frame.is_null() {
            *frame = u.frame;
        }
        u.handler
    }
    fn RtlUnwindEx(target_frame: u64, target_ip: u64, rec: *mut EXCEPTION_RECORD, ret: u64, ctx: *mut CONTEXT, _history: *mut c_void) {
        unwind_ex(target_frame, target_ip, rec, ret, ctx)
    }
    fn RtlRestoreContext(ctx: *const CONTEXT, rec: *const EXCEPTION_RECORD) {
        restore(ctx, rec)
    }
    fn RtlPcToFileHeader(pc: usize, base: *mut usize) -> usize {
        match loader::module_from_addr(pc) {
            Some(m) => {
                *base = m.base;
                m.base
            }
            None => {
                *base = 0;
                0
            }
        }
    }
    fn RtlAddFunctionTable(table: usize, count: DWORD, base: u64) -> u8 {
        DYN_TABLES.lock().unwrap().push(DynTable { base, table, count });
        1
    }
    fn RtlDeleteFunctionTable(table: usize) -> u8 {
        DYN_TABLES.lock().unwrap().retain(|t| t.table != table);
        1
    }
    fn RtlInstallFunctionTableCallback(_id: u64, _base: u64, _len: DWORD, _cb: usize, _ctx: usize, _dll: LPCWSTR) -> u8 {
        0
    }
    fn RaiseFailFastException(rec: *mut EXCEPTION_RECORD, _ctx: *mut CONTEXT, _flags: DWORD) {
        libc::fflush(std::ptr::null_mut());
        let code = if rec.is_null() { 0xc000_0409 } else { (*rec).code };
        eprintln!("nle: fast fail 0x{code:08X}");
        libc::_exit(3)
    }
    fn RtlRaiseStatus(status: u32) -> ! {
        raise_simple(status, 0)
    }
}

crate::shim_extra!("ntdll" => NT_EXTRA;
    "RaiseException" => raise_exception,
    "RtlRaiseException" => rtl_raise_exception,
    "RtlUnwind" => rtl_unwind,
);

crate::shim_extra!("kernel32" => K32_EXTRA;
    "RaiseException" => raise_exception,
);

crate::shim_extra!("crt" => CRT_EXTRA;
    "__C_specific_handler" => c_specific_handler,
);

crate::shim_extra!("ntdll" => NT_EXTRA2;
    "__C_specific_handler" => c_specific_handler,
);

#[allow(dead_code)]
fn _keep(_: &str) {
    let _ = strings::to_wide("");
}
