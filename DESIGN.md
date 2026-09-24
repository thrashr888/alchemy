# Alchemy — DESIGN.md

Design system for Alchemy, a local-first research notebook app for macOS
(Tauri + React + Tailwind v4). This file follows the Stitch DESIGN.md format;
agents and humans should treat it as the source of truth for visual and
interaction decisions. Tokens live in [src/index.css](src/index.css) and
[src/lib/themes.ts](src/lib/themes.ts); shared primitives in
[src/components/ui.tsx](src/components/ui.tsx).

## 1. Visual Theme & Atmosphere

Linear-inspired, macOS-native density. Near-black canvas, faint hairline
borders, one restrained indigo accent, tight 13px type. Calm and utilitarian:
the user's sources and the model's answers are the interface; chrome recedes.
Motion is minimal and fast (150ms), never springy.

The reference grammar (Linear, Vercel, Claude desktop, Finder) in one
paragraph: everything sits on **one sheet of paper** — regions separate by
hairline borders and spacing, never tonal fills; **color only when it means
something** (status, identity, links, diffs — never decoration); **text
carries hierarchy**, not boxes; **shadows are whispers** and borders carry
the edge; active states are quiet tinted pills; radius is disciplined and
un-nested.

The workspace is a Finder-style arrangement: the window is one chrome
container (`app-root`) holding the titlebar and two floating **side-cards**
(`side-card` — Sources and Studio, inset rounded-xl), while the center
chat/reader column stays **uncontained** — it is the paper itself, never a
third card. Optional **glass mode** (Settings → Appearance) makes the window
transparent behind macOS Liquid Glass: the chrome layer and side-cards go
translucent, the center goes fully transparent, and content cards carry
their own opaque surfaces.

Empty chat may use the animated dithered "aetheric mist" WebGL background
(`DitherBackground`) — the app's primary decorative element: behind content,
tinted from theme tokens, static under `prefers-reduced-motion`, and hidden
entirely under glass (the material is the ambience there). The hero's
transmutation sigil (`AlchemySymbol`) is the one other sanctioned ambient
element: its slow cross-fade and rotation are deliberately outside the 250ms
cap (they are its whole character) and go static under
`prefers-reduced-motion` like everything else.
Shader work is previewed, never shipped blind: `python3
scripts/shader-harness.py --serve` renders every backdrop mode and tile field
in one contact sheet (`.claude/skills/shaders/SKILL.md` has the loop).

The app is themeable (37 schemes, dark and light) — never hardcode a hex in a
component; always go through the semantic tokens below.

## 2. Color Palette & Roles

Tokens are CSS custom properties set per-theme. Defaults shown are the
"Midnight" theme.

| Token | Midnight value | Role |
|---|---|---|
| `--background` | `#08090a` | App canvas |
| `--surface` | `#0d0e10` | Side-cards, cards at rest |
| `--surface-2` | `#141517` | Inputs, hover fills, nested surfaces |
| `--elevated` | `#18191c` | Menus, modals, toasts (highest surface) |
| `--foreground` | `#eceef1` | Primary text |
| `--muted-foreground` | `#8a8f98` | Secondary text, labels |
| `--subtle-foreground` | `#62666d` | Captions, tertiary metadata |
| `--border` | `rgba(255,255,255,0.07)` | Hairline dividers |
| `--border-strong` | `rgba(255,255,255,0.12)` | Interactive/hover borders, elevated edges |
| `--primary` | `#5e6ad2` | Accent: primary buttons, active states |
| `--citation` | `#8b95f5` | Citation chips, links, accent text on dark |
| `--destructive` | `#eb5757` | Errors, delete affordances |
| `--success` | `#4cb782` | Confirmation states |
| `--ring` | `#5e6ad2` | Focus rings (always visible on keyboard focus) |

Rules: dark themes spread surface steps apart for depth; light themes keep
them close. Accent text on tinted fills uses `--citation`, never gray. Errors
tint their container (`bg-destructive/10`) rather than sitting on plain gray.

Derived materials (defined in `index.css`, never inline):

- `--chrome` — the frame tone: `--background` nudged toward
  `--foreground` (4% light / 7% dark). Under `.glass` the app root washes
  it at 45% over the material in the Tinted style; Clear drops the wash.
- `side-card` — panel cards: light schemes are border-only (`--background`
  fill + hairline + 4% shadow, the Vercel/Linear treatment); dark schemes
  get a tonal lift (`--surface` mixed 4% toward foreground); under `.glass`,
  84% translucent (Tinted) or 70% (Clear). The shared frame (radius,
  border, overflow) lives in the `.side-card` rule itself.
