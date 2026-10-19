/* A classic Win32 window: register a class, create the window, run the message loop, draw text
 * when asked to paint.
 *
 *   x86_64-w64-mingw32-gcc -O2 -o window.exe window.c
 *   nle window.exe              # needs an X11 display; close the window to quit
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
