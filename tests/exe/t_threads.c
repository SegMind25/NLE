#include <windows.h>
#include <string.h>
#include "check.h"

static CRITICAL_SECTION g_cs;
static volatile LONG g_counter = 0;
static long g_plain = 0;
static DWORD g_tls_index;
static __thread int g_thread_local = 7;
static SRWLOCK g_srw = SRWLOCK_INIT;
static CONDITION_VARIABLE g_cv = CONDITION_VARIABLE_INIT;
static int g_ready = 0;
static HANDLE g_event, g_mutex, g_sem;

static DWORD WINAPI worker_sum(LPVOID arg) {
    int n = (int)(INT_PTR)arg;
    for (int i = 0; i < 10000; i++) {
        InterlockedIncrement(&g_counter);
        EnterCriticalSection(&g_cs);
        g_plain++;
        LeaveCriticalSection(&g_cs);
    }
    return (DWORD)(n * 2);
}

static DWORD WINAPI worker_tls(LPVOID arg) {
    TlsSetValue(g_tls_index, arg);
    g_thread_local = (int)(INT_PTR)arg;
    Sleep(20);
    if ((INT_PTR)TlsGetValue(g_tls_index) != (INT_PTR)arg) return 1;
    if (g_thread_local != (int)(INT_PTR)arg) return 2;
    return 0;
}

static DWORD WINAPI worker_wait_event(LPVOID arg) {
    (void)arg;
    WaitForSingleObject(g_event, INFINITE);
    return 55;
}

static DWORD WINAPI worker_cv(LPVOID arg) {
    (void)arg;
    AcquireSRWLockExclusive(&g_srw);
    while (!g_ready) SleepConditionVariableSRW(&g_cv, &g_srw, INFINITE, 0);
    ReleaseSRWLockExclusive(&g_srw);
    return 66;
}

static DWORD WINAPI worker_mutex(LPVOID arg) {
    (void)arg;
    DWORD r = WaitForSingleObject(g_mutex, 5000);
    if (r != WAIT_OBJECT_0) return 99;
    g_plain += 1000;
    ReleaseMutex(g_mutex);
    return 0;
}

static DWORD WINAPI worker_exit(LPVOID arg) {
    (void)arg;
    ExitThread(123);
    return 0;
}

