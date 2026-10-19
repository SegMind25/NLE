/* Message-level checks of the common controls: image lists, list view (items, sub-items, sorting, virtual lists), tree view,
 * tab, tool bar, progress / track / spin / status bars, window subclassing and TaskDialog. Prints "ok NAME" or
 * "FAIL NAME got=.. want=.." for each check and "DONE n" at the end. */
#include <windows.h>
#include <commctrl.h>
#include <stdio.h>
#include <string.h>

static int fails;
static int disp_calls;
static int sub_seen, sub_other;

#define CHECK(name, cond) do { int ok_ = !!(cond); printf("%s %s\n", ok_ ? "ok" : "FAIL", name); if (!ok_) fails++; fflush(stdout); } while (0)
#define CHECKI(name, got, want) do { long g_ = (long)(got), w_ = (long)(want); if (g_ == w_) printf("ok %s\n", name); else { printf("FAIL %s got=%ld want=%ld\n", name, g_, w_); fails++; } fflush(stdout); } while (0)
#define CHECKS(name, got, want) do { if (strcmp((got), (want)) == 0) printf("ok %s\n", name); else { printf("FAIL %s got=%s want=%s\n", name, (got), (want)); fails++; } fflush(stdout); } while (0)

static LRESULT CALLBACK proc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
    if (msg == WM_NOTIFY) {
        NMHDR *nm = (NMHDR *)lp;
        if (nm->code == LVN_GETDISPINFOA) {
            NMLVDISPINFOA *d = (NMLVDISPINFOA *)lp;
            disp_calls++;
            if (d->item.mask & LVIF_TEXT) sprintf(d->item.pszText, "Item %d.%d", d->item.iItem, d->item.iSubItem);
            return 0;
        }
    }
    if (msg == WM_DESTROY) { PostQuitMessage(0); return 0; }
    return DefWindowProcA(h, msg, wp, lp);
}

static LRESULT CALLBACK subproc(HWND h, UINT msg, WPARAM wp, LPARAM lp, UINT_PTR id, DWORD_PTR ref) {
    if (msg == WM_USER + 5) { sub_seen++; return (LRESULT)(ref + id); }
    if (msg == WM_USER + 6) { sub_other++; }
    return DefSubclassProc(h, msg, wp, lp);
}

static int CALLBACK by_param_desc(LPARAM a, LPARAM b, LPARAM sort) { (void)sort; return (int)(b - a); }

static HWND make(const char *cls, DWORD style, int id, HWND parent, int w, int h) {
    return CreateWindowExA(0, cls, "", WS_CHILD | WS_VISIBLE | style, 0, 0, w, h, parent, (HMENU)(INT_PTR)id, GetModuleHandleA(NULL), NULL);
}

static void image_lists(HWND win) {
    HIMAGELIST il = ImageList_Create(16, 16, ILC_COLOR32 | ILC_MASK, 2, 2);
    HICON ic = LoadIcon(NULL, IDI_APPLICATION);
    CHECK("icon loads", ic != NULL);
    int i0 = ImageList_ReplaceIcon(il, -1, ic), i1 = ImageList_ReplaceIcon(il, -1, ic);
    CHECKI("image list first index", i0, 0);
    CHECKI("image list second index", i1, 1);
    CHECKI("image list count", ImageList_GetImageCount(il), 2);
    int cx = 0, cy = 0;
    ImageList_GetIconSize(il, &cx, &cy);
    CHECKI("image list size", cx * 100 + cy, 1616);
    HDC screen = GetDC(win);
    HDC dc = CreateCompatibleDC(screen);
    HBITMAP bmp = CreateCompatibleBitmap(screen, 32, 32);
    SelectObject(dc, bmp);
    PatBlt(dc, 0, 0, 32, 32, WHITENESS);
    ImageList_Draw(il, 0, dc, 4, 4, ILD_NORMAL);
    int ink = 0;
    for (int y = 0; y < 32; y++) for (int x = 0; x < 32; x++) if (GetPixel(dc, x, y) != RGB(255, 255, 255)) ink++;
    CHECK("image list draws", ink > 20);
    CHECK("nothing drawn outside the image", GetPixel(dc, 0, 0) == RGB(255, 255, 255) && GetPixel(dc, 25, 25) == RGB(255, 255, 255));
    ImageList_Remove(il, 0);
    CHECKI("image list remove", ImageList_GetImageCount(il), 1);
    DeleteObject(bmp);
    DeleteDC(dc);
    ReleaseDC(win, screen);
    ImageList_Destroy(il);
}

