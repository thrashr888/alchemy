//! Deterministic fixture libraries (docs/RFC-professional-grade.md).
//!
//! One seeding entry point shared by the perf budgets (pillar 2) and the
//! fidelity contract tests (pillar 3): a generated corpus of N sources, run
//! through the real chunk → embed → store path with the built-in embedder, so
//! it needs no Ollama and no network beyond the embedder's one-time download.
//!
//! Nothing here is checked in. The 10k-chunk store measures 86 MB on disk;
//! instead the generator writes into `target/fixture-cache/` and marks the
//! directory `seeded.ok` when it finishes, so the first run pays the seeding
//! cost and every run after it opens a warm store. Deleting the cache
//! directory is the only invalidation — bump `GENERATION` when the generator's
//! output changes so stale caches rebuild themselves.

use std::path::PathBuf;

use crate::ai::Ai;
use crate::db::Db;
use crate::ingest;
use crate::models::{Notebook, Source};

/// Bump when the generated text, chunking, or store layout changes, so caches
/// from an older generator are not silently reused.
const GENERATION: u32 = 1;

/// Chunks per Lance commit while seeding. Large enough that a 10k-chunk store
/// is a handful of commits (the BEIR eval learned this the expensive way: one
/// commit per document turned corpus seeding into an afternoon).
const SEED_BATCH: usize = 2_000;

/// A generated document chunks into about a dozen rows at the ingest default
/// of 280 words per chunk (four ~320-word sections, split on their headings).
/// Only used to size the constants below — the tests report the real count.
const CHUNKS_PER_SOURCE: usize = 12;

/// A small library — the default for tests that only need a store with real
/// content in it. ~580 chunks.
pub(crate) const SMALL: usize = 48;

/// ~10k chunks, the store size the RFC's search budget names. Seeding it takes
/// about 20 s and 86 MB, so only `#[ignore]`d tests ask for it; the cache
/// carries every run after.
pub(crate) const LARGE: usize = 10_000 / CHUNKS_PER_SOURCE;

/// A seeded store plus the embedder that filled it.
pub(crate) struct Library {
    pub(crate) db: Db,
    pub(crate) ai: Ai,
    pub(crate) notebook_id: String,
    pub(crate) sources: usize,
    pub(crate) chunks: usize,
    pub(crate) dir: PathBuf,
    /// False when this run seeded the store, true when it opened a warm one.
    /// Perf numbers from a run that just wrote 10k rows are not comparable to
    /// numbers from a warm open, so tests print this.
    pub(crate) cached: bool,
}

impl Library {
    /// On-disk size of the seeded store, in megabytes. Reported by the perf
    /// tests: a store that suddenly doubles on disk explains a latency change.
    pub(crate) fn disk_mb(&self) -> f64 {
        fn walk(dir: &std::path::Path) -> u64 {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return 0;
            };
            entries
                .flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path()),
                    _ => e.metadata().map(|m| m.len()).unwrap_or(0),
                })
                .sum()
        }
        walk(&self.dir) as f64 / (1024.0 * 1024.0)
    }
}

/// Seed (or cache-hit) a library of `sources` generated documents.
/// Returns None when the built-in embedder cannot be reached, matching the
/// eval harness's skip contract.
pub(crate) async fn library(sources: usize) -> Option<Library> {
    let ai = crate::evals::builtin_ai().await?;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/fixture-cache")
        .join(format!("store-g{GENERATION}-{sources}"));
    // The marker guards against a half-seeded cache left by an aborted run.
    let marker = dir.join("seeded.ok");
    let cached = marker.exists();

    let db = Db::open(&dir).await.expect("open fixture db");
    db.set_fusion(ai.fusion_params());

    let notebook_id = "fixture-nb".to_string();
    // The marker doubles as the chunk count, so a cache hit still reports the
    // store's real size.
    let chunks = if cached {
        std::fs::read_to_string(&marker)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    } else {
        let n = seed(&ai, &db, &notebook_id, sources).await;
        std::fs::write(&marker, n.to_string()).ok();
        n
    };

    Some(Library {
        db,
        ai,
        notebook_id,
        sources,
        chunks,
        dir,
        cached,
    })
}

