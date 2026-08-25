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
#include <stdint.h>
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
/* Signalled by publish_frame() so the writer wakes the moment a frame exists
   rather than on its own timer — see writer_thread(). */
/* Initialised in main() against CLOCK_MONOTONIC.
   NOT PTHREAD_COND_INITIALIZER: that condvar measures absolute timeouts
   against CLOCK_REALTIME, while the deadlines here come from
   CLOCK_MONOTONIC. Monotonic time is seconds-since-boot and realtime is
   seconds-since-1970, so every deadline looked decades overdue,
   pthread_cond_timedwait returned ETIMEDOUT immediately and the writer
   thread spun at ~65% of a core doing nothing. */
static pthread_cond_t g_frame_ready;
static unsigned char *g_fill = NULL;
static unsigned char *g_latest = NULL;
static unsigned char *g_write = NULL;
static volatile int g_latest_valid = 0;

/* Per-buffer "which chroma rows are out of date in THIS buffer", one bit per
   chroma row. Converting the whole frame every time is wasted work: a desktop
   typically changes a small band, and EVDI already tells us which.
   Why per buffer rather than one global list: with triple buffering each
   buffer was last filled at a different moment, so each is stale in a
   different set of rows. A single shared list would leave whichever buffer
   was skipped holding torn, half-updated content. Damage is therefore
   recorded into all three, and cleared only for the buffer just converted.
   The masks swap together with the buffer pointers they describe. */
static unsigned char *g_dirty_fill = NULL;
static unsigned char *g_dirty_latest = NULL;
static unsigned char *g_dirty_write = NULL;
static int g_dirty_bytes = 0;      /* size of one mask */
static int g_chroma_rows = 0;      /* mode height / 2 */
static int g_packed_size = 0;          /* NV12 frame: out_w*out_h*3/2 */

/* Integer downscale applied on the way to the encoder. The desktop keeps its
   native mode — window layout and scaling are unaffected — while the stream
   carries fewer pixels. That cuts both the bytes crossing into the encoder and
   the tablet's decode time, which measurements put at roughly 7-8ms fixed plus
   1.2ms per megapixel. An integer divisor means the tablet upscales by a whole
   number, avoiding resampling artefacts on top of the softness.
   1 = native, and keeps the original tight conversion loop. */
static int g_scale = 1;
static int g_out_w = 0;
static int g_out_h = 0;
static volatile int g_buffers_ready = 0;

/* How long an unchanged screen may go without a frame being re-sent.
   This is not just about the client's read timeout. The encoder's keyframe
   interval (-g) counts frames, not seconds, so the slower we feed it while
   idle, the longer the wall-clock gap between IDRs — and a client that joins
   or recovers from a drop cannot start decoding until one arrives. At 200ms
   the idle floor is 5fps, which keeps that gap bounded at a few seconds while
   still cutting idle work by an order of magnitude.
   The real fix is requesting an IDR on demand, which needs the in-process
   encoder rather than a pipe into the ffmpeg CLI. */
#define IDLE_KEEPALIVE_MS 200
static long long g_last_write_ms = 0;

static long long now_us(void);
/* Capture-side latency: grab → NV12 → handed to the encoder's FIFO.
   Reported as percentiles so the cost of the pipe-to-ffmpeg design can be
   compared against the decode and wire costs measured on the other side. */
#define LAT_SAMPLES 256
static int g_lat_us[LAT_SAMPLES];
static int g_lat_count = 0;
static long long g_grab_us = 0;      /* when the frame now in g_fill was grabbed */
static long long g_latest_grab_us = 0;
static long long g_write_grab_us = 0;

static int cmp_int(const void *a, const void *b) {
    int x = *(const int *)a, y = *(const int *)b;
    return (x > y) - (x < y);
}

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

static long long now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
}

static void on_dpms(int dpms_mode, void *user_data) {
    (void)user_data;
    g_dpms_on = (dpms_mode == 0) ? 1 : 0;
    fprintf(stderr, "[evdi-helper] DPMS: %d (%s)\n", dpms_mode, g_dpms_on ? "ON" : "OFF");
}

