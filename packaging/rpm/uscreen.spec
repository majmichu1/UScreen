Name:           uscreen
Version:        1.1.0
Release:        1%{?dist}
Summary:        Android tablet as a USB second display and graphics tablet
License:        MIT
URL:            https://github.com/majmichu1/UScreen
Source0:        uscreen-%{version}-linux-x86_64.tar.gz
BuildArch:      x86_64
Requires:       ffmpeg android-tools libdrm
# evdi is packaged on openSUSE (evdi, libevdi1) but not on Fedora at all;
# it is recommended rather than required so the package installs on both.
Recommends:     evdi libevdi1

%description
Turns an Android tablet into a low-latency second monitor for a KDE Wayland
desktop over USB, with touch and stylus forwarded back. Can also act as a
plain graphics tablet for the host's own screen.

%prep
%setup -q -n uscreen-%{version}

%install
install -Dm755 bin/uscreen     %{buildroot}%{_bindir}/uscreen
install -Dm755 bin/uscreen-gui %{buildroot}%{_bindir}/uscreen-gui
install -Dm755 bin/evdi_helper %{buildroot}%{_libdir}/uscreen/evdi_helper
install -Dm644 scripts/uscreen.desktop %{buildroot}%{_datadir}/applications/uscreen.desktop
install -Dm644 scripts/uscreen.service %{buildroot}%{_userunitdir}/uscreen.service
install -Dm644 packaging/uscreen-evdi.conf    %{buildroot}%{_modprobedir}/uscreen-evdi.conf
install -Dm644 packaging/uscreen-modules.conf %{buildroot}%{_modulesloaddir}/uscreen.conf

%post
if lsmod | grep -q '^evdi'; then
    modprobe -r evdi 2>/dev/null && modprobe evdi 2>/dev/null || \
        echo "uscreen: evdi is in use; reboot for the virtual display device to appear."
else
    modprobe evdi 2>/dev/null || true
fi
modprobe uinput 2>/dev/null || true

%files
%{_bindir}/uscreen
%{_bindir}/uscreen-gui
%{_libdir}/uscreen/evdi_helper
%{_datadir}/applications/uscreen.desktop
%{_userunitdir}/uscreen.service
%{_modprobedir}/uscreen-evdi.conf
%{_modulesloaddir}/uscreen.conf
