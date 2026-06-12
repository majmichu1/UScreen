#!/usr/bin/env bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC} $1"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; }

check_bazzite() {
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        if [ "$ID" = "bazzite" ] || [ "$ID_LIKE" = "fedora" ]; then
            return 0
        fi
    fi
    return 1
}

list_connectors() {
    echo "Available DRM connectors:"
    for path in /sys/class/drm/card*-*/status; do
        local connector="${path%/status}"
        connector="${connector#*/card?-}"
        local status
        status=$(cat "$path" 2>/dev/null || echo "unknown")
        printf "  %-20s %s\n" "$connector" "$status"
    done

    if command -v kscreen-doctor &>/dev/null; then
        echo ""
        echo "KDE Plasma displays:"
        kscreen-doctor -o 2>/dev/null || true
    fi
}

setup_with_crh() {
    info "Using Bazzite's custom-resolution-helper..."
    if command -v custom-resolution-helper &>/dev/null; then
        info "Run: sudo custom-resolution-helper add"
        echo ""
        echo "This will guide you through:"
        echo "  1. Selecting a display port to use as virtual"
        echo "  2. Setting resolution (2960x1848 for Tab S9 Ultra)"
        echo "  3. Enabling 'always on' (the 'e' flag)"
        echo "  4. Rebooting to apply"
    else
        error "custom-resolution-helper not found. Install bazzite or use manual method."
        return 1
    fi
}

setup_manual() {
    echo "================================================"
    echo "  Manual Virtual Display Setup"
    echo "================================================"
    echo ""

    echo "Step 1: List available connectors"
    list_connectors
    echo ""

    echo "Step 2: Pick an unused connector (e.g., DP-2, HDMI-A-1)"
    read -rp "Enter connector name: " CONNECTOR
    read -rp "Enter resolution (e.g., 2960x1848): " RESOLUTION
    read -rp "Enter refresh rate Hz (e.g., 60): " REFRESH

    echo ""
    echo "Step 3: Add kernel parameter"
    local KARG="video=${CONNECTOR}:${RESOLUTION}@${REFRESH}e"

    if check_bazzite || command -v rpm-ostree &>/dev/null; then
        echo "Running: sudo rpm-ostree kargs --append-if-missing=\"$KARG\""
        sudo rpm-ostree kargs --append-if-missing="$KARG"
        echo "Optionally add EDID: sudo rpm-ostree kargs --append-if-missing=\"drm.edid_firmware=${CONNECTOR}:edid/s9ultra.bin\""
    elif command -v grubby &>/dev/null; then
        warn "RHEL/Fedora detected. Add to GRUB_CMDLINE_LINUX in /etc/default/grub"
    else
        warn "Add '$KARG' to your kernel command line and reboot."
        if [ -f /etc/default/grub ]; then
            echo "Edit /etc/default/grub, add to GRUB_CMDLINE_LINUX, then run: sudo update-grub"
        fi
    fi

    echo ""
    echo "After reboot, verify with: kscreen-doctor -o"
    echo "Then start uscreen: uscreen --display $CONNECTOR"
}

setup_evdi() {
    echo "================================================"
    echo "  EVDI-based Virtual Display Setup"
    echo "================================================"
    echo ""

    if command -v rpm-ostree &>/dev/null; then
        info "For Bazzite, you can rebase to an EVDI-enabled image:"
        echo "  sudo rpm-ostree rebase ostree-unverified-registry:ghcr.io/opdude/bazzite-evdi:stable"
        echo "  sudo systemctl reboot"
        echo ""
        warn "This switches your system to a community build!"
        echo ""
    fi

    info "Or install EVDI manually:"
    echo "  sudo dnf install evdi-dkms libevdi"
    echo "  sudo modprobe evdi"
    echo "  ls /dev/dri/card* (should show new card)"
}

main() {
    echo "================================================"
    echo "  UScreen Virtual Display Setup"
    echo "================================================"
    echo ""

    PS3="Select method: "
    options=(
        "custom-resolution-helper (Bazzite/KDE - recommended)"
        "Manual kernel parameter setup"
        "EVDI kernel module"
        "List available connectors"
        "Cancel"
    )

    select opt in "${options[@]}"; do
        case $REPLY in
            1) setup_with_crh; break ;;
            2) setup_manual; break ;;
            3) setup_evdi; break ;;
            4) list_connectors; break ;;
            5) echo "Canceled"; exit 0 ;;
            *) echo "Invalid option" ;;
        esac
    done

    echo ""
    echo "After setting up the virtual display and rebooting:"
    echo "  1. Verify: kscreen-doctor -o"
    echo "  2. Start: uscreen --display <YOUR_CONNECTOR>"
}

main "$@"
