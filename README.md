# Alchemy

A local-first, privacy-respecting clone of [NotebookLM](https://notebooklm.google/),
built as a native desktop app. Import your sources, chat with them grounded in
citations, and generate documents — all running **100% on your machine**
via [Ollama](https://ollama.com). No API keys, no cloud, nothing leaves your laptop.

Prefer a cloud LLM (or can't run local models)? Alchemy also routes chat through any
OpenAI-compatible gateway, including **[IBM Bob](https://bob.ibm.com)** — while
keeping your sources on-device via a built-in embedder. See
[Using a cloud LLM](#using-a-cloud-llm-ibm-bob-or-any-openai-compatible-gateway).

> Built with **Tauri 2 + React** front-end, a **Rust** backend, **LanceDB** for
> embedded vector + relational storage, and a **Linear-inspired** UI with 12 themes.

[![CI](https://github.com/thrashr888/alchemy/actions/workflows/ci.yml/badge.svg)](https://github.com/thrashr888/alchemy/actions/workflows/ci.yml)

---

## Features

- **Notebooks** — a home screen of notebooks (most-recent first); opens to your last one.
- **Sources** — import **PDF**, **Office** (`.docx` / `.pptx` / `.xlsx`), **images**,
  **text**, **Markdown**, paste text, or fetch a **URL**. Each is extracted, chunked,
  and embedded locally. Drag-and-drop onto the Sources panel. Failed/blocked imports
  show an error badge and can be retried; edited/refreshed sources are re-embedded.
- **OCR** — image sources and scanned/image-only PDFs are transcribed by a local
  vision model (dedicated OCR models like `glm-ocr` / `deepseek-ocr` recommended).
- **Grounded chat** — streamed answers that cite the exact source passages they drew
  from, with a **"Deep research"** agentic mode that plans multiple retrieval steps.
  Copy a response or save it as a note.
- **Studio generators** — one-click **Summary**, **FAQ**, **Study guide**, **Briefing**,
  **Timeline**, **Problems** (finds errors/gaps/contradictions), plus HashiCorp-style
  **PRD**, **PR/FAQ**, **RFC**, and a **Skill** (SKILL.md) generator. Add custom
  instructions, and **rebuild** any document against the latest sources.
- **Notes** — a **WYSIWYG** editor (Markdown under the hood), copy to clipboard, and
  **Convert to source** to fold a note into the retrievable source set.
- **Periodic reports** — schedule a notebook to refresh its URL sources and generate a
  timestamped report note on an interval.
- **Model tooling** — live chat/embed **health check**, per-model **tokens/sec**
  tracking, MLX-accelerated model suggestions, and safe **re-embed-on-model-switch**.
- **12 themes** — Midnight, Light, Slate, Dracula, Monokai, GitHub, Solarized,
  Tokyo Night, Claude, OpenAI, Catppuccin Latte, Sepia.

## Architecture

```
┌──────────────────────────────── Tauri window ───────────────────────────────┐
│  React + Tailwind                                                            │
│  Home (notebook picker)  |  Sources │ Chat (streaming) │ Studio (docs+notes) │
└───────────────────────────────── IPC (invoke / events) ─────────────────────┘
                                     │
┌───────────────────────────────── Rust backend ──────────────────────────────┐
│  commands.rs   Tauri command surface + per-model stats                       │
│  ingest.rs     extract (pdf/office/url/text) → normalize → chunk             │
│  pdf.rs        PDFium page rasterization for scanned-PDF OCR                  │
│  ai/ollama.rs  embeddings, streaming chat, OCR over Ollama HTTP              │
│  agent.rs      agentic "deep research" retrieval loop                        │
│  rag.rs        retrieval prompt assembly + generator prompts                 │
│  db.rs         LanceDB tables: notebooks, sources, chunks(+vectors),         │
│                messages, notes, report_schedules                             │
└──────────────────────────────────────────────────────────────────────────────┘
                                     │
                              Ollama (localhost:11434)
```

The RAG loop: a question is embedded, the `chunks` table is vector-searched
(filtered to the active notebook), the top passages become numbered excerpts in
the prompt, and the model answers with `[n]` citations that map back to the
retrieved chunks shown in the UI. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Install (Apple Silicon)

Download the latest `Alchemy_x.y.z_aarch64.dmg` from
[Releases](https://github.com/thrashr888/alchemy/releases), open it, and drag
**Alchemy** to Applications. The build is ad-hoc signed (not notarized), so on
first launch **right-click → Open** (or run `xattr -cr /Applications/Alchemy.app`).

Requires [Ollama](https://ollama.com) running locally.

## Prerequisites (development)

- **[Ollama](https://ollama.com)** running locally (`ollama serve`).
- Models pulled — for example:

  ```bash
  ollama pull nomic-embed-text        # embeddings
  ollama pull gpt-oss:120b            # chat (or any chat model)
  ollama pull glm-ocr                 # OCR (optional, for images / scanned PDFs)
  ```

- **Rust** (stable) and **Node** with **pnpm**. `protoc` is required to build
  LanceDB (`brew install protobuf`).

## Develop

```bash
pnpm install
pnpm tauri dev
```

The first build compiles LanceDB and is slow; subsequent builds are fast.

## Test & lint

```bash
cd src-tauri
cargo test          # unit tests (+ a graceful-skip Ollama integration test)
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
```

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs the frontend
typecheck/build plus the above on every push and PR.

## Configuration

Open **Settings** (gear icon) to set the Ollama URL and choose models. Defaults:

| Setting          | Default                    |
| ---------------- | -------------------------- |
| Ollama URL       | `http://localhost:11434`   |
| Chat model       | `gpt-oss:120b`             |
| Embedding model  | `nomic-embed-text:latest`  |
| Vision model     | _(unset — OCR disabled)_   |

Switching the embedding model prompts to **re-embed all sources** (models produce
incompatible vectors), so retrieval never silently breaks. Data is stored in the OS
app-data directory under `lancedb/`.

## Using a cloud LLM (IBM Bob or any OpenAI-compatible gateway)

Ollama stays the default, but Alchemy can route **chat, generation, deep research,
and OCR** through any OpenAI-compatible gateway instead — useful on machines that
can't run local models (e.g. a 16 GB laptop). **[IBM Bob](https://bob.ibm.com)** is
supported out of the box, and the same path works for LM Studio, vLLM, LiteLLM, or
Ollama's own `/v1` endpoint.

With a gateway selected, **sources still stay local**: embeddings run on the
**built-in CPU embedder** (a ~30 MB model downloaded on first use, no Ollama
required), so nothing but your chat turns leaves the machine.

### Getting an IBM Bob API key

Bob is IBM's internal AI gateway (LiteLLM under the hood). If you have Bob access:

1. Sign in to the **[Bob web portal](https://bob.ibm.com/login)** with your IBMid.
2. Open **API Keys** and create a key with the **Inference** scope.
3. Copy the key — it starts with `bob_` (e.g. `bob_prod_…`). Copy the whole string.

You can also confirm access from the CLI — the same key works there:

```bash
export BOBSHELL_API_KEY="bob_prod_…"
```

### Configuring it in Alchemy

During **onboarding** (or later in **Settings → Models**):

1. Switch the provider to **IBM Bob**.
2. Paste your `bob_…` key. Leave **Gateway URL** empty to use the default
   (`https://api.us-east.bob.ibm.com/inference/v1`).
3. Click **Save & check** — Alchemy lists Bob's models, auto-selects one, and shows
   **Connected**. Pick a different model from the dropdown any time.

Keys are stored only in the local `ai_config.json` and sent solely as an auth header
to the gateway you configure — never anywhere else. Usage is billed to your Bob
account (silently; Alchemy does no in-app accounting).

| Setting          | Default                                       |
| ---------------- | --------------------------------------------- |
| Provider         | IBM Bob                                        |
| Gateway URL      | `https://api.us-east.bob.ibm.com/inference/v1`|
| Chat model       | _(pick from the gateway's list)_              |
| Vision model     | `sonnet-4.6` (OCR for images / scanned PDFs)  |
| Embeddings       | Built-in (`potion-base-8M`, on-device CPU)    |

## Releases

Releases are built by GitHub Actions
([.github/workflows/release.yml](.github/workflows/release.yml)) on any `v*` tag
(or manual dispatch), producing a **macOS arm64** `.dmg` in a **draft** GitHub
Release. Cut one with:

```bash
# bump version in package.json + src-tauri/tauri.conf.json first
git tag v0.1.0 && git push origin v0.1.0
```

Code signing/notarization is optional — set the `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`,
`APPLE_PASSWORD`, and `APPLE_TEAM_ID` repo secrets to produce a signed, notarized
build; without them the app is ad-hoc signed (open it the first time via
right-click → Open).

The app bundles the [PDFium](https://github.com/bblanchon/pdfium-binaries) library
(for scanned-PDF OCR). An Intel (x86_64) build is possible on a `macos-13` runner
but those runners queue for hours; it can be re-added to the matrix if needed.

## Scope

Audio/video overviews are intentionally out of scope. Notes are not embedded into
retrieval on their own — **Convert to source** to make a note retrievable.