static void list_view(HWND win) {
    HWND lv = make(WC_LISTVIEW, LVS_REPORT, 10, win, 300, 200);
    LVCOLUMNA col = {LVCF_TEXT | LVCF_WIDTH, 0, 100, "Name", 0, 0, 0, 0};
    CHECKI("insert column 0", SendMessageA(lv, LVM_INSERTCOLUMNA, 0, (LPARAM)&col), 0);
    col.pszText = "Size";
    col.cx = 60;
    CHECKI("insert column 1", SendMessageA(lv, LVM_INSERTCOLUMNA, 1, (LPARAM)&col), 1);
    CHECKI("column width", SendMessageA(lv, LVM_GETCOLUMNWIDTH, 1, 0), 60);
    char name[40];
    for (int i = 0; i < 5; i++) {
        sprintf(name, "file%d", i);
        LVITEMA it = {0};
        it.mask = LVIF_TEXT | LVIF_PARAM | LVIF_IMAGE;
        it.iItem = i;
        it.pszText = name;
        it.lParam = 100 + i;
        it.iImage = i % 2;
        CHECKI("insert item", SendMessageA(lv, LVM_INSERTITEMA, 0, (LPARAM)&it), i);
        char size[16];
        sprintf(size, "%d", i * 10);
        LVITEMA sub = {0};
        sub.iSubItem = 1;
        sub.pszText = size;
        SendMessageA(lv, LVM_SETITEMTEXTA, i, (LPARAM)&sub);
    }
    CHECKI("item count", SendMessageA(lv, LVM_GETITEMCOUNT, 0, 0), 5);
    char buf[40] = "";
    LVITEMA q = {0};
    q.iSubItem = 1;
    q.pszText = buf;
    q.cchTextMax = sizeof buf;
    SendMessageA(lv, LVM_GETITEMTEXTA, 3, (LPARAM)&q);
    CHECKS("sub-item text", buf, "30");
    LVITEMA g = {0};
    g.mask = LVIF_PARAM | LVIF_TEXT;
    g.iItem = 2;
    g.pszText = buf;
    g.cchTextMax = sizeof buf;
    CHECKI("get item", SendMessageA(lv, LVM_GETITEMA, 0, (LPARAM)&g), 1);
    CHECKI("item lParam", g.lParam, 102);
    CHECKS("item text", buf, "file2");
    LVFINDINFOA fi = {0};
    fi.flags = LVFI_STRING;
    fi.psz = "file3";
    CHECKI("find item by text", SendMessageA(lv, LVM_FINDITEMA, -1, (LPARAM)&fi), 3);
    fi.flags = LVFI_PARAM;
    fi.lParam = 104;
    CHECKI("find item by param", SendMessageA(lv, LVM_FINDITEMA, -1, (LPARAM)&fi), 4);
    LVITEMA st = {0};
    st.state = LVIS_SELECTED | LVIS_FOCUSED;
    st.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
    SendMessageA(lv, LVM_SETITEMSTATE, 1, (LPARAM)&st);
    CHECKI("selected count", SendMessageA(lv, LVM_GETSELECTEDCOUNT, 0, 0), 1);
    CHECKI("next selected", SendMessageA(lv, LVM_GETNEXTITEM, -1, LVNI_SELECTED), 1);
    CHECKI("item state", SendMessageA(lv, LVM_GETITEMSTATE, 1, LVIS_SELECTED), LVIS_SELECTED);
    RECT r;
    r.left = LVIR_BOUNDS;
    CHECKI("item rect", SendMessageA(lv, LVM_GETITEMRECT, 0, (LPARAM)&r), 1);
    CHECK("item rect has height", r.bottom - r.top >= 16 && r.right > r.left);
    LVHITTESTINFO ht = {{20, (r.top + r.bottom) / 2 + 2 * (r.bottom - r.top)}, 0, 0, 0};
    SendMessageA(lv, LVM_HITTEST, 0, (LPARAM)&ht);
    CHECKI("hit test item", ht.iItem, 2);
    SendMessageA(lv, LVM_SORTITEMS, 0, (LPARAM)by_param_desc);
    LVITEMA first = {0};
    first.mask = LVIF_PARAM;
    first.iItem = 0;
    SendMessageA(lv, LVM_GETITEMA, 0, (LPARAM)&first);
    CHECKI("sorted first lParam", first.lParam, 104);
    CHECKI("delete item", SendMessageA(lv, LVM_DELETEITEM, 0, 0), 1);
    CHECKI("count after delete", SendMessageA(lv, LVM_GETITEMCOUNT, 0, 0), 4);
    SendMessageA(lv, LVM_SETVIEW, LV_VIEW_LIST, 0);
    CHECKI("view", SendMessageA(lv, LVM_GETVIEW, 0, 0), LV_VIEW_LIST);
    SendMessageA(lv, LVM_DELETEALLITEMS, 0, 0);
    CHECKI("count after delete all", SendMessageA(lv, LVM_GETITEMCOUNT, 0, 0), 0);
    DestroyWindow(lv);

    /* a virtual list gets its text from the parent */
    HWND vl = make(WC_LISTVIEW, LVS_REPORT | LVS_OWNERDATA, 11, win, 300, 200);
    col.pszText = "Name";
    col.cx = 100;
    SendMessageA(vl, LVM_INSERTCOLUMNA, 0, (LPARAM)&col);
    SendMessageA(vl, LVM_SETITEMCOUNT, 500, 0);
    CHECKI("virtual count", SendMessageA(vl, LVM_GETITEMCOUNT, 0, 0), 500);
    LVITEMA vq = {0};
    vq.iSubItem = 0;
    vq.pszText = buf;
    vq.cchTextMax = sizeof buf;
    SendMessageA(vl, LVM_GETITEMTEXTA, 7, (LPARAM)&vq);
    CHECKS("virtual text from the parent", buf, "Item 7.0");
    CHECK("the parent was asked", disp_calls > 0);
    DestroyWindow(vl);
}

