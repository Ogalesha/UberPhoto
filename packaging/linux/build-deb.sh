#!/usr/bin/env bash
# Crea un pacchetto Debian (.deb) per UberPhoto.
# Uso: packaging/linux/build-deb.sh [versione] [percorso-binario]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VERSION="${1:-${VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n1)}}"
BIN="${2:-$ROOT/target/release/uberphoto-app}"
ARCH="${ARCH:-$(dpkg --print-architecture)}"
EXIFTOOL_VERSION="${EXIFTOOL_VERSION:-13.59}"
STAGE="$ROOT/target/deb/uberphoto_${VERSION}_${ARCH}"

if [[ ! -x "$BIN" ]]; then
    echo "binario non trovato: $BIN (esegui prima cargo build --release -p uberphoto-app)" >&2
    exit 1
fi

# ExifTool incluso nel pacchetto: nessuna dipendenza esterna da installare.
CACHE="$ROOT/target/exiftool-cache"
SRC="$CACHE/Image-ExifTool-$EXIFTOOL_VERSION"
if [[ ! -f "$SRC/exiftool" ]]; then
    mkdir -p "$CACHE"
    ARCHIVE="$CACHE/Image-ExifTool-$EXIFTOOL_VERSION.tar.gz"
    if [[ ! -f "$ARCHIVE" ]]; then
        curl -fsSL --retry 3 -o "$ARCHIVE" \
            "https://sourceforge.net/projects/exiftool/files/Image-ExifTool-$EXIFTOOL_VERSION.tar.gz/download"
    fi
    tar -xzf "$ARCHIVE" -C "$CACHE"
fi

rm -rf "$STAGE"
install -Dm755 "$BIN" "$STAGE/usr/bin/uberphoto"
install -Dm644 "$ROOT/assets/UberPhoto.png" "$STAGE/usr/share/icons/hicolor/512x512/apps/uberphoto.png"
install -Dm644 "$ROOT/packaging/linux/uberphoto.desktop" "$STAGE/usr/share/applications/uberphoto.desktop"
mkdir -p "$STAGE/usr/share/doc/uberphoto"
install -m644 "$ROOT/README.md" "$STAGE/usr/share/doc/uberphoto/README.md"
install -m644 "$ROOT/CHANGELOG.md" "$STAGE/usr/share/doc/uberphoto/CHANGELOG.md"

install -d "$STAGE/usr/lib/uberphoto/exiftool"
cp -a "$SRC/lib" "$STAGE/usr/lib/uberphoto/exiftool/lib"
cp "$SRC/exiftool" "$STAGE/usr/lib/uberphoto/exiftool/exiftool.pl"
cp "$SRC/README" "$STAGE/usr/lib/uberphoto/exiftool/README"
cat > "$STAGE/usr/lib/uberphoto/exiftool/exiftool" <<'WRAPPER'
#!/bin/sh
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec perl -I "$DIR/lib" "$DIR/exiftool.pl" "$@"
WRAPPER
chmod 755 "$STAGE/usr/lib/uberphoto/exiftool/exiftool"
if command -v perl >/dev/null; then
    perl -I "$STAGE/usr/lib/uberphoto/exiftool/lib" \
        "$STAGE/usr/lib/uberphoto/exiftool/exiftool.pl" -ver >/dev/null
fi

SIZE="$(du -sk "$STAGE/usr" | cut -f1)"
mkdir -p "$STAGE/DEBIAN"
sed -e "s/@VERSION@/$VERSION/" -e "s/@ARCH@/$ARCH/" -e "s/@SIZE@/$SIZE/" \
    "$ROOT/packaging/linux/control.in" > "$STAGE/DEBIAN/control"

DEB="$ROOT/target/uberphoto_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$STAGE" "$DEB" >/dev/null
echo "$DEB"
