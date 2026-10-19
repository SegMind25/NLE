/* The smallest useful Windows program: arguments in, text and an exit code out.
 *
 *   x86_64-w64-mingw32-gcc -O2 -o hello.exe hello.c
 *   nle hello.exe one two        # runs natively, exit code 3
 */
#include <stdio.h>

int main(int argc, char **argv) {
    printf("hello from a Windows program, running natively on Linux\n");
    for (int i = 0; i < argc; i++)
        printf("  argv[%d] = %s\n", i, argv[i]);
    return argc;
}
