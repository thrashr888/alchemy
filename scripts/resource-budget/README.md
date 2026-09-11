# Resource budget checks

Run `pnpm dev --host 127.0.0.1 --port 8795`, then
`node scripts/resource-budget/check.mjs`. This uses the production reader,
note store, export selection, and shader components with an in-memory IPC
fixture. No real notebook or model is used by the browser fixture.

Coverage includes late note reads, preserving unsaved text across a metadata
refresh, saving, table/prose export selection, stripping generated bodies from
the collection, refusing deletion when an undo snapshot fails, repeated
undo/redo with new restored IDs, and stopping/resuming visible shaders.

For every GLSL mode, run `python3 scripts/shader-harness.py --serve` and
`node scripts/resource-budget/shaders.mjs`. The latter captures the contact
sheet and checks that animation changes the rendered pixels.

For a native WebKit check, use a temporary Tauri config with a distinct app
identifier and window URL pointing at this fixture. Native Tauri internals
are left intact; only the fixture's note-read API is substituted. The native
fixture posts its report to an optional HTTP collector on 127.0.0.1:8796;
allow that address in the temporary dev CSP. Do not change the shipping CSP.
A Tauri backend still runs its normal startup work against the separate app
data directory, so stop the test app and its helpers after the check.

## Observed on September 10, 2026

| Check | Before | After | What the number represents |
| --- | ---: | ---: | --- |
| 12 generated notes | 9,817,869 bytes | 1,569 bytes | JSON collection payload, from a real temporary LanceDB; bodies and prompts remain intact in individual reads |
| 2400 × 1600 web-image fixture | 7,798,885 bytes | 165,110 bytes | Original PNG vs native `sips` thumbnail PNG |
| Same image, decoded size | 15,360,000 bytes | 614,400 bytes | Dimension-derived RGBA estimate; 480 × 320 output, alpha retained |
| Two shaders, native WKWebView | 58 draws/second visible | 0 draws/second offscreen | Production components; scrolling back resumed drawing |
| Two shaders, Chromium | 60 draws/second visible | 0 draws/second offscreen | Production components; unmount stopped drawing |
| 100 change notifications in one burst | 100 reads without coalescing | 1 read | Fake-clock test; events during a read produce one additional trailing read |
| Graph cache retention | No eviction | 4 graphs / 16 layouts | Also limited to 4 MiB / 2 MiB estimated cache weight; active view is separate |

These are workload measurements and cache policies, not total application
footprint measurements. They do not establish a 50% reduction in Activity
Monitor memory. Shader comparisons are visible versus offscreen after the
change, not a before/after CPU percentage benchmark.

Native fixture result: note rendered, two canvases compiled, visible drawing
58, offscreen drawing 0, resumed true. Browser result: all scenarios passed.
Local gates: frontend build; 360 frontend tests; 10 CLI tests; 762 Rust tests
passed (46 ignored); Rust formatting and Clippy; production CSP scan.

Reproduce the data-path measurements:

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib note_summaries_exclude -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --lib large_transparent_image -- --nocapture
```

The thumbnail measurement is macOS-only. Neither measurement contacts an AI
provider or modifies the user's database.
