/* No DRM/X11 connection: only the kernel-facing buffer/grab boundary is fake.
   Conversion, mode handling, triple buffers and the FIFO writer are real. */
#define main evdi_helper_main
#include "evdi_helper.c"
#undef main
#include <assert.h>
#include <sys/resource.h>

static int grabs;

void evdi_register_buffer(evdi_handle handle, struct evdi_buffer buffer) {
    (void)handle; (void)buffer;
}

void evdi_unregister_buffer(evdi_handle handle, int id) {
    (void)handle; (void)id;
}

void evdi_grab_pixels(evdi_handle handle, struct evdi_rect *rects, int *n) {
    (void)handle;
    grabs++;
    rects[0] = (struct evdi_rect){0, 0, g_mode_w, g_mode_h};
    *n = 1;  /* reproduce full damage even when the pixels are unchanged */
}

static void mode(int w, int h, unsigned fmt) {
    on_mode_changed((struct evdi_mode){
        .width = w, .height = h, .refresh_rate = 60,
        .bits_per_pixel = 32, .pixel_format = fmt,
    }, NULL);
}

/* Consume the published frame through the same pointer/mask ownership boundary
   as the writer, without depending on thread scheduling in these checks. */
static void consume(void) {
    assert(g_latest_valid);
    unsigned char *tmp = g_write; g_write = g_latest; g_latest = tmp;
    tmp = g_dirty_write; g_dirty_write = g_dirty_latest; g_dirty_latest = tmp;
    g_latest_valid = 0;
}

static void black_frame(void) {
    memset(g_framebuffer, 0, (size_t)g_fb_size);
    grab_now();
    consume();
}

static void static_damage(void) {
    mode(4, 4, FOURCC('X','R','2','4'));
    black_frame();
    int before = grabs;
    for (int i = 0; i < 120; i++) {
        grab_now();
        assert(!g_latest_valid && "identical full damage must stay idle");
    }
    assert(grabs == before + 120); /* every EVDI grab still gets serviced */
}

static void padding_and_unused_channel(void) {
    const unsigned formats[] = {
        FOURCC('X','R','2','4'), FOURCC('X','B','2','4'),
        FOURCC('R','X','2','4'), FOURCC('B','X','2','4'),
    };
    for (size_t f = 0; f < sizeof(formats) / sizeof(formats[0]); f++) {
        mode(4, 4, formats[f]);
        black_frame();
        int unused = 6 - g_off_r - g_off_g - g_off_b;
        for (int y = 0; y < g_mode_h; y++) {
            unsigned char *row = g_framebuffer + y * g_mode_stride;
            memset(row + 16, 0xff, (size_t)g_mode_stride - 16);
            for (int x = 0; x < g_mode_w; x++) row[x * 4 + unused] = 0xff;
        }
        grab_now();
        assert(!g_latest_valid && "padding/unused bytes are not visible changes");
        g_framebuffer[g_off_r] = 255;
        grab_now();
        assert(g_latest_valid && "a real pixel must wake capture immediately");
        consume();
        assert(g_write[0] == 63); /* limited-range BT.709 red, independently fixed */
        assert(g_write[1] == 16); /* its black neighbour must remain black */
        g_framebuffer[g_off_g] = 255;
        grab_now(); consume();
        assert(g_write[0] == 219); /* yellow */
        g_framebuffer[g_off_b] = 255;
        grab_now(); consume();
        assert(g_write[0] == 235); /* white */
    }
}

static void comparison_boundaries(void) {
    const int widths[] = {2, 14, 18, 34, 66};
    const unsigned formats[] = {FOURCC('X','R','2','4'), FOURCC('X','B','2','4'),
                                FOURCC('R','X','2','4'), FOURCC('B','X','2','4')};
    for (size_t f = 0; f < sizeof(formats) / sizeof(formats[0]); f++) {
        for (size_t n = 0; n < sizeof(widths) / sizeof(widths[0]); n++) {
            mode(widths[n], 4, formats[f]);
            black_frame();
            int channels[] = {g_off_r, g_off_g, g_off_b};
            int unused = 6 - g_off_r - g_off_g - g_off_b;
            for (int y = 1; y < 4; y += 2) {
                for (int x = 0; x < widths[n]; x++) {
                    for (int c = 0; c < 3; c++) {
                        unsigned char *pixel = g_framebuffer + y * g_mode_stride + x * 4;
                        pixel[channels[c]] = 1;
                        grab_now(); consume(); /* even one RGB bit must be noticed */
                        pixel[unused] ^= 0xff;
                        grab_now();
                        assert(!g_latest_valid);
                        pixel[channels[c]] = 0;
                        grab_now(); consume();
                    }
                }
            }
            if (widths[n] >= 4) {
                unsigned char *row = g_framebuffer + 3 * g_mode_stride;
                row[4 + g_off_r] = row[8 + g_off_r] = 1;
                grab_now(); consume(); /* equal differences must not cancel */
                row[4 + g_off_r] = row[8 + g_off_r] = 0;
                grab_now(); consume();
            }
        }
    }
}

