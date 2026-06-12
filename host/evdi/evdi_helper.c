#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <poll.h>
#include <errno.h>
#include <time.h>
#include <dirent.h>
#include <pthread.h>
#include <sys/stat.h>
#include <fcntl.h>
#include "evdi_drm.h"
#include "evdi_lib.h"

static evdi_handle g_handle = EVDI_INVALID_HANDLE;
static int g_device_index = -1;
static volatile int g_running = 1;

static int g_capture_fifo_fd = -1;
static const char *g_fifo_path = NULL;
static int g_fps = 60;

/* EVDI-registered framebuffer (stride-padded, written by the kernel) */
static unsigned char *g_framebuffer = NULL;
static int g_fb_size = 0;
static int g_mode_w = 0;
static int g_mode_h = 0;
static int g_mode_bpp = 4;
static int g_mode_stride = 0;
static volatile int g_have_mode = 0;
static int g_dpms_on = 0;

/* Triple buffer for FIFO writes: grabber packs into g_fill, swaps with
   g_latest; writer swaps g_latest into g_write and streams it out.
   Pointer swaps only — no copies between threads, no stalls. */
static pthread_mutex_t g_swap_mutex = PTHREAD_MUTEX_INITIALIZER;
static unsigned char *g_fill = NULL;
static unsigned char *g_latest = NULL;
static unsigned char *g_write = NULL;
static volatile int g_latest_valid = 0;
static int g_packed_size = 0;          /* tightly packed w*h*4 */
static volatile int g_buffers_ready = 0;

static volatile int g_update_pending = 0;  /* request_update sent, waiting for update_ready */
static volatile int g_writer_busy = 0;     /* writer is streaming g_write to the FIFO */
static volatile int g_mode_generation = 0; /* bumped on every mode change */
static volatile long long g_grab_count = 0;

static void handle_signal(int sig) {
    (void)sig;
    g_running = 0;
}

static long long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static void on_dpms(int dpms_mode, void *user_data) {
    (void)user_data;
    g_dpms_on = (dpms_mode == 0) ? 1 : 0;
    fprintf(stderr, "[evdi-helper] DPMS: %d (%s)\n", dpms_mode, g_dpms_on ? "ON" : "OFF");
}

/* Pack the stride-padded framebuffer into the fill buffer and publish it
   as the latest frame. Called from the event-loop thread after a grab. */
static void publish_frame(void) {
    if (!g_buffers_ready || !g_framebuffer)
        return;

    int row_bytes = g_mode_w * g_mode_bpp;
    if (g_mode_stride == row_bytes) {
        memcpy(g_fill, g_framebuffer, (size_t)g_packed_size);
    } else {
        for (int y = 0; y < g_mode_h; y++) {
            memcpy(g_fill + (size_t)y * row_bytes,
                   g_framebuffer + (size_t)y * g_mode_stride,
                   (size_t)row_bytes);
        }
    }

    pthread_mutex_lock(&g_swap_mutex);
    unsigned char *tmp = g_latest;
    g_latest = g_fill;
    g_fill = tmp;
    g_latest_valid = 1;
    pthread_mutex_unlock(&g_swap_mutex);
}

static int g_buffer_registered = 0;

