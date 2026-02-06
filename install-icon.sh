#!/bin/bash
# Install Okena icon and desktop entry

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ASSETS_DIR="$SCRIPT_DIR/assets"
DESKTOP_SOURCE="$SCRIPT_DIR/okena.desktop"

echo "Installing Okena icons..."

# Install PNG icons at various sizes
for size in 16 32 48 64 128 256 512; do
    ICON_DIR="$HOME/.local/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$ICON_DIR"
    if [ -f "$ASSETS_DIR/app-icon-${size}.png" ]; then
        cp "$ASSETS_DIR/app-icon-${size}.png" "$ICON_DIR/okena.png"
        echo "  Installed ${size}x${size} PNG icon"
    fi
done

# Install desktop entry
DESKTOP_DIR="$HOME/.local/share/applications"
mkdir -p "$DESKTOP_DIR"
cp "$DESKTOP_SOURCE" "$DESKTOP_DIR/okena.desktop"
echo "Installed desktop entry"

# Update icon cache
if command -v gtk-update-icon-cache &> /dev/null; then
    gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
    echo "Updated icon cache"
fi

echo ""
echo "Installation complete!"
echo ""
echo "To use the icon:"
echo "  1. Run: cargo run"
echo "  2. The icon should appear in your task panel"
echo ""
echo "Note: You may need to log out and log back in for changes to take effect."
