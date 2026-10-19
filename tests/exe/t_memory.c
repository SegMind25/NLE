#include <windows.h>
#include <malloc.h>
#include <string.h>
#include "check.h"

static DWORD WINAPI reader(LPVOID p) {
    volatile char *v = (volatile char *)p;
    return v[0] == 'S' && v[1] == 'H' ? 0 : 1;
}

int main(void) {
    SYSTEM_INFO si;
    GetSystemInfo(&si);
    CHECK_EQ_INT(si.dwPageSize, 4096);
    CHECK_EQ_INT(si.dwAllocationGranularity, 65536);
    CHECK(si.dwNumberOfProcessors >= 1);

    /* reserve, commit, use, decommit, release */
    char *base = VirtualAlloc(NULL, 1 << 20, MEM_RESERVE, PAGE_NOACCESS);
    CHECK(base != NULL);
    CHECK_EQ_INT((uintptr_t)base % 65536, 0);
    MEMORY_BASIC_INFORMATION mbi;
    CHECK(VirtualQuery(base, &mbi, sizeof mbi) == sizeof mbi);
    CHECK_EQ_INT(mbi.State, MEM_RESERVE);
    CHECK_EQ_INT(mbi.RegionSize, 1 << 20);
    CHECK(VirtualAlloc(base, 8192, MEM_COMMIT, PAGE_READWRITE) == base);
    memset(base, 0x5a, 8192);
    CHECK_EQ_INT(base[8191], 0x5a);
    VirtualQuery(base, &mbi, sizeof mbi);
    CHECK_EQ_INT(mbi.State, MEM_COMMIT);
    CHECK_EQ_INT(mbi.Protect, PAGE_READWRITE);
    CHECK_EQ_INT(mbi.RegionSize, 8192);
    DWORD old = 0;
    CHECK(VirtualProtect(base, 4096, PAGE_READONLY, &old));
    CHECK_EQ_INT(old, PAGE_READWRITE);
    VirtualQuery(base, &mbi, sizeof mbi);
    CHECK_EQ_INT(mbi.Protect, PAGE_READONLY);
    CHECK(VirtualFree(base, 8192, MEM_DECOMMIT));
    VirtualQuery(base, &mbi, sizeof mbi);
    CHECK_EQ_INT(mbi.State, MEM_RESERVE);
    CHECK(VirtualAlloc(base, 4096, MEM_COMMIT, PAGE_READWRITE) == base);
    CHECK_EQ_INT(base[0], 0);
    CHECK(VirtualFree(base, 0, MEM_RELEASE));
    CHECK(!VirtualFree(base, 0, MEM_RELEASE));

    char *fixed = VirtualAlloc((void *)0x300000000ULL, 65536, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    CHECK(fixed == (char *)0x300000000ULL);
    strcpy(fixed, "fixed address");
    CHECK(VirtualFree(fixed, 0, MEM_RELEASE));

    /* executable memory: write a tiny function and call it */
    unsigned char *code = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    code[0] = 0x8d; code[1] = 0x04; code[2] = 0x11; /* lea eax,[rcx+rdx] */
    code[3] = 0xc3;
    CHECK(VirtualProtect(code, 4096, PAGE_EXECUTE_READ, &old));
    int (*add)(int, int) = (int (*)(int, int))code;
    CHECK_EQ_INT(add(40, 2), 42);
    VirtualFree(code, 0, MEM_RELEASE);

    /* heaps */
    HANDLE heap = GetProcessHeap();
    CHECK(heap != NULL);
    char *p = HeapAlloc(heap, HEAP_ZERO_MEMORY, 100);
    CHECK(p != NULL);
    for (int i = 0; i < 100; i++) CHECK_EQ_INT(p[i], 0);
    strcpy(p, "heap");
    p = HeapReAlloc(heap, HEAP_ZERO_MEMORY, p, 5000);
    CHECK_STR(p, "heap");
    CHECK(HeapSize(heap, 0, p) >= 5000);
    CHECK(HeapFree(heap, 0, p));
    HANDLE h2 = HeapCreate(0, 0, 0);
    CHECK(h2 != NULL);
    void *q = HeapAlloc(h2, 0, 64);
    CHECK(q != NULL);
    HeapFree(h2, 0, q);
    HeapDestroy(h2);

    char *g = GlobalAlloc(GPTR, 256);
    CHECK(g != NULL && g[100] == 0);
    GlobalFree(g);
    char *l = LocalAlloc(LMEM_ZEROINIT, 128);
    CHECK(l != NULL && l[127] == 0);
    LocalFree(l);

    char *m = malloc(1000);
    memset(m, 1, 1000);
    m = realloc(m, 100000);
    CHECK_EQ_INT(m[999], 1);
    free(m);
    int *c = calloc(1000, sizeof(int));
    CHECK_EQ_INT(c[999], 0);
    free(c);
    void *al = _aligned_malloc(1000, 64);
    CHECK_EQ_INT((uintptr_t)al % 64, 0);
    _aligned_free(al);
    char *a = _alloca(1000);
    memset(a, 7, 1000);
    CHECK_EQ_INT(a[500], 7);

    /* file mapping shared between views and threads */
    HANDLE map = CreateFileMappingA(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, 8192, "nle_map_test");
    CHECK(map != NULL);
    char *v1 = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, 0);
    CHECK(v1 != NULL);
    strcpy(v1, "SHARED");
    HANDLE map2 = OpenFileMappingA(FILE_MAP_READ, FALSE, "nle_map_test");
    CHECK(map2 != NULL);
    char *v2 = MapViewOfFile(map2, FILE_MAP_READ, 0, 0, 0);
    CHECK(v2 != NULL && v2 != v1);
    CHECK_STR(v2, "SHARED");
    HANDLE t = CreateThread(NULL, 0, reader, v2, 0, NULL);
    WaitForSingleObject(t, 5000);
    DWORD code2 = 9;
    GetExitCodeThread(t, &code2);
    CHECK_EQ_INT(code2, 0);
    CloseHandle(t);
    CHECK(UnmapViewOfFile(v1));
    CHECK(UnmapViewOfFile(v2));
    CloseHandle(map);
    CloseHandle(map2);

    MEMORYSTATUSEX ms = { sizeof ms };
    CHECK(GlobalMemoryStatusEx(&ms));
    CHECK(ms.ullTotalPhys > (1ULL << 24));
    return finish();
}
