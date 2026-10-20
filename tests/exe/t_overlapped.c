/* Overlapped I/O: pipes and sockets that finish later and report to an I/O completion port, AcceptEx / ConnectEx,
 * thread-pool waits, and the NT directory query libuv-style event loops use. */
#include <winsock2.h>
#include <ws2tcpip.h>
#include <mswsock.h>
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include "check.h"

typedef struct { ULONG_PTR status; ULONG_PTR information; } IOSB;
typedef LONG (NTAPI *NtQueryDirectoryFile_t)(HANDLE, HANDLE, PVOID, PVOID, IOSB *, PVOID, ULONG, ULONG, BOOLEAN, PVOID, BOOLEAN);

static HANDLE g_port;

/* the next packet, or 0 when none arrives in time */
static OVERLAPPED *next_packet(DWORD ms, DWORD *bytes, ULONG_PTR *key, DWORD *error) {
    OVERLAPPED *ov = NULL;
    BOOL ok = GetQueuedCompletionStatus(g_port, bytes, key, &ov, ms);
    *error = ok ? 0 : GetLastError();
    return ov;
}

static void test_pipes(void) {
    HANDLE server = CreateNamedPipeW(L"\\\\.\\pipe\\nle-test-overlapped", PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                                     PIPE_TYPE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, NULL);
    CHECK(server != INVALID_HANDLE_VALUE);
    HANDLE client = CreateFileW(L"\\\\.\\pipe\\nle-test-overlapped", GENERIC_READ | GENERIC_WRITE, 0, NULL, OPEN_EXISTING,
                                FILE_FLAG_OVERLAPPED, NULL);
    CHECK(client != INVALID_HANDLE_VALUE);
    CHECK(CreateIoCompletionPort(server, g_port, 7, 0) == g_port);

    /* a zero-byte read waits for data */
    OVERLAPPED rd;
    memset(&rd, 0, sizeof rd);
    char zero;
    BOOL ok = ReadFile(server, &zero, 0, NULL, &rd);
    CHECK(!ok && GetLastError() == ERROR_IO_PENDING);
    CHECK(!HasOverlappedIoCompleted(&rd));
    DWORD bytes, error;
    ULONG_PTR key;
    CHECK(next_packet(100, &bytes, &key, &error) == NULL && error == WAIT_TIMEOUT);

    /* data from the other end completes it */
    DWORD written = 0;
    OVERLAPPED wr;
    memset(&wr, 0, sizeof wr);
    ok = WriteFile(client, "hello", 5, &written, &wr);
    CHECK(ok || GetLastError() == ERROR_IO_PENDING);
    OVERLAPPED *got = next_packet(2000, &bytes, &key, &error);
    CHECK(got == &rd && key == 7 && error == 0);
    OVERLAPPED rd2;
    memset(&rd2, 0, sizeof rd2);
    char buf[16] = {0};
    DWORD n = 0;
    ok = ReadFile(server, buf, sizeof buf, &n, &rd2);
    if (!ok && GetLastError() == ERROR_IO_PENDING) ok = GetOverlappedResult(server, &rd2, &n, TRUE);
    CHECK(ok && n == 5 && memcmp(buf, "hello", 5) == 0);
    /* the packet of that read is on the port too */
    got = next_packet(1000, &bytes, &key, &error);
    CHECK(got == &rd2 && bytes == 5);

    /* a pending read can be cancelled; it then reports ERROR_OPERATION_ABORTED */
    OVERLAPPED rd3;
    memset(&rd3, 0, sizeof rd3);
    ok = ReadFile(server, buf, sizeof buf, NULL, &rd3);
    CHECK(!ok && GetLastError() == ERROR_IO_PENDING);
    CHECK(CancelIoEx(server, &rd3));
    got = next_packet(1000, &bytes, &key, &error);
    CHECK(got == &rd3 && error == ERROR_OPERATION_ABORTED);

    /* the other end going away ends the next read at once */
    CloseHandle(client);
    OVERLAPPED rd4;
    memset(&rd4, 0, sizeof rd4);
    ok = ReadFile(server, buf, sizeof buf, &n, &rd4);
    CHECK(!ok && GetLastError() == ERROR_BROKEN_PIPE);
    CloseHandle(server);
}

