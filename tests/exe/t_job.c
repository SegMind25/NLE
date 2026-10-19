#include <windows.h>
#include <stdio.h>
#include <string.h>
#include "check.h"

/* Job objects (the pip / distlib script launchers use them), SearchPath and WaitForInputIdle. */
int main(void) {
    HANDLE job = CreateJobObjectA(NULL, NULL);
    CHECK(job != NULL);

    JOBOBJECT_EXTENDED_LIMIT_INFORMATION info;
    DWORD ret = 0;
    memset(&info, 0xAA, sizeof info);
    CHECK(QueryInformationJobObject(job, JobObjectExtendedLimitInformation, &info, sizeof info, &ret));
    CHECK_EQ_INT(ret, sizeof info);
    CHECK_EQ_INT(info.BasicLimitInformation.LimitFlags, 0);

    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    CHECK(SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info, sizeof info));
    memset(&info, 0, sizeof info);
    CHECK(QueryInformationJobObject(job, JobObjectExtendedLimitInformation, &info, sizeof info, &ret));
    CHECK_EQ_INT(info.BasicLimitInformation.LimitFlags, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE);

    CHECK(!SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info, 8));
    CHECK_EQ_INT(GetLastError(), ERROR_BAD_LENGTH);
    CHECK(!AssignProcessToJobObject((HANDLE)0x1234, GetCurrentProcess()));
    CHECK(AssignProcessToJobObject(job, GetCurrentProcess()));

    /* Closing the job kills the processes assigned to it when KILL_ON_JOB_CLOSE is set. */
    STARTUPINFOA si;
    PROCESS_INFORMATION pi;
    memset(&si, 0, sizeof si);
    si.cb = sizeof si;
    char cmd[] = "Z:\\bin\\sleep 30";
    CHECK(CreateProcessA(NULL, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi));
    BOOL in_job = FALSE;
    CHECK(AssignProcessToJobObject(job, pi.hProcess));
    CHECK(IsProcessInJob(pi.hProcess, job, &in_job));
    CHECK(in_job);
    CHECK(WaitForSingleObject(pi.hProcess, 200) == WAIT_TIMEOUT);
    CHECK(CloseHandle(job));
    CHECK(WaitForSingleObject(pi.hProcess, 5000) == WAIT_OBJECT_0);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);

    /* SearchPath: explicit directory, extension, the default order, and a miss. */
    char buf[MAX_PATH], *part = NULL;
    char dir[MAX_PATH];
    GetCurrentDirectoryA(sizeof dir, dir);
    HANDLE f = CreateFileA("nle_search.txt", GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
    CHECK(f != INVALID_HANDLE_VALUE);
    CloseHandle(f);
    DWORD n = SearchPathA(dir, "nle_search", ".txt", sizeof buf, buf, &part);
    CHECK(n > 0);
    CHECK(part != NULL && strcmp(part, "nle_search.txt") == 0);
    CHECK_EQ_INT(n, strlen(buf));
    CHECK(SearchPathA(NULL, "nle_search.txt", NULL, sizeof buf, buf, NULL) > 0);
    CHECK(SearchPathA(NULL, "no_such_file_anywhere.dll", NULL, sizeof buf, buf, NULL) == 0);
    CHECK_EQ_INT(GetLastError(), ERROR_FILE_NOT_FOUND);
    WCHAR wbuf[MAX_PATH];
    CHECK(SearchPathW(NULL, L"nle_search.txt", NULL, MAX_PATH, wbuf, NULL) > 0);
    DeleteFileA("nle_search.txt");

    CHECK_EQ_INT(WaitForInputIdle(GetCurrentProcess(), 10), 0);

    if (g_failures == 0) printf("ALL OK (%d checks)\n", g_checks);
    return g_failures;
}
