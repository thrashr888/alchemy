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
       --apple-id thrashr888@gmail.com --team-id 5T4QSYSNP2
   ```

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
