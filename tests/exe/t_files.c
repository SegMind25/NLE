#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <io.h>
#include <fcntl.h>
#include "check.h"

static void cleanup(const char *dir) {
    char pattern[MAX_PATH];
    snprintf(pattern, sizeof pattern, "%s\\*", dir);
    WIN32_FIND_DATAA fd;
    HANDLE h = FindFirstFileA(pattern, &fd);
    if (h != INVALID_HANDLE_VALUE) {
        do {
            if (strcmp(fd.cFileName, ".") && strcmp(fd.cFileName, "..")) {
                char p[MAX_PATH];
                snprintf(p, sizeof p, "%s\\%s", dir, fd.cFileName);
                if (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) {
                    cleanup(p);
                    RemoveDirectoryA(p);
                } else {
                    DeleteFileA(p);
                }
            }
        } while (FindNextFileA(h, &fd));
        FindClose(h);
    }
}

int main(void) {
    char temp[MAX_PATH], dir[MAX_PATH], path[MAX_PATH], path2[MAX_PATH];
    CHECK(GetTempPathA(sizeof temp, temp) > 0);
    snprintf(dir, sizeof dir, "%snle_t_files_%lu", temp, GetCurrentProcessId());
    cleanup(dir);
    RemoveDirectoryA(dir);
    CHECK(CreateDirectoryA(dir, NULL));
    CHECK(!CreateDirectoryA(dir, NULL));
    CHECK_EQ_INT(GetLastError(), ERROR_ALREADY_EXISTS);
    DWORD attrs = GetFileAttributesA(dir);
    CHECK(attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_DIRECTORY));

    snprintf(path, sizeof path, "%s\\Hello.txt", dir);
    HANDLE h = CreateFileA(path, GENERIC_WRITE, 0, NULL, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, NULL);
    CHECK(h != INVALID_HANDLE_VALUE);
    DWORD n = 0;
    CHECK(WriteFile(h, "hello, world\n", 13, &n, NULL));
    CHECK_EQ_INT(n, 13);
    CloseHandle(h);
    h = CreateFileA(path, GENERIC_WRITE, 0, NULL, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, NULL);
    CHECK(h == INVALID_HANDLE_VALUE);
    CHECK_EQ_INT(GetLastError(), ERROR_FILE_EXISTS);

    char lower[MAX_PATH];
    snprintf(lower, sizeof lower, "%s\\hello.TXT", dir);
    h = CreateFileA(lower, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
    CHECK(h != INVALID_HANDLE_VALUE);
    char buf[64] = {0};
    CHECK(ReadFile(h, buf, sizeof buf, &n, NULL));
    CHECK_EQ_INT(n, 13);
    CHECK_STR(buf, "hello, world\n");
    CHECK(ReadFile(h, buf, sizeof buf, &n, NULL));
    CHECK_EQ_INT(n, 0);
    LARGE_INTEGER sz;
    CHECK(GetFileSizeEx(h, &sz));
    CHECK_EQ_INT(sz.QuadPart, 13);
    CHECK_EQ_INT(SetFilePointer(h, 7, NULL, FILE_BEGIN), 7);
    memset(buf, 0, sizeof buf);
    ReadFile(h, buf, 5, &n, NULL);
    CHECK_STR(buf, "world");
    LARGE_INTEGER zero = {0}, pos;
    CHECK(SetFilePointerEx(h, zero, &pos, FILE_CURRENT));
    CHECK_EQ_INT(pos.QuadPart, 12);
    CHECK_EQ_INT(GetFileType(h), FILE_TYPE_DISK);
    CloseHandle(h);

    h = CreateFileA(path, GENERIC_READ | GENERIC_WRITE, 0, NULL, OPEN_EXISTING, 0, NULL);
    SetFilePointer(h, 0, NULL, FILE_END);
    WriteFile(h, "more", 4, &n, NULL);
    SetFilePointer(h, 5, NULL, FILE_BEGIN);
    CHECK(SetEndOfFile(h));
    GetFileSizeEx(h, &sz);
    CHECK_EQ_INT(sz.QuadPart, 5);
    CloseHandle(h);

    h = CreateFileA(path, GENERIC_READ, 0, NULL, OPEN_EXISTING, 0, NULL);
    BY_HANDLE_FILE_INFORMATION info;
    CHECK(GetFileInformationByHandle(h, &info));
    CHECK_EQ_INT(info.nFileSizeLow, 5);
    CloseHandle(h);

    snprintf(path2, sizeof path2, "%s\\copy.txt", dir);
    CHECK(CopyFileA(path, path2, TRUE));
    CHECK(!CopyFileA(path, path2, TRUE));
    char path3[MAX_PATH];
    snprintf(path3, sizeof path3, "%s\\moved.txt", dir);
    CHECK(MoveFileA(path2, path3));
    CHECK_EQ_INT(GetFileAttributesA(path2), INVALID_FILE_ATTRIBUTES);
    CHECK_EQ_INT(GetLastError(), ERROR_FILE_NOT_FOUND);
    CHECK(GetFileAttributesA(path3) != INVALID_FILE_ATTRIBUTES);

    char sub[MAX_PATH];
    snprintf(sub, sizeof sub, "%s\\sub", dir);
    CHECK(CreateDirectoryA(sub, NULL));
    snprintf(path2, sizeof path2, "%s\\sub\\inner.dat", dir);
    h = CreateFileA(path2, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
    CHECK(h != INVALID_HANDLE_VALUE);
    CloseHandle(h);

    int count = 0, dirs = 0;
    snprintf(path, sizeof path, "%s\\*", dir);
    WIN32_FIND_DATAA fd;
    h = FindFirstFileA(path, &fd);
    CHECK(h != INVALID_HANDLE_VALUE);
    do {
        if (!strcmp(fd.cFileName, ".") || !strcmp(fd.cFileName, "..")) continue;
        count++;
        if (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) dirs++;
    } while (FindNextFileA(h, &fd));
    CHECK_EQ_INT(GetLastError(), ERROR_NO_MORE_FILES);
    FindClose(h);
    CHECK_EQ_INT(count, 3);
    CHECK_EQ_INT(dirs, 1);

    snprintf(path, sizeof path, "%s\\*.txt", dir);
    count = 0;
    h = FindFirstFileA(path, &fd);
    do count++; while (FindNextFileA(h, &fd));
    FindClose(h);
    CHECK_EQ_INT(count, 2);
    snprintf(path, sizeof path, "%s\\*.nothing", dir);
    h = FindFirstFileA(path, &fd);
    CHECK(h == INVALID_HANDLE_VALUE);
    CHECK_EQ_INT(GetLastError(), ERROR_FILE_NOT_FOUND);

    /* C runtime file I/O */
    snprintf(path, sizeof path, "%s\\crt.txt", dir);
    FILE *f = fopen(path, "w");
    CHECK(f != NULL);
    fprintf(f, "line1\nline2 %d %s\n", 42, "x");
    fputs("line3\n", f);
    fclose(f);
    f = fopen(path, "r");
    char line[128];
    CHECK(fgets(line, sizeof line, f) != NULL);
    CHECK_STR(line, "line1\n");
    int num = 0;
    char word[16];
    CHECK_EQ_INT(fscanf(f, "line2 %d %15s", &num, word), 2);
    CHECK_EQ_INT(num, 42);
    CHECK_STR(word, "x");
    fseek(f, 0, SEEK_END);
    CHECK_EQ_INT(ftell(f), 6 + 11 + 6 - 0);
    CHECK(feof(f) == 0);
    fclose(f);
    CHECK(fopen("C:\\definitely\\missing\\file.txt", "r") == NULL);

    struct _stat64 st;
    CHECK_EQ_INT(_stat64(path, &st), 0);
    CHECK_EQ_INT(st.st_size, 23);
    CHECK(st.st_mode & _S_IFREG);
    CHECK_EQ_INT(_access(path, 0), 0);
    CHECK(remove(path) == 0);
    CHECK_EQ_INT(_access(path, 0), -1);

    int fd2 = _open(path, _O_WRONLY | _O_CREAT | _O_TRUNC | _O_BINARY, 0666);
    CHECK(fd2 >= 0);
    CHECK_EQ_INT(_write(fd2, "abc", 3), 3);
    _close(fd2);
    fd2 = _open(path, _O_RDONLY | _O_BINARY);
    char b3[8] = {0};
    CHECK_EQ_INT(_read(fd2, b3, 8), 3);
    CHECK_STR(b3, "abc");
    _close(fd2);

    /* path APIs */
    char full[MAX_PATH], *part = NULL;
    DWORD len = GetFullPathNameA("a\\..\\b\\c.txt", sizeof full, full, &part);
    CHECK(len > 0);
    CHECK(part != NULL && strcmp(part, "c.txt") == 0);
    CHECK(strstr(full, "\\b\\c.txt") != NULL);
    char cwd[MAX_PATH];
    CHECK(GetCurrentDirectoryA(sizeof cwd, cwd) > 0);
    CHECK(cwd[1] == ':');
    CHECK(SetCurrentDirectoryA(dir));
    char cwd2[MAX_PATH];
    GetCurrentDirectoryA(sizeof cwd2, cwd2);
    CHECK(strstr(cwd2, "nle_t_files") != NULL);
    CHECK(GetFileAttributesA("SUB") != INVALID_FILE_ATTRIBUTES);
    CHECK(SetCurrentDirectoryA(cwd));

    cleanup(dir);
    CHECK(RemoveDirectoryA(dir));
    CHECK_EQ_INT(GetFileAttributesA(dir), INVALID_FILE_ATTRIBUTES);
    return finish();
}
