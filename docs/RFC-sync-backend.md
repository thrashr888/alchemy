# RFC: sync backend — own the engine, rent dumb storage

Alchemy is one Mac's app. A user with a laptop and a desktop today moves
notebooks by not moving them — re-importing sources by hand and losing
the chat and notes that made the notebook worth keeping. And a notebook
built at work is worth more when coworkers can contribute to it, so
user-to-user sharing is a design driver here, not an epilogue. The
question this RFC answers: how notebooks sync across a user's devices
now and between users next — comparing (a) iCloud/CloudKit, (b) plain
object storage (S3-compatible), (c) Cloudflare Workers/Durable Objects
+ R2, (d) the do-nothing control of dropping the data dir in iCloud
Drive, and (e) P2P/federated designs where the notebook's originator
is the host.

The framing insight: **the app must stay fully functional offline, so
the sync engine — queue, merge, conflict rules — is client-side Rust in
every option.** The candidates differ only in what sits on the other
end of the wire. So choose the dumbest transport that survives, put it
behind a trait, and upgrade transports — not engines — as sharing
becomes real. That insight survives this revision. What changed is who
runs the far end: bring-your-own-bucket fails the adoption test, so
the default becomes managed storage I operate — still dumb, still
holding only ciphertext — with the relay code public so anyone can
run theirs, which is where the P2P instinct in (e) lands: many
interchangeable dumb hosts, one of them mine.

## 1. Problem & goals

- **Multi-device, single user** ships first: same person, two Macs,
  one set of notebooks. **User-to-user sharing is a committed
  destination, not a hedge** — work notebooks want coworker
  contributions — so v1 pays sharing's structural costs up front
  (device-key identity, per-notebook keys, signed ops) and defers
  only its service half.
- **Offline is not a mode.** Sync is a background best-effort loop in
  the `sweep_due` style folder and git sources already use; no command
  ever blocks on the network, and a Mac that never syncs is a fully
  working Alchemy forever.
- **Privacy by default.** Source content never transits any host in
  plaintext — payloads are encrypted on-device before upload, and the
  sync host, including the one I run, stores ciphertext it cannot
  read. API keys and provider config never sync at
  all, in any phase; they are machine secrets (and machine-shaped:
  RAM-tiered model picks from RFC-inference-providers don't belong on
  another machine anyway).
- **Zero-setup adoption.** Sync that begins "paste your S3
  credentials" selects for users who have S3 credentials. The bar is
  stricter than "a sign-in works": on a second Mac signed into the
  same Apple ID, the app finds the identity key in iCloud Keychain
  and just syncs — no form, no passphrase, nothing burdensome or
  awkward. Bring-your-own anything is a power-user setting, never
  the pitch.
- **Solo-dev weight, renegotiated.** Operating a service is now
  accepted — that's what the adoption goal costs — but only in
  serverless form: a Worker and a bucket, no VMs, no database
  servers, tens of dollars a month at 10k users, nothing that pages
  at 3am. And vendor-death insurance is a requirement: if the
  service dies, the archive and the serverless transports leave
  every user whole — and because this is a public repo, the relay
  ships in it, so self-hosting is a supported deployment (one
  `wrangler deploy`), not a fork.
- **The notebook is the sync unit**, matching how every table is
  already keyed (`notebook_id`) and how sharing will eventually scope.
- **Every phase ships alone** and is useful without the next one.

## 2. Non-goals

- **Realtime co-editing.** No character-level merge, no presence
  cursors. §5 names the upgrade path; nothing in v1 blocks it.
- **A web viewer or publishing.** Different feature, different RFC.
- **A social layer.** The managed relay (phase 3) has accounts, but
  an account is a keypair, optionally labeled with an email for
  sharing and quotas — no passwords, no profiles, no discovery, no
  contact graph.
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

## 4. The options

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
  heads/<device-id>.json                    # cursor: latest hlc per notebook
  notebooks/<nb-id>/log/<hlc>.<device>.age  # op batches, append-only
  notebooks/<nb-id>/snapshot/<hlc>.age      # periodic compaction