static void test_sockets(void) {
    WSADATA wsa;
    CHECK_EQ_INT(WSAStartup(MAKEWORD(2, 2), &wsa), 0);
    LPFN_ACCEPTEX accept_ex = NULL;
    LPFN_CONNECTEX connect_ex = NULL;
    LPFN_GETACCEPTEXSOCKADDRS get_addrs = NULL;
    GUID g1 = WSAID_ACCEPTEX, g2 = WSAID_CONNECTEX, g3 = WSAID_GETACCEPTEXSOCKADDRS;
    SOCKET listener = WSASocketW(AF_INET, SOCK_STREAM, IPPROTO_TCP, NULL, 0, WSA_FLAG_OVERLAPPED);
    DWORD ret;
    CHECK_EQ_INT(WSAIoctl(listener, SIO_GET_EXTENSION_FUNCTION_POINTER, &g1, sizeof g1, &accept_ex, sizeof accept_ex, &ret, NULL, NULL), 0);
    CHECK_EQ_INT(WSAIoctl(listener, SIO_GET_EXTENSION_FUNCTION_POINTER, &g2, sizeof g2, &connect_ex, sizeof connect_ex, &ret, NULL, NULL), 0);
    CHECK_EQ_INT(WSAIoctl(listener, SIO_GET_EXTENSION_FUNCTION_POINTER, &g3, sizeof g3, &get_addrs, sizeof get_addrs, &ret, NULL, NULL), 0);
    CHECK(accept_ex && connect_ex && get_addrs);
    if (!accept_ex || !connect_ex || !get_addrs) return;

    struct sockaddr_in any = {0};
    any.sin_family = AF_INET;
    any.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    CHECK_EQ_INT(bind(listener, (struct sockaddr *)&any, sizeof any), 0);
    CHECK_EQ_INT(listen(listener, 4), 0);
    int len = sizeof any;
    CHECK_EQ_INT(getsockname(listener, (struct sockaddr *)&any, &len), 0);
    CHECK(CreateIoCompletionPort((HANDLE)listener, g_port, 100, 0) == g_port);

    SOCKET accepted = WSASocketW(AF_INET, SOCK_STREAM, IPPROTO_TCP, NULL, 0, WSA_FLAG_OVERLAPPED);
    char abuf[2 * (sizeof(struct sockaddr_in) + 16)];
    OVERLAPPED aov;
    memset(&aov, 0, sizeof aov);
    BOOL ok = accept_ex(listener, accepted, abuf, 0, sizeof(struct sockaddr_in) + 16, sizeof(struct sockaddr_in) + 16, &ret, &aov);
    CHECK(!ok && WSAGetLastError() == ERROR_IO_PENDING);

    SOCKET client = WSASocketW(AF_INET, SOCK_STREAM, IPPROTO_TCP, NULL, 0, WSA_FLAG_OVERLAPPED);
    struct sockaddr_in local = {0};
    local.sin_family = AF_INET;
    CHECK_EQ_INT(bind(client, (struct sockaddr *)&local, sizeof local), 0);
    CHECK(CreateIoCompletionPort((HANDLE)client, g_port, 200, 0) == g_port);
    OVERLAPPED cov;
    memset(&cov, 0, sizeof cov);
    ok = connect_ex(client, (struct sockaddr *)&any, sizeof any, NULL, 0, NULL, &cov);
    CHECK(!ok && WSAGetLastError() == ERROR_IO_PENDING);

    /* both finish: the accept on the listener's key, the connect on the client's */
    int seen_accept = 0, seen_connect = 0;
    for (int i = 0; i < 2; i++) {
        DWORD bytes, error;
        ULONG_PTR key;
        OVERLAPPED *ov = next_packet(3000, &bytes, &key, &error);
        if (ov == &aov && key == 100 && error == 0) seen_accept = 1;
        if (ov == &cov && key == 200 && error == 0) seen_connect = 1;
    }
    CHECK(seen_accept && seen_connect);
    CHECK_EQ_INT(setsockopt(accepted, SOL_SOCKET, SO_UPDATE_ACCEPT_CONTEXT, (char *)&listener, sizeof listener), 0);
    struct sockaddr *l = NULL, *r = NULL;
    int ll = 0, rl = 0;
    get_addrs(abuf, 0, sizeof(struct sockaddr_in) + 16, sizeof(struct sockaddr_in) + 16, &l, &ll, &r, &rl);
    CHECK(l && r && ll == sizeof(struct sockaddr_in) && rl == sizeof(struct sockaddr_in));
    CHECK(r && ((struct sockaddr_in *)r)->sin_addr.s_addr == htonl(INADDR_LOOPBACK));

    /* an overlapped receive waits for the peer's send */
    CHECK(CreateIoCompletionPort((HANDLE)accepted, g_port, 300, 0) == g_port);
    char data[32] = {0};
    WSABUF wb = {sizeof data, data};
    DWORD flags = 0, got = 0;
    OVERLAPPED rov;
    memset(&rov, 0, sizeof rov);
    int rc = WSARecv(accepted, &wb, 1, &got, &flags, &rov, NULL);
    CHECK(rc == SOCKET_ERROR && WSAGetLastError() == WSA_IO_PENDING);
    CHECK_EQ_INT(send(client, "ping", 4, 0), 4);
    DWORD bytes, error;
    ULONG_PTR key;
    OVERLAPPED *ov = next_packet(3000, &bytes, &key, &error);
    CHECK(ov == &rov && key == 300 && bytes == 4 && error == 0);
    DWORD xfer = 0;
    CHECK(WSAGetOverlappedResult(accepted, &rov, &xfer, FALSE, &flags) && xfer == 4 && memcmp(data, "ping", 4) == 0);

    /* an overlapped send finishes at once and still reports */
    WSABUF sb = {4, "pong"};
    OVERLAPPED sov;
    memset(&sov, 0, sizeof sov);
    rc = WSASend(accepted, &sb, 1, &got, 0, &sov, NULL);
    CHECK(rc == 0 || WSAGetLastError() == WSA_IO_PENDING);
    ov = next_packet(3000, &bytes, &key, &error);
    CHECK(ov == &sov && bytes == 4);
    char back[8] = {0};
    CHECK_EQ_INT(recv(client, back, sizeof back, 0), 4);
    CHECK(memcmp(back, "pong", 4) == 0);

    /* closing a socket completes what is pending on it as cancelled */
    OVERLAPPED cancel_ov;
    memset(&cancel_ov, 0, sizeof cancel_ov);
    rc = WSARecv(accepted, &wb, 1, &got, &flags, &cancel_ov, NULL);
    CHECK(rc == SOCKET_ERROR && WSAGetLastError() == WSA_IO_PENDING);
    closesocket(accepted);
    ov = next_packet(2000, &bytes, &key, &error);
    CHECK(ov == &cancel_ov && error == ERROR_OPERATION_ABORTED);
    closesocket(client);
    closesocket(listener);
}