static void buffer_rotation_and_mode(void) {
    mode(4, 4, FOURCC('X','R','2','4'));
    black_frame();
    for (int frame = 0; frame < 12; frame++) {
        int pixel = frame % 16;
        g_framebuffer[(pixel / 4) * g_mode_stride + (pixel % 4) * 4 + g_off_r] = 255;
        grab_now();
        consume();
        for (int i = 0; i < 16; i++)
            assert(g_write[i] == (i <= frame ? 63 : 16));
        grab_now();
        assert(!g_latest_valid);
    }
    mode(4, 4, FOURCC('X','B','2','4')); /* same bytes, different RGB meaning */
    grab_now();
    consume();
    assert(g_write[0] == 32); /* formerly red bytes now describe blue */
    mode(8, 6, FOURCC('X','R','2','4'));
    consume();
    black_frame();
    grab_now();
    assert(!g_latest_valid);
}

static void scaled_pixels(void) {
    g_scale = 2;
    mode(8, 8, FOURCC('X','R','2','4'));
    consume();
    black_frame();
    g_framebuffer[7 * g_mode_stride + 7 * 4 + g_off_r] = 255;
    grab_now();
    consume();
    assert(g_write[15] == 28); /* one red source pixel in a 2x2 black block */
    grab_now();
    assert(!g_latest_valid);
    g_scale = 1;
}

static void lagging_writer(void) {
    mode(8, 10, FOURCC('X','R','2','4'));
    consume();
    black_frame();
    for (int y = 0; y < 10; y++) {
        g_framebuffer[y * g_mode_stride + g_off_r] = 255;
        grab_now();
        grab_now(); /* duplicate must preserve the still-pending latest frame */
    }
    consume();
    for (int y = 0; y < 10; y++)
        for (int x = 0; x < 8; x++)
            assert(g_write[y * 8 + x] == (x == 0 ? 63 : 16));
}

static void vectorized_colour_blocks(void) {
    const unsigned formats[] = {FOURCC('X','R','2','4'), FOURCC('X','B','2','4'),
                                FOURCC('R','X','2','4'), FOURCC('B','X','2','4')};
    const unsigned char luma[] = {63, 172, 32};
    const unsigned char cb[] = {103, 41, 240}, cr[] = {240, 26, 118};
    for (int scale = 1; scale <= 4; scale++) {
        for (size_t f = 0; f < sizeof(formats) / sizeof(formats[0]); f++) {
            g_scale = scale;
            mode(66 * scale, 34 * scale, formats[f]);
            memset(g_framebuffer, 0, (size_t)g_fb_size);
            for (int y = 0; y < g_mode_h; y++) {
                for (int x = 0; x < g_mode_w; x++) {
                    int colour = (x / (2 * scale) + y / (2 * scale)) % 3;
                    int channel = colour == 0 ? g_off_r : colour == 1 ? g_off_g : g_off_b;
                    g_framebuffer[y * g_mode_stride + x * 4 + channel] = 255;
                }
            }
            grab_now(); consume();
            for (int y = 0; y < 34; y++)
                for (int x = 0; x < 66; x++)
                    assert(g_write[y * 66 + x] == luma[(x / 2 + y / 2) % 3]);
            for (int cy = 0; cy < 17; cy++) {
                for (int x = 0; x < 66; x += 2) {
                    int colour = (x / 2 + cy) % 3;
                    assert(g_write[66 * 34 + cy * 66 + x] == cb[colour]);
                    assert(g_write[66 * 34 + cy * 66 + x + 1] == cr[colour]);
                }
            }
            grab_now();
            assert(!g_latest_valid);
        }
    }
    g_scale = 1;
}

static long writer_sleeps;

static void *measured_writer(void *arg) {
    struct rusage before, after;
    assert(getrusage(RUSAGE_THREAD, &before) == 0);
    void *result = writer_thread(arg);
    assert(getrusage(RUSAGE_THREAD, &after) == 0);
    writer_sleeps = after.ru_nvcsw - before.ru_nvcsw;
    return result;
}

