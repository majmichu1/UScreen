# UScreen — USB Second Screen for Linux

Turn your Android tablet into a low-latency secondary display for Linux, connected via USB.

Inspired by SuperDisplay for Windows. Built for Linux (Bazzite, Fedora, Arch, etc.).

> **Status:** tested on Bazzite (KDE Plasma, NVIDIA) with a Samsung Galaxy Tab S9 Ultra.
> It should work with any Android 8+ tablet (auto-resolution) and any distro with the
> `evdi` kernel module — reports and PRs welcome!

## Features

- **Plug and play**: the daemon watches for the tablet over ADB — plug in the USB cable and the app launches on the tablet automatically
- **Any tablet**: the app reports its screen size and the virtual display is generated to match (auto-resolution), or pick a custom resolution in the GUI
- **Low latency**: event-driven EVDI capture, hardware encoding, IDR-aware frame skipping — the pipeline never lets latency accumulate
- **Settings UI on both sides**: a desktop GUI (`uscreen-gui`) on Linux, and a settings sheet in the Android app (bitrate / fps changes apply live, no reconnect needed)
- **Touch & S-Pen** forwarded back to Linux with pressure and tilt
- **Tray icon** on the desktop: connection state at a glance, and the mode switch without opening anything
- **Either side of the desktop**: the virtual screen can sit right, left, above or below your real ones
- **Wi-Fi fallback** when the cable is not an option, with the cost stated rather than glossed over

## Install (releases)

**Linux:** download `uscreen-*-linux-x86_64.tar.gz` from the releases page, extract, run:

```bash
./scripts/install.sh
```

This installs the dependencies for your distro, the binaries, a desktop entry
("UScreen" in your app menu) and does the one-time system setup. The GUI also
detects a missing setup and offers to fix it with one click.

**Android:** download `uscreen.apk` from the releases page onto the tablet and
open it (allow installing from unknown sources). Works on Android 8+.

## How it works

1. A **virtual display** is created on the Linux host via EVDI (no dummy plug needed)
2. The EVDI helper captures the virtual display framebuffer (event-driven `request_update`/`grab_pixels` cycle at the target fps)
3. The raw frames are piped to `ffmpeg` for **hardware-accelerated encoding** (NVENC on NVIDIA, VAAPI on AMD/Intel, or libx264)
4. Encoded H.264 frames are streamed over **USB** via ADB reverse tunnel
5. The Android app decodes and displays the stream using **hardware decoding** (MediaCodec)
6. Touch and S-Pen events are sent back over WebSocket and injected via **uinput**

## Prerequisites

- **Linux** with Wayland (KDE Plasma recommended) or X11
- **Android tablet** (Samsung Galaxy Tab S9 Ultra tested, any Android 8+ works)
- **USB cable** (USB 3.0+ for best performance)
- **EVDI kernel module** (`sudo modprobe evdi`)
- **ffmpeg** with your preferred encoder

## Quick Start

### 1. Install dependencies

```bash
# Run the installer:
./scripts/install.sh

# Or manually:
# Bazzite/Fedora:
sudo dnf install ffmpeg android-tools evdi-dkms libevdi-devel libdrm-devel

# Ubuntu/Debian:
sudo apt install ffmpeg android-tools-adb evdi-dkms libevdi-dev libdrm-dev

# Arch:
sudo pacman -S ffmpeg android-tools evdi
```

### 2. Build

```bash
make build
```

This builds both the EVDI helper (C) and the Rust daemon.

### 3. Install (optional)

```bash
make install
```

Copies binaries to `~/.local/bin/` and installs the systemd user service.

### 4. Install the Android app

Open the `android/` directory in Android Studio and build the APK, or:

```bash
cd android
./gradlew assembleDebug
adb install app/build/outputs/apk/debug/app-debug.apk
```

### 5. Connect

