# Releasing Alchemy

Releases are cut **locally** on an Apple Silicon Mac with
[`scripts/release.sh`](scripts/release.sh). On modern hardware a local build is
faster than CI and avoids the whole class of CI code-signing fragility (locked
keychains, secret drift), since your Developer ID and notary credentials already
live in your Keychain.

CI ([`.github/workflows/release.yml`](.github/workflows/release.yml)) stays as a
**manual fallback** for when you can't release from your machine.

---

## Cutting a release (the normal path)

```bash
RELEASE_APPROVED=yes scripts/release.sh 0.4.2
```

`RELEASE_APPROVED=yes` is the human gate: the script refuses to run
without it, and agents must never set it themselves — a person reviews
what ships and exports it deliberately.

That one command, from a clean `main`:

1. Bumps the version in `package.json`, `src-tauri/tauri.conf.json`, and
   `Cargo.toml`, and **commits the bump immediately** — that commit is the
   only step that needs your (1Password-backed) signing key, and it runs
   while the vault is still open from you approving the release. Nothing
   after it touches the vault, so a re-lock during the long build can't
   kill the release anymore.
2. Runs the quality gate — `tsc`, `cargo fmt`, `clippy`, `cargo test`.
3. Signs the bundled PDFium dylib, then builds a signed `.app` + `.dmg`.
4. Notarizes with Apple, staples the ticket, and verifies with `spctl`.
5. Tags `v0.4.2`, pushes, and creates the GitHub release with the notarized
   DMG and auto-generated notes. A failure between steps 1 and 5 strands the
   bump commit locally (never pushed) — the rerun flow below resets it away.

Typical time: a few minutes with a warm `target/` cache (vs. ~25 min on CI).

If the final push is rejected because main moved during the build, the tag
may already be on the remote pointing at a commit main will never contain.
Retract it, resync, and rerun — the second build is fast on the warm cache:
```bash
git push origin :refs/tags/vX.Y.Z && git tag -d vX.Y.Z
git checkout -- . && git fetch && git reset --hard origin/main
RELEASE_APPROVED=yes scripts/release.sh X.Y.Z
```
Also quiet the machine first: a dev instance's background sweeps can wedge
Ollama hard enough that the gate's live tests time out mid-release.

## Release notes format

Replace the auto-generated changelog with notes in the house style
(`gh release edit vX.Y.Z --title ... --notes ...`):

- **Title**: `vX.Y.Z — short, comma-separated feature summary`
- **Body**: `## Highlights` (bold-led bullets, most user-visible first),
  `## Fixes` (plain bullets), optional `## Notes` (upgrade caveats), ending
  with `**Full Changelog**: .../compare/vPREV...vX.Y.Z`

## One-time setup

### Environment

Set these in your shell environment (a profile, or direnv) — never in the
repo. The Tauri CLI and `notarytool` read the process environment; neither
loads a `.env` file.

| Variable | What it is | Needed for |
| -------- | ---------- | ---------- |
| `APPLE_SIGNING_IDENTITY` | full identity name, `Developer ID Application: Name (TEAMID)` | signing any bundle; release only needs it when the Keychain holds more than one |
| `APPLE_ID` | Apple ID email | creating the notary profile |
| `APPLE_TEAM_ID` | ten-character team identifier | creating the notary profile |

`APPLE_SIGNING_IDENTITY` matters for day-to-day work too, not just releases.
macOS keys privacy permissions on the signing identity, and an unsigned
bundle draws a fresh random one every build — so a rebuilt dev app reads as
a brand-new app and re-prompts for file access, blocking startup on a click
and making any startup timing meaningless. With the variable set,
`pnpm tauri build --debug --bundles app` produces a bundle whose designated
requirement is stable across rebuilds, and the grant sticks.

You need three things on your machine:

1. **A Developer ID Application certificate** in your login Keychain
   (Apple Developer Program → Certificates). Verify:
   ```bash
   security find-identity -v -p codesigning | grep "Developer ID Application"
   ```
   The script auto-detects it; override with `APPLE_SIGNING_IDENTITY` if you have
   more than one.

