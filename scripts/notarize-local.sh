#!/usr/bin/env bash
# Notarize + staple the locally-built, Developer ID-signed DMG, then verify
# Gatekeeper accepts it. Requires a one-time notary profile:
#   xcrun notarytool store-credentials alchemy-notary \
#     --apple-id thrashr888@gmail.com --team-id 5T4QSYSNP2
set -euo pipefail

PROFILE="${1:-alchemy-notary}"
DMG="src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/Alchemy_0.4.0_aarch64.dmg"

[ -f "$DMG" ] || { echo "DMG not found: $DMG -- run 'pnpm tauri build --target aarch64-apple-darwin' first."; exit 1; }

echo "==> Submitting to Apple notary service (this waits until done)..."
xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait

echo "==> Stapling the notarization ticket to the DMG..."
xcrun stapler staple "$DMG"

echo "==> Verifying Gatekeeper acceptance..."
spctl -a -t open --context context:primary-signature -vv "$DMG"
xcrun stapler validate "$DMG"

echo "==> Done. This DMG now opens with a normal double-click on any Mac."
