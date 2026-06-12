#!/usr/bin/env bash
# UScreen installer — works from a source checkout or a release tarball.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BIN_DIR="${HOME}/.local/bin"
APP_DIR="${HOME}/.local/share/applications"

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
info()  { echo -e "${GREEN}[INFO]${NC} $1"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; }

install_deps() {
    . /etc/os-release 2>/dev/null || true
    case "${ID:-unknown}" in
        fedora|bazzite|nobara|*silverblue*)
            if command -v rpm-ostree &>/dev/null && [ -e /run/ostree-booted ]; then
                info "Immutable Fedora detected — layering packages (reboot needed afterwards)"
                sudo rpm-ostree install --idempotent --allow-inactive \
                    ffmpeg android-tools evdi libevdi || true
            else
                sudo dnf install -y ffmpeg android-tools evdi-dkms libevdi || \
                sudo dnf install -y ffmpeg android-tools kernel-devel || true
            fi
            ;;
        ubuntu|debian|pop|linuxmint)
            sudo apt-get update
            sudo apt-get install -y ffmpeg adb evdi-dkms libevdi0 || \
            sudo apt-get install -y ffmpeg android-tools-adb evdi-dkms
            ;;
        arch|manjaro|endeavouros|cachyos)
            sudo pacman -S --needed --noconfirm ffmpeg android-tools evdi
            ;;
        *)
            warn "Unknown distro. Install manually: ffmpeg, adb (android-tools), evdi + libevdi"
            ;;
    esac
}

build_if_needed() {
    # Release tarballs ship prebuilt binaries next to this script's parent
    if [ -f "$PROJECT_DIR/bin/uscreen" ]; then
        return
    fi
    if [ ! -f "$PROJECT_DIR/target/release/uscreen" ]; then
        info "Building from source (needs rust + gcc)..."
        make -C "$PROJECT_DIR" build
    fi
}

install_files() {
    mkdir -p "$BIN_DIR" "$APP_DIR"
    local src_bin
    if [ -f "$PROJECT_DIR/bin/uscreen" ]; then
        src_bin="$PROJECT_DIR/bin"
    else
        src_bin="$PROJECT_DIR/target/release"
    fi

    rm -f "$BIN_DIR/uscreen" "$BIN_DIR/uscreen-gui" "$BIN_DIR/evdi_helper"
    cp "$src_bin/uscreen" "$BIN_DIR/uscreen"
    cp "$src_bin/uscreen-gui" "$BIN_DIR/uscreen-gui" 2>/dev/null || warn "uscreen-gui not found, skipping"
    if [ -f "$src_bin/evdi_helper" ]; then
        cp "$src_bin/evdi_helper" "$BIN_DIR/evdi_helper"
    else
        cp "$PROJECT_DIR/host/evdi/evdi_helper" "$BIN_DIR/evdi_helper"
    fi
    chmod +x "$BIN_DIR/uscreen" "$BIN_DIR/evdi_helper"
    info "Binaries installed to $BIN_DIR"

    cp "$SCRIPT_DIR/uscreen.desktop" "$APP_DIR/" && info "Desktop entry installed (UScreen in the app menu)"

    mkdir -p "${HOME}/.config/systemd/user"
    cp "$SCRIPT_DIR/uscreen.service" "${HOME}/.config/systemd/user/" 2>/dev/null || true
    systemctl --user daemon-reload 2>/dev/null || true
}

system_setup() {
    info "System setup (needs sudo): EVDI device at every boot"
    echo "options evdi initial_device_count=1" | sudo tee /etc/modprobe.d/uscreen-evdi.conf >/dev/null
    printf "evdi\nuinput\n" | sudo tee /etc/modules-load.d/uscreen.conf >/dev/null
    sudo modprobe evdi 2>/dev/null || warn "evdi module not available yet (reboot after installing evdi-dkms)"
    sudo modprobe uinput 2>/dev/null || true
    if [ "$(cat /sys/devices/evdi/count 2>/dev/null || echo 0)" = "0" ]; then
        echo 1 | sudo tee /sys/devices/evdi/add >/dev/null 2>&1 || true
    fi
}

main() {
    echo "================================================"
    echo "  UScreen installer"
    echo "================================================"
    install_deps
    build_if_needed
    install_files
    system_setup
    echo ""
    info "Done! Launch 'UScreen' from your app menu (or run: uscreen-gui)"
    info "Install the APK on your tablet, enable USB debugging, plug in — that's it."
}

main "$@"
