# RFC: sync backend — own the engine, rent dumb storage

Alchemy is one Mac's app. A user with a laptop and a desktop today moves
notebooks by not moving them — re-importing sources by hand and losing
the chat and notes that made the notebook worth keeping. The question
this RFC answers: how should notebooks sync across a user's devices now,
and between users later — comparing (a) iCloud/CloudKit, (b) plain
object storage (S3-compatible), (c) Cloudflare Durable Objects + R2, and
(d) the do-nothing control of dropping the data dir in iCloud Drive.

The framing insight: **the app must stay fully functional offline, so
the sync engine — queue, merge, conflict rules — is client-side Rust in
every option.** The four candidates differ only in what sits on the
other end of the wire. So choose the dumbest transport that survives,
put it behind a trait, and upgrade transports — not engines — when
sharing between people becomes real.

## 1. Problem & goals

- **Multi-device, single user** is the problem worth solving now: same
  person, two Macs, one set of notebooks. User-to-user sharing is the
  later chapter and must not be foreclosed, but it doesn't get to
  complicate v1.
- **Offline is not a mode.** Sync is a background best-effort loop in
  the `sweep_due` style folder and git sources already use; no command
  ever blocks on the network, and a Mac that never syncs is a fully
  working Alchemy forever.
- **Privacy by default.** Source content never transits a third party
  in plaintext — payloads are encrypted on-device before upload, to
  storage the *user* chose. API keys and provider config never sync at
  all, in any phase; they are machine secrets (and machine-shaped:
  RAM-tiered model picks from RFC-inference-providers don't belong on
  another machine anyway).
- **Solo-dev weight.** No accounts system, no deployed service, nothing
  to babysit at 3am — until sharing earns it with evidence.
- **The notebook is the sync unit**, matching how every table is
  already keyed (`notebook_id`) and how sharing will eventually scope.
- **Every phase ships alone** and is useful without the next one.

## 2. Non-goals

- **Realtime co-editing.** No character-level merge, no presence
  cursors. §5 names the upgrade path; nothing in v1 blocks it.
- **A web viewer or publishing.** Different feature, different RFC.
- **An accounts system in v1.** Identity arrives only with sharing.
- **Syncing machine config** — provider prefs, gateway API keys, MCP
  port, capture profiles. Never by default; keys never, period.
- **Vectors or derived indexes over the wire** (§3 argues this).
- **Version-history browsing** — sync keeps devices convergent; it is
  not git-for-notebooks. Tombstones are plumbing, not a UI.
- **Windows/Linux parity** — macOS-first, like the rest of the app.

## 3. What syncs — canonical vs derivable

The LanceDB tables (db.rs) split cleanly:

| Table | Class | Why |
|---|---|---|
| `notebooks` | **canonical** | title/color/timestamps — bytes of user intent |
| `sources` | **canonical** | the extracted text *is* the record; re-fetching a URL later yields a different page, so `content` must travel |
| `notes` | **canonical** | user- and artifact-authored text, the co-edit surface |
| `messages` | **canonical** | chat history + citations JSON; append-only |
| `report_schedules` | **canonical** | small config, notebook-scoped |
| `embed_overrides/*.json` (app data) | **canonical** | per-file tier choices from RFC-git-sources |
| `chunks` (+vectors) | derivable | re-run `chunk_text`/`chunk_code` + embed on arrival |
| `routes` | derivable | `ensure_router` self-heals by diffing summaries |
| FTS index | derivable | rebuilt with the chunks table |
| `note_usage` | derivable-ish | local telemetry; stays local in v1 |
| `traces/*.jsonl` | local, always | documented as strictly local |
| git clones (`<app-data>/git/`) | derivable | re-clone from origin |
| audio overviews | derivable | regenerate; wavs are the biggest bytes we own |

**Vectors re-embed locally; syncing them would be wrong even if it were
free.** The chunks table is created lazily per embedding dimension
precisely because devices may run different embedders — a 16 GB laptop
on the builtin model2vec and a desktop on an Ollama model literally
cannot share the table. A vector runs 1–3 KB per chunk (256–768 dims of
f32), routinely bigger than the text it indexes. And the builtin
embedder re-embeds a whole notebook in seconds. Chunking is
deterministic given content, and citations are stored as verbatim JSON
snippets on the message row, so cited text survives a re-chunk on the
other side. Sync ships **sources + notes + messages + notebook config**;
the ingest pipeline re-derives the rest on arrival, exactly as if the
sources had been added locally.

