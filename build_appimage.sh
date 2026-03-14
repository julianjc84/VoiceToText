#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

TOOLS_DIR="$SCRIPT_DIR/appimage-tools"
LINUXDEPLOY="$TOOLS_DIR/linuxdeploy-x86_64.AppImage"
GTK_PLUGIN="$TOOLS_DIR/linuxdeploy-plugin-gtk.sh"

# Download linuxdeploy and GTK plugin if not present
mkdir -p "$TOOLS_DIR"

if [[ ! -x "$LINUXDEPLOY" ]]; then
    echo "Downloading linuxdeploy..."
    wget -O "$LINUXDEPLOY" \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
    chmod +x "$LINUXDEPLOY"
fi

if [[ ! -x "$GTK_PLUGIN" ]]; then
    echo "Downloading linuxdeploy-plugin-gtk..."
    wget -O "$GTK_PLUGIN" \
        "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"
    chmod +x "$GTK_PLUGIN"
fi

# Build release binary if not present
if [[ ! -f "target/release/voice-to-text" ]]; then
    echo "Building release binary..."
    cargo build --release
fi

# Clean previous AppDir
rm -rf AppDir

# Set up AppDir structure
mkdir -p AppDir/usr/bin
cp target/release/voice-to-text AppDir/usr/bin/

# linuxdeploy requires icon filename to match the Icon= field in the .desktop file
ICON_TMP="$(mktemp -d)/voice-to-text.png"
cp assets/icon-256.png "$ICON_TMP"

# Add gtk-query-immodules-3.0 to PATH if available
GTK3_TOOLS="/usr/lib/x86_64-linux-gnu/libgtk-3-0t64"
if [[ -d "$GTK3_TOOLS" ]]; then
    export PATH="$GTK3_TOOLS:$PATH"
fi

# Build AppImage with GTK plugin
export DEPLOY_GTK_VERSION=3
export PATH="$TOOLS_DIR:$PATH"

"$LINUXDEPLOY" \
    --appdir AppDir \
    --executable target/release/voice-to-text \
    --desktop-file voice-to-text.desktop \
    --icon-file "$ICON_TMP" \
    --plugin gtk \
    --output appimage

echo ""
echo "AppImage built successfully:"
ls -lh Voice_to_Text*.AppImage 2>/dev/null || ls -lh voice-to-text*.AppImage 2>/dev/null || ls -lh *.AppImage
