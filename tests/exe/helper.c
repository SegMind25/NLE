#include <windows.h>

static int g_process_attach = 0;
static int g_thread_attach = 0;
static __thread int g_tls_value = 1234;

__declspec(dllexport) int helper_add(int a, int b) { return a + b; }
__declspec(dllexport) int helper_process_attach_count(void) { return g_process_attach; }
__declspec(dllexport) int helper_thread_attach_count(void) { return g_thread_attach; }
__declspec(dllexport) int helper_tls_value(void) { return g_tls_value; }
__declspec(dllexport) void helper_set_tls(int v) { g_tls_value = v; }
__declspec(dllexport) const char *helper_name(void) { return "helper.dll"; }

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID reserved) {
    (void)inst; (void)reserved;
    if (reason == DLL_PROCESS_ATTACH) g_process_attach++;
    if (reason == DLL_THREAD_ATTACH) g_thread_attach++;
    return TRUE;
}
