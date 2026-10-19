/* A tiny X11 test driver: finds a window by (part of) its title, then sends it input and
 * captures its contents. It plays the part of a user and a screen grabber when NLE is run
 * under Xvfb in the tests.
 *
 *   cc -o xdrive xdrive.c -lX11
 *   xdrive TITLE wait 10000 shot out.ppm click 40 30 key a type hello close
 *
 * Commands run in order: wait MS, sleep MS, shot FILE.ppm, click X Y [1|2|3], move X Y,
 * drag X1 Y1 X2 Y2, key KEYSYM, type TEXT, close, title TEXT (look for another window from now on),
 * expect-window (exit 3 unless the window exists).
 *
 * Two commands go through the X server like a real pointer, so grabs and pop-up windows behave as for a
 * user (they need libXtst): fclick X Y [1|2|3] clicks at a position given relative to the window (it may lie
 * outside it, over a pop-up), and rshot FILE.ppm captures the whole screen.
 */
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/keysym.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static Display *dpy;
static Window root;

/* libXtst has no development headers on most systems; these two prototypes are all that is used.
 * Build with -DNO_XTEST (and without -lXtst) where it is not installed: fclick then fails with status 5. */
#ifdef NO_XTEST
static int XTestFakeMotionEvent(Display *d, int s, int x, int y, unsigned long t) { (void)d; (void)s; (void)x; (void)y; (void)t; exit(5); }
static int XTestFakeButtonEvent(Display *d, unsigned int b, Bool p, unsigned long t) { (void)d; (void)b; (void)p; (void)t; exit(5); }
#else
extern int XTestFakeMotionEvent(Display *, int, int, int, unsigned long);
extern int XTestFakeButtonEvent(Display *, unsigned int, Bool, unsigned long);
#endif

static int title_matches(Window w, const char *needle) {
    char *name = NULL;
    Atom utf8 = XInternAtom(dpy, "UTF8_STRING", False);
    Atom net = XInternAtom(dpy, "_NET_WM_NAME", False);
    Atom type;
    int fmt;
    unsigned long n, after;
    unsigned char *data = NULL;
    if (XGetWindowProperty(dpy, w, net, 0, 256, False, utf8, &type, &fmt, &n, &after, &data) == Success && data) {
        int ok = strstr((char *)data, needle) != NULL;
        XFree(data);
        if (ok) return 1;
    }
    if (XFetchName(dpy, w, &name) && name) {
        int ok = strstr(name, needle) != NULL;
        XFree(name);
        return ok;
    }
    return 0;
}

static Window find(Window w, const char *needle) {
    if (w != root && title_matches(w, needle)) return w;
    Window r, p, *kids = NULL;
    unsigned int n = 0;
    if (!XQueryTree(dpy, w, &r, &p, &kids, &n)) return 0;
    Window found = 0;
    for (unsigned int i = 0; i < n && !found; i++) found = find(kids[i], needle);
    if (kids) XFree(kids);
    return found;
}

static void send_key(Window win, KeySym sym, unsigned state, int down) {
    XKeyEvent e;
    memset(&e, 0, sizeof e);
    e.type = down ? KeyPress : KeyRelease;
    e.display = dpy;
    e.window = win;
    e.root = root;
    e.same_screen = True;
    e.keycode = XKeysymToKeycode(dpy, sym);
    e.state = state;
    e.time = CurrentTime;
    XSendEvent(dpy, win, False, down ? KeyPressMask : KeyReleaseMask, (XEvent *)&e);
    XFlush(dpy);
}

static void send_button(Window win, int x, int y, int button, int down) {
    XButtonEvent e;
    memset(&e, 0, sizeof e);
    e.type = down ? ButtonPress : ButtonRelease;
    e.display = dpy;
    e.window = win;
    e.root = root;
    e.same_screen = True;
    e.x = x;
    e.y = y;
    e.button = button;
    e.state = down ? 0 : (Button1Mask << (button - 1));
    XSendEvent(dpy, win, False, down ? ButtonPressMask : ButtonReleaseMask, (XEvent *)&e);
    XFlush(dpy);
}

static void send_motion(Window win, int x, int y) {
    XMotionEvent e;
    memset(&e, 0, sizeof e);
    e.type = MotionNotify;
    e.display = dpy;
    e.window = win;
    e.root = root;
    e.same_screen = True;
    e.x = x;
    e.y = y;
    XSendEvent(dpy, win, False, PointerMotionMask, (XEvent *)&e);
    XFlush(dpy);
}

static int shot(Window win, const char *path) {
    XWindowAttributes a;
    XGetWindowAttributes(dpy, win, &a);
    XImage *img = XGetImage(dpy, win, 0, 0, a.width, a.height, AllPlanes, ZPixmap);
    if (!img) return 1;
    FILE *f = fopen(path, "wb");
    if (!f) return 1;
    fprintf(f, "P6\n%d %d\n255\n", a.width, a.height);
    for (int y = 0; y < a.height; y++)
        for (int x = 0; x < a.width; x++) {
            unsigned long p = XGetPixel(img, x, y);
            fputc((p >> 16) & 255, f);
            fputc((p >> 8) & 255, f);
            fputc(p & 255, f);
        }
    fclose(f);
    XDestroyImage(img);
    return 0;
}

