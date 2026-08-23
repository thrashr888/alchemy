# RFC: Multi-Select — Finder-style selection for sources and notes

Status: implemented (answers the backlog item "Allow drag to select for sources and notes, and shift-select, similar to Finder file selections, with the right-click dropdown options for refresh or remove"). One design change from the draft, noted in §Row structure: `CardAction` stays.

## Summary

Every list operation in the app is single-item: refresh one source, delete one note, tag one source, each with its own confirm and its own full re-list. Managing a 40-source notebook means 40 round trips through the ⋯ menu.

The proposal is Finder's selection grammar on the sources and notes lists — **rubber-band drag, shift-click range, ⌘-click toggle, ⌘A, Escape** — with the existing right-click menu becoming context-aware: right-click inside a selection shows batch verbs (Refresh 5 sources, Edit tags…, Remove 5…), right-click outside it collapses selection to that row and shows today's single-item menu. Plain click keeps opening the item — this is a reading app, not a file manager, so selection rides on modifiers and drag, never steals the primary click.

One selection axis, one owner: a new `picked` slice in the Zustand store, deliberately separate from `selectedSourceIds` (which is *chat scope* with inverted null-means-all semantics, not a UI selection). Batch verbs land as batched Rust commands — one IPC call, one re-list, one toast — and the matching MCP tools accept id arrays for agent parity.

## What exists today

- **No selection anywhere.** `selectedSourceIds` (`storeTypes.ts`) is retrieval scope: `null` = everything on, the map holds only *deselected* ids, persisted per notebook. Reusing it would conflate "in my chat context" with "about to be deleted" — disqualifying.
- **Right-click already works on every row.** `RowMenu` (`ui.tsx`) attaches a `contextmenu` listener to its nearest `.group` ancestor and opens the same ⋯ menu, portal-rendered with full keyboard nav. The primitive exists; it needs cursor positioning and swappable items.
- **The row click target is delicate.** Row content is `pointer-events-none` with an absolutely-positioned `CardAction` overlay swallowing the click (`SourcesPanel.tsx`, `StudioPanel.tsx`), `RowMenu` and the scope `SelectBox` floating above at `z-20`. Modifier clicks and a marquee cannot interpose behind that overlay.
- **Every mutation is single-item.** `refreshSource`/`deleteSource`/`deleteNote` each do one IPC call then a full re-list; looping them for N items produces N re-lists and N toasts. But the batch primitive exists on the Rust side: `db.delete_source_tree` already bulk-deletes folder children in one predicate because "a per-child loop was slow enough to trip the IPC timeout."
- **No shift/meta list handling** exists to conflict with; `shortcutBlocked(e)` is the house guard and `SHORTCUTS` in SettingsDialog is the registry (DESIGN.md §9).

## Design

### Selection model (store)

```ts
picked: { kind: "sources" | "notes"; ids: string[]; anchor: string | null } | null
```

- One selection at a time, app-wide — picking in Notes clears a Sources selection, like Finder windows.
- Actions: `pickOne`, `pickToggle`, `pickRange(orderedVisibleIds, toId)`, `pickAll(ids)`, `clearPicked`. Range order comes from the component's own `rows` memo (folders flattened, collapse-aware) — the store never re-derives list order.
- Cleared on notebook switch, Escape, and after a batch verb completes.

### Interaction grammar

- **⌘-click** toggles a row; **shift-click** ranges from anchor; **plain click** unchanged (opens viewer/note) but also collapses selection to that row and sets the anchor, so a following shift-click ranges naturally.
- **Rubber band**: pointer-down anywhere in the list that isn't a real control (menus, checkboxes, chevrons block it; the row surface and background don't) + 4px drag threshold starts a marquee; rows whose bounding rects intersect join the selection (additive with shift/⌘). Below threshold on background it's a clear; a drag that started suppresses the click that trails it.
- **⌘A** selects all sources by default, or all notes while a notes selection is active; it steps aside while the reader is open (select-all there means text). **Escape** clears; **Delete/Backspace** = Remove with the app confirm modal. All guarded by `shortcutBlocked(e)`, all registered in `SHORTCUTS`.
- **Right-click** on a row inside the selection → batch menu; outside it → selection collapses to that row, single menu (exact Finder behavior). Counts in labels: "Remove 5 sources…".

### Row structure

The draft proposed dropping the `CardAction` overlay for row-level activation; implementation kept it. The overlay exists to avoid nested interactive content (a row-as-button would wrap the menu and checkbox — the exact thing DESIGN §7 forbids), so instead `CardAction` now passes the click event through and the modifier branching (⌘/shift/plain) lives in its handler. Same z-order, same accessibility, one changed signature. Marquee lives on the list container and treats the overlay as row surface, not control. The chat-scope `SelectBox` column stays exactly as is — it's a different concept and keeps its far-right checkbox identity.

Selected rows get a quiet accent wash (`bg-accent/10`-style active state) — no left-border accents, per DESIGN §2/§7.

### Batch verbs

Sources (multi menu): **Refresh**, **Edit tags…** (applies to all), **Remove…**. Notes: **Copy text** (concatenated), **Remove…**. All are batch variants of existing single-item commands, per the backlog note.

Rust commands, one IPC call each:

- `delete_sources(notebook_id, ids)` — expands folder ids to their children, one Lance predicate delete (generalizing `delete_source_tree`), one `sources://changed` emit.
- `refresh_sources(notebook_id, ids)` — spawns a task iterating the existing refresh path sequentially; progress arrives through the ingest-queue/`sources://changed` events the UI already renders. No new progress UI.
- `delete_notes(notebook_id, ids)`; tags reuse `set_source_tags` per id server-side in one command.

### MCP

Per the backlog note ("keep agent parity by letting the matching MCP tools/commands accept id arrays"): `delete_source`, `set_source_tags`, and `delete_note` gain an optional `ids` array alongside `id`; a new `refresh_source` tool (id or ids) closes the existing gap where refresh is Tauri-only. Agent-reachable per house convention.

## Out of scope (v1)

- Gallery-pane selection (masonry marquee is its own geometry problem; the panel lists are where management happens). Fast-follow.
- Drag-and-drop of selected items (into folders, between notebooks) — selection is the prerequisite, DnD is its own RFC.
- Batch export, batch Second Look, batch "file under card".
- Arrow-key selection navigation (up/down + shift-arrows). Natural v2 on top of the same model.

## Alternatives considered

- **Reuse `selectedSourceIds` checkboxes** — rejected: inverted semantics, per-notebook persistence, and "in chat scope" ≠ "selected for an operation." Overloading it would make ⌘A silently change what chat sees.
- **Checkbox-per-row edit mode** (Gmail style) — rejected: adds a mode and a fourth interactive element per row; Finder grammar is modeless and the backlog item asks for it by name.
- **Frontend loop over single-item commands** — rejected: N re-lists, N toasts, and the folder-child precedent already proved per-item loops trip the IPC timeout.
- **A dedicated ContextMenu component** — rejected: `RowMenu` already owns right-click, keyboard nav, and glass styling; it needs cursor coords and swappable items, not a rewrite.

## Recommendation

Build the store slice + interaction grammar on SourcesPanel and StudioPanel, restructure the two rows off `CardAction`, extend `RowMenu` with cursor positioning + batch item sets, add the three batch commands, and extend the four MCP tools. [RFC-source-hygiene.md](RFC-source-hygiene.md)'s review modal consumes the same batch commands.