```bash
# 1. Enable USB debugging on your tablet (Settings → Developer Options)
# 2. Have UScreen start with your desktop, so plugging in is all it takes
#    (or tick "Start UScreen with the desktop" in uscreen-gui):
systemctl --user enable --now uscreen.service
# ...or start it just for this session:
uscreen start
# 3. Plug in the USB-C cable — that's it.
#    The daemon forwards the ADB ports and launches the app on the tablet.
```

If auto-forward fails, run manually:
```bash
adb reverse tcp:8890 tcp:8890
adb reverse tcp:8891 tcp:8891
```

## Settings

Settings live in `~/.config/uscreen/config.toml` and can be changed from three places:

- **`uscreen-gui`** — desktop app with status (daemon / tablet), start/stop, and all settings
- **The tablet app** — tap the ⚙ handle in the top-right corner; bitrate and fps apply live
- **CLI flags** — override the config file for one run (e.g. `uscreen --bitrate 30000 start`)
- **The tray icon** — mode switch, settings, quit

### Where the screen sits

`position` places the virtual screen `right` (default), `left`, `above` or
`below` everything else. Above and left move the rest of the desktop to make
room, in one atomic reconfiguration, because KDE will not accept a negative
position — it reports success and quietly leaves the output disabled.

### Codec

H.264 is the default because every device decodes it. If your tablet has a
hardware HEVC decoder — `uscreen doctor` will tell you — `hevc_nvenc` is worth
switching to: sharper text at the same bitrate, and on a Tab S9 Ultra it
measured slightly *faster* than H.264 (p50 15-18ms against 18-22ms), because
the tablet has a dedicated low-latency HEVC decoder.

With HEVC you can also turn on `ten_bit`, which encodes Main10. Be clear about
what that is and is not: **the captured desktop is 8-bit and cannot be
otherwise** — EVDI hands over ARGB8888 and there is no 10-bit path below us —
so this adds no colour. What it buys is precision in the encoder's own
arithmetic, which smooths the banding that shows on gradients at low bitrates.
It is not HDR, and it is not a step towards it while the capture stays 8-bit.
Measured cost on the tablet: none.

### Over Wi-Fi

The pipeline is not tied to USB — it speaks to whatever adb is connected to.
So `adb tcpip 5555` followed by `adb connect <tablet-ip>:5555` is all it takes,
and the daemon prefers the cable automatically whenever both are available.

It is a fallback, and the numbers say why. Measured on a quiet 5GHz/6GHz link
with excellent signal on both ends:

| | USB | Wi-Fi |
| --- | --- | --- |
| median latency | 22.0ms | 22.8ms |
| median p95 | 25.3ms | 78.6ms |
| worst frame seen | 32ms | 2546ms |

The median is fine. The tail is not, and no amount of signal strength fixes it
— those figures are already from a link with none of the usual excuses. Use it
when the cable is not an option, not instead of the cable.

(The app holds a low-latency Wi-Fi lock while streaming. Without it the median
was 32.0ms rather than 22.8ms: Android dozes the radio between frames, and a
stream of small packets sixty times a second is the traffic pattern power save
handles worst.)

### On-screen keyboard

The tablet's touch device is a real touchscreen as far as the desktop is
concerned, so KDE would pop its virtual keyboard up over whatever you are
working on. The daemon turns it off while it runs and puts the setting back on exit —
including after being killed rather than stopped, since the previous value is
saved to disk rather than kept in memory.

Worth knowing if you go looking: writing `VirtualKeyboardMode` into `kwinrc`
does not work. KWin does not re-read that file, so the value on disk and the
one actually in force disagree, and the keyboard keeps appearing. The property
on `org.kde.kwin.VirtualKeyboard` over D-Bus is what takes effect.

### Graphics tablet mode

The tablet stops being a second screen and becomes a drawing surface for the
screen you are already looking at, like a Wacom Intuos. Nothing is captured,
encoded or streamed, and the pen is mapped onto your own display instead of the
virtual one.

