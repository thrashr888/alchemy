Sources are the raw material of a notebook. Alchemy extracts each one, splits it into structure-aware chunks, and embeds those chunks locally, so search and chat can reach into the exact passage you need.

## What you can import

- **Files** — PDF, Office documents (.docx, .pptx, .xlsx), Box Notes, CSV/TSV, images, plain text, and Markdown. Drag them onto the Sources panel or use Add Source.
- **Pasted text** — paste anything; it becomes a searchable source with a title.
- **URLs** — paste a link and Alchemy fetches and extracts the page. Link-shared Google Docs, Sheets, and Slides work too, either by pasting the link or dragging the .gdoc / .gsheet / .gslides stubs from a local Google Drive folder.
- **Images and scanned PDFs** — transcribed by a local vision model (dedicated OCR models such as glm-ocr or deepseek-ocr are recommended in Settings).
- **Folders** — add any folder, or start from a detected cloud sync root (Google Drive, OneDrive, Dropbox, Box, iCloud Drive). Folder sources keep syncing on a timer, and cloud placeholder files are listed without forcing a download.
- **Mac apps** — connect Apple Notes (individual notes), Reminders lists, rolling Calendar windows, and Stocks watchlists. They re-sync automatically, and edits you make in Alchemy write back to the real app.
- **The web clipper** — a browser extension adds the current page, a link, or a selection to a notebook from the toolbar. Whole-page clips hand Alchemy the rendered DOM from your logged-in tab, so pages behind a login still capture — over a local endpoint, nothing leaves your Mac.

## Finding files you half-remember

Type a few characters into Add Source and a Spotlight-backed search returns ranked file and folder hits from your Mac, addable in one click. Recently modified, name-matching, ingestible files float to the top; Trash, node_modules, and Library noise is filtered out.

## Living sources

File sources remember their on-disk path: **Refresh** re-reads a changed file, URL sources re-fetch, and **Show in Finder** jumps to the original. Edited or refreshed sources are re-embedded automatically. Failed or blocked imports show an error badge and can be retried.

Every source also gets a **gist** — a short distilled overview written in the background — so corpus-wide questions like "which source covers X?" find the right document even when no single passage is an obvious match.

## Controlling what chat sees

The checkbox next to each source controls whether it participates in retrieval for your next question. For one-off precision, type **@** in the chat composer to name a specific source, folder, or note — that question retrieves from exactly what you named, overriding the checkboxes for that message.

## Sharing notebooks

A whole notebook (sources plus notes) exports as an Open Knowledge Format (OKF) bundle: plain markdown with YAML frontmatter, readable by humans and agents alike. Share it as a single .okf.zip, and import someone else's by dragging the zip onto the window — sources re-embed locally on arrival, and duplicates are skipped.

A practical note on scale: retrieval quality holds as notebooks grow. The retrieval budget adapts to corpus size, so a notebook with millions of characters stays fully searchable instead of diluting every answer. Import generously.
