#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <poll.h>
#include <errno.h>
#include <dirent.h>
#include <drm/drm.h>
#include "evdi_drm.h"
#include "evdi_lib.h"

static evdi_handle handle = NULL;
static struct evdi_buffer buf = {0};
static int mode_w = 0, mode_h = 0, mode_bpp = 0;
static int frame_count = 0;
static int fb_size = 0;
static int evfd = -1;

void on_dpms(int m, void *u) { (void)m; (void)u; fprintf(stderr, "DPMS: %d\n", m); }

void on_mode(struct evdi_mode m, void *u) {
    (void)u;
    mode_w = m.width; mode_h = m.height; mode_bpp = m.bits_per_pixel / 8;
    fb_size = mode_w * mode_h * mode_bpp;
    fprintf(stderr, "MODE: %dx%d bpp=%d fmt=0x%x\n", m.width, m.height, m.bits_per_pixel, m.pixel_format);
    if (buf.buffer) free(buf.buffer);
    buf.buffer = malloc(fb_size);
    memset(buf.buffer, 0xAB, fb_size);
    buf.id = 1;
    buf.width = m.width;
    buf.height = m.height;
    buf.stride = m.width * mode_bpp;
    buf.rects = NULL;
    buf.rect_count = 0;
    evdi_register_buffer(handle, buf);
    fprintf(stderr, "BUFFER: id=%d %p stride=%d\n", buf.id, buf.buffer, buf.stride);
}

void on_update(int buf_id, void *u) {
    (void)u;
    if (buf_id != buf.id || !buf.buffer) return;
    fprintf(stderr, "UPDATE_READY: id=%d\n", buf_id);
    if (evdi_request_update(handle, buf_id)) {
        frame_count++;
        fprintf(stderr, "FRAME %d: first bytes: %02x %02x %02x %02x %02x %02x %02x %02x\n",
                frame_count,
                ((unsigned char*)buf.buffer)[0],
                ((unsigned char*)buf.buffer)[1],
                ((unsigned char*)buf.buffer)[2],
                ((unsigned char*)buf.buffer)[3],
                ((unsigned char*)buf.buffer)[4],
                ((unsigned char*)buf.buffer)[5],
                ((unsigned char*)buf.buffer)[6],
                ((unsigned char*)buf.buffer)[7]);
        if (frame_count == 1) {
            fwrite(buf.buffer, 1, fb_size > 256 ? 256 : fb_size, stdout);
            fflush(stdout);
        }
    } else {
        fprintf(stderr, "request_update FAILED\n");
    }
}

void on_crtc(int s, void *u) { (void)s; (void)u; fprintf(stderr, "CRTC: %d\n", s); }
void on_cursor_set(struct evdi_cursor_set s, void *u) { (void)s; (void)u; }
void on_cursor_move(struct evdi_cursor_move m, void *u) { (void)m; (void)u; }

int main(int argc, char **argv) {
    const char *edid_path = argc > 1 ? argv[1] : "edid/s9ultra.bin";
    int add_first = 0;
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--card") == 0 && i+1 < argc) {
            evfd = atoi(argv[++i]);
        }
        if (strcmp(argv[i], "--add") == 0) add_first = 1;
    }

    if (add_first) {
        fprintf(stderr, "Adding EVDI device...\n");
        evdi_add_device();
        sleep(2);
    }

    if (evfd < 0) {
        DIR *dir = opendir("/sys/devices/platform/evdi.0/drm");
        if (dir) {
            struct dirent *e;
            while ((e = readdir(dir)) != NULL) {
                int c; if (sscanf(e->d_name, "card%d", &c) == 1) { evfd = c; break; }
            }
            closedir(dir);
        }
    }
    if (evfd < 0) { fprintf(stderr, "No card\n"); return 1; }
    fprintf(stderr, "Card: %d\n", evfd);

    handle = evdi_open(evfd);
    if (!handle) { fprintf(stderr, "Failed open\n"); return 1; }

    FILE *f = fopen(edid_path, "rb"); if (!f) return 1;
    fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
    unsigned char *edid = malloc(sz);
    fread(edid, 1, sz, f); fclose(f);

    evdi_connect(handle, edid, sz, 0);
    free(edid);
    fprintf(stderr, "Connected.\n");

    struct evdi_event_context ctx = {
        .dpms_handler = on_dpms,
        .mode_changed_handler = on_mode,
        .update_ready_handler = on_update,
        .crtc_state_handler = on_crtc,
        .cursor_set_handler = on_cursor_set,
        .cursor_move_handler = on_cursor_move,
    };

    int fd = evdi_get_event_ready(handle);
    for (int i = 0; i < 300 && frame_count < 3; i++) {
        struct pollfd pfd = {.fd = fd, .events = POLLIN};
        if (poll(&pfd, 1, 100) > 0 && (pfd.revents & POLLIN))
            evdi_handle_events(handle, &ctx);
    }

    fprintf(stderr, "Done. Frames: %d\n", frame_count);
    evdi_disconnect(handle);
    evdi_close(handle);
    free(buf.buffer);
    return 0;
}
