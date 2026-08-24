//! Performance budgets (docs/RFC-professional-grade.md Pillar 2).
//!
//! Named budgets asserted against the seeded fixture library (`fixtures.rs`),
//! so the next scan storm fails a test instead of a user.
//!
//! Every threshold below was set from a measurement on this machine, recorded
//! in a comment beside it, with roughly 2x headroom — these are regression
//! tripwires, not targets, and a run on a busy laptop must not turn red.
//!
//! Two things keep that promise. Each test is `#[ignore]`d out of the default
//! run and executed by its own CI step with `--test-threads=1`: these are
//! wall-clock measurements, and sharing a machine with 390 parallel tests
//! made them fail on load rather than on regression. And each budget asserts
//! on the *fastest* sample rather than p95 — contention can only make a
//! sample slower, so the minimum is the statistic load cannot fake, while a
//! real regression still raises the floor. A budget that goes red for
//! reasons unrelated to the code teaches everyone to ignore it, which is
//! worse than having no budget.
//! Tightening or loosening one is a deliberate commit.
//!
//! Two budgets from the RFC's table are deliberately absent rather than faked:
//!
//! - **Cold start → window interactive.** Out of process by definition. What
//!   is measurable lands in `traces/startup.jsonl` (see `trace::Startup`); the
//!   backend phases are there, and the paint that completes the number needs a
//!   front-end beacon.
//! - **Idle CPU over 60 s.** A test process is not an idle app: the scheduler,
//!   the FTS debouncer, and the webview are exactly what idle CPU is about, and
//!   none of them run here. `activity_stats` and Activity Monitor measure this
//!   honestly; a test cannot.
//!
//! Memory is measured but not asserted — see `search_latency_10k`.
//!
//! The RFC files chat first-token overhead under the retrieval trace; it is
//! asserted here too, because everything in it (embed, search, expansion,
//! manifest, prompt build) runs in-process and none of it needs a model.
//!
//!   cargo test --lib perf_budgets -- --nocapture
//!   cargo test --lib perf_budgets -- --ignored --nocapture   # 10k-chunk store

use std::time::Instant;

use crate::fixtures;

/// Queries per latency sample. Enough that p95 means something without making
/// the default test slow.
const SAMPLES: usize = 40;

/// Top-k the chat path asks for.
const K: usize = 8;

/// Wall-clock milliseconds for each query in `queries`, embedding excluded —
/// the budget is about retrieval, and the embedder is measured separately by
/// the eval harness.
async fn search_millis(lib: &fixtures::Library, queries: &[String]) -> Vec<f64> {
    let mut out = Vec::with_capacity(queries.len());
    let mut hits = 0usize;
    for q in queries {
        let qvec = lib.ai.embed_one(q).await.expect("embed query");
        let start = Instant::now();
        let found = lib
            .db
            .search_chunks(&lib.notebook_id, qvec, q, K, None)
            .await
            .expect("hybrid search");
        out.push(start.elapsed().as_secs_f64() * 1000.0);
        hits += found.len();
    }
    // A search that finds nothing is fast and meaningless. This is the guard
    // that keeps the budget honest — a store seeded without `flush_fts`, or a
    // filter typo, would otherwise post excellent numbers.
    assert!(
        hits >= queries.len(),
        "fixture searches returned {hits} citations over {} queries — \
         the store is not answering, so the latency numbers mean nothing",
        queries.len()
    );
    out
}

