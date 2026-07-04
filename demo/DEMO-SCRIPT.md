# DadgarCorp Demo Script

Demo persona: **Maya Chen, PM for Timberline** (DadgarCorp's internal developer platform), prepping the Timberline 2.0 launch. Eight interlocking sources: roadmap, customer research, metrics, pricing, a draft press release, competitive intel, support escalations, and engineering capacity notes.

## Setup (2 minutes)

1. Create a notebook: **Timberline 2.0 Launch**.
2. Drag all 8 files from `demo/dadgarcorp/` onto the Sources panel in one go — the queue processing makes a good shot itself.
   - The 7 `.md` files take their titles from their headings; the `.txt` file (`eng_capacity_h2_notes.txt`) gets an LLM-generated title — that's the smart-titling feature.
3. Optional: add one real URL source (any blog post) so the list shows a favicon + hostname row.
4. Click **Generate notebook summary** above the chat for the summary banner.
5. For a fuller Home screen, make 1–2 extra notebooks (e.g. **Competitive Intel** with just the competitive + pricing docs — duplicate detection is per-notebook, so re-importing there is fine).

## Screenshot flows

### 1. Grounded chat with citations (the hero shot)
Ask: **"What's blocking our enterprise deals, and how much revenue is at stake?"**
Expect an answer citing $2.4M / 9 opportunities from the metrics review and roadmap, with clickable [n] chips. Click a chip → the source reader opens with the passage highlighted (second hero shot). Good follow-ups (the suggested-follow-up chips now fill the composer — also screenshottable):
- "What did customers say about environment provisioning?"
- "Why did Brightbeam churn?"
- "What would Meridian Bank be worth if we ship on time?"

### 2. Deep research (step trail)
Toggle **Deep research: on** and ask: **"Build the case for and against holding the September 15 GA date."**
The step trail (search → read → answer) renders while it works. The honest answer spans the roadmap, capacity notes (no RC→GA buffer, migration tooling six weeks late), metrics (Meridian's October deadline), and support data.

### 3. Problems generator (planted conflicts — let it find them)
Studio → **Problems**. The corpus contains real, discoverable contradictions:
- **Pro pricing:** $79/dev/mo (pricing strategy) vs **$69** (press release draft)
- **GA date:** September 15 (roadmap, metrics review) vs **October 1** (press release draft)
- **Strategic tension:** SSO is Enterprise-only (pricing), but support data shows permissions pain concentrated in Team-tier accounts, and Brightbeam churned over SSO
- **Schedule risk:** roadmap says approved plan; capacity notes say migration tooling started six weeks late with zero RC→GA buffer

### 4. Documents for PMs
- **PRD** with instructions: `PRD for Environment Blueprints GA, grounded in the customer interview evidence`
- **PR/FAQ**: it will synthesize the draft press release + interviews (HashiCorp-style artifact, streams live in the preview modal — screenshot mid-stream)
- **Briefing**: clean exec-ready overview shot
- **Timeline**: the roadmap + incident dates give it real material

### 5. Reports
Schedule a **Weekly launch briefing** (Briefing, weekly) so the Reports section looks alive, then **Run now** for a timestamped note.

## Good chat questions, by feature

| To show | Ask |
| --- | --- |
| Numeric grounding | "What's our NRR and how did it change from Q1?" |
| Cross-source synthesis | "Which customers should be launch references and why?" |
| Honest no-answer | "What's our Q4 revenue target?" (not in the sources) |
| Contradiction catch | "What price is the Pro tier?" (sources disagree — it should surface both) |
| Source manifest | "What documents do I have in this notebook?" |

## Cast (for consistency if you extend the corpus)

Priya Raman (CPO) · Maya Chen (PM, demo persona) · Elena Sokolov (VP Eng) · Dana Ferreira (RevOps) · Danny Okafor (Sales) · Rosa Delgado (Support) · Theo Lindqvist (PMM) · Jules Park (Comms). Customers: Meridian Bank, Cascade Health, Vantage Logistics, Orbital Media, Northwind Grid. Competitors: Backstage, Corteza, EnvoyDeck.