/* --- BGRA -> NV12 conversion -------------------------------------------
   The kernel hands us a 32-bit BGRA framebuffer (21.9 MB at 2960x1848).
   Feeding that raw to the encoder is memory-bandwidth bound: it gets copied
   three times per frame (pack + pipe write + pipe read), which caps the
   pipeline around 70 fps even though NVENC itself sits near-idle. NV12 is
   1.5 bytes/px (8.2 MB) — 2.7x less data through every stage — and NVENC
   accepts it natively, so the encoder no longer color-converts either.

   Conversion uses BT.709 limited-range coefficients (matching the bt709 and
   `-color_range tv` tags the encoder writes) and is split across a small
   thread pool so it costs ~1 ms rather than stalling the capture loop.

   Full range was tried and reverted. It is theoretically better — 256 levels
   instead of 220 — but this tablet's decoder does not act on the SPS
   full-range flag even when the format also declares COLOR_RANGE_FULL: it
   assumes limited range, stretches the levels downwards and visibly crushes
   shadows. Limited range costs precision, not range (the decoder expands
   16-235 back to 0-255), so the only real exposure is slight banding in
   gradients, which is the lesser problem by a wide margin. */

typedef struct {
    const unsigned char *src;   /* BGRA, stride-padded */
    unsigned char *ydst;        /* Y plane, w bytes/row */
    unsigned char *uvdst;       /* interleaved CbCr, w bytes per chroma row */
    int w, h, stride;           /* source dimensions */
    int ow, oh;                 /* destination dimensions (w/scale, h/scale) */
    int scale;                  /* 1 = no downscale */
    int cy0, cy1;               /* chroma-row range [cy0, cy1) this job owns */
    const unsigned char *dirty; /* NULL = convert everything */
} conv_job_t;

static inline int row_is_dirty(const unsigned char *mask, int cy) {
    return mask == NULL || (mask[cy >> 3] & (1u << (cy & 7)));
}

/* Downscaling variant: every output pixel is the mean of a scale x scale
   source block, and each chroma sample the mean of the 2*scale square it
   covers. Box averaging rather than point sampling — dropping pixels would
   alias hard on desktop content, where single-pixel lines are everywhere. */
static inline void convert_strip_scaled(const conv_job_t *j) {
    const int n = j->scale, stride = j->stride, ow = j->ow;
    const int inv = n * n;
    for (int cy = j->cy0; cy < j->cy1; cy++) {
        if (!row_is_dirty(j->dirty, cy))
            continue;
        unsigned char *yo0 = j->ydst + (size_t)(cy * 2) * ow;
        unsigned char *yo1 = yo0 + ow;
        unsigned char *uv  = j->uvdst + (size_t)cy * ow;
        for (int ox = 0; ox < ow; ox += 2) {
            int csb = 0, csg = 0, csr = 0;      /* chroma: whole 2n x 2n block */
            for (int q = 0; q < 4; q++) {       /* four output luma pixels */
                int oxx = ox + (q & 1);
                int oyy = cy * 2 + (q >> 1);
                int sb = 0, sg = 0, sr = 0;
                for (int dy = 0; dy < n; dy++) {
                    const unsigned char *row =
                        j->src + (size_t)(oyy * n + dy) * stride + (size_t)(oxx * n) * 4;
                    for (int dx = 0; dx < n; dx++) {
                        sb += row[dx * 4];
                        sg += row[dx * 4 + 1];
                        sr += row[dx * 4 + 2];
                    }
                }
                int b = sb / inv, g = sg / inv, r = sr / inv;
                unsigned char yv =
                    (unsigned char)(((47 * r + 157 * g + 16 * b + 128) >> 8) + 16);
                if (q < 2) yo0[oxx] = yv; else yo1[oxx] = yv;
                csb += b; csg += g; csr += r;
            }
            int ab = csb >> 2, ag = csg >> 2, ar = csr >> 2;
            int cb = (((-26 * ar - 87 * ag + 112 * ab + 128) >> 8) + 128);
            int cr = (((112 * ar - 102 * ag - 10 * ab + 128) >> 8) + 128);
            if (cb < 0) cb = 0; else if (cb > 255) cb = 255;
            if (cr < 0) cr = 0; else if (cr > 255) cr = 255;
            uv[ox]     = (unsigned char)cb;
            uv[ox + 1] = (unsigned char)cr;
        }
    }
}

