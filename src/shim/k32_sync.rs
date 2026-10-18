//! kernel32: synchronisation. Critical sections, SRW locks, condition variables, one-time init,
//! futex-style address waits, kernel objects (events, mutexes, semaphores, timers) and handles.
//!
//! The user-space primitives are futex based and valid from an all-zero state, as on Windows:
//! a zero-filled `CRITICAL_SECTION` or `SRWLOCK` works without an init call.

use crate::runtime::handles::{self, EventObj, Kind, MutexObj, ProcObj, SemObj, TimerObj};
use crate::runtime::{strings, teb};
use crate::win::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ---- futex ---------------------------------------------------------------------------------------
pub unsafe fn futex_wait(addr: *const u32, expected: u32, timeout_ms: Option<u32>) -> bool {
    let ts;
    let tp = match timeout_ms {
        Some(ms) => {
            ts = libc::timespec { tv_sec: (ms / 1000) as libc::time_t, tv_nsec: ((ms % 1000) as i64) * 1_000_000 };
            &ts as *const libc::timespec
        }
        None => std::ptr::null(),
    };
    let r = libc::syscall(libc::SYS_futex, addr, libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG, expected, tp, std::ptr::null::<u32>(), 0);
    !(r == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ETIMEDOUT))
}

pub unsafe fn futex_wake(addr: *const u32, n: i32) {
    libc::syscall(libc::SYS_futex, addr, libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG, n, std::ptr::null::<u32>(), std::ptr::null::<u32>(), 0);
}

fn atomic_u32<'a>(p: *mut u32) -> &'a AtomicU32 {
    unsafe { &*(p as *const AtomicU32) }
}

// ---- critical sections -----------------------------------------------------------------------------
unsafe fn cs_word(cs: *mut CRITICAL_SECTION) -> &'static AtomicU32 {
    atomic_u32(&mut (*cs).lock_count as *mut i32 as *mut u32)
}

unsafe fn cs_lock_raw(cs: *mut CRITICAL_SECTION) {
    let w = cs_word(cs);
    if w.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
        return;
    }
    for _ in 0..100 {
        std::hint::spin_loop();
        if w.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            return;
        }
    }
    while w.swap(2, Ordering::Acquire) != 0 {
        futex_wait(w.as_ptr(), 2, None);
    }
}

unsafe fn cs_unlock_raw(cs: *mut CRITICAL_SECTION) {
    let w = cs_word(cs);
    if w.swap(0, Ordering::Release) == 2 {
        futex_wake(w.as_ptr(), 1);
    }
}

pub unsafe fn cs_enter(cs: *mut CRITICAL_SECTION) {
    let tid = handles::current_tid() as usize;
    if std::ptr::read_volatile(&(*cs).owning_thread) == tid {
        (*cs).recursion_count += 1;
        return;
    }
    cs_lock_raw(cs);
    std::ptr::write_volatile(&mut (*cs).owning_thread, tid);
    (*cs).recursion_count = 1;
}

pub unsafe fn cs_leave(cs: *mut CRITICAL_SECTION) {
    let tid = handles::current_tid() as usize;
    if std::ptr::read_volatile(&(*cs).owning_thread) != tid {
        return;
    }
    (*cs).recursion_count -= 1;
    if (*cs).recursion_count == 0 {
        std::ptr::write_volatile(&mut (*cs).owning_thread, 0);
        cs_unlock_raw(cs);
    }
}

unsafe fn cs_init(cs: *mut CRITICAL_SECTION) {
    (*cs).debug_info = std::ptr::null_mut();
    (*cs).lock_count = 0;
    (*cs).recursion_count = 0;
    (*cs).owning_thread = 0;
    (*cs).lock_semaphore = 0;
    (*cs).spin_count = 0;
}

// ---- SRW locks ---------------------------------------------------------------------------------------
const SRW_EXCL: u32 = 1 << 31;
const SRW_WAITERS: u32 = 1 << 30;
const SRW_READERS: u32 = SRW_WAITERS - 1;

