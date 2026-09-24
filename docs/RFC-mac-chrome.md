# RFC: The Mac chrome — toolbar, panes, inspector

Status: built 2026-09-23 (phase 1: toolbar, panes, Studio inspector).
Origin: Reminders item "make it look Apple AF" and the two-iteration mock
canvas reviewed with Paul (claude.ai/artifact/AyEF93Wg1kdu4F7jMVQoUr).

## What we keep

The three panes in the NotebookLM arrangement; every theme token and the
shader backdrop; glass as a choice (Off / Tinted / Clear) rather than the
default; the transmutation sigil and the notebook summary on a blank chat.
Pure SwiftUI reads generic and cheap; the point is to borrow its part
shapes, not its palette.

## Phase 1, built

1. **Toolbar as title bar.** One 52px strip (`.toolbar`) with the chrome
   tone and a bottom hairline; under glass it goes translucent. The
   notebook name is the window title with a subtitle beneath it — sources,
   On disk / Shared, when it was last written — and the same pop-up as
   before (the notebook's verbs, the switcher, All Notebooks). The "On
   disk" chip is gone into the subtitle; Show Bundle in Finder stays in the
   menu. The Notebooks text button became the Library glyph alone. Home's
   header wears the same class so the two screens share one strip.
2. **Panes, not cards.** Sources, Studio and their collapsed rails are
   `.side-pane`: full height, no margins or radius, one hairline toward the
   center. Opaque at rest with the side-card's tonal lift; under Tinted
   glass the theme surface at 78% (58% under Clear), so the desktop and the
   shader show through in the theme's color.
3. **Studio as an inspector.** A segmented control under the STUDIO header:
   Generate, Notes (with count), Reports (with count); the choice is
   remembered. Generate lists every generator as grouped rows — Start here,
   then the four shelves — with the count of that kind already in the
   notebook; templates sit on the Write shelf; one instructions field at
   the foot replaces "+ Add instructions". Nothing hides behind More.
4. **One ablation.** The chat toolbar's Open In dropdown went; the title
   menu had it already.

Verified in the dev app on Night City (glass tinted and off) and Midnight.

## Not built, decided

- ~~Home stays on its inset cards until the Registry and Brief usage counts
  from docs/RFC-ablation.md items 4 and 7 exist.~~ Superseded: Home is now
  the Library below, as a **re-arrangement** rather than a cut. Every
  surface the four cards held is a section of the one sheet, chosen from the
  sidebar, so items 4 and 7 still have everything they need to measure —
  nothing was removed, only moved. The Tags block is the one part of the
  Home spec not built: tags are a per-source field, and a corpus-wide tag
  list needs a backend rollup rather than a scan per render.
- The composer's three pills, the Sources chip strip, and the DEV badge
  keep their places for now; each is a separate small change.
- Sidebars honoring the macOS accent color is an Appearance option to add,
  not a default: themes decide selection color.

## Contrast floor (was: known tension)

Under glass with a bright desktop the transparent center column read
light and the theme's text lost contrast (visible on Home with Night
City). Resolved on the side of reading: the center column is the
**sheet** (`.sheet` in `src/index.css`) and keeps a floor under glass —
the theme background at 86% for Tinted, 72% for Clear — while the
sidebars and toolbar stay the translucent material (78% / 58% surface,
55% / 0% chrome). Menus (`.menu-glass`) floor at 84% of the elevated
tone for the same reason. The desktop shows through where the chrome is,
not where the words are, which is also how Finder, Mail and Notes draw
it.

## Measurements (the spec, from the iteration-2 mocks)

Every number below is read off the approved canvas (`Main2`, `Home2`,
`Settings2`, `Welcome2`). Build to these; divergence needs a reason in
the ledger. Tokens: 13px is `text-body`, 12px `text-caption`, 11px
`text-micro`. "Hairline" is `--border`; "strong hairline" is
`--border-strong`. "Inset hairline" is `box-shadow: inset 0 0 0 0.5px`
(a group edge that does not add to the box), "wash" is `--selection`.

### Shared

