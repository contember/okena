#!/bin/bash
set -euo pipefail

# macOS App Bundle Script for Muxy
# Usage: ./scripts/bundle-macos.sh [--target <target>] [--skip-build] [--dmg]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Defaults
TARGET=""
SKIP_BUILD=false
CREATE_DMG=false
APP_NAME="Muxy"
BUNDLE_ID="com.contember.muxy"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --dmg)
            CREATE_DMG=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Detect target if not specified
if [[ -z "$TARGET" ]]; then
    ARCH=$(uname -m)
    if [[ "$ARCH" == "arm64" ]]; then
        TARGET="aarch64-apple-darwin"
    else
        TARGET="x86_64-apple-darwin"
    fi
fi

echo "==> Building Muxy for macOS"
echo "    Target: $TARGET"

cd "$PROJECT_ROOT"

# Get version from Cargo.toml
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
echo "    Version: $VERSION"

# Build if not skipping
if [[ "$SKIP_BUILD" == false ]]; then
    echo "==> Building release binary..."
    cargo build --release --target "$TARGET"
fi

# Verify binary exists (check target-specific path first, then default)
BINARY_PATH="target/$TARGET/release/muxy"
if [[ ! -f "$BINARY_PATH" ]]; then
    # Try default release path (when built without --target)
    BINARY_PATH="target/release/muxy"
    if [[ ! -f "$BINARY_PATH" ]]; then
        echo "Error: Binary not found"
        echo "Checked: target/$TARGET/release/muxy"
        echo "Checked: target/release/muxy"
        echo "Run without --skip-build or build first with: cargo build --release"
        exit 1
    fi
    echo "    Using default release binary"
fi

# Setup paths
DIST_DIR="$PROJECT_ROOT/dist"
APP_BUNDLE="$DIST_DIR/$APP_NAME.app"
CONTENTS_DIR="$APP_BUNDLE/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

# Clean and create bundle structure
echo "==> Creating app bundle structure..."
rm -rf "$APP_BUNDLE"
mkdir -p "$MACOS_DIR"
mkdir -p "$RESOURCES_DIR"

# Copy binary
echo "==> Copying binary..."
cp "$BINARY_PATH" "$MACOS_DIR/muxy"
chmod +x "$MACOS_DIR/muxy"

# Create Info.plist with version
echo "==> Creating Info.plist..."
sed "s/VERSION/$VERSION/g" "$PROJECT_ROOT/macos/Info.plist" > "$CONTENTS_DIR/Info.plist"

# Create icns from PNG icons
echo "==> Creating app icon..."
ICONSET_DIR="$DIST_DIR/AppIcon.iconset"
rm -rf "$ICONSET_DIR"
mkdir -p "$ICONSET_DIR"

# Copy PNG files to iconset with correct naming
cp "$PROJECT_ROOT/assets/app-icon-16.png" "$ICONSET_DIR/icon_16x16.png"
cp "$PROJECT_ROOT/assets/app-icon-32.png" "$ICONSET_DIR/icon_16x16@2x.png"
cp "$PROJECT_ROOT/assets/app-icon-32.png" "$ICONSET_DIR/icon_32x32.png"
cp "$PROJECT_ROOT/assets/app-icon-64.png" "$ICONSET_DIR/icon_32x32@2x.png"
cp "$PROJECT_ROOT/assets/app-icon-128.png" "$ICONSET_DIR/icon_128x128.png"
cp "$PROJECT_ROOT/assets/app-icon-256.png" "$ICONSET_DIR/icon_128x128@2x.png"
cp "$PROJECT_ROOT/assets/app-icon-256.png" "$ICONSET_DIR/icon_256x256.png"
cp "$PROJECT_ROOT/assets/app-icon-512.png" "$ICONSET_DIR/icon_256x256@2x.png"
cp "$PROJECT_ROOT/assets/app-icon-512.png" "$ICONSET_DIR/icon_512x512.png"

# Create 512@2x if we have it, otherwise duplicate 512
if [[ -f "$PROJECT_ROOT/assets/app-icon-1024.png" ]]; then
    cp "$PROJECT_ROOT/assets/app-icon-1024.png" "$ICONSET_DIR/icon_512x512@2x.png"
else
    cp "$PROJECT_ROOT/assets/app-icon-512.png" "$ICONSET_DIR/icon_512x512@2x.png"
fi

# Convert to icns
iconutil -c icns "$ICONSET_DIR" -o "$RESOURCES_DIR/AppIcon.icns"
rm -rf "$ICONSET_DIR"

# Create PkgInfo
echo "APPL????" > "$CONTENTS_DIR/PkgInfo"

echo "==> App bundle created at: $APP_BUNDLE"

# Create DMG if requested
if [[ "$CREATE_DMG" == true ]]; then
    echo "==> Creating DMG..."
    DMG_NAME="Muxy-$VERSION-$TARGET.dmg"
    DMG_PATH="$DIST_DIR/$DMG_NAME"

    # Remove existing DMG
    rm -f "$DMG_PATH"

    # Create temporary DMG directory
    DMG_TEMP="$DIST_DIR/dmg-temp"
    rm -rf "$DMG_TEMP"
    mkdir -p "$DMG_TEMP"

    # Copy app to temp directory
    cp -R "$APP_BUNDLE" "$DMG_TEMP/"

    # Create symlink to Applications
    ln -s /Applications "$DMG_TEMP/Applications"

    # Create DMG
    hdiutil create -volname "$APP_NAME" \
        -srcfolder "$DMG_TEMP" \
        -ov -format UDZO \
        "$DMG_PATH"

    # Clean up
    rm -rf "$DMG_TEMP"

    echo "==> DMG created at: $DMG_PATH"
fi

echo "==> Done!"
echo ""
echo "To install, either:"
echo "  1. Drag '$APP_NAME.app' to /Applications"
echo "  2. Run: cp -R \"$APP_BUNDLE\" /Applications/"
