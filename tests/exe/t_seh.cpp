#include <windows.h>
#include <csetjmp>
#include <csignal>
#include <cstdio>
#include <cstring>
#include <map>
#include <stdexcept>
#include <string>
#include <vector>
#include "check.h"

struct Guard {
    int *counter;
    explicit Guard(int *c) : counter(c) {}
    ~Guard() { ++*counter; }
};

static int g_dtors = 0;

static int thrower(int depth) {
    Guard g(&g_dtors);
    if (depth == 0) throw std::runtime_error("deep failure");
    return thrower(depth - 1) + 1;
}

struct MyError : std::exception {
    int code;
    explicit MyError(int c) : code(c) {}
    const char *what() const noexcept override { return "MyError"; }
};

static DWORD WINAPI thread_thrower(LPVOID) {
    try {
        throw MyError(77);
    } catch (const MyError &e) {
        return e.code;
    }
    return 0;
}

static LONG WINAPI fixup_handler(EXCEPTION_POINTERS *ep) {
    if (ep->ExceptionRecord->ExceptionCode == EXCEPTION_ACCESS_VIOLATION) {
        void *addr = (void *)ep->ExceptionRecord->ExceptionInformation[1];
        DWORD old;
        VirtualProtect((void *)((uintptr_t)addr & ~4095ULL), 4096, PAGE_READWRITE, &old);
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    if (ep->ExceptionRecord->ExceptionCode == 0xE0000001) {
        CHECK_EQ_INT(ep->ExceptionRecord->NumberParameters, 2);
        CHECK_EQ_INT(ep->ExceptionRecord->ExceptionInformation[0], 11);
        CHECK_EQ_INT(ep->ExceptionRecord->ExceptionInformation[1], 22);
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    return EXCEPTION_CONTINUE_SEARCH;
}

static jmp_buf g_jb;
static volatile int g_sig = 0;
static void on_fpe(int sig) {
    g_sig = sig;
    longjmp(g_jb, 1);
}

int main() {
    try {
        throw 42;
    } catch (int v) {
        CHECK_EQ_INT(v, 42);
    }

    try {
        thrower(5);
        CHECK(false);
    } catch (const std::runtime_error &e) {
        CHECK_STR(e.what(), "deep failure");
    }
    CHECK_EQ_INT(g_dtors, 6);

    try {
        try {
            throw MyError(5);
        } catch (const std::exception &e) {
            CHECK_STR(e.what(), "MyError");
            throw;
        }
    } catch (const MyError &e) {
        CHECK_EQ_INT(e.code, 5);
    }

    try {
        throw std::string("string payload");
    } catch (...) {
        CHECK(true);
    }

    std::vector<int> v = { 1, 2, 3 };
    try {
        v.at(10);
        CHECK(false);
    } catch (const std::out_of_range &) {
        CHECK(true);
    }

    std::map<std::string, int> m;
    m["a"] = 1;
    m["b"] = 2;
    CHECK_EQ_INT(m.size(), 2);

    HANDLE t = CreateThread(NULL, 0, thread_thrower, NULL, 0, NULL);
    WaitForSingleObject(t, 5000);
    DWORD code = 0;
    GetExitCodeThread(t, &code);
    CHECK_EQ_INT(code, 77);
    CloseHandle(t);

    void *veh = AddVectoredExceptionHandler(1, fixup_handler);
    char *page = (char *)VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_NOACCESS);
    page[10] = 'Q';
    CHECK_EQ_INT(page[10], 'Q');
    ULONG_PTR args[2] = { 11, 22 };
    RaiseException(0xE0000001, 0, 2, args);
    RemoveVectoredExceptionHandler(veh);
    VirtualFree(page, 0, MEM_RELEASE);

    signal(SIGFPE, on_fpe);
    if (setjmp(g_jb) == 0) {
        volatile int zero = 0;
        volatile int r = 100 / zero;
        (void)r;
        CHECK(false);
    } else {
        CHECK_EQ_INT(g_sig, SIGFPE);
    }

    signal(SIGSEGV, [](int s) { g_sig = s; longjmp(g_jb, 1); });
    if (setjmp(g_jb) == 0) {
        volatile int *np = nullptr;
        *np = 1;
        CHECK(false);
    } else {
        CHECK_EQ_INT(g_sig, SIGSEGV);
    }

    jmp_buf jb2;
    volatile int steps = 0;
    int rv = setjmp(jb2);
    if (rv == 0) {
        steps = 1;
        longjmp(jb2, 9);
    }
    CHECK_EQ_INT(rv, 9);
    CHECK_EQ_INT(steps, 1);
    return finish();
}
