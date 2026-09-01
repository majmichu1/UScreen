# Benchmarks

Numbers measured by the project, with the method, so they can be reproduced
or argued with. They are from one machine and one tablet; other hardware will
differ.

## Test configuration

| | |
| --- | --- |
| Date | 2026-08-26 to 2026-08-31 |
| UScreen | 1.0.0 – 1.1.0 |
| Host | Laptop, NVIDIA GeForce RTX 5060 Laptop GPU, Bazzite (Fedora Atomic) with KDE Plasma 6 on Wayland, kernel 7.2 |
| Tablet | Samsung Galaxy Tab S9 Ultra (Snapdragon 8 Gen 2), 2960×1848 @ 90 Hz |
| Link | USB-C cable, adb over USB; Wi-Fi tests on a 5 GHz (tablet) / 6 GHz (host) link to the same router, RSSI −37 / −44 dBm |
| Stream | 2960×1848, 90 fps target, constant-quality VBR (`quality = 12`, `bitrate` cap 60 Mbps), H.264 NVENC unless stated |
| Desktop content | a mostly static desktop with occasional window movement; bitrate on a static desktop settles around 0.5–4 Mbps |

## How latency is measured

The daemon stamps every encoded frame with a sequence number and starts a
clock. The app hands the sequence number to the decoder as the presentation
timestamp and, when Android's `onFrameRendered` callback fires for that frame,
sends it back over the input socket with the time it spent on the tablet
(arrival → on screen). The daemon then reports, every five seconds:

- **encode→display**: total time from the frame leaving the encoder to being
  on the tablet's screen, on the host's clock — no clock synchronisation
  needed;
- the tablet's **decode+render** share of that, and the remainder as **wire**.

It does not include capture (compositor → encoder), which the helper reports
separately (`capture→fifo`, typically 8–20 ms at 90 Hz and dominated by
waiting for the next compositor frame).

## Results

### End-to-end, USB

| codec | p50 | p95 | notes |
| --- | --- | --- | --- |
| H.264 (NVENC) | 18–22 ms | 23–31 ms | default |
| HEVC (NVENC) | 15–18 ms | 20–23 ms | tablet has a dedicated low-latency HEVC decoder |
| HEVC Main10 (10-bit) | 16–17 ms | 19–22 ms | no measurable cost over 8-bit |

Split of the ~22 ms H.264 figure: tablet decode+render ≈ 15 ms, USB/adb hop
≈ 5–7 ms, encoder < 1 ms. The tablet's decoder is ~7–8 ms fixed plus ~1.2 ms
per megapixel, which is why `stream_scale = 2` (a quarter of the pixels)
brought p50 from 22 ms to 16 ms at the cost of softer text.

### USB vs Wi-Fi (H.264, quiet link)

| | USB | Wi-Fi, no radio lock | Wi-Fi, with lock (1.0.0+) |
| --- | --- | --- | --- |
| p50 (median of 5-second windows) | 22.0 ms | 32.0 ms | 22.8 ms |
| p95 (median of windows) | 25.3 ms | 113.7 ms | 78.6 ms |
| windows with p95 < 60 ms | 7/7 | 14/73 | 10/31 |
| worst single frame | 32 ms | 5775 ms | 2546 ms |

The app's low-latency Wi-Fi lock fixed the median (Android was dozing the
radio between frames). The tail is the wireless medium and the router, and no
signal strength fixes it — these figures are from a link with none of the
usual excuses. Wi-Fi stays a fallback.

### Host CPU

| encoder path | pipeline CPU (helper + encoder) |
| --- | --- |
| ffmpeg child process (default) | ~190 % of a core |
| in-process libavcodec (`--features inproc-encoder`) | ~97 % |

Latency identical either way; the encoder is not on the critical path.

Capture helper while the output is disabled (tablet unplugged, or graphics-
tablet mode): 96 % of a core before 0.4.0 (a poll loop with a deadline in the
past), 1.6 % after.

## Limitations

- One host, one tablet model. The tablet's decoder dominates the budget, so
  other tablets will land elsewhere; a Snapdragon 8 Gen 2 is a fast one.
- The wire figure lumps adb, USB and the app's socket read together.
- "Windows with p95 < 60 ms" is a coarse stutter indicator, not a standard.
- No measurement yet of AMD/Intel VAAPI encoders or of libx264.

Reports with other hardware are welcome as
[compatibility issues](https://github.com/majmichu1/UScreen/issues/new?template=compatibility.yml);
the daemon's `Latency encode→display` log line is all it takes.
