/* Synthetic native conversion benchmark. Only the EVDI boundary is stubbed;
 * no device, FIFO, writer, encoder or GPU is opened. See docs/evdi-performance-journey.md. */
#define main evdi_helper_main
#include "evdi_helper.c"
#undef main
#include <assert.h>

void evdi_register_buffer(evdi_handle h, struct evdi_buffer b) {(void)h; (void)b;}
void evdi_unregister_buffer(evdi_handle h, int id) {(void)h; (void)id;}
void evdi_grab_pixels(evdi_handle h, struct evdi_rect *r, int *n) {
    (void)h; r[0] = (struct evdi_rect){0, 0, g_mode_w, g_mode_h}; *n = 1;
}

static void consume(void) {
    assert(g_latest_valid);
    unsigned char *p = g_write; g_write = g_latest; g_latest = p;
    p = g_dirty_write; g_dirty_write = g_dirty_latest; g_dirty_latest = p;
    g_latest_valid = 0;
}

static double elapsed_ms(struct timespec a, struct timespec b) {
    return (b.tv_sec - a.tv_sec) * 1000.0 + (b.tv_nsec - a.tv_nsec) / 1e6;
}

static int cmp_double(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return (x > y) - (x < y);
}

static void update_source(const char *name, int frame, unsigned char **texture) {
    if (!strcmp(name, "full"))
        memset(g_framebuffer, (frame & 1) ? 255 : 0, (size_t)g_fb_size);
    else if (!strcmp(name, "noise"))
        memcpy(g_framebuffer, texture[frame & 1], (size_t)g_fb_size);
    else if (!strcmp(name, "bar")) {
        int x = g_mode_w - 256 + frame % 240;
        int old_x = g_mode_w - 256 + (frame + 239) % 240;
        for (int y = 0; y < g_mode_h; y++) {
            memset(g_framebuffer + (size_t)y*g_mode_stride + old_x*4, 0, 16*4);
            memset(g_framebuffer + (size_t)y*g_mode_stride + x*4, 255, 16*4);
        }
    } else if (!strcmp(name, "latepixel"))
        g_framebuffer[(size_t)(g_mode_h-1)*g_mode_stride + (g_mode_w-1)*4 + g_off_r] = (frame & 1) ? 255 : 0;
    else assert(!strcmp(name, "static"));
}

static int integer_arg(const char *text, int maximum) {
    char *end;
    errno = 0;
    long value = strtol(text, &end, 10);
    return errno || *end || end == text || value < 1 || value > maximum ? 0 : (int)value;
}

