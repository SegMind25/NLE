/* Common controls (comctl32): a tool bar, a status bar, a tab control whose pages hold a report-view list, a tree and a
 * group of progress bar / track bar / spin control. With --log every action is printed:
 * "TB 1001", "TAB 1", "LV select 2 notes.txt", "TV select Documents", "TRACK 40", "SPIN 7", ... */
#include <windows.h>
#include <commctrl.h>
#include <stdio.h>
#include <string.h>

#define ID_TOOLBAR 200
#define ID_STATUS 201
#define ID_TAB 202
#define ID_LIST 203
#define ID_TREE 204
#define ID_PROGRESS 205
#define ID_TRACK 206
#define ID_SPIN 207
#define ID_SPINEDIT 208

#define CMD_NEW 1001
#define CMD_OPEN 1002
#define CMD_SAVE 1003
#define CMD_CUT 1004
#define CMD_COPY 1005
#define CMD_PASTE 1006
#define CMD_BOLD 1007

static HWND toolbar, status, tab, list, tree, progress, track, spin, spin_edit, labels[3];
static int log_events = 0;

static void say(const char *fmt, ...) {
    char buf[200];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    SendMessageA(status, SB_SETTEXTA, 0, (LPARAM)buf);
    if (log_events) { printf("%s\n", buf); fflush(stdout); }
}

static void show_page(int page) {
    ShowWindow(list, page == 0 ? SW_SHOW : SW_HIDE);
    ShowWindow(tree, page == 1 ? SW_SHOW : SW_HIDE);
    for (int i = 0; i < 3; i++) ShowWindow(labels[i], page == 2 ? SW_SHOW : SW_HIDE);
    ShowWindow(progress, page == 2 ? SW_SHOW : SW_HIDE);
    ShowWindow(track, page == 2 ? SW_SHOW : SW_HIDE);
    ShowWindow(spin, page == 2 ? SW_SHOW : SW_HIDE);
    ShowWindow(spin_edit, page == 2 ? SW_SHOW : SW_HIDE);
}

static void layout(HWND h) {
    RECT rc;
    GetClientRect(h, &rc);
    SendMessageA(toolbar, TB_AUTOSIZE, 0, 0);
    SendMessageA(status, WM_SIZE, 0, 0);
    RECT tr, sr;
    GetWindowRect(toolbar, &tr);
    GetWindowRect(status, &sr);
    int top = tr.bottom - tr.top + 6, bottom = rc.bottom - (sr.bottom - sr.top) - 6;
    MoveWindow(tab, 6, top, rc.right - 12, bottom - top, TRUE);
    RECT page = {0, 0, rc.right - 12, bottom - top};
    SendMessageA(tab, TCM_ADJUSTRECT, FALSE, (LPARAM)&page);
    int x = 6 + page.left, y = top + page.top, w = page.right - page.left, hh = page.bottom - page.top;
    MoveWindow(list, x, y, w, hh, TRUE);
    MoveWindow(tree, x, y, w, hh, TRUE);
    MoveWindow(labels[0], x + 10, y + 12, 80, 18, TRUE);
    MoveWindow(progress, x + 100, y + 12, 220, 18, TRUE);
    MoveWindow(labels[1], x + 10, y + 52, 80, 18, TRUE);
    MoveWindow(track, x + 100, y + 44, 220, 32, TRUE);
    MoveWindow(labels[2], x + 10, y + 96, 80, 18, TRUE);
    MoveWindow(spin_edit, x + 100, y + 92, 60, 22, TRUE);
}

static void item_text(HWND tv, HTREEITEM item, char *buf, int n) {
    TVITEM it;
    memset(&it, 0, sizeof it);
    it.mask = TVIF_TEXT;
    it.hItem = item;
    it.pszText = buf;
    it.cchTextMax = n;
    buf[0] = 0;
    TreeView_GetItem(tv, &it);
}