static void tree_view(HWND win) {
    HWND tv = make(WC_TREEVIEW, TVS_HASBUTTONS | TVS_HASLINES | TVS_LINESATROOT, 12, win, 250, 200);
    HIMAGELIST il = ImageList_Create(16, 16, ILC_COLOR32 | ILC_MASK, 2, 2);
    ImageList_ReplaceIcon(il, -1, LoadIcon(NULL, IDI_APPLICATION));
    SendMessageA(tv, TVM_SETIMAGELIST, TVSIL_NORMAL, (LPARAM)il);
    CHECK("tree image list", SendMessageA(tv, TVM_GETIMAGELIST, TVSIL_NORMAL, 0) == (LRESULT)il);
    TVINSERTSTRUCTA ins = {0};
    ins.hParent = TVI_ROOT;
    ins.hInsertAfter = TVI_LAST;
    ins.item.mask = TVIF_TEXT | TVIF_IMAGE | TVIF_SELECTEDIMAGE | TVIF_PARAM;
    ins.item.pszText = "pear";
    ins.item.lParam = 7;
    HTREEITEM pear = (HTREEITEM)SendMessageA(tv, TVM_INSERTITEMA, 0, (LPARAM)&ins);
    ins.item.pszText = "apple";
    ins.item.lParam = 8;
    HTREEITEM apple = (HTREEITEM)SendMessageA(tv, TVM_INSERTITEMA, 0, (LPARAM)&ins);
    ins.hParent = apple;
    ins.item.pszText = "seed";
    ins.item.lParam = 9;
    HTREEITEM seed = (HTREEITEM)SendMessageA(tv, TVM_INSERTITEMA, 0, (LPARAM)&ins);
    CHECK("tree items have handles", pear && apple && seed && pear != apple);
    CHECKI("tree count", SendMessageA(tv, TVM_GETCOUNT, 0, 0), 3);
    CHECK("root", SendMessageA(tv, TVM_GETNEXTITEM, TVGN_ROOT, 0) == (LRESULT)pear);
    CHECK("next sibling", SendMessageA(tv, TVM_GETNEXTITEM, TVGN_NEXT, (LPARAM)pear) == (LRESULT)apple);
    CHECK("child", SendMessageA(tv, TVM_GETNEXTITEM, TVGN_CHILD, (LPARAM)apple) == (LRESULT)seed);
    CHECK("parent", SendMessageA(tv, TVM_GETNEXTITEM, TVGN_PARENT, (LPARAM)seed) == (LRESULT)apple);
    char buf[40] = "";
    TVITEMA it = {0};
    it.mask = TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN | TVIF_HANDLE;
    it.hItem = apple;
    it.pszText = buf;
    it.cchTextMax = sizeof buf;
    SendMessageA(tv, TVM_GETITEMA, 0, (LPARAM)&it);
    CHECKS("item text", buf, "apple");
    CHECKI("item param", it.lParam, 8);
    CHECKI("item has children", it.cChildren, 1);
    SendMessageA(tv, TVM_SORTCHILDREN, 0, (LPARAM)TVI_ROOT);
    CHECK("sorted: apple first", SendMessageA(tv, TVM_GETNEXTITEM, TVGN_ROOT, 0) == (LRESULT)apple);
    CHECKI("collapsed: seed not shown", SendMessageA(tv, TVM_GETVISIBLECOUNT, 0, 0) >= 2, 1);
    CHECK("expand", SendMessageA(tv, TVM_EXPAND, TVE_EXPAND, (LPARAM)apple));
    RECT r;
    *(HTREEITEM *)&r = seed;
    CHECKI("seed is shown after expanding", SendMessageA(tv, TVM_GETITEMRECT, TRUE, (LPARAM)&r), 1);
    TVHITTESTINFO ht = {{(r.left + r.right) / 2, (r.top + r.bottom) / 2}, 0, 0};
    SendMessageA(tv, TVM_HITTEST, 0, (LPARAM)&ht);
    CHECK("hit test finds the item", ht.hItem == seed && (ht.flags & TVHT_ONITEMLABEL));
    CHECK("select", SendMessageA(tv, TVM_SELECTITEM, TVGN_CARET, (LPARAM)seed));
    CHECK("selected", SendMessageA(tv, TVM_GETNEXTITEM, TVGN_CARET, 0) == (LRESULT)seed);
    CHECK("delete", SendMessageA(tv, TVM_DELETEITEM, 0, (LPARAM)apple));
    CHECKI("count after delete", SendMessageA(tv, TVM_GETCOUNT, 0, 0), 1);
    CHECK("selection moved off the deleted item", SendMessageA(tv, TVM_GETNEXTITEM, TVGN_CARET, 0) != (LRESULT)seed);
    DestroyWindow(tv);
    ImageList_Destroy(il);
}