For drawing this removes display latency from the loop entirely — you watch the
host's screen, which has none — and with nothing being captured or encoded the
pipeline costs next to nothing. Pressure, tilt, the eraser and the stylus
button all work exactly as they do in display mode.

**Switch it from the tablet.** Tap the ⚙ in the corner and flip *Graphics
tablet*; the change takes effect immediately, with no restart and no trip to
the computer. The host tears the virtual display down, moves the pen onto your
own screen and stops encoding — and puts it all back when you switch off. The
mode is remembered, so the tablet comes back the way you left it.

It is also settable from the host, as a checkbox in `uscreen-gui` or with
`--pen-only`. The flag applies to that run only; a switch made from the tablet
is a deliberate choice and is written to the config file.

### Stream detail vs latency

The desktop always runs at full resolution. `stream_scale` controls only what
is sent to the tablet: at `2` a quarter of the pixels are transmitted and the
tablet upscales them by a whole number. The tablet's decoder costs roughly
7-8 ms fixed plus 1.2 ms per megapixel, so fewer pixels reach the screen
sooner — worth it for games, softer for text.

### Resolution

By default `auto_resolution = true`: the tablet app reports its native screen
size when it connects and the host regenerates the virtual display (EDID) to
match — any tablet works out of the box. Untick "Auto" in the GUI to force a
custom resolution (e.g. a lower one for weaker hardware).

## Building a release APK

```bash
cd android
keytool -genkeypair -keystore uscreen-release.keystore -alias uscreen \
        -keyalg RSA -keysize 2048 -validity 10000
# create keystore.properties with: storeFile / storePassword / keyAlias / keyPassword
./gradlew assembleRelease   # → app/build/outputs/apk/release/app-release.apk
```

`keystore.properties` and the keystore are gitignored — keep them safe; the
same key must sign every future update.

## Releasing (maintainers)

```bash
make dist   # → dist/uscreen-<version>-linux-x86_64.tar.gz + dist/.../uscreen.apk
```

Upload both files to a GitHub release.

## Building with the in-process encoder (optional)

By default the daemon pipes raw frames into an `ffmpeg` child process, which
needs no build-time dependencies beyond Rust and gcc. The `inproc-encoder`
feature encodes through libavcodec instead, removing that process boundary:

```bash
cargo build --release --features inproc-encoder
```

Measured on a Tab S9 Ultra at 2960x1848, same settings both ways:

| | ffmpeg process | in-process |
|---|---|---|
| Latency p50 | 22.2-23.1 ms | 22.0-22.9 ms |
| Pipeline CPU | ~188% | **~97%** |

Latency is unchanged, and that is expected: the budget is dominated by the
tablet's decoder (~15 ms) and the USB hop (~7 ms), neither of which the encoder
sits in front of. What it buys is roughly a whole CPU core back, and the ability
to ask for a keyframe on demand — so a client attaching to a mostly-static
screen gets a picture at once instead of waiting for the scheduled one.

It needs the ffmpeg development headers (`ffmpeg-devel` on Fedora,
`libavcodec-dev libavformat-dev libavutil-dev` on Debian/Ubuntu). On atomic
distributions such as Bazzite or Silverblue, where layering development
packages is awkward, build inside a container instead — the resulting binary
links against the host's ffmpeg libraries and runs on the host unchanged:

```bash
distrobox create --name uscreen-tools --image registry.fedoraproject.org/fedora:latest
distrobox enter uscreen-tools -- sudo dnf install -y \
    libavcodec-free-devel libavutil-free-devel libavformat-free-devel \
    libswscale-free-devel libswresample-free-devel libavfilter-free-devel \
    libavdevice-free-devel clang gcc
distrobox enter uscreen-tools -- cargo build --release --features inproc-encoder
```

The `-free` headers are fine here: they declare the same API, and at runtime the
binary uses whichever ffmpeg the host has installed.

