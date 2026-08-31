# Alchemy Basic Security Review

Date: 2026-08-31
Reviewed revision: `7b79d22` (`codex/reminders-cli-navigation-chat`)
Remediation branch: `codex/security-hardening`
Scope: React/Vite renderer, Tauri/Rust trust boundary, local HTTP services, archive ingestion, release supply chain, and production dependency advisories.

## Remediation status

All five findings have implementation fixes on the remediation branch:

| ID | Status | Implemented mitigation |
| --- | --- | --- |
| SEC-001 | Mitigated | App data is repaired to `0700`; credential and local-auth files are atomically written `0600`, including existing-install migration. Platform Keychain storage and a redacted renderer DTO remain worthwhile defense in depth. |
| SEC-002 | Fixed | PDFium is pinned to `154.0.8035.0` / Chromium revision `8035`, downloaded from immutable versioned URLs, and verified with committed architecture-specific SHA-256 values before extraction. |
| SEC-003 | Fixed | A restrictive Tauri CSP is enabled; the asset protocol is limited to app-owned audio; full images are loaded by source ID through a bounded backend command. |
| SEC-004 | Fixed | MCP requires a stable 256-bit bearer credential stored in owner-only files; the CLI and existing/one-click agent connectors authenticate. The clip service allowlists only the published Chrome extension origin. Firefox safely falls back to URL-only clipping for full-page capture. |
| SEC-005 | Fixed | OKF imports enforce entry, per-file, aggregate, and compression-ratio budgets while streaming, reject unsafe paths, and use RAII scratch directories for cleanup on every path. |

Validation on 2026-08-31:

- `pnpm build` passed.
- `pnpm test` passed: 157 tests.
- `pnpm test:cli` passed: 6 tests.
- `cargo fmt -- --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --lib` passed: 395 passed, 42 intentionally ignored.
- `pnpm tauri build --debug --no-bundle` passed and produced the desktop binary.
- The PDFium negative fixture was rejected by the real fetch script before extraction; the same check is now in CI.

## Executive summary

The original review found **two high-severity issues and three medium-severity hardening gaps**. This branch addresses all five, with Keychain-backed secret storage and renderer-side secret redaction retained as follow-up defense in depth:

1. Credential-bearing configuration is persisted as ordinary JSON and the current macOS file is readable by other local accounts (`0644` inside a `0755` directory).
2. Release builds download the publisher's latest PDFium native library without pinning or integrity verification, then sign and bundle it as trusted application code.

The renderer's untrusted-content handling is otherwise comparatively careful: Markdown is sanitized, Mermaid runs in strict mode and its SVG output is sanitized, shell-like repair commands are allowlisted, and application updates are signature-verified. Current production dependency audits found no vulnerability-class advisories.

## Risk summary

| ID | Severity | Finding | Remediation |
| --- | --- | --- | --- |
| SEC-001 | High | Credential-bearing config is stored with unsafe filesystem permissions | Mitigated |
| SEC-002 | High | Unpinned, unverified native PDFium is downloaded and signed during releases | Fixed |
| SEC-003 | Medium | CSP is disabled while the asset protocol exposes most of the home and temp trees | Fixed |
| SEC-004 | Medium | Default-on localhost services rely on request origin rather than authentication | Fixed |
| SEC-005 | Medium | OKF ZIP extraction has no decompression resource limits | Fixed |

## Findings

### SEC-001 — Credential-bearing config is stored with unsafe filesystem permissions

**Severity:** High
**Category:** CWE-732 (Incorrect Permission Assignment for Critical Resource)

**Evidence**

- `ProviderEntry.api_key`, the legacy `openai_api_key`, and `notion_token` are serializable fields in `src-tauri/src/ai/mod.rs:20-28`, `src-tauri/src/ai/mod.rs:70-79`, and `src-tauri/src/ai/mod.rs:157-160`.
- The complete structure is serialized and written with `std::fs::write` in `src-tauri/src/commands.rs:11855-11866`; no explicit Unix mode is applied.
- The config is loaded from the app-data directory at `ai_config.json` in `src-tauri/src/lib.rs:161-179`.
- Live verification on this Mac found the app-data directory at mode `0755` and `ai_config.json` at mode `0644`. I checked metadata only and deliberately did not inspect credential values.
- `get_ai_config` returns the complete config, including secret fields, to the renderer at `src-tauri/src/commands.rs:11840-11844`.

**Impact**

When provider or Notion credentials are configured, another local OS account can read them from disk. Returning the same secrets to the renderer also increases the consequence of a future renderer compromise.

**Recommended fix**

1. Immediately create the app-data directory with mode `0700` and credential-bearing files with mode `0600`; migrate permissions for existing installations on startup. Use an atomic create/write path that sets the mode before secret bytes are written.
2. Store API keys and tokens in the platform credential store (macOS Keychain) and persist only non-secret configuration plus opaque secret references in JSON. Apple documents Keychain Services as the platform storage facility for passwords and other small secrets: <https://developer.apple.com/documentation/security/keychain-services>.
3. Return a redacted settings DTO to the renderer. Treat a blank or unchanged secret field as “preserve existing secret” rather than round-tripping the real value through the webview.
4. Add tests for new-install and migration modes, including an existing `0644` file.

