//! Kernel object table (`HANDLE`s) and the wait engine behind `WaitForSingleObject` & co.
//!
//! All state transitions of waitable objects happen under one global lock and every change
//! wakes all waiters, which then re-evaluate their predicates. That keeps `WaitForMultipleObjects`
//! with `bWaitAll` correct without per-object lock ordering.

use crate::win::*;
use std::cell::Cell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub struct FileObj {
    pub fd: AtomicI32,
    pub owns_fd: bool,
    pub is_tty: bool,
    pub is_dir: bool,
    pub is_pipe: bool,
    pub append: bool,
    pub path: Option<PathBuf>,
    pub delete_on_close: Option<PathBuf>,
    pub console_mode: AtomicU32,
    pub console_attr: AtomicU32,
}

impl FileObj {
    pub fn new(fd: i32, owns_fd: bool) -> FileObj {
        let is_tty = unsafe { libc::isatty(fd) } == 1;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let (is_dir, is_pipe) = if unsafe { libc::fstat(fd, &mut st) } == 0 {
            let fmt = st.st_mode & libc::S_IFMT;
            (fmt == libc::S_IFDIR, fmt == libc::S_IFIFO || fmt == libc::S_IFSOCK)
        } else {
            (false, false)
        };
        FileObj {
            fd: AtomicI32::new(fd),
            owns_fd,
            is_tty,
            is_dir,
            is_pipe,
            append: false,
            path: None,
            delete_on_close: None,
            console_mode: AtomicU32::new(0),
            console_attr: AtomicU32::new(7),
        }
    }

    pub fn fd(&self) -> i32 {
        self.fd.load(Ordering::Relaxed)
    }
}