| element | spec |
| --- | --- |
| caps label | 11px, 600, uppercase, tracking .04em, subtle color |
| list row | 28px high, padding 0 8px, radius 6, gap 8, 13px; selected = wash; 1px between rows |
| icon button (`tb`) | 24px high, min-width 28, padding 0 7px, radius 6, 12px, muted; hover surface-2 + foreground |
| segmented track | padding 2, radius 7, surface-2, inset hairline |
| segmented button | 22px high, padding 0 10px (0 8px icon-only), radius 5, 12px/500, muted; on = elevated bg + strong inset hairline + 0 1px 2px shadow, foreground |
| grouped list (`group`) | radius 10, surface-2, inset hairline, rows split by hairlines |
| group row | 38px high (Settings: 40px), padding 0 12px, gap 10, 13px, 16px icon, trailing count 11px subtle |
| footnote | 11px, muted, padding 6px 12px 0 |
| pop-up button | 22px high, padding 0 6px 0 10px, radius 6, elevated bg, strong inset hairline, 12px, 10px chevron |
| field (search/filter/instructions) | 26px high (instructions 30), padding 0 8px, radius 8, surface-2, inset hairline, 13px (12 in the inspector), subtle icon 16px |
| switch | 26×16 track, 12px knob, primary when on |
| tag dot | 8px, 4px side margins |

### Toolbar (Workspace and Home)

52px high, padding 0 14px, gap 12, hairline below. Traffic-light gutter
60px. Sidebar toggle, then Back/Forward with 2px between. Title pill:
36px high, radius 8, padding 0 12px, gap 10; a 22px icon tile radius 6 in
a 22% primary wash; name 13px/600 line-height 16; subtitle 11px muted
line-height 13 (`7 sources · On disk · synced 2 min ago`); 10px chevron.
View segmented, `md`: 28px buttons, 13px/500 labels, padding 0 12px,
14px icons, in a 32px track (padding 2, radius 8) — centered in the
remaining width. Right side: model-activity slot 54px (a fixed width,
running or idle), search field 200×26 shrinking to 120 at the window's
1040 minimum, inspector toggle (surface-2 when the inspector is open).
Home swaps the title pill for the sigil + "Alchemy" 13px/600 and carries
grid/table segmented, sort pop-up (26px, surface-2, "Recently updated"),
search 200×26, and a primary "New Notebook" 26px radius 8.

**Built, not drawn.** The pill grew 32 → 36 with 12px sides (at 8 the tile
touched the edge and the hover wash read as a box around the icon), and the
view switcher moved from the shared 22px segmented to `md` — it is the only
segmented control that is navigation, and at filter size it read as a hint.

### Sources pane

260px, padding 10, gap 12 between blocks, hairline right. Header: caps
"Sources" with the count 12px normal-case beside it, then filter and add
icon buttons (24px, min-width 24) — no collapse button, because the
toolbar's sidebar toggle is visible whether the pane is open or shut. The
filter button is disabled when the notebook has one kind of source, no
tags and nothing missing: there is nothing behind it to open. "All
selected" row 26px, 12px muted,
14px checkbox. Source rows per the shared row spec, 1px apart, title
13px, checkbox 14px radius 4 (primary when on). Tags block: caps padding
4px 8px 6px, rows with an 8px dot and a 12px count. Grow row pinned to
the bottom: 32px, surface-2, inset hairline, radius 6, then a 6px amber
dot and "1 to review" 12px.

### Sheet (chat)

Center column, padding-top 26, 26px between blocks, everything centered.
Summary card 640 wide: radius 10, strong inset hairline, padding 16px
18px, 13px/1.5, caps label with 8px below. Sigil. Suggested-question
pills: 28px, radius 14, padding 0 12px, surface-2, inset hairline,
foreground, 8px apart, wrapping within 560px. Composer: 680 wide, radius
22, glass over the sheet, strong hairline + 0 12px 32px shadow, padding
10px 10px 8px 16px, 8px between the input and the button row; input 14px;
row = model pill (24px, radius 12, padding 0 10px, surface-2, inset
hairline, `Claude Code · Balanced` with a chevron), attach (24px round,
icon only), send (28px circle, primary, white glyph) pushed right; 18px
below the composer to the window edge. No chat toolbar row: Open In and
Share live in the title pop-up, Clear and tuning in the composer's menu.