/// Wall-clock milliseconds to get from a typed question to a built prompt:
/// embed, hybrid search, neighbor expansion, source manifest, prompt assembly
/// — everything `send_message` does before the first token can be asked for,
/// which is the RFC's "chat first-token overhead, excl. model". Reranking is
/// left out on purpose: it is a model call, and the budget says excl. model.
async fn chat_overhead_millis(lib: &fixtures::Library, queries: &[String]) -> Vec<f64> {
    let profile = lib.ai.profile(crate::inference::Role::Chat);
    let persona = crate::rag::persona_block(&lib.ai.config().profile);
    let mut out = Vec::with_capacity(queries.len());
    for q in queries {
        let start = Instant::now();
        let qvec = lib.ai.embed_one(q).await.expect("embed query");
        let sources = lib
            .db
            .list_sources(&lib.notebook_id)
            .await
            .expect("list sources");
        let corpus_chars: i64 = sources.iter().map(|s| s.char_count).sum();
        let k = profile.retrieve_k_for(corpus_chars);
        let citations = lib
            .db
            .search_chunks(&lib.notebook_id, qvec, q, k, None)
            .await
            .expect("hybrid search");
        let expanded = if profile.neighbor_expansion {
            lib.db
                .expand_neighbor_excerpts(&citations)
                .await
                .unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };
        let manifest: Vec<(String, String, String)> = sources
            .into_iter()
            .map(|s| (s.title, s.url, s.tags))
            .collect();
        let messages = crate::rag::build_chat_messages(
            &[],
            q,
            crate::rag::Excerpts {
                citations: &citations,
                expanded: &expanded,
            },
            &manifest,
            "",
            &persona,
            &profile,
        );
        out.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(!messages.is_empty(), "prompt build produced no messages");
    }
    out
}

/// (p50, p95) of a millisecond sample, nearest-rank.
fn percentiles(mut ms: Vec<f64>) -> (f64, f64) {
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |p: f64| ms[(((ms.len() as f64) * p).ceil() as usize).clamp(1, ms.len()) - 1];
    (at(0.50), at(0.95))
}

/// The fastest sample — what this machine can do when nothing is in the way.
///
/// Budgets assert on this rather than on p95, because `cargo test` runs the
/// whole suite in parallel and these are wall-clock measurements: a fixture
/// seeding on another thread inflates p95 without anything having regressed.
/// Contention can only ever make a sample slower, so the minimum is the one
/// statistic load cannot fake — and a real regression raises the floor too,
/// which is exactly what the budget is meant to catch. p50/p95 are still
/// printed, because the spread is worth seeing even when it can't be
/// asserted on.
fn best(ms: &[f64]) -> f64 {
    ms.iter().copied().fold(f64::INFINITY, f64::min)
}

/// Resident set size of this process in megabytes, or None where `ps` cannot
/// answer. Reported only — see the call site.
fn rss_mb() -> Option<f64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8(out.stdout)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .map(|kb| kb / 1024.0)
}

/// Hybrid search over a small seeded store. The default run: fast, and it
/// catches the gross regressions (a full scan per query, a rebuilt index per
/// query, an N+1 over sources) without waiting on a 10k-chunk seed.
#[tokio::test]
#[ignore = "wall-clock budget: runs serially in its own CI step, not alongside 390 parallel tests"]
async fn search_latency_small() {
    let Some(lib) = fixtures::library(fixtures::SMALL).await else {
        return;
    };
    let queries = fixtures::queries(lib.sources, SAMPLES);
    // Warm the table handle and the FTS index before sampling.
    search_millis(&lib, &queries[..4]).await;
    let samples = search_millis(&lib, &queries).await;
    let (p50, p95) = percentiles(samples.clone());
    let fastest = best(&samples);
    eprintln!(
        "hybrid search, {} sources / {} chunks, {:.0} MB on disk (cached={}): \
         best {fastest:.0} ms, p50 {p50:.0} ms, p95 {p95:.0} ms",
        lib.sources,
        lib.chunks,
        lib.disk_mb(),
        lib.cached
    );

    // Measured 2026-08-23, debug build, M-series laptop: best 13-14 ms,
    // p50 14-18 ms.
    assert!(
        fastest < 60.0,
        "small-store hybrid search best {fastest:.0} ms"
    );

    let chat = chat_overhead_millis(&lib, &queries).await;
    let (c50, c95) = percentiles(chat.clone());
    let chat_best = best(&chat);
    eprintln!(
        "chat overhead excl. model: best {chat_best:.0} ms, p50 {c50:.0} ms, \
         p95 {c95:.0} ms"
    );
    // Measured 2026-08-23, debug build: best ~50 ms. The RFC's 500 ms is the
    // budget at library scale; this store is 48 sources, so it gets a
    // threshold its own size.
    assert!(chat_best < 120.0, "chat overhead best {chat_best:.0} ms");
}

