/* Tiny assertion helpers for the Windows test programs. Each program prints one line per failed
 * check and "ALL OK" at the end when nothing failed; the exit code is the number of failures. */
#ifndef CHECK_H
#define CHECK_H

#include <stdio.h>
#include <stdlib.h>

static int g_failures = 0;
static int g_checks = 0;

#define CHECK(cond)                                                              \
    do {                                                                         \
        g_checks++;                                                              \
        if (!(cond)) {                                                           \
            g_failures++;                                                        \
            printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);               \
            fflush(stdout);                                                      \
        }                                                                        \
    } while (0)

#define CHECK_EQ_INT(a, b)                                                       \
    do {                                                                         \
        long long _a = (long long)(a), _b = (long long)(b);                      \
        g_checks++;                                                              \
        if (_a != _b) {                                                          \
            g_failures++;                                                        \
            printf("FAIL %s:%d: %s == %s (got %lld, want %lld)\n", __FILE__,     \
                   __LINE__, #a, #b, _a, _b);                                    \
            fflush(stdout);                                                      \
        }                                                                        \
    } while (0)

#define CHECK_STR(a, b)                                                          \
    do {                                                                         \
        const char *_a = (a), *_b = (b);                                         \
        g_checks++;                                                              \
        if (!_a || !_b || strcmp(_a, _b) != 0) {                                 \
            g_failures++;                                                        \
            printf("FAIL %s:%d: %s (got \"%s\", want \"%s\")\n", __FILE__,       \
                   __LINE__, #a, _a ? _a : "(null)", _b ? _b : "(null)");        \
            fflush(stdout);                                                      \
        }                                                                        \
    } while (0)

static int finish(void) {
    if (g_failures == 0)
        printf("ALL OK (%d checks)\n", g_checks);
    else
        printf("%d of %d checks FAILED\n", g_failures, g_checks);
    fflush(stdout);
    return g_failures > 100 ? 100 : g_failures;
}

#endif