static volatile LONG g_work_done;
static DWORD WINAPI work_item(LPVOID p) {
    InterlockedAdd(&g_work_done, (LONG)(INT_PTR)p);
    return 0;
}

static volatile LONG g_wait_calls, g_wait_timeouts;
static VOID CALLBACK wait_callback(PVOID ctx, BOOLEAN timed_out) {
    (void)ctx;
    InterlockedIncrement(&g_wait_calls);
    if (timed_out) InterlockedIncrement(&g_wait_timeouts);
}

static void test_thread_pool(void) {
    CHECK(QueueUserWorkItem(work_item, (PVOID)5, WT_EXECUTEDEFAULT));
    CHECK(QueueUserWorkItem(work_item, (PVOID)6, WT_EXECUTEDEFAULT));
    for (int i = 0; i < 100 && g_work_done != 11; i++) Sleep(20);
    CHECK_EQ_INT(g_work_done, 11);

    HANDLE ev = CreateEventW(NULL, FALSE, FALSE, NULL);
    HANDLE wait = NULL;
    CHECK(RegisterWaitForSingleObject(&wait, ev, wait_callback, NULL, INFINITE, WT_EXECUTEONLYONCE));
    SetEvent(ev);
    for (int i = 0; i < 100 && g_wait_calls < 1; i++) Sleep(20);
    CHECK_EQ_INT(g_wait_calls, 1);
    CHECK_EQ_INT(g_wait_timeouts, 0);
    CHECK(UnregisterWaitEx(wait, INVALID_HANDLE_VALUE));

    /* a wait that times out calls back with timed_out set, repeatedly until it is unregistered */
    g_wait_calls = 0;
    HANDLE never = CreateEventW(NULL, TRUE, FALSE, NULL);
    CHECK(RegisterWaitForSingleObject(&wait, never, wait_callback, NULL, 30, WT_EXECUTEDEFAULT));
    Sleep(250);
    CHECK(UnregisterWaitEx(wait, INVALID_HANDLE_VALUE));
    CHECK(g_wait_calls >= 2 && g_wait_timeouts == g_wait_calls);
    CloseHandle(ev);
    CloseHandle(never);
}

