.PHONY: all build build-helper install clean run android adb edid status stop list dist setup-system

VERSION = 0.2.0

CARGO = cargo
CC = gcc
ADB = adb
BIN_DIR = ${HOME}/.local/bin
DATA_DIR = ${HOME}/.local/share/uscreen

all: build

build-helper:
	$(CC) -O3 -march=native -o host/evdi/evdi_helper host/evdi/evdi_helper.c -levdi -ldrm -lpthread -Ihost/evdi -I/usr/include
	@echo "✓ EVDI helper: host/evdi/evdi_helper"

build: build-helper
	$(CARGO) build --release
	@echo "✓ Binaries: $(PWD)/target/release/uscreen and uscreen-gui"

install: build edid
	mkdir -p $(BIN_DIR) $(DATA_DIR)/edid
	# rm first: cp into a running binary fails with "text file busy"
	rm -f $(BIN_DIR)/uscreen $(BIN_DIR)/uscreen-gui $(BIN_DIR)/evdi_helper
	cp target/release/uscreen $(BIN_DIR)/uscreen
	cp target/release/uscreen-gui $(BIN_DIR)/uscreen-gui
	cp host/evdi/evdi_helper $(BIN_DIR)/evdi_helper
	cp edid/s9ultra.bin $(DATA_DIR)/edid/
	@echo "✓ Installed to $(BIN_DIR)/uscreen, $(BIN_DIR)/uscreen-gui and $(BIN_DIR)/evdi_helper"
	mkdir -p ${HOME}/.local/share/applications
	cp scripts/uscreen.desktop ${HOME}/.local/share/applications/ 2>/dev/null || true
	@echo "✓ Desktop entry installed (UScreen in the app menu)"
	mkdir -p ${HOME}/.config/systemd/user/ 2>/dev/null || true
	cp scripts/uscreen.service ${HOME}/.config/systemd/user/ 2>/dev/null || true
	systemctl --user daemon-reload 2>/dev/null || true
	@echo "✓ systemd user service installed"

# One-time system setup (needs sudo): pre-create an EVDI device at boot so
# the daemon never needs root, and load the required modules.
setup-system:
	echo "options evdi initial_device_count=1" | sudo tee /etc/modprobe.d/uscreen-evdi.conf
	printf "evdi\nuinput\n" | sudo tee /etc/modules-load.d/uscreen.conf
	sudo modprobe evdi || true
	sudo modprobe uinput || true
	@if [ "$$(cat /sys/devices/evdi/count 2>/dev/null || echo 0)" = "0" ]; then \
		echo 1 | sudo tee /sys/devices/evdi/add; \
	fi
	@echo "✓ System setup done (EVDI device available now and at every boot)"

run: build
	sudo modprobe -q evdi || true
	sudo modprobe -q uinput || true
	./target/release/uscreen start

adb:
	$(ADB) reverse tcp:8890 tcp:8890
	$(ADB) reverse tcp:8891 tcp:8891
	@echo "✓ ADB ports: 8890 (video), 8891 (input)"

android:
	cd android && ./gradlew assembleDebug --no-daemon
	@echo "✓ APK: android/app/build/outputs/apk/debug/app-debug.apk"

android-install: android
	$(ADB) install -r android/app/build/outputs/apk/debug/app-debug.apk
	@echo "✓ APK installed on device"

edid:
	mkdir -p edid
	python3 scripts/gen-edid.py 2960 1848 60 edid/s9ultra.bin
	@echo "✓ EDID generated: edid/s9ultra.bin"

list:
	./target/release/uscreen list-displays

status:
	./target/release/uscreen status

stop:
	./target/release/uscreen stop

# Release tarball: prebuilt binaries + installer. Upload to GitHub releases
# together with the release APK (android/app/build/outputs/apk/release/).
dist: build
	rm -rf dist/uscreen-$(VERSION)
	mkdir -p dist/uscreen-$(VERSION)/bin dist/uscreen-$(VERSION)/scripts
	cp target/release/uscreen target/release/uscreen-gui host/evdi/evdi_helper dist/uscreen-$(VERSION)/bin/
	cp scripts/install.sh scripts/uscreen.desktop scripts/uscreen.service scripts/51-uscreen.rules dist/uscreen-$(VERSION)/scripts/
	cp README.md dist/uscreen-$(VERSION)/
	cd android && ./gradlew assembleRelease -q && cp app/build/outputs/apk/release/app-release.apk ../dist/uscreen-$(VERSION)/uscreen.apk 2>/dev/null || true
	tar -C dist -czf dist/uscreen-$(VERSION)-linux-x86_64.tar.gz uscreen-$(VERSION)
	@echo "✓ Release: dist/uscreen-$(VERSION)-linux-x86_64.tar.gz"
	@ls dist/uscreen-$(VERSION)/uscreen.apk 2>/dev/null && echo "✓ APK: dist/uscreen-$(VERSION)/uscreen.apk" || true

clean:
	cd host && $(CARGO) clean
	rm -f host/evdi/evdi_helper
	rm -rf dist
	@echo "✓ Cleaned"
