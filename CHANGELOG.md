# Changelog

Full notes for each version are on the
[releases page](https://github.com/majmichu1/UScreen/releases).

## Unreleased

- Several tablets: a slot with no EVDI device of its own is now refused with a
  message saying what to set, instead of starting a session whose helper hunts
  for any free card — with several starting at once that could put two helpers
  on one card. Extra tablets also get their remembered panel applied and
  written back under their own serial, which until now only the first one did
  ([#5](https://github.com/majmichu1/UScreen/issues/5)).
- Each tablet is remembered by its serial. The panel it reported is written to
  a `[tablets.<serial>]` block in the config and used the next time that tablet
  is plugged in, so the display is built for it straight away instead of coming
  up as the previous tablet and being torn down and rebuilt a second later —
  one helper start instead of two here, and on a slow laptop that was most of
  the half-minute before the first picture
  ([#17](https://github.com/majmichu1/UScreen/issues/17)). The block also takes
  `fps`, `bitrate`, `quality`, `stream_scale` and `position` for a tablet that
  should differ from the global settings.
- App: palm rejection really works now. The tablet does not report a resting
  hand as `TOOL_TYPE_PALM` at all — it arrives as an ordinary finger, which is
  why the desktop scrolled and opened windows by itself while drawing. Fingers
  are now ignored while the pen is in play (half a second after its last
  movement), and anything a finger left pressed is lifted when the pen takes
  over, which is how graphics tablets have always behaved.
- The mouse cursor parked where the pen was lifted lands in the right place on
  multi-monitor desktops. The pen is a tablet tool, which KWin keeps inside the
  output it is assigned to; a plain absolute pointer is not, and KWin spreads it
  across the whole desktop whatever the assignment says, so the cursor appeared
  further left and lower the larger the rest of the desktop was. The daemon now
  converts the position itself ([#18](https://github.com/majmichu1/UScreen/issues/18)).

## 1.2.6 — 2026-09-19

- New setting `pointer_handoff` (GUI: *Leave the mouse cursor where the pen
  was lifted*). On, as before, the pen parks the desktop cursor where it was
  lifted; off, no "UScreen Pointer" device is created at all, so the pen and
  touch drive only the tablet's screen and the mouse stays where it was —
  no more dragging the cursor back from the tablet to carry on working
  ([#18](https://github.com/majmichu1/UScreen/issues/18)). The misplaced
  cursor on mixed-scale layouts reported there is not fixed yet; turning the
  handoff off avoids it.
- The daemon checks that the configured encoder can actually encode a frame
  before using it, and switches to the first one that can (NVENC, then VAAPI,
  then libx264), saving the choice. A fresh config said `h264_nvenc`, and on
  a machine without NVIDIA that meant ffmpeg dying on every start while the
  tablet showed a spinner forever
  ([#15](https://github.com/majmichu1/UScreen/issues/15)). `uscreen doctor`
  checks the same way instead of trusting `ffmpeg -encoders`, which lists
  what ffmpeg was built with rather than what works.
- GUI: the config path no longer draws on top of the status message
  ([#16](https://github.com/majmichu1/UScreen/issues/16)).

## 1.2.5 — 2026-09-17

- App: palm rejection works now. Android's hidden `TOOL_TYPE_PALM` is 5; the
  app compared against 6, so a resting palm was forwarded as a finger and
  scrolled or clicked things — since the feature was added. A finger that
  Android reclassifies as a palm mid-gesture is also lifted on the host
  rather than left pressed.
- Daemon: a second daemon started as plain `uscreen` (no subcommand) was not
  recognised as one and could run alongside the first, both fighting for the
  capture device.
- Helper: the BT.709 chroma coefficients summed to −1, so pure greys and
  white carried a one-step blue cast (Cb 127 instead of 128).
- Daemon: the generated EDID is written to a temporary name and renamed
  into place, so a crash mid-write can no longer leave a truncated file that
  every later start would use.
- App: the frame-arrival ring index wraps instead of overflowing after 2³¹
  frames (about 14 months of continuous use).

## 1.2.4 — 2026-09-17

- App: when the decoder keeps stalling it now steps down a ladder instead of
  restarting the same way forever — the hardware decoder, then the hardware
  decoder without low-latency hints, then Android's software decoder, two
  stalls per step. One rendered frame no longer counts as recovery, since a
  decoder that only ever produces keyframes shows exactly one frame per
  restart, which is what a Galaxy Tab S10 FE+ on Android 16 does with the
  1.2.3 watchdog: 1 fps, the picture blinking on and off
  ([#10](https://github.com/majmichu1/UScreen/issues/10)). The decoder's name
  and output format are logged, so the next `logcat` names the component.
- App: frame timestamps handed to the decoder advance by 10 ms per frame
  rather than 1 µs. The host's sequence number still rides in them for the
  latency loop; the difference is that a decoder which paces or drops frames
  by timestamp now sees plausible ones.
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
