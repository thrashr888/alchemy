#!/usr/bin/env bash
#
# Assemble the web clipper for each browser store.
#
# One source tree (extension/src) plus one manifest per browser
# (extension/chrome, extension/firefox) -- the browsers disagree about the
# background key and about which permissions are safe to ask for, and nothing
# else. This script is the only place those halves are glued together, so
# background.js never gets copy-pasted.
#
# Usage:  scripts/build-extensions.sh [chrome|firefox]     (default: both)
#
# Output (gitignored):
#   extension/dist/<browser>/       unpacked -- load this for local testing
#   extension/dist/<browser>.zip    upload this to the store
#
# Keep this file ASCII: non-ASCII bytes after a $VAR have crashed scripts here
# under `set -u`.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ext="$root/extension"
dist="$ext/dist"

build() {
  browser="$1"
  manifest="$ext/$browser/manifest.json"
  if [ ! -f "$manifest" ]; then
    echo "no manifest for $browser at $manifest" >&2
    exit 1
  fi

  # Fail before packaging rather than at the store's upload form.
  if ! python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$manifest"; then
    echo "$browser manifest is not valid JSON" >&2
    exit 1
  fi

  out="$dist/$browser"
  rm -rf "$out" "$dist/$browser.zip"
  mkdir -p "$out"

  # Only what the browser runs: the shared script, the icons, the manifest.
  # Listing art (extension/store) and docs stay out of the package.
  cp "$ext/src/background.js" "$out/background.js"
  cp -R "$ext/src/icons" "$out/icons"
  cp "$manifest" "$out/manifest.json"
  find "$out" -name '.DS_Store' -delete

  # Stores want the files at the zip root, not nested under a folder.
  (cd "$out" && zip -q -r -X "$dist/$browser.zip" .)

  version="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['version'])" "$manifest")"
  size="$(wc -c <"$dist/$browser.zip" | tr -d ' ')"
  echo "$browser $version -> extension/dist/$browser.zip ($size bytes)"
}

case "${1:-both}" in
  chrome) build chrome ;;
  firefox) build firefox ;;
  both) build chrome; build firefox ;;
  *) echo "usage: $0 [chrome|firefox]" >&2; exit 1 ;;
esac