/// Write `sources` generated documents into an empty store. Returns the
/// number of chunk rows written.
async fn seed(ai: &Ai, db: &Db, notebook_id: &str, sources: usize) -> usize {
    db.create_notebook(&Notebook {
        id: notebook_id.to_string(),
        title: "Fixture Library".into(),
        created_at: 0,
        updated_at: 0,
        color: "#eb5757".into(),
        icon: String::new(),
        status: String::new(),
        growth_web: false,
        source_count: 0,
        note_count: 0,
        report_count: 0,
    })
    .await
    .expect("create fixture notebook");

    // Bulk-write mode: chunk writes only mark the BM25 index dirty, and this
    // seeder flushes once at the end rather than paying a Tantivy rebuild per
    // batch. Forgetting the flush is the classic failure — BM25 then returns
    // nothing and every hybrid measurement silently becomes vector-only.
    db.defer_fts(true);

    let mut rows: Vec<(String, String, i32, String)> = Vec::new();
    let mut inputs: Vec<String> = Vec::new();
    let mut written = 0usize;
    for i in 0..sources {
        let (title, body) = document(i);
        let extracted = ingest::extract_pasted(&title, &body).expect("extract fixture");
        let doc_chunks = ingest::chunk_text(&extracted.title, &extracted.text);
        let source_id = format!("fx-src-{i:06}");
        for (j, c) in doc_chunks.iter().enumerate() {
            rows.push((
                source_id.clone(),
                format!("{source_id}-c{j}"),
                j as i32,
                c.text.clone(),
            ));
            inputs.push(c.embed_text.clone());
        }
        // Source rows go in one at a time: db.rs keeps its bulk `add_batch`
        // private and only chunks have a public bulk path. One commit per
        // source is the price, and it is why the cache exists.
        db.insert_source(
            &Source {
                id: source_id,
                notebook_id: notebook_id.to_string(),
                title: extracted.title.clone(),
                source_type: "text".into(),
                url: String::new(),
                content: extracted.text.clone(),
                char_count: extracted.text.chars().count() as i64,
                chunk_count: doc_chunks.len() as i64,
                created_at: i as i64,
                status: "ready".into(),
                error: String::new(),
                parent_id: String::new(),
                mtime: 0,
                tags: String::new(),
                note: String::new(),
                image_url: String::new(),
                author: String::new(),
                fetched_at: 0,
                fetch_failures: 0,
            },
            &[],
            &[],
        )
        .await
        .expect("store fixture source");

        if rows.len() >= SEED_BATCH {
            written += flush_rows(ai, db, notebook_id, &mut rows, &mut inputs).await;
            eprintln!("fixtures: seeded {}/{sources} sources", i + 1);
        }
    }
    written += flush_rows(ai, db, notebook_id, &mut rows, &mut inputs).await;

    db.defer_fts(false);
    db.flush_fts().await.expect("flush fixture fts");
    // A store with one commit per source is a store with one fragment per
    // source; compacting makes the fixture look like a maintained install
    // rather than a pathological one, which is what the budgets are about.
    db.maintain().await.expect("compact fixture store");
    written
}

async fn flush_rows(
    ai: &Ai,
    db: &Db,
    notebook_id: &str,
    rows: &mut Vec<(String, String, i32, String)>,
    inputs: &mut Vec<String>,
) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let embeddings = ai.embed(inputs).await.expect("embed fixture batch");
    db.add_chunk_rows(notebook_id, rows, &embeddings)
        .await
        .expect("store fixture chunks");
    let n = rows.len();
    rows.clear();
    inputs.clear();
    n
}

// ---- Generated documents ---------------------------------------------------