int main(void) {
    InitializeCriticalSection(&g_cs);

    HANDLE th[4];
    for (int i = 0; i < 4; i++) th[i] = CreateThread(NULL, 0, worker_sum, (LPVOID)(INT_PTR)(i + 1), 0, NULL);
    for (int i = 0; i < 4; i++) {
        CHECK(th[i] != NULL);
        CHECK_EQ_INT(WaitForSingleObject(th[i], 10000), WAIT_OBJECT_0);
        DWORD code = 0;
        CHECK(GetExitCodeThread(th[i], &code));
        CHECK_EQ_INT(code, (i + 1) * 2);
        CloseHandle(th[i]);
    }
    CHECK_EQ_INT(g_counter, 40000);
    CHECK_EQ_INT(g_plain, 40000);

    g_tls_index = TlsAlloc();
    CHECK(g_tls_index != TLS_OUT_OF_INDEXES);
    HANDLE tt[3];
    for (int i = 0; i < 3; i++) tt[i] = CreateThread(NULL, 0, worker_tls, (LPVOID)(INT_PTR)(100 + i), 0, NULL);
    WaitForMultipleObjects(3, tt, TRUE, 10000);
    for (int i = 0; i < 3; i++) {
        DWORD code = 99;
        GetExitCodeThread(tt[i], &code);
        CHECK_EQ_INT(code, 0);
        CloseHandle(tt[i]);
    }
    CHECK_EQ_INT(g_thread_local, 7);
    TlsFree(g_tls_index);

    g_event = CreateEventA(NULL, TRUE, FALSE, NULL);
    HANDLE ew = CreateThread(NULL, 0, worker_wait_event, NULL, 0, NULL);
    Sleep(50);
    CHECK_EQ_INT(WaitForSingleObject(ew, 0), WAIT_TIMEOUT);
    SetEvent(g_event);
    CHECK_EQ_INT(WaitForSingleObject(ew, 5000), WAIT_OBJECT_0);
    DWORD ec = 0;
    GetExitCodeThread(ew, &ec);
    CHECK_EQ_INT(ec, 55);
    CloseHandle(ew);
    CloseHandle(g_event);

    HANDLE cvt = CreateThread(NULL, 0, worker_cv, NULL, 0, NULL);
    Sleep(50);
    AcquireSRWLockExclusive(&g_srw);
    g_ready = 1;
    ReleaseSRWLockExclusive(&g_srw);
    WakeAllConditionVariable(&g_cv);
    CHECK_EQ_INT(WaitForSingleObject(cvt, 5000), WAIT_OBJECT_0);
    GetExitCodeThread(cvt, &ec);
    CHECK_EQ_INT(ec, 66);
    CloseHandle(cvt);

    g_mutex = CreateMutexA(NULL, FALSE, NULL);
    long before = g_plain;
    HANDLE mt[3];
    for (int i = 0; i < 3; i++) mt[i] = CreateThread(NULL, 0, worker_mutex, NULL, 0, NULL);
    WaitForMultipleObjects(3, mt, TRUE, 10000);
    CHECK_EQ_INT(g_plain - before, 3000);
    for (int i = 0; i < 3; i++) CloseHandle(mt[i]);
    CHECK_EQ_INT(WaitForSingleObject(g_mutex, 0), WAIT_OBJECT_0);
    CHECK_EQ_INT(WaitForSingleObject(g_mutex, 0), WAIT_OBJECT_0);
    ReleaseMutex(g_mutex);
    ReleaseMutex(g_mutex);
    CloseHandle(g_mutex);

    g_sem = CreateSemaphoreA(NULL, 2, 5, NULL);
    CHECK_EQ_INT(WaitForSingleObject(g_sem, 0), WAIT_OBJECT_0);
    CHECK_EQ_INT(WaitForSingleObject(g_sem, 0), WAIT_OBJECT_0);
    CHECK_EQ_INT(WaitForSingleObject(g_sem, 10), WAIT_TIMEOUT);
    LONG prev = -1;
    CHECK(ReleaseSemaphore(g_sem, 3, &prev));
    CHECK_EQ_INT(prev, 0);
    CHECK(!ReleaseSemaphore(g_sem, 10, NULL));
    CHECK_EQ_INT(GetLastError(), ERROR_TOO_MANY_POSTS);
    CloseHandle(g_sem);

    HANDLE ex = CreateThread(NULL, 0, worker_exit, NULL, 0, NULL);
    WaitForSingleObject(ex, 5000);
    GetExitCodeThread(ex, &ec);
    CHECK_EQ_INT(ec, 123);
    CloseHandle(ex);

    HANDLE susp = CreateThread(NULL, 0, worker_sum, (LPVOID)(INT_PTR)1, CREATE_SUSPENDED, NULL);
    Sleep(30);
    CHECK_EQ_INT(WaitForSingleObject(susp, 0), WAIT_TIMEOUT);
    CHECK_EQ_INT(ResumeThread(susp), 1);
    CHECK_EQ_INT(WaitForSingleObject(susp, 5000), WAIT_OBJECT_0);
    CloseHandle(susp);

    HANDLE timer = CreateWaitableTimerA(NULL, FALSE, NULL);
    LARGE_INTEGER due;
    due.QuadPart = -300000;
    DWORD t0 = GetTickCount();
    CHECK(SetWaitableTimer(timer, &due, 0, NULL, NULL, FALSE));
    CHECK_EQ_INT(WaitForSingleObject(timer, 5000), WAIT_OBJECT_0);
    DWORD dt = GetTickCount() - t0;
    CHECK(dt >= 20 && dt < 1000);
    CloseHandle(timer);

    HANDLE named1 = CreateEventA(NULL, TRUE, FALSE, "nle_test_event");
    HANDLE named2 = CreateEventA(NULL, TRUE, FALSE, "nle_test_event");
    CHECK_EQ_INT(GetLastError(), ERROR_ALREADY_EXISTS);
    SetEvent(named1);
    CHECK_EQ_INT(WaitForSingleObject(named2, 0), WAIT_OBJECT_0);

    CHECK_EQ_INT(GetCurrentThreadId() != 0, 1);
    CHECK(GetCurrentProcessId() != 0);
    DeleteCriticalSection(&g_cs);
    return finish();
}
