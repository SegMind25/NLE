#include <windows.h>
#include <stdio.h>
#include <string.h>
#include "check.h"

static int enum_count = 0;
static BOOL CALLBACK count_names(HMODULE m, LPCSTR type, LPSTR name, LONG_PTR lp) {
    (void)m; (void)type; (void)name; (void)lp;
    enum_count++;
    return TRUE;
}

int main(void) {
    HMODULE self = GetModuleHandleA(NULL);
    char a[64];
    WCHAR w[64];

    /* string tables */
    CHECK_EQ_INT(LoadStringA(self, 1, a, sizeof a), 14);
    CHECK_STR(a, "Hello resource");
    CHECK_EQ_INT(LoadStringW(self, 17, w, 64), 16);
    CHECK(wcscmp(w, L"In another block") == 0);
    CHECK_EQ_INT(LoadStringA(self, 1, a, 6), 5);
    CHECK_STR(a, "Hello");
    CHECK_EQ_INT(LoadStringA(self, 99, a, sizeof a), 0);
    LPCWSTR direct = NULL;
    CHECK_EQ_INT(LoadStringW(self, 2, (LPWSTR)&direct, 0), 13);
    CHECK(direct != NULL && wcsncmp(direct, L"Second string", 13) == 0);

    /* raw resources */
    HRSRC r = FindResourceA(self, "BLOB", RT_RCDATA);
    CHECK(r != NULL);
    if (r) {
        CHECK_EQ_INT(SizeofResource(self, r), 9);
        HGLOBAL g = LoadResource(self, r);
        const char *p = (const char *)LockResource(g);
        CHECK(p != NULL && strcmp(p, "NLE-BLOB") == 0);
    }
    CHECK(FindResourceA(self, "blob", RT_RCDATA) != NULL);              /* names are case-insensitive */
    CHECK(FindResourceA(self, MAKEINTRESOURCEA(7), RT_RCDATA) != NULL);
    CHECK(FindResourceA(self, "#7", RT_RCDATA) != NULL);
    CHECK(FindResourceA(self, "NOPE", RT_RCDATA) == NULL);
    CHECK_EQ_INT(GetLastError(), ERROR_RESOURCE_NAME_NOT_FOUND);
    CHECK(FindResourceA(self, "BLOB", RT_CURSOR) == NULL);
    CHECK_EQ_INT(GetLastError(), ERROR_RESOURCE_TYPE_NOT_FOUND);
    CHECK(FindResourceExA(self, RT_RCDATA, MAKEINTRESOURCEA(7), MAKELANGID(LANG_ENGLISH, SUBLANG_ENGLISH_US)) != NULL);
    enum_count = 0;
    CHECK(EnumResourceNamesA(self, RT_RCDATA, count_names, 0));
    CHECK_EQ_INT(enum_count, 2);

    /* an icon: opaque red on the left half, transparent on the right */
    HICON icon = LoadIconA(self, MAKEINTRESOURCEA(1));
    CHECK(icon != NULL);
    HDC screen = GetDC(NULL);
    HDC dc = CreateCompatibleDC(screen);
    HBITMAP bmp = CreateCompatibleBitmap(screen, 40, 40);
    HGDIOBJ old = SelectObject(dc, bmp);
    RECT all = {0, 0, 40, 40};
    HBRUSH white = CreateSolidBrush(RGB(255, 255, 255));
    FillRect(dc, &all, white);
    CHECK(DrawIconEx(dc, 4, 4, icon, 0, 0, 0, NULL, DI_NORMAL));
    CHECK_EQ_INT(GetPixel(dc, 6, 8), RGB(255, 0, 0));
    CHECK_EQ_INT(GetPixel(dc, 16, 8), RGB(255, 255, 255));
    CHECK_EQ_INT(GetPixel(dc, 2, 2), RGB(255, 255, 255));
    CHECK(DrawIcon(dc, 20, 20, icon));
    CHECK_EQ_INT(GetPixel(dc, 22, 24), RGB(255, 0, 0));
    HICON big = (HICON)LoadImageA(self, MAKEINTRESOURCEA(1), IMAGE_ICON, 32, 32, 0);
    CHECK(big != NULL);
    CHECK(DestroyIcon(big));

    /* a bitmap */
    HBITMAP rb = LoadBitmapA(self, MAKEINTRESOURCEA(2));
    CHECK(rb != NULL);
    BITMAP info;
    CHECK(GetObjectA(rb, sizeof info, &info) > 0);
    CHECK_EQ_INT(info.bmWidth, 4);
    CHECK_EQ_INT(info.bmHeight, 2);
    HDC mem = CreateCompatibleDC(screen);
    HGDIOBJ oldb = SelectObject(mem, rb);
    CHECK_EQ_INT(GetPixel(mem, 0, 0), RGB(0, 0, 255));
    CHECK_EQ_INT(GetPixel(mem, 1, 0), RGB(0, 255, 0));
    CHECK_EQ_INT(GetPixel(mem, 2, 0), RGB(255, 0, 0));
    CHECK_EQ_INT(GetPixel(mem, 3, 0), RGB(255, 255, 255));
    CHECK_EQ_INT(GetPixel(mem, 0, 1), RGB(0, 0, 0));
    CHECK(BitBlt(dc, 0, 30, 4, 2, mem, 0, 0, SRCCOPY));
    CHECK_EQ_INT(GetPixel(dc, 2, 30), RGB(255, 0, 0));
    SelectObject(mem, oldb);
    DeleteObject(rb);
    DeleteDC(mem);

    SelectObject(dc, old);
    DeleteObject(bmp);
    DeleteObject(white);
    DeleteDC(dc);
    ReleaseDC(NULL, screen);

    /* a module without resources */
    HMODULE k32 = GetModuleHandleA("kernel32.dll");
    CHECK(FindResourceA(k32, "BLOB", RT_RCDATA) == NULL);

    if (g_failures == 0) printf("ALL OK (%d checks)\n", g_checks);
    return g_failures;
}
