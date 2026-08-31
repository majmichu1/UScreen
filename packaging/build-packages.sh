#!/usr/bin/env bash
# Build the .deb and .rpm from an already-built portable dist tree
# (scripts/build-release.sh). Runs inside the Debian 12 build container,
# which has dpkg-deb and rpmbuild.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION="$(sed -n 's/^VERSION = //p' Makefile)"
D="dist/uscreen-$VERSION"
[ -x "$D/bin/uscreen" ] || { echo "run scripts/build-release.sh first"; exit 1; }

distrobox enter "${USCREEN_BUILD_CONTAINER:-uscreen-build}" -- bash -lc '
  set -e
  cd '"$PWD"'
  V='"$VERSION"'; D='"$D"'

  # ---- .deb ----
  R=dist/deb-root; rm -rf "$R"
  install -Dm755 $D/bin/uscreen        $R/usr/bin/uscreen
  install -Dm755 $D/bin/uscreen-gui    $R/usr/bin/uscreen-gui
  install -Dm755 $D/bin/evdi_helper    $R/usr/lib/uscreen/evdi_helper
  install -Dm644 scripts/uscreen.desktop $R/usr/share/applications/uscreen.desktop
  install -Dm644 scripts/uscreen.service $R/usr/lib/systemd/user/uscreen.service
  install -Dm644 packaging/uscreen-evdi.conf    $R/usr/lib/modprobe.d/uscreen-evdi.conf
  install -Dm644 packaging/uscreen-modules.conf $R/usr/lib/modules-load.d/uscreen.conf
  mkdir -p $R/DEBIAN
  sed "s/^Version: .*/Version: $V/" packaging/deb/control > $R/DEBIAN/control
  install -m755 packaging/deb/postinst $R/DEBIAN/postinst
  fakeroot dpkg-deb --build --root-owner-group $R dist/uscreen_${V}_amd64.deb
  dpkg-deb --info dist/uscreen_${V}_amd64.deb | grep -E "Package|Version|Depends"

  # ---- .rpm ----
  RB=$PWD/dist/rpmbuild; rm -rf $RB; mkdir -p $RB/{SOURCES,SPECS,BUILD,RPMS,SRPMS}
  cp dist/uscreen-$V-linux-x86_64.tar.gz $RB/SOURCES/
  sed "s/^Version:.*/Version:        $V/" packaging/rpm/uscreen.spec > $RB/SPECS/uscreen.spec
  rpmbuild --define "_topdir $RB" --define "_userunitdir /usr/lib/systemd/user" \
           --define "_modprobedir /usr/lib/modprobe.d" --define "_modulesloaddir /usr/lib/modules-load.d" \
           -bb $RB/SPECS/uscreen.spec 2>&1 | grep -E "Wrote|error" 
  cp $RB/RPMS/x86_64/uscreen-$V-*.rpm dist/
'
ls -la dist/*.deb dist/*.rpm
