//! Core-loop fidelity contracts (docs/RFC-professional-grade.md Pillar 3).
//!
//! import -> chunk -> cite -> land-on-the-exact-passage, proven against a
//! hostile-input corpus. Three contracts, in the order a failure costs the
//! most confidence:
//!
//! 1. **Ingest never panics.** Every corpus file yields either content or a
//!    designed error. A file that "succeeds" with empty text is a failure —
//!    a scanned PDF has to say it is scanned, not quietly import nothing.
//! 2. **Citation round-trip.** For each text-bearing file: ingest, embed,
//!    store, run a fixed query through the real hybrid search, take the top
//!    citation, and assert its offsets slice `source.content` back to
//!    exactly the excerpt the prompt saw — in bytes, in chars, and in the
//!    UTF-16 units the webview counts in. The CJK/emoji and RTL fixtures
//!    exist to make that last one bite.
//! 3. **Reader landing.** For PDFs, a citation resolves to a known-good
//!    1-indexed page. The frontend scroll is out of scope; the backend
//!    mapping is not.
//!
//! Only the built-in embedder is used, so this runs in CI without Ollama.
//!
//!   cargo test --lib fidelity -- --nocapture
//!   cargo test --lib fidelity -- --ignored --nocapture   # the 2,000-page PDF
//!
//! The corpus lives in `src-tauri/fixtures/hostile/`. The small awkward files
//! are checked in and readable in a diff; the ones that would bloat the repo
//! (the PDFs, the OOXML containers, 10 MB of inline SVG) are synthesized into
//! a temp dir at test time by the generators below. Nothing is downloaded, and
//! nothing depends on the machine the tests run on.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::ai::Ai;
use crate::db::Db;
use crate::evals::builtin_ai;
use crate::inference::ContextProfile;
use crate::ingest;
use crate::models::Source;
use crate::rag;

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/// What ingest must do with a corpus file. There is no third option: a
/// silent empty success is the failure this table is here to forbid.
enum Expect {
    /// Extraction succeeds and the text carries this marker verbatim.
    Text(&'static str),
    /// Extraction fails, and the message says something the user can act on
    /// — it must contain this fragment (lowercased comparison).
    Error(&'static str),
}

struct Fixture {
    name: &'static str,
    /// True for the files synthesized into the generated directory; false for
    /// the ones checked into `fixtures/hostile/`.
    generated: bool,
    exercises: &'static str,
    expect: Expect,
}

/// The hostile corpus, one row per RFC table entry. `huge.pdf` is not here —
/// it is slow enough to earn its own `#[ignore]`d test.
const CORPUS: &[Fixture] = &[
    Fixture {
        name: "rtl-hebrew-arabic.md",
        generated: false,
        exercises: "RTL chunk boundaries and offsets",
        expect: Expect::Text("TKT-8814-QA"),
    },
    Fixture {
        name: "cjk-emoji.md",
        generated: false,
        exercises: "char-vs-byte offsets across CJK, ZWJ emoji, keycaps",
        expect: Expect::Text("JP-2024-0731"),
    },
    Fixture {
        name: "minified-run.txt",
        generated: false,
        exercises: "one unbroken 400-token line — the word-window chunk fallback",
        expect: Expect::Text("MINRUN-7788"),
    },
    Fixture {
        name: "empty.txt",
        generated: false,
        exercises: "0-byte file — designed error, not an empty import",
        expect: Expect::Error("no readable text"),
    },
    Fixture {
        name: "empty.pdf",
        generated: false,
        exercises: "0-byte file on the PDF path",
        expect: Expect::Error("pdf"),
    },
    Fixture {
        name: "truncated.pdf",
        generated: false,
        exercises: "broken xref / dangling object reference",
        expect: Expect::Error("pdf"),
    },
    Fixture {
        name: "not-a-pdf.pdf",
        generated: false,
        exercises: "wrong extension — HTML wearing .pdf",
        expect: Expect::Error("pdf"),
    },
    Fixture {
        name: "plain-text-named.docx",
        generated: false,
        exercises: "wrong extension — plain text with no ZIP container",
        expect: Expect::Error("extract"),
    },
    Fixture {
        name: "scanned-only.pdf",
        generated: true,
        exercises: "no text layer — must report scanned, never empty",
        expect: Expect::Error("scanned"),
    },
    Fixture {
        name: "rotated-columns.pdf",
        generated: true,
        exercises: "rotated + multi-column pages, offset -> page mapping",
        expect: Expect::Text("NEEDLE-P3-LEFT"),
    },
    Fixture {
        name: "broken-rels.docx",
        generated: true,
        // The relationship part is truncated mid-attribute and the body
        // hyperlinks to an id that was never declared. anydoc recovers the
        // run text anyway — which is the right answer, and the reason this
        // row expects content rather than an error.
        exercises: "malformed relationship XML — recovers the text, no panic",
        expect: Expect::Text("BROKENDOC-3311"),
    },
    Fixture {
        name: "no-document-part.docx",
        generated: true,
        exercises: "valid ZIP, missing main part — ingest error path, no panic",
        expect: Expect::Error("extract"),
    },
    Fixture {
        name: "huge-svg.html",
        generated: true,
        exercises: "web ingest limits — megabytes of inline SVG around thin prose",
        expect: Expect::Text("SVGDOC-6120"),
    },
];

/// Checked-in fixtures.
fn committed_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hostile")
}

/// Where the synthesized fixtures land: one stable directory, written once
/// per process. Stable rather than per-run so repeated `cargo test` passes
/// reuse it instead of leaving 10 MB of SVG behind each time, and written
/// through a rename so a second process reading mid-write can never see a
/// half-file.
fn generated_dir() -> &'static Path {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join("alchemy-hostile-corpus");
        std::fs::create_dir_all(&dir).expect("create generated fixture dir");
        write(&dir, "scanned-only.pdf", &scanned_pdf());
        write(&dir, "rotated-columns.pdf", &rotated_columns_pdf());
        write(&dir, "broken-rels.docx", &broken_rels_docx());
        write(&dir, "no-document-part.docx", &no_document_part_docx());
        write(&dir, "huge-svg.html", huge_svg_html().as_bytes());
        dir
    });
    dir.as_path()
}

