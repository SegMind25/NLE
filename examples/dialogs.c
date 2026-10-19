/* Dialog boxes from resources: a menu bar and accelerators loaded from the program's resources, a modal
 * Settings dialog (edit, check box, radio buttons, list box, combo box) and an About dialog with an icon.
 * Ctrl+S opens Settings; Tab, Enter, Esc and Alt+letter work as in any dialog. Start it with --log to print
 * the result of each dialog ("SETTINGS name=Ada shout=1 colour=Green list=Beta combo=Two", "CANCEL"). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

#define IDI_APP 1
#define IDM_SETTINGS 100
#define IDM_ABOUT 101
#define IDM_EXIT 102
#define IDD_SETTINGS 200
#define IDD_ABOUT 201
#define IDC_NAME 300
#define IDC_SHOUT 301
#define IDC_RED 302
#define IDC_GREEN 303
#define IDC_BLUE 304
#define IDC_LIST 305
#define IDC_COMBO 306

typedef struct { char name[64]; int shout; int colour; int list; int combo; } Settings;

static Settings settings = {"", 0, IDC_RED, 0, 0};
static int log_events = 0;
static char status[160] = "Press Ctrl+S";
static const char *colours[] = {"Red", "Green", "Blue"};
static const char *items[] = {"Alpha", "Beta", "Gamma", "Delta", "Epsilon"};
static const char *choices[] = {"One", "Two", "Three"};

static INT_PTR CALLBACK settings_proc(HWND dlg, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_INITDIALOG: {
        Settings *s = (Settings *)lp;
        SetDlgItemTextA(dlg, IDC_NAME, s->name);
        CheckDlgButton(dlg, IDC_SHOUT, s->shout ? BST_CHECKED : BST_UNCHECKED);
        CheckRadioButton(dlg, IDC_RED, IDC_BLUE, s->colour);
        for (int i = 0; i < 5; i++) SendDlgItemMessageA(dlg, IDC_LIST, LB_ADDSTRING, 0, (LPARAM)items[i]);
        SendDlgItemMessageA(dlg, IDC_LIST, LB_SETCURSEL, s->list, 0);
        for (int i = 0; i < 3; i++) SendDlgItemMessageA(dlg, IDC_COMBO, CB_ADDSTRING, 0, (LPARAM)choices[i]);
        SendDlgItemMessageA(dlg, IDC_COMBO, CB_SETCURSEL, s->combo, 0);
        SetWindowLongPtrA(dlg, DWLP_USER, (LONG_PTR)s);
        return TRUE; /* the system puts the focus on the first control */
    }
    case WM_COMMAND:
        switch (LOWORD(wp)) {
        case IDOK: {
            Settings *s = (Settings *)GetWindowLongPtrA(dlg, DWLP_USER);
            GetDlgItemTextA(dlg, IDC_NAME, s->name, sizeof s->name);
            s->shout = IsDlgButtonChecked(dlg, IDC_SHOUT) == BST_CHECKED;
            s->colour = IsDlgButtonChecked(dlg, IDC_RED) ? IDC_RED : IsDlgButtonChecked(dlg, IDC_GREEN) ? IDC_GREEN : IDC_BLUE;
            s->list = (int)SendDlgItemMessageA(dlg, IDC_LIST, LB_GETCURSEL, 0, 0);
            s->combo = (int)SendDlgItemMessageA(dlg, IDC_COMBO, CB_GETCURSEL, 0, 0);
            EndDialog(dlg, IDOK);
            return TRUE;
        }
        case IDCANCEL:
            EndDialog(dlg, IDCANCEL);
            return TRUE;
        }
        break;
    }
    return FALSE;
}

static INT_PTR CALLBACK about_proc(HWND dlg, UINT msg, WPARAM wp, LPARAM lp) {
    (void)lp;
    if (msg == WM_INITDIALOG) return TRUE;
    if (msg == WM_COMMAND && (LOWORD(wp) == IDOK || LOWORD(wp) == IDCANCEL)) {
        EndDialog(dlg, LOWORD(wp));
        return TRUE;
    }
    return FALSE;
}

static LRESULT CALLBACK proc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_COMMAND:
        if (LOWORD(wp) == IDM_SETTINGS) {
            Settings copy = settings;
            INT_PTR r = DialogBoxParamA(GetModuleHandleA(NULL), MAKEINTRESOURCEA(IDD_SETTINGS), h, settings_proc, (LPARAM)&copy);
            if (r == IDOK) {
                settings = copy;
                snprintf(status, sizeof status, "name=%s shout=%d colour=%s list=%s combo=%s", settings.name, settings.shout, colours[settings.colour - IDC_RED], settings.list >= 0 ? items[settings.list] : "-", settings.combo >= 0 ? choices[settings.combo] : "-");
                if (log_events) { printf("SETTINGS %s\n", status); fflush(stdout); }
            } else {
                if (log_events) { printf("CANCEL\n"); fflush(stdout); }
            }
            InvalidateRect(h, NULL, TRUE);
        } else if (LOWORD(wp) == IDM_ABOUT) {
            char title[64];
            LoadStringA(GetModuleHandleA(NULL), IDM_ABOUT, title, sizeof title);
            DialogBoxParamA(GetModuleHandleA(NULL), MAKEINTRESOURCEA(IDD_ABOUT), h, about_proc, 0);
            if (log_events) { printf("ABOUT closed (%s)\n", title); fflush(stdout); }
        } else if (LOWORD(wp) == IDM_EXIT) {
            DestroyWindow(h);
        }
        return 0;
    case WM_PAINT: {
        PAINTSTRUCT ps;
        HDC dc = BeginPaint(h, &ps);
        SetBkMode(dc, TRANSPARENT);
        DrawIconEx(dc, 12, 12, LoadIconA(GetModuleHandleA(NULL), MAKEINTRESOURCEA(IDI_APP)), 0, 0, 0, NULL, DI_NORMAL);
        TextOutA(dc, 56, 14, status, (int)strlen(status));
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
    wc.lpszClassName = "NleDialogs";
    wc.lpszMenuName = "MAINMENU";
    wc.hIcon = LoadIconA(inst, MAKEINTRESOURCEA(IDI_APP));
    RegisterClassA(&wc);
    HACCEL accel = LoadAcceleratorsA(inst, "MAINACCEL");
    DWORD style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    RECT r = {0, 0, 420, 80};
    AdjustWindowRect(&r, style, TRUE);
    HWND h = CreateWindowA("NleDialogs", "NLE dialogs", style | WS_VISIBLE, 100, 100, r.right - r.left, r.bottom - r.top, NULL, NULL, inst, NULL);
    ShowWindow(h, show);
    MSG m;
    while (GetMessage(&m, NULL, 0, 0) > 0) {
        if (!TranslateAccelerator(h, accel, &m)) {
            TranslateMessage(&m);
            DispatchMessage(&m);
        }
    }
    return 0;
}
