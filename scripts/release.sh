#!/usr/bin/env bash
#
# Cut a release from this machine:
#   bump+commit (needs the unlocked vault) -> gate -> build -> sign ->
#   notarize -> tag -> publish (all keyless from the commit on).
#
# On Apple Silicon this is faster and far more reliable than the CI path -- the
# signing identity and notary profile live in your Keychain, so the whole class
# of CI signing bugs (locked keychains, secret drift) simply can't happen. CI
# (.github/workflows/release.yml) remains as a manual fallback. See RELEASE.md.
#
# Usage:  scripts/release.sh <version>        e.g. scripts/release.sh 0.4.2
#
# Config (env overrides, sensible defaults):
#   APPLE_SIGNING_IDENTITY      auto-detected from your Keychain if unset
#   NOTARY_PROFILE              notarytool keychain profile name (default: alchemy-notary)
#   APPLE_PROVISIONING_PROFILE  path to a Developer ID .provisionprofile carrying
#                               the iCloud container. Optional; unset builds and
#                               signs exactly as before. See RELEASE.md.
#
set -euo pipefail

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "usage: scripts/release.sh <version>   (e.g. scripts/release.sh 0.4.2)" >&2
  exit 1
fi

# Releases always require a human decision. Agents must never set this
# variable themselves -- a person exports it after reviewing what ships:
#   RELEASE_APPROVED=yes scripts/release.sh <version>
if [ "${RELEASE_APPROVED:-}" != "yes" ]; then
  echo "release: refusing to run without human approval." >&2
  echo "Review the pending changes, then rerun with RELEASE_APPROVED=yes." >&2
  exit 1
fi
TAG="v$VERSION"
TARGET="aarch64-apple-darwin"

# Milestone lines carry elapsed time plus a typical duration, so a watcher
# can tell "slow but normal" from "stuck". Warm means target/ survived since
# the last release; a toolchain or dependency bump makes the next build cold.
START_TS="$(date +%s)"
phase() {
  _e="$(( $(date +%s) - START_TS ))"
  printf '==> [%dm%02ds] %s\n' "$(( _e / 60 ))" "$(( _e % 60 ))" "$1"
}
NOTARY_PROFILE="${NOTARY_PROFILE:-alchemy-notary}"
SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-$(
  security find-identity -v -p codesigning |
    awk -F'"' '/Developer ID Application/{print $2; exit}'
)}"
DMG="src-tauri/target/$TARGET/release/bundle/dmg/Alchemy_${VERSION}_aarch64.dmg"
DYLIB="src-tauri/libs/libpdfium.dylib"
UPDATER_TGZ="src-tauri/target/$TARGET/release/bundle/macos/Alchemy.app.tar.gz"

# Updater artifacts are signed with the Tauri updater key (separate from the
# Apple identity). Defaults to the local keyfile; CI passes the env directly.
UPDATER_KEY_FILE="$HOME/.tauri/alchemy.key"
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  [ -f "$UPDATER_KEY_FILE" ] || { echo "Updater key not found: $UPDATER_KEY_FILE (see RELEASE.md)." >&2; exit 1; }
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "$UPDATER_KEY_FILE")"
fi
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

cd "$(dirname "$0")/.."

# --- Preconditions -----------------------------------------------------------
[ -n "$SIGNING_IDENTITY" ] || { echo "No 'Developer ID Application' identity in your Keychain." >&2; exit 1; }
[ "$(git rev-parse --abbrev-ref HEAD)" = "main" ] || { echo "Not on main." >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "Working tree not clean -- commit or stash first." >&2; exit 1; }
git rev-parse "$TAG" >/dev/null 2>&1 && { echo "Tag $TAG already exists." >&2; exit 1; }
xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1 \
  || { echo "Notary profile '$NOTARY_PROFILE' not found. See RELEASE.md (one-time setup)." >&2; exit 1; }

echo "==> Releasing $TAG  (identity: ${SIGNING_IDENTITY%% (*}..., profile: $NOTARY_PROFILE)"

