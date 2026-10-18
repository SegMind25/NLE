#include <winsock2.h>
#include <ws2tcpip.h>
#include <stdio.h>
#include <string.h>
#include "check.h"

static SOCKET g_listener;

static DWORD WINAPI server_thread(LPVOID p) {
    (void)p;
    struct sockaddr_in peer;
    int plen = sizeof peer;
    SOCKET c = accept(g_listener, (struct sockaddr *)&peer, &plen);
    if (c == INVALID_SOCKET) return 1;
    char buf[16] = {0};
    int n = recv(c, buf, 5, 0);
    if (n != 5 || memcmp(buf, "PING!", 5) != 0) return 2;
    if (send(c, "PONG!", 5, 0) != 5) return 3;
    closesocket(c);
    return 0;
}

int main(void) {
    WSADATA wsa;
    CHECK_EQ_INT(WSAStartup(MAKEWORD(2, 2), &wsa), 0);
    CHECK_EQ_INT(LOBYTE(wsa.wVersion), 2);

    g_listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    CHECK(g_listener != INVALID_SOCKET);
    int one = 1;
    CHECK_EQ_INT(setsockopt(g_listener, SOL_SOCKET, SO_REUSEADDR, (char *)&one, sizeof one), 0);
    struct sockaddr_in addr = {0};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = inet_addr("127.0.0.1");
    addr.sin_port = 0;
    CHECK_EQ_INT(bind(g_listener, (struct sockaddr *)&addr, sizeof addr), 0);
    CHECK_EQ_INT(listen(g_listener, 4), 0);
    int alen = sizeof addr;
    CHECK_EQ_INT(getsockname(g_listener, (struct sockaddr *)&addr, &alen), 0);
    CHECK_EQ_INT(addr.sin_family, AF_INET);
    CHECK(ntohs(addr.sin_port) > 0);
    int type = 0, tl = sizeof type;
    CHECK_EQ_INT(getsockopt(g_listener, SOL_SOCKET, SO_TYPE, (char *)&type, &tl), 0);
    CHECK_EQ_INT(type, SOCK_STREAM);

    HANDLE t = CreateThread(NULL, 0, server_thread, NULL, 0, NULL);
    SOCKET c = socket(AF_INET, SOCK_STREAM, 0);
    CHECK_EQ_INT(connect(c, (struct sockaddr *)&addr, sizeof addr), 0);
    CHECK_EQ_INT(send(c, "PING!", 5, 0), 5);
    fd_set rd;
    FD_ZERO(&rd);
    FD_SET(c, &rd);
    struct timeval tv = { 5, 0 };
    CHECK_EQ_INT(select(0, &rd, NULL, NULL, &tv), 1);
    CHECK(FD_ISSET(c, &rd));
    char buf[16] = {0};
    CHECK_EQ_INT(recv(c, buf, sizeof buf, 0), 5);
    CHECK_STR(buf, "PONG!");
    CHECK_EQ_INT(recv(c, buf, sizeof buf, 0), 0);
    WaitForSingleObject(t, 5000);
    DWORD code = 9;
    GetExitCodeThread(t, &code);
    CHECK_EQ_INT(code, 0);
    CloseHandle(t);
    closesocket(c);
    closesocket(g_listener);

    SOCKET c2 = socket(AF_INET, SOCK_STREAM, 0);
    u_long nb = 1;
    CHECK_EQ_INT(ioctlsocket(c2, FIONBIO, &nb), 0);
    CHECK_EQ_INT(connect(c2, (struct sockaddr *)&addr, sizeof addr), SOCKET_ERROR);
    int err = WSAGetLastError();
    CHECK(err == WSAECONNREFUSED || err == WSAEWOULDBLOCK);
    closesocket(c2);

    SOCKET u = socket(AF_INET, SOCK_DGRAM, 0);
    struct sockaddr_in ua = {0};
    ua.sin_family = AF_INET;
    ua.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    bind(u, (struct sockaddr *)&ua, sizeof ua);
    int ul = sizeof ua;
    getsockname(u, (struct sockaddr *)&ua, &ul);
    SOCKET u2 = socket(AF_INET, SOCK_DGRAM, 0);
    CHECK_EQ_INT(sendto(u2, "dgram", 5, 0, (struct sockaddr *)&ua, sizeof ua), 5);
    char db[16] = {0};
    struct sockaddr_in from;
    int fl = sizeof from;
    CHECK_EQ_INT(recvfrom(u, db, sizeof db, 0, (struct sockaddr *)&from, &fl), 5);
    CHECK_STR(db, "dgram");
    CHECK_EQ_INT(from.sin_addr.s_addr, htonl(INADDR_LOOPBACK));
    WSABUF wb = { 3, db };
    DWORD sent = 0;
    CHECK_EQ_INT(WSASendTo ? 0 : 1, 0);
    (void)wb; (void)sent;
    closesocket(u);
    closesocket(u2);

    struct addrinfo hints = {0}, *res = NULL;
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    CHECK_EQ_INT(getaddrinfo("localhost", "8080", &hints, &res), 0);
    CHECK(res != NULL);
    if (res) {
        CHECK_EQ_INT(res->ai_family, AF_INET);
        CHECK_EQ_INT(((struct sockaddr_in *)res->ai_addr)->sin_port, htons(8080));
        freeaddrinfo(res);
    }
    CHECK(getaddrinfo("no-such-host.invalid", NULL, &hints, &res) != 0);
    char host[64];
    CHECK_EQ_INT(gethostname(host, sizeof host), 0);
    CHECK(strlen(host) > 0);
    CHECK_STR(inet_ntoa(*(struct in_addr *)&(unsigned long){ htonl(0x7f000001) }), "127.0.0.1");
    WSACleanup();
    return finish();
}