One refinement: **derivable-elsewhere sources ship the recipe, not the
food.** A remote git source (RFC-git-sources) syncs its parent row,
scope, include-ladder rung, and pinned sha — the receiving device
re-clones and re-ingests to the identical state, kilobytes instead of a
repo. Origin-bound sources that *can't* re-materialize (folder paths,
mac sources from another machine's Notes) sync their content as
snapshots; the refresh sweeps simply no-op where the origin is absent,
so freshness stays a property of the device that owns the origin.

Payload reality: a text corpus is megabytes. Vectors, clones, and audio
— the gigabytes — never travel.

## 4. The four options

### 4a. iCloud/CloudKit

The seductive option: every Mac user is already signed in, the private
database [bills against the user's own iCloud quota — $0 to the
developer at any scale](https://developer.apple.com/icloud/cloudkit/),
and Developer ID apps outside the App Store [may hold CloudKit
entitlements](https://developer.apple.com/developer-id/).

- **Auth**: best in class — the OS session is the account. No signup.
- **Offline/conflict**: real support (change tokens, per-zone deltas),
  but the good path is [CKSyncEngine](https://developer.apple.com/documentation/cloudkit/cksyncengine)
  (macOS 14+), which is Swift-only.
- **The Rust problem is disqualifying.** There is no first-party Rust
  SDK. [CloudKit Web Services](https://developer.apple.com/library/archive/documentation/DataManagement/Conceptual/CloudKitWebServicesReference/SettingUpWebServices.html)
  looks like the escape hatch, but server-to-server keys reach only the
  **public** database — private-database access requires a per-user web
  auth redirect with expiring session tokens, a browser dance inside a
  desktop app. The honest native path is the Swift-sidecar pattern from
  RFC-inference-providers — except the FM sidecar is a thin pipe, and
  this sidecar would *be the sync engine*: CKSyncEngine state, batching,
  retry, and conflict logic living in the one language the app doesn't.
- **Privacy**: content transits Apple. It's the user's own account —
  defensible — but E2E is Apple's key schedule, not ours.
- **Cost** (1/100/10k users): $0/$0/$0. Unbeatable.
- **Weight**: provisioning profiles, no local emulator, dev/prod
  container environments, and CI that already fights codesign/notarize.
  Plus the ceiling: sharing later means CKShare, Apple IDs required,
  and any future web/cross-platform story walled off.

**Verdict: rejected as the engine, despite the price.** The $0 is real,
but it buys a Swift sync engine married to one transport we can't test
locally, can't self-host, and can't extend to non-Apple recipients.

### 4b. Plain object storage (S3-compatible)

The client is the whole engine; the server is a disk. Each device
appends encrypted op batches to a per-notebook log and pulls the
others':

```
<bucket>/alchemy/v1/
  devices/<device-id>.json                  # registry + embedder info
  notebooks/<nb-id>/log/<hlc>.<device>.age  # op batches, append-only
  notebooks/<nb-id>/snapshot/<hlc>.age      # periodic compaction
```

- **Auth**: the weak point, solved by scope. v1 is **bring-your-own
  bucket** — R2, S3, MinIO, anything S3-shaped — credentials pasted
  once per device into the Keychain. This is Zotero's WebDAV move
  ([data syncs free; files go to storage *you* configure](https://www.zotero.org/support/sync))
  and it is exactly right for single-user multi-device: the user's own
  storage, no vendor between their devices but the one they picked.
  Multi-tenant hosting would need a token service — that's option (c).
- **Offline/conflict**: append-only logs keyed by device id never
  contend; compaction claims use conditional writes, which [S3](https://aws.amazon.com/about-aws/whats-new/2024/08/amazon-s3-conditional-writes/)
  and [R2](https://developers.cloudflare.com/r2/api/s3/extensions/)
  both support. LWW semantics live entirely in the client (§5).
- **Privacy**: best available. Payloads are `age`-encrypted on-device
  (passphrase-derived key, Obsidian's shape: [E2E, vendor never sees
  plaintext or keys](https://help.obsidian.md/sync/security)); even
  the user's chosen bucket holds ciphertext.
- **Cost** (1/100/10k): $0 to the developer at every scale. Per user,
  [R2's free tier](https://developers.cloudflare.com/r2/pricing/)
  (10 GB, 1M class-A + 10M class-B ops/mo, zero egress) swallows a
  text corpus outright; S3 runs pennies.
- **Weight**: one Rust module. The `object_store` crate is *already in
  Alchemy's dependency graph* (Lance is built on it — Cargo.lock has
  0.13.2), `age` is a small pure-Rust add, and the polling loop is the
  existing `sweep_due` throttle pattern with a push debounce. A
  Settings pane, not a service.

**Verdict: the v1 transport.** All engine, no server, honest privacy.

### 4c. Cloudflare Durable Objects + R2

One DO per shared notebook: a single-threaded coordinator with SQLite
state that orders writes, assigns revisions, fans out over hibernating
WebSockets; R2 holds blobs.

- **Auth**: must be built — accounts, magic links, token issuance in
  the Worker. This is the actual cost of (c): it's the first option
  where Alchemy operates a service with users.
- **Offline/conflict**: server ordering makes multi-writer merge
  *easier* — but offline devices still queue and reconcile, so the
  client engine from (b) is built regardless. (c) = (b) + a server.
- **Privacy**: content transits Cloudflare; default remains client-side
  encryption with the DO relaying ciphertext it cannot read.
- **Cost** (1/100/10k): [$5/mo Workers Paid](https://developers.cloudflare.com/workers/platform/pricing/)
  at 1 and 100; at 10k users sync-delta traffic runs roughly $15–25/mo
  ([requests $0.15/M past 1M, hibernated sockets bill no duration,
  SQLite storage billing live since Jan 2026](https://developers.cloudflare.com/durable-objects/platform/pricing/)).
  Dollars are noise; the price is operating it.
- **Weight**: everything in (b) plus a TypeScript codebase, deploys,
  migrations, monitoring, abuse handling. Real weight for one person.

**Verdict: the right *sharing* architecture, bought too early if bought
now.** It reuses (b)'s op format verbatim — a DO is a smart log with a
push channel — so deferring it costs nothing but time not spent.

### 4d. Control: the data dir in iCloud Drive

Point iCloud Drive/Dropbox/Syncthing at the LanceDB directory and hope.

- Lance is a directory of versioned manifests and fragment files with
  invariants *between* files; file syncers replicate files
  independently and out of order. A half-arrived manifest points at
  fragments that aren't there yet; two devices compacting concurrently
  corrupt the dataset; iCloud's dataless-file eviction can page out a
  fragment mid-read. And because every write rewrites Lance versions —
  vectors included — the syncer re-uploads gigabytes forever.
- It also syncs exactly what must not travel (machine config) and
  misses what must (Keychain-held anything).

**Verdict: rejected as live sync — and clarifying as a control.** What
the control teaches: any real design must beat "zip it and AirDrop it."
So ship that honestly as phase 1 — a notebook archive — and let it
double as the backup story and person-to-person sharing v0.

| | (a) CloudKit | (b) object storage | (c) DO + R2 | (d) files |
|---|---|---|---|---|
| Auth | OS session | BYO bucket creds | build accounts | none |
| Offline engine | Swift sidecar | Rust, in-app | Rust + server | n/a |
| Privacy default | Apple's keys | E2E, user's bucket | E2E, CF relays | none |
| $ at 1/100/10k | 0/0/0 | 0/0/0 (dev) | 5/5/~20 per mo | 0 |
| Solo-dev weight | high, alien | low, native | medium, ongoing | zero |
| Sharing ceiling | Apple-only | crude (creds) | the real path | AirDrop |

## 5. Conflict model — LWW rows, tombstones, conflict copies

Single-user multi-device conflicts are rare and row-shaped. The model:

- **Ops, not table dumps.** Every mutation emits an op
  (`upsert`/`delete`, table, row id, row payload) stamped with a hybrid
  logical clock (`max(wall_ms, last+1)` — ~40 lines, no crate) plus
  device id. Op batches append to the log; devices apply each other's
  logs idempotently.
- **Last-writer-wins per row**, ordered by (HLC, device id). Rows
  gain a `rev` column via the additive lazy-migration pattern db.rs
  already uses (the `field_with_name` upgrades for `color`, `kind`,
  `model`) — no schema migration event.
- **Messages are append-only**: UUID ids, union merge, conflicts
  impossible by construction.
- **Deletes are tombstones** in a small `tombstones` table (table, row
  id, HLC), retained 90 days so a long-offline device can't resurrect
  the dead; live tables stay clean and Lance deletes stay real.
- **Notes get one special case.** Notes are the only surface where
  both sides plausibly edit the same text while apart. When two
  upserts to one note straddle the common ancestor, newer wins and the
  loser is written back as a sibling note ("Title (conflict from
  MacBook)") — Obsidian's conflict-copy behavior, which loses nothing
  and needs no merge UI. Zotero's per-object versions with a resolution
  dialog solve the same problem with more ceremony than a notebook
  needs.
- **CRDTs: no, and here's the tripwire.** Automerge/loro buy
  character-level convergence for live co-editing — the problem §2
  excluded. Anytype's [any-sync](https://github.com/anyproto/any-sync)
  shows what full-CRDT costs: it's a platform (tree CRDTs, ACLs,
  consensus-free verification), not a feature. Obsidian ships
  diff-match-patch and conflict copies; that's the weight class of
  this app. The named upgrade: if shared notebooks (phase 3) grow
  simultaneous note editing in practice, note *bodies* — and only note
  bodies — move to a CRDT text type behind the same op format.

## 6. Recommendation

**Build the transport-agnostic op log in Rust (§5), ship the notebook
archive first, run v1 sync over the user's own S3-compatible bucket
with client-side encryption, and add the Durable Object service only
when a second human asks to share a notebook — reusing the same op
format over its WebSocket.**

Why this and not the others, compressed: CloudKit's $0 buys a
Swift-resident engine on an untestable Apple-only transport — wrong
seam for a Rust app with a cross-user future. Durable Objects is the
correct sharing endgame but forces accounts and operations before any
second user exists — and since its client half is (b)'s engine anyway,
deferring it is free. File-syncing the live database corrupts it. The
BYO-bucket path is the only option that is pure Rust, already half in
the dependency tree, $0 at every scale that matters this year, E2E
encrypted by construction, and structurally a prefix of the sharing
architecture rather than a detour from it.

## 7. Phases

1. **Notebook archive (export/import).** A `.alchemy` zip: sources
   (rows + content), notes, messages, notebook row, embed overrides —
   no vectors, no clones. Import runs the normal ingest pipeline to
   re-chunk/re-embed. Doubles as backup and as sharing v0 (AirDrop the
   file). *Gate: export on Mac A, import on Mac B, chat with citations
   works after local re-embed; a re-import of the same archive dedups
   instead of duplicating.* Shippable alone — and worth shipping even
   if every later phase dies.
2. **The engine + S3-compatible transport.** Op emission on every
   mutation path, HLC + `rev` columns, tombstones, LWW apply,
   note conflict copies; `Transport` trait with the `object_store`
   implementation; `age` encryption with Keychain-held key; push
   debounced behind the existing sweep cadence, manual Sync Now;
   Settings → Sync pane (bucket, passphrase, per-notebook opt-out —
   default all on, smart-defaults rule). *Gate: two real Macs against
   one R2 bucket through a week of daily use — zero lost rows, a
   forced concurrent note edit yields a conflict copy, offline edits
   reconcile on reconnect, and the loop stays quiet on battery/network
   (no busy polling).*
3. **Sharing service (DO + R2).** Worker with magic-link accounts, one
   DO per shared notebook speaking the phase-2 op format over
   hibernating WebSockets, R2 for blobs, payloads still ciphertext
   (per-recipient key wrapping). *Gated on demand, not on roadmap: it
   starts when a real second person asks — the smart-defaults ethos
   applied to infrastructure.* *Gate: a shared notebook edited by two
   accounts converges; revoking a member stops their sync.*
4. **Later, evidence-gated:** CRDT note bodies (§5 tripwire); a
   folder-target transport (same trait — makes Syncthing/iCloud Drive
   users first-class by syncing the *log*, not the database); LAN
   peer sync; `note_usage` counter merge.

## 8. Open questions

- **Key ceremony for sharing** (phase 3): per-recipient wrapping of
  the notebook key (X25519) vs. re-encrypting history on membership
  change — decide when the DO design lands.
- **Archive format versioning**: the `.alchemy` zip should carry a
  schema version and tolerate additive columns — pin the rule in
  phase 1 so old archives import forever.
- **Attachment-sized sources**: PDFs sync as extracted text today;
  does the original file belong in the log (as an R2 blob) once people
  expect the source document itself on the second device?
- **Cadence defaults**: hourly like git probes, or minutes? Start
  hourly + on-mutation debounce, let field use argue it down.
- **note_usage**: keep local forever, or sum counters cross-device so
  the curator sees whole-user behavior? Leaning local until the
  curator demonstrably suffers.
