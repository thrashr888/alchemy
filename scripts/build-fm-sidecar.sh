#!/bin/sh
# Build the Foundation Models sidecar (RFC-inference-providers) and stage it
# where dev builds resolve it. Release bundling: copy the binary into the
# app's resources as binaries/alchemy-fm (and codesign it — see the PDFium
# precedent in release.sh) when wiring externalBin.
set -eu
cd "$(dirname "$0")/../sidecar/alchemy-fm"
swift build -c release
echo "built: $(pwd)/.build/release/alchemy-fm"
