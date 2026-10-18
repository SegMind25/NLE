#include <windows.h>
#include <stdio.h>
#include <string.h>
#include "check.h"

int main(void) {
    HANDLE in = GetStdHandle(STD_INPUT_HANDLE);
    HANDLE out = GetStdHandle(STD_OUTPUT_HANDLE);
    HANDLE err = GetStdHandle(STD_ERROR_HANDLE);
    CHECK(in != NULL && in != INVALID_HANDLE_VALUE);
    CHECK(out != NULL && out != INVALID_HANDLE_VALUE);
    CHECK(err != NULL && err != INVALID_HANDLE_VALUE);
    DWORD mode;
    CHECK(!GetConsoleMode(out, &mode));
    CHECK(!GetConsoleMode(in, &mode));
    DWORD ty = GetFileType(out);
    CHECK(ty == FILE_TYPE_PIPE || ty == FILE_TYPE_DISK);
    CHECK_EQ_INT(GetConsoleOutputCP(), 65001);
    CONSOLE_SCREEN_BUFFER_INFO info;
    CHECK(!GetConsoleScreenBufferInfo(out, &info));

    char line[64] = {0};
    DWORD n = 0;
    CHECK(ReadFile(in, line, 6, &n, NULL));
    CHECK_EQ_INT(n, 6);
    CHECK(memcmp(line, "hello\n", 6) == 0);
    char l2[64];
    CHECK(fgets(l2, sizeof l2, stdin) != NULL);
    CHECK_STR(l2, "world\n");
    int a = 0, b = 0;
    CHECK_EQ_INT(scanf("%d %d", &a, &b), 2);
    CHECK_EQ_INT(a + b, 30);
    CHECK_EQ_INT(getchar(), '\n');
    CHECK(fgets(l2, sizeof l2, stdin) == NULL);
    CHECK(feof(stdin));

    const char msg[] = "direct WriteFile\n";
    CHECK(WriteFile(out, msg, sizeof msg - 1, &n, NULL));
    CHECK_EQ_INT(n, sizeof msg - 1);
    CHECK(WriteFile(err, "to stderr\n", 10, &n, NULL));
    printf("printf line\n");
    fprintf(stderr, "fprintf stderr\n");
    puts("puts line");
    fflush(stdout);
    SetConsoleTextAttribute(out, 12);
    return finish();
}