static inline void convert_strip(const conv_job_t *j) {
    const int w = j->w, stride = j->stride;
    for (int cy = j->cy0; cy < j->cy1; cy++) {
        if (!row_is_dirty(j->dirty, cy))
            continue;
        int y0 = cy * 2, y1 = y0 + 1;
        const unsigned char *row0 = j->src + (size_t)y0 * stride;
        const unsigned char *row1 = j->src + (size_t)y1 * stride;
        unsigned char *yo0 = j->ydst + (size_t)y0 * w;
        unsigned char *yo1 = j->ydst + (size_t)y1 * w;
        unsigned char *uv  = j->uvdst + (size_t)cy * w;
        for (int x = 0; x < w; x += 2) {
            const unsigned char *p;
            int b, g, r, sb, sg, sr;
            p = row0 + (size_t)x * 4;       b = p[0]; g = p[1]; r = p[2];
            yo0[x]   = (unsigned char)(((47*r + 157*g + 16*b + 128) >> 8) + 16);
            sb = b; sg = g; sr = r;
            p = row0 + (size_t)(x+1) * 4;   b = p[0]; g = p[1]; r = p[2];
            yo0[x+1] = (unsigned char)(((47*r + 157*g + 16*b + 128) >> 8) + 16);
            sb += b; sg += g; sr += r;
            p = row1 + (size_t)x * 4;       b = p[0]; g = p[1]; r = p[2];
            yo1[x]   = (unsigned char)(((47*r + 157*g + 16*b + 128) >> 8) + 16);
            sb += b; sg += g; sr += r;
            p = row1 + (size_t)(x+1) * 4;   b = p[0]; g = p[1]; r = p[2];
            yo1[x+1] = (unsigned char)(((47*r + 157*g + 16*b + 128) >> 8) + 16);
            sb += b; sg += g; sr += r;
            int ar = sr >> 2, ag = sg >> 2, ab = sb >> 2;  /* 2x2 chroma average */
            int cb = (((-26*ar - 87*ag + 112*ab + 128) >> 8) + 128);
            int cr = (((112*ar - 102*ag - 10*ab + 128) >> 8) + 128);
            if (cb < 0) cb = 0; else if (cb > 255) cb = 255;
            if (cr < 0) cr = 0; else if (cr > 255) cr = 255;
            uv[x]   = (unsigned char)cb;
            uv[x+1] = (unsigned char)cr;
        }
    }
}

#define MAX_CONV_THREADS 8
static int g_nthreads = 1;
static pthread_t g_pool[MAX_CONV_THREADS];
static conv_job_t g_jobs[MAX_CONV_THREADS];
static pthread_mutex_t g_pool_mtx = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t g_pool_go = PTHREAD_COND_INITIALIZER;
static pthread_cond_t g_pool_done = PTHREAD_COND_INITIALIZER;
static int g_pool_gen = 0;       /* bumped to dispatch a frame */
static int g_pool_active = 0;    /* worker jobs still running this gen */
static int g_pool_shutdown = 0;

static void *conv_worker(void *arg) {
    int id = (int)(intptr_t)arg;
    int last_gen = 0;
    for (;;) {
        pthread_mutex_lock(&g_pool_mtx);
        while (g_pool_gen == last_gen && !g_pool_shutdown)
            pthread_cond_wait(&g_pool_go, &g_pool_mtx);
        if (g_pool_shutdown) { pthread_mutex_unlock(&g_pool_mtx); break; }
        last_gen = g_pool_gen;
        pthread_mutex_unlock(&g_pool_mtx);

        if (g_jobs[id].scale > 1) convert_strip_scaled(&g_jobs[id]);
        else convert_strip(&g_jobs[id]);

        pthread_mutex_lock(&g_pool_mtx);
        if (--g_pool_active == 0)
            pthread_cond_signal(&g_pool_done);
        pthread_mutex_unlock(&g_pool_mtx);
    }
    return NULL;
}

