#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>
#include "check.h"

int main(void) {
    const char *utf8 = "h\xc3\xa9llo \xe2\x9c\x93 \xf0\x9f\x98\x80";
    WCHAR w[64];
    int n = MultiByteToWideChar(CP_UTF8, 0, utf8, -1, w, 64);
    CHECK_EQ_INT(n, 11);
    CHECK_EQ_INT(w[1], 0xe9);
    CHECK_EQ_INT(w[8], 0xd83d);
    CHECK_EQ_INT(w[9], 0xde00);
    char back[64];
    CHECK_EQ_INT(WideCharToMultiByte(CP_UTF8, 0, w, -1, back, 64, NULL, NULL), 16);
    CHECK_STR(back, utf8);
    CHECK_EQ_INT(MultiByteToWideChar(CP_UTF8, 0, utf8, -1, NULL, 0), 11);
    CHECK_EQ_INT(MultiByteToWideChar(CP_UTF8, 0, utf8, -1, w, 3), 0);
    CHECK_EQ_INT(GetLastError(), ERROR_INSUFFICIENT_BUFFER);
    CHECK_EQ_INT(MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, "\xff\xfe", -1, w, 64), 0);
    CHECK_EQ_INT(GetLastError(), ERROR_NO_UNICODE_TRANSLATION);
    CHECK_EQ_INT(GetACP(), CP_UTF8);

    WCHAR up[] = L"MiXed \u00e9\u00e0";
    CharUpperW(up);
    CHECK(wcscmp(up, L"MIXED \u00c9\u00c0") == 0);
    CharLowerW(up);
    CHECK(wcscmp(up, L"mixed \u00e9\u00e0") == 0);
    CHECK_EQ_INT(CompareStringW(LOCALE_USER_DEFAULT, 0, L"abc", -1, L"abd", -1), CSTR_LESS_THAN);
    CHECK_EQ_INT(CompareStringW(LOCALE_USER_DEFAULT, NORM_IGNORECASE, L"ABC", -1, L"abc", -1), CSTR_EQUAL);
    CHECK(lstrcmpiW(L"Hello", L"hELLO") == 0);
    CHECK(lstrcmpW(L"a", L"b") < 0);
    WORD types[4];
    GetStringTypeW(CT_CTYPE1, L"a1 !", 4, types);
    CHECK(types[0] & C1_ALPHA);
    CHECK(types[1] & C1_DIGIT);
    CHECK(types[2] & C1_SPACE);
    CHECK(types[3] & C1_PUNCT);
    WCHAR lm[16];
    CHECK_EQ_INT(LCMapStringW(LOCALE_USER_DEFAULT, LCMAP_LOWERCASE, L"ABC", -1, lm, 16), 4);
    CHECK(wcscmp(lm, L"abc") == 0);
    WCHAR dec[8];
    CHECK(GetLocaleInfoW(LOCALE_USER_DEFAULT, LOCALE_SDECIMAL, dec, 8) > 0);
    CHECK(wcscmp(dec, L".") == 0);
    CHECK(GetUserDefaultLCID() == 0x409);

    WCHAR temp[MAX_PATH], path[MAX_PATH];
    GetTempPathW(MAX_PATH, temp);
    swprintf(path, MAX_PATH, L"%ls\u0442\u0435\u0441\u0442_%lu.txt", temp, GetCurrentProcessId());
    HANDLE h = CreateFileW(path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
    CHECK(h != INVALID_HANDLE_VALUE);
    DWORD wr;
    WriteFile(h, "x", 1, &wr, NULL);
    CloseHandle(h);
    WIN32_FIND_DATAW fd;
    WCHAR pattern[MAX_PATH];
    swprintf(pattern, MAX_PATH, L"%ls\u0442\u0435\u0441\u0442_*.txt", temp);
    HANDLE fh = FindFirstFileW(pattern, &fd);
    CHECK(fh != INVALID_HANDLE_VALUE);
    if (fh != INVALID_HANDLE_VALUE) {
        CHECK(wcsncmp(fd.cFileName, L"\u0442\u0435\u0441\u0442_", 5) == 0);
        FindClose(fh);
    }
    CHECK(DeleteFileW(path));

    wprintf(L"wide output: %ls %d\n", L"\u00e9\u00e0 \u2713", 7);
    printf("narrow output: %s\n", utf8);
    fputws(L"fputws \u00fc\n", stdout);

    WCHAR wsz[] = L"12345 tail";
    WCHAR *end;
    CHECK_EQ_INT(wcstol(wsz, &end, 10), 12345);
    CHECK(wcscmp(end, L" tail") == 0);
    CHECK_EQ_INT(towupper(L'\u00e9'), 0xc9);
    CHECK(iswalpha(L'\u00e9'));
    return finish();
}
