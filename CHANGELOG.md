# Changelog

Full notes for each version are on the
[releases page](https://github.com/majmichu1/UScreen/releases).

## 1.1.0 — 2026-08-31

- Security: session token between app and daemon; capture FIFO moved out of
  `/tmp` into the per-user runtime directory.
- Packages: `.deb`, `.rpm`, PKGBUILD archive and `SHA256SUMS` in every release;
  binaries built against Debian 12 glibc so they run on any current
  distribution.
- Several tablets at once (`max_tablets`), each as its own screen.
- Update checks in the app, the GUI and the tray (report only).
- udev rule for `/dev/uinput`, so input works outside Bazzite.
- Fixes: PID-file race between daemon restarts, `uscreen stop` matching
  unrelated processes, GUI tablet detection with two adb devices, PATH-free
  menu and tray launching, atomic config writes with change logging.
- Minimum Android is 8.1 (it always was, in practice).

## 1.0.2 — 2026-08-30

- openSUSE support; userspace and kernel-module installs split so one failing
  package does not take the rest down; PATH check.

## 1.0.1 — 2026-08-30

- Fixes for issue #2: correct package names on Arch (AUR), Debian and Fedora;
  the daemon explains a missing EVDI device instead of retrying forever.

## 1.0.0 — 2026-08-26

- Wi-Fi as a fallback transport with a low-latency Wi-Fi lock, system tray
  icon, virtual screen on any side of the desktop, HEVC and 10-bit encoding.

## 0.4.0 — 2026-08-26

- Graphics-tablet mode switchable from the tablet; capture helper no longer
  spins a core when the output is disabled.

## 0.3.0 — 2026-08-25

- `uscreen doctor`, end-to-end latency measurement, input mapped to the
  virtual display (issue #1), optional in-process encoder.