/// The RFC's search budget at its stated scale: a ~10k-chunk store.
/// `#[ignore]`d because the first run seeds it — about 20 s and 86 MB — and a
/// default `cargo test` should not pay that; the fixture cache carries every
/// run after.
#[tokio::test]
#[ignore = "seeds a ~10k-chunk fixture store on first run; cached after"]
async fn search_latency_10k() {
    let Some(lib) = fixtures::library(fixtures::LARGE).await else {
        return;
    };
    let queries = fixtures::queries(lib.sources, SAMPLES);
    search_millis(&lib, &queries[..4]).await;
    let samples = search_millis(&lib, &queries).await;
    let (p50, p95) = percentiles(samples.clone());
    let fastest = best(&samples);
    eprintln!(
        "hybrid search, {} sources / {} chunks, {:.0} MB on disk (cached={}): \
         best {fastest:.0} ms, p50 {p50:.0} ms, p95 {p95:.0} ms",
        lib.sources,
        lib.chunks,
        lib.disk_mb(),
        lib.cached
    );

    // Reported, never asserted: `cargo test` shares one process across tests,
    // so resident memory here is the harness plus the embedder plus whatever
    // ran alongside — not attributable to the store. The number is worth
    // printing for calibration; asserting it would be a number about the
    // test harness dressed up as a number about the app.
    // Measured 2026-08-23: 215-229 MB warm, 497 MB on the run that also
    // seeded the store. The RFC's ceiling is 800 MB for a 10k-*source*
    // library, an axis this fixture (833 sources, 10k chunks) does not reach.
    if let Some(mb) = rss_mb() {
        eprintln!("process RSS after the sweep: {mb:.0} MB (reported, not asserted)");
    }

    // Measured 2026-08-23, debug build, M-series laptop: best ~32 ms,
    // p50 32-38 ms. The RFC proposed 300 ms for this scale and the machine
    // came in six times under it, so the tripwire sits near 4x measured
    // instead — a regression to 200 ms would still be a regression.
    assert!(
        fastest < 120.0,
        "10k-chunk hybrid search best {fastest:.0} ms"
    );

    let chat = chat_overhead_millis(&lib, &queries).await;
    let (c50, c95) = percentiles(chat.clone());
    let chat_best = best(&chat);
    eprintln!(
        "chat overhead excl. model: best {chat_best:.0} ms, p50 {c50:.0} ms, \
         p95 {c95:.0} ms"
    );
    // Measured 2026-08-23, debug build: best ~116 ms (RFC budget 500 ms).
    // The term that grows here is the source manifest — `list_sources` scans
    // every row in the notebook on every question — so this is the number to
    // watch as libraries get wide rather than deep.
    assert!(chat_best < 350.0, "chat overhead best {chat_best:.0} ms");
}