```

**Discovery is a `GetObject` on the head files, never a `ListObjects`
sweep.** A device publishes its head cursor on push; readers fetch the
handful of head files they know about and pull only the named batches.
This looks like a detail and is actually the cost model (4c): listing
is billed as a mutation, so a poll loop built on `ListObjects` costs
more than every other line item combined.

- **Auth**: the fork this revision turns on. The previous draft made
  v1 **bring-your-own bucket** — R2, S3, MinIO, anything S3-shaped —
  credentials pasted once per device into the Keychain, Zotero's
  WebDAV move ([storage *you*
  configure](https://www.zotero.org/support/sync)). Rejected as the
  mainline: it severely limits adoption — the pitch can't open with a
  bucket. Note Zotero's full shape, though: WebDAV is its *escape
  hatch*, while the default nearly everyone takes is Zotero-hosted
  storage. Same here: the mainline gets a managed end — a token
  service in front of my R2, which is (c)'s first half — and BYO
  survives as the hidden setting for people who want to hold their
  own bytes (the self-host story (e) fans actually want).
- **Offline/conflict**: append-only logs keyed by device id never
  contend; compaction claims use conditional writes, which [S3](https://aws.amazon.com/about-aws/whats-new/2024/08/amazon-s3-conditional-writes/)
  and [R2](https://developers.cloudflare.com/r2/api/s3/extensions/)
  both support. LWW semantics live entirely in the client (§5).
- **Privacy**: best available. Payloads are `age`-encrypted on-device
  under per-notebook keys wrapped to device identities (§5), the
  posture Obsidian ships: [E2E, vendor never sees plaintext or
  keys](https://help.obsidian.md/sync/security). Whoever holds the
  bucket — me included — holds ciphertext.
- **Cost** (1/100/10k): $0 to the developer at every scale. Per user,
  [R2's free tier](https://developers.cloudflare.com/r2/pricing/)
  (10 GB, 1M class-A + 10M class-B ops/mo, zero egress) swallows a
  text corpus outright; S3 runs pennies.
- **Weight**: one Rust module. The `object_store` crate is *already in
  Alchemy's dependency graph* (Lance is built on it — Cargo.lock has
  0.13.2), `age` is a small pure-Rust add, and the polling loop is the
  existing `sweep_due` throttle pattern with a push debounce. A
  Settings pane, not a service.

**Verdict: the transport mechanics for everything that follows — no
longer the auth story.** Log format, conditional writes, encryption,
and the `object_store` client ship verbatim; the revision reassigns
who holds the bucket (4c) and demotes BYO creds to a power-user
setting.

### 4c. Cloudflare: an R2 relay now, Durable Objects at sharing

Two separable halves. The first — **a doorman Worker in front of R2**
— is the managed end of (b): a device-key registry, per-user quotas,
and short-lived S3 credentials scoped to the user's prefix via [R2
temporary
credentials](https://developers.cloudflare.com/r2/api/tokens/), so the
client runs the identical `object_store` code path as BYO. No Durable
Objects, no WebSockets, no per-notebook server state — a doorman, not
a database. The Worker lives in this public repo and the app takes
the relay URL as configuration with mine as the default — so "one
multi-tenant sync source I host, and anyone can host their own" is
not a roadmap item, it's a property of the first release.

- **Auth**: must be built, but in its *thin* form — and thinner than
  passwords ever get. A request is authorized by a device-key
  signature (§5); an account is auto-created on first sync and *is*
  the pubkey set. No password table exists on any relay, mine or
  self-hosted. An emailed magic link attaches an address when the
  user wants one — the handle sharing needs, and the sybil brake
  free quotas need; Sign in with Apple or passkeys can layer on
  later as polish, not plumbing (§8). The revision accepts the ops
  cost knowingly: this is the first option where Alchemy operates a
  service with users, bought because zero-setup adoption is worth an
  operated service.
- **Offline/conflict**: unchanged from (b) — the relay is dumb
  storage; every merge decision stays in the client engine.
- **Privacy**: unchanged from (b) — I relay and store ciphertext I
  cannot read. Quotas and rate limits are the entire abuse surface,
  because nothing stored is legible enough to moderate.
- **Cost** (1/100/10k): ~$0 / ~$5 / ~$15–25 per month.
  [R2](https://developers.cloudflare.com/r2/pricing/) runs
  $0.015/GB-mo with zero egress — 10k users × ~50 MB of ciphertext ≈
  500 GB ≈ $7.50 — plus [$5 Workers
  Paid](https://developers.cloudflare.com/workers/platform/pricing/)
  and request overage. Cheap enough to give away indefinitely; if a
  storage-heavy minority ever matters, Obsidian's $4–8/user/mo shows
  the market rate for exactly this service.
- **Weight**: one small Worker (auth, token minting, quotas) and one
  deploy pipeline — bounded, and every line is shared with the
  eventual sharing service.

The second half — **one DO per shared notebook**: a single-threaded
coordinator with SQLite state that orders writes, assigns revisions,
and fans out over hibernating WebSockets
([requests $0.15/M past 1M, hibernated sockets bill no duration,
SQLite storage billing live since Jan 2026](https://developers.cloudflare.com/durable-objects/platform/pricing/))
— stays exactly as previously drafted, and stays deferred: server
ordering makes multi-writer merge *easier*, but offline devices still
queue and reconcile, so it adds to (b)'s engine rather than replacing
it.

**Verdict: the doorman is the v1 backend; the DO ships with sharing.**
The previous draft called all of (c) "bought too early if bought now."
That stays true of the coordinator and stops being true of the
doorman — adoption promoted it from someday-service to v1
infrastructure, in the smallest form that can exist.

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

**Verdict: rejected as live sync — but the Obsidian reading of this
option is real, and it gets promoted.** Obsidian survives on iCloud
and Dropbox not because file syncers are safe but because its
canonical data is a folder of small independent files. Alchemy's live
database can never be that; the §5 op log already is — write-once
batch files, one writer per filename, no cross-file invariants at
rest — so the same-file conflicts syncers bungle are impossible by
construction. Pointing the *log* (not the database) at a folder in
iCloud Drive/Dropbox/Syncthing yields real multi-device sync with no
account, no service, and no vendor; the syncer's quirks (dataless
eviction delaying a pull, deletion lag around compaction) degrade
freshness, never integrity. So (d) splits: database-in-folder stays
rejected; **log-in-folder becomes the second first-class transport**
— the no-account path, and the insurance if the managed relay ever
dies. And the control still teaches: everything above must beat "zip
it and AirDrop it," so the archive ships first regardless.

### 4e. P2P and federated — the originator as host

The tempting third way around both managed cost and BYO friction:
rent no storage at all. Notebooks live where they originate and flow
member-to-member — live connections, or a federated store where the
originating device is owner/host/server. The research frontier here
is real: [FedEDB (TKDE
2024)](https://www.computer.org/csdl/journal/tk/2024/11/10352972/1SOBTQ1BQY0)
builds a federated *encrypted* data store on consortium blockchains —
multi-owner searchable encryption so untrusted nodes can answer
queries over ciphertext, zero-knowledge proofs so results are
verifiably complete. Instructive, and instructively wrong for
Alchemy: all that machinery exists so an untrusted *host* can compute
on data it must not read. Alchemy has no such host — every member
device is a trusted endpoint holding plaintext, search runs against a
local index (§3 made vectors per-device on purpose), and the wire
only ever carries ciphertext blobs. A dumb host dissolves, for free,
the problem FedEDB spends consensus, SSE, and ZK proofs solving.

Sort the P2P family by where the always-on node hides:

- **Live-query federation** — members query the originator's index
  over a live connection — makes a notebook exactly as available as
  the owner's laptop lid. Offline symmetry is the point of this app:
  members must read, chat, and write while the originator sleeps.
  And §3's embedder reality cuts deeper: indexes don't transfer
  across devices, so a query either runs on the owner's hardware
  (availability) or against a local re-embed of content the member
  already holds — which is replication wearing a costume.
- **Store-and-forward between devices** — the honest P2P shape:
  nothing is read live; peers push and pull op batches when they
  meet (Syncthing-shaped; [iroh](https://www.iroh.computer/) is the
  Rust-native kit). The engine doesn't care — the op log was built
  for exactly this. What fails is the *meeting*: two Macs whose lids
  are rarely open at the same time exchange nothing, and WAN
  traversal needs rendezvous/relay infrastructure someone operates
  anyway. Every mature P2P system quietly grows an always-on
  store-and-forward node; ours is just declared up front.
- **The hub that admits it's a hub** — one multi-tenant sync source,
  self-hostable by anyone: not a P2P consolation prize, it's the
  design (4c). Because batches are signed and encrypted client-side,
  a relay is a commodity — the app treats the relay URL as
  configuration, my deployment as the default, and a work team that
  wants custody runs the same public-repo Worker on their own
  Cloudflare account. Federation here costs one URL field, not a
  protocol: the notebook's owner picks where it lives, which is
  "originator as owner/host" in the only form that survives a
  closed laptop.

What the decentralized apps worth studying actually teach:

- **Keybase** ([docs](https://book.keybase.io/docs)): identity is a
  set of per-device keys; a team is a symmetric key wrapped to each
  member's devices; the server is an untrusted ciphertext store.
  That is exactly the sharing model §5 adopts. Keybase also teaches
  the exit lesson (acquired, then frozen): a sync service must leave
  its users whole when it dies — which is what the archive, the
  folder transport, and BYO are for.
- **Bluesky/atproto**
  ([overview](https://atproto.com/guides/overview)): the PDS makes
  "originator as host" real, with portable identity so hosts stay
  replaceable — but the protocol is engineered for *public
  broadcast* data; records are signed, not secret, and a private E2E
  notebook is exactly what it doesn't do. The transferable idea is
  self-certifying data: §5 signs op batches with device keys, so a
  batch is trustworthy regardless of which host relayed it — the
  property that makes relay, folder, and BYO interchangeable
  transports instead of three separate trust decisions.

**Verdict: adopted in hub form, rejected in laptop form.** Keep
originator-as-authority (membership, keys, and home relay belong to
the notebook's owner, not to my server), device-key identity, signed
batches, replaceable self-hostable relays. Reject only the version
where members' laptops must find each other awake. LAN peer sync
(iroh, same op format) stays on the later list as an accelerator —
sugar, not architecture.

| | (a) CloudKit | (b) BYO bucket | (c) relay → +DO | (d) log in folder | (e) P2P/fed |
|---|---|---|---|---|---|
| Auth | OS session | user's creds | device key via iCloud Keychain | OS/Dropbox session | key exchange, DIY |
| Engine | Swift sidecar | Rust, in-app | Rust, in-app | Rust, in-app | Rust + rendezvous |
| Privacy | Apple's keys | E2E | E2E, ciphertext relay | E2E | E2E |
| $/mo (dev) at 1/100/10k | 0/0/0 | 0/0/0 | ~0/~5/~20 | 0/0/0 | relays, eventually |
| User setup | none | owns a bucket | none-ish (email optional) | none (has iCloud) | none, until NAT |
| Solo-dev weight | high, alien | low | medium, bounded | low | high, ongoing |
| Sharing path | Apple-only | crude (creds) | the real one | shared folder, crude | unbuilt dream |

## 5. Conflict & trust model — LWW rows, conflict copies, device keys

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
- **Identity is a keypair; access is key wrapping; auth is therefore
  distributed.** Every device holds an Ed25519 keypair in the
  Keychain and signs each op batch it uploads — batches are
  self-certifying whatever relayed them (4e's lesson). An account
  *is* a set of pubkeys, optionally labeled with an email; a relay —
  mine or self-hosted — only verifies signatures, holds no
  passwords, and any relay accepts the same identity, which makes
  auth as portable as the data. The identity key is stored as a
  synchronizable Keychain item, so iCloud Keychain (or 1Password)
  carries it to the user's other Macs and a second device enrolls
  itself with zero ceremony; explicit device-to-device approval is
  the fallback when Keychain sync is off. Each notebook gets a
  random symmetric content key, wrapped (X25519) to the account's
  device keys; sharing (phase 4) is the identical wrap extended to a
  member's devices — Keybase's team shape. Revocation rotates the
  notebook key and re-wraps to survivors. Lose every device *and*
  the Keychain copy and no relay can help — it holds ciphertext
  (§8).
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
  this app. The named upgrade: if shared notebooks (phase 4) grow
  simultaneous note editing in practice, note *bodies* — and only note
  bodies — move to a CRDT text type behind the same op format.

## 6. Recommendation

Build **one sync engine in Rust**, behind a `Transport` trait, so the
same engine runs over a local folder, a relay I host, or the user's
own bucket. Ship it in stages that each stand alone. The whole point,
in one line: **turn Alchemy on and your notebooks follow you to every
Mac with nothing to set up; invite a coworker to one when you're
ready.**

### What users get (in shipping order)

- **Notebook archive** — export a notebook to a `.alchemy` file,
  import it anywhere. Backup and AirDrop-sharing in one. No account,
  no network.
- **Folder sync** — point Alchemy at an iCloud Drive or Dropbox
  folder; your Macs converge through it. No account, no service,
  using storage the user already pays for.
- **Managed sync** — one switch. Notebooks sync through a relay I
  host, free, encrypted so I can't read them.
- **Shared notebooks** — invite a coworker; you both edit and changes
  merge live.
- **Self-hosted relay** — the relay is in this repo; a team runs its
  own with one deploy and keeps full custody.
- **Bring-your-own bucket** — power users point sync at their own
  S3/R2. A hidden setting, never the pitch.

### The flows, plainly

- **Enable.** Settings → Sync → one switch, "Sync: on." Done. Behind
  it, the app creates a device keypair in the Keychain and starts
  syncing. No form, no password, no signup screen.
- **Auth.** The account *is* the device keys — there is no password
  anywhere.
  - *Second Mac, same Apple ID:* iCloud Keychain carries the identity
    key over, the app recognizes itself, and it syncs with zero
    setup.
  - *Keychain sync off:* approve the new device from an existing one,
    or type a recovery phrase.
  - *Email is optional:* attach one via a magic link — needed only so
    people can share notebooks with you and so the free tier can tell
    accounts apart. Never a login step.
- **Sync.** Every edit becomes a small signed, encrypted batch. A
  quiet background loop pushes yours and pulls others' on the cadence
  the app already uses for folders and git, plus a manual "Sync Now."
  It never blocks: offline, the app is fully itself and catches up on
  reconnect. A new Mac rebuilds its own search index locally, so
  vectors and clones never cross the wire.
- **Share.** Open a notebook, invite a coworker. The notebook's key is
  wrapped to their device keys, and a small coordinator keeps both
  sides converged in real time. The notebook lives on its owner's
  relay — mine or the team's own — so sharing at work never means
  handing your research to me. Remove someone and their access is
  rotated out.

### Why this shape

- **CloudKit** is free but locks the engine into Swift on an
  Apple-only transport I can't test locally or extend to non-Apple
  people. Rejected.
- **Bring-your-own bucket** was the previous pick; "paste your S3
  credentials" is a wall most users won't climb. Demoted to a
  power-user toggle.
- **Pure P2P** puts the always-on node on a laptop that closes. The
  hosted relay is the honest version of the same instinct — and
  because it's self-hostable and holds only ciphertext, it keeps
  P2P's real win (own your host) without the physics problem.
- **File-syncing the live database** corrupts it; file-syncing the
  *log* is safe, which turns iCloud Drive and Dropbox into a
  legitimate free transport.
- **The managed relay** is the one real cost — an operated service —
  and I'm paying it on purpose, in the smallest form that exists: a
  doorman in front of a bucket, ciphertext only, tens of dollars a
  month. "It just works on my second Mac" is worth that.

Every stage is a strict prefix of the next. The archive stands alone;
the folder transport proves the engine; the relay swaps the folder
for a bucket I host; sharing adds a coordinator over the identical op
format. Nothing here is a detour.

## 7. Phases

1. **Notebook archive (export/import).** A `.alchemy` zip: sources
   (rows + content), notes, messages, notebook row, embed overrides —
   no vectors, no clones. Import runs the normal ingest pipeline to
   re-chunk/re-embed. Doubles as backup and as sharing v0 (AirDrop the
   file). *Gate: export on Mac A, import on Mac B, chat with citations
   works after local re-embed; a re-import of the same archive dedups
   instead of duplicating.* Shippable alone — and worth shipping even
   if every later phase dies.
2. **The engine, proven over the folder transport.** Op emission on
   every mutation path, HLC + `rev` columns, tombstones, LWW apply,
   note conflict copies, device keys + batch signing; `Transport`
   trait whose first implementation is a plain directory — the test
   double that is also a product: pointed at iCloud Drive, Dropbox,
   or Syncthing it is real multi-device sync with no account and no
   service (the 4d promotion). `age` encryption with Keychain-held
   notebook keys; push debounced behind the existing sweep cadence,
   manual Sync Now; per-notebook opt-out, default all on
   (smart-defaults rule). *Gate: two real Macs against one iCloud
   Drive folder through a week of daily use — zero lost rows, a
   forced concurrent note edit yields a conflict copy, offline edits
   reconcile on reconnect, and the loop stays quiet on
   battery/network (no busy polling).*
3. **The managed relay — sync becomes a product.** The doorman
   Worker, open source in this repo: device-key auth (account
   auto-created on the first signed request), per-user quotas,
   scoped R2 temporary credentials, optional magic-link email
   attach; the client reuses (b)'s `object_store` transport pointed
   at the configured relay URL, mine by default. Settings → Sync
   collapses to one "Sync: on" switch; relay URL, folder, and
   BYO-bucket live under Advanced. *Gate: a second Mac on the same
   Apple ID syncs with zero configuration — no form, no passphrase —
   via the iCloud Keychain-carried identity; a stranger can
   `wrangler deploy` the relay from the public repo and point the
   app at their URL with one field; the operator view shows sizes
   and ciphertext, nothing legible; hitting quota degrades to a
   clear error, never data loss.*
4. **Sharing (DO + R2).** One DO per shared notebook ordering the
   phase-2 op format over hibernating WebSockets; membership is the
   notebook key wrapped to a member's device keys (§5); revocation
   rotates and re-wraps. A shared notebook lives on its owner's
   relay — members sync it from wherever the owner hosts, my
   deployment or their own — so a work team keeps custody without
   losing the product. Multi-writer editing stays LWW + conflict
   copies until the §5 tripwire trips. *Gate: a notebook shared
   between two accounts converges under concurrent edits from both;
   the same flow works against a self-hosted relay; revoking a
   member stops their sync and rotates the key.*
5. **Later, evidence-gated:** CRDT note bodies (§5 tripwire); LAN
   peer sync (iroh) as a same-op-format accelerator; `note_usage`
   counter merge; original-file blobs for attachment-sized sources
   (§8).

## 8. Open questions

- **Auth polish** (phase 3): device-key auth with optional magic
  link is decided. Open: is quota-per-pubkey enough sybil
  resistance for free storage, or does the free tier require a
  verified email from day one? And do passkeys or Sign in with
  Apple ever earn their ceremony when no password exists to replace
  — a real UX win, or just entitlement pain (§4a's complaint)?
- **Token mechanics** (phase 3): R2 temporary credentials scoped to a
  user prefix vs. proxying every byte through the Worker — temp creds
  keep the pure `object_store` path; the proxy makes quota
  enforcement exact. Decide with a load test; egress is free either
  way.
- **Free quota and a paid tier**: what's free (1 GB of ciphertext?),
  and at what point does a storage-heavy minority justify an
  Obsidian-priced tier ($4–8/mo)? No tier before real cost data.
- **Recovery floor** (phase 2): iCloud Keychain carries the identity
  key and any surviving device can enroll a new one; for the user
  with Keychain sync off and one dead Mac, is the answer a printable
  recovery phrase or an honest "local data survives; the relay copy
  re-uploads"? Escrow stays off the table — I hold ciphertext,
  period.
- **Self-host surface** (phase 3): the public-repo Worker invites
  self-hosting; how much do those deployments get — a versioned wire
  protocol doc, migration notes on breaking changes, nothing else?
  Decide before the first outside relay exists, not after.
- **Key ceremony for sharing** (phase 4): rotate-and-rewrap on every
  membership change, or lazy rotation on revocation only — decide
  when the DO design lands.
- **Folder-transport semantics on laggy syncers** (phase 2): the
  90-day tombstone horizon vs. a device that pulls a compacted log
  later still; and how iCloud's dataless-file eviction interacts
  with pull cadence.
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
