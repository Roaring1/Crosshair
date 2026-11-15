#include <X11/Xatom.h>
#include <X11/Xlib.h>

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s WINDOW_ID\n", argv[0]);
        return 2;
    }

    char *end = NULL;
    errno = 0;
    unsigned long raw_window = strtoul(argv[1], &end, 0);
    if (errno != 0 || end == argv[1] || *end != '\0' || raw_window == 0) {
        fprintf(stderr, "invalid X11 window id: %s\n", argv[1]);
        return 2;
    }

    Display *display = XOpenDisplay(NULL);
    if (display == NULL) {
        fprintf(stderr, "cannot open the current X11 display\n");
        return 1;
    }

    Atom wm_protocols = XInternAtom(display, "WM_PROTOCOLS", False);
    Atom wm_delete = XInternAtom(display, "WM_DELETE_WINDOW", False);
    XEvent event = {0};
    event.xclient.type = ClientMessage;
    event.xclient.window = (Window)raw_window;
    event.xclient.message_type = wm_protocols;
    event.xclient.format = 32;
    event.xclient.data.l[0] = (long)wm_delete;
    event.xclient.data.l[1] = (long)CurrentTime;

    Status sent = XSendEvent(display, (Window)raw_window, False, NoEventMask, &event);
    XFlush(display);
    XCloseDisplay(display);
    if (sent == 0) {
        fprintf(stderr, "could not send WM_DELETE_WINDOW to 0x%lx\n", raw_window);
        return 1;
    }
    return 0;
}