/// Import throughput for a 100-page PDF, embedding excluded — the extract and
/// chunk legs, which are the ones that have regressed before.
#[tokio::test]
#[ignore = "wall-clock budget: runs serially in its own CI step, not alongside 390 parallel tests"]
async fn import_throughput_pdf() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/fixture-cache");
    std::fs::create_dir_all(&dir).expect("fixture cache dir");
    let path = dir.join("throughput-100p.pdf");
    fixtures::write_pdf(&path, 100).expect("generate fixture pdf");

    // Best of three, for the same reason the latency sweeps assert on their
    // fastest sample: this runs alongside the rest of the suite.
    let mut secs = f64::INFINITY;
    let mut last = None;
    for _ in 0..3 {
        let start = Instant::now();
        let t = crate::pdf::extract_text(path.to_str().expect("utf-8 path")).expect("extract pdf");
        let e = t.markdown();
        let c = crate::ingest::chunk_text("Throughput Fixture", &e);
        secs = secs.min(start.elapsed().as_secs_f64());
        last = Some((t, e, c));
    }
    let (text, extracted, chunks) = last.expect("three runs happened");
    eprintln!(
        "import, 100-page PDF: {secs:.2} s excl. embedding \
         ({} pages, {} chars, {} chunks)",
        text.pages.len(),
        extracted.chars().count(),
        chunks.len()
    );

    // The generator has to actually produce a text layer for this to measure
    // anything; a silently empty extraction would post a perfect time.
    assert_eq!(text.pages.len(), 100, "fixture PDF lost pages");
    assert!(
        text.pages_needing_ocr.is_empty(),
        "fixture PDF has no text layer on pages {:?}",
        text.pages_needing_ocr
    );
    assert!(chunks.len() > 20, "fixture PDF chunked to {}", chunks.len());

    // Measured 2026-08-23, debug build, M-series laptop: best 0.48 s. The
    // RFC's 10 s budget is for real-world PDFs; this fixture is deliberately
    // plain, so holding it to 10 s would assert nothing.
    assert!(secs < 1.5, "100-page PDF import took {secs:.2} s");
}

/// Cold app start, at library scale.
///
/// `list_notebooks` is the call boot waits on, and it used to scan the
/// sources and notes tables end to end on every refresh. On a large library
/// that grew slow enough to reach the frontend's 30s IPC timeout — which
/// rejected init's whole `Promise.all` and rendered the shelf as a brand-new
/// install over an intact library. That is the regression this budget
/// exists to catch, so it measures the two shapes separately:
///
/// - **cold**, a fresh `Db::open` reading the persisted counts cache, which
///   is what an app launch actually does;
/// - **warm**, a repeat call on the same handle, which is what every
///   `mcp://changed` refresh does.
///
/// Neither may approach the IPC timeout, and the whole point of the cache is
/// that neither depends on corpus size.
#[tokio::test]
#[ignore = "wall-clock budget: runs serially in its own CI step, not alongside 390 parallel tests"]
async fn notebook_list_latency_cold_and_warm() {
    let Some(lib) = fixtures::library(fixtures::LARGE).await else {
        return;
    };
    // Warm the cache the way a first launch does, then measure a genuine
    // cold open: a new Db over the same directory, as a relaunch would.
    lib.db.list_notebooks().await.expect("prime counts");

    let mut cold = Vec::new();
    for _ in 0..3 {
        let db = crate::db::Db::open(&lib.dir).await.expect("reopen store");
        let t = Instant::now();
        let list = db.list_notebooks().await.expect("list notebooks");
        cold.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(!list.is_empty(), "fixture library has notebooks to count");
    }

    let mut warm = Vec::new();
    for _ in 0..SAMPLES {
        let t = Instant::now();
        lib.db.list_notebooks().await.expect("list notebooks");
        warm.push(t.elapsed().as_secs_f64() * 1000.0);
    }

    // What the cache is worth: drop the file and make it scan for real. Also
    // a correctness check — a recount must agree with what was served from
    // the cache, or the cache is confidently wrong, which is worse than slow.
    let cached_counts: Vec<(String, i64, i64)> = lib
        .db
        .list_notebooks()
        .await
        .expect("list notebooks")
        .into_iter()
        .map(|n| (n.id, n.source_count, n.note_count))
        .collect();
    let _ = std::fs::remove_file(lib.dir.join("notebook-counts.json"));
    let scanning = crate::db::Db::open(&lib.dir).await.expect("reopen store");
    let t = Instant::now();
    let recounted = scanning.list_notebooks().await.expect("list notebooks");
    let recount_ms = t.elapsed().as_secs_f64() * 1000.0;
    let recounted: Vec<(String, i64, i64)> = recounted
        .into_iter()
        .map(|n| (n.id, n.source_count, n.note_count))
        .collect();
    assert_eq!(
        cached_counts, recounted,
        "cached counts must equal a full recount"
    );

    let cold_best = best(&cold);
    let warm_best = best(&warm);
    eprintln!(
        "list_notebooks, {} sources / {} chunks: cold {cold_best:.0} ms, \
         warm {warm_best:.0} ms, uncached recount {recount_ms:.0} ms",
        lib.sources, lib.chunks
    );

    // Measured 2026-08-24, debug build, M-series laptop: cold 3 ms, warm
    // 2 ms, uncached recount 11 ms — against a frontend timeout of
    // 30_000 ms. The gap between cached and recount is the point, and it
    // widens with the corpus: the scan is O(rows), the cache O(notebooks).
    // The thresholds sit far below the IPC ceiling on purpose — by the time
    // this call takes a second, something has gone wrong that a 30s
    // tripwire would never catch.
    assert!(cold_best < 1_000.0, "cold list_notebooks {cold_best:.0} ms");
    assert!(warm_best < 250.0, "warm list_notebooks {warm_best:.0} ms");
}

