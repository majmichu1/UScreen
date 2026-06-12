#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <poll.h>
#include <errno.h>
#include <dirent.h>
#include "evdi_drm.h"
#include "evdi_lib.h"

static volatile int g_running = 1;
void handle_sig(int s) { (void)s; g_running = 0; }

int main(int argc, char **argv) {
    const char *edid_path = argc > 1 ? argv[1] : "edid/s9ultra.bin";
    int card = argc > 2 ? atoi(argv[2]) : -1;

    signal(SIGINT, handle_sig);
    signal(SIGTERM, handle_sig);

    // Find card if not specified
    if (card < 0) {
        DIR *dir = opendir("/sys/devices/platform/evdi.0/drm");
        if (dir) {
            struct dirent *e;
            while ((e = readdir(dir)) != NULL) {
                int c; if (sscanf(e->d_name, "card%d", &c) == 1) { card = c; break; }
            }
            closedir(dir);
        }
    }
    if (card < 0) { fprintf(stderr, "No card\n"); return 1; }
    fprintf(stderr, "Using card%d\n", card);

    evdi_handle handle = evdi_open(card);
    if (!handle) { fprintf(stderr, "Failed open\n"); return 1; }

    FILE *f = fopen(edid_path, "rb"); if (!f) return 1;
    fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
    unsigned char *edid = malloc(sz);
    fread(edid, 1, sz, f); fclose(f);

    evdi_connect(handle, edid, sz, 0);
    free(edid);
    fprintf(stderr, "Connected.\n");

    // First, let's handle events until we get mode info
    int evfd = evdi_get_event_ready(handle);
    int have_mode = 0;
    int mode_w = 0, mode_h = 0, stride = 0;
    void *fb = NULL;

    struct evdi_event_context ctx = {
        .dpms_handler = (void*)0,
        .mode_changed_handler = (void*)1, // will check by ptr value
        .update_ready_handler = (void*)2,
        .crtc_state_handler = (void*)3,
        .cursor_set_handler = (void*)4,
        .cursor_move_handler = (void*)5,
    };

    // We need a real context - let me handle this differently
    // For now, just poll and handle events for a bit
    fprintf(stderr, "Polling for events...\n");
    for (int i = 0; i < 50; i++) {
        struct pollfd pfd = {.fd = evfd, .events = POLLIN};
        if (poll(&pfd, 1, 100) <= 0) continue;
        
        // Handle events to process the mode change
        // We need proper handlers, let me set them up
        break;
    }

    // Actually, the clean approach: use evdi_grab_pixels directly
    // It's a blocking call that waits for the next frame
    // Before that, we need to handle events to get the mode

    // Let me set up proper handlers and handle events first
    fprintf(stderr, "Setting up proper handlers...\n");

    // Re-open and reconnect with proper handlers
    evdi_close(handle);
    handle = evdi_open(card);
    
    // Allocate a buffer for pixel data
    // Start with 4K sized buffer
    unsigned char *pixels = malloc(2960 * 1848 * 4);
    struct evdi_buffer buf = {
        .id = 1,
        .buffer = pixels,
        .width = 2960,
        .height = 1848,
        .stride = 2960 * 4,
        .rects = NULL,
        .rect_count = 0,
    };

    // Wait for mode from KWin by handling events
    int mode_set = 0;
    struct evdi_event_context ctx2 = {0};
    ctx2.mode_changed_handler = (void (*)(struct evdi_mode, void*))(+[](struct evdi_mode m, void *ud) {
        (void)ud;
        int *w = (int*)ud;
        w[0] = m.width; w[1] = m.height;
        fprintf(stderr, "MODE: %dx%d bpp=%d\n", m.width, m.height, m.bits_per_pixel);
    });
    ctx2.dpms_handler = (void(*)(int,void*))(+[](int m, void*ud){(void)m;(void)ud;});
    ctx2.crtc_state_handler = (void(*)(int,void*))(+[](int s,void*ud){(void)s;(void)ud;});
    ctx2.update_ready_handler = (void(*)(int,void*))(+[](int id,void*ud){(void)id;(void)ud;fprintf(stderr,"UR: %d\n",id);});
    ctx2.cursor_set_handler = (void(*)(struct evdi_cursor_set,void*))(+[](struct evdi_cursor_set s,void*ud){(void)s;(void)ud;});
    ctx2.cursor_move_handler = (void(*)(struct evdi_cursor_move,void*))(+[](struct evdi_cursor_move m,void*ud){(void)m;(void)ud;});
    ctx2.user_data = (void*)pixels; // reuse as mode storage

    evdi_connect(handle, (unsigned char*)"", 0, 0); // will fail but we reconnect next

actually this is getting messy with lambdas. Let me just use the evdi_grab_pixels function directly as documented.

The evdi_grab_pixels function signature:
  evdi_grab_pixels(evdi_handle handle, struct evdi_rect *rects, int *num_rects);

This function BLOCKS until new frame is available. It fills rects with dirty regions. The pixel data lands in the registered buffer.

So the flow should be:
1. evdi_connect(handle, edid, edid_len)
2. Wait for MODE_CHANGED event via poll + handle_events
3. Allocate buffer matching the mode
4. evdi_register_buffer(handle, buf)
5. In a loop:
   a. Call evdi_handle_events() to process pending events (non-blocking if using poll)
   b. Call evdi_grab_pixels(handle, rects, &num_rects) - blocks until next frame
   c. The registered buffer now has the pixel data

Actually wait - I'm not sure if we need to call evdi_handle_events in the loop with evdi_grab_pixels. Let me check...

The evdi_grab_pixels function is the OLDER API. It internally waits for events from the kernel and reads the pixel data. The event loop and evdi_grab_pixels are two different APIs:
- Event loop API: poll+handle_events, then react to callbacks (update_ready, etc.)
- Grab API: register_buffer, then call evdi_grab_pixels in a loop (it blocks)

Actually, looking at the evdi library more carefully (from my knowledge of the library source):

`evdi_grab_pixels` is the blocking API that should be called in a loop. It returns the pixel data into the registered buffer and fills rects with dirty regions. It handles events internally.

But wait - I also see `evdi_request_update` which is part of the event-driven API. Let me think about which API to use...

In the evdi source (https://github.com/DisplayLink/evdi), the `evdi_grab_pixels` function is defined as:
1. It first handles any pending events
2. Then calls `evdi_grab_pixels` on the DRM fd, which is an ioctl that blocks until a new frame is available
3. After the ioctl returns, the pixel data is in the buffer surface
4. It fills rects with the dirty rectangle info

Actually, I just realized the issue with my grammar. The lambdas with + prefix won't work in C, only in C++.

Let me write a simple, clean test that uses evdi_grab_pixels properly.
EOF

# Let me write a much simpler test
cat > /tmp/evdi_grab_test2.c << 'EOF'
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <poll.h>
#include <errno.h>
#include <dirent.h>
#include "evdi_drm.h"
#include "evdi_lib.h"

static volatile int g_running = 1;
void on_sig(int s) { (void)s; g_running = 0; }

// Event handlers
static int g_mode_w = 0, g_mode_h = 0, g_mode_bpp = 4, g_have_mode = 0;
static int g_evfd = -1;
static struct evdi_event_context g_ctx = {0};
static unsigned char *g_fb = NULL;
static int g_fb_size = 0;
static struct evdi_buffer g_buf = {0};

void on_mode(struct evdi_mode m, void *ud) {
    (void)ud;
    g_mode_w = m.width;
    g_mode_h = m.height;
    g_mode_bpp = m.bits_per_pixel / 8;
    if (g_mode_bpp < 1) g_mode_bpp = 4;
    g_fb_size = g_mode_w * g_mode_h * g_mode_bpp;
    fprintf(stderr, "MODE: %dx%d bpp=%d\n", g_mode_w, g_mode_h, g_mode_bpp);
    
    if (g_fb) free(g_fb);
    g_fb = malloc(g_fb_size);
    memset(g_fb, 0, g_fb_size);
    
    g_buf.id = 1;
    g_buf.buffer = g_fb;
    g_buf.width = g_mode_w;
    g_buf.height = g_mode_h;
    g_buf.stride = g_mode_w * g_mode_bpp;
    g_buf.rects = NULL;
    g_buf.rect_count = 0;
    g_have_mode = 1;
}

void on_dpms(int m, void *ud) { (void)m; (void)ud; fprintf(stderr, "DPMS: %d\n", m); }
void on_crtc(int s, void *ud) { (void)s; (void)ud; fprintf(stderr, "CRTC: %d\n", s); }
void on_ur(int id, void *ud) { (void)id; (void)ud; fprintf(stderr, "UR: %d\n", id); }
void on_cs(struct evdi_cursor_set s, void *ud) { (void)s; (void)ud; }
void on_cm(struct evdi_cursor_move m, void *ud) { (void)m; (void)ud; }

int main(int argc, char **argv) {
    const char *edid_path = argc > 1 ? argv[1] : "edid/s9ultra.bin";
    int card = argc > 2 ? atoi(argv[2]) : -1;
    signal(SIGINT, on_sig); signal(SIGTERM, on_sig);

    if (card < 0) {
        DIR *dir = opendir("/sys/devices/platform/evdi.0/drm");
        if (dir) {
            struct dirent *e;
            while ((e = readdir(dir)) != NULL) {
                int c; if (sscanf(e->d_name, "card%d", &c) == 1) { card = c; break; }
            }
            closedir(dir);
        }
    }
    if (card < 0) { fprintf(stderr, "No card\n"); return 1; }
    fprintf(stderr, "Card: %d\n", card);

    evdi_handle handle = evdi_open(card);
    if (!handle) { fprintf(stderr, "Open failed\n"); return 1; }

    FILE *f = fopen(edid_path, "rb"); if (!f) return 1;
    fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
    unsigned char *edid = malloc(sz);
    fread(edid, 1, sz, f); fclose(f);
    evdi_connect(handle, edid, sz, 0);
    free(edid);
    fprintf(stderr, "Connected.\n");

    g_ctx.dpms_handler = on_dpms;
    g_ctx.mode_changed_handler = on_mode;
    g_ctx.update_ready_handler = on_ur;
    g_ctx.crtc_state_handler = on_crtc;
    g_ctx.cursor_set_handler = on_cs;
    g_ctx.cursor_move_handler = on_cm;

    g_evfd = evdi_get_event_ready(handle);

    // Wait for mode + register buffer
    for (int i = 0; i < 100 && !g_have_mode; i++) {
        struct pollfd pfd = {.fd = g_evfd, .events = POLLIN};
        if (poll(&pfd, 1, 200) > 0 && (pfd.revents & POLLIN))
            evdi_handle_events(handle, &g_ctx);
    }
    
    if (!g_have_mode) { fprintf(stderr, "No mode received\n"); return 1; }
    
    // Register the buffer
    evdi_register_buffer(handle, g_buf);
    fprintf(stderr, "Buffer registered: %dx%d stride=%d\n", g_buf.width, g_buf.height, g_buf.stride);
    
    // Now try evdi_grab_pixels - this should BLOCK until a frame is available
    fprintf(stderr, "Calling evdi_grab_pixels...\n");
    
    struct evdi_rect rects[64];
    int num_rects = 64;
    
    int frame = 0;
    while (g_running && frame < 3) {
        // First handle any pending events
        struct pollfd pfd = {.fd = g_evfd, .events = POLLIN};
        if (poll(&pfd, 1, 0) > 0 && (pfd.revents & POLLIN))
            evdi_handle_events(handle, &g_ctx);
        
        fprintf(stderr, "Grab %d...\n", frame);
        num_rects = 64;
        evdi_grab_pixels(handle, rects, &num_rects);
        frame++;
        
        fprintf(stderr, "Frame %d: %d rects, fb[0..7]=%02x %02x %02x %02x %02x %02x %02x %02x\n",
                frame, num_rects,
                g_fb[0], g_fb[1], g_fb[2], g_fb[3],
                g_fb[4], g_fb[5], g_fb[6], g_fb[7]);
                
        if (frame == 1) {
            fwrite(g_fb, 1, g_fb_size > 64 ? 64 : g_fb_size, stdout);
            fflush(stdout);
        }
    }

    fprintf(stderr, "Done: %d frames\n", frame);
    evdi_disconnect(handle);
    evdi_close(handle);
    free(g_fb);
    return 0;
}
EOF

gcc -Wall -O2 -Ihost/evdi -o /tmp/evdi_grab_test2 /tmp/evdi_grab_test2.c -levdi -ldrm 2>&1
echo "compile: $?"