/// Write atomically: a temp name in the same directory, then a rename.
fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    let staging = dir.join(format!(".{name}.{}", uuid::Uuid::new_v4()));
    std::fs::write(&staging, bytes).expect("write generated fixture");
    std::fs::rename(&staging, &path).expect("publish generated fixture");
    path
}

/// Every fixture in `CORPUS` by name, committed and generated alike.
fn corpus_paths() -> HashMap<&'static str, PathBuf> {
    let generated = generated_dir();
    CORPUS
        .iter()
        .map(|f| {
            let base = if f.generated {
                generated.to_path_buf()
            } else {
                committed_dir()
            };
            (f.name, base.join(f.name))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

/// Minimal PDF writer.
///
/// No PDF-authoring crate is in the tree, and these fixtures have to be
/// deliberately awkward — pages rotated 90 and 270 degrees, two-column text
/// blocks, an image-only scan, two thousand pages — so they are assembled
/// object by object here. The repo keeps the recipe, not the megabytes.
struct Pdf {
    objects: Vec<Vec<u8>>,
}

impl Pdf {
    fn new() -> Self {
        Self {
            objects: Vec::new(),
        }
    }

    /// Append an object, returning its 1-indexed object number.
    fn add(&mut self, body: Vec<u8>) -> usize {
        self.objects.push(body);
        self.objects.len()
    }

    /// Claim an object number now and fill it in later — the catalog and the
    /// page tree both have to name objects that do not exist yet.
    fn reserve(&mut self) -> usize {
        self.add(Vec::new())
    }

    fn set(&mut self, id: usize, body: Vec<u8>) {
        self.objects[id - 1] = body;
    }

    /// A stream object: `dict` fields, the computed `/Length`, and the data.
    fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
        let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream");
        out
    }

    /// Serialize with a classic cross-reference table.
    fn finish(self, root: usize) -> Vec<u8> {
        let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::with_capacity(self.objects.len());
        for (i, body) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let startxref = out.len();
        let count = self.objects.len() + 1;
        out.extend_from_slice(format!("xref\n0 {count}\n0000000000 65535 f \n").as_bytes());
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {count} /Root {root} 0 R >>\nstartxref\n{startxref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        out
    }
}

/// One page of a generated PDF.
struct PageSpec {
    /// `/Rotate` in degrees — 0, 90, 180 or 270.
    rotate: i32,
    /// Content stream operators.
    content: String,
}

/// PDF string-literal escaping. Fixture text stays ASCII: the fixtures use
/// the base-14 Helvetica with WinAnsi encoding, which has no Hebrew, Arabic
/// or CJK glyphs (see `rtl_is_covered_by_markdown_not_by_pdf`).
fn esc(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('(', r"\(")
        .replace(')', r"\)")
}

/// One `BT … ET` block: `lines` set at (x, y) and running down the page.
fn text_block(x: i32, y: i32, size: i32, leading: i32, lines: &[String]) -> String {
    let mut s = format!("BT\n/F1 {size} Tf\n{leading} TL\n{x} {y} Td\n");
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            s.push_str("T*\n");
        }
        s.push_str(&format!("({}) Tj\n", esc(line)));
    }
    s.push_str("ET\n");
    s
}

fn lines(text: &[&str]) -> Vec<String> {
    text.iter().map(|s| s.to_string()).collect()
}

/// Assemble a document from page specs. `image` adds an 8x8 grayscale
/// XObject to every page's resources as `/Im1`.
fn build_pdf(pages: &[PageSpec], image: bool) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let catalog = pdf.reserve();
    let tree = pdf.reserve();
    let font = pdf.add(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    );
    // A checkerboard is enough: the point is that the page carries pixels and
    // no text items, which is what makes the detector call it a scan.
    let img = image.then(|| {
        let hex: String = (0..64)
            .map(|i: usize| {
                if (i / 8 + i % 8).is_multiple_of(2) {
                    "00"
                } else {
                    "FF"
                }
            })
            .collect::<String>()
            + ">";
        pdf.add(Pdf::stream(
            "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray \
             /BitsPerComponent 8 /Filter /ASCIIHexDecode",
            hex.as_bytes(),
        ))
    });

    let mut kids = Vec::with_capacity(pages.len());
    for page in pages {
        let content = pdf.add(Pdf::stream("", page.content.as_bytes()));
        let mut resources = format!("/Font << /F1 {font} 0 R >>");
        if let Some(img) = img {
            resources.push_str(&format!(" /XObject << /Im1 {img} 0 R >>"));
        }
        kids.push(
            pdf.add(
                format!(
                    "<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 612 792] /Rotate {} \
                 /Resources << {resources} >> /Contents {content} 0 R >>",
                    page.rotate
                )
                .into_bytes(),
            ),
        );
    }
    let kid_refs = kids
        .iter()
        .map(|k| format!("{k} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");
    pdf.set(
        tree,
        format!(
            "<< /Type /Pages /Count {} /Kids [{kid_refs}] >>",
            kids.len()
        )
        .into_bytes(),
    );
    pdf.set(
        catalog,
        format!("<< /Type /Catalog /Pages {tree} 0 R >>").into_bytes(),
    );
    pdf.finish(catalog)
}

