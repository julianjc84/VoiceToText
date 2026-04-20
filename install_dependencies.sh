#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEP_FILE="$SCRIPT_DIR/dependencies.txt"

if [[ ! -f "$DEP_FILE" ]]; then
    echo "Error: dependencies.txt not found at $DEP_FILE"
    exit 1
fi

# Read package names from dependencies.txt, skipping comments and blank lines
mapfile -t PACKAGES < <(grep -v '^\s*#' "$DEP_FILE" | grep -v '^\s*$')

# Map Debian/Ubuntu package names to Fedora/RHEL equivalents
declare -A FEDORA_MAP=(
    [build-essential]="gcc gcc-c++ make"
    [libgtk-3-dev]="gtk3-devel"
    [libasound2-dev]="alsa-lib-devel"
    [libxdo-dev]="libxdo-devel"
    [xclip]="xclip"
    [wl-clipboard]="wl-clipboard"
)

# Map Debian/Ubuntu package names to Arch Linux equivalents
declare -A ARCH_MAP=(
    [build-essential]="base-devel"
    [cmake]="cmake"
    [libgtk-3-dev]="gtk3"
    [libasound2-dev]="alsa-lib"
    [libxdo-dev]="xdotool"
    [xdotool]="xdotool"
    [xclip]="xclip"
    [wtype]="wtype"
    [ydotool]="ydotool"
    [wl-clipboard]="wl-clipboard"
)

# Arch-specific extras not in dependencies.txt
ARCH_EXTRAS=(pipewire-alsa)

if command -v apt &>/dev/null; then
    echo "Detected Debian/Ubuntu — using apt"
    echo "Installing: ${PACKAGES[*]}"
    sudo apt install -y "${PACKAGES[@]}"
elif command -v dnf &>/dev/null; then
    echo "Detected Fedora/RHEL — using dnf"
    FEDORA_PACKAGES=()
    for pkg in "${PACKAGES[@]}"; do
        if [[ -n "${FEDORA_MAP[$pkg]+x}" ]]; then
            # shellcheck disable=SC2206
            FEDORA_PACKAGES+=(${FEDORA_MAP[$pkg]})
        else
            FEDORA_PACKAGES+=("$pkg")
        fi
    done
    echo "Installing: ${FEDORA_PACKAGES[*]}"
    sudo dnf install -y "${FEDORA_PACKAGES[@]}"
elif command -v pacman &>/dev/null; then
    echo "Detected Arch Linux — using pacman"
    ARCH_PACKAGES=()
    for pkg in "${PACKAGES[@]}"; do
        if [[ -n "${ARCH_MAP[$pkg]+x}" ]]; then
            # shellcheck disable=SC2206
            ARCH_PACKAGES+=(${ARCH_MAP[$pkg]})
        else
            ARCH_PACKAGES+=("$pkg")
        fi
    done
    # Add Arch-specific extras
    ARCH_PACKAGES+=("${ARCH_EXTRAS[@]}")
    # Deduplicate
    mapfile -t ARCH_PACKAGES < <(printf '%s\n' "${ARCH_PACKAGES[@]}" | sort -u)
    echo "Installing: ${ARCH_PACKAGES[*]}"
    sudo pacman -S --needed "${ARCH_PACKAGES[@]}"

    # Set up ydotool system service for Wayland compositors without
    # virtual-keyboard-v1 support (KWin/Mutter: KDE Plasma, GNOME).
    # Not needed on wlroots/smithay (Sway, Hyprland, niri) — wtype handles
    # typing directly there — but installing the unit is harmless.
    if [[ ! -f /etc/systemd/system/ydotoold.service ]]; then
        echo "Setting up ydotoold system service..."
        sudo tee /etc/systemd/system/ydotoold.service > /dev/null <<'UNIT'
[Unit]
Description=ydotoold daemon

[Service]
ExecStartPre=/usr/sbin/modprobe uinput
ExecStart=/usr/bin/ydotoold --socket-path=/run/ydotoold/socket --socket-perm=0666
RuntimeDirectory=ydotoold
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
UNIT
        sudo systemctl daemon-reload
        sudo systemctl enable --now ydotoold
        echo "ydotoold service installed and started."
    fi

    # Ensure uinput module loads at boot
    if [[ ! -f /etc/modules-load.d/uinput.conf ]]; then
        echo uinput | sudo tee /etc/modules-load.d/uinput.conf > /dev/null
    fi

    # Add user to input group if not already a member
    if ! groups | grep -qw input; then
        echo "Adding $USER to 'input' group (log out and back in to take effect)..."
        sudo usermod -aG input "$USER"
    fi
else
    echo "Error: Unsupported package manager. Install these packages manually:"
    printf '  %s\n' "${PACKAGES[@]}"
    exit 1
fi

echo "Done — all dependencies installed."
