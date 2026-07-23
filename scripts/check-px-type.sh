#!/usr/bin/env bash
#
# Straggler sweep: fail if scalable UI type is written in px instead of the
# rem type tokens, so the whole UI keeps tracking the macOS Accessibility text
# size (see DESIGN.md section 3 and src-tauri/src/textsize.rs).
#
# Two checks:
#   1. Tailwind `text-[Npx]` literals anywhere in the frontend  -> always wrong;
#      use a semantic `text-*` class, or an arbitrary rem literal for a rare
#      size (e.g. text-[1.0625rem] for 17px).
#   2. Raw CSS `font-size: Npx` OUTSIDE the two intentional exceptions:
#        - @media print blocks (print/PDF is a fixed document, must not scale)
#        - .slide-* rules (the 960x540 slide canvas is a document, not chrome)
#      Everything else must be rem.
#
# Exits non-zero on any hit. ASCII-only on purpose (set -u + non-ASCII bytes
# after a $VAR can abort mid-script).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

status=0

# 1. text-[Npx] in the frontend (tsx/ts/css).
if rg -n --glob 'src/**/*.{tsx,ts,css}' 'text-\[[0-9]+(\.[0-9]+)?px\]'; then
  echo ""
  echo "FAIL: px font-size literals above. Convert to a semantic text-* token"
  echo "      (text-micro/caption/body/card/section/page, text-badge for count"
  echo "      badges) or an arbitrary rem literal for a rare size."
  status=1
fi

# 2. Raw font-size:Npx in CSS, skipping @media print and .slide-* blocks.
css_hits="$(
  rg -l --glob 'src/**/*.css' 'font-size' 2>/dev/null | while read -r f; do
    awk -v F="$f" '
      function count(s, ch,   n, i) {
        n = 0
        for (i = 1; i <= length(s); i++) if (substr(s, i, 1) == ch) n++
        return n
      }
      {
        # Evaluate this line against the skip state established by the openers
        # on earlier lines (a font-size line sits below its selector/{ ).
        if (skip == 0 && $0 ~ /font-size:[ \t]*[0-9.]+px/) {
          print F ":" FNR ":" $0
        }
        opener = ($0 ~ /@media[ \t]+print/) || ($0 ~ /slide/)
        o = count($0, "{"); c = count($0, "}")
        for (i = 0; i < o; i++) { depth++; sk[depth] = (skip > 0 || opener) ? 1 : 0; if (sk[depth]) skip++ }
        for (i = 0; i < c; i++) { if (depth > 0 && sk[depth]) skip--; if (depth > 0) delete sk[depth]; if (depth > 0) depth-- }
      }
    ' "$f"
  done
)"

if [ -n "$css_hits" ]; then
  printf '%s\n' "$css_hits"
  echo ""
  echo "FAIL: raw px font-size above (outside @media print / .slide-* canvas)."
  echo "      Convert to rem so it scales with the system text size."
  status=1
fi

if [ "$status" -eq 0 ]; then
  echo "OK: no scalable px type found."
fi

exit "$status"