# --- Optional: the iCloud container (RELEASE.md, "iCloud container") ---------
# A Developer ID app may claim an iCloud container only with a matching
# provisioning profile embedded at Contents/embedded.provisionprofile. The
# entitlement without the profile makes the signed app refuse to launch, so
# the two arrive together or neither does: point APPLE_PROVISIONING_PROFILE at
# a .provisionprofile and this build picks up Entitlements.icloud.plist and
# embeds that profile. Unset, everything below runs exactly as it did.
APP_BUNDLE="src-tauri/target/$TARGET/release/bundle/macos/Alchemy.app"
PROFILE_DEST="src-tauri/embedded.provisionprofile"
ICLOUD_CONTAINER="iCloud.com.thrashr888.alchemy"
# Word-split on purpose: empty means "no extra flags", and the path holds no
# spaces. Not an array, because bash 3.2 (what /usr/bin/env bash finds on a
# stock Mac) errors on an empty array expansion under set -u.
ICLOUD_BUILD_FLAGS=""
# A stale copy from an earlier run must never ride along into a plain build.
rm -f "$PROFILE_DEST"
if [ -n "${APPLE_PROVISIONING_PROFILE:-}" ]; then
  [ -f "$APPLE_PROVISIONING_PROFILE" ] || {
    echo "No provisioning profile at $APPLE_PROVISIONING_PROFILE (see RELEASE.md)." >&2; exit 1; }
  # Verify before a 40-minute build rather than after: an expired profile, or
  # one for the wrong container, produces an app that will not launch.
  python3 - "$APPLE_PROVISIONING_PROFILE" "$ICLOUD_CONTAINER" <<'PYEOF'
import datetime, plistlib, subprocess, sys
path, container = sys.argv[1], sys.argv[2]
raw = subprocess.run(["security", "cms", "-D", "-i", path],
                     capture_output=True).stdout
try:
    profile = plistlib.loads(raw)
except Exception:
    sys.exit("provisioning profile: could not decode %s" % path)
expires = profile.get("ExpirationDate")
if expires is None:
    sys.exit("provisioning profile: no ExpirationDate; is this a profile?")
if expires.replace(tzinfo=datetime.timezone.utc) <= datetime.datetime.now(
        datetime.timezone.utc):
    sys.exit("provisioning profile expired %s -- download a fresh one" % expires)
ents = profile.get("Entitlements", {})
claimed = ents.get("com.apple.developer.icloud-container-identifiers", [])
if container not in claimed:
    sys.exit("provisioning profile does not carry %s (it has: %s)"
             % (container, ", ".join(claimed) or "no iCloud containers"))
print("    profile: %s  team %s  expires %s"
      % (profile.get("Name", "?"),
         ", ".join(profile.get("TeamIdentifier", [])) or "?", expires))
PYEOF
  cp "$APPLE_PROVISIONING_PROFILE" "$PROFILE_DEST"
  ICLOUD_BUILD_FLAGS="--config src-tauri/tauri.icloud.conf.json"
  echo "==> iCloud container $ICLOUD_CONTAINER (Entitlements.icloud.plist + embedded profile)"
fi

# --- Version bump + commit (signing happens NOW, not after the build) --------
# The bump commit is the only step that needs the 1Password-backed signing
# key, and the vault is open right now -- the human just approved this run.
# Committing before the ~15-minute gate/build/notarize window is what stops
# the vault's re-lock from killing the release at the finish line (it did,
# twice, on 2026-08-20). The tag is lightweight and the push is keyless, so
# nothing after this line touches the vault.
node -e "for (const f of ['package.json','src-tauri/tauri.conf.json']) {
  const j = require('./'+f); j.version = '$VERSION';
  require('fs').writeFileSync(f, JSON.stringify(j, null, 2) + '\n');
}"
perl -i -pe 'if (!$d && /^version = /) { s/^version = ".*"/version = "'"$VERSION"'"/; $d=1 }' src-tauri/Cargo.toml
# Sync Cargo.lock (cargo update --workspace rewrites workspace members.
# versions without touching deps or build scripts; metadata --no-deps did NOT write the lock and tripped the post-build dirty-tree guard on v0.43.0).
(cd src-tauri && cargo update --workspace --quiet)
git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "$TAG"
# From here to the push, a failure strands this commit locally (never
# pushed). The rerun flow in RELEASE.md resets to origin/main first, so a
# stranded bump is self-healing; say so instead of leaving a mystery.
trap 'echo "release: failed after the bump commit -- local main carries an unpushed $TAG bump; rerun per RELEASE.md (reset to origin/main first)." >&2' ERR

# --- Quality gate (fast dev feature set) ------------------------------------
phase "Quality gate  (typical: ~2 min warm, ~10 min cold)"
# Releases run from an isolated clone (no node_modules yet); without this,
# `pnpm exec tsc` silently falls through to whatever global tsc is on PATH.
pnpm install --frozen-lockfile --ignore-scripts
# The natives must exist before ANY cargo step: tauri's build script
# resolves libs/libpdfium.dylib and binaries/alchemy-fm as resources, so
# even the clippy gate needs them in a fresh clone. Both are idempotent.
scripts/fetch-pdfium.sh
scripts/build-fm-sidecar.sh
pnpm exec tsc --noEmit
(
  cd src-tauri
  cargo fmt -- --check
  cargo clippy --no-default-features --features debug -- -D warnings
  cargo test --no-default-features --features debug --lib
)