static void fake_click(Window win, int x, int y, int button) {
    int rx, ry;
    Window child;
    XTranslateCoordinates(dpy, win, root, x, y, &rx, &ry, &child);
    XTestFakeMotionEvent(dpy, -1, rx, ry, 0);
    XFlush(dpy);
    usleep(80000);
    XTestFakeButtonEvent(dpy, button, True, 0);
    XFlush(dpy);
    usleep(80000);
    XTestFakeButtonEvent(dpy, button, False, 0);
    XFlush(dpy);
    usleep(120000);
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: xdrive TITLE command...\n");
        return 2;
    }
    dpy = XOpenDisplay(NULL);
    if (!dpy) {
        fprintf(stderr, "xdrive: cannot open the display\n");
        return 2;
    }
    root = DefaultRootWindow(dpy);
    const char *needle = argv[1];
    Window win = 0;
    for (int i = 2; i < argc; i++) {
        const char *c = argv[i];
        if (!strcmp(c, "wait")) {
            int ms = atoi(argv[++i]);
            for (int t = 0; t <= ms && !win; t += 20) {
                win = find(root, needle);
                if (!win) usleep(20000);
            }
            if (!win) {
                fprintf(stderr, "xdrive: no window titled `%s`\n", needle);
                return 3;
            }
            usleep(150000); /* let it paint */
        } else if (!strcmp(c, "sleep")) {
            usleep(atoi(argv[++i]) * 1000);
        } else if (!strcmp(c, "title")) {
            needle = argv[++i];
            win = 0;
        } else if (!strcmp(c, "expect-window")) {
            if (!find(root, needle)) return 3;
        } else {
            if (!win) win = find(root, needle);
            if (!win) {
                fprintf(stderr, "xdrive: no window titled `%s`\n", needle);
                return 3;
            }
            if (!strcmp(c, "shot")) {
                if (shot(win, argv[++i])) return 4;
            } else if (!strcmp(c, "click")) {
                int x = atoi(argv[++i]), y = atoi(argv[++i]), b = 1;
                if (i + 1 < argc && argv[i + 1][0] >= '1' && argv[i + 1][0] <= '3' && !argv[i + 1][1]) b = atoi(argv[++i]);
                send_motion(win, x, y);
                send_button(win, x, y, b, 1);
                send_button(win, x, y, b, 0);
            } else if (!strcmp(c, "fclick")) {
                int x = atoi(argv[++i]), y = atoi(argv[++i]), b = 1;
                if (i + 1 < argc && argv[i + 1][0] >= '1' && argv[i + 1][0] <= '3' && !argv[i + 1][1]) b = atoi(argv[++i]);
                fake_click(win, x, y, b);
            } else if (!strcmp(c, "rshot")) {
                if (shot(root, argv[++i])) return 4;
            } else if (!strcmp(c, "drag")) {
                int x1 = atoi(argv[++i]), y1 = atoi(argv[++i]), x2 = atoi(argv[++i]), y2 = atoi(argv[++i]);
                send_motion(win, x1, y1);
                send_button(win, x1, y1, 1, 1);
                for (int k = 1; k <= 8; k++) {
                    send_motion(win, x1 + (x2 - x1) * k / 8, y1 + (y2 - y1) * k / 8);
                    usleep(15000);
                }
                send_button(win, x2, y2, 1, 0);
            } else if (!strcmp(c, "move")) {
                int x = atoi(argv[++i]), y = atoi(argv[++i]);
                send_motion(win, x, y);
            } else if (!strcmp(c, "key")) {
                KeySym s = XStringToKeysym(argv[++i]);
                send_key(win, s, 0, 1);
                send_key(win, s, 0, 0);
            } else if (!strcmp(c, "type")) {
                for (const char *p = argv[++i]; *p; p++) {
                    char buf[2] = {*p, 0};
                    KeySym s = *p == ' ' ? XK_space : XStringToKeysym(buf);
                    unsigned st = (*p >= 'A' && *p <= 'Z') ? ShiftMask : 0;
                    send_key(win, s, st, 1);
                    send_key(win, s, st, 0);
                }
            } else if (!strcmp(c, "close")) {
                XEvent e;
                memset(&e, 0, sizeof e);
                e.xclient.type = ClientMessage;
                e.xclient.window = win;
                e.xclient.message_type = XInternAtom(dpy, "WM_PROTOCOLS", False);
                e.xclient.format = 32;
                e.xclient.data.l[0] = XInternAtom(dpy, "WM_DELETE_WINDOW", False);
                XSendEvent(dpy, win, False, NoEventMask, &e);
                XFlush(dpy);
            } else {
                fprintf(stderr, "xdrive: unknown command %s\n", c);
                return 2;
            }
            usleep(60000);
        }
    }
    XCloseDisplay(dpy);
    return 0;
}