static void test_directory_query(void) {
    CreateDirectoryW(L"nle_ntdir", NULL);
    HANDLE f = CreateFileW(L"nle_ntdir\\alpha.txt", GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    DWORD w;
    WriteFile(f, "12345", 5, &w, NULL);
    CloseHandle(f);
    f = CreateFileW(L"nle_ntdir\\beta.dat", GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    CloseHandle(f);
    HANDLE dir = CreateFileW(L"nle_ntdir", FILE_LIST_DIRECTORY | SYNCHRONIZE, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL,
                             OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, NULL);
    CHECK(dir != INVALID_HANDLE_VALUE);
    NtQueryDirectoryFile_t query = (NtQueryDirectoryFile_t)GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "NtQueryDirectoryFile");
    CHECK(query != NULL);
    if (!query || dir == INVALID_HANDLE_VALUE) return;
    char buf[4096];
    IOSB iosb;
    int found_alpha = 0, found_beta = 0, total = 0;
    LONG st = query(dir, NULL, NULL, NULL, &iosb, buf, sizeof buf, 1 /* FileDirectoryInformation */, FALSE, NULL, TRUE);
    CHECK_EQ_INT(st, 0);
    for (char *p = buf;;) {
        ULONG next = *(ULONG *)p;
        ULONG name_len = *(ULONG *)(p + 60);
        WCHAR *name = (WCHAR *)(p + 64);
        total++;
        if (name_len == 18 && memcmp(name, L"alpha.txt", 18) == 0) {
            found_alpha = 1;
            CHECK_EQ_INT(*(LONGLONG *)(p + 40), 5);
        }
        if (name_len == 16 && memcmp(name, L"beta.dat", 16) == 0) found_beta = 1;
        if (!next) break;
        p += next;
    }
    CHECK(found_alpha && found_beta);
    CHECK_EQ_INT(total, 4); /* "." and ".." as well */
    st = query(dir, NULL, NULL, NULL, &iosb, buf, sizeof buf, 1, FALSE, NULL, FALSE);
    CHECK_EQ_INT((unsigned)st, 0x80000006u); /* STATUS_NO_MORE_FILES */
    CloseHandle(dir);
    DeleteFileW(L"nle_ntdir\\alpha.txt");
    DeleteFileW(L"nle_ntdir\\beta.dat");
    RemoveDirectoryW(L"nle_ntdir");
}

int main(void) {
    g_port = CreateIoCompletionPort(INVALID_HANDLE_VALUE, NULL, 0, 0);
    CHECK(g_port != NULL);
    test_pipes();
    test_sockets();
    test_thread_pool();
    test_directory_query();
    CloseHandle(g_port);
    return finish();
}
