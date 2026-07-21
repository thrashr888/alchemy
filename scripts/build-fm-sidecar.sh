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
(cd sidecar/alchemy-fm && swift build -c release)
mkdir -p src-tauri/binaries
cp sidecar/alchemy-fm/.build/release/alchemy-fm src-tauri/binaries/alchemy-fm
echo "staged: src-tauri/binaries/alchemy-fm"