static void tab_toolbar_bars(HWND win) {
    HWND tab = make(WC_TABCONTROL, 0, 13, win, 200, 100);
    for (int i = 0; i < 3; i++) {
        char t[16];
        sprintf(t, "tab%d", i);
        TCITEMA ti = {0};
        ti.mask = TCIF_TEXT | TCIF_PARAM;
        ti.pszText = t;
        ti.lParam = 50 + i;
        CHECKI("insert tab", SendMessageA(tab, TCM_INSERTITEMA, i, (LPARAM)&ti), i);
    }
    CHECKI("tab count", SendMessageA(tab, TCM_GETITEMCOUNT, 0, 0), 3);
    CHECKI("tab select", SendMessageA(tab, TCM_SETCURSEL, 2, 0), 0);
    CHECKI("tab current", SendMessageA(tab, TCM_GETCURSEL, 0, 0), 2);
    char buf[16] = "";
    TCITEMA gt = {0};
    gt.mask = TCIF_TEXT | TCIF_PARAM;
    gt.pszText = buf;
    gt.cchTextMax = sizeof buf;
    SendMessageA(tab, TCM_GETITEMA, 1, (LPARAM)&gt);
    CHECKS("tab text", buf, "tab1");
    CHECKI("tab param", gt.lParam, 51);
    RECT ir;
    SendMessageA(tab, TCM_GETITEMRECT, 1, (LPARAM)&ir);
    TCHITTESTINFO th = {{(ir.left + ir.right) / 2, (ir.top + ir.bottom) / 2}, 0};
    CHECKI("tab hit test", SendMessageA(tab, TCM_HITTEST, 0, (LPARAM)&th), 1);
    RECT page = {0, 0, 200, 100};
    SendMessageA(tab, TCM_ADJUSTRECT, FALSE, (LPARAM)&page);
    CHECK("the page area is inside the tab control", page.left > 0 && page.top > 15 && page.right < 200 && page.bottom < 100);
    SendMessageA(tab, TCM_DELETEITEM, 0, 0);
    CHECKI("tab delete", SendMessageA(tab, TCM_GETITEMCOUNT, 0, 0), 2);
    DestroyWindow(tab);

    HWND tb = make(TOOLBARCLASSNAME, CCS_TOP, 14, win, 300, 30);
    SendMessageA(tb, TB_BUTTONSTRUCTSIZE, sizeof(TBBUTTON), 0);
    TBADDBITMAP ab = {HINST_COMMCTRL, IDB_STD_SMALL_COLOR};
    SendMessageA(tb, TB_ADDBITMAP, 0, (LPARAM)&ab);
    SendMessageA(tb, TB_ADDSTRINGA, 0, (LPARAM)"One\0Two\0\0");
    TBBUTTON b[3] = {
        {STD_FILENEW, 301, TBSTATE_ENABLED, BTNS_BUTTON, {0}, 11, 0},
        {0, 0, 0, BTNS_SEP, {0}, 0, 0},
        {STD_FILESAVE, 302, TBSTATE_ENABLED, BTNS_CHECK, {0}, 22, 1},
    };
    SendMessageA(tb, TB_ADDBUTTONS, 3, (LPARAM)b);
    CHECKI("toolbar count", SendMessageA(tb, TB_BUTTONCOUNT, 0, 0), 3);
    CHECKI("command to index", SendMessageA(tb, TB_COMMANDTOINDEX, 302, 0), 2);
    CHECKI("enabled", SendMessageA(tb, TB_ISBUTTONENABLED, 301, 0) != 0, 1);
    SendMessageA(tb, TB_ENABLEBUTTON, 301, MAKELONG(FALSE, 0));
    CHECKI("disabled", SendMessageA(tb, TB_ISBUTTONENABLED, 301, 0), 0);
    SendMessageA(tb, TB_CHECKBUTTON, 302, MAKELONG(TRUE, 0));
    CHECKI("checked", SendMessageA(tb, TB_ISBUTTONCHECKED, 302, 0) != 0, 1);
    CHECKI("state", SendMessageA(tb, TB_GETSTATE, 302, 0) & TBSTATE_CHECKED, TBSTATE_CHECKED);
    RECT br;
    SendMessageA(tb, TB_GETITEMRECT, 2, (LPARAM)&br);
    CHECK("button rectangle", br.right - br.left >= 16 && br.left > 0);
    TBBUTTON got;
    SendMessageA(tb, TB_GETBUTTON, 2, (LPARAM)&got);
    CHECKI("get button command", got.idCommand, 302);
    TBBUTTONINFOA bi = {sizeof bi, TBIF_COMMAND | TBIF_IMAGE, 0, 5, 0, 0, 0, 0, NULL, 0};
    bi.idCommand = 303;
    bi.iImage = STD_FIND;
    SendMessageA(tb, TB_SETBUTTONINFOA, 302, (LPARAM)&bi);
    CHECKI("button renumbered", SendMessageA(tb, TB_COMMANDTOINDEX, 303, 0), 2);
    SendMessageA(tb, TB_DELETEBUTTON, 1, 0);
    CHECKI("toolbar count after delete", SendMessageA(tb, TB_BUTTONCOUNT, 0, 0), 2);
    DestroyWindow(tb);

    HWND pb = make(PROGRESS_CLASS, 0, 15, win, 100, 16);
    SendMessageA(pb, PBM_SETRANGE32, 0, 50);
    SendMessageA(pb, PBM_SETPOS, 10, 0);
    SendMessageA(pb, PBM_DELTAPOS, 5, 0);
    CHECKI("progress delta", SendMessageA(pb, PBM_GETPOS, 0, 0), 15);
    SendMessageA(pb, PBM_SETSTEP, 7, 0);
    SendMessageA(pb, PBM_STEPIT, 0, 0);
    CHECKI("progress step", SendMessageA(pb, PBM_GETPOS, 0, 0), 22);
    SendMessageA(pb, PBM_SETPOS, 99, 0);
    CHECKI("progress is clamped", SendMessageA(pb, PBM_GETPOS, 0, 0), 50);
    DestroyWindow(pb);

    HWND tk = make(TRACKBAR_CLASS, 0, 16, win, 150, 30);
    SendMessageA(tk, TBM_SETRANGE, TRUE, MAKELONG(10, 90));
    SendMessageA(tk, TBM_SETPOS, TRUE, 40);
    CHECKI("track pos", SendMessageA(tk, TBM_GETPOS, 0, 0), 40);
    CHECKI("track min", SendMessageA(tk, TBM_GETRANGEMIN, 0, 0), 10);
    CHECKI("track max", SendMessageA(tk, TBM_GETRANGEMAX, 0, 0), 90);
    SendMessageA(tk, TBM_SETPOS, TRUE, 500);
    CHECKI("track is clamped", SendMessageA(tk, TBM_GETPOS, 0, 0), 90);
    DestroyWindow(tk);

    HWND ud = make(UPDOWN_CLASS, UDS_ALIGNRIGHT | UDS_ARROWKEYS, 17, win, 16, 22);
    SendMessageA(ud, UDM_SETRANGE32, -5, 5);
    SendMessageA(ud, UDM_SETPOS32, 0, 3);
    int lo = 0, hi = 0;
    SendMessageA(ud, UDM_GETRANGE32, (WPARAM)&lo, (LPARAM)&hi);
    CHECKI("updown range", lo * 100 + hi, -495);
    CHECKI("updown pos", SendMessageA(ud, UDM_GETPOS32, 0, 0), 3);
    DestroyWindow(ud);

    HWND sb = make(STATUSCLASSNAME, 0, 18, win, 300, 20);
    int parts[3] = {100, 200, -1};
    SendMessageA(sb, SB_SETPARTS, 3, (LPARAM)parts);
    CHECKI("status parts", SendMessageA(sb, SB_GETPARTS, 0, 0), 3);
    SendMessageA(sb, SB_SETTEXTA, 1, (LPARAM)"middle");
    char t[32] = "";
    SendMessageA(sb, SB_GETTEXTA, 1, (LPARAM)t);
    CHECKS("status text", t, "middle");
    CHECKI("status text length", LOWORD(SendMessageA(sb, SB_GETTEXTLENGTHA, 1, 0)), 6);
    DestroyWindow(sb);
}

