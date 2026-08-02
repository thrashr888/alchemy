#!/bin/sh
# Build the Foundation Models sidecar (RFC-inference-providers) and stage it
# at src-tauri/binaries/alchemy-fm, which is both where dev resolves it
# (BaseDirectory::Resource maps to src-tauri in dev) and what
# tauri.conf.json bundles into release Resources. Release flows re-sign the
# staged binary with the Developer ID (PDFium precedent) before bundling.
# macOS-only; a no-op elsewhere.
set -eu
[ "$(uname -s)" = "Darwin" ] || exit 0
cd "$(dirname "$0")/.."
SOURCE_DIR="sidecar/alchemy-fm"
DEST="src-tauri/binaries/alchemy-fm"

# pnpm install invokes this hook even when the Swift package has not changed.
# Reuse the staged executable until a tracked package/source file is newer;
# this also makes the explicit CI/release preparation step cheap after install.
if [ -f "$DEST" ] \
  && [ -f "$SOURCE_DIR/Package.swift" ] \
  && [ -f "$SOURCE_DIR/Sources/alchemy-fm/main.swift" ] \
  && [ -z "$(find "$SOURCE_DIR" -path "$SOURCE_DIR/.build" -prune -o -type f -newer "$DEST" -print -quit)" ]; then
  echo "up to date: $DEST"
  exit 0
fi

(cd "$SOURCE_DIR" && swift build -c release)
mkdir -p "$(dirname "$DEST")"
cp "$SOURCE_DIR/.build/release/alchemy-fm" "$DEST"
echo "staged: src-tauri/binaries/alchemy-fm"
