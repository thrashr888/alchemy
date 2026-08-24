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
STAMP="$DEST.sha256"

# Git checkout gives every source file a fresh timestamp, while GitHub Actions
# restores caches afterward with their original timestamps. Compare content,
# not mtimes, so a restored sidecar genuinely skips the Swift build but any
# package/source edit invalidates it.
inputs_hash() {
  find "$SOURCE_DIR" -path "$SOURCE_DIR/.build" -prune -o -type f -print \
    | LC_ALL=C sort \
    | while IFS= read -r file; do shasum -a 256 "$file"; done \
    | shasum -a 256 \
    | awk '{print $1}'
}

INPUTS_HASH="$(inputs_hash)"
if [ -f "$DEST" ] \
  && [ -f "$STAMP" ] \
  && [ "$(cat "$STAMP")" = "$INPUTS_HASH" ]; then
  echo "up to date: $DEST"
  exit 0
fi

(cd "$SOURCE_DIR" && swift build -c release)
mkdir -p "$(dirname "$DEST")"
cp "$SOURCE_DIR/.build/release/alchemy-fm" "$DEST"
printf '%s\n' "$INPUTS_HASH" > "$STAMP"
echo "staged: src-tauri/binaries/alchemy-fm"
