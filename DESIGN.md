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
(`DitherBackground`) — the app's only decorative element: behind content,
tinted from theme tokens, static under `prefers-reduced-motion`, and hidden
entirely under glass (the material is the ambience there).

The app is themeable (21 schemes, dark and light) — never hardcode a hex in a
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

- `app-root` — the window chrome: `--background` nudged toward
  `--foreground` (4% light / 7% dark); under `.glass` it drops to 55%
  opacity over the vibrancy.
- `side-card` — panel cards: light schemes are border-only (`--background`
  fill + hairline + 4% shadow, the Vercel/Linear treatment); dark schemes
  get a tonal lift (`--surface` mixed 4% toward foreground); under `.glass`,
  70% translucent.
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

| Style | Size / weight | Usage |
|---|---|---|
| Page title | 22px / 600, tight tracking | Home "Your notebooks" |
| Section title | 15px / 600 | Hero headings, app name |
| Body / controls | 13px / 400–500 | Default UI text, buttons, inputs, prose |
| Card title | 13–14px / 500 | Notebook and note cards |
| Caption | 12px / 400 | Toasts, metadata, hints |
| Micro-label | 11px / 500, uppercase + tracking-wide | Panel headers ("SOURCES", "NOTES") |

Floors: 11px is the minimum text size anywhere; 10px only for numeric count
badges. Chat prose is 13px at line-height 1.65 (user-adjustable 12/13/15px).
Never introduce a webfont; the system stack is deliberate.

## 4. Component Stylings

- **Buttons** (`ui.Button`): heights 28px (`sm`, icon) / 32px (`md`); radius 6px.
  Variants: `primary` (accent fill, subtle 1px shadow), `secondary`
  (surface-2 fill + strong border), `ghost` (text-only, surface-2 on hover),
  `danger` (10% destructive tint). Focus: 2px ring in `--ring`. Disabled: 50%
  opacity, no pointer events.
- **Inputs / Textareas**: 32px tall, `surface-2` fill, 1px `--input` border,
  radius 6px; focus swaps border to `ring/70` plus a 1px ring — no glow.
- **Cards / list rows**: `surface` fill, 1px `--border`, radius 6–10px; hover
  raises to `surface-2` and `--border-strong`. Clickable cards are
  keyboard-operable (`role="button"`, `tabIndex=0`, Enter/Space — use
  `cardButtonProps` from `lib/utils`). Row actions hidden until
  hover **or focus-within**, never hover-only.
- **Menus**: `menu-glass` material (see §2), hairline edge (see §6), radius 6px, 13px items;
  open focuses the first item, arrows cycle, Escape closes and restores focus,
  `role="menu"`/`menuitem`.
- **Modals**: `elevated`, radius 10px, hairline + soft shadow, 44px header
  with 13px semibold title; scrim `black/40` with 2px backdrop blur. Escape
  closes; focus is trapped and restored. Confirmations use the app modal,
  never `window.confirm`.
- **Toasts**: bottom-center, `elevated/90` with backdrop blur, status-tinted
  border, 12px text.
- **Icons**: lucide, 16px (`h-4`) in headers/toolbars, 14px (`h-3.5`) in dense
  rows and inline actions. Nothing interactive below 14px. Icon-only buttons
  always carry `aria-label` (and usually `title`). Monochrome by default —
  see the icon color policy in §2.
- **Empty states** (`ui.EmptyState`): centered small icon + 13px title +
  one gray sentence. Every empty section uses it — no bare paragraphs.
- **Tool confirmations** (chat): process, not conversation — one quiet
  12px gray row with a 12px icon, no bubble, no role label.
- **Document properties** (`DocProperties` in the reader): Linear-style
  label/value rows (type, origin, dates, size) at the top of a document,
  12px, hairline-separated from content. Answers "what is this" before
  the prose.

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

## 9. Agent Prompt Guide

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
- Clickable non-button elements: spread `cardButtonProps(onActivate)` and add
  `cursor-pointer`; reveal row actions with
  `opacity-0 group-hover:opacity-100 group-focus-within:opacity-100`.
- New keyboard shortcuts: add the listener at the owning component, guard with
  `shortcutBlocked(e)`, and register the shortcut in `SHORTCUTS` in
  `SettingsDialog.tsx` so the Shortcuts tab stays truthful.
- Text sizes: pick from 11/12/13/15/22px. Radii: `rounded-md` (6px) controls,
  `rounded-lg` (10px) overlays/cards. Icons: `h-4` toolbar, `h-3.5` dense.
- Example prompt: "Add a 'pin source' action to each source row: h-3.5 lucide
  Pin icon button, hidden until hover/focus-within, aria-label with the source
  title, confirm nothing, toast on success."
