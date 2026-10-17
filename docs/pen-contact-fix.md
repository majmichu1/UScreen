# Light pen contacts and libinput

On Xiaomi Pad 7 with Focus Pen Pro, Android can report contact before pressure
rises above zero. UScreen forwards `BTN_TOUCH=1`, but libinput uses pressure
thresholds for a pressure-capable tablet, so some light contacts never become
tip-down events. See [issue #34](https://github.com/majmichu1/UScreen/issues/34).

There are two required parts to this workaround:

- The host sends at least one pressure unit during pen-down and contact-move.
  Positive values within 1–4096 are unchanged; hover and release remain zero.
- `AttrPressureRange=1:0` makes libinput recognize that one-unit contact.
  The host pressure floor alone does not bypass libinput's threshold.

## Automatic installation

DEB, RPM and Arch packages install `packaging/libinput/99-uscreen-pen.quirks`
as `/usr/share/libinput/99-uscreen-pen.quirks`. The package manager updates
and removes this file with UScreen.

The source (`make install`, `make setup-system`) and tarball installers add
the same quirk to a marked block at the beginning of
`/etc/libinput/local-overrides.quirks`. This also works when `/usr` is
read-only. Reinstalling replaces only that block; other sections, including
existing user overrides, are preserved and retain precedence.

The quirk matches only `UScreen Pen*`, vendor `0x4553`, with udev type
`tablet`, and sets `AttrPressureRange=1:0`. No Android update is needed.

Validate the installed quirks with `libinput quirks validate`. To check the
match, run `libinput quirks list /dev/input/eventN`, replacing `eventN` with
the event node for `UScreen Pen`; it should include `AttrPressureRange=1:0`.

libinput reads quirks when its context starts. On KDE Wayland, stop UScreen,
log out and back in, then start the updated daemon. Restarting only UScreen
does not refresh KWin's existing libinput context. Quirks are an internal
libinput interface; see the [libinput documentation](https://wayland.freedesktop.org/libinput/doc/latest/device-quirks.html).

## Validation

The regression test checks pen and eraser contact with zero pressure, contact
continuing at zero pressure, preservation of positive samples, and zero
pressure on hover, lift and proximity exit.

Run the installer regression tests with
`python3 -m unittest discover -s scripts/tests -v`. They use temporary roots
to check installation, updates, removal, preservation of user settings and
permissions, and rejection of symlinks or damaged block markers.

In retained UScreen 1.2.9 hardware captures on CachyOS/KDE Wayland with
libinput 1.32.0, Xiaomi Pad 7, Android 16 / HyperOS 3.0.304.0 and Focus Pen Pro,
only one of five light virtual contacts initially produced a libinput tip
event. With both changes, seven light, five normal and one sustained virtual
contact all produced matching tip-down/up pairs; hover produced no tip events.
Those are recorded virtual-contact counts, not an independently observed
number of human taps. Application brush strokes and Samsung hardware were
not separately verified by those captures.