/// Topic vocabularies. Each document draws from exactly one, so semantically
/// close neighbors exist for vector search to sort through, while the
/// per-document identifier gives BM25 something only it can find.
const TOPICS: &[(&str, &[&str])] = &[
    (
        "Field Survey",
        &[
            "transect",
            "quadrat",
            "canopy",
            "sediment",
            "salinity",
            "estuary",
            "biomass",
            "sampling",
            "tidal",
            "understory",
            "moss",
            "lichen",
            "burrow",
            "fledgling",
        ],
    ),
    (
        "Operations Review",
        &[
            "throughput",
            "backlog",
            "escalation",
            "runbook",
            "rollout",
            "staging",
            "capacity",
            "incident",
            "handover",
            "queue",
            "retention",
            "quota",
            "failover",
            "latency",
        ],
    ),
    (
        "Kitchen Notebook",
        &[
            "brine",
            "proof",
            "ferment",
            "sear",
            "reduction",
            "emulsion",
            "zest",
            "braise",
            "sourdough",
            "hydration",
            "crumb",
            "yeast",
            "stock",
            "chiffonade",
        ],
    ),
    (
        "Travel Log",
        &[
            "ferry",
            "hostel",
            "trailhead",
            "switchback",
            "market",
            "cathedral",
            "tramline",
            "harbour",
            "vineyard",
            "border",
            "sleeper",
            "platform",
            "bakery",
            "ridge",
        ],
    ),
    (
        "Legal Memo",
        &[
            "indemnity",
            "covenant",
            "assignment",
            "arbitration",
            "severability",
            "waiver",
            "counterparty",
            "recital",
            "warranty",
            "novation",
            "escrow",
            "tortious",
            "jurisdiction",
            "remedy",
        ],
    ),
    (
        "Lab Protocol",
        &[
            "aliquot",
            "centrifuge",
            "buffer",
            "incubate",
            "titration",
            "reagent",
            "supernatant",
            "pipette",
            "gradient",
            "assay",
            "lysate",
            "plating",
            "colony",
            "dilution",
        ],
    ),
];

const CONNECTIVES: &[&str] = &[
    "the",
    "after",
    "before",
    "against",
    "measured",
    "recorded",
    "compared with",
    "held at",
    "reported for",
    "reviewed by",
    "logged near",
    "adjusted for",
];

