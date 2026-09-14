# RFC: A shared notebook — two people, one folder

Status: phase 1 built on `cld/shared-notebook` (2026-09-14); phases 2–4 pending.
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

## Open questions

1. Should sharing move the folder, or copy it and keep the private one?
   Proposed: move. Two copies of one notebook is the failure mode §5.7
   spent a release cleaning up.
2. The share sheet: cider, a Swift sidecar call, or "open the folder in
   Finder and tell the user which menu"? Proposed: try `NSSharingService`
   from the fm sidecar first; fall back to revealing the folder with a
   one-line instruction.
3. Does the deletion-as-proposal rule need a setting, or is it always on
   for shared notebooks? Proposed: always on.

## Phasing

1. Shared root watch + "Open shared notebook" offer (join side).
2. Share with someone… (move + share sheet + `shared` mark).
3. Deletion as proposal for shared notebooks.
4. Two-Apple-ID test on the household Macs; then the docs page.
