#!/bin/bash
set -e

REPO="contember/okena"
BOLD="\033[1m"
DIM="\033[2m"
GREEN="\033[32m"
CYAN="\033[36m"
RED="\033[31m"
RESET="\033[0m"

info() { printf "  ${DIM}%s${RESET}\n" "$1"; }
step() { printf "  ${CYAN}>::${RESET} %s\n" "$1"; }
done_msg() { printf "\n  ${GREEN}✓${RESET} ${BOLD}%s${RESET}\n" "$1"; }
err() { printf "  ${RED}error:${RESET} %s\n" "$1" >&2; exit 1; }

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$ARCH" in
  x86_64) ARCH="x64" ;;
  aarch64|arm64) ARCH="arm64" ;;
  *) err "Unsupported architecture: $ARCH" ;;
esac

case "$OS" in
  darwin|linux) ;;
  *) err "Unsupported OS: $OS. For Windows, use install.ps1" ;;
esac

# Get version (from argument or latest release)
if [ -n "$1" ]; then
  VERSION="$1"
else
  step "Fetching latest version"
  VERSION=$(curl -sL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"v([^"]+)".*/\1/')
fi

[ -z "$VERSION" ] && err "Failed to determine version. Usage: install.sh [version]"

echo ""
printf "  ${BOLD}Okena${RESET} ${DIM}v%s${RESET}  ${DIM}%s/%s${RESET}\n" "$VERSION" "$OS" "$ARCH"
echo ""

# Create temp directory
TMP_DIR=$(mktemp -d)
trap "rm -rf $TMP_DIR" EXIT

if [ "$OS" = "darwin" ]; then
  ARTIFACT="okena-macos-${ARCH}"
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ARTIFACT}.zip"

  step "Downloading"
  curl -sL "$DOWNLOAD_URL" -o "$TMP_DIR/okena.zip"

  step "Extracting"
  unzip -q "$TMP_DIR/okena.zip" -d "$TMP_DIR"

  if [ -d "/Applications/Okena.app" ]; then
    step "Removing previous installation"
    rm -rf "/Applications/Okena.app"
  fi

  step "Installing to /Applications"
  mv "$TMP_DIR/Okena.app" "/Applications/"

  step "Clearing quarantine"
  chmod +x "/Applications/Okena.app/Contents/MacOS/okena"
  xattr -cr "/Applications/Okena.app" 2>/dev/null || true

  done_msg "Installed to /Applications/Okena.app"
  echo ""
  info "Launch from Applications, Spotlight, or run:"
  printf "  ${BOLD}open /Applications/Okena.app${RESET}\n"
  echo ""

else
  ARTIFACT="okena-linux-x64"
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ARTIFACT}.tar.gz"

  step "Downloading"
  curl -sL "$DOWNLOAD_URL" | tar xz -C "$TMP_DIR"

  INSTALL_DIR="${HOME}/.local/bin"
  mkdir -p "$INSTALL_DIR"

  step "Installing binary to $INSTALL_DIR"
  mv "$TMP_DIR/okena" "$INSTALL_DIR/"
  chmod +x "$INSTALL_DIR/okena"

  step "Installing icons and desktop entry"

  ICON_DIR="${HOME}/.local/share/icons/hicolor"
  for size in 16 32 48 64 128 256 512; do
    mkdir -p "$ICON_DIR/${size}x${size}/apps"
    if [ -f "$TMP_DIR/icons/app-icon-${size}.png" ]; then
      cp "$TMP_DIR/icons/app-icon-${size}.png" "$ICON_DIR/${size}x${size}/apps/okena.png"
    fi
  done

  mkdir -p "$ICON_DIR/scalable/apps"
  if [ -f "$TMP_DIR/icons/app-icon-simple.svg" ]; then
    cp "$TMP_DIR/icons/app-icon-simple.svg" "$ICON_DIR/scalable/apps/okena.svg"
  fi

  DESKTOP_DIR="${HOME}/.local/share/applications"
  mkdir -p "$DESKTOP_DIR"
  if [ -f "$TMP_DIR/okena.desktop" ]; then
    cp "$TMP_DIR/okena.desktop" "$DESKTOP_DIR/"
  fi

  if command -v gtk-update-icon-cache &> /dev/null; then
    gtk-update-icon-cache -f -t "$ICON_DIR" 2>/dev/null || true
  fi
  if command -v update-desktop-database &> /dev/null; then
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
  fi

  done_msg "Installed successfully"
  echo ""
  info "Binary:  $INSTALL_DIR/okena"
  info "Desktop: $DESKTOP_DIR/okena.desktop"
  echo ""

  if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    printf "  ${DIM}Note: Add %s to your PATH:${RESET}\n" "$INSTALL_DIR"
    printf "  ${BOLD}export PATH=\"\$HOME/.local/bin:\$PATH\"${RESET}\n"
    echo ""
  fi

  info "Launch from your application menu or run:"
  printf "  ${BOLD}okena${RESET}\n"
  echo ""
fi
