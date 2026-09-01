# Architecture

## Pipeline

1. **Virtual display.** The daemon starts a small C helper that opens an EVDI
   device (`/dev/dri/cardN` provided by the `evdi` kernel module) and presents
   a generated EDID with the tablet's resolution and physical size. KWin sees a
   new monitor and starts rendering to it; the daemon enables it and places it
   next to the real screens through `kscreen-doctor`.
2. **Capture.** The helper runs an event-driven `request_update` / `grab_pixels`
   cycle at the target frame rate, converts BGRA to NV12 (BT.709, limited
   range) and writes whole frames into a FIFO in the per-user runtime
   directory. Only rows the compositor reported as damaged are converted.
3. **Encode.** `ffmpeg` (or libavcodec in-process) encodes with NVENC, VAAPI or
   libx264 in constant-quality mode, no B-frames, no lookahead, one keyframe
   per second plus keyframes on demand when a client joins.
4. **Stream.** A TCP server on loopback sends length-prefixed Annex B access
   units; `adb reverse` carries the port to the tablet over USB. A client that
   falls behind is skipped forward to the newest keyframe rather than fed a
   backlog.
5. **Decode.** The Android app feeds the stream to a MediaCodec hardware
   decoder rendering straight to a SurfaceView, low-latency mode where the
   device offers it.
6. **Input.** Touch and pen events go back over a WebSocket on a second port
   and are injected through three uinput devices (touchscreen, pen tablet with
   pressure/tilt/eraser/button, absolute pointer). On KDE the daemon maps
   these devices onto the virtual output over KWin's D-Bus interface so
   coordinates land on the right screen.
7. **Latency loop.** Every frame carries a sequence number; the app echoes it
   when the frame reaches the screen, and the daemon logs p50/p95 end-to-end.

## Processes

- `uscreen` — the daemon: adb monitor, per-tablet sessions, tray icon, config.
- `evdi_helper` — one per tablet slot, owns one EVDI card.
- `ffmpeg` — one per slot (unless built with the in-process encoder).
- `uscreen-gui` — optional settings window, talks to the daemon through the
  config file and the PID file.

## Protocol

**Video (TCP, loopback, port 8890 + 2·slot).** The client first sends the
64-character hex session token. Then the server sends packets of
`u32 length (big-endian)`, `u8 type`, payload: type 0 is codec configuration
(SPS/PPS, plus VPS for HEVC), type 1 is a frame with a `u32` sequence number
before the Annex B data.

**Input (WebSocket, loopback, port 8891 + 2·slot).** JSON messages. The first
must be `{"type":"auth","token":"…"}`; the server then replies with a greeting
`{"status":"connected","width":…,"height":…,"codec":"h264"|"hevc","pen_only":bool}`
and repeats it whenever the mode changes. Client messages:

```json
{"type":"touch","x":0.5,"y":0.3,"pressure":1.0,"action":0,"slot":0}
{"type":"pen","x":0.5,"y":0.3,"pressure":0.8,"tilt_x":12.0,"tilt_y":-3.0,"eraser":false,"action":2}
{"type":"resolution","width":2960,"height":1848,"width_mm":314,"height_mm":195}
{"type":"config","bitrate":20000,"fps":60}
{"type":"mode","pen_only":true}
{"type":"rendered","seq":1234,"decode_us":14200}
```

Coordinates are normalised 0–1; tilt is in degrees; touch actions are
0 down, 1 up, 2 move; pen actions add 3 hover, 4 hover exit, 5/6 stylus
button down/up.

## Security model

Loopback-only ports, a per-run random token required before any data flows,
private runtime directory for the FIFO and token, no network traffic except an
optional update check. Details and threat model in [SECURITY.md](../SECURITY.md).