static LRESULT CALLBACK proc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_SIZE:
        if (toolbar) layout(h);
        return 0;
    case WM_COMMAND:
        if (LOWORD(wp) >= CMD_NEW && LOWORD(wp) <= CMD_BOLD) {
            if (LOWORD(wp) == CMD_BOLD)
                say("TB %d %s", LOWORD(wp), SendMessageA(toolbar, TB_ISBUTTONCHECKED, CMD_BOLD, 0) ? "on" : "off");
            else
                say("TB %d", LOWORD(wp));
        }
        return 0;
    case WM_CTLCOLORSTATIC:
        /* the labels sit on a white tab page */
        SetBkColor((HDC)wp, RGB(255, 255, 255));
        return (LRESULT)GetStockObject(WHITE_BRUSH);
    case WM_HSCROLL:
        if ((HWND)lp == track) {
            int pos = (int)SendMessageA(track, TBM_GETPOS, 0, 0);
            SendMessageA(progress, PBM_SETPOS, pos, 0);
            say("TRACK %d", pos);
        }
        return 0;
    case WM_NOTIFY: {
        NMHDR *nm = (NMHDR *)lp;
        char text[100];
        if (nm->hwndFrom == tab && nm->code == TCN_SELCHANGE) {
            int sel = TabCtrl_GetCurSel(tab);
            show_page(sel);
            say("TAB %d", sel);
        } else if (nm->hwndFrom == list && nm->code == LVN_ITEMCHANGED) {
            NMLISTVIEW *n = (NMLISTVIEW *)lp;
            if ((n->uChanged & LVIF_STATE) && (n->uNewState & LVIS_SELECTED) && !(n->uOldState & LVIS_SELECTED)) {
                ListView_GetItemText(list, n->iItem, 0, text, sizeof text);
                say("LV select %d %s", n->iItem, text);
            }
        } else if (nm->hwndFrom == list && nm->code == LVN_COLUMNCLICK) {
            say("LV column %d", ((NMLISTVIEW *)lp)->iSubItem);
        } else if (nm->hwndFrom == list && nm->code == NM_DBLCLK) {
            say("LV open %d", ListView_GetNextItem(list, -1, LVNI_SELECTED));
        } else if (nm->hwndFrom == tree && nm->code == TVN_SELCHANGED) {
            item_text(tree, ((NMTREEVIEW *)lp)->itemNew.hItem, text, sizeof text);
            say("TV select %s", text);
        } else if (nm->hwndFrom == tree && nm->code == TVN_ITEMEXPANDED) {
            NMTREEVIEW *n = (NMTREEVIEW *)lp;
            item_text(tree, n->itemNew.hItem, text, sizeof text);
            say("TV %s %s", n->action == TVE_EXPAND ? "expand" : "collapse", text);
        } else if (nm->hwndFrom == spin && nm->code == UDN_DELTAPOS) {
            NMUPDOWN *u = (NMUPDOWN *)lp;
            say("SPIN %d", u->iPos + u->iDelta);
        }
        return 0;
    }
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcA(h, msg, wp, lp);
}

static HWND child(const char *cls, const char *text, DWORD style, DWORD ex, int id, HWND parent) {
    HWND c = CreateWindowExA(ex, cls, text, WS_CHILD | style, 0, 0, 10, 10, parent, (HMENU)(INT_PTR)id, GetModuleHandleA(NULL), NULL);
    SendMessageA(c, WM_SETFONT, (WPARAM)GetStockObject(DEFAULT_GUI_FONT), TRUE);
    return c;
}

static HTREEITEM add_node(HWND tv, HTREEITEM parent, const char *text, int kids, int image) {
    TVINSERTSTRUCT ins;
    memset(&ins, 0, sizeof ins);
    ins.hParent = parent;
    ins.hInsertAfter = TVI_LAST;
    ins.item.mask = TVIF_TEXT | TVIF_IMAGE | TVIF_SELECTEDIMAGE | (kids ? TVIF_CHILDREN : 0);
    ins.item.pszText = (char *)text;
    ins.item.cChildren = kids;
    ins.item.iImage = image;
    ins.item.iSelectedImage = image;
    return TreeView_InsertItem(tv, &ins);
}

/* a small image list of the built-in icons */
static HIMAGELIST icons(void) {
    HIMAGELIST il = ImageList_Create(16, 16, ILC_COLOR32 | ILC_MASK, 4, 4);
    LPCSTR ids[] = {IDI_APPLICATION, IDI_INFORMATION, IDI_WARNING, IDI_QUESTION};
    for (int i = 0; i < 4; i++) ImageList_AddIcon(il, LoadIcon(NULL, ids[i]));
    return il;
}

