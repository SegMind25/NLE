/* Message boxes from a console program: the answer comes back as the exit code.
 *
 *   x86_64-w64-mingw32-gcc -O2 -o msgbox.exe msgbox.c
 *   nle msgbox.exe
 */
#include <windows.h>
#include <stdio.h>

int main(void) {
    int a = MessageBoxA(NULL, "Do you want NLE to run this program?\n\nIt is a Windows program.", "Question", MB_YESNO | MB_ICONQUESTION);
    printf("answer: %s\n", a == IDYES ? "yes" : a == IDNO ? "no" : "other");
    MessageBoxA(NULL, "All done.", "NLE", MB_OK | MB_ICONINFORMATION);
    return a == IDYES ? 0 : 1;
}
