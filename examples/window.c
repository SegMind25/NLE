/* A classic Win32 window. NLE does not implement graphics (user32 is partly stubbed, gdi32 is
 * missing), so this one shows what an unsupported program looks like:
 *
 *   nle inspect window.exe      # verdict: partial, with the missing functions listed
 *   nle window.exe              # stops with a clear message instead of misbehaving
 */
#include <windows.h>

static LRESULT CALLBACK proc(HWND h, UINT msg, WPARAM w, LPARAM l) {
    switch (msg) {
    case WM_PAINT: {
        PAINTSTRUCT ps;
        HDC dc = BeginPaint(h, &ps);
        TextOutA(dc, 20, 20, "Hello from a Win32 window", 25);
        EndPaint(h, &ps);
        return 0;
    }
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcA(h, msg, w, l);
}

int main(void) {
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = GetModuleHandleA(NULL);
    wc.lpszClassName = "NleDemo";
    RegisterClassA(&wc);
    HWND win = CreateWindowExA(0, "NleDemo", "NLE demo", WS_OVERLAPPEDWINDOW, 100, 100, 480, 240, NULL, NULL, wc.hInstance, NULL);
    ShowWindow(win, SW_SHOW);
    MSG m;
    while (GetMessageA(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessageA(&m);
    }
    return 0;
}
