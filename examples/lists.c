/* List boxes and combo boxes: a sorted single-selection list, a multiple-selection list, a drop-down list
 * and an editable drop-down. Selections are shown in the label; start it with --log to print them
 * ("LB 12 Apple", "CB 104 select 2 Cherry", "CB 105 text hello", ...). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

#define ID_LIST 101
#define ID_MULTI 102
#define ID_STATIC 103
#define ID_DROP 104
#define ID_EDITABLE 105

static HWND label;
static int log_events = 0;
static const char *fruit[] = {"Cherry", "Apple", "Elderberry", "Banana", "Date", "Fig", "Grape", "Honeydew", "Kiwi", "Lemon", "Mango", "Nectarine"};

static void say(const char *fmt, ...) {
    char buf[200];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    SetWindowTextA(label, buf);
    if (log_events) { printf("%s\n", buf); fflush(stdout); }
}

static LRESULT CALLBACK proc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_COMMAND: {
        int id = LOWORD(wp), code = HIWORD(wp);
        char text[100];
        if (id == ID_LIST && code == LBN_SELCHANGE) {
            int i = (int)SendMessageA((HWND)lp, LB_GETCURSEL, 0, 0);
            SendMessageA((HWND)lp, LB_GETTEXT, i, (LPARAM)text);
            say("LB %d %s", i, text);
        } else if (id == ID_LIST && code == LBN_DBLCLK) {
            say("LB dblclk");
        } else if (id == ID_MULTI && code == LBN_SELCHANGE) {
            int n = (int)SendMessageA((HWND)lp, LB_GETSELCOUNT, 0, 0);
            int items[20];
            SendMessageA((HWND)lp, LB_GETSELITEMS, 20, (LPARAM)items);
            char list[100] = "";
            for (int k = 0; k < n && k < 8; k++) sprintf(list + strlen(list), "%d ", items[k]);
            say("MULTI %d: %s", n, list);
        } else if (id == ID_DROP && code == CBN_SELCHANGE) {
            int i = (int)SendMessageA((HWND)lp, CB_GETCURSEL, 0, 0);
            SendMessageA((HWND)lp, CB_GETLBTEXT, i, (LPARAM)text);
            say("CB %d select %d %s", id, i, text);
        } else if (id == ID_DROP && (code == CBN_DROPDOWN || code == CBN_CLOSEUP)) {
            say("CB %d %s", id, code == CBN_DROPDOWN ? "open" : "closed");
        } else if (id == ID_EDITABLE && code == CBN_SELCHANGE) {
            GetWindowTextA((HWND)lp, text, sizeof text);
            int i = (int)SendMessageA((HWND)lp, CB_GETCURSEL, 0, 0);
            say("CB %d select %d %s", id, i, text);
        } else if (id == ID_EDITABLE && code == CBN_EDITCHANGE) {
            GetWindowTextA((HWND)lp, text, sizeof text);
            say("CB %d text %s", id, text);
        }
        return 0;
    }
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcA(h, msg, wp, lp);
}

static HWND child(const char *cls, DWORD style, DWORD ex, int x, int y, int w, int h, HWND parent, int id) {
    HWND c = CreateWindowExA(ex, cls, "", WS_CHILD | WS_VISIBLE | style, x, y, w, h, parent, (HMENU)(INT_PTR)id, GetModuleHandleA(NULL), NULL);
    SendMessageA(c, WM_SETFONT, (WPARAM)GetStockObject(DEFAULT_GUI_FONT), TRUE);
    return c;
}

int WINAPI WinMain(HINSTANCE inst, HINSTANCE prev, LPSTR cmd, int show) {
    (void)prev;
    log_events = strstr(cmd, "--log") != NULL;
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursor(NULL, IDC_ARROW);
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE + 1);
    wc.lpszClassName = "NleLists";
    RegisterClassA(&wc);
    DWORD style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    RECT r = {0, 0, 380, 250};
    AdjustWindowRect(&r, style, FALSE);
    HWND h = CreateWindowA("NleLists", "NLE lists", style | WS_VISIBLE, 100, 100, r.right - r.left, r.bottom - r.top, NULL, NULL, inst, NULL);

    HWND list = child("LISTBOX", WS_BORDER | WS_VSCROLL | WS_TABSTOP | LBS_NOTIFY | LBS_SORT, 0, 10, 10, 150, 110, h, ID_LIST);
    for (int i = 0; i < 12; i++) SendMessageA(list, LB_ADDSTRING, 0, (LPARAM)fruit[i]);
    HWND multi = child("LISTBOX", WS_BORDER | WS_VSCROLL | WS_TABSTOP | LBS_NOTIFY | LBS_EXTENDEDSEL, WS_EX_CLIENTEDGE, 10, 130, 150, 80, h, ID_MULTI);
    for (int i = 0; i < 12; i++) { char t[32]; sprintf(t, "Row %d", i); SendMessageA(multi, LB_ADDSTRING, 0, (LPARAM)t); }
    HWND drop = child("COMBOBOX", WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST, 0, 180, 10, 190, 180, h, ID_DROP);
    for (int i = 0; i < 12; i++) SendMessageA(drop, CB_ADDSTRING, 0, (LPARAM)fruit[i]);
    HWND edit = child("COMBOBOX", WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWN, 0, 180, 50, 190, 180, h, ID_EDITABLE);
    for (int i = 0; i < 4; i++) SendMessageA(edit, CB_ADDSTRING, 0, (LPARAM)fruit[i]);
    label = child("STATIC", 0, 0, 180, 100, 190, 40, h, ID_STATIC);
    SetWindowTextA(label, "pick something");
    ShowWindow(h, show);
    MSG m;
    while (GetMessage(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessage(&m);
    }
    return 0;
}
