//! First-run example notebooks: two ready-made notebooks seeded once, ever —
//! "Introduction to Alchemy" (the onboarding tour as real, searchable
//! sources) and "Earnings Reports for Top 50 Corporations" (a demo corpus
//! shaped like real research). Same marker-file contract as
//! `ensure_default_brief`: the marker means "offered already," and it is
//! written only after every source lands, so a transient failure (no
//! embedder configured yet) retries next launch while a user deletion never
//! resurrects. Content lives in `src-tauri/examples/` and is compiled in
//! via `include_str!`.
//!
//! Each earnings source carries the company's investor-relations page as its
//! `url` (a stable index that always points at the latest reports — never a
//! per-quarter deep link, those rot), and its body repeats it alongside the
//! SEC EDGAR 10-K index for US filers.

use crate::ai::Ai;
use crate::commands::{app_data_dir, new_id, now, AppState};
use crate::db::{Db, NOTEBOOK_PALETTE};
use crate::ingest;
use crate::models::{Notebook, Source};

pub(crate) const INTRO_TITLE: &str = "Introduction to Alchemy";
pub(crate) const EARNINGS_TITLE: &str = "Earnings Reports for Top 50 Corporations";
pub(crate) const AI_RESEARCH_TITLE: &str = "AI Research: Landmark Papers";

