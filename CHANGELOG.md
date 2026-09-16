# Changelog

Full notes for each version are on the
[releases page](https://github.com/majmichu1/UScreen/releases).

## Unreleased

- GUI: the settings window scrolls. It was clipped at the window's height with
  no scrollbar and a dead wheel, so on a small or portrait screen the lower
  settings were unreachable ([#14](https://github.com/majmichu1/UScreen/issues/14)).
- Fix: saving settings from the GUI erased the address remembered by
  `uscreen wifi`, since the GUI rewrites the whole config file and did not
  know the field yet.

## 1.2.3 — 2026-09-15

- Fix: the virtual monitor no longer exists while no tablet is attached. The
  EVDI helper used to run from daemon start, so the desktop saw a connected
  (if disabled) monitor at every boot — and a KDE layout saved as "only the
  UScreen screen" came up with the real screens black and no tablet in sight.
  The helper now starts when a tablet becomes a screen and stops when it stops
  being one, so the monitor appears and disappears like a cable
  ([#12](https://github.com/majmichu1/UScreen/issues/12)).
- App: a decoder that keeps accepting frames but never shows any is now
  restarted after 1.5 s, and after a second stall restarted without the
  low-latency hints. A Galaxy Tab S10 FE+ on Android 16 rendered one frame and
  then nothing while the host kept sending; the old check only noticed a
  decoder that stopped *taking* frames
  ([#10](https://github.com/majmichu1/UScreen/issues/10)).
- Packaging: the `.deb` recommends `evdi-dkms` instead of depending on it, so
  a module that fails to build for an unusual kernel no longer leaves
  `uscreen` unconfigured with its udev rule and modprobe file missing;
  `docs/installation.md` explains building the upstream module on kernels
  newer than the distribution's `evdi-dkms` supports, and the helper's hint
  when no EVDI device appears no longer refers to a Makefile target that
  package installs do not have
  ([#13](https://github.com/majmichu1/UScreen/issues/13)).
- Fix: with two Android devices attached the daemon took whichever one adb
  listed first on every check, so any reshuffle of that list looked like a
  different tablet being plugged in — the port forwards moved, the stream on
  the real tablet froze on its last frame and the app went black when it
  reconnected. The daemon now stays with the device it is driving for as long
  as it is attached, and when it has to choose it prefers the one that has
  the UScreen app installed ([#10](https://github.com/majmichu1/UScreen/issues/10)).

## 1.2.2 — 2026-09-14

- Fix: the pen's tilt axes were the wrong way round — a pen leaning right
  reported as leaning towards the user and vice versa. Android measures the
  stylus direction clockwise from the top of the screen, and that was
  decomposed into x and y the other way round
  ([#11](https://github.com/majmichu1/UScreen/issues/11)).
- `uscreen doctor` tells two Android devices apart from one device reachable
  two ways, and names them with their model. The daemon drives the first and
  launches the app there, so a second device sitting next to it shows nothing
  — which is worth saying rather than reporting "reachable 2 ways".

## 1.2.1 — 2026-09-13

- `uscreen wifi` sets the tablet up for wireless use in one step — it switches
  its adb to the network, remembers the address and the daemon reconnects to
  it by itself whenever the cable is out, so the adb dance is not something to
  repeat by hand ([#8](https://github.com/majmichu1/UScreen/issues/8)).
  `uscreen wifi --off` forgets it. The video and input ports stay on loopback.
- Fix: on Debian- and Ubuntu-based Plasma systems the daemon never mapped the
  tablet's touch and pen onto the virtual display, so they drove the wrong
  screen — plain `qdbus` is Qt5's and is often not installed there, only
  `qdbus-qt6`. KWin is now reached through systemd's `busctl`, which every
  target distribution has, with the whole qdbus family as a fallback
  ([#9](https://github.com/majmichu1/UScreen/issues/9),
  [#10](https://github.com/majmichu1/UScreen/issues/10)). The same call path
  carries on-screen-keyboard suppression, which was silently off there too.
- `uscreen doctor` reports whether KWin can be reached at all, instead of
  leaving the Desktop section empty when it cannot.
- The settings window now identifies itself to the desktop as `uscreen`, the
  same name as its menu entry, so the KDE task bar shows the UScreen icon for
  it instead of a generic monitor.

## 1.2.0 — 2026-09-12

- Performance: ffmpeg was converting every frame through RGB on the CPU
  because the BT.709 tags were given as output options, which ffmpeg 7+
  reads as a request to convert. Tagging the input instead drops the
  encoder process from about four cores to under half a core at 60 fps and
  removes a needless colour round-trip.
- Capture: the helper asks the compositor for the next frame as soon as the
  previous one is copied out instead of waiting for the next period tick,
  and keeps the framebuffer on huge pages, which cuts the copy from about
  6.5 ms to 4.5 ms at 2960×1848. Under motion that is 58–63 frames/s where
  it was 52–57 (at a 90 fps target), and a 30 fps target no longer lands
  at 24. The 5-second stats line now also shows how long the compositor
  took to answer and how long the copy took, which is how this was found.
- Icons: the app, the menu entry, the settings window and the tray now share
  one UScreen icon (amber tablet and stylus on charcoal) instead of the stock
  Android tile and a generic display glyph; the tray shows a dimmed variant
  in graphics-tablet mode.
- `uscreen doctor` warns when Samsung's Motion smoothness is on Standard,
  which holds the panel at 60 Hz whatever the app asks for.
- App: *Rotate automatically* in the ⚙ sheet follows the tilt sensor between
  the two landscape directions, so the tablet can be held camera-down for
  drawing (requested in
  [discussion #7](https://github.com/majmichu1/UScreen/discussions/7)); with
  it off, *Camera up* / *Camera down* pin the direction. The app reads the
  sensor itself because the system's own sensor mode never flipped the
  reference tablet.
- Fix: after leaving graphics-tablet mode the pen and touch could stay mapped
  to the laptop screen. The daemon now waits for the virtual display to be
  enabled before mapping the input devices and verifies the mapping instead of
  assuming it ([#6](https://github.com/majmichu1/UScreen/issues/6)).

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
