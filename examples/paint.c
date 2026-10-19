/* Freehand drawing with GDI: a back buffer in a memory DC, mouse drawing, keyboard colours,
 * a timer-driven bouncing ball, and text.
 *
 *   x86_64-w64-mingw32-gcc -O2 -mwindows -o paint.exe paint.c
 *   nle paint.exe        # drag to draw, 1-4 change colour, C clears, Esc quits
 */
#include <windows.h>
#include <stdio.h>

static HDC back;
static HBITMAP back_bmp;
static int width = 640, height = 400;
static COLORREF colour = RGB(0, 120, 215);
static BOOL drawing;
static int last_x, last_y, ball_x = 60, ball_y = 60, dx = 5, dy = 4, strokes;

static void new_buffer(HWND win) {
    HDC dc = GetDC(win);
    HDC old = back;
    HBITMAP old_bmp = back_bmp;
    back = CreateCompatibleDC(dc);
    back_bmp = CreateCompatibleBitmap(dc, width, height);
    SelectObject(back, back_bmp);
    RECT r = {0, 0, width, height};
    FillRect(back, &r, (HBRUSH)GetStockObject(WHITE_BRUSH));
    if (old) {
        BitBlt(back, 0, 0, width, height, old, 0, 0, SRCCOPY);
        DeleteDC(old);
        DeleteObject(old_bmp);
    }
    ReleaseDC(win, dc);
}

static LRESULT CALLBACK proc(HWND win, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_CREATE:
        new_buffer(win);
        SetTimer(win, 1, 30, NULL);
        return 0;
    case WM_SIZE: {
        int w = LOWORD(lp), h = HIWORD(lp);
        if (w > 0 && h > 0 && (w != width || h != height)) {
            width = w;
            height = h;
            new_buffer(win);
        }
        return 0;
    }
    case WM_LBUTTONDOWN:
        SetCapture(win);
        drawing = TRUE;
        last_x = (short)LOWORD(lp);
        last_y = (short)HIWORD(lp);
        strokes++;
        return 0;
    case WM_MOUSEMOVE:
        if (drawing) {
            int x = (short)LOWORD(lp), y = (short)HIWORD(lp);
            HPEN pen = CreatePen(PS_SOLID, 4, colour);
            HPEN old = (HPEN)SelectObject(back, pen);
            MoveToEx(back, last_x, last_y, NULL);
            LineTo(back, x, y);
            SelectObject(back, old);
            DeleteObject(pen);
            last_x = x;
            last_y = y;
            InvalidateRect(win, NULL, FALSE);
        }
        return 0;
    case WM_LBUTTONUP:
        drawing = FALSE;
        ReleaseCapture();
        return 0;
    case WM_KEYDOWN:
        switch (wp) {
        case '1': colour = RGB(0, 120, 215); break;
        case '2': colour = RGB(232, 17, 35); break;
        case '3': colour = RGB(16, 124, 16); break;
        case '4': colour = RGB(255, 140, 0); break;
        case 'C': {
            RECT r = {0, 0, width, height};
            FillRect(back, &r, (HBRUSH)GetStockObject(WHITE_BRUSH));
            strokes = 0;
            InvalidateRect(win, NULL, FALSE);
            break;
        }
        case VK_ESCAPE: PostMessageA(win, WM_CLOSE, 0, 0); break;
        }
        return 0;
    case WM_TIMER:
        ball_x += dx;
        ball_y += dy;
        if (ball_x < 10 || ball_x > width - 10) dx = -dx;
        if (ball_y < 40 || ball_y > height - 10) dy = -dy;
        InvalidateRect(win, NULL, FALSE);
        return 0;
    case WM_ERASEBKGND:
        return 1;
    case WM_PAINT: {
        PAINTSTRUCT ps;
        HDC dc = BeginPaint(win, &ps);
        BitBlt(dc, 0, 0, width, height, back, 0, 0, SRCCOPY);
        HBRUSH ball = CreateSolidBrush(colour);
        HBRUSH old = (HBRUSH)SelectObject(dc, ball);
        Ellipse(dc, ball_x - 10, ball_y - 10, ball_x + 10, ball_y + 10);
        SelectObject(dc, old);
        DeleteObject(ball);
        char info[96];
        snprintf(info, sizeof info, "Drag to draw   1-4: colour   C: clear   strokes: %d", strokes);
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, RGB(60, 60, 60));
        RECT bar = {8, 6, width - 8, 30};
        DrawTextA(dc, info, -1, &bar, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        EndPaint(win, &ps);
        return 0;
    }
    case WM_DESTROY:
        KillTimer(win, 1);
        PostQuitMessage(strokes);
        return 0;
    }
    return DefWindowProcA(win, msg, wp, lp);
}

int WINAPI WinMain(HINSTANCE inst, HINSTANCE prev, LPSTR cmd, int show) {
    WNDCLASSA wc = {0};
    wc.style = CS_HREDRAW | CS_VREDRAW;
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursorA(NULL, IDC_CROSS);
    wc.lpszClassName = "NlePaint";
    RegisterClassA(&wc);
    RECT r = {0, 0, width, height};
    AdjustWindowRect(&r, WS_OVERLAPPEDWINDOW, FALSE);
    HWND win = CreateWindowExA(0, "NlePaint", "NLE paint", WS_OVERLAPPEDWINDOW, 100, 100, r.right - r.left, r.bottom - r.top, NULL, NULL, inst, NULL);
    ShowWindow(win, show);
    MSG m;
    while (GetMessageA(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessageA(&m);
    }
    return (int)m.wParam;
}