impl Drop for FileObj {
    fn drop(&mut self) {
        let fd = self.fd.load(Ordering::Relaxed);
        if self.owns_fd && fd >= 0 {
            unsafe { libc::close(fd) };
        }
        if let Some(p) = &self.delete_on_close {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub struct EventObj {
    pub manual: bool,
    pub signaled: AtomicBool,
}

pub struct MutexObj {
    pub owner: AtomicU32,
    pub count: AtomicU32,
}

pub struct SemObj {
    pub count: AtomicI32,
    pub max: i32,
}

pub struct ThreadObj {
    pub tid: AtomicU32,
    pub done: AtomicBool,
    pub exit_code: AtomicU32,
    pub suspended: Mutex<bool>,
    pub resume: Condvar,
    pub pthread: AtomicU64,
}

pub struct ProcObj {
    pub pid: AtomicU32,
    pub done: AtomicBool,
    pub exit_code: AtomicU32,
}

pub struct FindEntry {
    pub name: String,
    pub path: PathBuf,
}

pub struct FindObj {
    pub entries: Mutex<(Vec<FindEntry>, usize)>,
}

pub struct TimerObj {
    pub manual: bool,
    pub signaled: AtomicBool,
    /// Monotonic deadline in ns, 0 when the timer is not armed.
    pub due_ns: AtomicU64,
    pub period_ms: AtomicU32,
}

pub struct MappingObj {
    pub fd: i32,
    pub owns_fd: bool,
    pub size: u64,
    pub protect: u32,
}

impl Drop for MappingObj {
    fn drop(&mut self) {
        if self.owns_fd && self.fd >= 0 {
            unsafe { libc::close(self.fd) };
        }
    }
}

pub struct Completion {
    pub bytes: u32,
    pub key: usize,
    pub overlapped: usize,
    /// the NTSTATUS of the operation that finished; 0 for a packet the program posted itself
    pub status: u32,
}

pub struct IocpObj {
    pub queue: Mutex<VecDeque<Completion>>,
    pub cv: Condvar,
}

/// A job object: the limits a program set (kept as the `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` it passed, 144 bytes
/// on x64) and the child processes assigned to it. Closing the last handle kills the members when the program
/// asked for `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
pub struct JobObj {
    pub limits: Mutex<[u8; 144]>,
    pub members: Mutex<Vec<Arc<Kind>>>,
}

impl JobObj {
    pub fn kill_members(&self) {
        for m in self.members.lock().unwrap().iter() {
            if let Kind::Process(p) = &**m {
                let pid = p.pid.load(Ordering::SeqCst);
                if pid != 0 && !p.done.load(Ordering::SeqCst) && pid != std::process::id() {
                    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                }
            }
        }
    }
}

impl Drop for JobObj {
    fn drop(&mut self) {
        let flags = u32::from_le_bytes(self.limits.lock().unwrap()[16..20].try_into().unwrap());
        if flags & 0x2000 != 0 {
            self.kill_members();
        }
    }
}

pub enum Kind {
    Job(JobObj),
    File(FileObj),
    Event(EventObj),
    Mutex(MutexObj),
    Semaphore(SemObj),
    Thread(ThreadObj),
    Process(ProcObj),
    Find(FindObj),
    Timer(TimerObj),
    Mapping(MappingObj),
    Iocp(IocpObj),
}

struct Table {
    slots: Vec<Option<Arc<Kind>>>,
    free: Vec<usize>,
}

static TABLE: Mutex<Table> = Mutex::new(Table { slots: Vec::new(), free: Vec::new() });

fn to_handle(idx: usize) -> HANDLE {
    (idx + 1) * 4
}

fn to_index(h: HANDLE) -> Option<usize> {
    if h == 0 || h % 4 != 0 || h > (1 << 28) {
        None
    } else {
        Some(h / 4 - 1)
    }
}

pub fn insert(kind: Kind) -> HANDLE {
    insert_arc(Arc::new(kind))
}

pub fn insert_arc(obj: Arc<Kind>) -> HANDLE {
    let mut t = TABLE.lock().unwrap();
    if let Some(i) = t.free.pop() {
        t.slots[i] = Some(obj);
        to_handle(i)
    } else {
        t.slots.push(Some(obj));
        to_handle(t.slots.len() - 1)
    }
}

pub fn get(h: HANDLE) -> Option<Arc<Kind>> {
    let i = to_index(h)?;
    TABLE.lock().unwrap().slots.get(i).and_then(|s| s.clone())
}

pub fn close(h: HANDLE) -> bool {
    let Some(i) = to_index(h) else { return false };
    let removed = {
        let mut t = TABLE.lock().unwrap();
        match t.slots.get_mut(i) {
            Some(slot @ Some(_)) => {
                let o = slot.take();
                t.free.push(i);
                o
            }
            _ => None,
        }
    };
    removed.is_some()
}

pub fn duplicate(h: HANDLE) -> Option<HANDLE> {
    get(h).map(insert_arc)
}

// ---- standard handles -------------------------------------------------------------------
static STD_HANDLES: [AtomicUsize; 3] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

pub fn init_std_handles() {
    for fd in 0..3 {
        let h = insert(Kind::File(FileObj::new(fd, false)));
        STD_HANDLES[fd as usize].store(h, Ordering::SeqCst);
    }
}

pub fn std_handle(which: DWORD) -> HANDLE {
    let idx = match which {
        STD_INPUT_HANDLE => 0,
        STD_OUTPUT_HANDLE => 1,
        STD_ERROR_HANDLE => 2,
        _ => return INVALID_HANDLE_VALUE,
    };
    STD_HANDLES[idx].load(Ordering::SeqCst)
}

pub fn set_std_handle(which: DWORD, h: HANDLE) -> bool {
    let idx = match which {
        STD_INPUT_HANDLE => 0,
        STD_OUTPUT_HANDLE => 1,
        STD_ERROR_HANDLE => 2,
        _ => return false,
    };
    STD_HANDLES[idx].store(h, Ordering::SeqCst);
    true
}

// ---- time and thread ids ------------------------------------------------------------------
pub fn now_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

thread_local! {
    static TID: Cell<u32> = const { Cell::new(0) };
}

pub fn current_tid() -> u32 {
    TID.with(|t| {
        if t.get() == 0 {
            // the system call, not libc::gettid(): that function only exists in glibc 2.30 and newer
            t.set(unsafe { libc::syscall(libc::SYS_gettid) } as u32);
        }
        t.get()
    })
}

// ---- waiting ---------------------------------------------------------------------------------
struct SyncState {
    lock: Mutex<()>,
    cv: Condvar,
}

static SYNC: SyncState = SyncState { lock: Mutex::new(()), cv: Condvar::new() };

/// Runs `f` under the global sync lock and wakes every waiter afterwards.
pub fn locked_change<R>(f: impl FnOnce() -> R) -> R {
    let g = SYNC.lock.lock().unwrap();
    let r = f();
    drop(g);
    SYNC.cv.notify_all();
    r
}

pub fn notify() {
    let g = SYNC.lock.lock().unwrap();
    drop(g);
    SYNC.cv.notify_all();
}

fn timer_poll(t: &TimerObj, now: u64) {
    let due = t.due_ns.load(Ordering::SeqCst);
    if due != 0 && now >= due {
        t.signaled.store(true, Ordering::SeqCst);
        let period = t.period_ms.load(Ordering::SeqCst) as u64;
        if period > 0 {
            let mut next = due + period * 1_000_000;
            if next <= now {
                next = now + period * 1_000_000;
            }
            t.due_ns.store(next, Ordering::SeqCst);
        } else {
            t.due_ns.store(0, Ordering::SeqCst);
        }
    }
}

fn is_signaled(k: &Kind, tid: u32, now: u64) -> Option<bool> {
    Some(match k {
        Kind::Event(e) => e.signaled.load(Ordering::SeqCst),
        Kind::Mutex(m) => {
            let o = m.owner.load(Ordering::SeqCst);
            o == 0 || o == tid
        }
        Kind::Semaphore(s) => s.count.load(Ordering::SeqCst) > 0,
        Kind::Thread(t) => t.done.load(Ordering::SeqCst),
        Kind::Process(p) => p.done.load(Ordering::SeqCst),
        Kind::Timer(t) => {
            timer_poll(t, now);
            t.signaled.load(Ordering::SeqCst)
        }
        Kind::File(f) => {
            let mut p = libc::pollfd { fd: f.fd(), events: libc::POLLIN, revents: 0 };
            unsafe { libc::poll(&mut p, 1, 0) > 0 }
        }
        _ => return None,
    })
}

fn acquire(k: &Kind, tid: u32) {
    match k {
        Kind::Event(e) => {
            if !e.manual {
                e.signaled.store(false, Ordering::SeqCst);
            }
        }
        Kind::Mutex(m) => {
            m.owner.store(tid, Ordering::SeqCst);
            m.count.fetch_add(1, Ordering::SeqCst);
        }
        Kind::Semaphore(s) => {
            s.count.fetch_sub(1, Ordering::SeqCst);
        }
        Kind::Timer(t) => {
            if !t.manual {
                t.signaled.store(false, Ordering::SeqCst);
            }
        }
        _ => {}
    }
}

/// Result of waiting: an object index, a timeout, or failure (not waitable).
pub fn wait_objects(objs: &[Arc<Kind>], wait_all: bool, timeout_ms: u32) -> u32 {
    let tid = current_tid();
    let deadline = (timeout_ms != INFINITE).then(|| Instant::now() + Duration::from_millis(timeout_ms as u64));
    let mut guard = SYNC.lock.lock().unwrap();
    loop {
        let now = now_ns();
        let mut states = Vec::with_capacity(objs.len());
        for o in objs {
            match is_signaled(o, tid, now) {
                Some(s) => states.push(s),
                None => {
                    set_last_error(ERROR_INVALID_HANDLE);
                    return WAIT_FAILED;
                }
            }
        }
        if wait_all {
            if states.iter().all(|&s| s) {
                for o in objs {
                    acquire(o, tid);
                }
                return WAIT_OBJECT_0;
            }
        } else if let Some(i) = states.iter().position(|&s| s) {
            acquire(&objs[i], tid);
            return WAIT_OBJECT_0 + i as u32;
        }
        let mut wait: Option<Duration> = None;
        if let Some(d) = deadline {
            let rem = d.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return WAIT_TIMEOUT;
            }
            wait = Some(rem);
        }
        for o in objs {
            if let Kind::Timer(t) = &**o {
                let due = t.due_ns.load(Ordering::SeqCst);
                if due != 0 {
                    let until = Duration::from_nanos(due.saturating_sub(now).max(1));
                    wait = Some(wait.map_or(until, |w| w.min(until)));
                }
            }
        }
        guard = match wait {
            Some(d) => SYNC.cv.wait_timeout(guard, d).unwrap().0,
            None => SYNC.cv.wait(guard).unwrap(),
        };
    }
}

pub fn release_mutex(m: &MutexObj, tid: u32) -> bool {
    locked_change(|| {
        if m.owner.load(Ordering::SeqCst) != tid {
            return false;
        }
        let c = m.count.fetch_sub(1, Ordering::SeqCst);
        if c <= 1 {
            m.count.store(0, Ordering::SeqCst);
            m.owner.store(0, Ordering::SeqCst);
        }
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(manual: bool, signaled: bool) -> Arc<Kind> {
        Arc::new(Kind::Event(EventObj { manual, signaled: AtomicBool::new(signaled) }))
    }

    #[test]
    fn handles_are_multiples_of_four_and_reusable() {
        let h1 = insert(Kind::Event(EventObj { manual: true, signaled: AtomicBool::new(false) }));
        assert_eq!(h1 % 4, 0);
        assert!(get(h1).is_some());
        assert!(close(h1));
        assert!(get(h1).is_none());
        assert!(!close(h1));
    }

    #[test]
    fn auto_reset_event_is_consumed() {
        let e = event(false, true);
        assert_eq!(wait_objects(&[e.clone()], false, 0), WAIT_OBJECT_0);
        assert_eq!(wait_objects(&[e], false, 0), WAIT_TIMEOUT);
    }

    #[test]
    fn manual_reset_event_stays_signaled() {
        let e = event(true, true);
        assert_eq!(wait_objects(&[e.clone()], false, 0), WAIT_OBJECT_0);
        assert_eq!(wait_objects(&[e], false, 0), WAIT_OBJECT_0);
    }

    #[test]
    fn wait_all_needs_every_object() {
        let a = event(true, true);
        let b = event(true, false);
        assert_eq!(wait_objects(&[a.clone(), b.clone()], true, 0), WAIT_TIMEOUT);
        if let Kind::Event(e) = &*b {
            e.signaled.store(true, Ordering::SeqCst);
        }
        assert_eq!(wait_objects(&[a, b], true, 0), WAIT_OBJECT_0);
    }

    #[test]
    fn wait_wakes_on_signal_from_other_thread() {
        let e = event(true, false);
        let e2 = e.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            locked_change(|| {
                if let Kind::Event(ev) = &*e2 {
                    ev.signaled.store(true, Ordering::SeqCst);
                }
            });
        });
        assert_eq!(wait_objects(&[e], false, 5000), WAIT_OBJECT_0);
        t.join().unwrap();
    }
}