One limitation: the in-process encoder feeds libavcodec NV12 straight from the
FIFO, with no conversion step to hang 10-bit on, so `ten_bit` does nothing
there. The daemon says so on startup rather than ignoring it quietly. Use the
default build for 10-bit.

## Performance Tuning

### NVIDIA GPUs (NVENC) — recommended
```bash
uscreen --encoder h264_nvenc --bitrate 30000 --fps 60
```

### AMD/Intel GPUs (VAAPI)
```bash
uscreen --encoder h264_vaapi --bitrate 20000 --fps 60
```

### CPU only (libx264)
```bash
uscreen --encoder libx264 --bitrate 15000 --fps 30
```

## CLI Options

```
USAGE: uscreen [OPTIONS] [COMMAND]

COMMANDS:
  start           Start the uscreen daemon
  stop            Stop the uscreen daemon
  status          Show daemon status
  list-displays   List available displays
  doctor          Diagnose the setup and report what is wrong

OPTIONS (all default to ~/.config/uscreen/config.toml):
  --encoder <ENCODER>     H.264 encoder: h264_nvenc, h264_vaapi, libx264
  --fps <FPS>             Frame rate
  --bitrate <BITRATE>     Bitrate in kbps
  --width <WIDTH>         Capture width
  --height <HEIGHT>       Capture height
  --video-port <PORT>     Video stream port
  --input-port <PORT>     Input WebSocket port
  --quality <Q>           Constant-quality target (12-32, lower = sharper)
  --stream-scale <N>      Downscale the stream only (1 = native, 2 = half)
  --helper <PATH>         Path to evdi_helper binary
  --edid <PATH>           Path to EDID binary
```

## Project Structure

```
uscreen/
├── host/              # Rust daemon (capture, encode, stream, input injection)
│   ├── src/
│   │   ├── main.rs    # CLI, orchestration, plug-and-play ADB monitor
│   │   ├── capture.rs # EVDI helper + ffmpeg management, live settings restart
│   │   ├── stream.rs  # TCP video server, IDR-aware backlog skipping
│   │   ├── input.rs   # WebSocket input server + uinput injection + config channel
│   │   ├── config.rs  # ~/.config/uscreen/config.toml
│   │   ├── latency.rs # End-to-end latency measurement
│   │   ├── doctor.rs  # `uscreen doctor` diagnostics
│   │   └── vdisplay.rs # EVDI discovery via sysfs
│   ├── evdi/          # C helper for EVDI framebuffer capture
│   └── Cargo.toml
├── gui/               # Linux desktop GUI (egui): status, start/stop, settings
├── android/           # Android app (Kotlin/Compose)
│   └── app/src/main/java/com/uscreen/
│       ├── MainActivity.kt    # Fullscreen UI, settings sheet, stats overlay
│       ├── VideoReceiver.kt   # TCP client + MediaCodec decoder (render thread)
│       ├── TouchCapture.kt    # Touch/S-Pen capture + WebSocket + config push
│       └── Prefs.kt           # Persisted tablet-side settings
├── edid/              # Custom EDID files
├── scripts/           # Setup and automation
│   ├── install.sh     # Dependency installer
│   ├── gen-edid.py    # EDID generator for custom resolutions
│   ├── uscreen.desktop    # App menu entry for the GUI
│   └── uscreen.service    # systemd user service
└── Makefile
```

## Protocol

### Video Stream (TCP 8890, loopback only)
- **Length-prefixed packets**: 4-byte big-endian length, then a 1-byte type
  (`0` = codec config, `1` = frame)
- Frame packets carry a 4-byte big-endian sequence number before the payload.
  The tablet passes it through the decoder as the presentation timestamp and
  echoes it back on the input socket once the frame is on screen, which is how
  end-to-end latency is measured on a single clock.
- The first packet sent to a new client contains **SPS+PPS** codec configuration
- Frame data is in Annex B format (with start codes)

### Input Stream (WebSocket 8891)
- **JSON messages** with the following format:

