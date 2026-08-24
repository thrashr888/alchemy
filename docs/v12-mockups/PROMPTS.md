# Alchemy V12 mockup prompts

These mockups were created with the built-in image-generation tool. The supplied
Alchemy screenshot was used as the visual and layout reference.

## 1. The Verified Draft

```text
Use case: ui-mockup
Asset type: high-fidelity desktop macOS Tauri app product mockup
Input images: Image 1 is the strict visual and layout reference for the existing Alchemy app. Create a new screen in the same product; do not reproduce Image 1's chat content.
Primary request: Create “Version 1 — The Verified Draft,” a shippable Alchemy V12 Reader screen showing sentence-level adversarial audit of a finished investigative report.
Scene/backdrop: full 1200×768-style app window, same macOS title bar and three-pane Alchemy workspace as Image 1.
Style/medium: realistic product UI mockup, not concept art. Preserve the existing dark plum/lavender theme, rounded side panels, compact SF Pro-like typography, faint hairline dividers, turquoise links, lavender primary controls, and magenta activity accents.
Composition/framing: keep the current proportions: narrow Sources panel left, wide Reader document center, contextual rail right.
Subject: notebook title “Northstar Records Inquiry”. Reader tab selected. Left SOURCES list includes “Board minutes — March 4”, “Cost forecast — Q1”, “Interview — Dana Lee”, “Procurement email archive”, and other plausible records, with small source icons and checkmarks.
Center: polished report titled “Procurement chronology”. At the top show exactly “14 claims, 13 verified, 1 flagged.” with a restrained segmented status line. Render readable report paragraphs with small numbered citation pills. Quiet teal check marks sit in the margin beside supported sentences. One selected sentence is underlined in coral and reads exactly: “The agency first learned of the $18.4M overrun on March 14.” Attach a small “Contradicted” badge. Add a compact bottom action “Export with evidence”.
Right rail: header exactly “AUDIT”. Show “CONTRADICTED” and “Verified independently”. Show independently retrieved evidence cards titled “Board minutes — March 4 · p. 6” and “Interview transcript · 42:18”. Include actions “Open evidence”, “Revise sentence”, “Search trace”, “Re-run Audit”, and “Export with evidence”. Top notebook header includes small chips “Local only” and “$0.18”.
Color semantics: supported = restrained teal; contradicted/flagged = coral-magenta; neutral controls = lavender and gray.
Constraints: text must be readable and correctly spelled, especially all quoted strings. Keep status icon plus text, never color alone. No gradients, no glow, no giant cards, no dashboard KPI wall, no graphs, no knowledge graph, no avatars, no teams, no enterprise/admin UI, no cloud-control-room aesthetic, no watermark.
```

## 2. The Living Case File

