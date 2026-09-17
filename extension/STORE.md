# Store listing copy

Paste-ready text for the Chrome Web Store and AMO listing forms. The name,
summary, description, URLs, and art are shared; the two stores differ only in
the permission questions at the end.

## Name

Alchemy Web Clipper

## Summary (Chrome "short description" ≤132 chars; AMO summary ≤250)

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

Private by construction: the extension collects nothing, stores nothing, and
talks to no remote server. In Chrome, clipping a whole page reads that page
and sends it to the Alchemy app on your own machine (`127.0.0.1`), so private
and login-walled pages capture too; every other clip composes an alchemy://
link and hands it to the app. Everything stays on your Mac. The first click
shows the browser's standard "Open Alchemy.app?" confirmation; tick the
remember box to skip it in future.

Requires the Alchemy app for macOS (free, open source, MPL-2.0):
https://thrashr888.github.io/alchemy/

## Category / language

Productivity · English

## URLs for the listing form

- Homepage: https://thrashr888.github.io/alchemy/
- Support: https://github.com/thrashr888/alchemy/issues
- Privacy policy: https://thrashr888.github.io/alchemy/privacy.html

## Chrome privacy questionnaire answers

- Single purpose: send the current page (its rendered content, or just the
  URL), a link URL, or selected text to the Alchemy app on the user's Mac.
- Data collected: none. The page is delivered only to the user's own
  machine and nothing is retained by the extension.
- `contextMenus` justification: adds the three right-click clipping
  actions (page, link, selection).
- `activeTab` + `scripting` justification: reads the current tab's content
  only when the user clicks the toolbar button or a clipping menu item, to
  capture the page for the app.
- `http://127.0.0.1/*` host permission justification: hands the captured
  page to the local Alchemy app's receiver; no other host is contacted.
- Remote code: none. No analytics, no external (non-localhost) requests.

## AMO (Firefox) submission

Upload `extension/dist/firefox.zip` at
https://addons.mozilla.org/developers/. The account is free; signing is
required either way, because release Firefox refuses unsigned add-ons even
when you distribute them yourself. Listing steps are in `README.md`.

Listing fields beyond the shared copy above:

- Category: Productivity (under Extensions).
- License: MPL-2.0, matching the app.
- Data collection: none. The manifest says the same thing in
  `browser_specific_settings.gecko.data_collection_permissions`.
- Screenshots: both files in `store/` apply unchanged.

Note to reviewers (paste into the reviewer-notes field):

> The add-on is one file, `background.js`, with no build step, no bundler,
> and no remote code. It contacts no remote host. `alchemy://add?url=…` is
> the deep link of Alchemy, a free open-source macOS app
> (https://github.com/thrashr888/alchemy); clicking the button navigates the
> current tab to that link, which Firefox confirms with its usual external-
> application prompt. The Firefox build requests only `contextMenus` and
> `activeTab`, and it sends page content nowhere: the Chrome build's
> localhost handoff is gated on a fixed `chrome-extension://` origin the app
> allowlists, which a `moz-extension://` UUID cannot match.

Two Firefox-specific differences from the Chrome listing, if asked:

- Permissions are narrower. No `scripting`, no host permissions, so the
  install prompts for nothing.
- No rendered-DOM capture. Firefox assigns each install its own
  `moz-extension://` origin, which the app cannot allowlist in advance
  without trusting every extension on the machine; the Firefox build clips
  URLs, links, and selections, and Alchemy fetches the page itself.
