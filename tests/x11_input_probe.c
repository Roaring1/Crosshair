#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/shape.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int inspect(Display *display, Window window) {
    XWindowAttributes attributes;
    if (!XGetWindowAttributes(display, window, &attributes)) return 3;
    XWMHints *hints = XGetWMHints(display, window);
    int count = 0;
    int ordering = 0;
    XRectangle *rectangles = XShapeGetRectangles(display, window, ShapeInput, &count, &ordering);
    printf("DEPTH %d\n", attributes.depth);
    printf("INPUT_HINT %s\n",
           hints && (hints->flags & InputHint) && hints->input == False ? "false" : "not-disabled");
    printf("INPUT_SHAPE_RECTS %d\n", count);
    if (rectangles) XFree(rectangles);
    if (hints) XFree(hints);
    XSync(display, False);
    return 0;
}

int main(int argc, char **argv) {
    Display *display = XOpenDisplay(NULL);
    if (!display) return 2;

    if (argc == 3 && strcmp(argv[1], "inspect") == 0) {
        Window target = (Window)strtoul(argv[2], NULL, 0);
        int result = inspect(display, target);
        XCloseDisplay(display);
        return result;
    }

    int screen = DefaultScreen(display);
    Window root = RootWindow(display, screen);
    unsigned int width = (unsigned int)DisplayWidth(display, screen);
    unsigned int height = (unsigned int)DisplayHeight(display, screen);
    Window window = XCreateSimpleWindow(display, root, 0, 0, width, height, 0, 0, 0);
    XSelectInput(display, window, ButtonPressMask | StructureNotifyMask);
    XStoreName(display, window, "Crosshair X11 click-through probe");
    XMapRaised(display, window);
    XSync(display, False);
    printf("READY %lu\n", (unsigned long)window);
    fflush(stdout);
    for (;;) {
        XEvent event;
        XNextEvent(display, &event);
        if (event.type == ButtonPress) {
            printf("CLICK %d %d %u\n",
                   event.xbutton.x, event.xbutton.y, event.xbutton.button);
            fflush(stdout);
        }
    }
}
