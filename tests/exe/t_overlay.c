#include <windows.h>
#include <stdio.h>
#include <string.h>
#include "check.h"

/* A program that reads its own file, as launchers and self-extracting archives do. The test harness appends
 * MARKER after the linker is done, so the data is an overlay: it is not part of any section. */
#define MARKER "NLE-OVERLAY-MARKER!"

int main(void) {
    char path[MAX_PATH];
    CHECK(GetModuleFileNameA(NULL, path, sizeof path) > 0);
    HANDLE f = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
    CHECK(f != INVALID_HANDLE_VALUE);
    if (f == INVALID_HANDLE_VALUE) return 1;

    char head[2];
    DWORD got = 0;
    CHECK(ReadFile(f, head, 2, &got, NULL) && got == 2);
    CHECK(head[0] == 'M' && head[1] == 'Z');

    DWORD size = GetFileSize(f, NULL);
    CHECK(size > sizeof MARKER);
    char tail[sizeof MARKER];
    CHECK(SetFilePointer(f, -(LONG)(sizeof MARKER - 1), NULL, FILE_END) != INVALID_SET_FILE_POINTER);
    memset(tail, 0, sizeof tail);
    CHECK(ReadFile(f, tail, sizeof MARKER - 1, &got, NULL) && got == sizeof MARKER - 1);
    CHECK(strcmp(tail, MARKER) == 0);

    /* The PE header says where the sections end; the overlay starts right after them. */
    LONG pe_off;
    CHECK(SetFilePointer(f, 0x3c, NULL, FILE_BEGIN) == 0x3c);
    CHECK(ReadFile(f, &pe_off, 4, &got, NULL) && got == 4);
    CHECK(SetFilePointer(f, pe_off, NULL, FILE_BEGIN) == (DWORD)pe_off);
    char sig[4];
    CHECK(ReadFile(f, sig, 4, &got, NULL) && got == 4);
    CHECK(memcmp(sig, "PE\0\0", 4) == 0);
    CloseHandle(f);

    if (g_failures == 0) printf("ALL OK (%d checks)\n", g_checks);
    return g_failures;
}