static void on_mode_changed(struct evdi_mode mode, void *user_data) {
    (void)user_data;
    fprintf(stderr, "[evdi-helper] Mode: %dx%d@%dHz %dbpp fmt=0x%x\n",
            mode.width, mode.height, mode.refresh_rate,
            mode.bits_per_pixel, mode.pixel_format);
    printf("MODE_CHANGED %d %d %d\n", mode.width, mode.height, mode.refresh_rate);
    fflush(stdout);

    int new_w = mode.width;
    int new_h = mode.height;
    int new_bpp = mode.bits_per_pixel / 8;
    if (new_bpp < 1) new_bpp = 4;

    /* Same geometry (KWin re-applying the mode)? Keep everything as-is.
       Re-registering on every event is what crashed libevdi before. */
    if (g_buffer_registered && new_w == g_mode_w && new_h == g_mode_h
            && new_bpp == g_mode_bpp) {
        fprintf(stderr, "[evdi-helper] Mode unchanged, keeping buffer\n");
        g_update_pending = 0;
        return;
    }

    g_mode_w = new_w;
    g_mode_h = new_h;
    g_mode_bpp = new_bpp;

    int row_bytes = g_mode_w * g_mode_bpp;
    int aligned_stride = (row_bytes + 63) & ~63;  /* DRM buffers are 64-byte aligned */
    g_mode_stride = aligned_stride;
    g_fb_size = g_mode_stride * g_mode_h;

    pthread_mutex_lock(&g_swap_mutex);
    g_latest_valid = 0;
    g_buffers_ready = 0;
    g_mode_generation++;
    pthread_mutex_unlock(&g_swap_mutex);

    /* Wait (bounded) for the writer to finish any in-flight write before
       freeing the buffer it's reading. Writer stalls are bounded because
       FIFO writes poll with a timeout, but never spin here forever — a
       stuck event loop blocks KWin's output handling. */
    for (int i = 0; i < 1000 && g_writer_busy; i++)
        usleep(1000);
    if (g_writer_busy) {
        fprintf(stderr, "[evdi-helper] Writer stuck during mode change — leaking old buffers\n");
        /* Deliberately leak instead of freeing under the writer: a one-off
           leak beats a use-after-free. */
        g_fill = NULL;
        g_latest = NULL;
        g_write = NULL;
    }

    /* The kernel must drop its reference to the old framebuffer BEFORE we
       free it — freeing first is a use-after-free during a pending grab. */
    if (g_buffer_registered) {
        evdi_unregister_buffer(g_handle, 0);
        g_buffer_registered = 0;
    }

    free(g_framebuffer);
    g_framebuffer = malloc(g_fb_size);

    g_packed_size = row_bytes * g_mode_h;
    free(g_fill);   g_fill = malloc(g_packed_size);
    free(g_latest); g_latest = malloc(g_packed_size);
    free(g_write);  g_write = malloc(g_packed_size);

    if (!g_framebuffer || !g_fill || !g_latest || !g_write) {
        fprintf(stderr, "[evdi-helper] Failed to allocate framebuffers\n");
        g_have_mode = 0;
        return;
    }

    /* Dark gray initial frame so the tablet shows something immediately */
    memset(g_framebuffer, 0x18, g_fb_size);

    struct evdi_buffer buf = {
        .id = 0,
        .buffer = g_framebuffer,
        .width = g_mode_w,
        .height = g_mode_h,
        .stride = g_mode_stride,
        .rects = NULL,
        .rect_count = 0,
    };
    evdi_register_buffer(g_handle, buf);
    g_buffer_registered = 1;

    pthread_mutex_lock(&g_swap_mutex);
    g_buffers_ready = 1;
    pthread_mutex_unlock(&g_swap_mutex);
    publish_frame();

    g_have_mode = 1;
    g_update_pending = 0;
    fprintf(stderr, "[evdi-helper] Buffer 0 registered: %dx%d stride=%d (row_bytes=%d)\n",
            g_mode_w, g_mode_h, buf.stride, row_bytes);
}

static void grab_now(void) {
    struct evdi_rect rects[64];
    int num_rects = 64;
    evdi_grab_pixels(g_handle, rects, &num_rects);
    if (num_rects > 0) {
        g_grab_count++;
        publish_frame();
    }
}

static void on_update_ready(int buffer_to_be_updated, void *user_data) {
    (void)user_data;
    (void)buffer_to_be_updated;
    g_update_pending = 0;
    grab_now();
}

static void on_crtc_state(int state, void *user_data) {
    (void)user_data;
    fprintf(stderr, "[evdi-helper] CRTC state: %d\n", state);
}

static void on_cursor_set(struct evdi_cursor_set cursor_set, void *user_data) {
    (void)user_data;
    (void)cursor_set;
}

static void on_cursor_move(struct evdi_cursor_move cursor_move, void *user_data) {
    (void)user_data;
    (void)cursor_move;
}

/* (Re)open the capture FIFO without blocking forever: O_NONBLOCK open fails
   with ENXIO while no reader (ffmpeg) has the other end open. */
