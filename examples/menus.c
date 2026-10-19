/* A window with a menu bar (File, Edit, View with a Zoom submenu, Help), keyboard accelerators and a
 * context menu on the right mouse button. The menus are built with CreateMenu / AppendMenu; checking and
 * graying items works as usual. Start it with --log to print every command ("CMD 101 New", ...). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

enum { ID_NEW = 101, ID_OPEN, ID_EXIT, ID_UNDO, ID_CUT, ID_COPY, ID_PASTE, ID_WRAP, ID_Z100, ID_Z150, ID_Z200, ID_ABOUT, ID_REFRESH };

static HMENU bar, view_menu;
static HACCEL accel;
static int log_events = 0, wrap = 0, zoom = ID_Z100;
static char status[128] = "no command yet";

static const char *name_of(int id) {
    switch (id) {
    case ID_NEW: return "New";
    case ID_OPEN: return "Open";
    case ID_EXIT: return "Exit";
    case ID_UNDO: return "Undo";
    case ID_CUT: return "Cut";
    case ID_COPY: return "Copy";
    case ID_PASTE: return "Paste";
    case ID_WRAP: return "Word wrap";
    case ID_Z100: return "100%";
    case ID_Z150: return "150%";
    case ID_Z200: return "200%";
    case ID_ABOUT: return "About";
    case ID_REFRESH: return "Refresh";
    }
    return "?";
}

static void say(const char *what, int id) {
    snprintf(status, sizeof status, "%s: %s", what, name_of(id));
    if (log_events) { printf("%s %d %s\n", what, id, name_of(id)); fflush(stdout); }
}

static HMENU build_menus(void) {
    HMENU file = CreatePopupMenu(), edit = CreatePopupMenu(), zoom_menu = CreatePopupMenu(), help = CreatePopupMenu();
    AppendMenuA(file, MF_STRING, ID_NEW, "&New\tCtrl+N");
    AppendMenuA(file, MF_STRING, ID_OPEN, "&Open...");
    AppendMenuA(file, MF_SEPARATOR, 0, NULL);
    AppendMenuA(file, MF_STRING, ID_EXIT, "E&xit");
    AppendMenuA(edit, MF_STRING, ID_UNDO, "&Undo");
    AppendMenuA(edit, MF_SEPARATOR, 0, NULL);
    AppendMenuA(edit, MF_STRING | MF_GRAYED, ID_CUT, "Cu&t");
    AppendMenuA(edit, MF_STRING, ID_COPY, "&Copy");
    AppendMenuA(edit, MF_STRING, ID_PASTE, "&Paste");
    AppendMenuA(zoom_menu, MF_STRING, ID_Z100, "100%");
    AppendMenuA(zoom_menu, MF_STRING, ID_Z150, "150%");
    AppendMenuA(zoom_menu, MF_STRING, ID_Z200, "200%");
    CheckMenuRadioItem(zoom_menu, ID_Z100, ID_Z200, ID_Z100, MF_BYCOMMAND);
    view_menu = CreatePopupMenu();
    AppendMenuA(view_menu, MF_STRING, ID_WRAP, "&Word wrap");
    AppendMenuA(view_menu, MF_POPUP, (UINT_PTR)zoom_menu, "&Zoom");
    AppendMenuA(help, MF_STRING, ID_ABOUT, "&About");
    HMENU m = CreateMenu();
    AppendMenuA(m, MF_POPUP, (UINT_PTR)file, "&File");
    AppendMenuA(m, MF_POPUP, (UINT_PTR)edit, "&Edit");
    AppendMenuA(m, MF_POPUP, (UINT_PTR)view_menu, "&View");
    AppendMenuA(m, MF_POPUP, (UINT_PTR)help, "&Help");
    return m;
}

static LRESULT CALLBACK proc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_COMMAND: {
        int id = LOWORD(wp);
        if (id == ID_WRAP) {
            wrap = !wrap;
            CheckMenuItem(view_menu, ID_WRAP, MF_BYCOMMAND | (wrap ? MF_CHECKED : MF_UNCHECKED));
        } else if (id >= ID_Z100 && id <= ID_Z200) {
            zoom = id;
            CheckMenuRadioItem(GetSubMenu(view_menu, 1), ID_Z100, ID_Z200, id, MF_BYCOMMAND);
        } else if (id == ID_EXIT) {
            DestroyWindow(h);
            return 0;
        }
        say(HIWORD(wp) == 1 ? "ACCEL" : "CMD", id);
        InvalidateRect(h, NULL, TRUE);
        return 0;
    }
    case WM_INITMENUPOPUP:
        if (log_events) { printf("INIT\n"); fflush(stdout); }
        return 0;
    case WM_RBUTTONUP: {
        HMENU pop = CreatePopupMenu();
        AppendMenuA(pop, MF_STRING, ID_COPY, "&Copy");
        AppendMenuA(pop, MF_STRING, ID_PASTE, "&Paste");
        POINT pt = {(short)LOWORD(lp), (short)HIWORD(lp)};
        ClientToScreen(h, &pt);
        int cmd = TrackPopupMenu(pop, TPM_RETURNCMD | TPM_LEFTALIGN, pt.x, pt.y, 0, h, NULL);
        DestroyMenu(pop);
        say("POPUP", cmd);
        InvalidateRect(h, NULL, TRUE);
        return 0;
    }
    case WM_PAINT: {
        PAINTSTRUCT ps;
        HDC dc = BeginPaint(h, &ps);
        SetBkMode(dc, TRANSPARENT);
        TextOutA(dc, 16, 16, status, (int)strlen(status));
        char z[64];
        snprintf(z, sizeof z, "word wrap: %s   zoom: %s", wrap ? "on" : "off", name_of(zoom));
        TextOutA(dc, 16, 40, z, (int)strlen(z));
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
    wc.lpszClassName = "NleMenus";
    RegisterClassA(&wc);
    bar = build_menus();
    ACCEL keys[] = {{FVIRTKEY | FCONTROL, 'N', ID_NEW}, {FVIRTKEY, VK_F5, ID_REFRESH}};
    accel = CreateAcceleratorTableA(keys, 2);
    DWORD style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    RECT r = {0, 0, 360, 120};
    AdjustWindowRect(&r, style, TRUE);
    HWND h = CreateWindowA("NleMenus", "NLE menus", style | WS_VISIBLE, 100, 100, r.right - r.left, r.bottom - r.top, NULL, bar, inst, NULL);
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