static void conv_pool_init(void) {
    long n = sysconf(_SC_NPROCESSORS_ONLN);
    g_nthreads = (int)(n - 2);              /* leave cores for ffmpeg/KWin */
    if (g_nthreads < 1) g_nthreads = 1;
    if (g_nthreads > MAX_CONV_THREADS) g_nthreads = MAX_CONV_THREADS;
    /* Worker threads handle jobs 1..n-1; the caller runs job 0 itself. */
    for (int i = 1; i < g_nthreads; i++)
        pthread_create(&g_pool[i], NULL, conv_worker, (void *)(intptr_t)i);
    fprintf(stderr, "[evdi-helper] NV12 conversion using %d thread(s)\n", g_nthreads);
}

static void bgra_to_nv12(const unsigned char *src, unsigned char *dst,
                         const unsigned char *dirty) {
    int ch = g_out_h / 2;
    unsigned char *ydst = dst;
    unsigned char *uvdst = dst + (size_t)g_out_w * g_out_h;
    for (int i = 0; i < g_nthreads; i++) {
        g_jobs[i] = (conv_job_t){ src, ydst, uvdst, g_mode_w, g_mode_h, g_mode_stride,
                                  g_out_w, g_out_h, g_scale,
                                  ch * i / g_nthreads, ch * (i + 1) / g_nthreads,
                                  dirty };
    }
    if (g_nthreads > 1) {
        pthread_mutex_lock(&g_pool_mtx);
        g_pool_active = g_nthreads - 1;
        g_pool_gen++;
        pthread_cond_broadcast(&g_pool_go);
        pthread_mutex_unlock(&g_pool_mtx);
    }
    if (g_jobs[0].scale > 1) convert_strip_scaled(&g_jobs[0]);
    else convert_strip(&g_jobs[0]);   /* caller does its own strip */
    if (g_nthreads > 1) {
        pthread_mutex_lock(&g_pool_mtx);
        while (g_pool_active > 0)
            pthread_cond_wait(&g_pool_done, &g_pool_mtx);
        pthread_mutex_unlock(&g_pool_mtx);
    }
}

static void mark_all_dirty(void) {
    if (!g_dirty_fill) return;
    memset(g_dirty_fill,   0xFF, (size_t)g_dirty_bytes);
    memset(g_dirty_latest, 0xFF, (size_t)g_dirty_bytes);
    memset(g_dirty_write,  0xFF, (size_t)g_dirty_bytes);
}

/* Record damaged chroma rows into every buffer's mask: each of them now lacks
   this content until it is individually reconverted. */
static void mark_damage(const struct evdi_rect *rects, int n) {
    if (!g_dirty_fill || g_chroma_rows <= 0) return;
    for (int i = 0; i < n; i++) {
        int y0 = rects[i].y1, y1 = rects[i].y2;
        if (y1 < y0) { int t = y0; y0 = y1; y1 = t; }
        /* Source rows map onto output chroma rows through the scale: one
           chroma row covers 2*scale source rows. */
        int div = 2 * g_scale;
        int c0 = y0 / div, c1 = (y1 + div - 1) / div;
        if (c0 < 0) c0 = 0;
        if (c1 > g_chroma_rows) c1 = g_chroma_rows;
        for (int cy = c0; cy < c1; cy++) {
            unsigned char bit = (unsigned char)(1u << (cy & 7));
            g_dirty_fill[cy >> 3]   |= bit;
            g_dirty_latest[cy >> 3] |= bit;
            g_dirty_write[cy >> 3]  |= bit;
        }
    }
}

/* Convert the stride-padded BGRA framebuffer into the NV12 fill buffer and
   publish it as the latest frame. Called from the event-loop thread. */