static int try_open_fifo(void) {
    int fd = open(g_fifo_path, O_WRONLY | O_NONBLOCK);
    if (fd < 0)
        return -1;
    /* Switch back to blocking writes once connected */
    int flags = fcntl(fd, F_GETFL);
    fcntl(fd, F_SETFL, flags & ~O_NONBLOCK);
    /* Enlarge the pipe so a full-frame write doesn't take hundreds of
       64KB round-trips with the encoder. */
    fcntl(fd, F_SETPIPE_SZ, 1 << 20);
    fprintf(stderr, "[evdi-helper] Capture FIFO opened\n");
    return fd;
}

/* Writer thread: paces the FIFO at the target fps, always sending the most
   recent frame. Blocking writes here never stall capture, and the FIFO is
   reopened automatically when the encoder restarts. */
static void *writer_thread(void *arg) {
    (void)arg;
    struct timespec deadline;
    clock_gettime(CLOCK_MONOTONIC, &deadline);
    long period_ns = 1000000000L / (g_fps > 0 ? g_fps : 60);
    int have_frame = 0;
    int frame_generation = -1;

    while (g_running) {
        deadline.tv_nsec += period_ns;
        while (deadline.tv_nsec >= 1000000000L) {
            deadline.tv_nsec -= 1000000000L;
            deadline.tv_sec += 1;
        }
        clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &deadline, NULL);

        if (g_capture_fifo_fd < 0) {
            g_capture_fifo_fd = try_open_fifo();
            if (g_capture_fifo_fd < 0)
                continue;
        }

        int size;
        pthread_mutex_lock(&g_swap_mutex);
        if (!g_buffers_ready) {
            pthread_mutex_unlock(&g_swap_mutex);
            continue;
        }
        if (frame_generation != g_mode_generation) {
            /* Buffers were reallocated; previous g_write content is gone */
            frame_generation = g_mode_generation;
            have_frame = 0;
        }
        if (g_latest_valid) {
            unsigned char *tmp = g_write;
            g_write = g_latest;
            g_latest = tmp;
            g_latest_valid = 0;
            have_frame = 1;
        }
        size = g_packed_size;
        g_writer_busy = have_frame;
        pthread_mutex_unlock(&g_swap_mutex);

        if (!have_frame)
            continue;  /* nothing grabbed yet for this mode */

        /* Bounded write: if the encoder stops reading for >250ms per chunk
           it is stalled or dead — close the FIFO and resync on reopen.
           An unbounded write() here would wedge the whole helper. */
        const unsigned char *ptr = g_write;
        size_t remaining = (size_t)size;
        while (remaining > 0 && g_running) {
            struct pollfd wfd = { .fd = g_capture_fifo_fd, .events = POLLOUT };
            int pr = poll(&wfd, 1, 250);
            if (pr <= 0 || (wfd.revents & (POLLERR | POLLHUP))) {
                fprintf(stderr, "[evdi-helper] Encoder not reading — closing FIFO\n");
                close(g_capture_fifo_fd);
                g_capture_fifo_fd = -1;
                break;
            }
            ssize_t written = write(g_capture_fifo_fd, ptr, remaining);
            if (written <= 0) {
                if (errno == EINTR) continue;
                fprintf(stderr, "[evdi-helper] FIFO write failed: %s\n", strerror(errno));
                close(g_capture_fifo_fd);
                g_capture_fifo_fd = -1;
                break;
            }
            ptr += written;
            remaining -= (size_t)written;
        }
        g_writer_busy = 0;
    }
    return NULL;
}

