#!/bin/bash
set -e

REPO="contember/okena"

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$ARCH" in
  x86_64) ARCH="x64" ;;
  aarch64|arm64) ARCH="arm64" ;;
  *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

case "$OS" in
  darwin|linux) ;;
  *) echo "Unsupported OS: $OS. For Windows, use install.ps1"; exit 1 ;;
esac

# Get version (from argument or latest release)
if [ -n "$1" ]; then
  VERSION="$1"
else
  echo "Fetching latest version..."
  VERSION=$(curl -sL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"v([^"]+)".*/\1/')
fi

if [ -z "$VERSION" ]; then
  echo "Failed to determine version. Specify version as argument: ./install.sh 1.0.0"
  exit 1
fi

echo "Installing Okena v${VERSION} for ${OS}/${ARCH}..."

# Create temp directory
TMP_DIR=$(mktemp -d)
trap "rm -rf $TMP_DIR" EXIT

if [ "$OS" = "darwin" ]; then
  # macOS installation
  ARTIFACT="okena-macos-${ARCH}"
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ARTIFACT}.zip"

  echo "Downloading from ${DOWNLOAD_URL}..."
  curl -sL "$DOWNLOAD_URL" -o "$TMP_DIR/okena.zip"

  echo "Extracting..."
  unzip -q "$TMP_DIR/okena.zip" -d "$TMP_DIR"

  # Remove old installation if exists
  if [ -d "/Applications/Okena.app" ]; then
    echo "Removing existing installation..."
    rm -rf "/Applications/Okena.app"
  fi

  echo "Installing to /Applications..."
  mv "$TMP_DIR/Okena.app" "/Applications/"

  # Remove quarantine attribute
  echo "Removing quarantine attribute..."
  xattr -rd com.apple.quarantine "/Applications/Okena.app" 2>/dev/null || true

  # Ad-hoc code signing
  echo "Signing application..."
  codesign --force --deep --sign - "/Applications/Okena.app" 2>/dev/null || true

  echo ""
  echo "✓ Okena installed to /Applications/Okena.app"
  echo ""
  echo "You can now launch it from Applications or Spotlight."

else
  # Linux installation
  ARTIFACT="okena-linux-x64"
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ARTIFACT}.tar.gz"

  echo "Downloading from ${DOWNLOAD_URL}..."
  curl -sL "$DOWNLOAD_URL" | tar xz -C "$TMP_DIR"

  # Install binary
  INSTALL_DIR="${HOME}/.local/bin"
  mkdir -p "$INSTALL_DIR"
  mv "$TMP_DIR/okena" "$INSTALL_DIR/"
  chmod +x "$INSTALL_DIR/okena"

  # Install icons
  ICON_DIR="${HOME}/.local/share/icons/hicolor"
  for size in 16 32 48 64 128 256 512; do
    mkdir -p "$ICON_DIR/${size}x${size}/apps"
    if [ -f "$TMP_DIR/icons/app-icon-${size}.png" ]; then
      cp "$TMP_DIR/icons/app-icon-${size}.png" "$ICON_DIR/${size}x${size}/apps/okena.png"
    fi
  done

  # Install scalable icon
  mkdir -p "$ICON_DIR/scalable/apps"
  if [ -f "$TMP_DIR/icons/app-icon-simple.svg" ]; then
    cp "$TMP_DIR/icons/app-icon-simple.svg" "$ICON_DIR/scalable/apps/okena.svg"
  fi

  # Install desktop entry
  DESKTOP_DIR="${HOME}/.local/share/applications"
  mkdir -p "$DESKTOP_DIR"
  if [ -f "$TMP_DIR/okena.desktop" ]; then
    cp "$TMP_DIR/okena.desktop" "$DESKTOP_DIR/"
  fi

  # Update icon cache
  if command -v gtk-update-icon-cache &> /dev/null; then
    gtk-update-icon-cache -f -t "$ICON_DIR" 2>/dev/null || true
  fi

  # Update desktop database
  if command -v update-desktop-database &> /dev/null; then
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
  fi

  echo ""
  echo "✓ Okena installed successfully"
  echo ""
  echo "  Binary: $INSTALL_DIR/okena"
  echo "  Desktop entry: $DESKTOP_DIR/okena.desktop"
  echo ""

  # Check if ~/.local/bin is in PATH
  if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo "Note: $INSTALL_DIR is not in your PATH."
    echo "Add it to your shell profile:"
    echo ""
    echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
    echo ""
  fi

  echo "Launch from your application menu or run: okena"
fi
