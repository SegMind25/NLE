//! Legacy thread-pool entry points (`QueueUserWorkItem`, `RegisterWaitForSingleObject`) as threads of their own, and
//! the kernel32 odds and ends event loops such as libuv's reach for: named pipe state, I/O counters, disk geometry.

use crate::runtime::paths;
use crate::runtime::strings::*;
use crate::shim::{k32_sync, k32_thread};
use crate::win::*;
use std::sync::atomic::{AtomicBool, Ordering};

const WT_EXECUTEONLYONCE: DWORD = 0x8;
const ERROR_NOT_FOUND: DWORD = 1168;

type WorkFn = unsafe extern "win64" fn(usize) -> DWORD;
type WaitCallback = unsafe extern "win64" fn(usize, u8);

unsafe extern "win64" fn work_item_thread(param: usize) -> DWORD {
    let (f, ctx) = *(param as *const (usize, usize));
    libc::free(param as *mut libc::c_void);
    let f: WorkFn = std::mem::transmute(f);
    f(ctx)
}

/// One registered wait: a thread that waits on the object and on `cancel`, and calls back.
struct Registration {
    cancel: HANDLE,
    thread: HANDLE,
    stop: AtomicBool,
    object: HANDLE,
    callback: usize,
    context: usize,
    millis: DWORD,
    flags: DWORD,
}

unsafe extern "win64" fn wait_thread(param: usize) -> DWORD {
    let reg = &*(param as *const Registration);
    loop {
        let handles = [reg.object, reg.cancel];
        let r = k32_sync::WaitForMultipleObjects(2, handles.as_ptr(), FALSE, reg.millis);
        if reg.stop.load(Ordering::SeqCst) || r == 1 {
            break;
        }
        let timed_out = r == WAIT_TIMEOUT;
        if r != 0 && !timed_out {
            break;
        }
        let f: WaitCallback = std::mem::transmute(reg.callback);
        f(reg.context, timed_out as u8);
        if reg.flags & WT_EXECUTEONLYONCE != 0 {
            break;
        }
    }
    0
}

unsafe fn unregister(wait: HANDLE, completion: HANDLE) -> BOOL {
    if wait == 0 {
        set_last_error(ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let reg = Box::from_raw(wait as *mut Registration);
    reg.stop.store(true, Ordering::SeqCst);
    k32_sync::SetEvent(reg.cancel);
    // INVALID_HANDLE_VALUE means "return once no callback is running"; any other handle is signalled afterwards
    if completion != 0 {
        k32_sync::WaitForSingleObject(reg.thread, INFINITE);
    }
    k32_sync::CloseHandle(reg.thread);
    k32_sync::CloseHandle(reg.cancel);
    if completion != 0 && completion != INVALID_HANDLE_VALUE {
        k32_sync::SetEvent(completion);
    }
    TRUE
}

crate::shims! { "kernel32" => POOL_SHIMS;
    fn QueueUserWorkItem(func: usize, context: usize, _flags: DWORD) -> BOOL {
        if func == 0 {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        let arg = libc::malloc(16) as *mut (usize, usize);
        *arg = (func, context);
        let h = k32_thread::create_thread(0, work_item_thread as *const () as usize, arg as usize, 0, std::ptr::null_mut());
        if h == 0 {
            libc::free(arg as *mut libc::c_void);
            set_last_error(ERROR_NOT_ENOUGH_MEMORY);
            return FALSE;
        }
        k32_sync::CloseHandle(h);
        TRUE
    }
    fn RegisterWaitForSingleObject(out: *mut HANDLE, object: HANDLE, callback: usize, context: usize, millis: DWORD, flags: DWORD) -> BOOL {
        let cancel = k32_sync::CreateEventW(0, TRUE, FALSE, std::ptr::null());
        let reg = Box::into_raw(Box::new(Registration { cancel, thread: 0, stop: AtomicBool::new(false), object, callback, context, millis, flags }));
        let thread = k32_thread::create_thread(0, wait_thread as *const () as usize, reg as usize, 0, std::ptr::null_mut());
        if thread == 0 {
            drop(Box::from_raw(reg));
            set_last_error(ERROR_NOT_ENOUGH_MEMORY);
            return FALSE;
        }
        (*reg).thread = thread;
        *out = reg as HANDLE;
        TRUE
    }
    fn UnregisterWait(wait: HANDLE) -> BOOL {
        unregister(wait, 0)
    }
    fn UnregisterWaitEx(wait: HANDLE, completion: HANDLE) -> BOOL {
        unregister(wait, completion)
    }
    fn AreFileApisANSI() -> BOOL {
        TRUE
    }
    fn SetFileApisToANSI() {}
    fn SetFileApisToOEM() {}
    fn SetNamedPipeHandleState(_h: HANDLE, _mode: *const DWORD, _max: *const DWORD, _timeout: *const DWORD) -> BOOL {
        TRUE
    }
    fn GetNamedPipeHandleStateA(_h: HANDLE, state: *mut DWORD, instances: *mut DWORD, max: *mut DWORD, timeout: *mut DWORD, _user: usize, _user_len: DWORD) -> BOOL {
        for p in [state, instances, max, timeout] {
            if !p.is_null() {
                *p = 0;
            }
        }
        TRUE
    }
    fn GetNamedPipeClientProcessId(_h: HANDLE, pid: *mut DWORD) -> BOOL {
        *pid = libc::getpid() as DWORD;
        TRUE
    }
    fn GetNamedPipeServerProcessId(_h: HANDLE, pid: *mut DWORD) -> BOOL {
        *pid = libc::getpid() as DWORD;
        TRUE
    }
    fn CancelSynchronousIo(_thread: HANDLE) -> BOOL {
        set_last_error(ERROR_NOT_FOUND);
        FALSE
    }
    fn GetProcessIoCounters(_h: HANDLE, counters: *mut u64) -> BOOL {
        std::ptr::write_bytes(counters, 0, 6);
        TRUE
    }
    // sectors per cluster, bytes per sector, free and total clusters of the file system holding `root`
    fn GetDiskFreeSpaceW(root: LPCWSTR, spc: *mut DWORD, bps: *mut DWORD, free: *mut DWORD, total: *mut DWORD) -> BOOL {
        let path = if root.is_null() { paths::cwd() } else { paths::resolve(&w_ptr_to_string(root)) };
        let Ok(c) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) else { return FALSE };
        let mut vfs: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut vfs) != 0 {
            set_last_error(ERROR_PATH_NOT_FOUND);
            return FALSE;
        }
        let unit = (vfs.f_frsize.max(512)) as u64;
        let clamp = |n: u64| n.min(u32::MAX as u64) as DWORD;
        for (p, v) in [(spc, unit / 512), (bps, 512), (free, vfs.f_bavail as u64), (total, vfs.f_blocks as u64)] {
            if !p.is_null() {
                *p = clamp(v);
            }
        }
        TRUE
    }
    fn K32GetModuleBaseNameW(_process: HANDLE, module: HMODULE, buf: LPWSTR, size: DWORD) -> DWORD {
        let mut full = vec![0u16; 1024];
        let n = crate::shim::k32_proc::GetModuleFileNameW(module, full.as_mut_ptr(), 1024) as usize;
        let name = String::from_utf16_lossy(&full[..n]);
        let base = name.rsplit(['\\', '/']).next().unwrap_or("").to_string();
        return_w(buf, size, &base)
    }
}
