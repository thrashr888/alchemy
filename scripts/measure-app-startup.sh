#!/bin/zsh
# Measure the same cold-launch interval a Dock click starts: LaunchServices
# `open` until the app records a requested startup phase. The app must not be
# running; this script refuses to interrupt background work or an open window.

set -euo pipefail

app_path="${1:-/Applications/Alchemy.app}"
phase="${2:-window_interactive}"
trace_path="${HOME}/Library/Application Support/com.thrashr888.alchemy/traces/startup.jsonl"

if [[ ! -d "$app_path" ]]; then
  print -u2 "Alchemy app not found: $app_path"
  exit 2
fi

if pgrep -x alchemy >/dev/null; then
  print -u2 "Alchemy is already running. Quit it cleanly before measuring a cold launch."
  exit 2
fi

start_ms=$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')
open "$app_path"

# A healthy release currently reaches an interactive window in a few Dock
# bounces. Twenty seconds is long enough to catch a real regression while
# still turning a wedged launch into an explicit failure.
for _ in {1..400}; do
  if [[ -f "$trace_path" ]]; then
    # Read raw lines and ignore only an incomplete line at EOF. The app may be
    # between write(2) calls while this polling loop snapshots the trace.
    result=$(jq -Rsc \
      --arg phase "$phase" \
      --argjson start "$start_ms" \
      '
        [split("\n")[] | fromjson?]
        | group_by(.boot)
        | map(
            select((map(.ts) | min) >= ($start - 250))
            | . as $boot
            | (map(select(.phase == $phase)) | last) as $target
            | select($target != null)
            | {
                boot: .[0].boot,
                version: .[0].version,
                phase: $target.phase,
                openToPhaseMs: ($target.ts - $start),
                inProcessMs: $target.ms,
                phases: map({phase, ms})
              }
          )
        | sort_by(.openToPhaseMs)
        | first // empty
      ' "$trace_path")
    if [[ -n "$result" ]]; then
      print -r -- "$result"
      exit 0
    fi
  fi
  sleep 0.05
done

print -u2 "Alchemy did not record $phase within 20 seconds."
exit 1
