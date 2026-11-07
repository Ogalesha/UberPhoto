#!/usr/bin/env bash
# Build UberPhoto.app and the .tar.gz archive for macOS.
# Usage: packaging/macos/bundle.sh [version] [binary-path]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VERSION="${1:-${VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n1)}}"
BIN="${2:-${BIN:-$ROOT/target/release/uberphoto-app}}"
EXIFTOOL_VERSION="${EXIFTOOL_VERSION:-13.59}"
EXIFTOOL_SHA256="${EXIFTOOL_SHA256:-668ea3acececb7235fbd0f4900e72d5f12c9b07e5c778fd36cb1e9b5828fd65a}"
APP="$ROOT/target/UberPhoto.app"

if [[ ! -x "$BIN" ]]; then
    echo "binary not found: $BIN (run cargo build --release -p uberphoto-app first)" >&2
    exit 1
fi

# ExifTool bundled in the app: no external dependency to install.
CACHE="$ROOT/target/exiftool-cache"
SRC="$CACHE/Image-ExifTool-$EXIFTOOL_VERSION"
ARCHIVE="$CACHE/Image-ExifTool-$EXIFTOOL_VERSION.tar.gz"

verify_sha256() {
    local file="$1" expected="$2" actual
    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$file" | awk '{print $1}')"
    else
        actual="$(shasum -a 256 "$file" | awk '{print $1}')"
    fi
    if [[ "$actual" != "$expected" ]]; then
        echo "invalid ExifTool checksum for $file: $actual" >&2
        return 1
    fi
}

if [[ ! -f "$SRC/exiftool" ]]; then
    mkdir -p "$CACHE"
    if [[ ! -f "$ARCHIVE" ]]; then
        curl -fsSL --retry 3 -o "$ARCHIVE" \
            "https://sourceforge.net/projects/exiftool/files/Image-ExifTool-$EXIFTOOL_VERSION.tar.gz/download"
    fi
    verify_sha256 "$ARCHIVE" "$EXIFTOOL_SHA256"
    tar -xzf "$ARCHIVE" -C "$CACHE"
elif [[ -f "$ARCHIVE" ]]; then
    verify_sha256 "$ARCHIVE" "$EXIFTOOL_SHA256"
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
install -m755 "$BIN" "$APP/Contents/MacOS/uberphoto"

install -d "$APP/Contents/Resources/exiftool"
cp -a "$SRC/lib" "$APP/Contents/Resources/exiftool/lib"
cp "$SRC/exiftool" "$APP/Contents/Resources/exiftool/exiftool.pl"
cp "$SRC/README" "$APP/Contents/Resources/exiftool/README"
cat > "$APP/Contents/Resources/exiftool/exiftool" <<'WRAPPER'
#!/bin/sh
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec perl -I "$DIR/lib" "$DIR/exiftool.pl" "$@"
WRAPPER
chmod 755 "$APP/Contents/Resources/exiftool/exiftool"

# .icns icon from the source image, when Apple tools are available.
if command -v sips >/dev/null && command -v iconutil >/dev/null; then
    ICONSET="$ROOT/target/UberPhoto.iconset"
    rm -rf "$ICONSET"
    mkdir -p "$ICONSET"
    for size in 16 32 64 128 256 512; do
        sips -z "$size" "$size" "$ROOT/assets/UberPhoto.png" \
            --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
        double=$((size * 2))
        sips -z "$double" "$double" "$ROOT/assets/UberPhoto.png" \
            --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/UberPhoto.icns"
    rm -rf "$ICONSET"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>UberPhoto</string>
    <key>CFBundleDisplayName</key><string>UberPhoto</string>
    <key>CFBundleIdentifier</key><string>io.github.ogalesha.uberphoto</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleExecutable</key><string>uberphoto</string>
    <key>CFBundleIconFile</key><string>UberPhoto</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSHumanReadableCopyright</key><string>Copyright (c) 2026 Mirgen — MIT License</string>
</dict>
</plist>
PLIST

ARCHIVE="$ROOT/target/uberphoto-macos-${MACOS_ARCH:-aarch64}.app.tar.gz"
tar -czf "$ARCHIVE" -C "$ROOT/target" UberPhoto.app
echo "$ARCHIVE"