**Mitigation if full Keychain migration is deferred**

Enforcing `0700`/`0600` materially reduces exposure to other local accounts, but it does not protect secrets from another process running as the same user or from renderer compromise.

### SEC-002 — Unpinned, unverified native PDFium is downloaded and signed during releases

**Severity:** High
**Category:** CWE-494 (Download of Code Without Integrity Check)

**Evidence**

- `scripts/fetch-pdfium.sh:30-37` downloads `releases/latest/download/...tgz`, extracts `libpdfium.dylib`, and installs it without checking a version, checksum, signature, or provenance attestation.
- The release workflow invokes that script at `.github/workflows/release.yml:53-54`.
- The downloaded library is then signed with the application's Developer ID at `.github/workflows/release.yml:87-100` and bundled as a Tauri resource (`src-tauri/tauri.conf.json:45-47`).
- The local release script follows the same fetch-then-sign path at `scripts/release.sh:88-115`.

**Impact**

Compromise of the upstream release, maintainer account, release-asset replacement path, or delivery chain can insert arbitrary native code into a release. Alchemy then applies its own signature to that code, so users receive it as trusted code executing inside the application process with the user's data access and app permissions.

**Recommended fix**

1. Pin a reviewed PDFium release identifier and architecture-specific SHA-256 values in version-controlled source.
2. Download from an immutable, versioned asset URL; reject redirects to an unexpected host if the download tooling follows redirects.
3. Verify the archive hash before extraction and fail closed on any mismatch. If the upstream publishes signatures or provenance, verify those in addition to the committed hash.
4. Record the pinned version and hashes in the release PR so upgrades are explicit and reviewable.
5. Add a CI test that intentionally supplies a wrong checksum and confirms the fetch script refuses the artifact.

**Mitigation if dynamic “latest” tracking is required**

Fetch release metadata first, require an allowlisted signer/provenance identity, and pin the resolved release plus digest into the release commit before building. TLS and later code signing do not authenticate the downloaded bytes as the version Alchemy intended to ship.

### SEC-003 — CSP is disabled while the asset protocol exposes most of the home and temp trees

**Severity:** Medium
**Category:** Defense in depth / excessive resource exposure

**Evidence**

- `src-tauri/tauri.conf.json:29-38` sets `csp` to `null`, enables the asset protocol, and allows `$HOME/**` and `$TEMP/**` in addition to the app's audio directory.
- The renderer uses `convertFileSrc` for audio and image content at `src/components/AudioNote.tsx:18-23` and `src/components/ReaderPane.tsx:1190-1206`.
- Tauri states that CSP protection is only enabled when configured and recommends making it as restrictive as possible: <https://v2.tauri.app/security/csp/>.
- Tauri's asset-protocol guidance recommends narrow app/cache/resource directories rather than broad home-directory globs: <https://v2.tauri.app/security/asset-protocol/>.

**Impact**

This is not a demonstrated XSS by itself. However, if script execution is achieved in the privileged renderer through a future dependency or rendering flaw, there is no CSP containment and the asset protocol can expose ordinary files across the user's home and temp directories to the webview. With unrestricted network connections, those bytes could also be exfiltrated.

The current Markdown and Mermaid sanitizers reduce exploit likelihood but do not replace platform-level containment.

**Recommended fix**

1. Add a production CSP with a restrictive `default-src`, explicit `connect-src`, and only the asset/image/media sources the app actually needs. Test chat providers, local Ollama, updater flows, Mermaid, fonts, blob images, and audio under the policy.
2. Remove `$HOME/**` and `$TEMP/**` from the static asset scope. Keep app-owned media under dedicated app-data subdirectories.
3. For user-selected files outside app data, prefer copying/importing the needed bytes into an app-owned cache, returning bytes through a narrowly scoped command, or using Tauri's persisted dynamic scope rather than a permanent whole-home grant.
4. Add a regression test or packaged-app smoke check that verifies the CSP header is present and a known-disallowed connection is blocked.

### SEC-004 — Default-on localhost services rely on request origin rather than authentication

**Severity:** Medium
**Category:** CWE-306 (Missing Authentication for Critical Function)

**Evidence**

- MCP and clip services default to enabled in `src-tauri/src/ai/mod.rs:83-101`.
- MCP binds to `127.0.0.1`, but its only request guard rejects the presence of an `Origin` header; requests without `Origin` are accepted (`src-tauri/src/mcp/mod.rs:110-150`). There is no bearer token or client identity check.
- The MCP surface includes destructive notebook/source/note operations and file exports; for example, notebook deletion is implemented at `src-tauri/src/mcp/notebooks.rs:150-163` and note export to a caller-supplied path at `src-tauri/src/mcp/notes.rs:181-195`.
- MCP can also write through Alchemy's Apple Notes and Reminders integration at `src-tauri/src/mcp/mac.rs:38-117`.
- The clip service accepts requests with no origin or with any `chrome-extension://` or `moz-extension://` origin (`src-tauri/src/clip.rs:177-214`) and exposes the routes on loopback without authentication (`src-tauri/src/clip.rs:224-250`).

