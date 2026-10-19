/* A text viewer with both scroll bars. The bars belong to the window itself (WS_VSCROLL / WS_HSCROLL);
 * the program keeps the range and position with SetScrollInfo and scrolls when it gets WM_VSCROLL
 * and WM_HSCROLL. Keys, the wheel and the bars all work. Start it with --log to print every
 * scroll message ("V <code> <top line>", "H <code> <left column>"). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

#define LINES 100
#define COLS 90
#define LINE_H 16
#define CHAR_W 8

static int top = 0, left = 0, log_events = 0;
static const char *text = "the quick brown fox jumps over the lazy dog 0123456789 abcdefghijklmnopqrstuvwxyz";

static int clampi(int v, int lo, int hi) { return v < lo ? lo : v > hi ? hi : v; }

static void update_bars(HWND h) {
    RECT rc;
    GetClientRect(h, &rc);
    SCROLLINFO si = {sizeof si, SIF_RANGE | SIF_PAGE | SIF_POS};
    si.nMin = 0;
    si.nMax = LINES - 1;
    si.nPage = rc.bottom / LINE_H;
    si.nPos = top;
    SetScrollInfo(h, SB_VERT, &si, TRUE);
    si.nMax = COLS - 1;
    si.nPage = rc.right / CHAR_W;
    si.nPos = left;
    SetScrollInfo(h, SB_HORZ, &si, TRUE);
    GetScrollInfo(h, SB_VERT, &si);
    top = si.nPos;
}

static int scroll_pos(HWND h, int bar, WPARAM wp) {
    SCROLLINFO si = {sizeof si, SIF_ALL};
    GetScrollInfo(h, bar, &si);
    int pos = si.nPos;
    switch (LOWORD(wp)) {
    case SB_LINEUP: pos--; break;
    case SB_LINEDOWN: pos++; break;
    case SB_PAGEUP: pos -= si.nPage; break;
    case SB_PAGEDOWN: pos += si.nPage; break;
    case SB_THUMBTRACK: pos = si.nTrackPos; break;
    case SB_TOP: pos = si.nMin; break;
    case SB_BOTTOM: pos = si.nMax; break;
    }
    return clampi(pos, si.nMin, si.nMax);
}

static LRESULT CALLBACK proc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_SIZE:
        update_bars(h);
        return 0;
    case WM_VSCROLL:
        if (LOWORD(wp) != SB_ENDSCROLL) {
            top = scroll_pos(h, SB_VERT, wp);
            update_bars(h);
            InvalidateRect(h, NULL, TRUE);
        }
        if (log_events) { printf("V %u %d\n", (unsigned)LOWORD(wp), top); fflush(stdout); }
        return 0;
    case WM_HSCROLL:
        if (LOWORD(wp) != SB_ENDSCROLL) {
            left = scroll_pos(h, SB_HORZ, wp);
            update_bars(h);
            InvalidateRect(h, NULL, TRUE);
        }
        if (log_events) { printf("H %u %d\n", (unsigned)LOWORD(wp), left); fflush(stdout); }
        return 0;
    case WM_MOUSEWHEEL:
        top = clampi(top - (short)HIWORD(wp) / WHEEL_DELTA * 3, 0, LINES - 1);
        update_bars(h);
        InvalidateRect(h, NULL, TRUE);
        return 0;
    case WM_KEYDOWN:
        if (wp == VK_DOWN) SendMessage(h, WM_VSCROLL, SB_LINEDOWN, 0);
        else if (wp == VK_UP) SendMessage(h, WM_VSCROLL, SB_LINEUP, 0);
        else if (wp == VK_NEXT) SendMessage(h, WM_VSCROLL, SB_PAGEDOWN, 0);
        else if (wp == VK_PRIOR) SendMessage(h, WM_VSCROLL, SB_PAGEUP, 0);
        else if (wp == VK_HOME) SendMessage(h, WM_VSCROLL, SB_TOP, 0);
        else if (wp == VK_END) SendMessage(h, WM_VSCROLL, SB_BOTTOM, 0);
        else if (wp == VK_RIGHT) SendMessage(h, WM_HSCROLL, SB_LINEDOWN, 0);
        else if (wp == VK_LEFT) SendMessage(h, WM_HSCROLL, SB_LINEUP, 0);
        return 0;
    case WM_PAINT: {
        PAINTSTRUCT ps;
        HDC dc = BeginPaint(h, &ps);
        RECT rc;
        GetClientRect(h, &rc);
        SetBkMode(dc, TRANSPARENT);
        for (int i = 0; i <= rc.bottom / LINE_H; i++) {
            int n = top + i;
            if (n >= LINES) break;
            char line[200];
            int len = snprintf(line, sizeof line, "%3d  %s", n + 1, text);
            TextOutA(dc, -left * CHAR_W, i * LINE_H, line, len);
        }
        EndPaint(h, &ps);
        return 0;
    }
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcA(h, msg, wp, lp);
}

int WINAPI WinMain(HINSTANCE inst, HINSTANCE prev, LPSTR cmd, int show) {
    (void)prev;
    log_events = strstr(cmd, "--log") != NULL;
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursor(NULL, IDC_ARROW);
    wc.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    wc.lpszClassName = "NleScroll";
    RegisterClassA(&wc);
    DWORD style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    RECT r = {0, 0, 320, 200};
    AdjustWindowRect(&r, style, FALSE); /* the bars share the 320 x 200 */
    HWND h = CreateWindowA("NleScroll", "NLE scroll bars", style | WS_VSCROLL | WS_HSCROLL | WS_VISIBLE, 100, 100, r.right - r.left, r.bottom - r.top, NULL, NULL, inst, NULL);
    ShowWindow(h, show);
    MSG m;
    while (GetMessage(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessage(&m);
    }
    return 0;
}
