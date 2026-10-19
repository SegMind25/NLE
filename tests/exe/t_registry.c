#include <windows.h>
#include <shlwapi.h>
#include <stdio.h>
#include <string.h>
#include "check.h"

#define APP "Software\\NleTest\\Registry"

static int child_main(void) {
    char buf[64] = {0};
    DWORD len = sizeof buf, type = 0;
    LONG r = RegGetValueA(HKEY_CURRENT_USER, APP, "FromParent", RRF_RT_REG_SZ, &type, buf, &len);
    if (r != ERROR_SUCCESS) return 100 + (int)r;
    printf("child saw [%s]\n", buf);
    return strcmp(buf, "persisted") == 0 ? 0 : 99;
}

int main(int argc, char **argv) {
    if (argc > 1 && !strcmp(argv[1], "child")) return child_main();

    HKEY k;
    RegDeleteTreeA(HKEY_CURRENT_USER, "Software\\NleTest");
    CHECK_EQ_INT(RegOpenKeyExA(HKEY_CURRENT_USER, APP, 0, KEY_READ, &k), ERROR_FILE_NOT_FOUND);

    DWORD disp = 0;
    CHECK_EQ_INT(RegCreateKeyExA(HKEY_CURRENT_USER, APP, 0, NULL, 0, KEY_ALL_ACCESS, NULL, &k, &disp), ERROR_SUCCESS);
    CHECK_EQ_INT(disp, REG_CREATED_NEW_KEY);
    HKEY k2;
    CHECK_EQ_INT(RegCreateKeyExA(HKEY_CURRENT_USER, "SOFTWARE\\nletest\\registry", 0, NULL, 0, KEY_ALL_ACCESS, NULL, &k2, &disp), ERROR_SUCCESS);
    CHECK_EQ_INT(disp, REG_OPENED_EXISTING_KEY);
    RegCloseKey(k2);

    /* values of every common type, written with the ANSI and wide APIs */
    DWORD dw = 0xdeadbeef;
    CHECK_EQ_INT(RegSetValueExA(k, "Number", 0, REG_DWORD, (BYTE *)&dw, sizeof dw), ERROR_SUCCESS);
    CHECK_EQ_INT(RegSetValueExA(k, "Text", 0, REG_SZ, (BYTE *)"héllo", 7), ERROR_SUCCESS);
    CHECK_EQ_INT(RegSetValueExW(k, L"Wide", 0, REG_SZ, (BYTE *)L"wide ü", 14), ERROR_SUCCESS);
    BYTE blob[5] = {0, 1, 2, 0xff, 0};
    CHECK_EQ_INT(RegSetValueExA(k, "Blob", 0, REG_BINARY, blob, sizeof blob), ERROR_SUCCESS);
    CHECK_EQ_INT(RegSetValueExA(k, "Expand", 0, REG_EXPAND_SZ, (BYTE *)"%SystemRoot%\\x", 15), ERROR_SUCCESS);
    CHECK_EQ_INT(RegSetValueExA(k, "FromParent", 0, REG_SZ, (BYTE *)"persisted", 10), ERROR_SUCCESS);
    CHECK_EQ_INT(RegSetValueExA(k, NULL, 0, REG_SZ, (BYTE *)"default", 8), ERROR_SUCCESS);

    DWORD type, size = sizeof dw, out = 0;
    CHECK_EQ_INT(RegQueryValueExA(k, "number", NULL, &type, (BYTE *)&out, &size), ERROR_SUCCESS);
    CHECK_EQ_INT(type, REG_DWORD);
    CHECK_EQ_INT(out, 0xdeadbeef);

    char text[32] = {0};
    size = sizeof text;
    CHECK_EQ_INT(RegQueryValueExA(k, "Text", NULL, &type, (BYTE *)text, &size), ERROR_SUCCESS);
    CHECK_STR(text, "héllo");
    CHECK_EQ_INT(size, 7);
    wchar_t wtext[32] = {0};
    size = sizeof wtext;
    CHECK_EQ_INT(RegQueryValueExW(k, L"Text", NULL, &type, (BYTE *)wtext, &size), ERROR_SUCCESS);
    CHECK(wcscmp(wtext, L"héllo") == 0);
    CHECK_EQ_INT(size, 12);

    /* size query, then too-small buffer */
    size = 0;
    CHECK_EQ_INT(RegQueryValueExA(k, "Blob", NULL, &type, NULL, &size), ERROR_SUCCESS);
    CHECK_EQ_INT(size, 5);
    BYTE small[2];
    size = sizeof small;
    CHECK_EQ_INT(RegQueryValueExA(k, "Blob", NULL, &type, small, &size), ERROR_MORE_DATA);
    CHECK_EQ_INT(size, 5);
    CHECK_EQ_INT(RegQueryValueExA(k, "Nope", NULL, &type, NULL, &size), ERROR_FILE_NOT_FOUND);

    size = sizeof text;
    CHECK_EQ_INT(RegQueryValueExA(k, NULL, NULL, &type, (BYTE *)text, &size), ERROR_SUCCESS);
    CHECK_STR(text, "default");

    /* RegGetValue: type filter and environment expansion */
    char expanded[MAX_PATH] = {0};
    size = sizeof expanded;
    CHECK_EQ_INT(RegGetValueA(k, NULL, "Expand", RRF_RT_REG_SZ, &type, expanded, &size), ERROR_SUCCESS);
    CHECK_EQ_INT(type, REG_SZ);
    CHECK(strncmp(expanded, "C:\\windows\\x", 12) == 0);
    size = sizeof expanded;
    CHECK_EQ_INT(RegGetValueA(k, NULL, "Number", RRF_RT_REG_SZ, NULL, expanded, &size), ERROR_UNSUPPORTED_TYPE);
    size = sizeof out;
    CHECK_EQ_INT(RegGetValueA(HKEY_CURRENT_USER, APP, "Number", RRF_RT_DWORD, NULL, &out, &size), ERROR_SUCCESS);
    CHECK_EQ_INT(out, 0xdeadbeef);

    /* enumeration */
    CHECK_EQ_INT(RegCreateKeyExA(k, "Beta", 0, NULL, 0, KEY_ALL_ACCESS, NULL, &k2, NULL), ERROR_SUCCESS);
    RegCloseKey(k2);
    CHECK_EQ_INT(RegCreateKeyExA(k, "alpha", 0, NULL, 0, KEY_ALL_ACCESS, NULL, &k2, NULL), ERROR_SUCCESS);
    RegCloseKey(k2);
    char name[32];
    DWORD nlen = sizeof name;
    CHECK_EQ_INT(RegEnumKeyExA(k, 0, name, &nlen, NULL, NULL, NULL, NULL), ERROR_SUCCESS);
    CHECK_STR(name, "alpha");
    CHECK_EQ_INT(nlen, 5);
    nlen = sizeof name;
    CHECK_EQ_INT(RegEnumKeyExA(k, 1, name, &nlen, NULL, NULL, NULL, NULL), ERROR_SUCCESS);
    CHECK_STR(name, "Beta");
    nlen = sizeof name;
    CHECK_EQ_INT(RegEnumKeyExA(k, 2, name, &nlen, NULL, NULL, NULL, NULL), ERROR_NO_MORE_ITEMS);
    nlen = 3;
    CHECK_EQ_INT(RegEnumKeyExA(k, 0, name, &nlen, NULL, NULL, NULL, NULL), ERROR_MORE_DATA);

    DWORD subkeys = 0, values = 0, maxname = 0, maxdata = 0;
    CHECK_EQ_INT(RegQueryInfoKeyA(k, NULL, NULL, NULL, &subkeys, NULL, NULL, &values, &maxname, &maxdata, NULL, NULL), ERROR_SUCCESS);
    CHECK_EQ_INT(subkeys, 2);
    CHECK_EQ_INT(values, 7);
    CHECK_EQ_INT(maxname, 10);
    int seen = 0;
    for (DWORD i = 0;; i++) {
        nlen = sizeof name;
        size = sizeof text;
        LONG r = RegEnumValueA(k, i, name, &nlen, NULL, &type, (BYTE *)text, &size);
        if (r == ERROR_NO_MORE_ITEMS) break;
        CHECK_EQ_INT(r, ERROR_SUCCESS);
        seen++;
        if (!strcmp(name, "Text")) CHECK_STR(text, "héllo");
    }
    CHECK_EQ_INT(seen, 7);

    /* shlwapi helpers */
    char viashl[32] = {0};
    size = sizeof viashl;
    CHECK_EQ_INT(SHGetValueA(HKEY_CURRENT_USER, APP, "FromParent", &type, viashl, &size), ERROR_SUCCESS);
    CHECK_STR(viashl, "persisted");

    /* another process sees what we wrote */
    char self[MAX_PATH], cmd[MAX_PATH + 16];
    GetModuleFileNameA(NULL, self, sizeof self);
    snprintf(cmd, sizeof cmd, "\"%s\" child", self);
    STARTUPINFOA si = {sizeof si};
    PROCESS_INFORMATION pi;
    CHECK(CreateProcessA(NULL, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi));
    WaitForSingleObject(pi.hProcess, 30000);
    DWORD code = 77;
    GetExitCodeProcess(pi.hProcess, &code);
    CHECK_EQ_INT(code, 0);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);

    /* the seeded Windows version keys are there */
    HKEY nt;
    CHECK_EQ_INT(RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", 0, KEY_READ, &nt), ERROR_SUCCESS);
    char build[16] = {0};
    size = sizeof build;
    CHECK_EQ_INT(RegQueryValueExA(nt, "CurrentBuildNumber", NULL, NULL, (BYTE *)build, &size), ERROR_SUCCESS);
    CHECK_STR(build, "19045");
    RegCloseKey(nt);

    /* deletion rules */
    CHECK_EQ_INT(RegDeleteValueA(k, "Number"), ERROR_SUCCESS);
    CHECK_EQ_INT(RegDeleteValueA(k, "Number"), ERROR_FILE_NOT_FOUND);
    RegCloseKey(k);
    CHECK_EQ_INT(RegDeleteKeyA(HKEY_CURRENT_USER, APP), ERROR_ACCESS_DENIED);
    CHECK_EQ_INT(RegDeleteTreeA(HKEY_CURRENT_USER, "Software\\NleTest"), ERROR_SUCCESS);
    CHECK_EQ_INT(RegOpenKeyExA(HKEY_CURRENT_USER, APP, 0, KEY_READ, &k), ERROR_FILE_NOT_FOUND);
    return finish();
}
