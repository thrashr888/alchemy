# Testing two laptops on one Mac

Run the isolated sync regression suite:

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib okf -- --test-threads=1
```

`src-tauri/src/okf/sync_tests.rs` constructs two real `AppState`s with separate
Lance databases, settings paths, binding records and manifests in temporary
directories. It calls the production writer and reconciler. It does not open
the desktop app, read personal notebooks, use iCloud, or require a model.
The source fixture points inference at an unavailable loopback endpoint;
retrieval quality is outside this test, but persisted source content is real.

The shared-directory cases exercise overlapping reconciliation, unread edits,
deletion followed by a stale writer, the deletion grace period, and repeated
bidirectional exchanges. A second arrangement uses separate bundle directories
and explicitly delivers files out of order and more than once, then reopens the
database and manifest to verify restart behavior. Tests also cover source edits,
initial import filename ownership, and corrupt or unwritable sync records.

Recovery tests interrupt the actual import after inserting a note or source,
and interrupt exports before emission, after emission, and between rename and
rewrite. Reopening the database must keep the reserved identity and later user
edits. Source tags and origin-device metadata must survive insertion failures.
Deletion tests replay observed old bytes after restart, after another replica
accepts a deletion, and after a new item reuses the deleted item's filename.

`sync_index_tests.rs` runs a controlled loopback embedding server. It holds
requests open while ordinary note imports, edits, and subsequent sync passes
finish, then verifies that a stale response cannot replace the latest index.
`note_index.rs` also tests unavailable inference, deletion during indexing,
queue coalescing, and durable retry markers across database reopen.

Four database-backed regressions were reproduced before the fixes: overlapping
reconcilers imported five notes twice; an applied edit was reported again on the
next pass; a stale writer recreated a remotely deleted file; and an unrelated
write acknowledged an unread file's clock, hiding the remote edit.

The writer and reconciler now serialize per local notebook, persist successful
claims, acknowledge imported content, and retain the local representation of an
imported file so machine-specific IDs and timestamps do not trigger an export.
The writer reads pending remote changes before gathering local content and
does not recreate missing claimed files. Initial binding preserves uniquely
matched files' original paths; ambiguous matches stop binding without changing
those files. Concept and manifest writes use atomic replacement.

Before inserting an incoming row, the reconciler checkpoints its reserved ID.
Before emitting a new file or moving one, the writer checkpoints its path and
identity. Recovery completes these claims before reading incoming files. A
reserved destination containing unrecognized bytes stops recovery and preserves
both the local row and the file: another device may have created that filename
before this export emitted anything. A
successful manifest save also leaves an initialization marker: losing an
established manifest stops sync instead of treating the notebook as new.
Observed file hashes stay in local deletion history, including retired rename
paths. Replayed unclaimed files remain on disk for inspection; they do not
recreate deleted database rows. A replay over a newer item at the same path
leaves that row intact and schedules restoration of its current file.

Note rows persist before indexing. One worker per database coalesces queued
note IDs and bounds each embedding request to 30 seconds. Publication checks
the current note under a brief mutation lock, so stale or deleted versions
cannot be indexed. Failed embedding retains prior chunks and records a retry
for the next launch or edit.

This is deterministic coverage of Alchemy's sync logic, not a simulation of
Apple's complete FileProvider transport. Existing placeholder tests cover
eviction guards, but real iCloud hydration, account state, conflict copies and
two-machine delivery timing still need a transport check. Local deletion history
recognizes exact versions this installation observed; it cannot recognize an
unseen revision or inform a device that never saw the deletion. Portable
deletion identity and transport validation remain in `alchemy-release-alv`.
Atomic binding-record recovery remains in `alchemy-release-2c3`; missing
established manifests currently require restoration, rather than automatic
reconstruction. Existing duplicate notebooks are not automatically merged or
deleted by these tests.
