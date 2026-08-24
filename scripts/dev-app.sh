#!/usr/bin/env bash
#
# Build and launch the dev app as a real, stably-signed .app bundle.
#
# Two problems this exists to avoid, both of which look like app bugs:
#
#   1. `tauri build --no-bundle` produces a bare executable: no icon, and no
#      Info.plist, so the alchemy:// scheme and the file associations are not
#      registered and anything depending on them silently does nothing.
#
#   2. An ad-hoc signed bundle gets a random signing identifier and a fresh
#      cdhash on every build. macOS privacy permissions are keyed on the
#      signing identity, so every rebuild looks like a brand-new app and
#      re-prompts for file access, which blocks startup until someone clicks,
#      and makes any startup timing meaningless. Signing with a Developer ID
#      gives a designated requirement built from the bundle id and the team,
#      which does not change when the binary does, so the grant sticks.
#
# Usage:  scripts/dev-app.sh [--no-launch]
#
set -euo pipefail

cd "$(dirname "$0")/.."

# First Developer ID Application identity in the keychain. Falls back to an
# unsigned build rather than failing: an unsigned dev app still runs, it just
# re-asks for permissions.
IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null \
  | grep 'Developer ID Application' \
  | head -1 \
  | sed -E 's/.*"(.*)"/\1/')"

if [ -n "$IDENTITY" ]; then
  echo "Signing as: $IDENTITY"
  export APPLE_SIGNING_IDENTITY="$IDENTITY"
else
  echo "No Developer ID Application identity found - building unsigned."
  echo "Expect macOS to re-ask for file access after every rebuild."
fi

# Not gated on the exit code: the bundler builds the .app and then fails on
# the updater artifact, which wants a private signing key no dev machine
# needs. The .app existing is the success signal that matters here.
npx tauri build --debug --bundles app || true

APP="src-tauri/target/debug/bundle/macos/Alchemy.app"
if [ ! -d "$APP" ]; then
  echo "Build did not produce $APP" >&2
  exit 1
fi

if [ "${1:-}" = "--no-launch" ]; then
  echo "Built $APP"
  exit 0
fi

# Replace any running copy so the launch is the build that just happened.
pkill -f "Alchemy.app/Contents/MacOS" 2>/dev/null || true
open "$APP"
echo "Launched $APP"
