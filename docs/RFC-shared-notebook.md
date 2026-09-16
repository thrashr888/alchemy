# RFC: A shared notebook — two people, one folder

Status: phases 1–3 built on `cld/shared-notebook` (phase 1 2026-09-14,
phases 2–3 2026-09-16); phase 4 — the two-Apple-ID run on the household
Macs — pending.
Origin: Reminders item "make it so my wife can collab on a notebook over
icloud." Builds on [RFC-okf-live.md](RFC-okf-live.md) §5.6 (shared
folders: iCloud, Dropbox, two Macs) and §5.7 (the Notebooks folder).

## Summary

Two Macs on two Apple IDs edit one notebook, and each sees the other's
sources and notes within a minute, with the other person's name on them.
Nothing new moves over a wire we own: the folder is the transport, as it
already is for one person with two Macs. What is new is the *setup* — a
folder the app can share — and three small rules that make a second
person, rather than a second machine, behave.

## What already works, and the one thing that doesn't

§5.6 built the two-Mac case: per-machine manifests outside the bundle,
per-writer log entries, `human:<account>` by-lines, cloud-stub hydration,
conflict copies as notes, and (since v0.60.0) a pair of files sharing one
sync id held rather than stalling the notebook. All of it is keyed on
*machine*, not *Apple ID*, so a second person's Mac is just another
machine to the reconciler.

The thing that does not work is where the notebooks live. Stage two put
them in the app's own iCloud container
(`~/Library/Mobile Documents/iCloud~com~thrashr888~alchemy/Documents`),
and an app container is private to one Apple ID: Finder offers no
"Share Folder" inside it, and nothing under it can be shared with another
account. Shared folders are a feature of iCloud Drive proper
(`com~apple~CloudDocs`), where a folder shared with another Apple ID
appears in that person's iCloud Drive under "Shared".

So a shared notebook is a bound bundle in a plain iCloud Drive folder —
exactly the stage-one layout — that one person shares from Finder and the
other person binds. The Notebooks folder picker already lets a bundle
live anywhere; the binding verb already binds any folder.

## Design

### 1. Share from Alchemy, not from Finder alone

A notebook's ⋯ menu gains **Share with someone…**. It:

1. Moves the bundle from the container into `iCloud Drive/Alchemy Shared/`
   (a rename, never a delete; `rebind_moved` keeps the binding id and the
   manifest, as the stage-two migration does).
2. Opens Finder's share sheet on that folder
   (`NSSharingService` for `com.apple.share.CloudSharing`, via cider or a
   tiny Swift call) so the owner picks the person and "can make changes".
3. Marks the notebook `shared` in the binding, which is what §3 below
   keys on.

Moving the folder is the honest part: a notebook is either private to
this Apple ID or it isn't, and the folder it sits in is the switch.

### 2. Join on the other Mac

On the second Mac, the shared folder arrives under iCloud Drive → Shared.
The Notebooks folder watch (§5.7, "a second Mac opens what it finds")
watches only the Notebooks folder root; it learns one more root, the
`Shared` folder, and probes what lands there as bundles. A bundle found
there is offered as **Open shared notebook "X"** in the sidebar, the same
banner style as the on-disk offer, and binds on accept. No Apple ID
plumbing: the OS put the folder there because a person accepted the
share.

### 3. Two people, three rules

- **By-lines name the person.** `human:<account>` is the macOS short
  name, which is already a person, not a machine. The sidebar's agent
  by-line treatment ("edited by kim") applies to another account's edits.
  Nothing to build beyond showing it in the reader's title bar for shared
  notebooks.
- **Deletes are proposals across people.** Within one person's Macs a
  deletion record is authoritative. Between two people it is a proposal:
  the other side's reconciler moves the row to the Trash (recoverable,
  visible) rather than removing it, and the log says who. This is the one
  reconciler change, gated on the `shared` mark.