**Touch event:**
```json
{"type":"touch","x":0.5,"y":0.3,"pressure":1.0,"action":0,"slot":0}
```
Actions: 0=DOWN, 1=UP, 2=MOVE. Coordinates are normalized (0.0-1.0).

**Pen/S-Pen event:**
```json
{"type":"pen","x":0.5,"y":0.3,"pressure":0.8,"tilt_x":0.2,"tilt_y":0.1,"action":2}
```

## Troubleshooting

### "Failed to open /dev/uinput"
```bash
sudo modprobe uinput
# For permanent: echo uinput | sudo tee /etc/modules-load.d/uinput.conf
```

### "EVDI device did not appear"
```bash
sudo modprobe evdi
# Check: ls /dev/dri/card*
```

### Black screen on tablet
1. Ensure the EVDI helper is running (`uscreen status`)
2. Check encoder output: `RUST_LOG=uscreen=debug uscreen start`
3. Verify ADB forwarding: `adb reverse --list`

### Touch events not working
- Ensure `/dev/uinput` is accessible (may need `sudo` or udev rule)
- Check: `ls -la /dev/uinput`

## Roadmap

### Shipped

- [x] Core streaming pipeline (capture → encode → stream → decode)
- [x] Android app with MediaCodec rendering
- [x] Touch/S-Pen event capture and WebSocket transmission
- [x] uinput injection (touch → actual input in Linux)
- [x] Proper SPS/PPS codec config handling
- [x] Fullscreen immersive mode
- [x] Plug-and-play: ADB monitor with auto port forwarding + app launch
- [x] Event-driven EVDI capture (request_update/grab cycle at full fps)
- [x] Linux GUI (uscreen-gui) with status and settings
- [x] Android settings UI (bitrate/fps applied live)
- [x] Persistent config (~/.config/uscreen/config.toml)
- [x] Auto-create virtual display at tablet resolution
- [x] Touch/S-Pen mapped to the virtual display, not the whole desktop
- [x] `uscreen doctor` diagnostics
- [x] End-to-end latency measurement
- [x] Starts with the desktop — plug the cable in and it works
- [x] Stream downscaling for lower decode latency (`stream_scale`)
- [x] Optional in-process encoder (libavcodec instead of an ffmpeg process)
- [x] Graphics tablet mode (pen drives the host's own screen)
- [x] On-screen keyboard kept out of the way in both modes
- [x] Switching between the two modes from the tablet, without a restart
- [x] System tray icon: state, mode switch, settings, quit
- [x] Wi-Fi as a fallback transport, with the cost measured and stated
- [x] Virtual screen on any side of the desktop
- [x] HEVC, and 10-bit encoding on top of it

### Next

- [ ] **Several tablets at once**, each as its own virtual screen. Everything
      below the daemon is currently single-instance — one helper, one FIFO,
      one EDID, one video port, one input port, one set of uinput devices — so
      this is a real piece of work rather than a loop. It is not in 1.0
      because it could not be honestly tested: verifying it needs a second
      tablet, and shipping a feature nobody has run is worse than not shipping
      it.
- [ ] **AOA transport.** Would remove the USB debugging requirement, which is
      the last thing between this and simply plugging a cable in.

### Being explored

- [ ] **HDR.** Blocked below us, and not by bandwidth — 10-bit costs perhaps a
      quarter more bits, which a lower frame rate would more than pay for. The
      problem is that EVDI hands over 8-bit ARGB, so there is nothing 10-bit to
      capture in the first place; `ten_bit` above encodes an 8-bit desktop in
      10 bits, which is a different thing entirely. Real HDR would take EVDI
      growing a 10-bit format, and HDR metadata in the generated EDID. Worth
      noting that `uscreen doctor` currently asks you to turn the tablet's
      colour enhancements *off*, for accuracy.

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Commit your changes (`git commit -am 'Add my feature'`)
4. Push to the branch (`git push origin feature/my-feature`)
5. Open a Pull Request

## License

MIT