static void run_event_loop(evdi_handle handle) {
    struct evdi_event_context evtctx = {
        .dpms_handler = on_dpms,
        .mode_changed_handler = on_mode_changed,
        .update_ready_handler = on_update_ready,
        .crtc_state_handler = on_crtc_state,
        .cursor_set_handler = on_cursor_set,
        .cursor_move_handler = on_cursor_move,
        .user_data = NULL,
    };

    struct pollfd fds[1];
    fds[0].fd = evdi_get_event_ready(handle);
    fds[0].events = POLLIN;

    long long last_stats_ms = now_ms();
    long long last_request_ms = 0;
    long long last_fallback_grab_ms = 0;
    long long stats_grab_base = 0;
    long request_period_ms = 1000 / (g_fps > 0 ? g_fps : 60);
    if (request_period_ms < 1) request_period_ms = 1;

    while (g_running) {
        int ret = poll(fds, 1, 4);
        if (ret < 0) {
            if (errno == EINTR) continue;
            fprintf(stderr, "[evdi-helper] poll() error: %s\n", strerror(errno));
            break;
        }

        if (ret > 0 && (fds[0].revents & POLLIN)) {
            /* update_ready / mode_changed handlers fire from here */
            evdi_handle_events(handle, &evtctx);
        }

        if (!g_have_mode)
            continue;

        long long now = now_ms();

        /* Core capture cycle: request a fresh frame from the compositor at
           the target fps. If the kernel says pixels are ready right away,
           grab immediately; otherwise update_ready will fire and grab. */
        if (!g_update_pending && (now - last_request_ms) >= request_period_ms) {
            last_request_ms = now;
            if (evdi_request_update(handle, 0)) {
                grab_now();
            } else {
                g_update_pending = 1;
            }
        }

        /* Watchdog: if a request got lost (compositor hiccup), don't stay
           stuck waiting for update_ready forever. */
        if (g_update_pending && (now - last_request_ms) > 250) {
            g_update_pending = 0;
            grab_now();
        }

        /* Fallback grab once a second in case no events flow at all */
        if ((now - last_fallback_grab_ms) >= 1000) {
            last_fallback_grab_ms = now;
            if (!g_update_pending)
                grab_now();
        }

        if (now - last_stats_ms >= 5000) {
            double elapsed = (now - last_stats_ms) / 1000.0;
            long long grabs = g_grab_count - stats_grab_base;
            fprintf(stderr, "[evdi-helper] %.1f grabs/s (total %lld), mode:%d dpms:%d pending:%d\n",
                    elapsed > 0 ? grabs / elapsed : 0,
                    g_grab_count, g_have_mode, g_dpms_on, g_update_pending);
            stats_grab_base = g_grab_count;
            last_stats_ms = now;
        }
    }
}

static int find_evdi_device(void) {
    DIR *dir = opendir("/sys/devices/platform");
    if (!dir) return -1;

    struct dirent *entry;
    int found = -1;
    while ((entry = readdir(dir)) != NULL) {
        if (strncmp(entry->d_name, "evdi.", 5) != 0)
            continue;

        char drm_path[256];
        snprintf(drm_path, sizeof(drm_path), "/sys/devices/platform/%s/drm", entry->d_name);

        DIR *drm_dir = opendir(drm_path);
        if (!drm_dir) continue;

        struct dirent *drm_entry;
        while ((drm_entry = readdir(drm_dir)) != NULL) {
            if (strncmp(drm_entry->d_name, "card", 4) != 0)
                continue;
            int card = atoi(drm_entry->d_name + 4);
            if (card > 0) {
                found = card;
            }
        }
        closedir(drm_dir);
        if (found >= 0) break;
    }
    closedir(dir);
    return found;
}

static int wait_for_device(int timeout_ms) {
    int waited = 0;
    const int step = 100;
    while (waited < timeout_ms) {
        int idx = find_evdi_device();
        if (idx >= 0) return idx;
        usleep(step * 1000);
        waited += step;
    }
    return -1;
}

