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
else
    echo "Error: Unsupported package manager. Install these packages manually:"
    printf '  %s\n' "${PACKAGES[@]}"
    exit 1
fi

echo "Done — all dependencies installed."