- `menu-glass` — every floating surface (row menus, palettes, popovers,
  ⌘K): 72% `--elevated` over `backdrop-blur`, frosted like native macOS
  menus, in and out of glass mode.

Icon color policy: type and navigation icons are monochrome
(`text-muted-foreground` — the theme's cast carries through its gray).
Color in iconography is reserved for semantics: status dots, error/warning
states, notebook identity, favicons (real content), links. No decorative
left-border accents anywhere — identity color rides in dots and chips.

## 3. Typography Rules

System font first — SF Pro on macOS:
`-apple-system, BlinkMacSystemFont, "SF Pro Text", system-ui, "Segoe UI", sans-serif`.
Monospace: `"SF Mono", ui-monospace, monospace`.

Type is sized in **rem**, not px, so the whole UI tracks the macOS
Accessibility text size. The native layer (`src-tauri/src/textsize.rs`) reads
the effective Dynamic Type size and publishes `--system-text-scale`; the root
`font-size` is `calc(16px * var(--system-text-scale, 1))`, so every rem
multiplies into it. At a 16px root (scale 1.0) the tokens below are the exact
px they replaced — pixel-identical to the pre-scale UI.

Use the semantic `text-*` classes (backed by `@theme` tokens in `index.css`),
never `text-[Npx]`. The rem values are exact 16ths of px:

| Style | Class / token | rem (px @16) | Weight | Usage |
|---|---|---|---|---|
| Page title | `text-page` / `--text-page` | 1.375rem (22px) | 600, tight tracking | Home "Your notebooks" |
| Section title | `text-section` / `--text-section` | 0.9375rem (15px) | 600 | Hero headings, app name |
| Card title | `text-card` / `--text-card` | 0.875rem (14px) | 500 | Notebook and note cards (13px cards use Body) |
| Body / controls | `text-body` / `--text-body` | 0.8125rem (13px) | 400–500 | Default UI text, buttons, inputs, prose |
| Caption | `text-caption` / `--text-caption` | 0.75rem (12px) | 400 | Toasts, metadata, hints |
| Micro-label | `text-micro` / `--text-micro` | 0.6875rem (11px) | 500, uppercase + tracking-wide | Panel headers ("SOURCES", "NOTES") |
| Count badge | `text-badge` / `--text-badge` | 0.625rem (10px) | 500 | Numeric count badges only (the floor) |

Rare one-off sizes (10.5/11.5/12.5/13.5/17/26px) don't earn a token — write
them as arbitrary **rem** literals (`text-[1.0625rem]` for 17px, etc.), never
px. Floors: 11px (`text-micro`) is the minimum text size anywhere; 10px
(`text-badge`) only for numeric count badges. Chat prose is 13px at
line-height 1.65 (user-adjustable 12/13/15px, also in rem so it composes with
the system scale). Fixed-canvas artifacts — slide decks (`.slide-surface`, a
960×540 design surface scaled by transform) and print/PDF export — stay px and
do NOT scale with accessibility: they are documents, not chrome. Never
introduce a webfont; the system stack is deliberate.

## 4. Component Stylings

- **Buttons** (`ui.Button`): heights 28px (`sm`, icon) / 32px (`md`); radius 6px.
  Variants: `primary` (accent fill, subtle 1px shadow), `secondary`
  (surface-2 fill + strong border), `ghost` (text-only, surface-2 on hover),
  `danger` (10% destructive tint). Focus: 2px ring in `--ring`. Disabled: 50%
  opacity, no pointer events.
- **Inputs / Textareas**: 32px tall, `surface-2` fill, 1px `--input` border,
  radius 6px; focus swaps border to `ring/70` plus a 1px ring — no glow.
- **Selects** (`ui.Select`): a native `<select>` with `appearance-none` and
  a lucide chevron drawn over it, same height and fill as inputs. Never a
  bare styled `<select>` — WKWebView ignores its padding and height, so the
  text sits flush against the border.
- **Cards / list rows**: `surface` fill, 1px `--border`, radius 6–10px; hover
  raises to `surface-2` and `--border-strong`. Clickable cards use
  `ui.CardAction` — an `absolute inset-0` real button rendered as a *sibling*
  of the card content (card is `relative`, secondary controls sit above it
  with `relative z-20`), so nothing interactive ever nests. Row actions
  hidden until hover **or focus-within**, never hover-only. **At most two
  quick actions show on a row**; everything else — edit, delete, pause —
  lives in the row's ⋯ menu (`ui.RowMenu`), which right-clicking the row
  opens too (§9, objects are direct). List containers are `select-none`:
  row text is chrome, never a text selection.
- **Toggle chips** (`ui.Chip`): one option in a filter row, a tag to narrow
  by, a mode a pane can be in. Radius 6px, no border; at rest muted text,
  selected = `surface-2` fill + medium weight — the same quiet tinted pill
  every active control uses. `sm` (12px) in filter rows, `xs` (11px) in
  dense strips. **Never the accent for a selected filter**: selection is
  not a status, so color there would mean nothing (§1). The one colored
  element a chip may carry is a `dot` swatch standing for a colored thing.
- **Segmented controls** (`ui.Segmented`): two to four exclusive states of
  one thing (grid/table, newest/A–Z) in one bordered 8px track, segments
  6px, the active one `surface-2`. Icon-only segments carry `hint` as
  their name. More than four options, or long names: a `Select`.
- **Sorting**: tables sort by their column headers (`HomeTable`: the
  header is the button, the arrow shows direction, text starts ascending
  and counts and dates descending). Every other list sorts through
  `ui.SortMenu` — an ArrowUpDown trigger opening a radio menu with the
  current order ticked — so the affordance reads the same in Studio, the
  gallery and the registry. No bare `<select>` for sort, no two-way
  toggle: even a two-order list uses the menu.
- **Filter fields** (`ui.SearchField`): the search glass drawn in at left,
  a clear button once there is text, `type="search"`. `quiet` (28px,
  transparent fill) inside a side card, where a filled box would be a box
  in a box; `field` (32px, `surface-2`) in a page toolbar beside other
  32px controls. Placeholder reads "Filter <things>…".
- **Checkboxes**: a real `<input type="checkbox">` with the `select-quiet`
  class for list selection (Finder-style, §9); a `ui.Switch` for a setting
  that is on or off. Never a checkbox for a setting, never a switch for a
  selection.
- **Menus**: `menu-glass` material (see §2), hairline edge (see §6), radius 6px, 13px items;
  open focuses the first item, arrows cycle, Escape closes and restores focus,
  `role="menu"`/`menuitem`.
- **Modals**: `elevated`, radius 10px, hairline + soft shadow, 44px header
  with 13px semibold title; scrim `black/40` with 2px backdrop blur. Escape
  closes; focus is trapped and restored. Confirmations use the app modal,
  never `window.confirm`. **Actions live in the `footer` slot** — a fixed
  bar under the scrolling body — never inside the form, so a long dialog
  keeps Cancel and Save in reach (a submit button outside the form points
  at it with `form="<id>"`).
- **Toasts**: bottom-center, `elevated/90` with backdrop blur, status-tinted
  border, 12px text.
- **Icons**: lucide, 16px (`h-4`) in headers/toolbars, 14px (`h-3.5`) in dense
  rows and inline actions. Nothing interactive below 14px. Icon-only buttons
  always carry `aria-label` (and usually `title`). Monochrome by default —
  see the icon color policy in §2.
- **Empty states** (`ui.EmptyState`): centered small icon + 13px title +
  one gray sentence. Every empty section uses it — no bare paragraphs.
- **Progress bars** (`ui.ProgressBar`): 4px `surface-2` track, `--primary`
  fill, 200ms width transition, floored at 3% so the first item doesn't
  read as an empty track. Determinate only — an indeterminate bar is a
  `Spinner` with words beside it. The element carries `aria-valuenow`/`max`
  in real units and an `aria-valuetext` that says "3 of 12 documents", never
  a percentage. Many small jobs of one kind collapse into one bar with the
  current item as a subtitle; they do not each get a row.
- **Tool confirmations** (chat): process, not conversation — one quiet
  12px gray row with a 12px icon, no bubble, no role label.
- **Document properties** (`DocProperties` in the reader): Linear-style
  label/value rows (type, origin, dates, size) at the top of a document,
  12px, hairline-separated from content. Answers "what is this" before
  the prose.
- **Data tables** (rendered markdown, everywhere `Markdown` runs): one
  hairline frame around the whole table (rounded 8px, the horizontal
  scroll container), horizontal hairlines between rows, NO vertical grid —
  column gaps do that job. Headers are 12px medium muted labels on a
  `border-strong` rule, one line, never wrapped. Bare figures right-align
  with tabular numerals (`.cell-num`, stamped by the renderer). Spreadsheet
  section rows (one label, then empties) render as muted subheads
  (`.tr-section`); an all-blank header row renders as nothing. Cells wrap
  at word boundaries only — wide tables scroll, words never shatter.

### Consistency ledger

One app does not need a design system, but it does need one answer per
question. The primitives above are those answers; a component that draws
its own version of one is a bug unless it is listed here with its reason.
The measurable check (2026-09-22): raw `<select>`s outside `ui.tsx` went
from 17 to 0; toggle chips from four drawn styles to one; sort controls
from four shapes to two (headers for tables, the menu for the rest);
filter fields from three to one. Raw `<button>`s (216) are fine — most are
icon buttons and row bodies styled for their spot; the rule is that a
button that *looks like* a primitive uses the primitive.

Justified exceptions, kept on purpose:

- **Onboarding doors** are cards with a title and a note, not chips —
  they carry a paragraph, and a chip cannot. They keep their own layout
  and the accent border for the chosen door: choosing a model is the one
  place first-run color means something.
- **The graph's degree badge** (`GraphView`) is a tabular-nums label that
  toggles a highlight; it reads as data, not a filter, and stays a plain
  label.
- **The reader's title field** and the command palette's input are
  transparent, unbordered inputs by design: the text *is* the surface.
- **The chat composer** is its own component: a growing textarea with its
  toolbar, not an `Input`.
- **Pickers that show the thing they pick** — the notebook color swatches
  and icon grid (`NotebookEditModal`), the theme strips and the big
  icon-and-label tiles in Appearance (`SettingsTabs`) — are not chips: the
  swatch, glyph or strip *is* the label. They keep their own shapes and use
  the ring for the selected one.
- **Studio's generator tiles** (`GenTile`) are buttons with a family accent
  on the icon; the accent is wayfinding for note kinds (§2), not a state.

## 5. Layout Principles

8px grid with 4px half-steps. Key measures:

- Header bar: 48px (`h-12`) on every view, `data-tauri-drag-region`, left
  padding 84px clears macOS traffic lights (centered via
  `trafficLightPosition` in `tauri.conf.json`). No bottom rule — the cards
  provide the separation.
- Workspace arrangement: side-cards are inset `mx-2 mb-2 mt-1` with an 8px
  gap to the open center; the 4px top inset plus the center's `pt-1` puts
  the SOURCES / CHAT / STUDIO headers on one horizontal line. Collapsed
  rails are `w-12` cards that hug their content (`self-start`), not
  full-height strips.
- Side panels: Sources 280px default (drag 220–400), Studio 320px default
  (drag 260–460); resizable via `ResizeHandle` (double-click resets).
  Collapsed panels become 48px icon rails.
- Chat column: content max-width 720px, 20px horizontal padding.
- Panel padding: 16px (`px-4`) headers, 8px (`p-2`) list containers.
- Progressive disclosure everywhere: hover/focus-revealed actions, collapsed
  citations, "+ Add instructions" style inline expanders.

## 6. Depth & Elevation

Three surface steps (surface → surface-2 → elevated) do most of the work;
shadows are reserved for true overlays.

- Hairline edge for overlays (menus, modals): prefer
  `box-shadow: 0 0 0 0.5px var(--border-strong), <soft ambient shadow>` over a
  1px border — crisper on retina.
- Primary buttons: `0 1px 2px rgba(0,0,0,0.3)` only.
- No glows, no colored shadows, no inner shadows.
- Overlay scrims: `black/40` + slight backdrop blur; overlays themselves may
  use translucency + `backdrop-blur` (toasts) for a vibrancy feel.

## 7. Do's and Don'ts

Do:
- Use semantic tokens for every color; test dark and light themes.
- Make every action reachable by keyboard; keep focus visible (global
  `:focus-visible` outline is on — don't suppress it without replacing it).
- Guard global shortcuts with `shortcutBlocked()` and respect IME composition
  (`isComposing`) in Enter handlers.
- Keep text selectable only where content lives (`.selectable`, prose, inputs).
- Respect `prefers-reduced-motion` for any animation beyond a fade.

Don't:
- Don't hardcode hex values, add webfonts, or use pure black/white fills.
- Don't use bounce/elastic easing, animations >250ms, or decorative gradients.
- Don't nest interactive elements (no buttons inside buttons).
- Don't reveal actions on hover only — pair with `focus-within`.
- Don't use text under 11px, interactive icons under 14px, or gray text on
  colored fills.
- Don't add new UI chrome when a surface step or hairline would do.
- Don't separate regions with tonal fills — hairline + spacing is the tool.
- Don't tint icons or chrome decoratively; color is semantic (§2 policy).
- Don't use colored left-border accents; identity rides in dots and chips.
- Don't wrap the center chat/reader column in a card — only sidebars float.
- Don't give a floating surface its own background — use `menu-glass`.

## 8. Responsive Behavior

Desktop-only Tauri window: min 1040×640, default 1280×820. The layout must
stay usable at 1040px with both panels open at max width — the chat column
flexes and its content column caps at 720px. Panels collapse to 48px rails
rather than disappearing. No mobile breakpoints; instead, guarantee that
every panel width within its drag bounds truncates gracefully (single-line
truncation with `title` tooltips).

## 9. macOS Behavior — the Mac formula

Apple's HIG, distilled to one line:

> **The system is law, the menu is the index, the keyboard is complete,
> undo beats confirm, objects are direct, state survives.**

What each clause means here:

- **System is law** — appearance, accessibility text size, and reduced
  motion come from macOS and are never overridden (§3, §7). Standard edit
  shortcuts (⌘C/V/X/Z/A, ⌘F) always mean the standard thing.
- **Menu is the index** — every user-facing command appears in the native
  menu bar (`menu.rs`) with its shortcut. If it's not in a menu, it's not
  discoverable; the menu is the app's table of contents, not a formality.
- **Keyboard is complete** — anything clickable is reachable and operable
  by keyboard (§4, §7). New shortcuts register in `SHORTCUTS` (§10).
- **Undo beats confirm** — prefer an immediate, undoable action with a
  toast over a confirmation modal. Confirm only genuinely unrecoverable
  bulk loss; never interrogate the user about reversible things.
- **Objects are direct** — the things on screen are the things: right-click
  any object for its actions (mirroring its row actions), drag files in to
  import, drag/copy content out. No "select then hunt for a toolbar" flows.
- **State survives** — window size, panel widths, selection, and in-progress
  text restore on relaunch. Quitting is not losing your place.

When a rule here conflicts with a web idiom, the Mac wins.

### Menus — the NSMenu formula

Every menu is a native menu (NSMenu through `RowMenu`'s `native` default),
because a menu is where the app most obviously either is or isn't a Mac
app. The custom panel exists only for rows that need HTML the native row
can't hold, and a menu that needs it should first ask whether it needs
those rows. The rules, from the HIG's menu chapter and macOS 26's menus:

- **Title-style capitalization.** Every row: "Export Notebook…", "Keep on
  Disk as OKF…", "All Notebooks…". Names inside a row (a notebook's title,
  a source's) keep their own case.
- **An ellipsis means "asks for more."** A row that opens a dialog, a
  picker, or a confirmation ends in "…"; a row that acts at once doesn't.
- **Group, then order.** Related rows sit together behind dividers; the
  destructive pair (Archive, Delete…) sits last behind its own. A pop-up
  from a title lists the *choices* as its body, ticks the current one, and
  folds the object's verbs into one submenu named for the object
  ("Notebook"). One submenu level, never two. No disabled header rows —
  if a list needs a label to be understood, it needs fewer rows.
- **Symbols per group, all or none.** Apple's rule verbatim: "Use menu
  item icons sparingly and with purpose" and "provide icons for all menu
  items in a group, or none of them." So the divider groups decide: a
  group gets symbols only when every row in it has a purposeful one —
  the origin trio (Open Original `arrow.up.right.square`, Show in Finder
  `folder`, Copy URL `link`: file-system locations), the destructive pair
  (Archive `archivebox`, Delete… `trash`), or a list of objects that wear
  those glyphs elsewhere (each notebook's own icon in its own color in the
  switcher; the Library's `books.vertical`). A group of plain verbs —
  Rename, Refresh, Edit…, Add…, Copy Text, sort orders, hints — goes
  without, and one row that "deserves" a symbol never gets it alone: cut a
  divider so it has a group, or leave it plain. SF Symbols only, template
  images (the menu's text color, inverted under the highlight). If you
  can't find a symbol that clearly represents the row, don't show one.
- **Symbols are rasterized once.** `scripts/menu-symbols.swift` draws the
  named symbols with the menu's own configuration at small scale onto a 17×18 pt canvas (the scale and column AppKit gives menu symbols; muda draws every menu image 18 pt tall, so this is 1:1)
  (so labels share a column) into `src/assets/menu-symbols/`; a row names
  one by `symbol`. `menuicons.rs` marks them template and sets
  `preferredImageVisibility` — macOS 26+ hides every menu image without
  it. Rerun the script when a menu gains a symbol the set lacks. The one
  runtime raster is the notebook switcher's colored notebook icons.
- **The stock context menu never shows.** Reload / Share / AutoFill /
  Services is browser chrome. It appears only inside a text field or over
  a real selection in content (the reader, a note, a chat turn), where the
  platform's own text verbs belong; everywhere else right-click opens the
  object's menu or nothing.
- **The menu bar, the tray, and the Dock menu follow the same rules** and
  come out plain: none of their groups qualifies, so none carries a
  symbol. (macOS 26 adds its own gear to a standard Settings row; ours is
  custom and stays bare.) The Dock menu is titles only.

### Multi-select — the Finder pattern

Any surface listing selectable objects (source rows, gallery cards, note
rows, registry index) speaks one selection dialect, implemented once:

- **One shared selection.** Rows/cards carry `data-pick-id`; selection
  lives in the store's `picked` (kind + ids). The same objects shown on
  two surfaces (sidebar rows, gallery cards) share one selection — pick
  in either, both show it.
- **The gestures.** Plain click acts on the object (open). ⌘-click
  toggles. Shift-click ranges over the surface's visible order. A drag on
  the background rubber-bands (`useMarquee`); ⇧/⌘-drag unions against the
  selection as it stood when the drag began. Escape/background-click
  clears.
- **Selection is chrome.** The list container is `select-none` — object
  text never becomes a native text selection under a band. (The marquee's
  own `userSelect` guard only lands after the 4px drag threshold, so the
  container class is load-bearing, not belt-and-braces.) Prose surfaces
  (reader, chat) never host marquees; they keep real text selection.
- **The click after a band is the band's tail** — suppress it
  (`justEnded()`), or the drag "opens" whatever it ended on.
- **Selected look**: `ring-1 ring-primary` (offset on cards), never a
  tonal fill — hairlines over washes, §1.
- **Batch verbs ride the object menu.** Right-clicking a selected object
  shows the batch variants with counts ("Remove 4 sources…") in place of
  the singular menu; no separate toolbar appears.
- **Drag-out beats band** on surfaces whose rows export as files
  (`data-drag-out`): pressing an item means "take this file"; bands begin
  on background only — Finder draws the same line.

## 10. Agent Prompt Guide

Quick reference for agents building UI here:

- "Use the app's design tokens" means Tailwind classes bound to the theme:
  `bg-surface`, `bg-surface-2`, `bg-elevated`, `text-foreground`,
  `text-muted-foreground`, `text-subtle-foreground`, `border-border`,
  `border-border-strong`, `bg-primary`, `text-citation`, `text-destructive`,
  `text-success`, `ring-ring`.
- New buttons/inputs/modals/toasts/resize handles come from
  `src/components/ui.tsx` — extend those, don't fork styles inline.
- Structural materials: `app-root` (window chrome), `side-card` (panel
  cards), `menu-glass` (floating surfaces) — defined in `index.css`,
  scheme- and glass-aware. Use them instead of re-deriving backgrounds.
- Clickable cards: render `<CardAction label onClick />` from `ui.tsx` as a
  sibling of the card content (never wrap content in a button); reveal row
  actions with
  `opacity-0 group-hover:opacity-100 group-focus-within:opacity-100`.
- New keyboard shortcuts: add the listener at the owning component, guard with
  `shortcutBlocked(e)`, and register the shortcut in `SHORTCUTS` in
  `SettingsDialog.tsx` so the Shortcuts tab stays truthful.
- Text sizes: pick a semantic class — `text-micro` (11) `text-caption` (12)
  `text-body` (13) `text-card` (14) `text-section` (15) `text-page` (22),
  `text-badge` (10) for count badges only. Never `text-[Npx]` — rare sizes are
  arbitrary rem literals. Radii: `rounded-md` (6px) controls, `rounded-lg`
  (10px) overlays/cards. Icons: `h-4` toolbar, `h-3.5` dense.
- Example prompt: "Add a 'pin source' action to each source row: h-3.5 lucide
  Pin icon button, hidden until hover/focus-within, aria-label with the source
  title, confirm nothing, toast on success."

## 11. Accessibility & State Inventory

Two things a design system has to keep honest once it has 50 components: what
the app says to someone who cannot see it, and what each pane shows when it
has nothing, is waiting, has failed, or is quietly returning less than it
should. This section records both — the rules, and the current state of the
app measured against them, gaps included.

### Accessibility rules

- **Semantic HTML carries its own role.** A `<button>` never gets
  `role="button"`; a `<label>` wrapping its control never gets
  `aria-labelledby`. Redundant ARIA is worse than none — it is one more thing
  to go stale. Reach for ARIA only where the platform has nothing to say:
  icon-only controls, live updates, custom groupings, blocking overlays.
- **Icon-only buttons carry `aria-label`** (§4). When several copies of one
  control repeat down a list ("Keep", "Dismiss", a source icon in the rail),
  the label names the object it acts on, not just the verb.
- **Decorative icons are `aria-hidden`.** `Spinner` sets it for you; a lucide
  icon beside its own text label should set it too. Where an icon is the
  *only* carrier of meaning — a tick for a right answer, a red dot for a
  failed import — the meaning goes into `sr-only` text or the control's
  label. Color and shape do not survive being read aloud.
- **Live regions announce transitions, not streams.** A region wrapped around
  streaming chat tokens speaks every fragment as it lands, which is worse
  than silence. Use `LiveRegion` from `ui.tsx` and feed it the moments that
  matter: the question going out, the answer arriving with its citation
  count, a toast's text. Two properties make it work — the region stays
  mounted for the life of its host (a region that appears with its first
  message is announced unreliably), and each announcement is a fresh child
  node, so an identical sentence still speaks.
- **A blocking overlay is a dialog.** Anything covering the app —
  `MigrationOverlay`, `Onboarding` — sets `role="dialog"`, `aria-modal`, and
  a label, and takes focus. Otherwise a screen reader walks straight past it
  into the inert UI behind. `Modal` in `ui.tsx` already does all of this;
  prefer it, and match it when you cannot use it.
- **Charts are one labelled image.** A 91-cell heatmap read one square at a
  time is noise. `role="img"` plus an `aria-label` that says what the picture
  says (`ActivityTab`'s heatmap is the reference).

### VoiceOver traversal checklist

Re-runnable by hand: ⌘F5 to start VoiceOver, then walk the main window with
VO-→ and confirm each line. A failure here is a bug, not a nit.

1. **Titlebar** — notebook name is read once, not twice (the color dot is
   `aria-hidden`); "Open the command menu" and "Open settings" announce as
   buttons with those names.
2. **Degraded bar**, when one is showing — the title and the sentence are
   read together, and each fix button announces its own verb ("Start
   Ollama", "Install qwen3", "Rebuild now").
3. **Sources panel** — each row reads title, then type, then status; a failed
   import says it failed rather than showing a silent red dot. The collapsed
   rail reads "Show sources" plus the source it stands for.
4. **Chat** — send a question: the region says "Answering." once, stays
   silent through the stream, then says "Answer ready" with the citation
   count. Nothing repeats per token. An inline `[3]` reads "Citation 3,"
   plus the source title, not a bare number.
5. **Toasts** — trigger one (delete a source): the message is spoken once,
   without "Dismiss notification" trailing it. The Undo action is reachable
   by Tab while the toast is up.
6. **Studio** — the generate tiles announce their document kind; a disabled
   tile announces as dimmed rather than silently doing nothing.
7. **Settings** — tabs announce as a list with the current one marked;
   every switch reads its label and on/off state.
8. **Modals** — opening one moves focus inside, Escape closes it, and focus
   returns to whatever opened it.

### State inventory

Who owns each pane's four states today. "none" means the state genuinely is
not rendered — recorded as found, not as it ought to be.

| Pane | Empty | Loading | Error | Degraded |
|---|---|---|---|---|
| Notebook shelf (`HomeView`) | `AlchemyHero` branch; filter-empty line | none | `activityError` row + Retry (activity feed only) | `HealthBanner` |
| Home Staff + Brief (`HomeSections`) | `StaffQuiet` per group; "No brief yet" | `StaffQuiet` "Loading…" (watchers only) | none — toasts, and `FiledGroup` catches into an empty list | "Night Shift is off" button |
| Latest reports (`HomeReportsFeed`) | "You're all caught up"; `EmptyState` in `HomeView` | "Loading reports…" in `HomeView` | `EmptyState` "Reports unavailable" + Retry | none |
| Registry (`RegistrySection`) | `EmptyState` "No cards yet"; filter-empty | none | none — `load()` has no catch | orphan `Badge` + cleanup action; unconfirmed proposals |
| Sources (`SourcesPanel`) | `EmptyState` "No sources yet" / "No notebook selected" | one "Indexing n of m documents" card with a `ProgressBar`; single imports keep their own row | queue error row + Retry/Dismiss; "Import failed" per row | "n sources need attention" banner; hygiene badges; online-only line |
| Chat (`ChatPanel`) | `ChatHero`; disabled composer | `ThinkingDots`, `StepTrail`, streaming markdown | `ChatMessage` error branch + Retry + `FallbackOffers` | `ModelPill` "unavailable" rows; `HealthBanner` via `Workspace` |
| Agent pane (`AgentPane`) | `AgentBlankSlate` | `AgentBlankSlate` "Looking for agents…" | `FailureNotice` + Terminal + Retry | "Running without notebook access" notice |
| Reader (`ReaderPane`) | "No text stored for this source" | per-view spinners (source, live page, PDF pages, repo) | image / PDF / import failure lines | online-only placeholder + Download; live-view fallback; anchor downgrade |
| Studio (`StudioPanel`) | `EmptyState` "No notes yet" | per-tile spinners; streaming preview | none — generation failures are toast-only | "stale" badge; archived group |
| Gallery (`GalleryPane`) | `EmptyState`, four variants | none | none — thumbnail failures swallowed | per-card "Import failed" strip |
| Ledger (`LedgerPane`) | `EmptyState` "Nothing on the record yet" | "Loading…" | none — catch renders as empty | none |
| Graph (`GraphView`) | `EmptyState` "Nothing to graph yet" | two-stage progress with bar | none — catch renders as empty | none |
| Settings (`SettingsDialog`) | per-row "Not installed" | dialog spinner; per-section "Loading…" | Notion token error; hosted-agent failure + Sign in | dimmed "not installed" rows |
| Models (`ModelsTab`) | `FirstRunDoor` "None found" | per-provider "checking…" | probe error pill + message | "unavailable" pill per provider |
| Activity (`ActivityTab`) | "Your activity will appear here" | `Spinner` | "Couldn't load activity" | none |
| Command palette (`CommandPalette`) | "No matching commands" | live stage line + `Spinner` | weak — the failure is written into the answer slot as plain prose | none |
| Note window (`NoteWindow`) | "This note no longer exists" | `Spinner` + "Loading note…" | none | none |
| Reports (`Reports`) | `EmptyState` "No reports scheduled" | per-row run spinner | none | none |
| App shell (`App`) | — | — | `ErrorBoundary`, `FatalOverlay` | `HealthBanner`, `Onboarding`, `MigrationOverlay` |

### The degraded states, designed

`HealthBanner` owns the four the app can actually detect, each with the fix
inline rather than a sentence about the fix. It renders on both the shelf
(`HomeView`) and inside a notebook (`Workspace`), and stacks at most three
rows. Its live region stays mounted even when nothing is wrong, so a problem
that appears mid-session is spoken rather than discovered.

| State | Detected by | Says | Fix inline |
|---|---|---|---|
| Ollama isn't running | `modelHealth.reachable === false` with a broken role | "Alchemy needs it to answer questions and to index sources." | **Start Ollama** (`ollama serve` in Terminal), Check again |
| No model set for a role | role's `name` is blank | "Pick one in Settings and this works again." | **Choose a model** → Models |
| Model isn't installed | role detail names `ollama pull <model>` | "Install `<model>` and chat can answer again." | **Install `<model>`**, Check again |
| Search index is incomplete | `reindexPending()` in `src/lib/reindex.ts` | "A re-index didn't finish, so some sources won't appear in search or citations." | **Rebuild now**, Ignore |

Tone follows the stakes: `destructive` when nothing can be answered,
`warning` when the app runs but returns less than it should. The two Terminal
commands are the ones `terminal_command_allowed` permits; nothing else may be
launched from a banner.

The fourth state — "the embedding model changed" — is not a state the store
can be asked about. `chunk_count` on a source is written at ingest and is not
zeroed when a rebuild drops the index, so it cannot answer "is this
indexed?". What `reindex.ts` records instead is narrower and true: a rebuild
this app started and never saw finish. Only the Settings save path stamps it
today, so a model swapped through MCP or the config file will not raise the
banner — widen the stamp when another path starts dropping the index.

### Known gaps

Recorded so the next pass starts from fact:

- **Errors that render as empty.** `GraphView`, `LedgerPane`,
  `CommandPalette` search, and `HomeSections`' filed groups each `.catch()`
  into an empty result, so a failed fetch is indistinguishable from "there is
  nothing here."
- **No error state at all.** Registry (`load()` has no catch), Studio
  (generation failures are toast-only), Gallery, Note window, Reports.
- **No loading state.** The shelf, Registry, Studio's note list, Gallery, and
  the Sources list all render straight off the store with no per-collection
  loading flag, so a slow first load looks like an empty pane.
- **No frontend tests cover any of this.** The contrast matrix and the
  history stack are unit-tested; no component renders under test, so the
  table above is maintained by reading, not by CI.