int main(int argc, char *argv[]) {
    const char *edid_path = NULL;
    const char *fifo_path = NULL;

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--edid") == 0 && i + 1 < argc) {
            edid_path = argv[++i];
        } else if (strcmp(argv[i], "--capture-fifo") == 0 && i + 1 < argc) {
            fifo_path = argv[++i];
        } else if (strcmp(argv[i], "--fps") == 0 && i + 1 < argc) {
            g_fps = atoi(argv[++i]);
            if (g_fps < 1 || g_fps > 240) g_fps = 60;
        }
    }

    if (!edid_path) {
        fprintf(stderr, "Usage: %s --edid <edid.bin> [--capture-fifo <path>] [--fps <n>]\n", argv[0]);
        return 1;
    }

    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = handle_signal;
    sigaction(SIGINT, &sa, NULL);
    sigaction(SIGTERM, &sa, NULL);
    signal(SIGPIPE, SIG_IGN);

    /* Reuse an existing EVDI device if one is free (e.g. from a previous
       run) — adding a new DRM card on every restart floods the compositor
       with display hotplug events. */
    evdi_handle handle = EVDI_INVALID_HANDLE;
    int dev_idx = find_evdi_device();
    if (dev_idx >= 0) {
        handle = evdi_open(dev_idx);
        if (handle != EVDI_INVALID_HANDLE) {
            fprintf(stderr, "[evdi-helper] Reusing EVDI device /dev/dri/card%d\n", dev_idx);
        }
    }

    if (handle == EVDI_INVALID_HANDLE) {
        fprintf(stderr, "[evdi-helper] Creating EVDI device...\n");
        int written = evdi_add_device();
        if (written < 0) {
            fprintf(stderr, "[evdi-helper] Failed to add EVDI device (err=%d)\n", written);
            return 1;
        }

        fprintf(stderr, "[evdi-helper] Waiting for EVDI device...\n");
        dev_idx = wait_for_device(5000);
        if (dev_idx < 0) {
            fprintf(stderr, "[evdi-helper] EVDI device did not appear within timeout.\n"
                            "[evdi-helper] /sys/devices/evdi/add is root-only — run once:\n"
                            "[evdi-helper]   make setup-system\n");
            return 1;
        }
        fprintf(stderr, "[evdi-helper] Found EVDI device at /dev/dri/card%d\n", dev_idx);

        handle = evdi_open(dev_idx);
        if (handle == EVDI_INVALID_HANDLE) {
            fprintf(stderr, "[evdi-helper] Failed to open EVDI device /dev/dri/card%d\n", dev_idx);
            return 1;
        }
    }
    g_device_index = dev_idx;
    g_handle = handle;

    FILE *f = fopen(edid_path, "rb");
    if (!f) {
        fprintf(stderr, "[evdi-helper] Failed to open EDID file: %s\n", edid_path);
        evdi_close(handle);
        return 1;
    }
    fseek(f, 0, SEEK_END);
    long edid_size = ftell(f);
    if (edid_size <= 0 || edid_size > 32768) {
        fprintf(stderr, "[evdi-helper] Invalid EDID size: %ld\n", edid_size);
        fclose(f);
        evdi_close(handle);
        return 1;
    }
    fseek(f, 0, SEEK_SET);
    unsigned char *edid = malloc((size_t)edid_size);
    if (!edid) {
        fprintf(stderr, "[evdi-helper] Failed to allocate EDID buffer\n");
        fclose(f);
        evdi_close(handle);
        return 1;
    }
    size_t read_bytes = fread(edid, 1, (size_t)edid_size, f);
    fclose(f);
    if ((long)read_bytes != edid_size) {
        fprintf(stderr, "[evdi-helper] EDID read error: got %zu of %ld bytes\n", read_bytes, edid_size);
        free(edid);
        evdi_close(handle);
        return 1;
    }

    fprintf(stderr, "[evdi-helper] Connecting with EDID (%ld bytes)...\n", edid_size);
    evdi_connect(handle, edid, (unsigned int)edid_size, 0);
    free(edid);

    printf("EVDI_CONNECTED card%d\n", dev_idx);
    fflush(stdout);

    pthread_t writer = 0;
    if (fifo_path) {
        g_fifo_path = fifo_path;
        fprintf(stderr, "[evdi-helper] Capture FIFO: %s (opened on demand)\n", fifo_path);
        if (pthread_create(&writer, NULL, writer_thread, NULL) != 0) {
            fprintf(stderr, "[evdi-helper] Failed to start writer thread\n");
            return 1;
        }
    }

    fprintf(stderr, "[evdi-helper] Connected. Capture at %d fps. Entering event loop.\n", g_fps);
    run_event_loop(handle);

    g_running = 0;
    if (writer) pthread_join(writer, NULL);
    if (g_capture_fifo_fd >= 0) close(g_capture_fifo_fd);
    free(g_framebuffer);
    free(g_fill);
    free(g_latest);
    free(g_write);

    fprintf(stderr, "[evdi-helper] Disconnecting...\n");
    evdi_disconnect(handle);
    evdi_close(handle);
    g_handle = EVDI_INVALID_HANDLE;

    fprintf(stderr, "[evdi-helper] Done.\n");
    return 0;
}
