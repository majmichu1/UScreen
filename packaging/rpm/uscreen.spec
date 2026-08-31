Name:           uscreen
Version:        1.1.0
Release:        1%{?dist}
Summary:        Android tablet as a USB second display and graphics tablet
License:        MIT
URL:            https://github.com/majmichu1/UScreen
Source0:        uscreen-%{version}-linux-x86_64.tar.gz
BuildArch:      x86_64
Requires:       ffmpeg android-tools
# The helper ships with its own libevdi next to it (LGPL, $ORIGIN rpath), so
# only the kernel module is needed from the system. That is packaged on
# openSUSE (evdi) and not on Fedora at all, hence Recommends rather than
# Requires: the package must still install on Fedora, where evdi is built from
# source.
Recommends:     evdi

%description
Turns an Android tablet into a low-latency second monitor for a KDE Wayland
desktop over USB, with touch and stylus forwarded back. Can also act as a
plain graphics tablet for the host's own screen.

%prep
%setup -q -n uscreen-%{version}

%install
install -Dm755 bin/uscreen          %{buildroot}%{_bindir}/uscreen
install -Dm755 bin/uscreen-gui      %{buildroot}%{_bindir}/uscreen-gui
install -Dm755 bin/evdi_helper      %{buildroot}%{_libdir}/uscreen/evdi_helper
install -Dm755 bin/libevdi.so.1.15.0 %{buildroot}%{_libdir}/uscreen/libevdi.so.1.15.0
ln -sf libevdi.so.1.15.0            %{buildroot}%{_libdir}/uscreen/libevdi.so.1
install -Dm644 scripts/uscreen.desktop %{buildroot}%{_datadir}/applications/uscreen.desktop
install -Dm644 packaging/uscreen.service %{buildroot}%{_userunitdir}/uscreen.service
install -Dm644 packaging/uscreen-evdi.conf    %{buildroot}%{_modprobedir}/uscreen-evdi.conf
install -Dm644 packaging/uscreen-modules.conf %{buildroot}%{_modulesloaddir}/uscreen.conf
%{_udevrulesdir}/60-uscreen-uinput.rules
install -Dm644 packaging/60-uscreen-uinput.rules %{buildroot}%{_udevrulesdir}/60-uscreen-uinput.rules

%post
# initial_device_count is only read when evdi loads. Reload it so a device
# exists now, not only after the next reboot.
if lsmod | grep -q '^evdi'; then
    modprobe -r evdi 2>/dev/null && modprobe evdi 2>/dev/null || \
        echo "uscreen: evdi is in use; reboot for the virtual display device to appear."
else
    modprobe evdi 2>/dev/null || true
fi
modprobe uinput 2>/dev/null || true
udevadm control --reload 2>/dev/null || true
udevadm trigger --name-match=uinput 2>/dev/null || true

%files
%{_bindir}/uscreen
%{_bindir}/uscreen-gui
%{_libdir}/uscreen/evdi_helper
%{_libdir}/uscreen/libevdi.so.1
%{_libdir}/uscreen/libevdi.so.1.15.0
%{_datadir}/applications/uscreen.desktop
%{_userunitdir}/uscreen.service
%{_modprobedir}/uscreen-evdi.conf
%{_modulesloaddir}/uscreen.conf
