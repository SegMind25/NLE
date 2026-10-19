/* A tour of the Windows API as this program sees it: system facts, the registry, files, threads.
 *
 *   x86_64-w64-mingw32-gcc -O2 -o sysinfo.exe sysinfo.c
 *   nle sysinfo.exe
 *
 * Colours are used only when the output is a terminal.
 */
#include <windows.h>
#include <io.h>
#include <stdio.h>
#include <string.h>

static int colour;
#define C(code) (colour ? "\x1b[" code "m" : "")

static void heading(const char *text) { printf("\n%s%s%s\n", C("1;36"), text, C("0")); }
static void row(const char *key, const char *fmt, ...) __attribute__((format(printf, 2, 3)));
static void row(const char *key, const char *fmt, ...) {
    __builtin_va_list ap;
    printf("  %s%-18s%s ", C("2"), key, C("0"));
    __builtin_va_start(ap, fmt);
    vprintf(fmt, ap);
    __builtin_va_end(ap);
    printf("\n");
}

typedef struct { int from, to, count; } Slice;

static int is_prime(int n) {
    if (n < 2) return 0;
    for (int d = 2; (long long)d * d <= n; d++)
        if (n % d == 0) return 0;
    return 1;
}

static DWORD WINAPI count_primes(void *arg) {
    Slice *s = (Slice *)arg;
    s->count = 0;
    for (int n = s->from; n < s->to; n++) s->count += is_prime(n);
    return 0;
}

int main(void) {
    colour = _isatty(1);
    printf("%sWindows program%s running as a native Linux process\n", C("1;32"), C("0"));

    heading("System");
    SYSTEM_INFO si;
    GetNativeSystemInfo(&si);
    MEMORYSTATUSEX mem = {sizeof mem};
    GlobalMemoryStatusEx(&mem);
    char computer[64] = "", user[64] = "";
    DWORD n = sizeof computer;
    GetComputerNameA(computer, &n);
    n = sizeof user;
    GetUserNameA(user, &n);
    row("computer / user", "%s / %s", computer, user);
    row("processors", "%lu (x64: %s)", si.dwNumberOfProcessors, si.wProcessorArchitecture == PROCESSOR_ARCHITECTURE_AMD64 ? "yes" : "no");
    row("physical memory", "%.1f GiB", mem.ullTotalPhys / 1073741824.0);
    row("page size", "%lu bytes", si.dwPageSize);

    heading("Registry");
    HKEY key;
    char product[64] = "?", build[16] = "?";
    if (RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", 0, KEY_READ, &key) == ERROR_SUCCESS) {
        DWORD len = sizeof product;
        RegQueryValueExA(key, "ProductName", NULL, NULL, (BYTE *)product, &len);
        len = sizeof build;
        RegQueryValueExA(key, "CurrentBuildNumber", NULL, NULL, (BYTE *)build, &len);
        RegCloseKey(key);
    }
    row("ProductName", "%s", product);
    row("CurrentBuildNumber", "%s", build);

    heading("Files");
    char cwd[MAX_PATH], temp[MAX_PATH], file[MAX_PATH];
    GetCurrentDirectoryA(sizeof cwd, cwd);
    GetTempPathA(sizeof temp, temp);
    snprintf(file, sizeof file, "%sNLE_Demo.TXT", temp);
    row("current directory", "%s", cwd);
    row("temp directory", "%s", temp);
    HANDLE h = CreateFileA(file, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    DWORD written = 0;
    WriteFile(h, "written through CreateFile\n", 26, &written, NULL);
    CloseHandle(h);
    char lower[MAX_PATH];
    snprintf(lower, sizeof lower, "%snle_demo.txt", temp);
    row("case-insensitive", "%s", GetFileAttributesA(lower) != INVALID_FILE_ATTRIBUTES ? "nle_demo.txt found as NLE_Demo.TXT" : "not found");
    DeleteFileA(file);

    heading("Threads");
    enum { THREADS = 4, LIMIT = 2000000 };
    HANDLE th[THREADS];
    Slice part[THREADS];
    LARGE_INTEGER freq, t0, t1;
    QueryPerformanceFrequency(&freq);
    QueryPerformanceCounter(&t0);
    for (int i = 0; i < THREADS; i++) {
        part[i] = (Slice){i * (LIMIT / THREADS), (i + 1) * (LIMIT / THREADS), 0};
        th[i] = CreateThread(NULL, 0, count_primes, &part[i], 0, NULL);
    }
    WaitForMultipleObjects(THREADS, th, TRUE, INFINITE);
    QueryPerformanceCounter(&t1);
    int total = 0;
    for (int i = 0; i < THREADS; i++) {
        total += part[i].count;
        CloseHandle(th[i]);
    }
    row("primes below 2M", "%d", total);
    row("worker threads", "%d", THREADS);
    row("elapsed", "%.0f ms", (t1.QuadPart - t0.QuadPart) * 1000.0 / freq.QuadPart);

    printf("\n%sdone%s\n", C("1;32"), C("0"));
    return 0;
}
