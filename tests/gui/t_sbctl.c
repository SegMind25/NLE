/* SCROLLBAR controls: a vertical and a horizontal bar as child windows. Prints what the parent hears. */
#include <windows.h>
#include <stdio.h>

static HWND vbar, hbar;

static LRESULT CALLBACK proc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
    if (msg == WM_VSCROLL || msg == WM_HSCROLL) {
        HWND bar = (HWND)lp;
        int pos = GetScrollPos(bar, SB_CTL);
        switch (LOWORD(wp)) {
        case SB_LINEUP: pos--; break;
        case SB_LINEDOWN: pos++; break;
        case SB_PAGEUP: pos -= 10; break;
        case SB_PAGEDOWN: pos += 10; break;
        case SB_THUMBTRACK: pos = HIWORD(wp); break;
        }
        printf("%c %u %d\n", msg == WM_VSCROLL ? 'V' : 'H', (unsigned)LOWORD(wp), pos);
        fflush(stdout);
        if (LOWORD(wp) != SB_ENDSCROLL) SetScrollPos(bar, SB_CTL, pos, TRUE);
        return 0;
    }
    if (msg == WM_DESTROY) {
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcA(h, msg, wp, lp);
}

int main(void) {
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = GetModuleHandleA(NULL);
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE + 1);
    wc.lpszClassName = "SbCtl";
    RegisterClassA(&wc);
    HWND h = CreateWindowA("SbCtl", "sb", WS_POPUP | WS_VISIBLE, 100, 100, 200, 170, NULL, NULL, wc.hInstance, NULL);
    vbar = CreateWindowA("SCROLLBAR", "", WS_CHILD | WS_VISIBLE | SBS_VERT, 170, 10, 17, 120, h, (HMENU)1, wc.hInstance, NULL);
    hbar = CreateWindowA("SCROLLBAR", "", WS_CHILD | WS_VISIBLE | SBS_HORZ, 10, 140, 150, 17, h, (HMENU)2, wc.hInstance, NULL);
    SCROLLINFO si = {sizeof si, SIF_RANGE | SIF_PAGE | SIF_POS, 0, 109, 10, 0, 0};
    SetScrollInfo(vbar, SB_CTL, &si, TRUE);
    SendMessageA(hbar, SBM_SETRANGE, 0, 100);
    int lo = -1, hi = -1;
    GetScrollRange(hbar, SB_CTL, &lo, &hi);
    printf("range %d %d pos %d\n", lo, hi, (int)SendMessageA(hbar, SBM_GETPOS, 0, 0));
    fflush(stdout);
    ShowWindow(h, SW_SHOW);
    MSG m;
    while (GetMessage(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessage(&m);
    }
    return 0;
}
