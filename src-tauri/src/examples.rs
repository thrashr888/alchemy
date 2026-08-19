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
use crate::models::{Notebook, RegistryCard, Source};

pub(crate) const INTRO_TITLE: &str = "Introduction to Alchemy";
pub(crate) const EARNINGS_TITLE: &str = "Earnings Reports for Top 50 Corporations";
pub(crate) const AI_RESEARCH_TITLE: &str = "AI Research: Landmark Papers";

/// (title, url, body) — url is "" for the intro's pasted-text tour and the
/// company's top-level investor-relations page for earnings sources.
type ExampleSource = (&'static str, &'static str, &'static str);

/// (kind, name) — example registry cards (docs/RFC-registry.md), so a fresh
/// install's Registry shows a real cast instead of a blank page.
///
/// Every name here appears verbatim in the seeded sources, because these are
/// filed by the ordinary matcher rather than wired up by hand: seeding runs
/// `match_source_to_cards` and then confirms the name matches it made. The
/// receipts a new user sees ("name matched") are therefore true, and the
/// mechanism demonstrates itself.
///
/// Note which notebook contributes what. The earnings corpus is full of
/// *providers* — companies are exactly the thing documents accumulate about.
/// The AI-research corpus offers *threads* rather than objects, so its cards
/// are projects. The papers themselves are deliberately NOT cards: a
/// document is not a thing a document is about.
type ExampleCard = (&'static str, &'static str);

const EXAMPLE_CARDS: &[ExampleCard] = &[
    ("project", "Alchemy"),
    ("provider", "Apple"),
    ("provider", "Microsoft"),
    ("provider", "NVIDIA"),
    ("provider", "Alphabet"),
    ("provider", "Amazon"),
    ("project", "Retrieval-Augmented Generation"),
    ("project", "Scaling Laws"),
];

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
///
/// Additions land on existing installs through [`top_up_ai_research`], which
/// inserts by-title-missing entries into an already-seeded notebook — append
/// or insert here and bump [`EXAMPLES_VERSION`] so seeded installs get one
/// top-up pass. Two asked-for topics are deliberately mapped sideways:
/// "KV cache" has no standalone paper (multi-query attention IS its
/// literature), and A2A is a protocol spec, not a paper, so it is absent.
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
        "Auto-Encoding Variational Bayes (VAE)",
        "https://arxiv.org/abs/1312.6114",
        include_str!("../examples/ai-research/vae.md"),
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
        "CLIP: Learning Transferable Visual Models From Natural Language Supervision",
        "https://arxiv.org/abs/2103.00020",
        include_str!("../examples/ai-research/clip.md"),
    ),
    (
        "BLIP: Bootstrapping Language-Image Pre-training",
        "https://arxiv.org/abs/2201.12086",
        include_str!("../examples/ai-research/blip.md"),
    ),
    (
        "Sparsely-Gated Mixture-of-Experts",
        "https://arxiv.org/abs/1701.06538",
        include_str!("../examples/ai-research/moe.md"),
    ),
    (
        "RoFormer: Rotary Position Embedding (RoPE)",
        "https://arxiv.org/abs/2104.09864",
        include_str!("../examples/ai-research/rope.md"),
    ),
    (
        "Multi-Query Attention: One Write-Head is All You Need",
        "https://arxiv.org/abs/1911.02150",
        include_str!("../examples/ai-research/multi-query-attention.md"),
    ),
    (
        "FlashAttention: Fast and Memory-Efficient Exact Attention",
        "https://arxiv.org/abs/2205.14135",
        include_str!("../examples/ai-research/flashattention.md"),
    ),
    (
        "Chain-of-Thought Prompting Elicits Reasoning",
        "https://arxiv.org/abs/2201.11903",
        include_str!("../examples/ai-research/chain-of-thought.md"),
    ),
    (
        "ReAct: Synergizing Reasoning and Acting in Language Models",
        "https://arxiv.org/abs/2210.03629",
        include_str!("../examples/ai-research/react.md"),
    ),
    (
        "Adapters: Parameter-Efficient Transfer Learning for NLP",
        "https://arxiv.org/abs/1902.00751",
        include_str!("../examples/ai-research/adapters.md"),
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
        "GraphRAG: From Local to Global",
        "https://arxiv.org/abs/2404.16130",
        include_str!("../examples/ai-research/graphrag.md"),
    ),
    (
        "Scaling Laws for Neural Language Models",
        "https://arxiv.org/abs/2001.08361",
        include_str!("../examples/ai-research/scaling-laws.md"),
    ),
    (
        "Deep Reinforcement Learning from Human Preferences (RLHF)",
        "https://arxiv.org/abs/1706.03741",
        include_str!("../examples/ai-research/rlhf.md"),
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
/// Bump when the example content grows, so installs seeded by an older build
/// get one [`top_up_ai_research`] pass. The marker file's CONTENT carries the
/// version; the original release wrote "1".
const EXAMPLES_VERSION: &str = "2";

pub(crate) async fn ensure_example_notebooks(state: &AppState) -> bool {
    let marker = app_data_dir(state).join("examples-seeded");
    if let Ok(v) = std::fs::read_to_string(&marker) {
        if v.trim() == EXAMPLES_VERSION {
            return false;
        }
        // Seeded by an older build: add what that build didn't have, without
        // re-offering anything the user deleted. Failure (embedder not up
        // yet) leaves the marker at the old version so the next launch
        // retries, same contract as first seeding.
        let ai = state.ai.read().await.clone();
        let added = match top_up_ai_research(&state.db, &ai).await {
            Ok(n) => n > 0,
            Err(err) => {
                eprintln!("examples: papers top-up failed ({err:#}); will retry next launch");
                return false;
            }
        };
        if let Err(err) = std::fs::write(&marker, EXAMPLES_VERSION) {
            eprintln!("examples: couldn't write marker: {err}");
        }
        return added;
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
    if let Err(err) = seed_registry_cards(&state.db).await {
        // Same contract as the notebooks: leave the marker unwritten so the
        // next launch retries, rather than shipping a half-built cast.
        eprintln!("examples: seeding registry cards failed ({err:#}); will retry next launch");
        return seeded;
    }
    if let Err(err) = std::fs::write(&marker, EXAMPLES_VERSION) {
        eprintln!("examples: couldn't write marker: {err}");
    }
    seeded
}

/// Insert papers this build knows and the seeded notebook lacks (matched by
/// title). Touches nothing else: a deleted notebook stays deleted (`Ok(0)`),
/// and a paper the user removed only returns if a LATER version adds it back
/// under this mechanism — which only ever runs once per version bump.
async fn top_up_ai_research(db: &Db, ai: &Ai) -> anyhow::Result<usize> {
    let notebooks = db.list_notebooks().await?;
    let Some(nb) = notebooks.iter().find(|n| n.title == AI_RESEARCH_TITLE) else {
        return Ok(0);
    };
    let have: std::collections::HashSet<String> = db
        .list_sources(&nb.id)
        .await?
        .into_iter()
        .map(|s| s.title)
        .collect();
    let missing: Vec<&ExampleSource> = AI_RESEARCH_SOURCES
        .iter()
        .filter(|(title, _, _)| !have.contains(*title))
        .collect();
    if missing.is_empty() {
        return Ok(0);
    }
    // Embed everything before writing anything, like first seeding: a
    // mid-list embedder failure must not leave a half-topped-up notebook
    // behind a marker that says the work is done.
    let mut prepared = Vec::with_capacity(missing.len());
    for (src_title, url, body) in missing {
        prepared.push(prepare_source(ai, src_title, url, body).await?);
    }
    let n = prepared.len();
    let ts = now();
    for p in prepared {
        insert_prepared(db, &nb.id, ts, p).await?;
    }
    eprintln!("examples: added {n} papers to \u{201c}{AI_RESEARCH_TITLE}\u{201d}");
    Ok(n)
}

/// Seed the example cast, then let the ordinary matcher file it.
///
/// Cards whose name already exists are skipped, so this is idempotent across
/// retries and never fights a card the user made themselves.
///
/// What gets confirmed is the point. A name match is a guess, and blanket-
/// confirming every one of them would hand a new user a lie: "Apple" is
/// named in half the earnings corpus, so the Apple card would claim nine
/// documents when one is Apple's report and eight are rivals mentioning it.
/// So only the document whose TITLE carries the card's name is confirmed —
/// that one really is about the thing — and the rest stay proposed. A fresh
/// install therefore shows both halves of the mechanism: a card with its own
/// documents, and a queue of guesses waiting on a human.
async fn seed_registry_cards(db: &Db) -> anyhow::Result<()> {
    let existing = db.list_registry().await?;
    let mut seeded_ids: Vec<String> = Vec::new();
    for (kind, name) in EXAMPLE_CARDS {
        if existing.iter().any(|c| c.name.eq_ignore_ascii_case(name)) {
            continue;
        }
        let ts = now();
        let card = RegistryCard {
            id: new_id(),
            kind: (*kind).to_string(),
            name: (*name).to_string(),
            origin: String::new(),
            identifiers: String::new(),
            note: String::new(),
            facts: Vec::new(),
            attachments: Vec::new(),
            created_at: ts,
            updated_at: ts,
        };
        db.add_registry_card(&card).await?;
        seeded_ids.push(card.id);
    }
    if seeded_ids.is_empty() {
        return Ok(());
    }
    for nb in db.list_notebooks().await? {
        if ![INTRO_TITLE, EARNINGS_TITLE, AI_RESEARCH_TITLE].contains(&nb.title.as_str()) {
            continue;
        }
        for s in db.list_sources(&nb.id).await? {
            let Ok(text) = db.source_content(&s.id).await else {
                continue;
            };
            crate::commands::match_source_to_cards(db, &nb.id, &s.id, &text).await;
        }
    }
    for id in seeded_ids {
        let Some(mut card) = db.get_registry_card(&id).await? else {
            continue;
        };
        let name = card.name.to_lowercase();
        let mut touched = false;
        for a in card.attachments.iter_mut() {
            if a.status != "proposed" {
                continue;
            }
            let titled_for_it = match db.get_source(&a.source_id).await {
                Ok(Some(src)) => src.title.to_lowercase().contains(&name),
                _ => false,
            };
            if titled_for_it {
                a.status = "confirmed".into();
                touched = true;
            }
        }
        if touched {
            card.updated_at = now();
            db.update_registry_card(&card).await?;
        }
    }
    // Matching announced itself, but the confirm pass above runs after it —
    // without this an already-open window keeps the pre-confirm snapshot and
    // every seeded card reads "0 documents".
    crate::commands::notify_changed("registry", None);
    Ok(())
}

/// Seed one example notebook through the real chunk → embed → store path.
/// Skips quietly when a notebook with this title already exists (a partial
/// earlier attempt), so retries never duplicate. Every source is chunked and
/// embedded *before* anything is written, so the common failure — no
/// embedder available on a fresh machine — leaves no half-filled notebook.
struct Prepared {
    title: String,
    url: String,
    text: String,
    chunks: Vec<(String, i32, String)>,
    embeddings: Vec<Vec<f32>>,
}

/// Extract, chunk, and embed one example source, ready to insert.
async fn prepare_source(
    ai: &Ai,
    src_title: &str,
    url: &str,
    body: &str,
) -> anyhow::Result<Prepared> {
    let extracted = ingest::extract_pasted(src_title, body)?;
    let chunks = ingest::chunk_text(&extracted.title, &extracted.text);
    let inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let embeddings = ai.embed(&inputs).await?;
    let tuples = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (new_id(), i as i32, c.text.clone()))
        .collect();
    Ok(Prepared {
        title: extracted.title,
        url: url.to_string(),
        text: extracted.text,
        chunks: tuples,
        embeddings,
    })
}

/// Write one prepared source into a notebook.
async fn insert_prepared(db: &Db, notebook_id: &str, ts: i64, p: Prepared) -> anyhow::Result<()> {
    let source = Source {
        id: new_id(),
        notebook_id: notebook_id.to_string(),
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
    db.insert_source(&source, &p.chunks, &p.embeddings).await
}

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

    let mut prepared = Vec::with_capacity(sources.len());
    for (src_title, url, body) in sources {
        prepared.push(prepare_source(ai, src_title, url, body).await?);
    }

    let ts = now();
    let color = NOTEBOOK_PALETTE[notebooks.len() % NOTEBOOK_PALETTE.len()];
    let nb = Notebook {
        id: new_id(),
        title: title.to_string(),
        created_at: ts,
        updated_at: ts,
        color: color.to_string(),
        icon: String::new(),
        status: String::new(),
        source_count: 0,
        note_count: 0,
        report_count: 0,
    };
    db.create_notebook(&nb).await?;
    for p in prepared {
        insert_prepared(db, &nb.id, ts, p).await?;
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
        // The papers notebook: unique titles, arXiv abstract urls (the
        // stable form), bodies in the house shape with the url repeated in
        // the "Read the paper" footer, at essay length not stub length.
        assert_eq!(AI_RESEARCH_SOURCES.len(), 25);
        for (title, url, body) in AI_RESEARCH_SOURCES {
            assert!(!title.trim().is_empty());
            assert!(titles.insert(*title), "duplicate title {title}");
            assert!(
                url.starts_with("https://arxiv.org/abs/"),
                "{title}: url should be an arXiv abstract page, got {url}"
            );
            let n = words(body);
            assert!((120..=280).contains(&n), "{title}: {n} words");
            assert!(
                body.contains(&format!("Read the paper: {url}")),
                "{title} body missing its read-the-paper footer"
            );
            assert!(
                body.lines().next().unwrap_or("").contains("arXiv:"),
                "{title} first line missing its arXiv id"
            );
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
            icon: String::new(),
            status: String::new(),
            source_count: 0,
            note_count: 0,
            report_count: 0,
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

    /// The additive path: an install seeded before this build's papers
    /// existed gets exactly the missing ones — and a deleted notebook stays
    /// deleted. Uses the built-in embedder (CI-safe; skips offline).
    #[tokio::test]
    async fn top_up_adds_only_missing_papers() {
        let Some(ai) = crate::evals::builtin_ai().await else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("nbl-topup-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.expect("open db");

        // No papers notebook at all: deletion is final, top-up is a no-op.
        let n = top_up_ai_research(&db, &ai).await.expect("no-op path");
        assert_eq!(n, 0, "a deleted notebook must not be resurrected");

        // A v1 install: the notebook exists holding only the first paper.
        let first = &AI_RESEARCH_SOURCES[0];
        seed_notebook(&db, &ai, AI_RESEARCH_TITLE, std::slice::from_ref(first))
            .await
            .expect("seed one-paper notebook");

        let added = top_up_ai_research(&db, &ai).await.expect("top-up");
        assert_eq!(added, AI_RESEARCH_SOURCES.len() - 1);

        let nb = db
            .list_notebooks()
            .await
            .expect("list")
            .into_iter()
            .find(|n| n.title == AI_RESEARCH_TITLE)
            .expect("papers notebook");
        let titles: std::collections::HashSet<String> = db
            .list_sources(&nb.id)
            .await
            .expect("sources")
            .into_iter()
            .map(|s| s.title)
            .collect();
        for (title, _, _) in AI_RESEARCH_SOURCES {
            assert!(titles.contains(*title), "missing {title} after top-up");
        }

        // Running again adds nothing — by-title idempotence.
        let again = top_up_ai_research(&db, &ai).await.expect("idempotent");
        assert_eq!(again, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
