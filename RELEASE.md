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
scripts/release.sh 0.4.2
```

That one command, from a clean `main`:

1. Bumps the version in `package.json`, `src-tauri/tauri.conf.json`, and `Cargo.toml`.
2. Runs the quality gate — `tsc`, `cargo fmt`, `clippy`, `cargo test`.
3. Signs the bundled PDFium dylib, then builds a signed `.app` + `.dmg`.
4. Notarizes with Apple, staples the ticket, and verifies with `spctl`.
5. Commits the bump, tags `v0.4.2`, pushes, and creates the GitHub release with
   the notarized DMG and auto-generated notes.

Typical time: a few minutes with a warm `target/` cache (vs. ~25 min on CI).
Edit the release notes on GitHub afterward if you want more than the
auto-generated changelog.

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

3. **`gh`** authenticated with push + release access (`gh auth login`).

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
