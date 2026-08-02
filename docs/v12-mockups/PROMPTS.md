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