/// Two image-only pages: pixels, no text items anywhere.
fn scanned_pdf() -> Vec<u8> {
    let page = || PageSpec {
        rotate: 0,
        content: "q 420 0 0 540 96 126 cm /Im1 Do Q\n".to_string(),
    };
    build_pdf(&[page(), page()], true)
}

/// Five pages that between them rotate, split into columns, and do both at
/// once. Every page opens with a title set two sizes larger than its body, so
/// the extractor sees a heading and the structure-aware chunker splits on it —
/// which is what makes a citation page-scoped and therefore resolvable back to
/// a page. Each page also carries a unique `NEEDLE-Pn-…` marker.
fn rotated_columns_pdf() -> Vec<u8> {
    /// Title at the top of the page, set large enough that the extractor
    /// classifies it as a heading rather than body text.
    fn title(text: &str) -> String {
        text_block(60, 740, 20, 24, &lines(&[text]))
    }
    fn body(text: &[&str]) -> String {
        text_block(60, 690, 11, 15, &lines(text))
    }
    /// Two columns of equal length, side by side under the title.
    fn columns(left: &[&str], right: &[&str]) -> String {
        assert_eq!(left.len(), right.len(), "columns must be the same length");
        text_block(60, 690, 11, 15, &lines(left)) + &text_block(330, 690, 11, 15, &lines(right))
    }

    let p1 = title("Vestibule Systems Handbook")
        + &body(&[
            "NEEDLE-P1-VESTIBULE",
            "The vestibule assembly seals the platform gap while the train is",
            "stationary. Inspect the rubber gaiter for tears at every depot",
            "visit. Replacement gaiters ship in pairs and cure for twelve hours",
            "before the unit returns to service. A gaiter that has been patched",
            "more than twice is scrapped rather than repaired again, because the",
            "patch seams stiffen and tear the fabric beside them on the next",
            "cold morning. Record every scrap against the vehicle number.",
        ]);
    // Rotated 90 degrees: the text operators are unchanged, so extraction has
    // to honour the page rotation rather than the raw coordinates.
    let p2 = title("Hydraulic Dampers")
        + &body(&[
            "NEEDLE-P2-ROTATED",
            "Damper pressure is logged on the first Monday of each month. A",
            "reading below forty bar means the accumulator has bled down and",
            "the unit must come off the vehicle the same shift. Recharge with",
            "nitrogen only; compressed air voids the warranty and corrodes the",
            "bore from the inside where nobody will see it until the seal",
            "fails. Log the recharge pressure, the ambient temperature, and",
            "the bottle serial number in the depot register.",
        ]);
    // Two columns side by side — the extractor reads them as a table.
    let p3 = title("Cooling Loop")
        + &columns(
            &[
                "NEEDLE-P3-LEFT",
                "The cooling loop draws",
                "from the roof condenser",
                "and returns through the",
                "underfloor manifold.",
                "Glycol concentration is",
                "checked each autumn and",
                "topped up in November.",
            ],
            &[
                "NEEDLE-P3-RIGHT",
                "Fin combs live in the",
                "roof access locker.",
                "Straighten fins before",
                "washing, never after.",
                "A blocked condenser",
                "trips the high-side",
                "switch within minutes.",
            ],
        );
    // Rotated 270 degrees AND two columns — the ugly combination.
    let p4 = title("Firmware Rollback")
        + &columns(
            &[
                "NEEDLE-P4-INVERTED",
                "Rollback requires the",
                "depot dongle and a",
                "countersigned manifest",
                "from the duty engineer.",
                "The dongle is logged",
                "out and back in on the",
                "same shift, no later.",
            ],
            &[
                "NEEDLE-P4-RIGHT",
                "Manifests are JSON with",
                "a detached signature.",
                "Unsigned manifests are",
                "rejected at boot and the",
                "unit falls back to the",
                "previous image without",
                "asking anyone first.",
            ],
        );
    let p5 = title("Appendix: Spare Parts")
        + &body(&[
            "NEEDLE-P5-APPENDIX",
            "Spare gaiters, fin combs and nitrogen bottles are stocked at the",
            "north depot. Order codes are printed on the locker door and",
            "repeated in the depot register. Anything not on that door is",
            "ordered through the regional store with a week of lead time, so",
            "check the shelf before the shift rather than after it.",
        ]);
    build_pdf(
        &[
            PageSpec {
                rotate: 0,
                content: p1,
            },
            PageSpec {
                rotate: 90,
                content: p2,
            },
            PageSpec {
                rotate: 0,
                content: p3,
            },
            PageSpec {
                rotate: 270,
                content: p4,
            },
            PageSpec {
                rotate: 0,
                content: p5,
            },
        ],
        false,
    )
}

