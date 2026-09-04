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

This is deterministic coverage of Alchemy's sync logic, not a simulation of
Apple's complete FileProvider transport. Existing placeholder tests cover
eviction guards, but real iCloud hydration, account state, conflict copies and
two-machine delivery timing still need a transport check. Crash recovery between
a database insert and its manifest checkpoint, missing-manifest recovery, and
very old deleted-file replay are tracked in `alchemy-release-plr`. Existing
duplicate notebooks are not automatically merged or deleted by these tests.