/// (title, url, body) — url is "" for the intro's pasted-text tour and the
/// company's top-level investor-relations page for earnings sources.
type ExampleSource = (&'static str, &'static str, &'static str);

const INTRO_SOURCES: &[ExampleSource] = &[
    (
        "Getting Started",
        "",
        include_str!("../examples/intro/getting-started.md"),
    ),
    (
        "Source Management",
        "",
        include_str!("../examples/intro/source-management.md"),
    ),
    (
        "Chat & Citations",
        "",
        include_str!("../examples/intro/chat-and-citations.md"),
    ),
    (
        "Content Generation (Studio)",
        "",
        include_str!("../examples/intro/content-generation-studio.md"),
    ),
    (
        "Ledger Memory",
        "",
        include_str!("../examples/intro/ledger-memory.md"),
    ),
    (
        "Use Cases & Power Tips",
        "",
        include_str!("../examples/intro/use-cases-and-power-tips.md"),
    ),
];

const EARNINGS_SOURCES: &[ExampleSource] = &[
    (
        "Apple",
        "https://investor.apple.com",
        include_str!("../examples/earnings/apple.md"),
    ),
    (
        "Microsoft",
        "https://www.microsoft.com/en-us/investor",
        include_str!("../examples/earnings/microsoft.md"),
    ),
    (
        "NVIDIA",
        "https://investor.nvidia.com",
        include_str!("../examples/earnings/nvidia.md"),
    ),
    (
        "Alphabet",
        "https://abc.xyz/investor/",
        include_str!("../examples/earnings/alphabet.md"),
    ),
    (
        "Amazon",
        "https://ir.aboutamazon.com",
        include_str!("../examples/earnings/amazon.md"),
    ),
    (
        "Meta Platforms",
        "https://investor.atmeta.com",
        include_str!("../examples/earnings/meta-platforms.md"),
    ),
    (
        "Saudi Aramco",
        "https://www.aramco.com/en/investors",
        include_str!("../examples/earnings/saudi-aramco.md"),
    ),
    (
        "Broadcom",
        "https://investors.broadcom.com",
        include_str!("../examples/earnings/broadcom.md"),
    ),
    (
        "TSMC",
        "https://investor.tsmc.com",
        include_str!("../examples/earnings/tsmc.md"),
    ),
    (
        "Berkshire Hathaway",
        "https://www.berkshirehathaway.com",
        include_str!("../examples/earnings/berkshire-hathaway.md"),
    ),
    (
        "Tesla",
        "https://ir.tesla.com",
        include_str!("../examples/earnings/tesla.md"),
    ),
    (
        "Eli Lilly",
        "https://investor.lilly.com",
        include_str!("../examples/earnings/eli-lilly.md"),
    ),
    (
        "Walmart",
        "https://stock.walmart.com",
        include_str!("../examples/earnings/walmart.md"),
    ),
    (
        "JPMorgan Chase",
        "https://www.jpmorganchase.com/ir",
        include_str!("../examples/earnings/jpmorgan-chase.md"),
    ),
    (
        "Visa",
        "https://investor.visa.com",
        include_str!("../examples/earnings/visa.md"),
    ),
    (
        "Mastercard",
        "https://investor.mastercard.com",
        include_str!("../examples/earnings/mastercard.md"),
    ),
    (
        "Exxon Mobil",
        "https://corporate.exxonmobil.com/investors",
        include_str!("../examples/earnings/exxon-mobil.md"),
    ),
    (
        "Oracle",
        "https://investor.oracle.com",
        include_str!("../examples/earnings/oracle.md"),
    ),
    (
        "UnitedHealth Group",
        "https://www.unitedhealthgroup.com/investors.html",
        include_str!("../examples/earnings/unitedhealth-group.md"),
    ),
    (
        "Johnson & Johnson",
        "https://investor.jnj.com",
        include_str!("../examples/earnings/johnson-and-johnson.md"),
    ),
    (
        "Procter & Gamble",
        "https://www.pginvestor.com",
        include_str!("../examples/earnings/procter-and-gamble.md"),
    ),
    (
        "Costco",
        "https://investor.costco.com",
        include_str!("../examples/earnings/costco.md"),
    ),
    (
        "Home Depot",
        "https://ir.homedepot.com",
        include_str!("../examples/earnings/home-depot.md"),
    ),
    (
        "Netflix",
        "https://ir.netflix.net",
        include_str!("../examples/earnings/netflix.md"),
    ),
    (
        "Bank of America",
        "https://investor.bankofamerica.com",
        include_str!("../examples/earnings/bank-of-america.md"),
    ),
    (
        "AbbVie",
        "https://investors.abbvie.com",
        include_str!("../examples/earnings/abbvie.md"),
    ),
    (
        "Coca-Cola",
        "https://investors.coca-colacompany.com",
        include_str!("../examples/earnings/coca-cola.md"),
    ),
    (
        "Chevron",
        "https://www.chevron.com/investors",
        include_str!("../examples/earnings/chevron.md"),
    ),
    (
        "Merck",
        "https://www.merck.com/investor-relations/",
        include_str!("../examples/earnings/merck.md"),
    ),
    (
        "Samsung Electronics",
        "https://www.samsung.com/global/ir/",
        include_str!("../examples/earnings/samsung-electronics.md"),
    ),
    (
        "Toyota",
        "https://global.toyota/en/ir/",
        include_str!("../examples/earnings/toyota.md"),
    ),
    (
        "ASML",
        "https://www.asml.com/en/investors",
        include_str!("../examples/earnings/asml.md"),
    ),
    (
        "Novo Nordisk",
        "https://www.novonordisk.com/investors.html",
        include_str!("../examples/earnings/novo-nordisk.md"),
    ),
    (
        "LVMH",
        "https://www.lvmh.com/en/investors",
        include_str!("../examples/earnings/lvmh.md"),
    ),
    (
        "Tencent",
        "https://www.tencent.com/en-us/investors.html",
        include_str!("../examples/earnings/tencent.md"),
    ),
    (
        "SAP",
        "https://www.sap.com/investors/en.html",
        include_str!("../examples/earnings/sap.md"),
    ),
    (
        "Nestlé",
        "https://www.nestle.com/investors",
        include_str!("../examples/earnings/nestle.md"),
    ),
    (
        "Salesforce",
        "https://investor.salesforce.com",
        include_str!("../examples/earnings/salesforce.md"),
    ),
    (
        "AMD",
        "https://ir.amd.com",
        include_str!("../examples/earnings/amd.md"),
    ),
    (
        "PepsiCo",
        "https://www.pepsico.com/investors",
        include_str!("../examples/earnings/pepsico.md"),
    ),
    (
        "McDonald's",
        "https://corporate.mcdonalds.com/corpmcd/investors.html",
        include_str!("../examples/earnings/mcdonalds.md"),
    ),
    (
        "Cisco",
        "https://investor.cisco.com",
        include_str!("../examples/earnings/cisco.md"),
    ),
    (
        "Adobe",
        "https://www.adobe.com/investor-relations.html",
        include_str!("../examples/earnings/adobe.md"),
    ),
    (
        "IBM",
        "https://www.ibm.com/investor",
        include_str!("../examples/earnings/ibm.md"),
    ),
    (
        "Alibaba",
        "https://www.alibabagroup.com/en-US/ir",
        include_str!("../examples/earnings/alibaba.md"),
    ),
    (
        "T-Mobile US",
        "https://investor.t-mobile.com",
        include_str!("../examples/earnings/t-mobile.md"),
    ),
    (
        "American Express",
        "https://ir.americanexpress.com",
        include_str!("../examples/earnings/american-express.md"),
    ),
    (
        "GE Aerospace",
        "https://www.geaerospace.com/investor-relations",
        include_str!("../examples/earnings/ge-aerospace.md"),
    ),
    (
        "Linde",
        "https://investors.linde.com",
        include_str!("../examples/earnings/linde.md"),
    ),
    (
        "Roche",
        "https://www.roche.com/investors",
        include_str!("../examples/earnings/roche.md"),
    ),
];

/// Landmark AI papers, oldest ideas first where the lineage matters
/// (attention before the models built on it, scaling laws before the
/// alignment work that assumed them). Each body is a summary written for
/// this notebook; `url` is the paper's arXiv abstract page, which is stable
/// in a way that PDF links and conference URLs are not.
const AI_RESEARCH_SOURCES: &[ExampleSource] = &[
    (
        "Attention Is All You Need",
        "https://arxiv.org/abs/1706.03762",
        include_str!("../examples/ai-research/attention-is-all-you-need.md"),
    ),
    (
        "BERT: Pre-training of Deep Bidirectional Transformers",
        "https://arxiv.org/abs/1810.04805",
        include_str!("../examples/ai-research/bert.md"),
    ),
    (
        "Language Models are Few-Shot Learners (GPT-3)",
        "https://arxiv.org/abs/2005.14165",
        include_str!("../examples/ai-research/gpt-3-few-shot.md"),
    ),
    (
        "Deep Residual Learning for Image Recognition (ResNet)",
        "https://arxiv.org/abs/1512.03385",
        include_str!("../examples/ai-research/resnet.md"),
    ),
    (
        "Adam: A Method for Stochastic Optimization",
        "https://arxiv.org/abs/1412.6980",
        include_str!("../examples/ai-research/adam.md"),
    ),
    (
        "Generative Adversarial Networks",
        "https://arxiv.org/abs/1406.2661",
        include_str!("../examples/ai-research/gan.md"),
    ),
    (
        "Denoising Diffusion Probabilistic Models",
        "https://arxiv.org/abs/2006.11239",
        include_str!("../examples/ai-research/ddpm.md"),
    ),
    (
        "An Image is Worth 16x16 Words (Vision Transformer)",
        "https://arxiv.org/abs/2010.11929",
        include_str!("../examples/ai-research/vit.md"),
    ),
    (
        "Chain-of-Thought Prompting Elicits Reasoning",
        "https://arxiv.org/abs/2201.11903",
        include_str!("../examples/ai-research/chain-of-thought.md"),
    ),
    (
        "LoRA: Low-Rank Adaptation of Large Language Models",
        "https://arxiv.org/abs/2106.09685",
        include_str!("../examples/ai-research/lora.md"),
    ),
    (
        "Retrieval-Augmented Generation for Knowledge-Intensive NLP",
        "https://arxiv.org/abs/2005.11401",
        include_str!("../examples/ai-research/rag.md"),
    ),
    (
        "Scaling Laws for Neural Language Models",
        "https://arxiv.org/abs/2001.08361",
        include_str!("../examples/ai-research/scaling-laws.md"),
    ),
    (
        "Training Language Models to Follow Instructions (InstructGPT)",
        "https://arxiv.org/abs/2203.02155",
        include_str!("../examples/ai-research/instructgpt.md"),
    ),
    (
        "Lost in the Middle: How Language Models Use Long Contexts",
        "https://arxiv.org/abs/2307.03172",
        include_str!("../examples/ai-research/lost-in-the-middle.md"),
    ),
];

/// Seed the example notebooks once, ever. Returns true when anything new
/// landed (so the caller can nudge open windows to refresh). Failures leave
/// the marker unwritten and retry on the next launch; a user deleting the
/// notebooks after the marker exists is final.
pub(crate) async fn ensure_example_notebooks(state: &AppState) -> bool {
    let marker = app_data_dir(state).join("examples-seeded");
    if marker.exists() {
        return false;
    }
    // Clone the configured Ai under a momentary read guard — never held
    // across an await. The chunks table's vector dimensionality is fixed by
    // the first embedder that writes, so this must be the user's configured
    // embedder, not a forced fallback.
    let ai = state.ai.read().await.clone();
    let mut seeded = false;
    for (title, sources) in [
        (INTRO_TITLE, INTRO_SOURCES),
        (EARNINGS_TITLE, EARNINGS_SOURCES),
        (AI_RESEARCH_TITLE, AI_RESEARCH_SOURCES),
    ] {
        match seed_notebook(&state.db, &ai, title, sources).await {
            Ok(created) => seeded |= created,
            Err(err) => {
                // Quiet abort (usually: no embed model reachable yet) —
                // marker stays unwritten so the next launch retries.
                eprintln!("examples: seeding \u{201c}{title}\u{201d} failed ({err:#}); will retry next launch");
                return seeded;
            }
        }
    }
    if let Err(err) = std::fs::write(&marker, b"1") {
        eprintln!("examples: couldn't write marker: {err}");
    }
    seeded
}

/// Seed one example notebook through the real chunk → embed → store path.
/// Skips quietly when a notebook with this title already exists (a partial
/// earlier attempt), so retries never duplicate. Every source is chunked and
/// embedded *before* anything is written, so the common failure — no
/// embedder available on a fresh machine — leaves no half-filled notebook.
async fn seed_notebook(
    db: &Db,
    ai: &Ai,
    title: &str,
    sources: &[ExampleSource],
) -> anyhow::Result<bool> {
    let notebooks = db.list_notebooks().await?;
    if notebooks.iter().any(|n| n.title == title) {
        return Ok(false);
    }

    struct Prepared {
        title: String,
        url: String,
        text: String,
        chunks: Vec<(String, i32, String)>,
        embeddings: Vec<Vec<f32>>,
    }
    let mut prepared = Vec::with_capacity(sources.len());
    for (src_title, url, body) in sources {
        let extracted = ingest::extract_pasted(src_title, body)?;
        let chunks = ingest::chunk_text(&extracted.title, &extracted.text);
        let inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
        let embeddings = ai.embed(&inputs).await?;
        let tuples = chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (new_id(), i as i32, c.text.clone()))
            .collect();
        prepared.push(Prepared {
            title: extracted.title,
            url: url.to_string(),
            text: extracted.text,
            chunks: tuples,
            embeddings,
        });
    }

    let ts = now();
    let color = NOTEBOOK_PALETTE[notebooks.len() % NOTEBOOK_PALETTE.len()];
    let nb = Notebook {
        id: new_id(),
        title: title.to_string(),
        created_at: ts,
        updated_at: ts,
        color: color.to_string(),
        status: String::new(),
        source_count: 0,
    };
    db.create_notebook(&nb).await?;
    for p in prepared {
        let source = Source {
            id: new_id(),
            notebook_id: nb.id.clone(),
            title: p.title,
            source_type: "text".into(),
            url: p.url,
            content: p.text.clone(),
            char_count: p.text.chars().count() as i64,
            chunk_count: p.chunks.len() as i64,
            created_at: ts,
            status: "ready".into(),
            error: String::new(),
            parent_id: String::new(),
            mtime: 0,
            image_url: String::new(),
            author: String::new(),
            tags: String::new(),
            note: String::new(),
        };
        db.insert_source(&source, &p.chunks, &p.embeddings).await?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{Ai, AiConfig, AiRuntime};

    fn words(s: &str) -> usize {
        s.split_whitespace().count()
    }

    /// Content sanity: this ships to every new user, so hold it to the spec —
    /// six intro sources of real length, fifty earnings sources each carrying
    /// the "example data" disclaimer and stable latest-reports links, no
    /// blanks, no duplicate titles.
    #[test]
    fn example_content_is_sane() {
        assert_eq!(INTRO_SOURCES.len(), 6);
        assert_eq!(EARNINGS_SOURCES.len(), 50);

        let mut titles = std::collections::HashSet::new();
        for (title, _, body) in INTRO_SOURCES.iter().chain(EARNINGS_SOURCES) {
            assert!(!title.trim().is_empty());
            assert!(!body.trim().is_empty(), "{title} is empty");
            assert!(titles.insert(*title), "duplicate title {title}");
        }
        for (title, _, body) in INTRO_SOURCES {
            let n = words(body);
            assert!((350..=1000).contains(&n), "{title}: {n} words");
        }
        for (title, url, body) in EARNINGS_SOURCES {
            let n = words(body);
            assert!((120..=360).contains(&n), "{title}: {n} words");
            assert!(
                body.starts_with("Example data:"),
                "{title} missing the example-data disclaimer header"
            );
            // Latest-reports contract: a stable IR index url on the source
            // row, repeated in the body (with EDGAR for US filers).
            assert!(
                url.starts_with("https://"),
                "{title} missing an investor-relations url"
            );
            assert!(
                body.contains("Latest reports:") && body.contains(url),
                "{title} body missing the latest-reports line"
            );
        }
    }

    /// Retry-idempotence: a notebook whose title already exists is skipped
    /// before any embedding or writing happens, so a partially-failed first
    /// attempt can never duplicate on the next launch.
    #[tokio::test]
    async fn seed_skips_existing_title() {
        let dir = std::env::temp_dir().join(format!("nbl-examples-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");
        let ts = now();
        let nb = Notebook {
            id: new_id(),
            title: INTRO_TITLE.to_string(),
            created_at: ts,
            updated_at: ts,
            color: NOTEBOOK_PALETTE[0].to_string(),
            status: String::new(),
            source_count: 0,
        };
        db.create_notebook(&nb).await.expect("create notebook");

        // The Ai is never invoked on the skip path, so a bare default is fine
        // (an embed attempt against it would error, which the assert catches).
        let ai = Ai::new(AiConfig::default(), AiRuntime::default());
        let created = seed_notebook(&db, &ai, INTRO_TITLE, INTRO_SOURCES)
            .await
            .expect("skip path never errors");
        assert!(!created);
        let notebooks = db.list_notebooks().await.expect("list");
        assert_eq!(notebooks.len(), 1, "no duplicate notebook created");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
