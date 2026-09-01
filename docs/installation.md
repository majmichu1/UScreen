# Installing UScreen for Linux

Two halves: the Linux side (daemon, GUI, tray) and the Android app. They ship
together in every [release](https://github.com/majmichu1/UScreen/releases/latest)
and must be updated together — the session token between them needs both.

## Linux side: pick your file

| file | for | command |
| --- | --- | --- |
| `uscreen_<ver>_amd64.deb` | Debian 12+, Ubuntu 22.04+, Mint, Pop | `sudo apt install ./uscreen_*.deb` — pulls `evdi-dkms`, `ffmpeg` and `adb` |
| `uscreen-<ver>-1.x86_64.rpm` | openSUSE | `sudo zypper install ./uscreen-*.rpm` |
| `uscreen-<ver>-1.x86_64.rpm` | Fedora | enable [RPM Fusion](https://rpmfusion.org/Configuration) first (stock Fedora has no `ffmpeg`), then `sudo dnf install --allowerasing ./uscreen-*.rpm`, then build the evdi module from [DisplayLink/evdi](https://github.com/DisplayLink/evdi) — it is not packaged for Fedora |
| `uscreen-<ver>-PKGBUILD.tar.gz` | Arch, Manjaro, EndeavourOS, CachyOS | extract, `makepkg -si` — pulls `evdi-dkms` from the AUR |
| `uscreen-<ver>-linux-x86_64.tar.gz` | anything else, or Bazzite/Nobara | extract, `./scripts/install.sh` |

Bazzite and Nobara ship the evdi module in the image; the tarball's installer
is the right choice there (it layers `ffmpeg`/`android-tools` with rpm-ostree
if they are missing).

After a package install, enable the daemon for your login session:

```bash
systemctl --user enable --now uscreen
```

The tarball installer does this for you and also adds a menu entry.

Verify the checksums if you like: `sha256sum -c SHA256SUMS` next to the
downloaded files.

## Android side

Install `uscreen.apk` on the tablet (sideloading has to be allowed for your
browser or file manager). Then enable **USB debugging**: Settings → About →
tap *Build number* seven times → Developer options → USB debugging. Accept the
"Allow USB debugging" prompt from your computer the first time you plug in.

Minimum Android version is **8.1**.

## Plug in and use

1. Start the daemon (`systemctl --user enable --now uscreen`, or `uscreen
   start` for one session, or the *Start* button in `uscreen-gui`).
2. Plug the USB cable in. The daemon forwards the ports over adb and launches
   the app on the tablet by itself.
3. The tablet appears as a new monitor in your display settings, sized to the
   tablet's own panel. Touch and the pen work on it immediately.

That's the whole procedure. `uscreen doctor` tells you what is missing if
something is.

## What gets changed on the system

- `/etc/modprobe.d/uscreen-evdi.conf` with `options evdi initial_device_count=2`
- `/etc/modules-load.d/uscreen.conf` loading `evdi` and `uinput`
- a udev rule opening `/dev/uinput` to the logged-in user
- a systemd *user* unit (never a system service, never root)

See [SECURITY.md](../SECURITY.md) for the reasoning and for a complete
uninstall.

## Dependencies by distribution (for the curious)

Checked on each of them, not from memory:

| | ffmpeg | adb | evdi module |
| --- | --- | --- | --- |
| Arch | `ffmpeg` | `android-tools` | AUR: `evdi-dkms` |
| Debian / Ubuntu | `ffmpeg` | `adb` | `evdi-dkms` |
| Fedora | RPM Fusion, `--allowerasing` to replace `ffmpeg-free` | `android-tools` | not packaged — build from source |
| openSUSE | `ffmpeg` | `android-tools` | `evdi` |
| Bazzite / Nobara | in the image | `android-tools` | in the image |

The helper ships with its own copy of libevdi (LGPL-2.1, unmodified, found
through an `$ORIGIN` rpath), so no distribution needs a libevdi package.

## Building from source instead

See [development.md](development.md).