2. **A notary profile** named `alchemy-notary` (override with `NOTARY_PROFILE`).
   Create it once with an [app-specific password](https://account.apple.com)
   (Sign-In and Security → App-Specific Passwords):
   ```bash
   xcrun notarytool store-credentials alchemy-notary \
     --apple-id you@example.com --team-id YOURTEAMID
   ```

   Two things about this profile that have already cost a release attempt:

   - notarytool stores it in the **data-protection keychain**, which the
     legacy `security` tools (`dump-keychain`, `find-generic-password`)
     cannot see. "Not in `security` output" does not mean it's gone —
     `xcrun notarytool history --keychain-profile alchemy-notary` is the
     only meaningful check.
   - Access is per-context: a profile that works in your terminal can
     read as "No Keychain password item found" from an agent's
     non-interactive shell. If that happens, re-running the
     `store-credentials` command above from your own terminal refreshes
     the item and its authorization; the v0.40.0 release was unblocked
     exactly that way.

   The app-specific password itself is backed up in 1Password (Private
   vault, "Apple notary app-specific password"), so recreating the
   profile never requires minting a new password. An agent can recreate
   it without ever seeing the secret by piping it straight from 1Password
   into notarytool's stdin prompt:
   ```bash
   op read "op://Private/Apple notary app-specific password/password" |
     xcrun notarytool store-credentials alchemy-notary \
       --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID"
   ```
   `APPLE_ID` is the Apple ID email and `APPLE_TEAM_ID` the ten-character
   team identifier from the Developer portal (also the parenthesised part of
   the signing identity name). Keep both in your shell environment rather
   than in the repo.

3. **`gh`** authenticated with push + release access (`gh auth login`).

4. **A Tauri updater keypair** at `~/.tauri/alchemy.key` (the public key lives in
   `src-tauri/tauri.conf.json`). It signs the auto-update artifact each release.
   Generate once with:
   ```bash
   pnpm tauri signer generate --write-keys ~/.tauri/alchemy.key --password ""
   ```
   Losing this key means shipped apps can no longer verify updates. A verified
   copy lives in 1Password (Private vault, "Alchemy Tauri Updater Key").
   The release publishes `Alchemy.app.tar.gz`, its `.sig`, and `latest.json`
   alongside the DMG; the app checks
   `releases/latest/download/latest.json` on launch.

## iCloud container (stage two)

Optional, and off unless you turn it on. With it, the Notebooks folder is
Alchemy's own iCloud container — the branded "Alchemy" folder with the app
icon at the iCloud Drive root, at
`~/Library/Mobile Documents/iCloud~com~thrashr888~alchemy/Documents/`, which
is the same container an iPhone app would read. Without it, the app keeps
shipping stage one: a plain `iCloud Drive/Alchemy/` folder
(docs/RFC-okf-live.md §5.7).

A Developer ID app may claim an iCloud container only when a matching
provisioning profile is embedded in the bundle at
`Contents/embedded.provisionprofile`. The entitlement without the profile
produces an app that will not launch, so the release script treats them as
one thing: set `APPLE_PROVISIONING_PROFILE` and you get both, leave it unset
and the build signs exactly as it does today.

### What to do in the Apple Developer portal

All of this is at [developer.apple.com/account](https://developer.apple.com/account)
→ Certificates, Identifiers & Profiles. It is a one-time setup per profile
expiry.

1. **Identifiers → iCloud Containers → +.** Description `Alchemy`,
   Identifier `iCloud.com.thrashr888.alchemy`. That string is what
   `src-tauri/Entitlements.icloud.plist` and `src-tauri/Info.plist` already
   name; it is not a free choice.
2. **Identifiers → App IDs.** Open (or create, as an explicit App ID, not a
   wildcard) `com.thrashr888.alchemy`. Enable the **iCloud** capability,
   choose iCloud Documents, then Edit and assign the container from step 1.
   Save.
3. **Profiles → + → Distribution → Developer ID.** App ID
   `com.thrashr888.alchemy`, then your **Developer ID Application**
   certificate — the same one `security find-identity` shows. Name it
   something you will recognise (`Alchemy Developer ID iCloud`) and Generate.
4. **Download it.** You get a `.provisionprofile` file. Keep it out of the
   repo. The durable home is 1Password: a Document item in the Private
   vault named "Alchemy Developer ID provisioning profile", which the
   release script can read directly (below), so the file never has to
   persist on a machine. Store or replace it with
   `op document create <file> --title "Alchemy Developer ID provisioning profile" --vault Private`
   (or `op document edit` for a regenerated one).

Check the download before trusting it. A profile generated before the
container was assigned to the App ID decodes fine but carries an empty
`com.apple.developer.icloud-container-identifiers`, and the release script
refuses it. Decode and look:

```bash
security cms -D -i Alchemy.provisionprofile | plutil -p - | grep -A3 icloud-container
```

If the array is empty, redo step 2's Edit (tick the container, Save), then
generate and download the profile again; the earlier one is invalid.

### What to set on your machine

| Variable | What it is |
| -------- | ---------- |
| `APPLE_PROVISIONING_PROFILE` | absolute path to the `.provisionprofile` from step 4, or an `op://` reference to the 1Password document |

Same rule as the rest: shell environment, never the repo. The `op://` form
is `op://<vault>/<item title>/<file name as stored>`; the script fetches it
with `op read` into a temp file, after the 1Password app approves the
request, and fails before the build if it cannot.

```bash
export APPLE_PROVISIONING_PROFILE="op://Private/Alchemy Developer ID provisioning profile/Alchemy_Developer_ID_iCloud.provisionprofile"
RELEASE_APPROVED=yes scripts/release.sh 0.56.0
```

The script then verifies the profile before it builds anything — it must not
be expired and must carry `iCloud.com.thrashr888.alchemy` — copies it to
`src-tauri/embedded.provisionprofile` (gitignored), and builds with
`src-tauri/tauri.icloud.conf.json`, which swaps in
`Entitlements.icloud.plist` and embeds the profile. After the build it checks
that the profile really is inside the signed bundle and that the signature
carries the entitlement, and refuses to notarize if either is missing.

For a local bundle to try it on, the same two flags:

```bash
APPLE_PROVISIONING_PROFILE=... cp "$APPLE_PROVISIONING_PROFILE" src-tauri/embedded.provisionprofile
pnpm tauri build --debug --bundles app --config src-tauri/tauri.icloud.conf.json
```

Verify what you got:

```bash
codesign -d --entitlements - target/debug/bundle/macos/Alchemy.app
ls target/debug/bundle/macos/Alchemy.app/Contents/embedded.provisionprofile
```

Profiles expire. When the script says so, repeat step 3 and re-download —
nothing else changes, and a release with the variable unset is always
available as the way past it.

## Manual CI fallback

If you can't release locally, trigger the workflow from the **Actions → Release**
tab and run it against the tag ref. It needs these repo secrets (Settings →
Secrets and variables → Actions):

| Secret | What it is |
| ------ | ---------- |
| `APPLE_CERTIFICATE` | base64 of your Developer ID `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID` | your Apple ID email |
| `APPLE_PASSWORD` | an app-specific password |
| `APPLE_TEAM_ID` | your 10-character team ID |

Export the `.p12` for `APPLE_CERTIFICATE` straight from the Keychain (no GUI):

```bash
security export -k login.keychain-db -t identities -f pkcs12 \
  -P 'ExportPassword' -o signing.p12
base64 -i signing.p12 | gh secret set APPLE_CERTIFICATE
rm signing.p12
```

If the CI build ever hangs on signing, it's the keychain auto-lock — the
workflow already disables it; see the comments in `release.yml`.
