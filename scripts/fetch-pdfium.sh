#!/usr/bin/env bash
#
# Fetch the PDFium dynamic library (used for scanned-PDF OCR) into
# src-tauri/libs/. Idempotent for the pinned version: a no-op only when both
# the dylib and its version stamp match, so upgrades cannot reuse stale bytes.
#
# Usage:  scripts/fetch-pdfium.sh [arch]     arch defaults to `uname -m`
#
set -euo pipefail

# macOS-only asset; a no-op elsewhere so it's safe as a cross-platform
# postinstall hook.
[ "$(uname -s)" = "Darwin" ] || exit 0

cd "$(dirname "$0")/.."
DEST="src-tauri/libs/libpdfium.dylib"
STAMP="src-tauri/libs/.pdfium-version"
PDFIUM_REVISION="8035"
PDFIUM_VERSION="154.0.8035.0"

if [ -z "${PDFIUM_SOURCE_ARCHIVE:-}" ] && [ -f "$DEST" ] && [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$PDFIUM_VERSION" ]; then
  exit 0
fi

ARCH="${1:-$(uname -m)}"
case "$ARCH" in
  arm64 | aarch64)
    PKG="pdfium-mac-arm64"
    SHA256="308fd9c6eff1be5b7bde62e7a9a42f525075901314a2a50058ae0b6ea0ff30a2"
    ;;
  x86_64)
    PKG="pdfium-mac-x64"
    SHA256="9170dd3bb0f14a712369dd8a1978e77e0b5a05c4371aca2ee49727daabf3201a"
    ;;
  *)
    echo "fetch-pdfium: unsupported arch '$ARCH'" >&2
    exit 1
    ;;
esac

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
if [ -n "${PDFIUM_SOURCE_ARCHIVE:-}" ]; then
  # CI supplies a deliberately corrupt local fixture through this path. It is
  # never trusted: the same committed production digest still gates it.
  cp "$PDFIUM_SOURCE_ARCHIVE" "$TMP/pdfium.tgz"
else
  echo "fetch-pdfium: downloading $PKG at pinned PDFium $PDFIUM_VERSION..."
  EFFECTIVE_URL="$(curl -fsSL --proto '=https' --proto-redir '=https' --max-redirs 5 \
    --write-out '%{url_effective}' -o "$TMP/pdfium.tgz" \
    "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/${PDFIUM_REVISION}/${PKG}.tgz")"
  case "$EFFECTIVE_URL" in
    https://github.com/* | https://release-assets.githubusercontent.com/*) ;;
    *)
      echo "fetch-pdfium: refusing unexpected download host: $EFFECTIVE_URL" >&2
      exit 1
      ;;
  esac
fi
# Native code is verified before extraction and before Alchemy applies its own
# release signature. TLS or downstream code signing alone cannot establish
# that these are the exact reviewed upstream bytes.
printf '%s  %s\n' "$SHA256" "$TMP/pdfium.tgz" | shasum -a 256 -c -
tar xzf "$TMP/pdfium.tgz" -C "$TMP" lib/libpdfium.dylib
mkdir -p src-tauri/libs
mv "$TMP/lib/libpdfium.dylib" "$DEST"
printf '%s\n' "$PDFIUM_VERSION" > "$STAMP"
echo "fetch-pdfium: installed $DEST"
