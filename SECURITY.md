# Security

Alchemy is local-first: sources, embeddings, chat history, and generated
documents live on your Mac. Nothing leaves the device unless you configure a
remote model provider or opt a notebook into a web feature.

## Reporting a vulnerability

Report privately through GitHub: **Security → Report a vulnerability** on
this repository. Include steps to reproduce and the app version (Alchemy →
About). Please don't open public issues for security reports.

## What listens on this machine

Two loopback-only services, both switchable in Settings:

- **MCP server** (`127.0.0.1:41414`) — agent access to notebooks. Every
  request must carry the per-installation bearer token from the owner-only
  discovery file `<app-data>/mcp.json`; requests carrying a browser `Origin`
  header are rejected outright.
- **Clip receiver** (`127.0.0.1:41500`) — accepts rendered-page handoff only
  from the published Chrome clipper's exact extension origin. Everything
  else, including Firefox's per-install extension origins, falls back to
  URL-only clipping.

## Hardening in place

- Credential-bearing config and token files are written atomically with
  owner-only permissions (`0700` directories, `0600` files).
- The webview runs under a restrictive CSP; the asset protocol is scoped to
  app-owned audio, and original images load by source id through a bounded
  backend command rather than by filesystem path.
- The bundled PDFium library is pinned by version and per-architecture
  SHA-256 and verified before extraction; CI proves a tampered archive is
  refused.
- Notebook (`.okf.zip`) imports enforce entry-count, per-file, total-size,
  and compression-ratio budgets, reject unsafe paths, and clean up after
  failed extraction.
- Untrusted Markdown is sanitized, Mermaid renders in strict mode with
  sanitized SVG output, and app updates are signature-verified.

Point-in-time audit reports live in pull-request history rather than in the
tree; this file tracks the current posture.