int main(int argc, char **argv) {
    if (argc != 7) {
        fprintf(stderr, "Usage: %s full|noise|bar|latepixel|static WIDTH HEIGHT FRAMES THREADS verify|measure\n", argv[0]);
        return 2;
    }
    const char *name = argv[1];
    int width = integer_arg(argv[2], 2560), height = integer_arg(argv[3], 1600);
    int frames = integer_arg(argv[4], 1000), requested_threads = integer_arg(argv[5], 8);
    int verify = !strcmp(argv[6], "verify");
    if ((!verify && strcmp(argv[6], "measure")) || !frames ||
        !(requested_threads == 1 || requested_threads == 2 || requested_threads == 4 || requested_threads == 8) ||
        !(!strcmp(name, "full") || !strcmp(name, "noise") || !strcmp(name, "bar") ||
          !strcmp(name, "latepixel") || !strcmp(name, "static")) ||
        !((width == 1536 && height == 960) || (width == 2560 && height == 1600))) return 2;
    const int warmup = verify ? 0 : 24;
    pthread_condattr_t attr;
    assert(!pthread_condattr_init(&attr));
    assert(!pthread_condattr_setclock(&attr, CLOCK_MONOTONIC));
    assert(!pthread_cond_init(&g_frame_ready, &attr));
    assert(!pthread_condattr_destroy(&attr));
    g_conv_thread_limit = requested_threads;
    conv_pool_init();
    assert(g_nthreads >= 1 && g_nthreads <= requested_threads);
    g_scale = 1;
    on_mode_changed((struct evdi_mode){.width=width, .height=height,
        .refresh_rate=60, .bits_per_pixel=32,
        .pixel_format=FOURCC('X','R','2','4')}, NULL);
    consume();
    memset(g_framebuffer, 0, (size_t)g_fb_size);
    grab_now(); consume();

    unsigned char *texture[2] = {NULL, NULL};
    if (!strcmp(name, "noise")) {
        uint32_t seed = 20261105;
        for (int n = 0; n < 2; n++) {
            texture[n] = malloc((size_t)g_fb_size);
            assert(texture[n]);
            for (int i = 0; i < g_fb_size; i++) {
                seed = seed * 1664525u + 1013904223u;
                texture[n][i] = (unsigned char)(seed >> 24);
            }
        }
    }
    unsigned char *reference = verify ? malloc((size_t)g_packed_size) : NULL;
    unsigned char *saved_reference = verify ? malloc((size_t)g_fb_size) : NULL;
    assert(!verify || (reference && saved_reference && g_reference_frame));
    double *latency = calloc((size_t)frames, sizeof(double));
    assert(latency);
    double wall_ms = 0, cpu_ms = 0, update_wall_ms = 0;
    long long compared_bytes = 0;
    int published = 0, compared_frames = 0, budget_misses = 0;
    for (int i = 0; i < frames + warmup; i++) {
        struct timespec u0, u1, w0, w1, c0, c1;
        clock_gettime(CLOCK_MONOTONIC, &u0);
        update_source(name, i, texture);
        clock_gettime(CLOCK_MONOTONIC, &u1);
        clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &c0);
        clock_gettime(CLOCK_MONOTONIC, &w0);
        grab_now();
        clock_gettime(CLOCK_MONOTONIC, &w1);
        clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &c1);
        int did_publish = g_latest_valid;
        assert(did_publish == ((strcmp(name, "static") && i > 0) || !strcmp(name, "noise") || !strcmp(name, "bar")));
        if (did_publish) consume();
        if (verify) {
            /* Compare every byte to real one-thread full conversion, preserving
             * the incremental reference cache so verification cannot repair it. */
            memcpy(saved_reference, g_reference_frame, (size_t)g_fb_size);
            int threads = g_nthreads;
            g_nthreads = 1;
            bgra_to_nv12(g_framebuffer, reference, NULL);
            g_nthreads = threads;
            memcpy(g_reference_frame, saved_reference, (size_t)g_fb_size);
            if (memcmp(reference, g_write, (size_t)g_packed_size)) {
                fprintf(stderr, "BYTE_MISMATCH case=%s frame=%d threads=%d\n", name, i, threads);
                return 3;
            }
            compared_bytes += g_packed_size;
            compared_frames++;
        }
        if (i < warmup) continue;
        double duration = elapsed_ms(w0, w1);
        latency[i-warmup] = duration;
        wall_ms += duration;
        cpu_ms += elapsed_ms(c0, c1);
        update_wall_ms += elapsed_ms(u0, u1);
        published += did_publish;
        budget_misses += duration > 16.7;
    }
    qsort(latency, (size_t)frames, sizeof(double), cmp_double);
    printf("{\"case\":\"%s\",\"width\":%d,\"height\":%d,\"frames\":%d,\"warmup\":%d,"
           "\"published\":%d,\"threads\":%d,\"requested_threads\":%d,\"verify\":%s,\"compared_frames\":%d,"
           "\"compared_bytes\":%lld,\"byte_errors\":0,\"wall_ms_frame\":%.9f,"
           "\"process_cpu_ms_frame\":%.9f,\"source_update_wall_ms_frame\":%.9f,"
           "\"wall_p50_ms\":%.9f,\"wall_p95_ms\":%.9f,\"wall_p99_ms\":%.9f,"
           "\"wall_max_ms\":%.9f,\"over_16_7_ms\":%d}\n",
           name, width, height, frames, warmup, published, g_nthreads, requested_threads, verify ? "true" : "false",
           compared_frames, compared_bytes, wall_ms/frames, cpu_ms/frames, update_wall_ms/frames,
           latency[(frames-1)/2], latency[(frames*95+99)/100-1], latency[(frames*99+99)/100-1],
           latency[frames-1], budget_misses);
    pthread_mutex_lock(&g_pool_mtx);
    g_pool_shutdown = 1;
    pthread_cond_broadcast(&g_pool_go);
    pthread_mutex_unlock(&g_pool_mtx);
    for (int i = 1; i < g_nthreads; i++) assert(!pthread_join(g_pool[i], NULL));
    free(texture[0]); free(texture[1]); free(reference); free(saved_reference); free(latency);
    return 0;
}
