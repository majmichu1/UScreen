# UScreen for Linux — Android Tablet as a USB Second Monitor

**UScreen is an open-source SuperDisplay alternative for Linux.** It turns an
Android 8.1+ tablet into a real extended USB display and a pressure-sensitive
graphics tablet, with touch, S Pen pressure, tilt, eraser and stylus-button
support.

UScreen uses a direct ADB-over-USB connection — no Wi-Fi, USB tethering,
dummy HDMI plug or cloud account required. Nothing leaves the cable.

Tested on Bazzite (KDE Plasma, Wayland, NVIDIA) with a Samsung Galaxy Tab S9
Ultra. Packages and installation instructions cover Bazzite, Fedora,
Ubuntu/Debian, Arch Linux and openSUSE.

[**Download the latest release**](https://github.com/majmichu1/UScreen/releases/latest)
· [Install](#quick-install)
· [Compatibility](docs/compatibility.md)
· [Benchmarks](docs/benchmarks.md)
· [FAQ](#faq)
· [Website](https://majmichu1.github.io/UScreen/)

## Why UScreen?

- **A real second monitor, not a mirror.** A virtual display is created
  through the EVDI kernel module; the tablet appears in your display settings
  and you move windows onto it.
- **Pen that works like a tablet.** Pressure, tilt, eraser and button arrive
  in Linux as a graphics-tablet device — Krita, GIMP and Blender see a tablet.
  A one-tap *graphics tablet* mode uses the pen on your own screen with zero
  display latency.
- **Low latency, measured.** About 22 ms median end-to-end over USB with
  H.264, 15–18 ms with HEVC, on the reference hardware — the
  [numbers and the method](docs/benchmarks.md) are published.
- **Plug in and it works.** The daemon starts with your desktop, finds the
  tablet over adb, launches the app on it and sizes the display to its panel.
- **Private by construction.** Loopback-only ports guarded by a per-session
  token, no telemetry, no account. See [SECURITY.md](SECURITY.md).
- **Honest about its edges.** Wi-Fi is a fallback and the stutter is
  [quantified](docs/benchmarks.md#usb-vs-wi-fi-h264-quiet-link); KDE gets the
  full automation, other desktops get the display and manual mapping.

## Quick install

**1. Linux side** — pick the file for your distribution from the
[latest release](https://github.com/majmichu1/UScreen/releases/latest):

| file | distribution |
| --- | --- |
| `uscreen_<ver>_amd64.deb` | Debian 12+, Ubuntu 22.04+, Mint, Pop — `sudo apt install ./uscreen_*.deb` |
| `uscreen-<ver>-1.x86_64.rpm` | openSUSE (`zypper install`), Fedora (RPM Fusion first, then `dnf install --allowerasing`) |
| `uscreen-<ver>-PKGBUILD.tar.gz` | Arch and derivatives — extract, `makepkg -si` |
| `uscreen-<ver>-linux-x86_64.tar.gz` | Bazzite, Nobara, anything else — extract, `./scripts/install.sh` |

Then `systemctl --user enable --now uscreen` (the tarball installer does this
for you). Full details, including what the installer changes on the system,
in [docs/installation.md](docs/installation.md).

**2. Tablet** — install `uscreen.apk` and enable USB debugging (Settings →
Developer options).

**3. Plug in.** The daemon forwards the ports, launches the app and the
tablet shows up as a monitor. `uscreen doctor` diagnoses anything that is off.

Update both halves together: since 1.1.0 they share a session token.

If UScreen replaced a second monitor for you, a star on the repo and a
[compatibility report](https://github.com/majmichu1/UScreen/issues/new?template=compatibility.yml)
help the next Linux user find it.

## Verified compatibility

| host | tablet | result |
| --- | --- | --- |
| Bazzite, KDE Plasma 6 Wayland, NVIDIA RTX 5060 | Galaxy Tab S9 Ultra, Android 14 | works — reference setup, all benchmarks |
| Arch Linux, KDE Plasma Wayland | — | works ([#2](https://github.com/majmichu1/UScreen/issues/2)) |
| Debian 12 · Fedora 42 · openSUSE Tumbleweed | — | packages install and run (container-tested, no tablet) |

Any Android 8.1+ tablet with a hardware H.264 decoder should work — the
display is generated to match the tablet. More in
[docs/compatibility.md](docs/compatibility.md); reports are welcome.

## Performance

Measured on the reference hardware over USB (2960×1848, 90 fps target,
constant-quality encoding):

| | median | p95 |
| --- | --- | --- |
| H.264, NVENC | 18–22 ms | 23–31 ms |
| HEVC, NVENC | 15–18 ms | 20–23 ms |
| Wi-Fi fallback (H.264) | 22.8 ms | 78.6 ms, worst frames in seconds |

The tablet's hardware decoder is most of the budget (~15 ms), the USB hop
5–7 ms, the encoder under 1 ms. Method, CPU figures and limitations in
[docs/benchmarks.md](docs/benchmarks.md).

## Compared with the alternatives

Checked against each project's own documentation in August 2026; corrections
welcome.

| | UScreen | [SuperDisplay](https://superdisplay.app/) | [Weylus](https://github.com/H-M-H/Weylus) | [Sunshine](https://github.com/LizardByte/Sunshine) + Moonlight | [spacedesk](https://www.spacedesk.net/) |
| --- | --- | --- | --- | --- | --- |
| Linux host | **yes** | no (Windows, macOS) | yes | yes | no (Windows) |
| Real extended display | **yes** (EVDI) | yes | needs a separate virtual-display setup | needs an existing or dummy display | yes |
| Direct USB, no tethering | **yes** (adb) | yes | via adb port forward | no (network) | no (network) |
| S Pen pressure | **yes** | yes | yes | partial | partial |
| Tilt, eraser, button | **yes** | yes | pressure/tilt via browser API, no eraser | no | no |
| Hardware video encoding | NVENC / VAAPI / x264 | yes | yes (VAAPI/NVENC) | yes | yes |
| Open source | **MIT** | no | AGPL | GPL | no |
| Dummy HDMI plug | **no** | no | sometimes | often | no |

## Settings

Everything lives in `~/.config/uscreen/config.toml` and is reachable from
`uscreen-gui`, the ⚙ sheet in the tablet app, the tray icon, or CLI flags.

- **Graphics tablet mode** — flip *Graphics tablet* on the tablet: nothing is
  streamed, the pen drives your own screen, zero display latency. Switch back
  the same way; no restart.
- **Position** — `right` (default), `left`, `above`, `below` your real screens.
- **Codec** — `h264_nvenc` by default because every device decodes it;
  `hevc_nvenc` is sharper at the same bitrate and was faster on the reference
  tablet. `ten_bit` (HEVC Main10) smooths gradient banding — the desktop is
  8-bit, so it adds precision, not colour; it is not HDR.
- **Stream scale** — `stream_scale = 2` sends a quarter of the pixels for a
  ~6 ms lower decode time at the cost of softer text.
- **Several tablets** — `max_tablets` up to 4, each its own screen.
- **Wi-Fi** — `adb tcpip 5555` and `adb connect <ip>:5555`; the daemon
  prefers the cable when both are there.
- **Updates** — the app, the GUI and the tray tell you when a newer release
  exists; nothing installs itself. `check_updates = false` turns it off.

## FAQ

**Does it need Wi-Fi or USB tethering?** No — USB with USB debugging. Wi-Fi
is an optional fallback.

**Is it a mirror or an extension?** An extension; a real monitor in your
display settings. Graphics-tablet mode is a separate, non-display mode.

**Does S Pen pressure and tilt work?** Yes, plus eraser and button, as a
proper tablet device.

**Does it work on Bazzite / KDE Wayland?** That is the reference setup.
GNOME and X11 get the display and the stream; input mapping is manual there.

**Does it need a dummy HDMI plug?** No.

**Which Android versions?** 8.1 and newer.

**Is anything sent to the cloud?** No. The only outbound request is an
optional version check against GitHub.

**How do I uninstall it completely?** [SECURITY.md](SECURITY.md#how-to-uninstall-completely)
lists every file.

More in [docs/faq.md](docs/faq.md).

## Documentation

- [Installation](docs/installation.md) · [Troubleshooting](docs/troubleshooting.md)
- [Architecture and protocol](docs/architecture.md) · [Development, building, releasing](docs/development.md)
- [Benchmarks](docs/benchmarks.md) · [Compatibility](docs/compatibility.md) · [FAQ](docs/faq.md)
- [Security](SECURITY.md) · [Changelog](CHANGELOG.md)

## Roadmap

Shipped: extended display over USB, S Pen with pressure/tilt/eraser, graphics-
tablet mode switchable from the tablet, plug-and-play with autostart, tray
icon, any-side placement, several tablets, HEVC and 10-bit, Wi-Fi fallback,
packages for five distribution families, `uscreen doctor`, measured latency.

Next: **AOA transport** — removing the USB-debugging requirement, the last
step between this and simply plugging a cable in. Explored: HDR, currently
blocked by EVDI providing only 8-bit framebuffers.

## Contributing

Compatibility reports are the most useful thing right now; see
[CONTRIBUTING.md](CONTRIBUTING.md). Issues tagged `good first issue` are
self-contained. Questions go to
[Discussions](https://github.com/majmichu1/UScreen/discussions).

## License

MIT — see [LICENSE](LICENSE). The bundled libevdi client library is LGPL-2.1
from DisplayLink, unmodified — see [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