static void subclassing(HWND win) {
    HWND st = make("STATIC", 0, 19, win, 40, 20);
    CHECK("subclass", SetWindowSubclass(st, subproc, 3, 40));
    CHECKI("subclass proc answers", SendMessageA(st, WM_USER + 5, 0, 0), 43);
    SendMessageA(st, WM_USER + 6, 0, 0);
    CHECKI("subclass sees every message", sub_seen * 10 + sub_other, 11);
    DWORD_PTR ref = 0;
    CHECK("get subclass", GetWindowSubclass(st, subproc, 3, &ref) && ref == 40);
    SetWindowTextA(st, "hello");
    char buf[16];
    GetWindowTextA(st, buf, sizeof buf);
    CHECKS("unhandled messages reach the control", buf, "hello");
    CHECK("remove subclass", RemoveWindowSubclass(st, subproc, 3));
    CHECKI("gone", SendMessageA(st, WM_USER + 5, 0, 0), 0);
    DestroyWindow(st);
}

static void task_dialog(void) {
    int button = 0;
    HRESULT hr = TaskDialog(NULL, NULL, L"Title", L"Main instruction", L"Content", TDCBF_OK_BUTTON | TDCBF_CANCEL_BUTTON, TD_INFORMATION_ICON, &button);
    CHECKI("task dialog result", hr, 0);
    CHECKI("task dialog button", button, IDOK);
}

int main(void) {
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = GetModuleHandleA(NULL);
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE + 1);
    wc.lpszClassName = "ComctlCheck";
    RegisterClassA(&wc);
    HWND win = CreateWindowA("ComctlCheck", "comctl", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 50, 50, 500, 400, NULL, NULL, wc.hInstance, NULL);
    image_lists(win);
    list_view(win);
    tree_view(win);
    tab_toolbar_bars(win);
    subclassing(win);
    task_dialog();
    printf("DONE %d\n", fails);
    fflush(stdout);
    DestroyWindow(win);
    return fails ? 1 : 0;
}