```text
Use case: ui-mockup
Asset type: high-fidelity desktop macOS Tauri app product mockup
Input images: Image 1 is the strict visual and layout reference for the existing Alchemy app. Create a new screen in the same product; do not reproduce Image 1's chat content.
Primary request: Create “Version 2 — The Living Case File,” a shippable Alchemy V12 Claim Ledger screen with an active contradiction.
Scene/backdrop: full 1200×768-style app window, same macOS title bar and three-pane Alchemy workspace as Image 1.
Style/medium: realistic product UI mockup, not concept art. Preserve the existing dark plum/lavender theme, rounded side panels, compact SF Pro-like typography, faint hairline dividers, turquoise links, lavender primary controls, and magenta activity accents.
Composition/framing: keep the current proportions: narrow Sources panel left, wide main workspace center, contextual rail right.
Subject: notebook title “Northstar Records Inquiry”. Top center navigation reads exactly “Chat   Reader   Ledger”, with Ledger selected. Left SOURCES list includes “Board minutes — March 4”, “Cost forecast — Q1”, “Interview — Dana Lee”, “Procurement email archive”, and other plausible records, with small source icons and checkmarks.
Center: title “Claim Ledger”. Compact filters “All”, “Corroborated”, “Contradicted”, “Stale”. A dense thin-rule list of four professional claim rows, not oversized cards. Selected claim reads exactly “The agency first learned of the $18.4M overrun on March 14.” with a coral “Contradicted” status, confidence “91%”, and “5 anchors”. Other rows visibly demonstrate “Corroborated”, “Stale”, and “Asserted” states using icon plus text. Below or within the selected detail show lifecycle “Asserted → Corroborated → Contradicted”, two anchored passages, and the exact assurance “Verified against cited revision”. Actions: “Export with evidence”, “Open in Reader”, “Run Audit”.
Right rail: header exactly “CROSS-EXAMINATION”. Stack three compact evidence sections labeled “AGREES”, “CONTRADICTS”, and “REFINES”. The CONTRADICTS passage is dominant and cites “Board minutes — March 4 · p. 6”; REFINES cites “Interview transcript · 42:18”. Add a collapsed “Search trace” disclosure and a bottom field “Ask about this claim…”.
Color semantics: corroborated = restrained teal; contradicted = coral-magenta; stale = amber; asserted = lavender.
Constraints: text must be readable and correctly spelled, especially all quoted strings. Keep status icon plus text, never color alone. Keep a quiet legal/research instrument feel, not a spreadsheet and not a graph. No gradients, no glow, no giant cards, no KPI dashboard, no knowledge graph, no evidence-board canvas, no avatars, no teams, no enterprise/admin UI, no watermark.
```

## 3. The Morning Desk

```text
Use case: ui-mockup
Asset type: high-fidelity desktop macOS Tauri app product mockup
Input images: Image 1 is the strict visual and layout reference for the existing Alchemy app. Create a new screen in the same product; do not reproduce Image 1's chat content.
Primary request: Create “Version 3 — The Morning Desk,” a shippable Alchemy V12 Morning Brief screen after an overnight Night Shift.
Scene/backdrop: full 1200×768-style app window, same macOS title bar and three-pane Alchemy workspace as Image 1.
Style/medium: realistic product UI mockup, not concept art. Preserve the existing dark plum/lavender theme, rounded side panels, compact SF Pro-like typography, faint hairline dividers, turquoise links, lavender primary controls, and magenta activity accents.
Composition/framing: keep the current proportions: narrow Sources panel left, wide Reader artifact center, contextual rail right.
Subject: notebook title “Northstar Records Inquiry”. Left SOURCES shows watched sources with subtle “Updated” badges, one amber health warning, and one audio transcript source with timestamp icon.
Center Reader: header “Morning Brief · July 31”, small line “Night Shift completed 6:42 AM”, prominent but compact heading “3 decisions need you”, and button “Play 7 min brief”. Rank the artifact as “NEEDS YOUR DECISION”, “REVIEW TODAY”, and “FOR THE RECORD”. First decision card says the March 4 board minutes contradict the report’s March 14 awareness date, with actions “Review claim” and “Defer”. Show smaller sections “What changed overnight”, “Audit flags”, and “Librarian proposals” with concise rows, never a metrics dashboard.
Right rail: header exactly “NIGHT SHIFT”. Show a completed run at “6:42 AM”, one “Standing Question triggered”, a short activity list for sources changed and claims checked, two Librarian proposals with “Review” buttons, cost line “Tonight $0.84 · Month $12.40 / $25”, and the exact privacy assurance “This notebook never leaves the machine” beside a lock icon. Include a small next-run line “Tonight · 2:00 AM”.
Color semantics: decisions/contradictions = restrained coral-magenta; completed = teal; warnings = amber; primary actions = lavender.
Constraints: text must be readable and correctly spelled, especially all quoted strings. Keep status icon plus text, never color alone. Make it feel like a calm chief-of-staff brief, not an operations center. No gradients, no glow, no giant KPI cards, no charts, no graphs, no agent avatars, no teams, no permissions/admin UI, no autonomous outbound actions, no watermark.
```

