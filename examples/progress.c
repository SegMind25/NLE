/* A window with a progress bar from the common controls library (comctl32), which NLE implements itself:
 *
 *   nle inspect progress.exe    # verdict: native
 *   nle progress.exe
 */
#include <windows.h>
#include <commctrl.h>

static LRESULT CALLBACK proc(HWND h, UINT m, WPARAM w, LPARAM l) {
    if (m == WM_DESTROY) {
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcA(h, m, w, l);
}

int main(void) {
    INITCOMMONCONTROLSEX icc = {sizeof icc, ICC_PROGRESS_CLASS};
    InitCommonControlsEx(&icc);
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = GetModuleHandleA(NULL);
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE + 1);
    wc.lpszClassName = "NleProgress";
    RegisterClassA(&wc);
    HWND win = CreateWindowExA(0, "NleProgress", "Progress", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 100, 100, 320, 120, NULL, NULL, wc.hInstance, NULL);
    CreateWindowExA(0, PROGRESS_CLASSA, NULL, WS_CHILD | WS_VISIBLE, 20, 20, 260, 22, win, NULL, wc.hInstance, NULL);
    MSG msg;
    while (GetMessageA(&msg, NULL, 0, 0) > 0) {
        TranslateMessage(&msg);
        DispatchMessageA(&msg);
    }
    return 0;
}
