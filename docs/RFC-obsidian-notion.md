# RFC: Obsidian vaults & Notion pages as living sources

People keep their thinking in two places Alchemy can't read today: an
Obsidian vault on disk and a Notion workspace behind an API. Both should
be sources in the git-sources sense — living, resyncing, citable — not
one-shot imports that rot the moment the user keeps writing.

The asymmetry is the design: **an Obsidian vault is already almost a
folder source** (markdown files, on disk, watched by the existing
sweep), so phase 1 is vault *awareness*, not a new pipeline. **Notion is
a real integration** — token, API, block model, rate limits — and gets
the grammar-detect + tiered-ingest treatment git URLs got.

## 1. Goals

- A vault or a Notion page tree behaves like a repo source: add once,
  it stays fresh on the sync cadence, children appear as sub-sources
  with per-file show/hide, citations open the reader.
- Zero new chrome. Adding happens through the surfaces that exist:
  drop/pick a folder (vault auto-detected), paste a `notion.so` URL
  (grammar-detected), or the existing add dialog.
- Notion credentials follow the house rule: stored locally, sent only
  to Notion, never required for the rest of the app to work.

## 2. Non-goals

- **Write-back** to vaults or Notion (same call as RFC-git-sources §2;
  revisit only when the reader grows editing for local files).
- Obsidian plugins, canvas files, or graph-view parity. `.canvas` JSON
  and plugin data are skipped, not translated.
- Notion comments, users, permissions, or real-time blocks. We read
  page content, not workspace social state.
- OAuth app review. v1 uses an internal integration token the user
  creates themselves — no hosted redirect URI, nothing to deploy.

## 3. Obsidian: vault-aware folder sources

Detection: a picked/dropped folder containing `.obsidian/` is a vault.
The folder-source pipeline runs as today, with four vault behaviors:

- **Wikilinks resolve.** `[[Note Name]]` and `[[note#heading|alias]]`
  resolve against the vault's file set (Obsidian's shortest-unique-path
  rule) and rewrite to the target's source id at chunk time, so
  citations and the reader can hop between notes the way the vault
  does. Unresolvable links pass through as plain text.
- **Frontmatter is provenance, not prose.** Leading YAML is stripped
  from embed text (it poisons retrieval with key soup) and surfaced in
  the reader header; `tags:` join the chunk's embed prefix the way
  `[repo › path]` does, so tag vocabulary carries topical signal.
- **Embeds inline shallowly.** `![[image.png]]` becomes the image
  source if the file ingests anyway; `![[Other Note]]` becomes a
  wikilink (no transclusion expansion — chunk budgets stay per-file).
- **Dot-dirs skip.** `.obsidian/`, `.trash/`, and `.git/` never ingest;
  everything else follows the existing ignore-walk rules.

Vaults are prose, so the repo tier logic (code stays grep-only) never
triggers — a vault embeds fully like any docs folder. Resync is the
existing folder sweep; no new cadence machinery.

## 4. Notion: grammar, ingest, refresh

**Auth.** Settings → Sources gains a "Notion" row: paste an internal
integration token (`ntn_…`, created at notion.so/my-integrations; the
user shares specific pages with it inside Notion — that sharing step IS
the permission model, and the hint text says so). Token lives in the
config file like gateway keys, is sent only to `api.notion.com`, and its
absence changes nothing else in the app.

**Grammar.** The URL router learns `notion.so`/`*.notion.site` shapes
the way it learned git hosts. Pasting a page URL adds that page; the
32-hex id at the URL tail is the API handle. Without a token, the add
dialog explains and links the Settings row rather than failing opaquely.

**Ingest.** One page URL becomes one parent source; child pages become
sub-sources (the folder/repo children pattern, same show/hide and
promote/demote). Blocks convert to markdown: headings, paragraphs,
lists, quotes, callouts (→ blockquote), code (fenced, language kept),
tables, toggles (→ heading + body). Databases render as a markdown
table of their rows' properties; a row that is itself a page ingests as
a child page. Unknown block types degrade to their plain-text content.

**Refresh.** Notion returns `last_edited_time` per page — a cleaner
content stamp than git's ls-remote. The existing source sweep polls it
on the same cadence setting git sources use (default hourly); only
changed pages re-fetch. Rate limit is ~3 req/s: fetches serialize
through one polite queue with backoff on 429, reusing the gateway
retry shape.

## 5. Retrieval & citations

Nothing new. Vault notes and Notion pages are prose sources: chunked by
the structure-aware chunker, embedded with `[title › section]`
prefixes, cited with click-to-highlight in the reader. Wikilink and
child-page edges land in the same source-relation rows folder children
use, so the reader's tree pane works unchanged.

## 6. Phases

1. **Vault awareness** ✓ — detection (`.obsidian/` → `obsidian` source
   type, upgraded in place on rescan for pre-existing folders), wikilink
   resolution, frontmatter handling, dot-dir skips, plus a distinct icon
   / "Obsidian vault" label / vault map header. Gate met: a vault reads
   its wikilinks as hops in the file view.
2. **Notion pages** ✓ — token row (with a live workspace check), URL
   grammar, page + children ingest, `last_edited_time` refresh. Gate
   met: a shared page URL imports as a living source; edits re-export on
   the sweep.
3. **Notion databases** ✓ — `child_database` blocks resolve to an inline
   markdown table of their rows' properties (title column first; the
   common property types flatten to text, bounded at 500 rows). Row-page
   *bodies* aren't expanded yet — the table answers "what's in this
   list", which was the gate. Standalone database URLs (not embedded in
   a page) remain a follow-up.

Each phase lands behind the usual quality pass; no flags — vault
detection and the grammar are inert for users who never touch them.

## 7. Open questions

- Vault wikilink resolution when two vaults share a notebook: ids are
  per-source-tree, so collisions can't cross vaults — but should
  unresolved links surface in the repo-map style outline as "wanted
  notes"? (Cheap, possibly delightful; defer until asked.)
- Notion blocks that embed external URLs (bookmarks, embeds): ingest as
  the URL string now; auto-adding them as URL sources is a
  tool-route-style suggestion later.
- `notion.site` public pages without a token: scrapeable via the
  render-capture path (RFC-page-capture) today — the grammar should say
  so instead of demanding a token for public content.
