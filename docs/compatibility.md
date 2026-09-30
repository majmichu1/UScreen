# Compatibility

What UScreen has actually been run on. Rows come from the maintainer and from
[compatibility reports](https://github.com/majmichu1/UScreen/issues?q=label%3Acompatibility);
please add yours.

## Host

| distribution | desktop | GPU / encoder | result | source |
| --- | --- | --- | --- | --- |
| Bazzite (Fedora Atomic 42) | KDE Plasma 6, Wayland | NVIDIA RTX 5060 Laptop, `h264_nvenc` / `hevc_nvenc` | works; all measurements in [benchmarks.md](benchmarks.md) | maintainer |
| Arch Linux | KDE Plasma, Wayland | — | works — externally verified on a real system twice. v1.0.2: installed through `install.sh` plus the EVDI initialisation described in the issue; the application connected and ran successfully. v1.1.0: the PKGBUILD via `makepkg -si` with `evdi-dkms` from the AUR, no dependency or path issues; menu entry, tray icon and its Settings entry all work. A bug from that report — input staying on the laptop screen after leaving graphics-tablet mode ([issue #6](https://github.com/majmichu1/UScreen/issues/6)) — is fixed in 1.2.0 | [v1.0.2 report](https://github.com/majmichu1/UScreen/issues/2#issuecomment-5478643599), [v1.1.0 PKGBUILD report](https://github.com/majmichu1/UScreen/issues/3#issuecomment-5494961262) |
| KDE Neon (Plasma 6.7.5) | KDE Plasma, Wayland | AMD, `h264_vaapi` | works — touch, pen tilt (1.2.3) and daemon-at-boot with the virtual monitor appearing only while the tablet is attached (1.2.3), all confirmed by the reporter; evdi had to be installed by hand | [issue #9](https://github.com/majmichu1/UScreen/issues/9#issuecomment-5661447326), [#11](https://github.com/majmichu1/UScreen/issues/11) |
| Q4OS 6 (Debian 13) | KDE Plasma 6.2 | — | works on a mainline 6.14 kernel once the evdi module is built from upstream — Debian's `evdi-dkms` 1.14.8 does not build there; the `.deb` installs cleanly since 1.2.3 | [issue #13](https://github.com/majmichu1/UScreen/issues/13) |
| Q4OS 6 (Debian 13) | KDE Plasma 6.3 | AMD 3020e iGPU, `h264_vaapi` | works on a two-core, 3 GB laptop — about 30 s to the first picture and around 150 ms touch-to-screen; usable, not pleasant | [issue #17](https://github.com/majmichu1/UScreen/issues/17) |
| Arch Linux | KDE Plasma 6.7.5, Wayland | Intel Raptor Lake-P iGPU, `h264_vaapi` | works; on a mixed-scale layout the parked mouse cursor lands left of the pen — `pointer_handoff = false` avoids it | [issue #18](https://github.com/majmichu1/UScreen/issues/18) |
| Fedora 44 | KDE Plasma | — | works — "near perfectly" with a Galaxy Tab S9 FE, used for drawing; the one complaint (app locked to a single landscape direction) is being addressed | [discussion #7](https://github.com/majmichu1/UScreen/discussions/7) |
| Debian 12 | — | — | package installs and binaries run; not exercised with a tablet | maintainer, container |
| Fedora 42 | — | — | rpm installs; evdi must be built from source | maintainer, container |
| openSUSE Tumbleweed | — | — | dependencies resolve; not exercised with a tablet | maintainer, container |
| Arch Linux | KDE Plasma 6.7.5, Wayland | AMD Radeon 880M, `h264_vaapi` | works; with a Galaxy Tab A7 latency is high (p50 480 ms) | [issue #22](https://github.com/majmichu1/UScreen/issues/22) |
| Nobara 43 (Fedora) | KDE Plasma 6.6.2, Wayland | NVIDIA RTX 3050 Ti Mobile + AMD Vega, `hevc_nvenc` | works | [issue #23](https://github.com/majmichu1/UScreen/issues/23) |
| Linux Mint 22.3 | Cinnamon | Intel Lunar Lake | works (input mapping is manual on Cinnamon) | [issue #24](https://github.com/majmichu1/UScreen/issues/24) |
| Arch Linux (kernel 7.2) | KDE Plasma 6.7.5, Wayland | Intel UHD (i5-10210U), `h264_vaapi` | works on a Fire 7 (9th gen); smooth, some latency | [issue #27](https://github.com/majmichu1/UScreen/issues/27) |
| Ubuntu 26.04.1 LTS | GNOME, Wayland | Intel UHD 620, `libx264` | works with a Galaxy Tab A9+ (1.2.7, `.deb`) | [issue #28](https://github.com/majmichu1/UScreen/issues/28) |

Requirements that follow from the design:

- **Wayland with KDE Plasma** gets the full experience: the daemon places the
  virtual output and maps the pen and touch onto it through KWin's D-Bus
  interfaces, and suppresses the on-screen keyboard.
- **Hyprland** (since 1.2.7): the daemon switches the virtual output on with
  `hyprctl` when Hyprland leaves it off, and pins the pen and touch to it. A
  monitor rule of your own for the output takes precedence. Written against
  Hyprland's documented `hyprctl` interface and not yet confirmed on real
  hardware — reports welcome in
  [#19](https://github.com/majmichu1/UScreen/issues/19).
- **COSMIC** (from 1.2.8): the daemon switches the virtual output on with
  `cosmic-randr`. Not yet confirmed on real hardware, see
  [#26](https://github.com/majmichu1/UScreen/issues/26). Pen and touch
  mapping is manual there.
- **Other desktops** (GNOME, Sway, X11): the virtual display and the stream
  work wherever EVDI does, but output placement and input mapping are not
  automated — assign the "UScreen Pen"/"UScreen Touch" devices to the UScreen
  output in your desktop's settings. Reports welcome.
- **NVIDIA** uses NVENC; **AMD/Intel** use VAAPI (`h264_vaapi`); anything can
  fall back to `libx264` on the CPU.
- The evdi kernel module must be available: in the image (Bazzite, Nobara),
  from the repositories (Debian, Ubuntu, openSUSE), from the AUR (Arch) or
  built from source (Fedora).

## Tablet

| device | Android | stylus | result | source |
| --- | --- | --- | --- | --- |
| Samsung Galaxy Tab S9 Ultra | 14 | S Pen: pressure, tilt, eraser, button | works; HEVC Main10 decodes in hardware | maintainer |
| Samsung Galaxy Tab S9 FE | — | S Pen | works for drawing on a Fedora 44 KDE host | [discussion #7](https://github.com/majmichu1/UScreen/discussions/7) |
| Lenovo Tab K11 | 15 | Lenovo Tab Pen Plus | works on KDE Neon; 60–70 frames/s | [issue #11](https://github.com/majmichu1/UScreen/issues/11) |
| Samsung Galaxy Tab A7 (SM-T505) | 12 | — | works, but decoding is slow: p50 480 ms | [issue #22](https://github.com/majmichu1/UScreen/issues/22) |
| Samsung Galaxy Tab S10 FE+ | 16 | S Pen (button not working yet) | works over HEVC on Nobara KDE | [issue #23](https://github.com/majmichu1/UScreen/issues/23) |
| Samsung Galaxy S26 Ultra (phone) | 16 | — | works on Mint Cinnamon | [issue #24](https://github.com/majmichu1/UScreen/issues/24) |
| Amazon Fire 7 (9th gen) | Fire OS 7 (Android 9) | — | works over micro USB at 60 fps / 20 Mbps; MediaTek MT8163 decoder | [issue #27](https://github.com/majmichu1/UScreen/issues/27) |
| Samsung Galaxy Tab A9+ | 16 | — | works on Ubuntu 26.04 GNOME | [issue #28](https://github.com/majmichu1/UScreen/issues/28) |

Any Android 8.1+ device with a hardware H.264 decoder should work — the app
reports its own panel size and the virtual display is generated to match.
HEVC is optional and only worth enabling where `uscreen doctor` reports a
hardware HEVC decoder.

## Not supported

- Windows or macOS hosts (SuperDisplay covers those).
- iPads.
- Android below 8.1.