unsafe fn srw_excl(p: *mut u32) {
    let a = atomic_u32(p);
    loop {
        let cur = a.load(Ordering::Relaxed);
        if cur & (SRW_EXCL | SRW_READERS) == 0 {
            if a.compare_exchange_weak(cur, cur | SRW_EXCL, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                return;
            }
        } else if cur & SRW_WAITERS != 0 || a.compare_exchange(cur, cur | SRW_WAITERS, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            futex_wait(p, cur | SRW_WAITERS, None);
        }
    }
}

unsafe fn srw_try_excl(p: *mut u32) -> bool {
    let a = atomic_u32(p);
    let cur = a.load(Ordering::Relaxed);
    cur & (SRW_EXCL | SRW_READERS) == 0 && a.compare_exchange(cur, cur | SRW_EXCL, Ordering::Acquire, Ordering::Relaxed).is_ok()
}

unsafe fn srw_unlock_excl(p: *mut u32) {
    let a = atomic_u32(p);
    let prev = a.fetch_and(!(SRW_EXCL | SRW_WAITERS), Ordering::Release);
    if prev & SRW_WAITERS != 0 {
        futex_wake(p, i32::MAX);
    }
}

unsafe fn srw_shared(p: *mut u32) {
    let a = atomic_u32(p);
    loop {
        let cur = a.load(Ordering::Relaxed);
        if cur & SRW_EXCL == 0 {
            if a.compare_exchange_weak(cur, cur + 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                return;
            }
        } else if cur & SRW_WAITERS != 0 || a.compare_exchange(cur, cur | SRW_WAITERS, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            futex_wait(p, cur | SRW_WAITERS, None);
        }
    }
}

unsafe fn srw_try_shared(p: *mut u32) -> bool {
    let a = atomic_u32(p);
    let cur = a.load(Ordering::Relaxed);
    cur & SRW_EXCL == 0 && a.compare_exchange(cur, cur + 1, Ordering::Acquire, Ordering::Relaxed).is_ok()
}

unsafe fn srw_unlock_shared(p: *mut u32) {
    let a = atomic_u32(p);
    let prev = a.fetch_sub(1, Ordering::Release);
    if prev & SRW_READERS == 1 && prev & SRW_WAITERS != 0 {
        a.fetch_and(!SRW_WAITERS, Ordering::Relaxed);
        futex_wake(p, i32::MAX);
    }
}

// ---- condition variables -------------------------------------------------------------------------------
unsafe fn cv_wait(cv: *mut u32, release: impl FnOnce(), reacquire: impl FnOnce(), ms: DWORD) -> BOOL {
    let a = atomic_u32(cv);
    let seq = a.load(Ordering::SeqCst);
    release();
    let timeout = if ms == INFINITE { None } else { Some(ms) };
    let woke = futex_wait(cv, seq, timeout);
    reacquire();
    if woke {
        TRUE
    } else {
        set_last_error(ERROR_TIMEOUT);
        FALSE
    }
}

unsafe fn cv_wake(cv: *mut u32, n: i32) {
    atomic_u32(cv).fetch_add(1, Ordering::SeqCst);
    futex_wake(cv, n);
}

// ---- one-time init ------------------------------------------------------------------------------------
type InitOnceFn = unsafe extern "win64" fn(*mut u64, usize, *mut usize) -> BOOL;

// ---- named objects ---------------------------------------------------------------------------------------
static NAMED: Mutex<Option<HashMap<String, Arc<Kind>>>> = Mutex::new(None);

fn norm_name(n: &str) -> String {
    n.strip_prefix("Local\\").or_else(|| n.strip_prefix("Global\\")).unwrap_or(n).to_string()
}

pub fn named_get(name: &str) -> Option<Arc<Kind>> {
    NAMED.lock().unwrap().as_ref().and_then(|m| m.get(&norm_name(name)).cloned())
}

pub fn named_put(name: &str, obj: Arc<Kind>) {
    NAMED.lock().unwrap().get_or_insert_with(HashMap::new).insert(norm_name(name), obj);
}