## Shared Synthwave ’84 correction

The first pass was structurally successful but drifted toward Alchemy’s Midnight
theme. This targeted edit was applied to each final image:

```text
Use case: precise-object-edit
Input images: Image 1 is the edit target, a finished Alchemy V12 UI mockup.
Primary request: Change only the visual theme tokens and side-panel geometry so this looks exactly like the supplied current Alchemy app in its Synthwave ’84 theme.
Exact palette: canvas #1e1a29; surface #262335; surface-2 #2a2139; elevated #34294f; floating side-card fill approximately #2e2b3d; foreground #f4f2ff; muted text #9096c3; primary and focus #ff7edb; citation and links #36f9f6; success #72f1b8; destructive #fe4a56; borders subtle lavender rgba(176,132,235,0.16), strong rgba(176,132,235,0.26).
Geometry: keep the center workspace unboxed like a sheet of paper. Make only the left and right rails floating 12px-radius side cards with an 8px outer inset and a small gap from the center, matching the current Alchemy screenshot. Retain the 48px macOS overlay titlebar and compact density.
Constraints: preserve every word, number, label, icon, status, document passage, source row, control, spacing relationship, column width, and overall composition exactly. Do not add, remove, rewrite, or misspell any text. Change only palette and side-card container geometry. No gradients, no glow, no blur, no colored left-border decoration, no watermark.
```

## Night Shift area set (10–12)

Generated via `codex exec` with the built-in ImageGen tool;
09-steward-staff.png was the strict style and geometry reference for all three.

### 10. Tonight

```text
Use your image generation tool to create one PNG mockup. Read the reference image first.

Input image: docs/v12-mockups/09-steward-staff.png is the STRICT visual, palette, and layout reference — same product, same theme, same three-pane geometry. Create a new screen in the same product; do not reproduce its content.

Save the result to: docs/v12-mockups/10-night-shift-tonight.png

Use case: ui-mockup
Asset type: high-fidelity desktop macOS Tauri app product mockup, 1568x1003, realistic product UI, not concept art.
Primary request: Create "Night Shift · Tonight" — the commissioning desk of a new top-level Night Shift area. The user leaves one-off overnight jobs for the resident staff before bed.
Scene: full app window, macOS overlay title bar. Top-center segmented navigation reads exactly "Notebooks   Registry   Night Shift" with Night Shift selected.
Palette (Synthwave '84): canvas #1e1a29; surface #262335; elevated #34294f; floating side cards #2e2b3d with 12px radius; foreground #f4f2ff; muted #9096c3; primary/focus #ff7edb; links #36f9f6; success #72f1b8; amber warnings; borders subtle lavender rgba(176,132,235,0.16). Center workspace unboxed like a sheet of paper; left and right rails are floating side cards.
Left rail: header exactly "NIGHT SHIFT". Nav items with small icons: "Tonight" (selected, dot), "Standing orders · 7", "The record". Below, a small footer line "Resident · this Mac · next pass 2:00 AM".
Center: kicker line "TONIGHT · BEGINS 2:00 AM", heading exactly "The plan for tonight", right-aligned quiet button "Pause until morning".
Section "COMMISSIONED" with three dense flat rows (thin rules, not cards):
1. "Deep read — Japan 2027" with lavender chip "COMMISSION", sub-line "Read all 42 sources, rebuild the Growing Answer with a since-last-time delta." and right-aligned "est. $0.40 · Remove".
2. "Second Look — RFC: import pipeline" chip "COMMISSION", sub-line "Re-verify 22 claims with fresh retrieval, different engine." right "est. $0.25 · Remove".
3. "Re-gist Health & Insurance" chip "COMMISSION", sub-line "Distill 118 sources; sealed, local only." right "$0.00 · local · Remove".
Section "DUE ON SCHEDULE" with two rows: "Release brief — Alchemy Development · daily · 2:10 AM" and "Pricing page watch — Market Research · every 4 hours".
Bottom of center: a single prominent input field with placeholder exactly "Commission overnight work…" and a small send button, above it one quiet line exactly "Proposals and notes only. It will not act outward."
Right rail: header exactly "TONIGHT'S BUDGET". Rows: "Wall clock" "until 6:30 AM"; "Steps" "120"; "Metered spend cap" "$1.00" with a thin progress bar at zero; note line exactly "At cap: degrades to local". Below, small header "SEALED" with rows "Health & Insurance · never leaves this Mac" (green lock icon) and "Money · never leaves this Mac". Bottom small header "NEXT" with "First run · Deep read — Japan 2027 · 2:00 AM".
Constraints: all text readable and correctly spelled, especially quoted strings. Icon plus text for every status, never color alone. Compact SF Pro-like typography, hairline dividers, no gradients, no glow, no KPI cards, no charts, no avatars, no admin UI, no watermark, no colored left borders.
```