**Impact**

The browser-origin checks are useful against ordinary malicious web pages, but they are not authentication. Another local process or local OS account can call MCP without an `Origin` header to read or mutate Alchemy data, export files, or exercise Apple integration privileges granted to Alchemy. Any installed browser extension with localhost network access can submit clip payloads, enabling capture-cache poisoning. The practical likelihood is lower on a single-user Mac, which is why this is rated Medium rather than High.

**Recommended fix**

1. Generate a high-entropy per-installation or per-session bearer credential and require it on every MCP and clip request.
2. Store discovery files and token material with mode `0600` inside a `0700` directory. Do not put tokens in URLs or logs.
3. For the browser extension, allowlist the expected extension identifier and use an authenticated challenge or shared secret; do not treat the extension URI scheme alone as identity.
4. Preserve the browser-origin rejection as an additional defense, and add tests covering missing, invalid, rotated, and valid credentials.
5. Consider making the services opt-in until authenticated transport is implemented, particularly on shared workstations.

**False-positive / design note**

Loopback binding prevents remote-network access, and a same-user unsandboxed process may already have broad file access. The remaining concern is still material because MCP exposes convenient destructive operations, caller-chosen export paths, and permissions delegated to the Alchemy process.

### SEC-005 — OKF ZIP extraction has no decompression resource limits

**Severity:** Medium
**Category:** CWE-770 (Allocation of Resources Without Limits or Throttling)

**Evidence**

- `src-tauri/src/commands.rs:10372-10395` iterates every ZIP entry and streams it to disk with `std::io::copy`, but sets no entry-count, per-file, total-uncompressed-size, compression-ratio, or elapsed-time limit.
- Path traversal is correctly blocked with `enclosed_name()` at `src-tauri/src/commands.rs:10380-10383`.
- Cleanup in `src-tauri/src/commands.rs:10480-10490` only receives the scratch directory after extraction succeeds. If extraction fails after writing partial data—for example because the disk fills—the partial scratch directory is not returned for cleanup.

**Impact**

A user-selected malicious `.okf.zip` can expand far beyond its compressed size, consume available disk space, stall the import, and leave partial files in the system temp directory. Exploitation requires the user to open/import the archive, so this is a local denial-of-service issue rather than code execution.

**Recommended fix**

1. Before extraction, reject archives exceeding a conservative entry-count limit and sum declared uncompressed sizes using checked arithmetic against a total budget.
2. Enforce limits while streaming too—do not trust ZIP metadata. Cap each entry and the actual aggregate bytes written, aborting as soon as the budget is exceeded.
3. Optionally reject extreme compression ratios and unsupported entry types.
4. Use an RAII temporary directory so every error path removes partial extraction output.
5. Add tests for traversal names, too many entries, oversized declared sizes, actual streamed bytes over the cap, integer overflow, and cleanup after failure.

## Positive controls observed

- Untrusted Markdown passes through `rehype-sanitize` after raw HTML parsing (`src/components/Markdown.tsx:9-18`).
- Mermaid is configured with `securityLevel: "strict"`, disables HTML labels, and sanitizes SVG output with a narrow SVG profile and explicit dangerous tag/attribute blocks (`src/lib/mermaid.ts:145-181`).
- Model-adjacent terminal repair commands are constrained to a fixed allowlist and a validated Ollama model-name grammar (`src-tauri/src/commands.rs:657-674`, `src-tauri/src/commands.rs:907-932`).
- ZIP path traversal is rejected with `enclosed_name()`.
- The updater is configured with a public verification key (`src-tauri/tauri.conf.json:105-109`).
- MCP rejects browser origins and now requires bearer authentication before allocating a session.

## Dependency and CI observations

- `pnpm audit --prod --audit-level low`: no known production vulnerabilities found on 2026-08-31.
- `cargo audit`: no vulnerability-class advisories found on 2026-08-31. It did report maintenance/quality warnings, including a yanked transitive `chacha20` release; the reported `glib` advisory is not present in the checked macOS target graph.
- `.github/workflows/ci.yml` installs from the lockfile and runs build, tests, formatting, and Clippy, but does not currently run JavaScript or Rust advisory checks. Add scheduled and pull-request advisory scanning (with a documented warning policy) so the clean result does not depend on manual review.
- GitHub Actions are referenced by mutable version tags rather than immutable commit SHAs. Pinning third-party actions to reviewed SHAs would further reduce workflow supply-chain exposure.

## Scope and limitations

The original assessment was a basic, read-only static review plus local metadata and dependency-audit checks. Remediation added focused unit and integration tests, but did not include fuzzing, reverse engineering the packaged binary, GitHub organization/secret review, or a complete audit of the Rust dependency source. Findings are based on the reviewed revision and the local file modes observed on 2026-08-31; remediation status refers to `codex/security-hardening`.
