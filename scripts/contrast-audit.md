# Theme contrast audit

Audits every theme for WCAG text contrast on the live app — no
screenshots. `contrast-audit.js` walks visible text, composites each
element's effective background up the tree, and reports ratios below
4.5:1 (3:1 for large text) plus any text whose background chain ends
transparent (the "void background" bug class).

Run against the dev app (debug bridge required):

    # single theme, current view
    tauri-browser run-js "$(tr '\n' ' ' < scripts/contrast-audit.js)"

To sweep all themes in one eval, wrap the audit body in a loop over
`window.__store.getState().setTheme(id)` — applyTheme is synchronous.
Disable transitions first (`*{transition:none!important}`) or
mid-transition colors poison the numbers. Navigate to each surface
(home, chat, reader, settings) and re-run; the audit only sees what
is mounted.