unsafe fn create_named(name: Option<String>, same_kind: fn(&Kind) -> bool, make: impl FnOnce() -> Kind) -> HANDLE {
    if let Some(n) = &name {
        let mut g = NAMED.lock().unwrap();
        let map = g.get_or_insert_with(HashMap::new);
        if let Some(existing) = map.get(&norm_name(n)) {
            if same_kind(existing) {
                let h = handles::insert_arc(existing.clone());
                set_last_error(ERROR_ALREADY_EXISTS);
                return h;
            }
            set_last_error(ERROR_INVALID_HANDLE);
            return 0;
        }
        let obj = Arc::new(make());
        map.insert(norm_name(n), obj.clone());
        drop(g);
        set_last_error(ERROR_SUCCESS);
        return handles::insert_arc(obj);
    }
    set_last_error(ERROR_SUCCESS);
    handles::insert(make())
}

unsafe fn open_named(name: &str, same_kind: fn(&Kind) -> bool) -> HANDLE {
    match named_get(name) {
        Some(o) if same_kind(&o) => handles::insert_arc(o),
        _ => {
            set_last_error(ERROR_FILE_NOT_FOUND);
            0
        }
    }
}

fn is_event(k: &Kind) -> bool {
    matches!(k, Kind::Event(_))
}
fn is_mutex(k: &Kind) -> bool {
    matches!(k, Kind::Mutex(_))
}
fn is_sem(k: &Kind) -> bool {
    matches!(k, Kind::Semaphore(_))
}
fn is_timer(k: &Kind) -> bool {
    matches!(k, Kind::Timer(_))
}

unsafe fn opt_wname(p: LPCWSTR) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(strings::w_ptr_to_string(p))
    }
}

unsafe fn opt_aname(p: LPCSTR) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(strings::a_to_string(p))
    }
}

fn make_event(manual: bool, initial: bool) -> Kind {
    Kind::Event(EventObj { manual, signaled: AtomicBool::new(initial) })
}

fn make_mutex(owned: bool) -> Kind {
    let tid = handles::current_tid();
    Kind::Mutex(MutexObj { owner: AtomicU32::new(if owned { tid } else { 0 }), count: AtomicU32::new(owned as u32) })
}

fn make_sem(initial: i32, max: i32) -> Kind {
    Kind::Semaphore(SemObj { count: AtomicI32::new(initial), max })
}

fn make_timer(manual: bool) -> Kind {
    Kind::Timer(TimerObj { manual, signaled: AtomicBool::new(false), due_ns: AtomicU64::new(0), period_ms: AtomicU32::new(0) })
}

// ---- waits ------------------------------------------------------------------------------------------------
fn wait_single(h: HANDLE, ms: DWORD) -> DWORD {
    if h == CURRENT_THREAD || h == CURRENT_PROCESS {
        if ms != INFINITE {
            std::thread::sleep(std::time::Duration::from_millis(ms as u64));
            return WAIT_TIMEOUT;
        }
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    match handles::get(h) {
        Some(o) => handles::wait_objects(&[o], false, ms),
        None => {
            set_last_error(ERROR_INVALID_HANDLE);
            WAIT_FAILED
        }
    }
}

unsafe fn wait_multi(n: DWORD, hs: *const HANDLE, all: BOOL, ms: DWORD) -> DWORD {
    if n == 0 || n > 64 || hs.is_null() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return WAIT_FAILED;
    }
    let mut objs = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        match handles::get(*hs.add(i)) {
            Some(o) => objs.push(o),
            None => {
                set_last_error(ERROR_INVALID_HANDLE);
                return WAIT_FAILED;
            }
        }
    }
    handles::wait_objects(&objs, all != 0, ms)
}

fn timer_arm(t: &TimerObj, due: i64, period_ms: i32) {
    handles::locked_change(|| {
        t.signaled.store(false, Ordering::SeqCst);
        let now = handles::now_ns();
        let rel_ns: u64 = if due < 0 {
            (-(due as i128) * 100) as u64
        } else {
            let ft_now = teb::filetime_now() as i64;
            ((due - ft_now).max(0) as u64) * 100
        };
        t.period_ms.store(period_ms.max(0) as u32, Ordering::SeqCst);
        t.due_ns.store((now + rel_ns).max(1), Ordering::SeqCst);
    });
}

// ---- singly linked lists ------------------------------------------------------------------------------------
static SLIST_LOCK: Mutex<()> = Mutex::new(());

#[repr(C)]
pub struct SListHeader {
    head: usize,
    depth: u64,
}

