/* A window that draws with OpenGL. NLE does not implement OpenGL, so this one shows what an unsupported
 * program looks like:
 *
 *   nle inspect opengl.exe    # verdict: partial, with the missing functions listed
 *   nle opengl.exe            # stops with a clear message instead of misbehaving
 */
#include <windows.h>
#include <GL/gl.h>

static LRESULT CALLBACK proc(HWND h, UINT m, WPARAM w, LPARAM l) {
    if (m == WM_DESTROY) {
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcA(h, m, w, l);
}

int WINAPI WinMain(HINSTANCE inst, HINSTANCE prev, LPSTR cmd, int show) {
    (void)prev;
    (void)cmd;
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.lpszClassName = "NleOpenGL";
    RegisterClassA(&wc);
    HWND h = CreateWindowA("NleOpenGL", "NLE OpenGL", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 100, 100, 320, 240, NULL, NULL, inst, NULL);
    HDC dc = GetDC(h);
    PIXELFORMATDESCRIPTOR pfd = {sizeof pfd, 1, PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER, PFD_TYPE_RGBA, 32};
    SetPixelFormat(dc, ChoosePixelFormat(dc, &pfd), &pfd);
    HGLRC rc = wglCreateContext(dc);
    wglMakeCurrent(dc, rc);
    glClearColor(0.1f, 0.4f, 0.8f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);
    SwapBuffers(dc);
    ShowWindow(h, show);
    MSG m;
    while (GetMessage(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessage(&m);
    }
    return 0;
}
