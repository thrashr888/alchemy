#!/usr/bin/env bash
#
# Cut a release from this machine: gate → build → sign → notarize → publish.
#
# On Apple Silicon this is faster and far more reliable than the CI path — the
# signing identity and notary profile live in your Keychain, so the whole class
# of CI signing bugs (locked keychains, secret drift) simply can't happen. CI
# (.github/workflows/release.yml) remains as a manual fallback. See RELEASE.md.
#
# Usage:  scripts/release.sh <version>        e.g. scripts/release.sh 0.4.2
#
# Config (env overrides, sensible defaults):
#   APPLE_SIGNING_IDENTITY   auto-detected from your Keychain if unset
#   NOTARY_PROFILE           notarytool keychain profile name (default: alchemy-notary)
#
set -euo pipefail

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "usage: scripts/release.sh <version>   (e.g. scripts/release.sh 0.4.2)" >&2
  exit 1
fi
TAG="v$VERSION"
TARGET="aarch64-apple-darwin"
NOTARY_PROFILE="${NOTARY_PROFILE:-alchemy-notary}"
SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-$(
  security find-identity -v -p codesigning |
    awk -F'"' '/Developer ID Application/{print $2; exit}'
)}"
DMG="src-tauri/target/$TARGET/release/bundle/dmg/Alchemy_${VERSION}_aarch64.dmg"
DYLIB="src-tauri/libs/libpdfium.dylib"

cd "$(dirname "$0")/.."

# --- Preconditions -----------------------------------------------------------
[ -n "$SIGNING_IDENTITY" ] || { echo "No 'Developer ID Application' identity in your Keychain." >&2; exit 1; }
[ "$(git rev-parse --abbrev-ref HEAD)" = "main" ] || { echo "Not on main." >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "Working tree not clean — commit or stash first." >&2; exit 1; }
git rev-parse "$TAG" >/dev/null 2>&1 && { echo "Tag $TAG already exists." >&2; exit 1; }
xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1 \
  || { echo "Notary profile '$NOTARY_PROFILE' not found. See RELEASE.md (one-time setup)." >&2; exit 1; }

echo "==> Releasing $TAG  (identity: ${SIGNING_IDENTITY%% (*}…, profile: $NOTARY_PROFILE)"

# --- Version bump ------------------------------------------------------------
node -e "for (const f of ['package.json','src-tauri/tauri.conf.json']) {
  const j = require('./'+f); j.version = '$VERSION';
  require('fs').writeFileSync(f, JSON.stringify(j, null, 2) + '\n');
}"
perl -i -pe 'if (!$d && /^version = /) { s/^version = ".*"/version = "'"$VERSION"'"/; $d=1 }' src-tauri/Cargo.toml

# --- Quality gate (fast dev feature set) ------------------------------------
echo "==> Quality gate"
pnpm exec tsc --noEmit
(
  cd src-tauri
  cargo fmt -- --check
  cargo clippy --no-default-features --features debug -- -D warnings
  cargo test --no-default-features --features debug
)

# --- Build + sign ------------------------------------------------------------
# The bundled PDFium dylib ships ad-hoc-signed; fetch it (idempotent) then give
# it our Developer ID + secure timestamp so notarization accepts it. It's
# gitignored, so signing it never touches the working tree.
echo "==> Signing PDFium dylib + building"
scripts/fetch-pdfium.sh
codesign --force --timestamp --options runtime --sign "$SIGNING_IDENTITY" "$DYLIB"
APPLE_SIGNING_IDENTITY="$SIGNING_IDENTITY" pnpm tauri build --target "$TARGET"
[ -f "$DMG" ] || { echo "DMG not produced: $DMG" >&2; exit 1; }

# --- Notarize + staple + verify ---------------------------------------------
echo "==> Notarize start: $(date -u +%FT%TZ)"
xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$DMG"
echo "==> Notarize done:  $(date -u +%FT%TZ)"
spctl -a -t open --context context:primary-signature -vv "$DMG"

# --- Commit, tag, publish ----------------------------------------------------
echo "==> Committing, tagging, publishing"
git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "$TAG"
git tag "$TAG"
git push origin main "$TAG"
gh release create "$TAG" "$DMG" --title "Alchemy $TAG" --generate-notes

echo "==> Done. Released $TAG — https://github.com/thrashr888/alchemy/releases/tag/$TAG"
echo "    (edit the notes on GitHub if you want more than the auto-generated changelog.)"
