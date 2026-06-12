# UScreen — USB Second Screen for Linux

Turn your Android tablet into a low-latency secondary display for Linux, connected via USB.

Inspired by SuperDisplay for Windows. Built for Linux (Bazzite, Fedora, Arch, etc.).

## Features

- **Plug and play**: the daemon watches for the tablet over ADB — plug in the USB cable and the app launches on the tablet automatically
- **Any tablet**: the app reports its screen size and the virtual display is generated to match (auto-resolution), or pick a custom resolution in the GUI
- **Low latency**: event-driven EVDI capture, hardware encoding, IDR-aware frame skipping — the pipeline never lets latency accumulate
- **Settings UI on both sides**: a desktop GUI (`uscreen-gui`) on Linux, and a settings sheet in the Android app (bitrate / fps changes apply live, no reconnect needed)
- **Touch & S-Pen** forwarded back to Linux with pressure and tilt

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
# 2. Start the daemon (or click Start in uscreen-gui):
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
  setup-vdisplay  Setup virtual display via EVDI

OPTIONS (all default to ~/.config/uscreen/config.toml):
  --encoder <ENCODER>     H.264 encoder: h264_nvenc, h264_vaapi, libx264
  --fps <FPS>             Frame rate
  --bitrate <BITRATE>     Bitrate in kbps
  --width <WIDTH>         Capture width
  --height <HEIGHT>       Capture height
  --video-port <PORT>     Video stream port
  --input-port <PORT>     Input WebSocket port
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
│   │   └── vdisplay.rs # Virtual display manager
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
│   ├── uscreen.service    # systemd user service
│   └── 51-uscreen.rules   # udev rules for auto-detection
└── Makefile
```

## Protocol

### Video Stream (TCP 8890)
- **Length-prefixed frames**: Each frame has a 4-byte big-endian length prefix followed by raw H.264 data
- The first frame sent to new clients contains **SPS+PPS** codec configuration
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
- [ ] Auto-create virtual display at tablet resolution
- [ ] System tray icon
- [ ] Wi-Fi mode (fallback)
- [ ] Multi-monitor support
- [ ] HDR support

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Commit your changes (`git commit -am 'Add my feature'`)
4. Push to the branch (`git push origin feature/my-feature`)
5. Open a Pull Request

## License

MIT
