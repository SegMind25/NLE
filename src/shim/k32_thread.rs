//! kernel32: threads, thread-local storage (TLS) and fiber-local storage (FLS).
//!
//! Every Windows thread is a pthread whose entry trampoline builds a TEB, points `gs` at it,
//! runs the module thread-attach notifications and then calls the guest start routine.
//! `ExitThread` unwinds back to that trampoline with [`teb::escape_to_top`].

use crate::loader;
use crate::runtime::handles::{self, Kind, ThreadObj};
use crate::runtime::teb;
use crate::win::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

const STILL_ACTIVE: DWORD = 259;
const TLS_OUT_OF_INDEXES: DWORD = 0xffff_ffff;
const TLS_EXPANSION_SLOTS: usize = 1024;
const TEB_TLS_EXPANSION: usize = 0x1780;
const FLS_SLOTS: usize = 128;
const DEFAULT_STACK: usize = 8 << 20;

type FlsCallback = unsafe extern "win64" fn(usize);

struct StartInfo {
    start: usize,
    param: usize,
    obj: Arc<Kind>,
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<Kind>>> = const { RefCell::new(None) };
}

static THREADS: Mutex<Option<HashMap<u32, Arc<Kind>>>> = Mutex::new(None);

fn new_thread_obj(suspended: bool) -> Arc<Kind> {
    Arc::new(Kind::Thread(ThreadObj {
        tid: AtomicU32::new(0),
        done: AtomicBool::new(false),
        exit_code: AtomicU32::new(STILL_ACTIVE),
        suspended: Mutex::new(suspended),
        resume: Condvar::new(),
        pthread: AtomicU64::new(0),
    }))
}

fn register(tid: u32, obj: &Arc<Kind>) {
    THREADS.lock().unwrap().get_or_insert_with(HashMap::new).insert(tid, obj.clone());
}

/// The kernel object for the calling thread (created on demand for threads we did not spawn).
pub fn current_thread_obj() -> Arc<Kind> {
    CURRENT.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(o) = &*c {
            return o.clone();
        }
        let o = new_thread_obj(false);
        let tid = handles::current_tid();
        if let Kind::Thread(t) = &*o {
            t.tid.store(tid, Ordering::SeqCst);
        }
        register(tid, &o);
        *c = Some(o.clone());
        o
    })
}

unsafe fn fls_data(create: bool) -> *mut usize {
    let slot = teb().add(TEB_FLS_DATA) as *mut usize;
    if *slot == 0 && create {
        *slot = libc::calloc(FLS_SLOTS, 8) as usize;
    }
    *slot as *mut usize
}

static FLS_CALLBACKS: Mutex<[usize; FLS_SLOTS]> = Mutex::new([0; FLS_SLOTS]);
static FLS_USED: Mutex<[bool; FLS_SLOTS]> = Mutex::new([false; FLS_SLOTS]);

unsafe fn fls_run_callbacks() {
    let data = fls_data(false);
    if data.is_null() {
        return;
    }
    let cbs = *FLS_CALLBACKS.lock().unwrap();
    for (i, cb) in cbs.iter().enumerate() {
        let v = *data.add(i);
        if v != 0 && *cb != 0 {
            *data.add(i) = 0;
            let f: FlsCallback = std::mem::transmute(*cb);
            f(v);
        }
    }
}

unsafe fn thread_finish(obj: &Arc<Kind>, code: u32) {
    loader::thread_detach();
    fls_run_callbacks();
    if let Kind::Thread(t) = &**obj {
        handles::locked_change(|| {
            t.exit_code.store(code, Ordering::SeqCst);
            t.done.store(true, Ordering::SeqCst);
        });
        THREADS.lock().unwrap().as_mut().map(|m| m.remove(&t.tid.load(Ordering::SeqCst)));
    }
}