- **Chat stays yours.** Chat history, the ledger, and Studio artifacts are
  local (RFC-okf-live §5.6 non-goals). Notes are the shared surface; a
  generated note becomes shared the moment it is written to the bundle,
  which it is.

### 4. What we don't build

- Presence, cursors, or live co-editing. iCloud is minutes, not
  milliseconds, and §5.4's newest-wins plus conflict copies is the
  merge policy.
- Permissions inside the notebook. iCloud's read-only vs can-make-changes
  is the whole model.
- Non-iCloud shares. Dropbox and Drive shared folders already work the
  same way by pointing the Notebooks folder at them; nothing here
  excludes them, and nothing here adds their share sheets.

## Test

Two data dirs are not enough this time: the container/shared split is
about the OS. The test is two Macs, two Apple IDs (Paul's and his wife's):
share from Mac A, accept on Mac B, open on B, add a source on B, see it on
A with her name, delete it on A, see it in B's Trash with his. Everything
below the OS boundary — the shared root watch, the delete-as-proposal
rule — gets the usual two-data-dir tests.

### Phase 1, as built

- **Offered, not opened.** The Notebooks-folder pass opens what it finds
  on its own (§5.7); a bundle at the root of iCloud Drive is offered in
  the HealthBanner instead — "“X” is in your iCloud Drive. Open · Not now".
  That root is also the user's own space, and a bundle there may be an
  experiment as easily as a share. `okf::shared_offers_in` is the filter
  (one level down, not bound, not the Notebooks folder or inside it, not
  dismissed), read at mount and every five minutes: a share accepted in
  Finder lands with no event this app sees.
- **Ownership is not detected.** macOS marks a shared item through
  Foundation resource keys, not anything `mdls` or an xattr exposes, and
  the offer does not need it: a bundle you can open is a bundle you can
  open, whoever put it there.
- **One open path.** `open_found_folder` is the per-folder half of the
  Notebooks pass, factored out so the offer's Open runs the same
  rebind-or-import decision and the same claims. A dismissed folder is
  remembered by path in `okf/shared-dismissed.json`.
- **Agents see the same offers** through `list_shared_notebooks` and
  `open_shared_notebook`.

### Phase 2, as built (2026-09-16)

- **One verb, not two.** "Share with Someone…" replaces the old "Share
  Folder…", which only revealed a folder. Revealing the container was
  advice nobody could take: Finder has no Share inside an app container.
  The verb sits after Export Notebook… in both the ⋯ menus a notebook
  has (`notebookRowItems` on Home, `notebookVerbs` in the workspace —
  separate lists, so it was added to both), and wears no SF Symbol:
  DESIGN.md's menu rule is symbols per group, all or none, and that
  group of plain verbs has none.
- **A move, both ways.** `std::fs::rename` into
  `iCloud Drive/Alchemy Shared/`, `free_name`'s `-2` on a collision, and
  `rebind_moved` to repoint the binding — the same three the container
  migration uses, so the binding id and its manifest survive and no hash
  the reconciler holds is thrown away. Nothing is copied and nothing is
  deleted, which is what keeps it reversible: moving the folder back is
  the same move the other way, through the rebind the Notebooks folder
  already does. A notebook that was never on disk is bound into the
  shared folder and seeded through the ordinary bind instead.
- **A second root for the offer.** Phase 1 looked one level under iCloud
  Drive, which is where a share *sent to you* lands. What this Mac
  shares sits one level further down, in `Alchemy Shared/`, so the offer
  pass reads that folder too — otherwise the other Mac of the same
  person would never be offered what this one shared.
- **The mark.** `OkfBinding.shared`, over IPC as `shared`, is what §3
  keys on. The chip beside the notebook's name reads "Shared" instead of
  "On disk", with the folder and the promise in its tooltip: same
  hairline chip, no new color — being shared is not a warning.
- **Agents** get the move and the mark through `share_notebook`, and not
  the sheet; the tool description says to tell the user which Finder menu
  to use.

