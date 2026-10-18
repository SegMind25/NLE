//! kernel32: clocks, time conversion and time zones.

use crate::runtime::handles::now_ns;
use crate::runtime::teb::filetime_now;
use crate::win::*;

const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
const TICKS_PER_SEC: u64 = 10_000_000;

fn tick_ms() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000
}

fn local_offset_secs(unix: i64) -> i64 {
    let t = unix as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm.tm_gmtoff
}

pub fn filetime_to_systemtime(ft: u64, local: bool) -> SYSTEMTIME {
    let secs = (ft / TICKS_PER_SEC) as i64 - EPOCH_DIFF_SECS;
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        if local {
            libc::localtime_r(&t, &mut tm);
        } else {
            libc::gmtime_r(&t, &mut tm);
        }
    }
    SYSTEMTIME {
        year: (tm.tm_year + 1900) as WORD,
        month: (tm.tm_mon + 1) as WORD,
        day_of_week: tm.tm_wday as WORD,
        day: tm.tm_mday as WORD,
        hour: tm.tm_hour as WORD,
        minute: tm.tm_min as WORD,
        second: tm.tm_sec as WORD,
        milliseconds: ((ft % TICKS_PER_SEC) / 10_000) as WORD,
    }
}

pub fn systemtime_to_filetime(st: &SYSTEMTIME) -> Option<u64> {
    if st.month == 0 || st.month > 12 || st.day == 0 || st.day > 31 || st.hour > 23 || st.minute > 59 || st.second > 59 || st.milliseconds > 999 || st.year < 1601 {
        return None;
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = st.year as i32 - 1900;
    tm.tm_mon = st.month as i32 - 1;
    tm.tm_mday = st.day as i32;
    tm.tm_hour = st.hour as i32;
    tm.tm_min = st.minute as i32;
    tm.tm_sec = st.second as i32;
    let secs = unsafe { libc::timegm(&mut tm) } as i64;
    Some(((secs + EPOCH_DIFF_SECS) as u64) * TICKS_PER_SEC + st.milliseconds as u64 * 10_000)
}

unsafe fn fill_tzi(tzi: *mut TIME_ZONE_INFORMATION) {
    std::ptr::write_bytes(tzi as *mut u8, 0, std::mem::size_of::<TIME_ZONE_INFORMATION>());
    let now = filetime_now();
    let unix = (now / TICKS_PER_SEC) as i64 - EPOCH_DIFF_SECS;
    (*tzi).bias = -(local_offset_secs(unix) / 60) as LONG;
    let name = crate::runtime::strings::to_wide("UTC");
    for (i, c) in name.iter().enumerate() {
        (*tzi).standard_name[i] = *c;
        (*tzi).daylight_name[i] = *c;
    }
}

crate::shims! { "kernel32";
    fn GetSystemTimeAsFileTime(ft: *mut FILETIME) {
        *ft = FILETIME::from_u64(filetime_now());
    }
    fn GetSystemTimePreciseAsFileTime(ft: *mut FILETIME) {
        *ft = FILETIME::from_u64(filetime_now());
    }
    fn GetSystemTime(st: *mut SYSTEMTIME) {
        *st = filetime_to_systemtime(filetime_now(), false);
    }
    fn GetLocalTime(st: *mut SYSTEMTIME) {
        *st = filetime_to_systemtime(filetime_now(), true);
    }
    fn SetSystemTime(_st: *const SYSTEMTIME) -> BOOL {
        set_last_error(ERROR_PRIVILEGE_NOT_HELD);
        FALSE
    }
    fn SetLocalTime(_st: *const SYSTEMTIME) -> BOOL {
        set_last_error(ERROR_PRIVILEGE_NOT_HELD);
        FALSE
    }
    fn GetTickCount() -> DWORD {
        tick_ms() as DWORD
    }
    fn GetTickCount64() -> u64 {
        tick_ms()
    }
    fn QueryPerformanceCounter(out: *mut i64) -> BOOL {
        *out = (now_ns() / 100) as i64;
        TRUE
    }
    fn QueryPerformanceFrequency(out: *mut i64) -> BOOL {
        *out = TICKS_PER_SEC as i64;
        TRUE
    }
    fn QueryUnbiasedInterruptTime(out: *mut u64) -> BOOL {
        *out = now_ns() / 100;
        TRUE
    }
    fn QueryInterruptTime(out: *mut u64) {
        *out = now_ns() / 100;
    }
    fn QueryInterruptTimePrecise(out: *mut u64) {
        *out = now_ns() / 100;
    }
    fn FileTimeToSystemTime(ft: *const FILETIME, st: *mut SYSTEMTIME) -> BOOL {
        let v = (*ft).to_u64();
        if v >= 0x8000_0000_0000_0000 {
            set_last_error(ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        *st = filetime_to_systemtime(v, false);
        TRUE
    }
    fn SystemTimeToFileTime(st: *const SYSTEMTIME, ft: *mut FILETIME) -> BOOL {
        match systemtime_to_filetime(&*st) {
            Some(v) => {
                *ft = FILETIME::from_u64(v);
                TRUE
            }
            None => {
                set_last_error(ERROR_INVALID_PARAMETER);
                FALSE
            }
        }
    }
    fn FileTimeToLocalFileTime(ft: *const FILETIME, out: *mut FILETIME) -> BOOL {
        let v = (*ft).to_u64();
        let unix = (v / TICKS_PER_SEC) as i64 - EPOCH_DIFF_SECS;
        *out = FILETIME::from_u64((v as i64 + local_offset_secs(unix) * TICKS_PER_SEC as i64) as u64);
        TRUE
    }
    fn LocalFileTimeToFileTime(ft: *const FILETIME, out: *mut FILETIME) -> BOOL {
        let v = (*ft).to_u64();
        let unix = (v / TICKS_PER_SEC) as i64 - EPOCH_DIFF_SECS;
        *out = FILETIME::from_u64((v as i64 - local_offset_secs(unix) * TICKS_PER_SEC as i64) as u64);
        TRUE
    }
    fn SystemTimeToTzSpecificLocalTime(_tz: *const TIME_ZONE_INFORMATION, utc: *const SYSTEMTIME, local: *mut SYSTEMTIME) -> BOOL {
        match systemtime_to_filetime(&*utc) {
            Some(v) => {
                let unix = (v / TICKS_PER_SEC) as i64 - EPOCH_DIFF_SECS;
                let shifted = (v as i64 + local_offset_secs(unix) * TICKS_PER_SEC as i64) as u64;
                *local = filetime_to_systemtime(shifted, false);
                TRUE
            }
            None => {
                set_last_error(ERROR_INVALID_PARAMETER);
                FALSE
            }
        }
    }
    fn TzSpecificLocalTimeToSystemTime(_tz: *const TIME_ZONE_INFORMATION, local: *const SYSTEMTIME, utc: *mut SYSTEMTIME) -> BOOL {
        match systemtime_to_filetime(&*local) {
            Some(v) => {
                let unix = (v / TICKS_PER_SEC) as i64 - EPOCH_DIFF_SECS;
                let shifted = (v as i64 - local_offset_secs(unix) * TICKS_PER_SEC as i64) as u64;
                *utc = filetime_to_systemtime(shifted, false);
                TRUE
            }
            None => {
                set_last_error(ERROR_INVALID_PARAMETER);
                FALSE
            }
        }
    }
    fn CompareFileTime(a: *const FILETIME, b: *const FILETIME) -> i32 {
        let (x, y) = ((*a).to_u64(), (*b).to_u64());
        if x < y {
            -1
        } else if x > y {
            1
        } else {
            0
        }
    }
    fn GetTimeZoneInformation(tzi: *mut TIME_ZONE_INFORMATION) -> DWORD {
        fill_tzi(tzi);
        0
    }
    fn GetDynamicTimeZoneInformation(tzi: *mut u8) -> DWORD {
        std::ptr::write_bytes(tzi, 0, 432);
        fill_tzi(tzi as *mut TIME_ZONE_INFORMATION);
        0
    }
    fn GetSystemTimeAdjustment(adj: *mut DWORD, incr: *mut DWORD, disabled: *mut BOOL) -> BOOL {
        *adj = 156_250;
        *incr = 156_250;
        *disabled = TRUE;
        TRUE
    }
    fn GetSystemTimes(idle: *mut FILETIME, kernel: *mut FILETIME, user: *mut FILETIME) -> BOOL {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        let ft = |t: libc::timeval| FILETIME::from_u64(t.tv_sec as u64 * TICKS_PER_SEC + t.tv_usec as u64 * 10);
        if !idle.is_null() {
            *idle = FILETIME::default();
        }
        if !kernel.is_null() {
            *kernel = ft(ru.ru_stime);
        }
        if !user.is_null() {
            *user = ft(ru.ru_utime);
        }
        TRUE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemtime_roundtrip() {
        let st = SYSTEMTIME { year: 2024, month: 2, day_of_week: 0, day: 29, hour: 13, minute: 45, second: 7, milliseconds: 250 };
        let ft = systemtime_to_filetime(&st).unwrap();
        let back = filetime_to_systemtime(ft, false);
        assert_eq!((back.year, back.month, back.day, back.hour, back.minute, back.second, back.milliseconds), (2024, 2, 29, 13, 45, 7, 250));
        assert_eq!(back.day_of_week, 4);
    }

    #[test]
    fn rejects_invalid_dates() {
        let st = SYSTEMTIME { year: 2024, month: 13, day_of_week: 0, day: 1, hour: 0, minute: 0, second: 0, milliseconds: 0 };
        assert!(systemtime_to_filetime(&st).is_none());
    }
}