/// Deterministic filler so every page of the big PDF is different without a
/// random source: a fixed vocabulary indexed by a small LCG.
fn filler(seed: u64, words: usize) -> String {
    const VOCAB: &[&str] = &[
        "signal",
        "ballast",
        "sleeper",
        "gantry",
        "pantograph",
        "axle",
        "flange",
        "coupler",
        "brake",
        "traction",
        "catenary",
        "bogie",
        "junction",
        "siding",
        "interlock",
        "relay",
    ];
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..words)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            VOCAB[(state >> 33) as usize % VOCAB.len()]
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A 2,000-page book. Page 1873 carries the only occurrence of
/// `NEEDLE-DEEP-1873`, so a citation from it proves retrieval reaches the far
/// end of a long document.
fn huge_pdf(pages: usize) -> Vec<u8> {
    let specs: Vec<PageSpec> = (1..=pages)
        .map(|n| {
            // A page of prose, ~180 words — a real book page, so 2,000 of them
            // chunk as deeply as a real book would.
            let mut body = vec![format!(
                "Chapter {} - page {n} of {pages}",
                (n - 1) / 40 + 1
            )];
            body.extend((0..20).map(|line| filler(n as u64 * 64 + line, 9)));
            if n == 1873 {
                body.push("NEEDLE-DEEP-1873 the north depot keeps one spare".into());
                body.push("nitrogen bottle for the hydraulic accumulator.".into());
            }
            PageSpec {
                rotate: 0,
                content: text_block(72, 730, 11, 15, &body),
            }
        })
        .collect();
    build_pdf(&specs, false)
}

/// Write a ZIP from (name, body) pairs — the OOXML container both DOCX
/// fixtures wear, so the failures below come from the XML layer, not from a
/// corrupt archive.
fn zip_of(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in parts {
            zip.start_file(*name, opts).expect("zip entry");
            zip.write_all(body.as_bytes()).expect("zip write");
        }
        zip.finish().expect("finish zip");
    }
    buf.into_inner()
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;

const PACKAGE_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

/// A DOCX whose relationship part is truncated mid-attribute and whose body
/// hyperlinks to a relationship id that was never declared.
fn broken_rels_docx() -> Vec<u8> {
    zip_of(&[
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", PACKAGE_RELS),
        // Truncated mid-attribute, with no closing </Relationships>.
        (
            "word/_rels/document.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://exam"#,
        ),
        (
            "word/document.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body><w:p><w:hyperlink r:id="rId404"><w:r><w:t>BROKENDOC-3311 dangling relationship into a part that was never declared</w:t></w:r></w:hyperlink></w:p></w:body></w:document>"#,
        ),
    ])
}

/// A DOCX whose package relationships promise a main document part that is
/// not in the archive at all.
fn no_document_part_docx() -> Vec<u8> {
    zip_of(&[
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", PACKAGE_RELS),
    ])
}