extern "C" fn thread_entry(arg: *mut c_void) -> *mut c_void {
    unsafe {
        let si = Box::from_raw(arg as *mut StartInfo);
        let teb_ptr = teb::setup_thread();
        let tid = handles::current_tid();
        CURRENT.with(|c| *c.borrow_mut() = Some(si.obj.clone()));
        if let Kind::Thread(t) = &*si.obj {
            t.tid.store(tid, Ordering::SeqCst);
            register(tid, &si.obj);
            let mut s = t.suspended.lock().unwrap();
            while *s {
                s = t.resume.wait(s).unwrap();
            }
        }
        loader::thread_attach();
        let code = teb::call_guest_top(si.start, si.param);
        thread_finish(&si.obj, code);
        CURRENT.with(|c| *c.borrow_mut() = None);
        teb::teardown_thread(teb_ptr);
    }
    std::ptr::null_mut()
}

unsafe fn create_thread(stack: usize, start: usize, param: usize, flags: DWORD, tid_out: *mut DWORD) -> HANDLE {
    let obj = new_thread_obj(flags & CREATE_SUSPENDED != 0);
    let info = Box::into_raw(Box::new(StartInfo { start, param, obj: obj.clone() }));
    let mut attr: libc::pthread_attr_t = std::mem::zeroed();
    libc::pthread_attr_init(&mut attr);
    libc::pthread_attr_setdetachstate(&mut attr, libc::PTHREAD_CREATE_DETACHED);
    let sz = if stack == 0 { DEFAULT_STACK } else { stack.clamp(256 << 10, 1 << 30) };
    libc::pthread_attr_setstacksize(&mut attr, sz);
    let mut th: libc::pthread_t = 0;
    let r = libc::pthread_create(&mut th, &attr, thread_entry, info as *mut c_void);
    libc::pthread_attr_destroy(&mut attr);
    if r != 0 {
        drop(Box::from_raw(info));
        set_last_error(ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }
    if let Kind::Thread(t) = &*obj {
        t.pthread.store(th as u64, Ordering::SeqCst);
        while t.tid.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        if !tid_out.is_null() {
            *tid_out = t.tid.load(Ordering::SeqCst);
        }
    }
    handles::insert_arc(obj)
}

fn thread_of(h: HANDLE) -> Option<Arc<Kind>> {
    if h == CURRENT_THREAD {
        return Some(current_thread_obj());
    }
    handles::get(h).filter(|k| matches!(&**k, Kind::Thread(_)))
}

static TLS_USED: Mutex<[bool; TLS_MINIMUM_AVAILABLE + TLS_EXPANSION_SLOTS]> = Mutex::new([false; TLS_MINIMUM_AVAILABLE + TLS_EXPANSION_SLOTS]);

unsafe fn tls_slot_ptr(index: usize, create: bool) -> *mut usize {
    if index < TLS_MINIMUM_AVAILABLE {
        return teb().add(TEB_TLS_SLOTS + index * 8) as *mut usize;
    }
    let exp = teb().add(TEB_TLS_EXPANSION) as *mut usize;
    if *exp == 0 {
        if !create {
            return std::ptr::null_mut();
        }
        *exp = libc::calloc(TLS_EXPANSION_SLOTS, 8) as usize;
    }
    (*exp as *mut usize).add(index - TLS_MINIMUM_AVAILABLE)
}

crate::shims! { "kernel32";
    fn CreateThread(_attrs: usize, stack: usize, start: usize, param: usize, flags: DWORD, tid: *mut DWORD) -> HANDLE {
        create_thread(stack, start, param, flags, tid)
    }
    fn CreateRemoteThread(process: HANDLE, attrs: usize, stack: usize, start: usize, param: usize, flags: DWORD, tid: *mut DWORD) -> HANDLE {
        if process != CURRENT_PROCESS {
            set_last_error(ERROR_NOT_SUPPORTED);
            return 0;
        }
        CreateThread(attrs, stack, start, param, flags, tid)
    }
    fn ExitThread(code: DWORD) -> ! {
        if handles::current_tid() as i32 == libc::getpid() {
            loader::exit_process(code);
        }
        teb::escape_to_top(code)
    }
    fn GetCurrentThread() -> HANDLE {
        CURRENT_THREAD
    }
    fn GetCurrentThreadId() -> DWORD {
        handles::current_tid()
    }
    fn GetThreadId(h: HANDLE) -> DWORD {
        match thread_of(h).as_deref() {
            Some(Kind::Thread(t)) => t.tid.load(Ordering::SeqCst),
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                0
            }
        }
    }
    fn OpenThread(_access: DWORD, _inherit: BOOL, tid: DWORD) -> HANDLE {
        let found = THREADS.lock().unwrap().as_ref().and_then(|m| m.get(&tid).cloned());
        match found {
            Some(o) => handles::insert_arc(o),
            None => {
                set_last_error(ERROR_INVALID_PARAMETER);
                0
            }
        }
    }
    fn GetExitCodeThread(h: HANDLE, code: *mut DWORD) -> BOOL {
        match thread_of(h).as_deref() {
            Some(Kind::Thread(t)) => {
                if !code.is_null() {
                    *code = t.exit_code.load(Ordering::SeqCst);
                }
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }
    fn ResumeThread(h: HANDLE) -> DWORD {
        match thread_of(h).as_deref() {
            Some(Kind::Thread(t)) => {
                let mut s = t.suspended.lock().unwrap();
                let prev = *s as DWORD;
                *s = false;
                t.resume.notify_all();
                prev
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                u32::MAX
            }
        }
    }
    fn SuspendThread(_h: HANDLE) -> DWORD {
        set_last_error(ERROR_NOT_SUPPORTED);
        u32::MAX
    }
    fn TerminateThread(_h: HANDLE, _code: DWORD) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn SwitchToThread() -> BOOL {
        libc::sched_yield();
        TRUE
    }
    fn GetThreadPriority(_h: HANDLE) -> i32 {
        0
    }
    fn SetThreadPriority(_h: HANDLE, _p: i32) -> BOOL {
        TRUE
    }
    fn SetThreadPriorityBoost(_h: HANDLE, _disable: BOOL) -> BOOL {
        TRUE
    }
    fn GetThreadPriorityBoost(_h: HANDLE, out: *mut BOOL) -> BOOL {
        if !out.is_null() {
            *out = FALSE;
        }
        TRUE
    }
    fn SetThreadIdealProcessor(_h: HANDLE, _p: DWORD) -> DWORD {
        0
    }
    fn SetThreadAffinityMask(_h: HANDLE, _mask: usize) -> usize {
        usize::MAX
    }
    fn SetThreadDescription(_h: HANDLE, _desc: LPCWSTR) -> i32 {
        0
    }
    fn GetThreadDescription(_h: HANDLE, out: *mut *mut u16) -> i32 {
        if !out.is_null() {
            *out = libc::calloc(1, 2) as *mut u16;
        }
        0
    }
    fn SetThreadStackGuarantee(_size: *mut DWORD) -> BOOL {
        TRUE
    }
    fn GetThreadTimes(_h: HANDLE, creation: *mut FILETIME, exit: *mut FILETIME, kernel: *mut FILETIME, user: *mut FILETIME) -> BOOL {
        for p in [creation, exit, kernel, user] {
            if !p.is_null() {
                *p = FILETIME::default();
            }
        }
        TRUE
    }
    fn GetThreadContext(h: HANDLE, ctx: *mut CONTEXT) -> BOOL {
        if h == CURRENT_THREAD && !ctx.is_null() {
            crate::shim::ntdll::capture_context(ctx);
            return TRUE;
        }
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn SetThreadContext(_h: HANDLE, _ctx: *const CONTEXT) -> BOOL {
        set_last_error(ERROR_NOT_SUPPORTED);
        FALSE
    }
    fn QueueUserAPC(_f: usize, _h: HANDLE, _param: usize) -> DWORD {
        set_last_error(ERROR_NOT_SUPPORTED);
        0
    }
    fn GetCurrentProcessorNumber() -> DWORD {
        libc::sched_getcpu().max(0) as DWORD
    }
    fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize) {
        *low = (teb().add(TEB_STACK_LIMIT) as *const usize).read();
        *high = (teb().add(TEB_STACK_BASE) as *const usize).read();
    }
    fn SetThreadErrorMode(_new: DWORD, old: *mut DWORD) -> BOOL {
        if !old.is_null() {
            *old = 0;
        }
        TRUE
    }
    fn GetThreadErrorMode() -> DWORD {
        0
    }
    fn SetThreadExecutionState(_flags: DWORD) -> DWORD {
        0x8000_0000
    }

    fn TlsAlloc() -> DWORD {
        let mut used = TLS_USED.lock().unwrap();
        match used.iter().position(|u| !*u) {
            Some(i) => {
                used[i] = true;
                let p = tls_slot_ptr(i, true);
                if !p.is_null() {
                    *p = 0;
                }
                i as DWORD
            }
            None => {
                set_last_error(ERROR_NO_MORE_ITEMS);
                TLS_OUT_OF_INDEXES
            }
        }
    }
    fn TlsFree(index: DWORD) -> BOOL {
        let i = index as usize;
        let mut used = TLS_USED.lock().unwrap();
        if i >= used.len() || !used[i] {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        used[i] = false;
        let p = tls_slot_ptr(i, false);
        if !p.is_null() {
            *p = 0;
        }
        TRUE
    }
    fn TlsGetValue(index: DWORD) -> usize {
        set_last_error(ERROR_SUCCESS);
        let i = index as usize;
        if i >= TLS_MINIMUM_AVAILABLE + TLS_EXPANSION_SLOTS {
            set_last_error(ERROR_INVALID_PARAMETER);
            return 0;
        }
        let p = tls_slot_ptr(i, false);
        if p.is_null() {
            0
        } else {
            *p
        }
    }
    fn TlsSetValue(index: DWORD, value: usize) -> BOOL {
        let i = index as usize;
        if i >= TLS_MINIMUM_AVAILABLE + TLS_EXPANSION_SLOTS {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        let p = tls_slot_ptr(i, true);
        if p.is_null() {
            return FALSE;
        }
        *p = value;
        TRUE
    }

    fn FlsAlloc(callback: usize) -> DWORD {
        let mut used = FLS_USED.lock().unwrap();
        match used.iter().position(|u| !*u) {
            Some(i) => {
                used[i] = true;
                FLS_CALLBACKS.lock().unwrap()[i] = callback;
                let d = fls_data(true);
                *d.add(i) = 0;
                i as DWORD
            }
            None => {
                set_last_error(ERROR_NO_MORE_ITEMS);
                u32::MAX
            }
        }
    }
    fn FlsFree(index: DWORD) -> BOOL {
        let i = index as usize;
        let mut used = FLS_USED.lock().unwrap();
        if i >= FLS_SLOTS || !used[i] {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        used[i] = false;
        FLS_CALLBACKS.lock().unwrap()[i] = 0;
        let d = fls_data(false);
        if !d.is_null() {
            *d.add(i) = 0;
        }
        TRUE
    }
    fn FlsGetValue(index: DWORD) -> usize {
        set_last_error(ERROR_SUCCESS);
        let i = index as usize;
        if i >= FLS_SLOTS {
            set_last_error(ERROR_INVALID_PARAMETER);
            return 0;
        }
        let d = fls_data(false);
        if d.is_null() {
            0
        } else {
            *d.add(i)
        }
    }
    fn FlsSetValue(index: DWORD, value: usize) -> BOOL {
        let i = index as usize;
        if i >= FLS_SLOTS || !FLS_USED.lock().unwrap()[i] {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        let d = fls_data(true);
        *d.add(i) = value;
        TRUE
    }
}