### 11. Standing orders

```text
Use your image generation tool to create one PNG mockup. Read the reference image first.

Input image: docs/v12-mockups/09-steward-staff.png is the STRICT visual, palette, and layout reference — same product, same theme, same three-pane geometry. Create a new screen in the same product; do not reproduce its content.

Save the result to: docs/v12-mockups/11-night-shift-standing-orders.png

Use case: ui-mockup
Asset type: high-fidelity desktop macOS Tauri app product mockup, 1568x1003, realistic product UI, not concept art.
Primary request: Create "Night Shift · Standing orders" — the cross-notebook index of everything the user has commissioned to recur: scheduled reports, watchers, and standing questions, each a first-class object.
Scene: full app window, macOS overlay title bar. Top-center segmented navigation reads exactly "Notebooks   Registry   Night Shift" with Night Shift selected.
Palette (Synthwave '84): canvas #1e1a29; surface #262335; elevated #34294f; floating side cards #2e2b3d with 12px radius; foreground #f4f2ff; muted #9096c3; primary/focus #ff7edb; links #36f9f6; success #72f1b8; amber warnings; borders subtle lavender rgba(176,132,235,0.16). Center workspace unboxed like a sheet of paper; left and right rails are floating side cards.
Left rail: header exactly "NIGHT SHIFT". Nav items: "Tonight", "Standing orders · 7" (selected, dot), "The record". Footer line "Resident · this Mac · next pass 2:00 AM".
Center: heading exactly "Standing orders", small filter chips "All · Reports · Watchers · Questions", quiet right-aligned button "New standing order".
Group header "REPORTS" with three dense flat rows (thin rules, not cards), each with name, notebook, cadence, last run, next run:
1. "Release brief" · "Alchemy Development · daily" · "ran 2:10 AM · $0.19" · "next 2:10 AM" — selected row, subtle highlight.
2. "Sunday household brief" · "Home · weekly" · "ran Sun 7:00 AM · $0.00 local".
3. "Shipping record" · "Alchemy Development · Fridays" · "ran Fri 6:00 PM · $0.31".
Group header "WATCHERS" with two rows: "Pricing page changes" · "Market Research · every 4 hours" · green dot "quiet 3 days"; "LanceDB releases" · "Alchemy Development · daily" · magenta chip "CHANGED" · "diff waiting in the Brief".
Group header "STANDING QUESTIONS" with two rows: "When the 10-K drops, what changed?" · "Investments · on change" · "armed"; "New papers that complicate the 2024 synthesis" · "Coffee Science · weekly" · "armed".
Right rail (detail of the selected order): header exactly "RELEASE BRIEF". Lines: "Report · Alchemy Development", "Daily · 2:10 AM · prior runs threaded". Small header "LAST 5 RUNS" with five short rows like "Thu · 2:10 AM · $0.19 · 8 citations", "Wed · 2:10 AM · $0.22 · 11 citations". Small header "PRODUCES" with "Note: Release brief · unread dot". Buttons "Run now", "Pause", "Edit". Bottom quiet line exactly "Writes notes only. It will not act outward."
Constraints: all text readable and correctly spelled, especially quoted strings. Icon plus text for every status, never color alone. Compact SF Pro-like typography, hairline dividers, no gradients, no glow, no KPI cards, no charts, no avatars, no admin UI, no watermark, no colored left borders.
```

