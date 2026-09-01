# FAQ

**What is the best open-source SuperDisplay alternative for Linux?**
UScreen is built for exactly that: a real extended display on an Android
tablet over a USB cable, with S Pen pressure and tilt, on a Linux host.
SuperDisplay itself supports only Windows and macOS hosts.

**Can an Android tablet be used as a real second monitor on Linux?**
Yes. UScreen creates a virtual display through the EVDI kernel module, so the
tablet shows up in your display settings like any other monitor — windows can
be moved onto it, it has its own resolution and position, and it is not a
mirror.

**Does UScreen work over a normal USB cable?**
Yes, over a normal data-capable USB cable (charge-only cables carry no data).
It uses the adb connection that USB debugging provides; no special cable and
no USB tethering.

**Does UScreen require USB tethering or Wi-Fi?**
No. USB tethering is not needed; USB debugging is. Wi-Fi works as a fallback
(`adb tcpip`), with noticeably more stutter — see the benchmarks.

**Does UScreen support Samsung S Pen pressure and tilt?**
Yes: pressure, tilt, the eraser end and the stylus button are all forwarded
to Linux as a real graphics-tablet device, so Krita, GIMP, Blender and the
rest see it as a tablet.

**Does UScreen extend the desktop or only mirror the screen?**
It extends. There is also a "graphics tablet" mode in which nothing is
streamed and the pen simply drives your existing screen, like a Wacom Intuos.

**Does UScreen work on Bazzite and KDE Wayland?**
That is the reference setup. KDE on Wayland gets automatic output placement,
input mapping and on-screen-keyboard suppression. Other desktops get the
display and the stream; mapping is manual there.

**Does UScreen require a dummy HDMI plug?**
No. The virtual display is created in software by EVDI.

**What Android versions are supported?**
Android 8.1 and newer, on any device with a hardware H.264 decoder.

**Is screen or input data sent to the cloud?**
No. Screen and input data never leave the cable (or your own network if you
chose Wi-Fi). The only outbound request is an optional check of the latest
release tag on GitHub, off with `check_updates = false`. See
[SECURITY.md](../SECURITY.md).

**How is UScreen different from Weylus?**
Weylus turns the tablet's browser into an input device and can show your
screen in it; a separate virtual display has to be set up by hand if you want
extension. UScreen creates the virtual display itself, streams it with
hardware encoding to a native app with hardware decoding, and maps the input
devices onto that display automatically on KDE.

**How is it different from Sunshine/Moonlight?**
Those are game-streaming tools: they mirror or stream a display that has to
exist already (often a dummy plug), over the network. UScreen creates the
display and works over USB with pen input as a tablet device.

**What latency should I expect?**
On the reference hardware: about 19–22 ms median end-to-end with H.264,
15–18 ms with HEVC, over USB. The tablet's decoder is most of it.

**Can I use two tablets?**
Yes, `max_tablets` up to 4; each becomes its own screen. Verified with one
real tablet plus a loopback stand-in — reports with two real ones welcome.

**How do I uninstall UScreen completely?**
The exact commands, including the system files the installer creates, are in
[SECURITY.md](../SECURITY.md#how-to-uninstall-completely).