int WINAPI WinMain(HINSTANCE inst, HINSTANCE prev, LPSTR cmd, int show) {
    (void)prev;
    log_events = strstr(cmd, "--log") != NULL;
    INITCOMMONCONTROLSEX icc = {sizeof icc, ICC_WIN95_CLASSES | ICC_BAR_CLASSES | ICC_TAB_CLASSES | ICC_LISTVIEW_CLASSES | ICC_TREEVIEW_CLASSES | ICC_UPDOWN_CLASS};
    InitCommonControlsEx(&icc);
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursor(NULL, IDC_ARROW);
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE + 1);
    wc.lpszClassName = "NleCommon";
    RegisterClassA(&wc);
    DWORD style = WS_OVERLAPPEDWINDOW;
    RECT r = {0, 0, 520, 360};
    AdjustWindowRect(&r, style, FALSE);
    HWND h = CreateWindowA("NleCommon", "NLE common controls", style | WS_VISIBLE, 100, 100, r.right - r.left, r.bottom - r.top, NULL, NULL, inst, NULL);

    /* tool bar with the standard images */
    toolbar = child(TOOLBARCLASSNAME, "", WS_VISIBLE | TBSTYLE_FLAT | TBSTYLE_TOOLTIPS | CCS_TOP, 0, ID_TOOLBAR, h);
    SendMessageA(toolbar, TB_BUTTONSTRUCTSIZE, sizeof(TBBUTTON), 0);
    TBADDBITMAP ab = {HINST_COMMCTRL, IDB_STD_SMALL_COLOR};
    SendMessageA(toolbar, TB_ADDBITMAP, 0, (LPARAM)&ab);
    TBBUTTON buttons[] = {
        {STD_FILENEW, CMD_NEW, TBSTATE_ENABLED, BTNS_BUTTON, {0}, 0, 0},
        {STD_FILEOPEN, CMD_OPEN, TBSTATE_ENABLED, BTNS_BUTTON, {0}, 0, 0},
        {STD_FILESAVE, CMD_SAVE, TBSTATE_ENABLED, BTNS_BUTTON, {0}, 0, 0},
        {0, 0, TBSTATE_ENABLED, BTNS_SEP, {0}, 0, 0},
        {STD_CUT, CMD_CUT, TBSTATE_ENABLED, BTNS_BUTTON, {0}, 0, 0},
        {STD_COPY, CMD_COPY, TBSTATE_ENABLED, BTNS_BUTTON, {0}, 0, 0},
        {STD_PASTE, CMD_PASTE, 0, BTNS_BUTTON, {0}, 0, 0}, /* disabled */
        {0, 0, TBSTATE_ENABLED, BTNS_SEP, {0}, 0, 0},
        {STD_FIND, CMD_BOLD, TBSTATE_ENABLED, BTNS_CHECK, {0}, 0, 0},
    };
    SendMessageA(toolbar, TB_ADDBUTTONS, sizeof buttons / sizeof buttons[0], (LPARAM)buttons);

    /* status bar with three parts */
    status = child(STATUSCLASSNAME, "", WS_VISIBLE | SBARS_SIZEGRIP, 0, ID_STATUS, h);
    int parts[] = {330, 440, -1};
    SendMessageA(status, SB_SETPARTS, 3, (LPARAM)parts);
    SendMessageA(status, SB_SETTEXTA, 1, (LPARAM)"3 pages");
    SendMessageA(status, SB_SETTEXTA, 2, (LPARAM)"NLE");

    tab = child(WC_TABCONTROL, "", WS_VISIBLE | WS_CLIPSIBLINGS | WS_TABSTOP, 0, ID_TAB, h);
    const char *pages[] = {"Files", "Tree", "Controls"};
    for (int i = 0; i < 3; i++) {
        TCITEMA ti;
        memset(&ti, 0, sizeof ti);
        ti.mask = TCIF_TEXT;
        ti.pszText = (char *)pages[i];
        SendMessageA(tab, TCM_INSERTITEMA, i, (LPARAM)&ti);
    }

    /* page 0: a report-view list */
    list = child(WC_LISTVIEW, "", WS_VISIBLE | WS_TABSTOP | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS, WS_EX_CLIENTEDGE, ID_LIST, h);
    ListView_SetExtendedListViewStyle(list, LVS_EX_FULLROWSELECT | LVS_EX_GRIDLINES | LVS_EX_CHECKBOXES);
    const char *heads[] = {"Name", "Size", "Type"};
    int widths[] = {170, 80, 140};
    for (int i = 0; i < 3; i++) {
        LVCOLUMNA col;
        memset(&col, 0, sizeof col);
        col.mask = LVCF_TEXT | LVCF_WIDTH | LVCF_FMT;
        col.pszText = (char *)heads[i];
        col.cx = widths[i];
        col.fmt = i == 1 ? LVCFMT_RIGHT : LVCFMT_LEFT;
        SendMessageA(list, LVM_INSERTCOLUMNA, i, (LPARAM)&col);
    }
    SendMessageA(list, LVM_SETIMAGELIST, LVSIL_SMALL, (LPARAM)icons());
    const char *names[] = {"readme.txt", "notes.txt", "photo.png", "setup.exe", "archive.zip", "song.mp3", "report.doc", "data.csv", "icon.ico", "backup.tar"};
    const char *kinds[] = {"Text", "Text", "Image", "Application", "Archive", "Audio", "Document", "Spreadsheet", "Icon", "Archive"};
    for (int i = 0; i < 10; i++) {
        LVITEMA it;
        memset(&it, 0, sizeof it);
        it.mask = LVIF_TEXT | LVIF_IMAGE;
        it.iItem = i;
        it.iImage = i % 4;
        it.pszText = (char *)names[i];
        SendMessageA(list, LVM_INSERTITEMA, 0, (LPARAM)&it);
        char size[32];
        sprintf(size, "%d KB", 4 + i * 37 % 900);
        ListView_SetItemText(list, i, 1, size);
        ListView_SetItemText(list, i, 2, (char *)kinds[i]);
    }

    /* page 1: a tree */
    tree = child(WC_TREEVIEW, "", WS_TABSTOP | TVS_HASBUTTONS | TVS_HASLINES | TVS_LINESATROOT | TVS_SHOWSELALWAYS, WS_EX_CLIENTEDGE, ID_TREE, h);
    SendMessageA(tree, TVM_SETIMAGELIST, TVSIL_NORMAL, (LPARAM)icons());
    HTREEITEM root = add_node(tree, TVI_ROOT, "Computer", 1, 0);
    HTREEITEM docs = add_node(tree, root, "Documents", 1, 1);
    add_node(tree, docs, "Letters", 0, 1);
    add_node(tree, docs, "Reports", 0, 1);
    add_node(tree, root, "Pictures", 1, 1);
    add_node(tree, root, "Music", 0, 2);
    HTREEITEM net = add_node(tree, TVI_ROOT, "Network", 1, 3);
    add_node(tree, net, "Server", 0, 0);
    TreeView_Expand(tree, root, TVE_EXPAND);

    /* page 2: progress bar, track bar and a spin control with its edit box */
    labels[0] = child("STATIC", "Progress", SS_LEFT, 0, 300, h);
    labels[1] = child("STATIC", "Track", SS_LEFT, 0, 301, h);
    labels[2] = child("STATIC", "Spin", SS_LEFT, 0, 302, h);
    progress = child(PROGRESS_CLASS, "", 0, 0, ID_PROGRESS, h);
    SendMessageA(progress, PBM_SETRANGE32, 0, 100);
    SendMessageA(progress, PBM_SETPOS, 25, 0);
    track = child(TRACKBAR_CLASS, "", WS_TABSTOP | TBS_AUTOTICKS, 0, ID_TRACK, h);
    SendMessageA(track, TBM_SETRANGE, TRUE, MAKELONG(0, 100));
    SendMessageA(track, TBM_SETTICFREQ, 10, 0);
    SendMessageA(track, TBM_SETPOS, TRUE, 25);
    spin_edit = child("EDIT", "5", WS_TABSTOP | ES_NUMBER, WS_EX_CLIENTEDGE, ID_SPINEDIT, h);
    spin = child(UPDOWN_CLASS, "", UDS_SETBUDDYINT | UDS_ALIGNRIGHT | UDS_ARROWKEYS | UDS_NOTHOUSANDS, 0, ID_SPIN, h);
    SendMessageA(spin, UDM_SETRANGE32, 0, 50);

    show_page(0);
    /* controls created later sit below the ones created earlier: the pages go above the tab control */
    SetWindowPos(tab, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    layout(h);
    /* the spin control attaches to its edit box where the box is now */
    SendMessageA(spin, UDM_SETBUDDY, (WPARAM)spin_edit, 0);
    SendMessageA(spin, UDM_SETPOS32, 0, 5);
    SendMessageA(status, SB_SETTEXTA, 0, (LPARAM)"Ready");
    ShowWindow(h, show);
    MSG m;
    while (GetMessage(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessage(&m);
    }
    return 0;
}