static void fifo_keepalive(void) {
    mode(4, 4, FOURCC('X','R','2','4'));
    black_frame();
    int fds[2];
    assert(pipe2(fds, O_NONBLOCK | O_CLOEXEC) == 0);
    g_capture_fifo_fd = fds[1];
    g_last_write_ms = 0;
    g_running = 1;
    grab_now(); /* with no publication, the writer needs the next real change */
    g_framebuffer[g_off_r] = 255;
    grab_now();
    pthread_t writer;
    assert(pthread_create(&writer, NULL, measured_writer, NULL) == 0);
    unsigned char bytes[4096];
    int total = 0;
    for (int i = 0; i < 40; i++) {
        grab_now();
        usleep(16667);
        ssize_t n;
        while ((n = read(fds[0], bytes, sizeof(bytes))) > 0) total += (int)n;
    }
    int frames = total / g_packed_size;
    assert(total % g_packed_size == 0);
    assert(frames >= 2 && "idle must still supply decoder keepalive frames");
    assert(frames <= 8 && "constant false damage must not drive a 60fps stream");

    memset(g_framebuffer, 255, (size_t)g_fb_size);
    long long started = now_ms();
    grab_now();
    int changed = 0;
    while (!changed && now_ms() - started < 300) {
        struct pollfd pfd = {.fd = fds[0], .events = POLLIN};
        if (poll(&pfd, 1, 20) > 0) {
            ssize_t n = read(fds[0], bytes, sizeof(bytes));
            for (ssize_t i = 0; i + g_packed_size <= n; i += g_packed_size)
                if (bytes[i] == 235) changed = 1; /* real white output */
        }
    }
    assert(changed && now_ms() - started < 100);
    printf("FIFO: %d frames during 40 unchanged grabs; real change in %lld ms\n",
           frames, now_ms() - started);

    /* An idle writer must release its buffer claim before a mode switch;
       otherwise the event loop waits a second and abandons the old buffers. */
    usleep(50000);
    long long mode_started = now_ms();
    mode(6, 4, FOURCC('X','R','2','4'));
    long long mode_ms = now_ms() - mode_started;
    fprintf(stderr, "FIFO: mode change after idle took %lld ms\n", mode_ms);
    assert(mode_ms < 500 && "idle writer must not stall mode changes");
    /* Reads may combine old white frames and several new gray frames. */
    int gray_bytes = 0;
    long long gray_started = now_ms();
    while (gray_bytes < 36 && now_ms() - gray_started < 500) {
        struct pollfd new_mode = {.fd = fds[0], .events = POLLIN};
        if (poll(&new_mode, 1, 20) <= 0) continue;
        ssize_t n = read(fds[0], bytes, sizeof(bytes));
        for (ssize_t i = 0; i < n && gray_bytes < 36; i++) {
            if (!gray_bytes && bytes[i] != 37) continue;
            assert(bytes[i] == (gray_bytes < 24 ? 37 : 128)); /* 6x4 gray NV12 */
            gray_bytes++;
        }
    }
    assert(gray_bytes == 36);

    g_running = 0;
    pthread_mutex_lock(&g_swap_mutex);
    pthread_cond_broadcast(&g_frame_ready);
    pthread_mutex_unlock(&g_swap_mutex);
    pthread_join(writer, NULL);
    fprintf(stderr, "FIFO: writer voluntarily slept %ld times\n", writer_sleeps);
    assert(writer_sleeps <= 20 && "idle writer must wait for keepalive, not every frame tick");
    close(fds[0]); close(fds[1]);
    g_capture_fifo_fd = -1;
}

static void no_reference_memory(void) {
    mode(4, 4, FOURCC('X','R','2','4'));
    black_frame();
    free(g_reference_frame);
    g_reference_frame = NULL; /* optional cache unavailable, as after malloc failure */
    grab_now(); consume();
    assert(g_write[0] == 16);
    g_framebuffer[g_off_r] = 255;
    grab_now(); consume();
    assert(g_write[0] == 63);
}

int main(int argc, char **argv) {
    pthread_condattr_t attr;
    pthread_condattr_init(&attr);
    pthread_condattr_setclock(&attr, CLOCK_MONOTONIC);
    pthread_cond_init(&g_frame_ready, &attr);
    pthread_condattr_destroy(&attr);
    if (argc > 1 && (strcmp(argv[1], "pool") == 0 || strcmp(argv[1], "pool2") == 0)) {
        if (strcmp(argv[1], "pool2") == 0) g_conv_thread_limit = 2;
        conv_pool_init();
        assert(g_nthreads <= g_conv_thread_limit);
    }
    if (argc < 2 || strcmp(argv[1], "writer") != 0) {
        static_damage();
        padding_and_unused_channel();
        comparison_boundaries();
        buffer_rotation_and_mode();
        scaled_pixels();
        lagging_writer();
        vectorized_colour_blocks();
    }
    fifo_keepalive();
    no_reference_memory();
    pthread_mutex_lock(&g_pool_mtx);
    g_pool_shutdown = 1;
    pthread_cond_broadcast(&g_pool_go);
    pthread_mutex_unlock(&g_pool_mtx);
    for (int i = 1; i < g_nthreads; i++) pthread_join(g_pool[i], NULL);
    puts("PASS: false damage stays idle; RGB changes, buffer rotation, modes and scaling remain correct");
    return 0;
}
