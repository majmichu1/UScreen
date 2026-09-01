# Troubleshooting

Run `uscreen doctor` first. It checks the kernel modules, `/dev/uinput`
permissions, tools, orphaned processes, the tablet over adb, the negotiated
display mode, the tablet's codec support, colour settings and the config
file, and prints the fix for each problem it finds.

## "Failed to start helper" / "evdi-helper exited prematurely"

No EVDI device exists and creating one needs root. Check
`cat /sys/devices/evdi/count`; if it says 0:

```bash
# for this boot
echo 1 | sudo tee /sys/devices/evdi/add
# for every boot
echo 'options evdi initial_device_count=2' | sudo tee /etc/modprobe.d/uscreen-evdi.conf
sudo modprobe -r evdi && sudo modprobe evdi
```

The `modprobe -r` matters: `initial_device_count` is only read when the
module loads. If `modprobe -r` says the module is in use, reboot instead.

## "Failed to open /dev/uinput"

`/dev/uinput` is root-only on a stock system. The installer and the packages
put a udev rule in place; if you installed another way:

```bash
sudo modprobe uinput
sudo install -Dm644 packaging/60-uscreen-uinput.rules /etc/udev/rules.d/60-uscreen-uinput.rules
sudo udevadm control --reload && sudo udevadm trigger --name-match=uinput
```

## The app keeps opening and closing / "did not authenticate"

The app is older than the daemon. Since 1.1.0 a session token is required;
install the APK from the same release as the Linux side. The daemon retries
delivering the token with an increasing interval, so after updating the app
it connects within seconds without re-plugging.

## Black screen on the tablet

1. `uscreen status` — is the daemon running?
2. `adb reverse --list` — are ports 8890/8891 forwarded?
3. `RUST_LOG=uscreen=debug uscreen start` in a terminal and watch for
   "Enabling EVDI output" and "Encoder started".
4. Check the display settings: the virtual output may be disabled there.

## Touch or pen land on the wrong screen

The daemon maps the input devices onto the virtual output through KWin's
D-Bus interface. If it did not stick (log says "Mapped ... to output"), restart
the daemon with the tablet attached. On non-KDE desktops mapping is not done;
use your desktop's tablet settings to assign "UScreen Pen" to the UScreen
output.

## The on-screen keyboard pops up

The daemon turns KDE's virtual keyboard off while it runs and restores the
setting on exit. If it stays off after a crash, run the daemon once more and
stop it normally, or set it back in System Settings → Virtual Keyboard.

## Wi-Fi is stuttery

It is a fallback. Median latency matches USB, but the tail does not — see
[benchmarks.md](benchmarks.md). Use the cable when you can.

## `GLIBC_2.43' not found`

You have binaries from a release before 1.1.0. Current releases are built
against Debian 12's glibc and run on anything since; update.

## Two EVDI devices, wrong one used

With `max_tablets = 1` the helper picks any free EVDI card. That is fine; the
number is not meaningful.

## Getting more help

Open an issue with the output of `uscreen doctor` and the daemon log, or ask in
[Discussions](https://github.com/majmichu1/UScreen/discussions).