### 12. The record

```text
Use your image generation tool to create one PNG mockup. Read the reference image first.

Input image: docs/v12-mockups/09-steward-staff.png is the STRICT visual, palette, and layout reference — same product, same theme, same three-pane geometry. Create a new screen in the same product; do not reproduce its content.

Save the result to: docs/v12-mockups/12-night-shift-record.png

Use case: ui-mockup
Asset type: high-fidelity desktop macOS Tauri app product mockup, 1568x1003, realistic product UI, not concept art.
Primary request: Create "Night Shift · The record" — the run ledger of the Night Shift area: every overnight pass as a receipt of what was read, written, flagged, and spent. Morning-after review, not live monitoring.
Scene: full app window, macOS overlay title bar. Top-center segmented navigation reads exactly "Notebooks   Registry   Night Shift" with Night Shift selected.
Palette (Synthwave '84): canvas #1e1a29; surface #262335; elevated #34294f; floating side cards #2e2b3d with 12px radius; foreground #f4f2ff; muted #9096c3; primary/focus #ff7edb; links #36f9f6; success #72f1b8; amber warnings; borders subtle lavender rgba(176,132,235,0.16). Center workspace unboxed like a sheet of paper; left and right rails are floating side cards.
Left rail: header exactly "NIGHT SHIFT". Nav items: "Tonight", "Standing orders · 7", "The record" (selected, dot). Footer line "Resident · this Mac · next pass 2:00 AM".
Center: heading exactly "The record", sub-line "Every pass leaves a receipt."
Group header "LAST NIGHT · 4 RUNS · $0.58" with four dense flat receipt rows (thin rules, not cards):
1. "Deep read — Japan 2027" chip "COMMISSION", teal check, sub-line "Read 42 sources · wrote Growing Answer +1 delta · 2:00–3:41 AM" · right "$0.40 · Open note".
2. "Release brief — Alchemy Development" teal check, sub-line "Read 12 changed sources · wrote 1 report · 8 citations" · right "$0.19 · Open report". This row is selected with a subtle highlight.
3. "Pricing page watch — Market Research" magenta chip "CHANGED", sub-line "Diff summarized · flagged to the Brief" · right "$0.00 · local".
4. "Re-gist Health & Insurance" teal check, green lock icon, sub-line "118 gists refreshed · sealed run" · right "$0.00 · local".
Group header "WEDNESDAY · 3 RUNS · $0.41" with two quieter collapsed rows, then a muted line "Older nights…".
Right rail (receipt of the selected run): header exactly "RECEIPT". Lines: "Release brief · Thursday 2:10 AM", "Engine: gateway · gpt-5-codex", "Read: 12 sources, 3 notes", "Wrote: 1 report note", "Flagged: nothing". Small header "EGRESS" with "2 gateway calls · $0.19" and "0 agent CLI calls". Small header "AUTHORITY" with three checked lines "Read scheduled scopes", "Write reports & notes", and a dash line "No outward actions". Buttons "Open report", "Search trace". Bottom small line "Trace: retrieval.jsonl · 2:10:04 AM".
Constraints: all text readable and correctly spelled, especially quoted strings. Icon plus text for every status, never color alone. Compact SF Pro-like typography, hairline dividers, no gradients, no glow, no KPI cards, no charts, no avatars, no admin UI, no watermark, no colored left borders.
```