/// Measure `list_notebooks` against a real library rather than a fixture.
///
/// Point `ALCHEMY_STORE` at a COPY of a lancedb directory (`cp -c -R` clones
/// it in constant time on APFS) and run:
///
/// ```text
/// ALCHEMY_STORE=/tmp/store cargo test --lib real_library_notebook_list \
///     -- --ignored --nocapture
/// ```
///
/// Asserts nothing — fixtures are what CI gates on, because they are the
/// only thing every machine shares. This exists to answer "did that help on
/// MY library", where the fixture's answer is only suggestive.
#[tokio::test]
#[ignore = "opt-in: needs ALCHEMY_STORE pointing at a copy of a real library"]
async fn real_library_notebook_list() {
    let Ok(dir) = std::env::var("ALCHEMY_STORE") else {
        eprintln!("SKIP: set ALCHEMY_STORE to a copy of a lancedb directory");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let counts = dir.join("notebook-counts.json");

    // Uncached: what every refresh used to cost.
    let _ = std::fs::remove_file(&counts);
    let db = crate::db::Db::open(&dir).await.expect("open store");
    let t = Instant::now();
    let cold_scan = db.list_notebooks().await.expect("list");
    let scan_ms = t.elapsed().as_secs_f64() * 1000.0;

    // Cached, cold process: what a launch costs now.
    let reopened = crate::db::Db::open(&dir).await.expect("reopen store");
    let t = Instant::now();
    let cold_cached = reopened.list_notebooks().await.expect("list");
    let cold_ms = t.elapsed().as_secs_f64() * 1000.0;

    // Cached, warm handle: what every mcp://changed refresh costs now.
    let mut warm = Vec::new();
    for _ in 0..10 {
        let t = Instant::now();
        reopened.list_notebooks().await.expect("list");
        warm.push(t.elapsed().as_secs_f64() * 1000.0);
    }

    assert_eq!(
        cold_scan.len(),
        cold_cached.len(),
        "cache and scan must see the same notebooks"
    );
    let totals = |v: &[crate::models::Notebook]| -> (i64, i64) {
        (
            v.iter().map(|n| n.source_count).sum(),
            v.iter().map(|n| n.note_count).sum(),
        )
    };
    assert_eq!(
        totals(&cold_scan),
        totals(&cold_cached),
        "cached totals must equal the scanned ones"
    );

    eprintln!(
        "real library: {} notebooks, {} sources, {} notes\n  \
         uncached scan {scan_ms:.0} ms | cold cached {cold_ms:.0} ms | warm {:.0} ms",
        cold_scan.len(),
        totals(&cold_scan).0,
        totals(&cold_scan).1,
        best(&warm),
    );
}
