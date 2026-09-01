# Compatibility

What UScreen has actually been run on. Rows come from the maintainer and from
[compatibility reports](https://github.com/majmichu1/UScreen/issues?q=label%3Acompatibility);
please add yours.

## Host

| distribution | desktop | GPU / encoder | result | source |
| --- | --- | --- | --- | --- |
| Bazzite (Fedora Atomic 42) | KDE Plasma 6, Wayland | NVIDIA RTX 5060 Laptop, `h264_nvenc` / `hevc_nvenc` | works; all measurements in [benchmarks.md](benchmarks.md) | maintainer |
| Arch Linux | KDE Plasma, Wayland | — | works — externally verified on a real system twice. v1.0.2: installed through `install.sh` plus the EVDI initialisation described in the issue; the application connected and ran successfully. v1.1.0: the PKGBUILD via `makepkg -si` with `evdi-dkms` from the AUR, no dependency or path issues; menu entry, tray icon and its Settings entry all work. One open bug from that report: input stays on the laptop screen after leaving graphics-tablet mode ([issue #6](https://github.com/majmichu1/UScreen/issues/6)) | [v1.0.2 report](https://github.com/majmichu1/UScreen/issues/2#issuecomment-5478643599), [v1.1.0 PKGBUILD report](https://github.com/majmichu1/UScreen/issues/3#issuecomment-5494961262) |
| Debian 12 | — | — | package installs and binaries run; not exercised with a tablet | maintainer, container |
| Fedora 42 | — | — | rpm installs; evdi must be built from source | maintainer, container |
| openSUSE Tumbleweed | — | — | dependencies resolve; not exercised with a tablet | maintainer, container |

Requirements that follow from the design:

- **Wayland with KDE Plasma** gets the full experience: the daemon places the
  virtual output and maps the pen and touch onto it through KWin's D-Bus
  interfaces, and suppresses the on-screen keyboard.
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

Any Android 8.1+ device with a hardware H.264 decoder should work — the app
reports its own panel size and the virtual display is generated to match.
HEVC is optional and only worth enabling where `uscreen doctor` reports a
hardware HEVC decoder.

## Not supported

- Windows or macOS hosts (SuperDisplay covers those).
- iPads.
- Android below 8.1.
