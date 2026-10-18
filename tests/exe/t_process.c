#include <windows.h>
#include <tlhelp32.h>
#include <stdio.h>
#include <string.h>
#include "check.h"

static int child_main(int argc, char **argv) {
    printf("child argc=%d arg1=[%s] env=[%s]\n", argc, argc > 1 ? argv[1] : "", getenv("T_ENV_VAR") ? getenv("T_ENV_VAR") : "unset");
    fflush(stdout);
    char line[64] = {0};
    if (argc > 2 && !strcmp(argv[2], "stdin")) {
        if (fgets(line, sizeof line, stdin)) printf("child read [%s]", line);
        fflush(stdout);
    }
    return 42;
}

int main(int argc, char **argv) {
    if (argc > 1 && !strcmp(argv[1], "child")) return child_main(argc, argv);
    if (argc > 1 && !strcmp(argv[1], "spaced")) { printf("spaced argv1=[%s]\n", argc > 2 ? argv[2] : ""); return 3; }

    char self[MAX_PATH];
    CHECK(GetModuleFileNameA(NULL, self, sizeof self) > 0);

    /* plain run + exit code */
    char cmd[MAX_PATH * 2];
    snprintf(cmd, sizeof cmd, "\"%s\" child \"two words\"", self);
    STARTUPINFOA si = {sizeof si};
    PROCESS_INFORMATION pi;
    CHECK(CreateProcessA(NULL, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi));
    CHECK(pi.hProcess != NULL && pi.dwProcessId != 0);
    CHECK_EQ_INT(WaitForSingleObject(pi.hProcess, 30000), WAIT_OBJECT_0);
    DWORD code = 0;
    CHECK(GetExitCodeProcess(pi.hProcess, &code));
    CHECK_EQ_INT(code, 42);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);

    /* captured output through pipes, stdin fed from the parent */
    HANDLE rd_out, wr_out, rd_in, wr_in;
    SECURITY_ATTRIBUTES sa = {sizeof sa, NULL, TRUE};
    CHECK(CreatePipe(&rd_out, &wr_out, &sa, 0));
    CHECK(CreatePipe(&rd_in, &wr_in, &sa, 0));
    SetEnvironmentVariableA("T_ENV_VAR", "from-parent");
    STARTUPINFOA si2 = {sizeof si2};
    si2.dwFlags = STARTF_USESTDHANDLES;
    si2.hStdOutput = wr_out;
    si2.hStdError = wr_out;
    si2.hStdInput = rd_in;
    snprintf(cmd, sizeof cmd, "\"%s\" child stdin", self);
    CHECK(CreateProcessA(self, cmd, NULL, NULL, TRUE, 0, NULL, NULL, &si2, &pi));
    CloseHandle(wr_out);
    CloseHandle(rd_in);
    DWORD n;
    CHECK(WriteFile(wr_in, "ping\n", 5, &n, NULL));
    CloseHandle(wr_in);
    char out[512] = {0};
    size_t used = 0;
    while (used < sizeof out - 1 && ReadFile(rd_out, out + used, sizeof out - 1 - used, &n, NULL) && n) used += n;
    CHECK(strstr(out, "child argc=3 arg1=[child] env=[from-parent]") != NULL);
    CHECK(strstr(out, "child read [ping\n]") != NULL);
    WaitForSingleObject(pi.hProcess, 30000);
    GetExitCodeProcess(pi.hProcess, &code);
    CHECK_EQ_INT(code, 42);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    CloseHandle(rd_out);

    /* explicit environment block, quoting, working directory */
    char env[] = "T_ENV_VAR=block\0OTHER=1\0";
    STARTUPINFOA si3 = {sizeof si3};
    si3.dwFlags = STARTF_USESTDHANDLES;
    CHECK(CreatePipe(&rd_out, &wr_out, &sa, 0));
    si3.hStdOutput = wr_out;
    si3.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    si3.hStdError = GetStdHandle(STD_ERROR_HANDLE);
    snprintf(cmd, sizeof cmd, "\"%s\" spaced \"a \\\"quoted\\\" b\"", self);
    CHECK(CreateProcessA(NULL, cmd, NULL, NULL, TRUE, 0, env, NULL, &si3, &pi));
    CloseHandle(wr_out);
    memset(out, 0, sizeof out);
    used = 0;
    while (used < sizeof out - 1 && ReadFile(rd_out, out + used, sizeof out - 1 - used, &n, NULL) && n) used += n;
    CHECK(strstr(out, "spaced argv1=[a \"quoted\" b]") != NULL);
    WaitForSingleObject(pi.hProcess, 30000);
    GetExitCodeProcess(pi.hProcess, &code);
    CHECK_EQ_INT(code, 3);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    CloseHandle(rd_out);

    /* failures */
    STARTUPINFOA si4 = {sizeof si4};
    CHECK(!CreateProcessA("C:\\no\\such\\program.exe", NULL, NULL, NULL, FALSE, 0, NULL, NULL, &si4, &pi));
    CHECK_EQ_INT(GetLastError(), ERROR_FILE_NOT_FOUND);

    /* a process that is still running, then terminated */
    snprintf(cmd, sizeof cmd, "\"%s\" child stdin", self);
    CHECK(CreatePipe(&rd_in, &wr_in, &sa, 0));
    STARTUPINFOA si5 = {sizeof si5};
    si5.dwFlags = STARTF_USESTDHANDLES;
    si5.hStdInput = rd_in;
    si5.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
    si5.hStdError = GetStdHandle(STD_ERROR_HANDLE);
    CHECK(CreateProcessA(NULL, cmd, NULL, NULL, TRUE, 0, NULL, NULL, &si5, &pi));
    CHECK_EQ_INT(WaitForSingleObject(pi.hProcess, 200), WAIT_TIMEOUT);
    CHECK(GetExitCodeProcess(pi.hProcess, &code));
    CHECK_EQ_INT(code, STILL_ACTIVE);
    CHECK(TerminateProcess(pi.hProcess, 9));
    CHECK_EQ_INT(WaitForSingleObject(pi.hProcess, 30000), WAIT_OBJECT_0);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    CloseHandle(rd_in);
    CloseHandle(wr_in);

    /* toolhelp: we must see ourselves, and our main module */
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    CHECK(snap != INVALID_HANDLE_VALUE);
    PROCESSENTRY32 pe = {sizeof pe};
    int found = 0;
    if (Process32First(snap, &pe)) do { if (pe.th32ProcessID == GetCurrentProcessId()) found = 1; } while (Process32Next(snap, &pe));
    CHECK(found);
    CloseHandle(snap);
    snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, GetCurrentProcessId());
    MODULEENTRY32 me = {sizeof me};
    CHECK(Module32First(snap, &me));
    CHECK(me.modBaseAddr == (BYTE *)GetModuleHandleA(NULL));
    CloseHandle(snap);
    return finish();
}
