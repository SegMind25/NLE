/* A small form: label, text box, check box, radio buttons and push buttons, with a timer.
 *
 *   x86_64-w64-mingw32-gcc -O2 -mwindows -o controls.exe controls.c
 *   nle controls.exe
 */
#include <windows.h>
#include <stdio.h>

#define ID_NAME 101
#define ID_GREET 102
#define ID_CLEAR 103
#define ID_SHOUT 104
#define ID_RED 105
#define ID_GREEN 106
#define ID_BLUE 107
#define ID_OUTPUT 108
#define ID_TICKS 109
#define ID_TIMER 1

static HWND name_box, output, ticks;
static int tick_count;

static HFONT ui_font(void) { return (HFONT)GetStockObject(DEFAULT_GUI_FONT); }

static HWND child(const char *cls, const char *text, DWORD style, int x, int y, int w, int h, HWND parent, int id, DWORD ex) {
    HWND c = CreateWindowExA(ex, cls, text, WS_CHILD | WS_VISIBLE | style, x, y, w, h, parent, (HMENU)(INT_PTR)id, GetModuleHandleA(NULL), NULL);
    SendMessageA(c, WM_SETFONT, (WPARAM)ui_font(), TRUE);
    return c;
}

static void greet(HWND win) {
    char name[64], text[160];
    GetWindowTextA(name_box, name, sizeof name);
    if (!name[0]) strcpy(name, "stranger");
    const char *colour = IsDlgButtonChecked(win, ID_RED) ? "red" : IsDlgButtonChecked(win, ID_GREEN) ? "green" : "blue";
    snprintf(text, sizeof text, IsDlgButtonChecked(win, ID_SHOUT) ? "HELLO, %s! (%s)" : "Hello, %s! (%s)", name, colour);
    if (IsDlgButtonChecked(win, ID_SHOUT)) CharUpperA(text);
    SetWindowTextA(output, text);
}

static LRESULT CALLBACK proc(HWND win, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_CREATE: {
        child("STATIC", "Your name:", 0, 16, 18, 80, 20, win, 0, 0);
        name_box = child("EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL, 100, 14, 190, 24, win, ID_NAME, WS_EX_CLIENTEDGE);
        child("BUTTON", "&Greet", WS_TABSTOP | BS_DEFPUSHBUTTON, 300, 13, 80, 26, win, ID_GREET, 0);
        child("BUTTON", "Shout", WS_TABSTOP | BS_AUTOCHECKBOX, 100, 52, 80, 20, win, ID_SHOUT, 0);
        child("BUTTON", "Red", WS_GROUP | WS_TABSTOP | BS_AUTORADIOBUTTON, 100, 80, 60, 20, win, ID_RED, 0);
        child("BUTTON", "Green", BS_AUTORADIOBUTTON, 170, 80, 60, 20, win, ID_GREEN, 0);
        child("BUTTON", "Blue", BS_AUTORADIOBUTTON, 240, 80, 60, 20, win, ID_BLUE, 0);
        CheckDlgButton(win, ID_BLUE, BST_CHECKED);
        output = child("STATIC", "Type a name and press Greet.", SS_LEFT, 16, 116, 364, 24, win, ID_OUTPUT, WS_EX_STATICEDGE);
        child("BUTTON", "Clear", WS_TABSTOP | BS_PUSHBUTTON, 300, 156, 80, 26, win, ID_CLEAR, 0);
        ticks = child("STATIC", "Seconds open: 0", 0, 16, 160, 200, 20, win, ID_TICKS, 0);
        SetTimer(win, ID_TIMER, 1000, NULL);
        return 0;
    }
    case WM_COMMAND:
        switch (LOWORD(wp)) {
        case ID_GREET: greet(win); break;
        case ID_CLEAR:
            SetWindowTextA(name_box, "");
            SetWindowTextA(output, "Cleared.");
            SetFocus(name_box);
            break;
        }
        return 0;
    case WM_TIMER: {
        char t[40];
        snprintf(t, sizeof t, "Seconds open: %d", ++tick_count);
        SetWindowTextA(ticks, t);
        return 0;
    }
    case WM_DESTROY:
        KillTimer(win, ID_TIMER);
        PostQuitMessage(tick_count);
        return 0;
    }
    return DefWindowProcA(win, msg, wp, lp);
}

int WINAPI WinMain(HINSTANCE inst, HINSTANCE prev, LPSTR cmd, int show) {
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursorA(NULL, IDC_ARROW);
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE + 1);
    wc.lpszClassName = "NleControls";
    RegisterClassA(&wc);
    HWND win = CreateWindowExA(0, "NleControls", "NLE controls", WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX, 120, 120, 412 + 16, 232 + 39, NULL, NULL, inst, NULL);
    ShowWindow(win, show);
    UpdateWindow(win);
    MSG m;
    while (GetMessageA(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessageA(&m);
    }
    return (int)m.wParam;
}
