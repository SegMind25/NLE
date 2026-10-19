#include <windows.h>
#include <shellapi.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <wchar.h>
#include "check.h"

static int cmp_int(const void *a, const void *b) { return *(const int *)a - *(const int *)b; }
static int atexit_order = 0;
static void at1(void) { atexit_order = atexit_order * 10 + 1; }
static void at2(void) { atexit_order = atexit_order * 10 + 2; printf("atexit order %d\n", atexit_order); }

int main(int argc, char **argv) {
    CHECK_EQ_INT(argc, 3);
    CHECK_STR(argv[1], "first arg");
    CHECK_STR(argv[2], "second");

    char buf[512];
    CHECK(SetEnvironmentVariableA("NLE_T1", "value1"));
    CHECK_EQ_INT(GetEnvironmentVariableA("NLE_T1", buf, sizeof buf), 6);
    CHECK_STR(buf, "value1");
    CHECK_EQ_INT(GetEnvironmentVariableA("nle_t1", buf, sizeof buf), 6);
    CHECK_EQ_INT(GetEnvironmentVariableA("NLE_T1", buf, 3), 7);
    CHECK_EQ_INT(GetEnvironmentVariableA("NLE_NOPE", buf, sizeof buf), 0);
    CHECK_EQ_INT(GetLastError(), ERROR_ENVVAR_NOT_FOUND);
    ExpandEnvironmentStringsA("[%NLE_T1%][%NOPE_NOT_SET%]", buf, sizeof buf);
    CHECK_STR(buf, "[value1][%NOPE_NOT_SET%]");
    CHECK(GetEnvironmentVariableA("USERPROFILE", buf, sizeof buf) > 0);
    CHECK(strncmp(buf, "C:\\", 3) == 0);
    SetEnvironmentVariableA("NLE_T1", NULL);
    CHECK_EQ_INT(GetEnvironmentVariableA("NLE_T1", buf, sizeof buf), 0);
    LPCH block = GetEnvironmentStrings();
    int vars = 0;
    for (LPCH p = block; *p; p += strlen(p) + 1) vars++;
    CHECK(vars > 5);
    FreeEnvironmentStringsA(block);

    LPWSTR cmd = GetCommandLineW();
    int n = 0;
    LPWSTR *wargv = CommandLineToArgvW(cmd, &n);
    CHECK_EQ_INT(n, 3);
    CHECK(wcscmp(wargv[1], L"first arg") == 0);
    LocalFree(wargv);
    CHECK(strstr(GetCommandLineA(), "\"first arg\"") != NULL);

    char exe[MAX_PATH];
    DWORD len = GetModuleFileNameA(NULL, exe, sizeof exe);
    CHECK(len > 0 && exe[1] == ':');
    CHECK(strstr(exe, "t_system.exe") != NULL);
    HMODULE self = GetModuleHandleA(NULL);
    CHECK(self != NULL && ((char *)self)[0] == 'M' && ((char *)self)[1] == 'Z');
    CHECK(GetModuleHandleA("kernel32.dll") != NULL);
    CHECK(GetModuleHandleA("KERNEL32") != NULL);
    HMODULE k32 = LoadLibraryA("kernel32.dll");
    CHECK(k32 != NULL);
    CHECK(GetProcAddress(k32, "GetTickCount64") != NULL);
    CHECK(GetProcAddress(k32, "NoSuchFunctionAnywhere") == NULL);
    CHECK_EQ_INT(GetLastError(), ERROR_PROC_NOT_FOUND);
    CHECK(LoadLibraryA("definitely_missing_library.dll") == NULL);
    CHECK_EQ_INT(GetLastError(), ERROR_MOD_NOT_FOUND);
    CHECK(LoadLibraryA("user32.dll") != NULL);
    CHECK(GetProcAddress(LoadLibraryA("advapi32.dll"), "GetUserNameA") != NULL);

    OSVERSIONINFOEXW vi = { sizeof vi };
    typedef LONG (WINAPI *RtlGetVersionFn)(OSVERSIONINFOEXW *);
    RtlGetVersionFn rgv = (RtlGetVersionFn)GetProcAddress(GetModuleHandleA("ntdll.dll"), "RtlGetVersion");
    CHECK(rgv != NULL);
    CHECK_EQ_INT(rgv(&vi), 0);
    CHECK_EQ_INT(vi.dwMajorVersion, 10);
    BOOL wow = TRUE;
    IsWow64Process(GetCurrentProcess(), &wow);
    CHECK(!wow);
    char name[64];
    DWORD nlen = sizeof name;
    CHECK(GetComputerNameA(name, &nlen) && nlen > 0);
    nlen = sizeof name;
    CHECK(GetUserNameA(name, &nlen) && nlen > 1);
    GetSystemDirectoryA(buf, sizeof buf);
    CHECK_STR(buf, "C:\\windows\\system32");
    CHECK(IsProcessorFeaturePresent(PF_XMMI64_INSTRUCTIONS_AVAILABLE));

    LARGE_INTEGER f, c1, c2;
    QueryPerformanceFrequency(&f);
    CHECK_EQ_INT(f.QuadPart, 10000000);
    QueryPerformanceCounter(&c1);
    DWORD t0 = GetTickCount();
    Sleep(120);
    QueryPerformanceCounter(&c2);
    DWORD dt = GetTickCount() - t0;
    CHECK(dt >= 100 && dt < 1000);
    CHECK((c2.QuadPart - c1.QuadPart) >= 1000000);
    CHECK(GetTickCount64() > 0);
    SYSTEMTIME st, lt;
    GetSystemTime(&st);
    GetLocalTime(&lt);
    CHECK(st.wYear >= 2024 && st.wMonth >= 1 && st.wMonth <= 12);
    FILETIME ft;
    SystemTimeToFileTime(&st, &ft);
    SYSTEMTIME back;
    FileTimeToSystemTime(&ft, &back);
    CHECK_EQ_INT(back.wHour, st.wHour);
    CHECK_EQ_INT(back.wSecond, st.wSecond);
    time_t now = time(NULL);
    struct tm *tmv = gmtime(&now);
    CHECK_EQ_INT(tmv->tm_year + 1900, st.wYear);
    char tb[64];
    strftime(tb, sizeof tb, "%Y-%m-%d", tmv);
    CHECK_EQ_INT(strlen(tb), 10);
    struct tm tmx = { .tm_year = 100, .tm_mon = 0, .tm_mday = 1, .tm_hour = 12 };
    CHECK(mktime(&tmx) > 946000000);

    srand(1);
    CHECK_EQ_INT(rand(), 41);
    CHECK_EQ_INT(rand(), 18467);
    int arr[] = { 5, 3, 9, 1, 7, 2 };
    qsort(arr, 6, sizeof(int), cmp_int);
    CHECK_EQ_INT(arr[0], 1);
    CHECK_EQ_INT(arr[5], 9);
    int key = 7;
    int *found = bsearch(&key, arr, 6, sizeof(int), cmp_int);
    CHECK(found && *found == 7);

    char out[256];
    snprintf(out, sizeof out, "%d|%5.2f|%s|%x|%I64d|%zu|%c|%ls|%08.3f|%-4d|", 42, 3.14159, "str", 255, 1LL << 40, (size_t)99, 'Z', L"wide", 2.5, 7);
    CHECK_STR(out, "42| 3.14|str|ff|1099511627776|99|Z|wide|0002.500|7   |");
    CHECK_EQ_INT(snprintf(out, 5, "abcdefgh"), 8);
    CHECK_STR(out, "abcd");
    int a1 = 0; char w1[16]; double d1 = 0;
    CHECK_EQ_INT(sscanf("12 word 1.5e2", "%d %15s %lf", &a1, w1, &d1), 3);
    CHECK_EQ_INT(a1, 12);
    CHECK_STR(w1, "word");
    CHECK(d1 == 150.0);
    CHECK_EQ_INT(strtol("0x1F", NULL, 16), 31);
    CHECK_EQ_INT(strtol("-77abc", NULL, 10), -77);
    CHECK(strtod("2.5e3", NULL) == 2500.0);
    CHECK_EQ_INT(atoi("  123"), 123);
    char tok[] = "a,b;c";
    char *ctx = NULL;
    CHECK_STR(strtok_s(tok, ",;", &ctx), "a");
    CHECK_STR(strtok_s(NULL, ",;", &ctx), "b");
    char ov[] = "abcdef";
    memmove(ov + 2, ov, 4);
    CHECK_STR(ov, "ababcd");
    CHECK(_stricmp("HeLLo", "hello") == 0);
    CHECK(strstr("find the needle here", "needle") != NULL);
    CHECK_EQ_INT(toupper('a'), 'A');
    wchar_t wb[32];
    swprintf(wb, 32, L"%d-%ls", 5, L"x");
    CHECK(wcscmp(wb, L"5-x") == 0);
    CHECK_EQ_INT(wcslen(L"héllo"), 5);
    CHECK_EQ_INT(abs(-5), 5);

    DWORD flags = FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS;
    CHECK(FormatMessageA(flags, NULL, ERROR_FILE_NOT_FOUND, 0, buf, sizeof buf, NULL) > 10);
    CHECK(strstr(buf, "cannot find the file") != NULL);
    char *alloc = NULL;
    FormatMessageA(FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_STRING | FORMAT_MESSAGE_ARGUMENT_ARRAY, "x%1!d!y", 0, 0, (LPSTR)&alloc, 0, (va_list *)&(DWORD_PTR[]){ 77 });
    CHECK(alloc && strcmp(alloc, "x77y") == 0);
    LocalFree(alloc);

    atexit(at1);
    atexit(at2);
    return finish();
}