# --- Build + sign ------------------------------------------------------------
# The bundled PDFium dylib and the FM sidecar ship ad-hoc-signed; fetch/build
# them (idempotent) then give each our Developer ID + secure timestamp so
# notarization accepts them. Both are gitignored, so signing never touches
# the working tree.
phase "Signing PDFium dylib + FM sidecar + building  (typical: ~5 min warm, ~40 min cold)"
scripts/fetch-pdfium.sh
scripts/build-fm-sidecar.sh
codesign --force --timestamp --options runtime --sign "$SIGNING_IDENTITY" "$DYLIB"
codesign --force --timestamp --options runtime --sign "$SIGNING_IDENTITY" src-tauri/binaries/alchemy-fm
# shellcheck disable=SC2086  # ICLOUD_BUILD_FLAGS is meant to word-split.
APPLE_SIGNING_IDENTITY="$SIGNING_IDENTITY" pnpm tauri build --target "$TARGET" $ICLOUD_BUILD_FLAGS
[ -f "$DMG" ] || { echo "DMG not produced: $DMG" >&2; exit 1; }
# The profile has to be inside the bundle before codesign seals it, which is
# the bundler's ordering to get right, not ours. Check rather than assume: an
# app signed with the entitlement and no embedded profile does not launch, and
# finding that out from a notarized DMG is the expensive way.
if [ -n "${APPLE_PROVISIONING_PROFILE:-}" ]; then
  [ -f "$APP_BUNDLE/Contents/embedded.provisionprofile" ] || {
    echo "iCloud build produced no Contents/embedded.provisionprofile -- refusing to ship it." >&2
    exit 1; }
  codesign -d --entitlements - "$APP_BUNDLE" 2>/dev/null |
    grep -aq "com.apple.developer.icloud-container-identifiers" || {
      echo "iCloud build is not signed with the iCloud entitlement -- refusing to ship it." >&2
      exit 1; }
  codesign --verify --strict "$APP_BUNDLE"
fi
[ -f "$UPDATER_TGZ" ] || { echo "Updater artifact not produced: $UPDATER_TGZ" >&2; exit 1; }
[ -f "$UPDATER_TGZ.sig" ] || { echo "Updater signature not produced: $UPDATER_TGZ.sig" >&2; exit 1; }

# --- Notarize + staple + verify ---------------------------------------------
phase "Notarize start: $(date -u +%FT%TZ)  (typical: 2-10 min at Apple)"
xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$DMG"
phase "Notarize done:  $(date -u +%FT%TZ)"
spctl -a -t open --context context:primary-signature -vv "$DMG"

# --- Tag, publish ------------------------------------------------------------
# The bump commit already exists (made up top, while the vault was open);
# guard that the build didn't dirty the tracked tree, then tag it and push.
phase "Tagging, publishing  (typical: ~1 min)"
[ -z "$(git status --porcelain)" ] || { echo "Build dirtied the tracked tree -- investigate before tagging:" >&2; git status --porcelain >&2; exit 1; }
git tag "$TAG"
git push origin main "$TAG"

# latest.json points the in-app updater at this release's signed tarball.
LATEST_JSON="src-tauri/target/$TARGET/release/bundle/macos/latest.json"
python3 - "$VERSION" "$UPDATER_TGZ.sig" > "$LATEST_JSON" <<'PYEOF'
import json, sys, datetime
version, sig_path = sys.argv[1], sys.argv[2]
print(json.dumps({
    "version": version,
    "pub_date": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "platforms": {
        "darwin-aarch64": {
            "signature": open(sig_path).read().strip(),
            "url": f"https://github.com/thrashr888/alchemy/releases/download/v{version}/Alchemy.app.tar.gz",
        }
    },
}, indent=2))
PYEOF
gh release create "$TAG" "$DMG" "$UPDATER_TGZ" "$UPDATER_TGZ.sig" "$LATEST_JSON" \
  --title "Alchemy $TAG" --generate-notes

phase "Done. Released $TAG -- https://github.com/thrashr888/alchemy/releases/tag/$TAG"
echo "    (edit the notes on GitHub if you want more than the auto-generated changelog.)"
