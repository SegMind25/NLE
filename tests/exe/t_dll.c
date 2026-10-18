#include <windows.h>
#include <string.h>
#include "check.h"

__declspec(dllimport) int helper_add(int, int);
__declspec(dllimport) int helper_process_attach_count(void);
__declspec(dllimport) int helper_thread_attach_count(void);
__declspec(dllimport) int helper_tls_value(void);
__declspec(dllimport) void helper_set_tls(int);
__declspec(dllimport) const char *helper_name(void);

static DWORD WINAPI worker(LPVOID p) {
    (void)p;
    int before = helper_tls_value();
    helper_set_tls(5);
    return before == 1234 && helper_tls_value() == 5 ? 0 : 1;
}

int main(void) {
    CHECK_EQ_INT(helper_add(20, 22), 42);
    CHECK_STR(helper_name(), "helper.dll");
    CHECK_EQ_INT(helper_process_attach_count(), 1);
    CHECK_EQ_INT(helper_tls_value(), 1234);
    helper_set_tls(9);
    CHECK_EQ_INT(helper_tls_value(), 9);

    HANDLE t = CreateThread(NULL, 0, worker, NULL, 0, NULL);
    WaitForSingleObject(t, 5000);
    DWORD code = 7;
    GetExitCodeThread(t, &code);
    CHECK_EQ_INT(code, 0);
    CloseHandle(t);
    CHECK_EQ_INT(helper_thread_attach_count(), 1);
    CHECK_EQ_INT(helper_tls_value(), 9);

    HMODULE h = LoadLibraryA("helper.dll");
    CHECK(h != NULL);
    HMODULE h2 = GetModuleHandleA("HELPER.DLL");
    CHECK(h == h2);
    typedef int (*AddFn)(int, int);
    AddFn add = (AddFn)GetProcAddress(h, "helper_add");
    CHECK(add != NULL);
    CHECK_EQ_INT(add(1, 2), 3);
    CHECK(GetProcAddress(h, "missing_export") == NULL);
    char path[MAX_PATH];
    GetModuleFileNameA(h, path, sizeof path);
    CHECK(strstr(path, "helper.dll") != NULL);
    CHECK_EQ_INT(helper_process_attach_count(), 1);

    HMODULE dyn = LoadLibraryA("helper.dll");
    CHECK(dyn == h);
    return finish();
}
