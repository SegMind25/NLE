/* A window owned by one thread, and a second thread that sends it messages. SendMessage from
 * another thread must wait for the owner to answer; PostMessage must not.
 * Prints ALL OK and exits 0 when everything behaved.
 */
#include <windows.h>
#include <stdio.h>

static HWND win;
static int seen_user, seen_post;
static int failures;

#define CHECK(c) do { if (!(c)) { failures++; printf("FAIL line %d: %s\n", __LINE__, #c); } } while (0)

static LRESULT CALLBACK proc(HWND h, UINT m, WPARAM w, LPARAM l) {
    switch (m) {
    case WM_USER + 1:
        seen_user++;
        return (LRESULT)(w + l);
    case WM_USER + 2:
        seen_post++;
        return 0;
    case WM_USER + 3:
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcA(h, m, w, l);
}

static DWORD WINAPI worker(void *arg) {
    (void)arg;
    LRESULT r = SendMessageA(win, WM_USER + 1, 40, 2);
    CHECK(r == 42);
    r = SendMessageA(win, WM_USER + 1, 1, 1);
    CHECK(r == 2);
    CHECK(PostMessageA(win, WM_USER + 2, 0, 0));
    CHECK(PostMessageA(win, WM_USER + 2, 0, 0));
    char buf[64] = "";
    SendMessageA(win, WM_SETTEXT, 0, (LPARAM)"from another thread");
    GetWindowTextA(win, buf, sizeof buf);
    CHECK(strcmp(buf, "from another thread") == 0);
    PostMessageA(win, WM_USER + 3, 0, 0);
    return 0;
}

int main(void) {
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = GetModuleHandleA(NULL);
    wc.lpszClassName = "ThreadTest";
    RegisterClassA(&wc);
    win = CreateWindowExA(0, "ThreadTest", "t", WS_OVERLAPPEDWINDOW, 0, 0, 200, 100, NULL, NULL, wc.hInstance, NULL);
    CHECK(win != NULL);
    CHECK(SetPropA(win, "answer", (HANDLE)(INT_PTR)42));
    CHECK(GetPropA(win, "answer") == (HANDLE)(INT_PTR)42);
    CHECK(RemovePropA(win, "answer") == (HANDLE)(INT_PTR)42);
    CHECK(GetPropA(win, "answer") == NULL);
    HANDLE t = CreateThread(NULL, 0, worker, NULL, 0, NULL);
    MSG m;
    while (GetMessageA(&m, NULL, 0, 0) > 0) {
        TranslateMessage(&m);
        DispatchMessageA(&m);
    }
    WaitForSingleObject(t, 5000);
    CHECK(seen_user == 2);
    CHECK(seen_post == 2);
    if (failures) return failures;
    printf("ALL OK\n");
    return 0;
}