### Inspector (Studio)

300px, padding 10px 12px, gap 12, hairline left. Segmented stretched
(each button flex 1, centered) with counts in the label: `Notes 7`,
`Reports 1`, count 11px normal weight. Shelves: caps with padding 0 4px
(8px above the second shelf on), grouped lists per the shared spec, rows
38px; each shelf shows its top rows and folds the rest into one
disclosure row (`FAQ, Timeline, Data table, 4 more` muted, chevron
right). Instructions field pinned to the bottom: 30px, radius 8,
surface-2, inset hairline, 12px, `Instructions for the next generation…`.

### Home

Sidebar 220px, padding 10, gap 14: Library (Notebooks with count, Chats
with a 6px primary dot when unread, Shared, Nightly Reports, Archived),
Registry (Cards with count, Suggested with a primary count badge 11px/600
radius 9 padding 1px 6px), Tags (dots). Main: padding 22px 28px, gap 18;
h1 26px/700 tracking -.01em with `22 active · 3,760 sources` 13px muted
on the baseline; sections by recency (Today, Last 7 days, Earlier) with
caps and 10px below; cards 212 wide, 20px apart, gap 10 inside: thumb
140px high radius 10 surface with a strong inset hairline and padding 14
(a 10px color dot + 10px caps name, then 6px rounded lines), name
13px/600, meta 12px muted (`7 sources · 7 notes · 2 min ago`). Footer
pinned bottom: hairline above, padding-top 12, 12px muted `Last night: 2
reports written, 14 sources refreshed, 1 duplicate set aside.` with
"Read the Brief" as a link on the right.

**Thumb contents.** The canvas drew the thumb's body as rounded bars; built,
those bars are only the skeleton, and the body is the notebook's real
contents (`notebook_previews`). Under the caps name, 10px down: up to three
11px muted rows, each a 12px type glyph and one truncated title — the newest
sources, with the newest note taking the last row when there is one, so the
mix of material reads at a glance. When the notebook has lead images, the
bottom of the thumb (`mt-auto`, 8px above) is a strip of up to three 56×36
tiles, radius 6, `object-cover`, 6px apart, each under a hairline; images are
lazy and a tile that fails to load removes itself. The strip costs a line, so
a card with images shows two titles rather than three. A notebook with nothing
in it shows one muted `Add a source…`. The meta line leads with contents too —
newest note, else the last question behind a chat glyph, else the newest source
title — then `· Shared · 2 min ago`; the counts move to the card's `title`
tooltip (`7 sources · 29 notes`) and stay as they were in the table.

### Settings

Sidebar 200px, padding 12px 8px, 2px between rows; search 26px radius 7;
rows 28px padding 0 6px radius 6 with a 20px tile radius 5 (surface-2;
primary when selected) holding a 12px glyph; selected row = wash. Pane
header 52px, padding 0 20px, hairline below, title 15px/600. Content
padding 18px 20px, 18px between sections; caps padding 0 12px 6px; group
rows 40px; footnotes 11px. Appearance: Theme row = swatch strip (4×10×14
radius 4) + pop-up button naming the theme; Backdrop row = its name and
"moves" 12px muted + a switch; Selection color segmented; Glass = Window
material segmented + "Sidebars show through to the desktop" switch;
Text = Chat font, Text size segmented.

### First run

Brand pane 300 wide; sigil 150; "Alchemy" 20px/700; tagline 12px muted.
Right pane padding 34px 28px 0, gap 16; step chip 20px radius 10 11px;
title 20px/700; body 12px/1.45 muted; grouped radio list rows min 54px,
padding 9px 12px, gap 12, 16px radio (1.5px ring, primary when on with a
white 8px dot), 18px icon, title 13px/600 + hint 11px muted, status chip
right. Footer padding 14px 20px, hairline above, 12px preview text left,
buttons 28px radius 7 right.
