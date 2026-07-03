#!/usr/bin/env bash
#
# Fetch the PDFium dynamic library (used for scanned-PDF OCR) into
# src-tauri/libs/. Idempotent: a no-op if the dylib is already present, so it's
# safe to call from build.rs, CI, and scripts/release.sh alike.
#
# Usage:  scripts/fetch-pdfium.sh [arch]     arch defaults to `uname -m`
#
set -euo pipefail

# macOS-only asset; a no-op elsewhere so it's safe as a cross-platform
# postinstall hook.
[ "$(uname -s)" = "Darwin" ] || exit 0

cd "$(dirname "$0")/.."
DEST="src-tauri/libs/libpdfium.dylib"

[ -f "$DEST" ] && exit 0

ARCH="${1:-$(uname -m)}"
case "$ARCH" in
  arm64 | aarch64) PKG="pdfium-mac-arm64" ;;
  x86_64) PKG="pdfium-mac-x64" ;;
  *)
    echo "fetch-pdfium: unsupported arch '$ARCH'" >&2
    exit 1
    ;;
esac

echo "fetch-pdfium: downloading $PKG..."
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL -o "$TMP/pdfium.tgz" \
  "https://github.com/bblanchon/pdfium-binaries/releases/latest/download/${PKG}.tgz"
tar xzf "$TMP/pdfium.tgz" -C "$TMP" lib/libpdfium.dylib
mkdir -p src-tauri/libs
mv "$TMP/lib/libpdfium.dylib" "$DEST"
echo "fetch-pdfium: installed $DEST"