### Phase 3, as built (2026-09-16)

- **Deletion records carry a by-line.** `sync/deletions/<uuid>.by` holds
  one actor line beside the record — a file, not a field. The record is
  `deny_unknown_fields`, so a new key inside it would make every Alchemy
  already installed call the record invalid, and an invalid record stops
  that notebook's whole pass on the other person's Mac. Older clients
  skip any name in that folder that is not `<uuid>.json` without looking.
  A record with no by-line is read as ours, which is what every record
  written before this one is.
- **The mark is a manifest field, not a row status.** A source has no
  recoverable status to borrow — `ready | error | placeholder`, none of
  them a trash — and a new store column would brick older binaries on a
  shared store. So the manifest (machine-local, per binding, already the
  place the reconciler keeps its mind) grows
  `proposedDeletions: entity id → by-line`. While an entry is there the
  row stays, the writer keeps the claim but does not put the file back,
  and `vanish_verdict` keeps its hands off the claim entirely. The log
  line names the person.
- **Restore mints a new sync identity.** Answering Restore drops the
  claim and writes the notebook again, which gives the file a fresh
  `sync_id` — a tombstone is immutable and portable by design, so
  re-publishing under the old id would hand the other side a file their
  own record says is dead, and it would vanish again on their next pass.
  Remove applies their record here, the same removal an unshared binding
  would have done without asking.
- **Where it shows.** `DeletionProposalMark` in the sources list and the
  Studio note list: "Deleted by kim · Restore · Remove", a hairline chip
  and two plain buttons on the row's own metadata line. No confirmation
  on Remove (DESIGN.md §9): the delete already happened on their Mac,
  this only agrees with it, and Restore is the way back. Agents see the
  same questions through `deletion_proposals` / `resolve_deletion_proposal`.
- **Gated on `shared`, and only on `shared`.** An unshared binding is
  byte-for-byte the behavior that shipped: one person's two Macs are two
  machines, and a delete made on either is theirs and stands. Unsharing
  clears the open questions on the next pass.

## Open questions

1. Should sharing move the folder, or copy it and keep the private one?
   Proposed: move. Two copies of one notebook is the failure mode §5.7
   spent a release cleaning up. **Settled 2026-09-16: move.**
2. The share sheet: cider, a Swift sidecar call, or "open the folder in
   Finder and tell the user which menu"? Proposed: try `NSSharingService`
   from the fm sidecar first; fall back to revealing the folder with a
   one-line instruction. **Settled 2026-09-16: the sidecar, exactly that
   way.** `alchemy-fm --share <folder>` performs
   `NSSharingService(named: .cloudSharing)` and prints
   `{"type":"presented"}` once the sheet is up, then outlives the call —
   the person is in front of it choosing who to invite — and exits on
   completion or after ten minutes. Anything else (no sidecar, an older
   one that doesn't know the verb, a Mac where CloudSharing refuses, no
   answer inside twenty seconds) is not an error: the folder is revealed
   in Finder with "In Finder, click Share → Collaborate and add them."
   Not cider: this is Alchemy's own window server session, and the
   sidecar is already the AppKit we ship.
3. Does the deletion-as-proposal rule need a setting, or is it always on
   for shared notebooks? Proposed: always on. **Settled 2026-09-16:
   always on**, and it is the `shared` mark on the binding that switches
   it — no preference. Sharing is the setting.

## Phasing

1. Shared root watch + "Open shared notebook" offer (join side). **Done
   2026-09-14.**
2. Share with someone… (move + share sheet + `shared` mark). **Done
   2026-09-16.**
3. Deletion as proposal for shared notebooks. **Done 2026-09-16.**
4. Two-Apple-ID test on the household Macs; then the docs page. The one
   thing two data dirs cannot stand in for: whether macOS raises the
   CloudSharing sheet for a process with no window, and whether the
   folder lands where phase 1 looks for it on the other Apple ID.
