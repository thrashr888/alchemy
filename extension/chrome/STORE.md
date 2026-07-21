# Store listing copy

Paste-ready text for the Chrome Web Store (and AMO) listing forms.

## Name

Alchemy Web Clipper

## Summary (Chrome "short description", ≤132 chars)

Clip pages, links, and selections into Alchemy — the local-first research
notebook for macOS. One click, no account.

## Description (full)

Alchemy is a local-first research notebook for macOS: import sources, chat
with them grounded in citations, and turn notebooks into documents and
podcasts — on your machine, with your models.

This extension is the shortest path from browsing to notebook:

• Click the toolbar button to add the current page as a source.
• Right-click a link to add what it points at.
• Right-click selected text to save it as a text source, with the page URL
  kept as provenance.

Alchemy fetches and indexes the page itself, extracts the readable article,
and makes it citable in chat. If the page is a GitHub or git URL, Alchemy's
git-source machinery takes over — README-only by default, or the whole repo
if you choose.

Private by construction: the extension has no host permissions, collects
nothing, stores nothing, and makes no network requests. It composes an
alchemy:// link and hands it to the app — that's the entire mechanism. The
first click shows the browser's standard "Open Alchemy.app?" confirmation;
check "Always allow" to skip it in future.

Requires the Alchemy app for macOS (free, open source, MPL-2.0):
https://thrashr888.github.io/alchemy/

## Category / language

Productivity · English

## URLs for the listing form

- Homepage: https://thrashr888.github.io/alchemy/
- Support: https://github.com/thrashr888/alchemy/issues
- Privacy policy: https://thrashr888.github.io/alchemy/privacy.html

## Privacy questionnaire answers

- Single purpose: send the current page URL, a link URL, or selected text
  to the Alchemy app on the user's Mac via its alchemy:// URL scheme.
- Data collected: none.
- `contextMenus` justification: adds the three right-click clipping
  actions (page, link, selection).
- `activeTab` justification: reads the current tab's URL and title only
  when the user clicks the toolbar button or a clipping menu item.
- Remote code: none. No analytics, no external requests.

## AMO (Firefox) notes

Same copy applies. The manifest carries
`browser_specific_settings.gecko.id = clipper@alchemy.thrasher.dev`
(strict_min_version 121.0), and `background.scripts` alongside
`service_worker` so the same folder loads in both browsers. Submit the
identical zip at https://addons.mozilla.org/developers/.
