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

- Home stays on its inset cards until the Registry and Brief usage counts
  from docs/RFC-ablation.md items 4 and 7 exist.
- The composer's three pills, the Sources chip strip, and the DEV badge
  keep their places for now; each is a separate small change.
- Sidebars honoring the macOS accent color is an Appearance option to add,
  not a default: themes decide selection color.

## Known tension

Under glass with a bright desktop the transparent center column reads
light and the theme's text loses contrast (visible on Home with Night
City). That predates this work; the fix is either a floor on the center's
opacity under glass or a note in Appearance that glass wants a dark
desktop. Decide before glass becomes a default anywhere.
