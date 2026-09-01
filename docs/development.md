# Development

## Building from source

Dependencies: Rust (stable), gcc, `pkg-config`, `libdrm` headers, `ffmpeg`,
`adb`, and the evdi kernel module. See [installation.md](installation.md) for
the per-distribution package names.

```bash
make build            # EVDI helper (C) + Rust daemon + GUI
make install          # copies to ~/.local/bin, installs the systemd user unit
make setup-system     # modprobe.d / modules-load.d / udev rule (sudo)
```

The Android app:

```bash
cd android
./gradlew assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

Or open `android/` in Android Studio.

## Running and testing

```bash
RUST_LOG=uscreen=debug uscreen start      # in a terminal, with the tablet attached
cargo test --release --manifest-path host/Cargo.toml
cargo clippy --release --manifest-path host/Cargo.toml
cd android && ./gradlew lintDebug
```

`scripts/fake-tablet.py` pretends to be a tablet on the loopback ports
(authenticates, reports a resolution, acks frames). With
`USCREEN_FAKE_TABLET=fake1,fake2` and `max_tablets = 2` it exercises a
second pipeline without a second device.

## Project layout

```
host/              Rust daemon
  src/main.rs        CLI, orchestration, adb monitor, per-tablet sessions
  src/capture.rs     EVDI helper + encoder management, display placement
  src/encoder.rs     optional in-process libavcodec encoder
  src/stream.rs      TCP video server, session token, IDR-aware backlog skipping
  src/input.rs       WebSocket input server, uinput devices, KWin mapping
  src/config.rs      ~/.config/uscreen/config.toml
  src/runtime.rs     per-user runtime dir: FIFO, session token
  src/latency.rs     end-to-end latency measurement
  src/tray.rs        StatusNotifierItem tray icon
  src/update.rs      release check (report only)
  src/doctor.rs      `uscreen doctor`
  src/osk.rs         KDE on-screen keyboard suppression over D-Bus
  src/vdisplay.rs    EVDI discovery via sysfs
  src/edid.rs        EDID generation for the virtual display
  evdi/              C helper: EVDI framebuffer capture → NV12 → FIFO
gui/               egui desktop app: status, settings, start/stop
android/           Kotlin/Compose app: MediaCodec decoder, touch/pen capture
packaging/         deb control/postinst, rpm spec, PKGBUILD, udev/modprobe files
scripts/           install.sh, release build, fake tablet
docs/              this documentation and the GitHub Pages site
```

## Command line

```
uscreen [OPTIONS] [COMMAND]

COMMANDS
  start           start the daemon
  stop            stop the daemon
  status          show daemon status
  list-displays   list EVDI displays
  doctor          diagnose the setup and print fixes

OPTIONS (override ~/.config/uscreen/config.toml for this run only)
  --encoder <NAME>      h264_nvenc, hevc_nvenc, h264_vaapi, libx264
  --fps <N>             frame rate (30–90)
  --bitrate <KBPS>      bitrate ceiling
  --width/--height <N>  capture size (auto_resolution off)
  --quality <Q>         constant-quality target, 12–32, lower is sharper
  --stream-scale <N>    downscale the stream only, 1 = native, 2 = half
  --pen-only            graphics-tablet mode for this run
  --video-port/--input-port <PORT>
  --helper <PATH>, --edid <PATH>
```

## Encoder tuning

- NVIDIA: `h264_nvenc` (default) or `hevc_nvenc` — see the codec section of
  the README for when HEVC and `ten_bit` are worth it.
- AMD/Intel: `h264_vaapi`, constant-quality via `quality`.
- CPU: `libx264`, `ultrafast`/`zerolatency`; expect 30 fps at most on a laptop.

`quality` is what governs picture quality; `bitrate` is only a ceiling for
bursts. On a static desktop the stream sits far below it.

## Optional in-process encoder

```bash
cargo build --release --manifest-path host/Cargo.toml --features inproc-encoder
```

Encodes through libavcodec in-process instead of an `ffmpeg` child: same
latency, about one CPU core less, and keyframes on demand. Needs the ffmpeg
development headers (`ffmpeg-devel` from RPM Fusion on Fedora,
`libavcodec-dev libavformat-dev libavutil-dev libswscale-dev` on Debian). On
atomic distributions build inside a container (`distrobox`); the binary links
against the host's ffmpeg at runtime. `ten_bit` is not available on this path.

## Release APK

```bash
cd android
keytool -genkeypair -keystore uscreen-release.keystore -alias uscreen \
        -keyalg RSA -keysize 2048 -validity 10000
# keystore.properties: storeFile / storePassword / keyAlias / keyPassword
./gradlew assembleRelease     # app/build/outputs/apk/release/app-release.apk
```

The keystore and `keystore.properties` are gitignored. The same key must sign
every future release or users cannot update in place.

## Releasing (maintainers)

Binaries are built in a Debian 12 container so they run on any current glibc:

```bash
distrobox create --image debian:12 --name uscreen-build
# inside: build-essential pkg-config libdrm-dev git dpkg-dev fakeroot rpm curl,
#         the X11/Wayland dev packages for the GUI, and rustup

GH_TOKEN=... make publish NOTES=release-notes.md
```

`make publish` runs `scripts/build-release.sh` and `packaging/build-packages.sh`,
refuses to continue unless all five release files exist, creates the GitHub
release for the already-pushed tag and uploads everything plus `SHA256SUMS`.
Bump `VERSION` in the Makefile, `version` in both `Cargo.toml` files and
`versionCode`/`versionName` in `android/app/build.gradle.kts` first, and tag
`vX.Y.Z`.