/// Deterministic 64-bit mixer — a fixture corpus that changes between runs is
/// a fixture corpus that cannot hold a budget.
fn rng(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Words per section. Structure-aware chunking splits on the headings first
/// and then on the 280-word ingest default, so four sections at this length
/// come out at twelve chunks a document.
const SECTION_WORDS: usize = 320;
const SECTIONS: usize = 4;

/// The `(title, body)` of generated document `i`. Body is markdown with
/// headings so structure-aware chunking has real section boundaries.
pub(crate) fn document(i: usize) -> (String, String) {
    let (topic, vocab) = TOPICS[i % TOPICS.len()];
    let title = format!("{topic} {i:05}");
    let mut state = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut body = String::with_capacity(SECTIONS * SECTION_WORDS * 8);
    for s in 0..SECTIONS {
        body.push_str(&format!("\n\n# {} — part {}\n\n", topic, s + 1));
        // The identifier lands in the first section only, so exact-match
        // queries have exactly one correct chunk to find.
        if s == 0 {
            body.push_str(&format!("Reference {}. ", identifier(i)));
        }
        let mut words = 0;
        while words < SECTION_WORDS {
            let len = 8 + (rng(&mut state) % 12) as usize;
            for w in 0..len {
                if w == 0 {
                    body.push_str(
                        CONNECTIVES[(rng(&mut state) % CONNECTIVES.len() as u64) as usize],
                    );
                } else {
                    body.push(' ');
                    body.push_str(vocab[(rng(&mut state) % vocab.len() as u64) as usize]);
                }
            }
            body.push_str(". ");
            words += len;
        }
    }
    (title, body)
}

/// The unique token document `i` carries — the BM25-only needle.
pub(crate) fn identifier(i: usize) -> String {
    format!("ALC-{i:06}")
}

/// A deterministic query mix for latency measurement: alternating rare
/// identifiers (BM25 carries these) and topical prose (the vector leg carries
/// these), spread across the corpus rather than clustered at its start.
pub(crate) fn queries(sources: usize, count: usize) -> Vec<String> {
    let stride = (sources / count.max(1)).max(1);
    (0..count)
        .map(|n| {
            let i = (n * stride) % sources.max(1);
            if n % 2 == 0 {
                format!("what does {} say?", identifier(i))
            } else {
                let (topic, vocab) = TOPICS[i % TOPICS.len()];
                format!(
                    "{topic}: notes on {} and {}",
                    vocab[i % vocab.len()],
                    vocab[(i + 5) % vocab.len()]
                )
            }
        })
        .collect()
}

// ---- Generated PDFs --------------------------------------------------------

/// Write a `pages`-page PDF with a real text layer to `path`.
///
/// Hand-rolled rather than checked in: the import-throughput budget wants a
/// 100-page document, PDF is a text format, and a generated one costs nothing
/// in git. Deliberately plain — one Helvetica text block per page — because
/// this measures the extractor's throughput, not its cleverness. (Hostile PDF
/// shapes are pillar 3's corpus, under `fixtures/hostile/`.)
pub(crate) fn write_pdf(path: &std::path::Path, pages: usize) -> std::io::Result<()> {
    /// Lines of body text per page — roughly a full printed page.
    const LINES: usize = 44;

    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    let object = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", offsets.len(), body).as_bytes());
    };

    out.extend_from_slice(b"%PDF-1.4\n");
    // 1 = catalog, 2 = page tree, 3 = font; pages start at 4 and alternate
    // page dictionary / content stream.
    object(&mut out, &mut offsets, "<< /Type /Catalog /Pages 2 0 R >>");
    let kids: String = (0..pages)
        .map(|p| format!("{} 0 R ", 4 + p * 2))
        .collect::<String>();
    object(
        &mut out,
        &mut offsets,
        &format!("<< /Type /Pages /Kids [ {kids}] /Count {pages} >>"),
    );
    object(
        &mut out,
        &mut offsets,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );

    for p in 0..pages {
        let content_obj = 5 + p * 2;
        object(
            &mut out,
            &mut offsets,
            &format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {content_obj} 0 R >>"
            ),
        );
        let mut stream = String::from("BT\n/F1 11 Tf\n14 TL\n1 0 0 1 54 738 Tm\n");
        let (topic, vocab) = TOPICS[p % TOPICS.len()];
        let mut state = (p as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        for line in 0..LINES {
            // ASCII only, and no parenthesis or backslash: PDF literal strings
            // would need escaping, and the point here is throughput, not a
            // torture test of the extractor (that is pillar 3's corpus).
            let mut text = if line == 0 {
                format!("{topic} page {} reference {}", p + 1, identifier(p))
            } else {
                String::new()
            };
            while text.len() < 78 {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(vocab[(rng(&mut state) % vocab.len() as u64) as usize]);
            }
            stream.push_str(&format!("({text}) Tj T*\n"));
        }
        stream.push_str("ET");
        object(
            &mut out,
            &mut offsets,
            &format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            ),
        );
    }

    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(path, out)
}

/// The fixture store answers, and answers through both retrieval legs.
///
/// Each document carries a unique `ALC-` token that appears nowhere else, and
/// only BM25 can find a rare literal like that — an embedding of a fresh
/// identifier is noise. So a hit here proves the seeder flushed the FTS index,
/// which is the failure this corpus exists to avoid: without the flush every
/// hybrid measurement silently becomes a vector-only measurement, and every
/// perf number taken over it is measuring the wrong thing.
#[tokio::test]
async fn fixture_store_answers_exact_identifiers() {
    let Some(lib) = library(SMALL).await else {
        return;
    };
    for i in [0usize, SMALL / 3, SMALL - 1] {
        let id = identifier(i);
        let qvec = lib.ai.embed_one(&id).await.expect("embed identifier");
        let hits = lib
            .db
            .search_chunks(&lib.notebook_id, qvec, &id, 5, None)
            .await
            .expect("hybrid search");
        assert!(
            hits.iter().any(|c| c.snippet.contains(&id)),
            "fixture store did not return {id} — BM25 index missing (unflushed seed?)"
        );
    }
}
