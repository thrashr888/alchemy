# Alchemy

A local-first, privacy-respecting clone of [NotebookLM](https://notebooklm.google/),
built as a native desktop app. Import your sources, chat with them grounded in
citations, and generate study documents — all running **100% on your machine**
via [Ollama](https://ollama.com). No API keys, no cloud, nothing leaves your laptop.

> Built with **Tauri 2 + React** front-end, a **Rust** backend, **LanceDB** for
> embedded vector + relational storage, and a **Linear-inspired** dark UI.

---

## Features

- **Notebooks** — organize sources into separate notebooks.
- **Sources** — import **PDF**, **text**, **Markdown**, paste raw text, or fetch a **URL**.
  Each source is extracted, chunked, and embedded locally.
- **Grounded chat** — ask questions and get streamed answers that cite the exact
  source passages they drew from. The model is instructed to answer *only* from
  your sources.
- **Studio artifacts** — one-click **Summary**, **FAQ**, **Study guide**,
  **Briefing doc**, and **Timeline** generated from your sources.
- **Notes** — write your own Markdown notes alongside generated documents.
- **Settings** — point at any Ollama instance and pick your chat / embedding models.

## Architecture

```
┌──────────────────────────────── Tauri window ───────────────────────────────┐
│  React + Tailwind (Linear theme)                                             │
│  Sidebar │ Sources │ Chat (streaming) │ Studio (artifacts + notes)           │
└───────────────────────────────── IPC (invoke / events) ─────────────────────┘
                                     │
┌───────────────────────────────── Rust backend ──────────────────────────────┐
│  commands.rs   Tauri command surface                                         │
│  ingest.rs     extract (pdf/text/md/url) → normalize → chunk                 │
│  ai/ollama.rs  embeddings + streaming chat over Ollama HTTP                  │
│  rag.rs        retrieval prompt assembly + artifact prompts                  │
│  db.rs         LanceDB tables: notebooks, sources, chunks(+vectors),         │
│                messages, notes                                               │
└──────────────────────────────────────────────────────────────────────────────┘
                                     │
                              Ollama (localhost:11434)
```

The RAG loop: a question is embedded, the `chunks` table is vector-searched
(filtered to the active notebook), the top passages become numbered excerpts in
the prompt, and the model answers with `[n]` citations that map back to the
retrieved chunks shown in the UI.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for details.

## Prerequisites

- **[Ollama](https://ollama.com)** running locally (`ollama serve`).
- An **embedding model** and a **chat model** pulled:

  ```bash
  ollama pull nomic-embed-text        # embeddings (recommended)
  ollama pull llama3.1                # or any chat model you like
  ```

- **Rust** (stable) and **Node 18+** with **pnpm**.

## Develop

```bash
pnpm install
pnpm tauri dev
```

The first build compiles LanceDB and is slow; subsequent builds are fast.

## Build a release bundle

```bash
pnpm tauri build
```

## Configuration

Open **Settings** (gear icon) to set the Ollama URL and choose models from the
list of installed models. Defaults:

| Setting          | Default                    |
| ---------------- | -------------------------- |
| Ollama URL       | `http://localhost:11434`   |
| Chat model       | `gpt-oss:120b`             |
| Embedding model  | `nomic-embed-text:latest`  |
| Vision model     | _(unset — OCR disabled)_   |

Data is stored in the OS app-data directory under `lancedb/`.

## Notes & limitations

- Embedding dimensionality is detected from your embedding model on first
  ingest; switching to a model with a different dimension means existing chunks
  won't match — clear sources or start a fresh notebook if you change it.
- URL import does naive HTML-to-text extraction (good enough for articles).
- Audio Overview / slideshow generation are intentionally out of scope.

## Building & releases

```bash
pnpm install
pnpm tauri dev      # run locally
pnpm tauri build    # produce a .app + .dmg in src-tauri/target/release/bundle
```

Releases are built by GitHub Actions ([.github/workflows/release.yml](.github/workflows/release.yml))
on any `v*` tag (or manual dispatch). It builds **macOS arm64 + x86_64** `.dmg`s
and opens a **draft** GitHub Release with the assets. Cut a release with:

```bash
# bump version in package.json + src-tauri/tauri.conf.json first
git tag v0.1.0 && git push origin v0.1.0
```

Code signing/notarization is optional — set the `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`,
`APPLE_PASSWORD`, and `APPLE_TEAM_ID` repo secrets to produce a signed build;
without them the app is unsigned (open it the first time via right-click → Open).

The app bundles a per-architecture [PDFium](https://github.com/bblanchon/pdfium-binaries)
library for scanned-PDF OCR (downloaded per target in CI). Linux/Windows
releases would each need their own PDFium binary and are not yet wired up.