/// Roughly 10 MB of inline SVG wrapped around a few paragraphs of real prose.
/// The prose is what has to come out the other side.
fn huge_svg_html() -> String {
    let mut svg = String::with_capacity(11 << 20);
    svg.push_str(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4096 4096">"#);
    let mut n: u64 = 1;
    while svg.len() < 10 << 20 {
        n = n.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let a = n % 4096;
        let b = (n >> 12) % 4096;
        let c = (n >> 24) % 4096;
        svg.push_str(&format!(
            r##"<path d="M{a} {b} C{b} {c} {c} {a} {a} {c} L{b} {a} Z" fill="#3a5f7d" stroke="#12242e"/>"##
        ));
    }
    svg.push_str("</svg>");
    format!(
        r#"<!doctype html>
<html><head><title>Loom Diagram Report</title></head>
<body>
<article>
<h1>Loom Diagram Report</h1>
<p>Reference SVGDOC-6120. The loom diagram below traces every warp thread
through the heddle frames for a single repeat of the pattern. It is rendered
inline so the report stays one file, which is also why this document is large
enough to be worth a limits test.</p>
{svg}
<p>Reading the diagram: warp threads run top to bottom, weft picks run left to
right, and a filled cell means the warp passes over the weft. The draft repeats
every sixteen picks. Tension is measured at the back beam before each repeat,
and a reading outside four to six newtons means the beam needs re-tying.</p>
</article>
</body></html>
"#
    )
}

// ---------------------------------------------------------------------------
// Contract 1 — ingest never panics, and never succeeds silently empty
// ---------------------------------------------------------------------------

#[test]
fn ingest_survives_the_hostile_corpus() {
    let paths = corpus_paths();
    for f in CORPUS {
        let path = paths[f.name].to_string_lossy().to_string();
        // `extract_file` contains panics itself; `chunk_source` does not, so
        // the chunk call below is the live panic surface.
        let got = ingest::extract_file(&path);
        match (&f.expect, got) {
            (Expect::Text(marker), Ok(e)) => {
                assert!(
                    !e.text.trim().is_empty(),
                    "{}: extraction succeeded with empty text — a silent empty import \
                     is the failure this test exists to forbid ({})",
                    f.name,
                    f.exercises
                );
                assert!(
                    e.text.contains(marker),
                    "{}: extracted text lost its marker {marker} ({})",
                    f.name,
                    f.exercises
                );
                let chunks = ingest::chunk_source(&e, None);
                assert!(
                    !chunks.is_empty(),
                    "{}: extracted text but no chunks",
                    f.name
                );
                eprintln!(
                    "  {:<24} ok   {} chars, {} chunks   [{}]",
                    f.name,
                    e.text.chars().count(),
                    chunks.len(),
                    f.exercises
                );
            }
            (Expect::Text(_), Err(err)) => {
                panic!("{}: expected content, got error: {err}", f.name)
            }
            (Expect::Error(fragment), Err(err)) => {
                let msg = format!("{err:#}");
                assert!(
                    msg.to_lowercase().contains(&fragment.to_lowercase()),
                    "{}: error should name the problem ({fragment}), said: {msg}",
                    f.name
                );
                eprintln!("  {:<24} err  {msg}   [{}]", f.name, f.exercises);
            }
            (Expect::Error(_), Ok(e)) => panic!(
                "{}: expected a designed error, extraction returned {} chars",
                f.name,
                e.text.chars().count()
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Contract 2 — citation round-trip
// ---------------------------------------------------------------------------

/// A fixed query put to the real hybrid search, and what the winning citation
/// has to carry. Queries are per-fixture and each fixture gets its own
/// notebook, so this measures the offset invariant, not retrieval ranking.
struct RoundTrip {
    file: &'static str,
    query: &'static str,
    /// Substring the top citation's snippet must contain.
    expect: &'static str,
    /// For PDFs: the 1-indexed page the citation has to resolve to.
    page: Option<usize>,
}

const ROUND_TRIPS: &[RoundTrip] = &[
    RoundTrip {
        file: "rtl-hebrew-arabic.md",
        query: "TKT-8814-QA",
        expect: "TKT-8814-QA",
        page: None,
    },
    RoundTrip {
        file: "rtl-hebrew-arabic.md",
        query: "רשת אורחים סיסמה",
        expect: "רשת האורחים",
        page: None,
    },
    RoundTrip {
        file: "cjk-emoji.md",
        query: "JP-2024-0731",
        expect: "JP-2024-0731",
        page: None,
    },
    RoundTrip {
        file: "cjk-emoji.md",
        query: "第三季度服务器迁移",
        expect: "服务器迁移",
        page: None,
    },
    RoundTrip {
        file: "cjk-emoji.md",
        query: "ZWJ sequences skin tones flags keycaps",
        expect: "👩🏽‍🔬",
        page: None,
    },
    RoundTrip {
        file: "rotated-columns.pdf",
        query: "how often is damper pressure logged?",
        expect: "NEEDLE-P2-ROTATED",
        page: Some(2),
    },
    RoundTrip {
        file: "rotated-columns.pdf",
        query: "where does the cooling loop return through?",
        expect: "NEEDLE-P3-LEFT",
        page: Some(3),
    },
    RoundTrip {
        file: "rotated-columns.pdf",
        query: "what is needed to roll firmware back?",
        expect: "NEEDLE-P4-INVERTED",
        page: Some(4),
    },
    RoundTrip {
        file: "rotated-columns.pdf",
        query: "which depot stocks spare fin combs?",
        expect: "NEEDLE-P5-APPENDIX",
        page: Some(5),
    },
    RoundTrip {
        file: "huge-svg.html",
        query: "SVGDOC-6120 warp threads heddle frames",
        expect: "SVGDOC-6120",
        page: None,
    },
];

/// Ingest one file the way the app does and store it under its own notebook.
/// Returns the stored source, whose `content` is the text every offset in
/// this module is measured against.
async fn ingest_into(ai: &Ai, db: &Db, notebook_id: &str, path: &Path) -> Source {
    let extracted = ingest::extract_file(&path.to_string_lossy()).expect("extract fixture");
    let chunks = ingest::chunk_source(&extracted, None);
    assert!(!chunks.is_empty(), "{path:?}: no chunks");
    let embed_inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let embeddings = ai.embed(&embed_inputs).await.expect("embed fixture");
    let tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (format!("{notebook_id}-c{i}"), i as i32, c.text.clone()))
        .collect();
    let contexts: Vec<String> = chunks.iter().map(|c| c.context.clone()).collect();
    let source = Source {
        id: format!("{notebook_id}-src"),
        notebook_id: notebook_id.to_string(),
        title: extracted.title.clone(),
        source_type: extracted.source_type.clone(),
        url: path.to_string_lossy().to_string(),
        content: extracted.text.clone(),
        char_count: extracted.text.chars().count() as i64,
        chunk_count: tuples.len() as i64,
        created_at: 0,
        status: "ready".into(),
        error: String::new(),
        parent_id: String::new(),
        mtime: 0,
        author: String::new(),
        image_url: String::new(),
        tags: String::new(),
        note: String::new(),
        fetched_at: 0,
        fetch_failures: 0,
    };
    db.insert_source_ctx(&source, &tuples, &contexts, &embeddings)
        .await
        .expect("store fixture");
    source
}

/// The three units anyone measures a citation offset in have to agree on
/// what a span holds: bytes (Rust), chars (`Source.char_count`), and UTF-16
/// code units — the last being what the webview's `indexOf`/`Range` work in,
/// and where a CJK or emoji offset bug actually lands. Returns the slice.
fn slice_three_ways(label: &str, content: &str, start: usize, end: usize) -> String {
    assert!(
        content.is_char_boundary(start) && content.is_char_boundary(end),
        "{label}: byte span [{start}, {end}) splits a UTF-8 character"
    );
    let by_bytes = content[start..end].to_string();

    let char_start = content[..start].chars().count();
    let char_len = by_bytes.chars().count();
    let by_chars: String = content.chars().skip(char_start).take(char_len).collect();
    assert_eq!(
        by_chars,
        by_bytes,
        "{label}: char span [{char_start}, {}) disagrees with the byte span — byte \
         offsets were reused as char offsets somewhere",
        char_start + char_len
    );

    let units: Vec<u16> = content.encode_utf16().collect();
    let u_start = content[..start].encode_utf16().count();
    let u_len = by_bytes.encode_utf16().count();
    let by_utf16 = String::from_utf16(&units[u_start..u_start + u_len])
        .unwrap_or_else(|e| panic!("{label}: UTF-16 span splits a surrogate pair: {e}"));
    assert_eq!(
        by_utf16,
        by_bytes,
        "{label}: UTF-16 span [{u_start}, {}) disagrees with the byte span — the unit \
         the reader counts in",
        u_start + u_len
    );

    by_bytes
}

/// The invariant every reader feature stands on: the citation's snippet is a
/// verbatim span of the stored content, and that span slices back to exactly
/// the snippet in all three units. Returns the byte span.
fn assert_offsets_slice_exactly(label: &str, content: &str, snippet: &str) -> (usize, usize) {
    let start = content.find(snippet).unwrap_or_else(|| {
        panic!(
            "{label}: cited excerpt is not a verbatim span of the stored source text — \
             click-to-highlight has nothing to land on.\n  excerpt: {:?}",
            snippet.chars().take(80).collect::<String>()
        )
    });
    let end = start + snippet.len();
    assert_eq!(
        slice_three_ways(label, content, start, end),
        snippet,
        "{label}: the span does not slice back to the excerpt"
    );
    (start, end)
}

/// Interior whitespace collapsed to single spaces — the equivalence the
/// reader's `locatePassage` matches under (it joins the excerpt's words with
/// `\s+`).
fn squeezed(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Locate `snippet` in `content` ignoring whitespace runs, the way
/// `locatePassage` (src/components/ReaderPane.tsx) does. Returns the byte
/// span in `content`.
fn locate_ignoring_whitespace(content: &str, snippet: &str) -> Option<(usize, usize)> {
    let needle: Vec<char> = snippet.chars().filter(|c| !c.is_whitespace()).collect();
    if needle.is_empty() {
        return None;
    }
    let hay: Vec<(usize, char)> = content
        .char_indices()
        .filter(|(_, c)| !c.is_whitespace())
        .collect();
    (0..=hay.len().saturating_sub(needle.len())).find_map(|i| {
        let window = &hay[i..i + needle.len()];
        window
            .iter()
            .map(|(_, c)| *c)
            .eq(needle.iter().copied())
            .then(|| {
                let (last_at, last_c) = window[needle.len() - 1];
                (window[0].0, last_at + last_c.len_utf8())
            })
    })
}

#[tokio::test]
async fn citation_round_trip_on_hostile_files() {
    let Some(ai) = builtin_ai().await else { return };
    let paths = corpus_paths();
    let dir = std::env::temp_dir().join(format!("alchemy-fidelity-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");

    // One notebook per file: the winning citation is then necessarily from
    // the file under test, so a ranking wobble can never masquerade as an
    // offset bug.
    let mut sources: HashMap<&str, (String, Source)> = HashMap::new();
    for rt in ROUND_TRIPS {
        if sources.contains_key(rt.file) {
            continue;
        }
        let nb = format!("fid-{}", rt.file.replace(['.', '-'], "_"));
        let src = ingest_into(&ai, &db, &nb, &paths[rt.file]).await;
        sources.insert(rt.file, (nb, src));
    }
    db.flush_fts().await.expect("flush fts");

    for rt in ROUND_TRIPS {
        let (nb, src) = &sources[rt.file];
        let label = format!("{} / {:?}", rt.file, rt.query);
        let qvec = ai.embed_one(rt.query).await.expect("embed question");
        let citations = db
            .search_chunks(nb, qvec, rt.query, 4, None)
            .await
            .expect("hybrid search");
        assert!(!citations.is_empty(), "{label}: retrieval returned nothing");

        let top = citations
            .iter()
            .find(|c| c.snippet.contains(rt.expect))
            .unwrap_or_else(|| {
                panic!(
                    "{label}: no citation in the top {} carried {:?}; got {:?}",
                    citations.len(),
                    rt.expect,
                    citations
                        .iter()
                        .map(|c| c.snippet.chars().take(40).collect::<String>())
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(top.source_id, src.id, "{label}: cited the wrong source");

        // 1. The stored offsets slice the source text back to the excerpt.
        let (start, end) = assert_offsets_slice_exactly(&label, &src.content, &top.snippet);

        // 2. `char_count` is the same count the offsets are measured in.
        assert_eq!(
            src.char_count,
            src.content.chars().count() as i64,
            "{label}: stored char_count disagrees with the content"
        );

        // 3. …and that excerpt is exactly what the prompt saw.
        let expanded = db
            .expand_neighbor_excerpts(std::slice::from_ref(top))
            .await
            .expect("expand excerpts");
        let messages = rag::build_chat_messages(
            &[],
            rt.query,
            rag::Excerpts {
                citations: std::slice::from_ref(top),
                expanded: &expanded,
            },
            &[(src.title.clone(), String::new(), String::new())],
            "",
            "",
            &ContextProfile::default(),
        );
        let prompt = &messages.last().expect("prompt turn").content;
        assert!(
            prompt.contains(&src.content[start..end]),
            "{label}: the prompt excerpt is not the span the citation points at"
        );

        // 4. …and for a PDF, that excerpt sits on the page the reader opens to.
        let landed = rt.page.map(|want| {
            let got = citation_page(&paths[rt.file], &top.snippet);
            assert_eq!(got, want, "{label}: citation landed on the wrong page");
            got
        });

        eprintln!(
            "  {:<24} bytes [{start}, {end})  {} chars{}  <- {:?}",
            rt.file,
            top.snippet.chars().count(),
            landed.map(|p| format!("  page {p}")).unwrap_or_default(),
            rt.query
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The one class where a stored excerpt is *not* a byte-verbatim span of the
/// source: a paragraph with no sentence boundary anywhere in it falls back to
/// overlapping word windows (`ingest.rs::word_windows`), which re-joins words
/// with single spaces and so rewrites any run of interior whitespace. The
/// reader survives it — `locatePassage` (src/components/ReaderPane.tsx)
/// matches the excerpt's words separated by `\s+` — and that
/// whitespace-tolerant landing is what this pins, before and after any
/// tightening of the chunker.
#[tokio::test]
async fn word_window_excerpts_stay_locatable() {
    let Some(ai) = builtin_ai().await else { return };
    let path = committed_dir().join("minified-run.txt");
    let store = std::env::temp_dir().join(format!("alchemy-fidelity-mr-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&store).await.expect("open db");
    let nb = "fid-minified";
    let src = ingest_into(&ai, &db, nb, &path).await;
    db.flush_fts().await.expect("flush fts");

    // The fallback only runs for a single run of text with no sentence
    // boundary and more words than one chunk holds. If the fixture ever stops
    // being that, this test stops measuring anything — so say so here.
    assert_eq!(src.content.lines().count(), 1, "fixture must stay one line");
    assert!(
        !src.content.contains(['.', '!', '?']),
        "fixture must stay free of sentence boundaries"
    );
    assert!(
        src.content.split_whitespace().count() > 280,
        "fixture must be longer than one chunk"
    );
    assert!(
        src.chunk_count > 1,
        "the fixture should have been split, got {} chunk(s)",
        src.chunk_count
    );

    let query = "MINRUN-7788";
    let qvec = ai.embed_one(query).await.expect("embed question");
    let citations = db
        .search_chunks(nb, qvec, query, 4, None)
        .await
        .expect("hybrid search");
    let top = citations
        .iter()
        .find(|c| c.snippet.contains("MINRUN-7788"))
        .expect("a citation carrying the needle");

    // Every stored excerpt lands, cited or not.
    for (i, chunk) in db
        .source_chunk_rows(&src.id)
        .await
        .expect("stored chunks")
        .iter()
        .enumerate()
    {
        let label = format!("minified-run.txt chunk {i}");
        let (start, end) = locate_ignoring_whitespace(&src.content, &chunk.2)
            .unwrap_or_else(|| panic!("{label}: excerpt is not locatable in the source at all"));
        let slice = slice_three_ways(&label, &src.content, start, end);
        assert_eq!(
            squeezed(&slice),
            squeezed(&chunk.2),
            "{label}: the located span is not the excerpt"
        );
    }

    let (start, end) = locate_ignoring_whitespace(&src.content, &top.snippet).expect("locate");
    eprintln!(
        "  minified-run.txt        bytes [{start}, {end})  {} chunks",
        src.chunk_count
    );
    let _ = std::fs::remove_dir_all(&store);
}

// ---------------------------------------------------------------------------
// Contract 3 — reader landing
// ---------------------------------------------------------------------------

/// Which 1-indexed page each needle sits on in `rotated-columns.pdf`.
const PAGE_NEEDLES: &[(&str, usize)] = &[
    ("NEEDLE-P1-VESTIBULE", 1),
    ("NEEDLE-P2-ROTATED", 2),
    ("NEEDLE-P3-LEFT", 3),
    ("NEEDLE-P3-RIGHT", 3),
    ("NEEDLE-P4-INVERTED", 4),
    ("NEEDLE-P4-RIGHT", 4),
    ("NEEDLE-P5-APPENDIX", 5),
];

/// The 1-indexed PDF page a citation came from, resolved the way the reader
/// has to resolve it: the excerpt carries page markers, and the extractor
/// puts each marker on exactly one page. An excerpt that straddles pages has
/// no single answer, and this says so rather than picking one.
fn citation_page(path: &Path, snippet: &str) -> usize {
    let pdf = crate::pdf::extract_text(&path.to_string_lossy()).expect("extract pdf text");
    let mut pages: Vec<usize> = PAGE_NEEDLES
        .iter()
        .filter(|(needle, _)| snippet.contains(needle))
        .map(|(needle, page)| {
            assert_eq!(
                pages_carrying(&pdf.pages, needle),
                vec![*page],
                "{needle} must be extracted onto page {page} and nowhere else"
            );
            *page
        })
        .collect();
    pages.dedup();
    assert_eq!(
        pages.len(),
        1,
        "the excerpt should sit on exactly one page, matched pages {pages:?}"
    );
    pages[0]
}

/// The 1-indexed pages whose extracted markdown contains `needle`.
fn pages_carrying(pages: &[String], needle: &str) -> Vec<usize> {
    pages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.contains(needle))
        .map(|(i, _)| i + 1)
        .collect()
}

/// Page provenance survives extraction: every needle lands on exactly the
/// page it was drawn on, through rotation and two-column layout alike. This
/// is the half of "click a citation, land on the right page" that lives in
/// Rust — the citation's own text is pinned by the round-trip test above, and
/// this pins that text to a page.
#[test]
fn pdf_citations_resolve_to_known_good_pages() {
    let path = generated_dir()
        .join("rotated-columns.pdf")
        .to_string_lossy()
        .to_string();

    assert_eq!(crate::pdf::page_count(&path), 5, "PDFium page count");
    let extracted = crate::pdf::extract_text(&path).expect("extract pdf text");
    assert!(
        !extracted.is_scanned(),
        "a text-bearing PDF must not be routed to OCR"
    );
    assert_eq!(extracted.pages.len(), 5, "one entry per page");
    assert!(
        extracted.pages_needing_ocr.is_empty(),
        "no page here is a scan, got {:?}",
        extracted.pages_needing_ocr
    );

    for (needle, page) in PAGE_NEEDLES {
        assert_eq!(
            pages_carrying(&extracted.pages, needle),
            vec![*page],
            "{needle} must resolve to page {page} and nowhere else"
        );
    }

    // And the text a citation would carry survives into the ingested content
    // in the same document order the pages were written in.
    let e = ingest::extract_file(&path).expect("ingest pdf");
    let mut cursor = 0usize;
    for (needle, page) in PAGE_NEEDLES {
        let at = e.text[cursor..]
            .find(needle)
            .map(|i| i + cursor)
            .unwrap_or_else(|| panic!("{needle} (page {page}) missing or out of document order"));
        cursor = at;
    }
}

/// A scanned PDF has to say so. The honesty here is the whole point: the
/// error routes the file to the vision fallback, and a silent empty import
/// would leave the user with a source that looks fine and answers nothing.
#[test]
fn scanned_pdf_reports_no_text_layer() {
    let path = generated_dir()
        .join("scanned-only.pdf")
        .to_string_lossy()
        .to_string();

    let extracted = crate::pdf::extract_text(&path).expect("parse scanned pdf");
    assert!(
        extracted.is_scanned(),
        "an image-only PDF must classify as scanned, got pages {:?}",
        extracted.pages
    );
    assert_eq!(
        extracted.pages_needing_ocr,
        vec![1, 2],
        "every page needs OCR, 1-indexed"
    );

    let msg = match ingest::extract_file(&path) {
        Ok(e) => panic!(
            "a scanned PDF must not import as content, got {} chars",
            e.text.chars().count()
        ),
        Err(err) => format!("{err:#}").to_lowercase(),
    };
    assert!(
        msg.contains("scanned"),
        "the error has to name the reason so the caller can route to OCR: {msg}"
    );
}

/// The 2,000-page book: chunking depth and a citation from page 1873. Slow
/// (PDF generation plus a full extract plus a few thousand embeddings), so it
/// stays out of the default run.
///
///   cargo test --lib fidelity_huge_pdf -- --ignored --nocapture
#[tokio::test]
#[ignore = "generates and ingests a 2,000-page PDF"]
async fn fidelity_huge_pdf_cites_the_far_end() {
    let Some(ai) = builtin_ai().await else { return };
    let dir = std::env::temp_dir().join(format!("alchemy-hostile-huge-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create dir");
    let bytes = huge_pdf(2_000);
    eprintln!("  generated huge.pdf: {} MB", bytes.len() >> 20);
    let path = write(&dir, "huge.pdf", &bytes);
    let path_str = path.to_string_lossy().to_string();

    assert_eq!(crate::pdf::page_count(&path_str), 2_000);
    let pdf = crate::pdf::extract_text(&path_str).expect("extract huge pdf");
    assert_eq!(pdf.pages.len(), 2_000, "one entry per page");
    assert_eq!(
        pages_carrying(&pdf.pages, "NEEDLE-DEEP-1873"),
        vec![1873],
        "the deep needle sits on page 1873 and nowhere else"
    );

    let store =
        std::env::temp_dir().join(format!("alchemy-fidelity-huge-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&store).await.expect("open db");
    let nb = "fid-huge";
    let src = ingest_into(&ai, &db, nb, &path).await;
    db.flush_fts().await.expect("flush fts");
    eprintln!(
        "  ingested {} chars into {} chunks",
        src.char_count, src.chunk_count
    );
    assert!(
        src.chunk_count > 1_000,
        "a 2,000-page book should chunk deeply, got {}",
        src.chunk_count
    );

    let query = "NEEDLE-DEEP-1873 spare nitrogen bottle";
    let qvec = ai.embed_one(query).await.expect("embed question");
    let citations = db
        .search_chunks(nb, qvec, query, 4, None)
        .await
        .expect("hybrid search");
    let top = citations
        .iter()
        .find(|c| c.snippet.contains("NEEDLE-DEEP-1873"))
        .expect("a citation from page 1873");
    assert_offsets_slice_exactly("huge.pdf", &src.content, &top.snippet);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&store);
}

// ---------------------------------------------------------------------------
// Deliberately not tested
// ---------------------------------------------------------------------------

/// The RFC's corpus table also lists an RTL **PDF**. It is not here.
///
/// Every other fixture is synthesized from bytes this repo owns. An RTL PDF
/// cannot be: the base-14 fonts a hand-written PDF can name carry no Hebrew
/// or Arabic glyphs, so the file would have to embed a Unicode font — either
/// a megabyte of binary checked in, or a system font grabbed at test time,
/// which makes the fixture depend on the machine it runs on. Neither is worth
/// it, because the contract the RTL PDF would prove is already proven twice
/// over: `rtl-hebrew-arabic.md` pins RTL chunk boundaries and offsets end to
/// end, and `rotated-columns.pdf` pins the PDF text-extraction and page
/// mapping path. The one thing genuinely left uncovered is RTL *rendering* in
/// the reader, which is a frontend concern and Pillar 4's to verify.
#[test]
fn rtl_is_covered_by_markdown_not_by_pdf() {
    // A test that asserts nothing would be worse than no test. This one
    // asserts the part of the claim that can go stale: that the RTL corpus
    // is actually present and actually RTL.
    let text = std::fs::read_to_string(committed_dir().join("rtl-hebrew-arabic.md"))
        .expect("rtl fixture present");
    assert!(
        text.chars().any(|c| ('\u{0590}'..='\u{05FF}').contains(&c)),
        "fixture must carry Hebrew"
    );
    assert!(
        text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c)),
        "fixture must carry Arabic"
    );
}