crate::shims! { "kernel32";
    fn InitializeCriticalSection(cs: *mut CRITICAL_SECTION) {
        cs_init(cs);
    }
    fn InitializeCriticalSectionAndSpinCount(cs: *mut CRITICAL_SECTION, _spin: DWORD) -> BOOL {
        cs_init(cs);
        TRUE
    }
    fn InitializeCriticalSectionEx(cs: *mut CRITICAL_SECTION, _spin: DWORD, _flags: DWORD) -> BOOL {
        cs_init(cs);
        TRUE
    }
    fn EnterCriticalSection(cs: *mut CRITICAL_SECTION) {
        cs_enter(cs);
    }
    fn LeaveCriticalSection(cs: *mut CRITICAL_SECTION) {
        cs_leave(cs);
    }
    fn TryEnterCriticalSection(cs: *mut CRITICAL_SECTION) -> BOOL {
        let tid = handles::current_tid() as usize;
        if std::ptr::read_volatile(&(*cs).owning_thread) == tid {
            (*cs).recursion_count += 1;
            return TRUE;
        }
        if cs_word(cs).compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            std::ptr::write_volatile(&mut (*cs).owning_thread, tid);
            (*cs).recursion_count = 1;
            TRUE
        } else {
            FALSE
        }
    }
    fn DeleteCriticalSection(_cs: *mut CRITICAL_SECTION) {}
    fn SetCriticalSectionSpinCount(_cs: *mut CRITICAL_SECTION, _spin: DWORD) -> DWORD {
        0
    }

    fn InitializeSRWLock(p: *mut u64) {
        *p = 0;
    }
    fn AcquireSRWLockExclusive(p: *mut u32) {
        srw_excl(p);
    }
    fn ReleaseSRWLockExclusive(p: *mut u32) {
        srw_unlock_excl(p);
    }
    fn AcquireSRWLockShared(p: *mut u32) {
        srw_shared(p);
    }
    fn ReleaseSRWLockShared(p: *mut u32) {
        srw_unlock_shared(p);
    }
    fn TryAcquireSRWLockExclusive(p: *mut u32) -> u8 {
        srw_try_excl(p) as u8
    }
    fn TryAcquireSRWLockShared(p: *mut u32) -> u8 {
        srw_try_shared(p) as u8
    }

    fn InitializeConditionVariable(p: *mut u64) {
        *p = 0;
    }
    fn WakeConditionVariable(p: *mut u32) {
        cv_wake(p, 1);
    }
    fn WakeAllConditionVariable(p: *mut u32) {
        cv_wake(p, i32::MAX);
    }
    fn SleepConditionVariableCS(cv: *mut u32, cs: *mut CRITICAL_SECTION, ms: DWORD) -> BOOL {
        let rec = (*cs).recursion_count;
        cv_wait(
            cv,
            || {
                (*cs).recursion_count = 1;
                cs_leave(cs);
            },
            || {
                cs_enter(cs);
                (*cs).recursion_count = rec;
            },
            ms,
        )
    }
    fn SleepConditionVariableSRW(cv: *mut u32, srw: *mut u32, ms: DWORD, flags: DWORD) -> BOOL {
        let shared = flags & 1 != 0;
        cv_wait(
            cv,
            || if shared { srw_unlock_shared(srw) } else { srw_unlock_excl(srw) },
            || if shared { srw_shared(srw) } else { srw_excl(srw) },
            ms,
        )
    }

    fn InitOnceInitialize(p: *mut u64) {
        *p = 0;
    }
    fn InitOnceExecuteOnce(once: *mut u64, f: InitOnceFn, param: usize, ctx: *mut usize) -> BOOL {
        let a = &*(once as *const AtomicU64);
        loop {
            let v = a.load(Ordering::SeqCst);
            match v & 3 {
                2 => {
                    if !ctx.is_null() {
                        *ctx = (v & !3) as usize;
                    }
                    return TRUE;
                }
                0 => {
                    if a.compare_exchange(v, 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        let mut c = 0usize;
                        let ok = f(once, param, &mut c);
                        a.store(if ok != 0 { (c as u64 & !3) | 2 } else { 0 }, Ordering::SeqCst);
                        futex_wake(once as *const u32, i32::MAX);
                        return ok;
                    }
                }
                _ => {
                    futex_wait(once as *const u32, v as u32, Some(10));
                }
            }
        }
    }
    fn InitOnceBeginInitialize(once: *mut u64, flags: DWORD, pending: *mut BOOL, ctx: *mut usize) -> BOOL {
        let a = &*(once as *const AtomicU64);
        loop {
            let v = a.load(Ordering::SeqCst);
            match v & 3 {
                2 => {
                    *pending = FALSE;
                    if !ctx.is_null() {
                        *ctx = (v & !3) as usize;
                    }
                    return TRUE;
                }
                0 => {
                    if flags & 2 != 0 {
                        *pending = TRUE;
                        return TRUE;
                    }
                    if a.compare_exchange(v, 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        *pending = TRUE;
                        return TRUE;
                    }
                }
                _ => {
                    futex_wait(once as *const u32, v as u32, Some(10));
                }
            }
        }
    }
    fn InitOnceComplete(once: *mut u64, flags: DWORD, ctx: usize) -> BOOL {
        let a = &*(once as *const AtomicU64);
        a.store(if flags & 4 != 0 { 0 } else { (ctx as u64 & !3) | 2 }, Ordering::SeqCst);
        futex_wake(once as *const u32, i32::MAX);
        TRUE
    }

    fn WaitOnAddress(addr: *const u8, cmp: *const u8, size: usize, ms: DWORD) -> BOOL {
        let word = ((addr as usize) & !3) as *mut u32;
        let deadline = (ms != INFINITE).then(|| std::time::Instant::now() + std::time::Duration::from_millis(ms as u64));
        loop {
            let differs = std::slice::from_raw_parts(addr, size) != std::slice::from_raw_parts(cmp, size);
            if differs {
                return TRUE;
            }
            let val = *word;
            let mut wait = deadline.map(|d| d.saturating_duration_since(std::time::Instant::now()).as_millis() as u32);
            if wait == Some(0) {
                set_last_error(ERROR_TIMEOUT);
                return FALSE;
            }
            if size != 4 {
                wait = Some(wait.map_or(5, |w| w.min(5)));
            }
            futex_wait(word, val, wait);
        }
    }
    fn WakeByAddressSingle(addr: *const u8) {
        futex_wake(((addr as usize) & !3) as *const u32, 1);
    }
    fn WakeByAddressAll(addr: *const u8) {
        futex_wake(((addr as usize) & !3) as *const u32, i32::MAX);
    }

    fn CreateEventW(_attrs: usize, manual: BOOL, initial: BOOL, name: LPCWSTR) -> HANDLE {
        create_named(opt_wname(name), is_event, || make_event(manual != 0, initial != 0))
    }
    fn CreateEventA(_attrs: usize, manual: BOOL, initial: BOOL, name: LPCSTR) -> HANDLE {
        create_named(opt_aname(name), is_event, || make_event(manual != 0, initial != 0))
    }
    fn CreateEventExW(_attrs: usize, name: LPCWSTR, flags: DWORD, _access: DWORD) -> HANDLE {
        create_named(opt_wname(name), is_event, || make_event(flags & 1 != 0, flags & 2 != 0))
    }
    fn CreateEventExA(_attrs: usize, name: LPCSTR, flags: DWORD, _access: DWORD) -> HANDLE {
        create_named(opt_aname(name), is_event, || make_event(flags & 1 != 0, flags & 2 != 0))
    }
    fn OpenEventW(_access: DWORD, _inherit: BOOL, name: LPCWSTR) -> HANDLE {
        open_named(&strings::w_ptr_to_string(name), is_event)
    }
    fn OpenEventA(_access: DWORD, _inherit: BOOL, name: LPCSTR) -> HANDLE {
        open_named(&strings::a_to_string(name), is_event)
    }
    fn SetEvent(h: HANDLE) -> BOOL {
        match handles::get(h).as_deref() {
            Some(Kind::Event(e)) => {
                handles::locked_change(|| e.signaled.store(true, Ordering::SeqCst));
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }
    fn ResetEvent(h: HANDLE) -> BOOL {
        match handles::get(h).as_deref() {
            Some(Kind::Event(e)) => {
                handles::locked_change(|| e.signaled.store(false, Ordering::SeqCst));
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }
    fn PulseEvent(h: HANDLE) -> BOOL {
        match handles::get(h).as_deref() {
            Some(Kind::Event(e)) => {
                handles::locked_change(|| e.signaled.store(true, Ordering::SeqCst));
                std::thread::yield_now();
                handles::locked_change(|| e.signaled.store(false, Ordering::SeqCst));
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }

    fn CreateMutexW(_attrs: usize, owned: BOOL, name: LPCWSTR) -> HANDLE {
        create_named(opt_wname(name), is_mutex, || make_mutex(owned != 0))
    }
    fn CreateMutexA(_attrs: usize, owned: BOOL, name: LPCSTR) -> HANDLE {
        create_named(opt_aname(name), is_mutex, || make_mutex(owned != 0))
    }
    fn CreateMutexExW(_attrs: usize, name: LPCWSTR, flags: DWORD, _access: DWORD) -> HANDLE {
        create_named(opt_wname(name), is_mutex, || make_mutex(flags & 1 != 0))
    }
    fn CreateMutexExA(_attrs: usize, name: LPCSTR, flags: DWORD, _access: DWORD) -> HANDLE {
        create_named(opt_aname(name), is_mutex, || make_mutex(flags & 1 != 0))
    }
    fn OpenMutexW(_access: DWORD, _inherit: BOOL, name: LPCWSTR) -> HANDLE {
        open_named(&strings::w_ptr_to_string(name), is_mutex)
    }
    fn OpenMutexA(_access: DWORD, _inherit: BOOL, name: LPCSTR) -> HANDLE {
        open_named(&strings::a_to_string(name), is_mutex)
    }
    fn ReleaseMutex(h: HANDLE) -> BOOL {
        match handles::get(h).as_deref() {
            Some(Kind::Mutex(m)) => {
                if handles::release_mutex(m, handles::current_tid()) {
                    TRUE
                } else {
                    set_last_error(ERROR_NOT_OWNER);
                    FALSE
                }
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }

    fn CreateSemaphoreW(_attrs: usize, initial: i32, max: i32, name: LPCWSTR) -> HANDLE {
        if max <= 0 || initial < 0 || initial > max {
            set_last_error(ERROR_INVALID_PARAMETER);
            return 0;
        }
        create_named(opt_wname(name), is_sem, || make_sem(initial, max))
    }
    fn CreateSemaphoreA(_attrs: usize, initial: i32, max: i32, name: LPCSTR) -> HANDLE {
        if max <= 0 || initial < 0 || initial > max {
            set_last_error(ERROR_INVALID_PARAMETER);
            return 0;
        }
        create_named(opt_aname(name), is_sem, || make_sem(initial, max))
    }
    fn CreateSemaphoreExW(_attrs: usize, initial: i32, max: i32, name: LPCWSTR, _flags: DWORD, _access: DWORD) -> HANDLE {
        CreateSemaphoreW(0, initial, max, name)
    }
    fn CreateSemaphoreExA(_attrs: usize, initial: i32, max: i32, name: LPCSTR, _flags: DWORD, _access: DWORD) -> HANDLE {
        CreateSemaphoreA(0, initial, max, name)
    }
    fn OpenSemaphoreW(_access: DWORD, _inherit: BOOL, name: LPCWSTR) -> HANDLE {
        open_named(&strings::w_ptr_to_string(name), is_sem)
    }
    fn ReleaseSemaphore(h: HANDLE, count: i32, prev: *mut i32) -> BOOL {
        match handles::get(h).as_deref() {
            Some(Kind::Semaphore(s)) => handles::locked_change(|| {
                let cur = s.count.load(Ordering::SeqCst);
                if count <= 0 || cur as i64 + count as i64 > s.max as i64 {
                    set_last_error(ERROR_TOO_MANY_POSTS);
                    return FALSE;
                }
                if !prev.is_null() {
                    *prev = cur;
                }
                s.count.store(cur + count, Ordering::SeqCst);
                TRUE
            }),
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }

    fn CreateWaitableTimerW(_attrs: usize, manual: BOOL, name: LPCWSTR) -> HANDLE {
        create_named(opt_wname(name), is_timer, || make_timer(manual != 0))
    }
    fn CreateWaitableTimerA(_attrs: usize, manual: BOOL, name: LPCSTR) -> HANDLE {
        create_named(opt_aname(name), is_timer, || make_timer(manual != 0))
    }
    fn CreateWaitableTimerExW(_attrs: usize, name: LPCWSTR, flags: DWORD, _access: DWORD) -> HANDLE {
        create_named(opt_wname(name), is_timer, || make_timer(flags & 1 != 0))
    }
    fn OpenWaitableTimerW(_access: DWORD, _inherit: BOOL, name: LPCWSTR) -> HANDLE {
        open_named(&strings::w_ptr_to_string(name), is_timer)
    }
    fn SetWaitableTimer(h: HANDLE, due: *const i64, period: i32, _routine: usize, _arg: usize, _resume: BOOL) -> BOOL {
        match handles::get(h).as_deref() {
            Some(Kind::Timer(t)) if !due.is_null() => {
                timer_arm(t, *due, period);
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }
    fn SetWaitableTimerEx(h: HANDLE, due: *const i64, period: i32, _routine: usize, _arg: usize, _ctx: usize, _delay: DWORD) -> BOOL {
        SetWaitableTimer(h, due, period, 0, 0, 0)
    }
    fn CancelWaitableTimer(h: HANDLE) -> BOOL {
        match handles::get(h).as_deref() {
            Some(Kind::Timer(t)) => {
                handles::locked_change(|| t.due_ns.store(0, Ordering::SeqCst));
                TRUE
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }

    fn WaitForSingleObject(h: HANDLE, ms: DWORD) -> DWORD {
        wait_single(h, ms)
    }
    fn WaitForSingleObjectEx(h: HANDLE, ms: DWORD, _alertable: BOOL) -> DWORD {
        wait_single(h, ms)
    }
    fn WaitForMultipleObjects(n: DWORD, hs: *const HANDLE, all: BOOL, ms: DWORD) -> DWORD {
        wait_multi(n, hs, all, ms)
    }
    fn WaitForMultipleObjectsEx(n: DWORD, hs: *const HANDLE, all: BOOL, ms: DWORD, _alertable: BOOL) -> DWORD {
        wait_multi(n, hs, all, ms)
    }
    fn SignalObjectAndWait(signal: HANDLE, wait: HANDLE, ms: DWORD, _alertable: BOOL) -> DWORD {
        match handles::get(signal).as_deref() {
            Some(Kind::Event(_)) => {
                SetEvent(signal);
            }
            Some(Kind::Mutex(_)) => {
                ReleaseMutex(signal);
            }
            Some(Kind::Semaphore(_)) => {
                ReleaseSemaphore(signal, 1, std::ptr::null_mut());
            }
            _ => {
                set_last_error(ERROR_INVALID_HANDLE);
                return WAIT_FAILED;
            }
        }
        wait_single(wait, ms)
    }
    fn Sleep(ms: DWORD) {
        if ms == INFINITE {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
    }
    fn SleepEx(ms: DWORD, _alertable: BOOL) -> DWORD {
        Sleep(ms);
        0
    }

    fn CloseHandle(h: HANDLE) -> BOOL {
        if h == CURRENT_PROCESS || h == CURRENT_THREAD {
            return TRUE;
        }
        if handles::close(h) {
            TRUE
        } else {
            set_last_error(ERROR_INVALID_HANDLE);
            FALSE
        }
    }
    fn DuplicateHandle(_src_proc: HANDLE, src: HANDLE, _dst_proc: HANDLE, dst: *mut HANDLE, _access: DWORD, _inherit: BOOL, options: DWORD) -> BOOL {
        let obj = if src == CURRENT_THREAD {
            Some(crate::shim::k32_thread::current_thread_obj())
        } else if src == CURRENT_PROCESS {
            Some(self_process_obj())
        } else {
            handles::get(src)
        };
        match obj {
            Some(o) => {
                if !dst.is_null() {
                    *dst = handles::insert_arc(o);
                }
                if options & 1 != 0 {
                    handles::close(src);
                }
                TRUE
            }
            None => {
                set_last_error(ERROR_INVALID_HANDLE);
                FALSE
            }
        }
    }
    fn GetHandleInformation(h: HANDLE, flags: *mut DWORD) -> BOOL {
        if handles::get(h).is_none() && h != CURRENT_PROCESS && h != CURRENT_THREAD {
            set_last_error(ERROR_INVALID_HANDLE);
            return FALSE;
        }
        if !flags.is_null() {
            *flags = 0;
        }
        TRUE
    }
    fn SetHandleInformation(_h: HANDLE, _mask: DWORD, _flags: DWORD) -> BOOL {
        TRUE
    }

    fn InterlockedIncrement(p: *mut i32) -> i32 {
        (*(p as *const AtomicI32)).fetch_add(1, Ordering::SeqCst).wrapping_add(1)
    }
    fn InterlockedDecrement(p: *mut i32) -> i32 {
        (*(p as *const AtomicI32)).fetch_sub(1, Ordering::SeqCst).wrapping_sub(1)
    }
    fn InterlockedExchange(p: *mut i32, v: i32) -> i32 {
        (*(p as *const AtomicI32)).swap(v, Ordering::SeqCst)
    }
    fn InterlockedExchangeAdd(p: *mut i32, v: i32) -> i32 {
        (*(p as *const AtomicI32)).fetch_add(v, Ordering::SeqCst)
    }
    fn InterlockedCompareExchange(p: *mut i32, new: i32, cmp: i32) -> i32 {
        match (*(p as *const AtomicI32)).compare_exchange(cmp, new, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(v) | Err(v) => v,
        }
    }
    fn InterlockedIncrement64(p: *mut i64) -> i64 {
        (*(p as *const AtomicI64)).fetch_add(1, Ordering::SeqCst).wrapping_add(1)
    }
    fn InterlockedDecrement64(p: *mut i64) -> i64 {
        (*(p as *const AtomicI64)).fetch_sub(1, Ordering::SeqCst).wrapping_sub(1)
    }
    fn InterlockedExchange64(p: *mut i64, v: i64) -> i64 {
        (*(p as *const AtomicI64)).swap(v, Ordering::SeqCst)
    }
    fn InterlockedExchangeAdd64(p: *mut i64, v: i64) -> i64 {
        (*(p as *const AtomicI64)).fetch_add(v, Ordering::SeqCst)
    }
    fn InterlockedCompareExchange64(p: *mut i64, new: i64, cmp: i64) -> i64 {
        match (*(p as *const AtomicI64)).compare_exchange(cmp, new, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(v) | Err(v) => v,
        }
    }

    fn InitializeSListHead(h: *mut SListHeader) {
        (*h).head = 0;
        (*h).depth = 0;
    }
    fn InterlockedPushEntrySList(h: *mut SListHeader, entry: *mut usize) -> *mut usize {
        let _g = SLIST_LOCK.lock().unwrap();
        let prev = (*h).head;
        *entry = prev;
        (*h).head = entry as usize;
        (*h).depth += 1;
        prev as *mut usize
    }
    fn InterlockedPopEntrySList(h: *mut SListHeader) -> *mut usize {
        let _g = SLIST_LOCK.lock().unwrap();
        let first = (*h).head as *mut usize;
        if !first.is_null() {
            (*h).head = *first;
            (*h).depth -= 1;
        }
        first
    }
    fn InterlockedFlushSList(h: *mut SListHeader) -> *mut usize {
        let _g = SLIST_LOCK.lock().unwrap();
        let first = (*h).head as *mut usize;
        (*h).head = 0;
        (*h).depth = 0;
        first
    }
    fn QueryDepthSList(h: *const SListHeader) -> u16 {
        (*h).depth as u16
    }
    fn RtlFirstEntrySList(h: *const SListHeader) -> *mut usize {
        (*h).head as *mut usize
    }
}

static SELF_PROC: Mutex<Option<Arc<Kind>>> = Mutex::new(None);

pub fn self_process_obj() -> Arc<Kind> {
    SELF_PROC
        .lock()
        .unwrap()
        .get_or_insert_with(|| Arc::new(Kind::Process(ProcObj { pid: AtomicU32::new(std::process::id()), done: AtomicBool::new(false), exit_code: AtomicU32::new(259) })))
        .clone()
}