static void publish_frame(void) {
    if (!g_buffers_ready || !g_framebuffer)
        return;

    /* Only the rows this particular buffer is missing. */
    bgra_to_nv12(g_framebuffer, g_fill, g_dirty_fill);
    memset(g_dirty_fill, 0, (size_t)g_dirty_bytes);

    pthread_mutex_lock(&g_swap_mutex);
    unsigned char *tmp = g_latest;
    g_latest = g_fill;
    g_fill = tmp;
    /* The masks describe the buffers, so they travel with them. */
    unsigned char *dtmp = g_dirty_latest;
    g_dirty_latest = g_dirty_fill;
    g_dirty_fill = dtmp;
    g_latest_valid = 1;
    g_latest_grab_us = g_grab_us;
    pthread_cond_signal(&g_frame_ready);
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

    /* Stream dimensions: source divided by the scale, forced even because
       NV12 chroma covers 2x2 luma samples. */
    g_out_w = (g_mode_w / g_scale) & ~1;
    g_out_h = (g_mode_h / g_scale) & ~1;
    if (g_out_w < 2) g_out_w = 2;
    if (g_out_h < 2) g_out_h = 2;

    /* Packed buffers hold NV12 (Y plane + half-size interleaved CbCr). */
    g_packed_size = g_out_w * g_out_h * 3 / 2;
    free(g_fill);   g_fill = malloc(g_packed_size);
    free(g_latest); g_latest = malloc(g_packed_size);
    free(g_write);  g_write = malloc(g_packed_size);

    /* Fresh buffers hold nothing, so every row is stale in all of them. */
    g_chroma_rows = g_out_h / 2;
    g_dirty_bytes = (g_chroma_rows + 7) / 8;
    free(g_dirty_fill);   g_dirty_fill   = malloc((size_t)g_dirty_bytes);
    free(g_dirty_latest); g_dirty_latest = malloc((size_t)g_dirty_bytes);
    free(g_dirty_write);  g_dirty_write  = malloc((size_t)g_dirty_bytes);
    if (g_dirty_fill && g_dirty_latest && g_dirty_write)
        mark_all_dirty();

    if (!g_framebuffer || !g_fill || !g_latest || !g_write
            || !g_dirty_fill || !g_dirty_latest || !g_dirty_write) {
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
    pthread_cond_signal(&g_frame_ready);
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
        g_grab_us = now_us();
        /* Under the swap lock: the writer thread reassigns the mask pointers
           when it takes a frame, so touching them unlocked would race.
           If the driver returned more rectangles than we gave it room for,
           we cannot know what else changed — repaint everything. */
        pthread_mutex_lock(&g_swap_mutex);
        if (num_rects >= (int)(sizeof(rects) / sizeof(rects[0])))
            mark_all_dirty();
        else
            mark_damage(rects, num_rects);
        pthread_mutex_unlock(&g_swap_mutex);
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

/* Writer thread: sends the most recent frame, woken by the grabber rather than
   by a timer, and rate-limited so it never exceeds the target fps.

   The previous version free-ran on clock_nanosleep, entirely independent of
   when a frame was actually grabbed. A frame published just after a tick had to
   wait a whole period before being sent — half a frame of pure added latency on
   average, for nothing. Waiting on the condition variable removes that: the
   frame goes out as soon as it exists, and the minimum-interval check below
   still caps the rate.

   Blocking writes here never stall capture, and the FIFO is reopened
   automatically when the encoder restarts. */
static void *writer_thread(void *arg) {
    (void)arg;
    const long period_ns = 1000000000L / (g_fps > 0 ? g_fps : 60);
    struct timespec next_allowed;
    clock_gettime(CLOCK_MONOTONIC, &next_allowed);
    int have_frame = 0;
    int frame_generation = -1;

    while (g_running) {
        if (g_capture_fifo_fd < 0) {
            g_capture_fifo_fd = try_open_fifo();
            if (g_capture_fifo_fd < 0) {
                /* No reader yet — poll slowly instead of spinning. */
                struct timespec idle = { .tv_sec = 0, .tv_nsec = 50000000L };
                nanosleep(&idle, NULL);
                continue;
            }
        }

        int size;
        pthread_mutex_lock(&g_swap_mutex);
        /* Wait for a frame, but no longer than one period so shutdown and
           mode changes are still noticed promptly. */
        while (g_running && (!g_buffers_ready || !g_latest_valid)) {
            struct timespec wait_until;
            clock_gettime(CLOCK_MONOTONIC, &wait_until);
            wait_until.tv_nsec += period_ns;
            while (wait_until.tv_nsec >= 1000000000L) {
                wait_until.tv_nsec -= 1000000000L;
                wait_until.tv_sec += 1;
            }
            if (pthread_cond_timedwait(&g_frame_ready, &g_swap_mutex,
                                       &wait_until) == ETIMEDOUT)
                break;
        }
        if (!g_running) {
            pthread_mutex_unlock(&g_swap_mutex);
            break;
        }
        if (!g_buffers_ready) {
            pthread_mutex_unlock(&g_swap_mutex);
            continue;
        }
        if (frame_generation != g_mode_generation) {
            /* Buffers were reallocated; previous g_write content is gone */
            frame_generation = g_mode_generation;
            have_frame = 0;
        }
        int fresh = 0;
        if (g_latest_valid) {
            unsigned char *tmp = g_write;
            g_write = g_latest;
            g_latest = tmp;
            unsigned char *dtmp = g_dirty_write;
            g_dirty_write = g_dirty_latest;
            g_dirty_latest = dtmp;
            g_latest_valid = 0;
            g_write_grab_us = g_latest_grab_us;
            have_frame = 1;
            fresh = 1;
        }
        size = g_packed_size;
        g_writer_busy = have_frame;
        pthread_mutex_unlock(&g_swap_mutex);

        if (!have_frame)
            continue;  /* nothing grabbed yet for this mode */

        /* Nothing changed on screen: don't re-send the identical frame.
           A motionless desktop was still pushing 60 full NV12 frames a second
           through the FIFO — 8.2MB each, roughly half a gigabyte per second of
           pure memory traffic, plus an encode for every one of them, all to
           transmit no new information. The occasional keepalive keeps the
           encoder and the client's read timeout alive. */
        long long now_ms_write = now_ms();
        if (!fresh && (now_ms_write - g_last_write_ms) < IDLE_KEEPALIVE_MS)
            continue;
        g_last_write_ms = now_ms_write;

        /* Rate limit against an absolute schedule, never against "now".
           Rebasing on now would add the wait and write time to every period,
           so the stream drifts slower than the target — measured as 58fps
           against a 60fps target, with ffmpeg reporting speed=0.97x. */
        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        long long ahead_ns = (long long)(next_allowed.tv_sec - now.tv_sec) * 1000000000LL
                           + (next_allowed.tv_nsec - now.tv_nsec);
        if (ahead_ns > 0) {
            struct timespec gap = { .tv_sec = ahead_ns / 1000000000LL,
                                    .tv_nsec = ahead_ns % 1000000000LL };
            nanosleep(&gap, NULL);
        } else if (-ahead_ns > 4 * (long long)period_ns) {
            /* Fallen far behind (a stalled encoder, a mode change): resync
               instead of trying to catch up on a burst of stale frames. */
            next_allowed = now;
        }
        next_allowed.tv_nsec += period_ns;
        while (next_allowed.tv_nsec >= 1000000000L) {
            next_allowed.tv_nsec -= 1000000000L;
            next_allowed.tv_sec += 1;
        }

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

        /* Only freshly grabbed frames say anything about capture latency;
           keepalive repeats would report the age of stale content. */
        if (fresh && remaining == 0 && g_write_grab_us > 0
                && g_lat_count < LAT_SAMPLES) {
            long long d = now_us() - g_write_grab_us;
            if (d >= 0 && d < 1000000)
                g_lat_us[g_lat_count++] = (int)d;
        }
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
        /* Sleep exactly until the next capture request is due instead of a
           fixed 4ms tick. The fixed tick quantised every request to a 4ms grid,
           adding up to 4ms of jitter per frame — a quarter of the entire budget
           at 60fps, and half of it at 120.
           With no mode there is nothing to request, and the deadline below
           would sit permanently in the past — poll would return instantly and
           the loop would spin at 100% of a core. Wait on events only. */
        int timeout_ms;
        if (!g_have_mode) {
            timeout_ms = 100;
        } else {
            long long due = last_request_ms + request_period_ms;
            timeout_ms = (int)(due - now_ms());
            if (timeout_ms < 0) timeout_ms = 0;
            if (timeout_ms > 4) timeout_ms = 4;   /* stay responsive to events */
        }

        int ret = poll(fds, 1, timeout_ms);
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

            /* Capture-side latency: grab → convert → into the encoder's FIFO. */
            pthread_mutex_lock(&g_swap_mutex);
            int n = g_lat_count;
            int snapshot[LAT_SAMPLES];
            if (n > 0) memcpy(snapshot, g_lat_us, (size_t)n * sizeof(int));
            g_lat_count = 0;
            pthread_mutex_unlock(&g_swap_mutex);
            if (n > 0) {
                qsort(snapshot, (size_t)n, sizeof(int), cmp_int);
                fprintf(stderr,
                        "[evdi-helper] capture→fifo p50 %.1fms p95 %.1fms (%d frames)\n",
                        snapshot[n / 2] / 1000.0,
                        snapshot[(int)((n - 1) * 0.95)] / 1000.0, n);
            }

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
        } else if (strcmp(argv[i], "--scale") == 0 && i + 1 < argc) {
            g_scale = atoi(argv[++i]);
            if (g_scale < 1) g_scale = 1;
            if (g_scale > 4) g_scale = 4;
        } else if (strcmp(argv[i], "--fps") == 0 && i + 1 < argc) {
            g_fps = atoi(argv[++i]);
            if (g_fps < 1 || g_fps > 240) g_fps = 60;
        }
    }

    if (!edid_path) {
        fprintf(stderr, "Usage: %s --edid <edid.bin> [--capture-fifo <path>] [--fps <n>] [--scale <1-4>]\n", argv[0]);
        return 1;
    }

    {
        pthread_condattr_t ca;
        pthread_condattr_init(&ca);
        pthread_condattr_setclock(&ca, CLOCK_MONOTONIC);
        pthread_cond_init(&g_frame_ready, &ca);
        pthread_condattr_destroy(&ca);
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
        conv_pool_init();   /* spawn NV12 conversion workers before first grab */
        fprintf(stderr, "[evdi-helper] Capture FIFO: %s (opened on demand)\n", fifo_path);
        if (pthread_create(&writer, NULL, writer_thread, NULL) != 0) {
            fprintf(stderr, "[evdi-helper] Failed to start writer thread\n");
            return 1;
        }
    }

    fprintf(stderr, "[evdi-helper] Connected. Capture at %d fps. Entering event loop.\n", g_fps);
    run_event_loop(handle);

    g_running = 0;
    /* Wake the writer out of its condition wait so shutdown is immediate
       rather than up to one frame period late. */
    pthread_mutex_lock(&g_swap_mutex);
    pthread_cond_broadcast(&g_frame_ready);
    pthread_mutex_unlock(&g_swap_mutex);
    if (writer) pthread_join(writer, NULL);
    if (g_capture_fifo_fd >= 0) close(g_capture_fifo_fd);
    free(g_framebuffer);
    free(g_fill);
    free(g_latest);
    free(g_write);
    free(g_dirty_fill);
    free(g_dirty_latest);
    free(g_dirty_write);

    fprintf(stderr, "[evdi-helper] Disconnecting...\n");
    evdi_disconnect(handle);
    evdi_close(handle);
    g_handle = EVDI_INVALID_HANDLE;

    fprintf(stderr, "[evdi-helper] Done.\n");
    return 0;
}
