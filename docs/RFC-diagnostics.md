# RFC: Diagnostics — errors and crashes that leave a trace

Status: accepted

## Summary

Until now, a failure in Alchemy left nothing behind. The backend's 170-odd
`eprintln!`s go to a terminal that only exists under `pnpm tauri dev`; in the
installed app, launched from Finder, stdout and stderr are `/dev/null`. The
front-end had no `window.onerror`, no `unhandledrejection` handler, and no
React error boundary — a component that threw on render produced a white
window with no record anywhere. The unified log had zero entries from the app.
`~/Library/Logs/DiagnosticReports/` had no crash reports either, which is the
one honest signal in the list: `panic = unwind` means a panic inside a
`#[tauri::command]` unwinds into an `Err` string rather than killing the
process, so most of what feels like a crash never produces a crash report.

The net effect was that every bug arrived as a description from memory.

This adds one durable record of every failure, on both sides of the IPC
boundary, plus a way out of the states that leave the app unusable.

## Where records go

| Destination | What it is for |
| --- | --- |
| `~/Library/Logs/com.thrashr888.alchemy/alchemy.log` | The record. JSONL, one object per event, rotated at 2 MB keeping one generation. Under `~/Library/Logs` so Console.app lists it beside the crash reports, and so it survives an app-data reset. |
| The unified log (`os_log`) | Live tailing and system context: `log stream --predicate 'subsystem == "com.thrashr888.alchemy"'`, or Console.app filtered on that subsystem. Errors are `error` type, fatals are `fault`, so both persist without the user enabling anything. |
| stderr | Dev builds and `tauri-browser logs`. Free, and the first thing written. |
| `recent_errors` (IPC + MCP) | Reading the record back without knowing the path — for the UI, for a support conversation, and for an agent asked to fix the bug. |

A record is `{ts, time, level, origin, kind, message, version}` plus optional
`detail` (backtraces, component stacks) and `context` (whatever structured
extras the call site has). `level` is info | warn | error | fatal, `origin` is
rust | js, and `kind` is a short tag — `panic`, `ipc`, `render`,
`unhandled-rejection`, `startup`, `sweep`, `mcp` — that makes the log
greppable by failure shape rather than by wording.

## What gets captured

**Rust panics.** A process-wide hook installed on the first line of `run()`,
before the Tauri builder exists, so a panic during `setup` is recorded rather
than being a silent bounce in the Dock. It captures the payload, the location,
the thread, and a forced backtrace, then chains to the previous hook. The hook
body runs inside `catch_unwind`: a panic raised inside a panic hook aborts the
process, and turning a logged panic into a hard crash would defeat the point.

**Every failed IPC call.** `api.ts`'s `run()` is the single point every
backend failure passes through, after retries and timeouts have had their say.
One call there covers all ~200 commands and records the command name with the
message, which is the piece the user's description always lacks.

**Front-end throws and rejections.** `window.onerror` and
`unhandledrejection`, installed before React mounts. Unhandled rejections
matter most: they are how a front-end bug stays invisible, because the UI
simply never updates and nothing is thrown at anyone.

**Render crashes.** A root error boundary, with the component stack alongside
the JS stack — neither is derivable from the other.

**Silent background failures.** The sweeps that fail where no user is looking
— gist, enrichment, tags, night-shift reports, db maintenance — and the two
servers whose failure to bind is invisible until an agent can't connect (MCP,
clip).

## Not getting in an unrecoverable state

Capture is half of it. The other half is that the app must never leave the
user with a dead window and no next step.

- **Startup failures** used to be `.expect()`: no window, no message, nothing
  in any log. They now record a fatal and show a native alert naming the
  failure and the log path before exiting. The realistic case is a database
  left half-written by a hard kill.
- **Render crashes** show a recovery screen — what broke, `Restart Alchemy`,
  `Try again` (remounts the subtree, which is enough for a transient state
  bug), `Copy details`, and `Show log`. It renders outside the app's `Modal`
  on purpose: a modal renders inside the tree that just failed.
- **Backend fatals** emit `app://fatal`, which the front-end turns into the
  same screen. The last fatal is also readable via `pending_fatal`, so a
  window that reloaded past the event still shows the way out.
- **Restart** goes through `tauri-plugin-process`'s `relaunch()`, falling back
  to a webview reload if relaunch itself fails — that still clears a
  front-end fatal, which is the common case.
- **Help → Show Diagnostics Log** reveals the file in Finder, so "send me the
  log" is one menu item rather than a walk through a hidden directory.

## Two invariants

**Recording must never fail loudly.** Every path swallows its own errors. A
logger that can throw hands the caller a second error inside its error path,
which is how logging becomes a loop. The front-end reporter is guarded against
re-entry; the Rust side never `unwrap`s; a poisoned throttle admits the write
rather than silencing it.

**A flood must not become the log.** Identical (kind, message) pairs write at
most three times per minute, and every hundredth repeat thereafter carries the
running count so a runaway loop stays visible without being the whole file.
There is a global ceiling of 120 records per minute. Both sides throttle: the
front-end's copy exists so a component throwing on every render doesn't
generate an IPC storm on the way to being suppressed.

## The os_log mirror

Reaching the unified log with a real subsystem takes FFI —
`os_log_create` plus `_os_log_impl` — and two details that are easy to get
wrong and produce `<compose failure>` instead of a message:

- The format string must live in `__TEXT`. A Rust `static` lands in
  `__DATA_CONST`, where the log decoder can't find it by (image uuid, offset);
  `#[link_section = "__TEXT,__cstring"]` fixes it.
- The `dso` argument must be the mach header of the image that owns that
  string. `__dso_handle` is not it in a Rust binary — `dladdr` on the format
  string's address gives the right base.

The call is fixed-shape on purpose: one format string, one `%{public}s`
argument, always the same 12-byte buffer. Message text is passed as data, so a
message containing `%s` or `%@` is inert. It runs after the record is already
on disk, because an FFI bug on the panic path would be a crash inside a crash.
A unit test drives it with empty, oversized, NUL-bearing, and
format-specifier-bearing input.

## Agent surface

`recent_errors` is an MCP tool as well as an IPC command — the app's own error
log, readable by the agent being asked to fix it. Read-only: an agent can see
what broke, never edit the record of it. It is the answer to "I can't
reproduce it": the failure is usually already recorded.

## As built: crashes with no panic behind them (crashwatch.rs)

The paragraph above about `DiagnosticReports` being empty was true of the
failures this RFC set out to catch, and it hid the ones it didn't. A user
clicked a feed follow, the app disappeared, and nothing in `alchemy.log` said
so — because the panic hook only sees Rust's unwind path. Two shapes escape it
entirely:

- **A native crash.** WKWebView, the Lance/ONNX FFI, or anything else raising
  SIGSEGV/SIGABRT/SIGBUS kills the process where it stands. No hook runs. The
  only record is the `.ips` report macOS files in
  `~/Library/Logs/DiagnosticReports`, which nobody opens.
- **A plain exit.** An `exit(3)` from a dependency, a kill, a power loss.
  Nothing is written anywhere by anyone.

Neither can be caught as it happens. Both are caught at the next launch.

**The running stamp.** `setup` writes `running.json` (pid, version, start
time) into app-data; `RunEvent::Exit` deletes it. A stamp still present at the
next launch means the previous run never reached its shutdown path, and
records at `error` with that run's version, its lifetime, and the last ten log
lines it wrote — usually the whole diagnosis. Lifetime is dated from the last
line that run logged, which is approximate on purpose: a process that vanishes
cannot record the moment it stopped. A stamp whose pid is still alive is a
second Alchemy, not a crash; that records at `info` and the file is left to
its owner.

**The crash-report scan.** Off the main thread once the window is up,
`DiagnosticReports` is read for `.ips` files newer than a watermark in
app-data (`crash-scan.json`) whose header line names this app by `bundleID`,
`app_name`, or `coalitionName`. Each becomes one `fatal` record with the
exception type and signal, the termination reason, and the crashed thread's
top twelve frames resolved against `usedImages`, so the line naming the
faulting library is right there. A first-ever scan looks back 24 hours, or a
fresh install on an old Mac would replay every crash the machine ever had. The
legacy `.crash` text format gets its presence and path noted, nothing more.
The reports themselves are never deleted or modified — they belong to macOS.

**These fatals are quiet.** `Event::quiet()` records at `fatal` and skips
`app://fatal`. A crash report is a real fatal that belongs in the log and in
`recent_errors`, but it happened to a process that no longer exists; raising
the restart screen over it would ask the user to escape a run that already
ended. What they get instead is one line in the health banner — "Alchemy
crashed last time. The report is in the log." — with Reveal and Dismiss,
driven by `crash_notice`. It is not persisted: the notice belongs to the
launch after the crash, not to every launch thereafter.

**Window closes record at `info`.** "The app quit" and "the window closed"
are indistinguishable in a bug report and are not the same event. With the
menu-bar icon on, closing the main window hides it and the process stays
resident — which is exactly the state a user describes as quitting.

**No signal handler.** A SIGSEGV/SIGABRT/SIGBUS handler writing a last-gasp
marker was considered and skipped. To be correct it may only `write(2)` to a
pre-opened fd — no allocation, no formatting, no `note!` — and must re-raise
the default action, and it would compete for those signals with the handlers
WebKit and the system crash reporter install. It would buy one bit ("we died
by signal") that the `.ips` scan already reports with the exception type and a
stack, and the stamp covers the signal-free exits the scan cannot see. If that
changes, the constraints above are the bar.

**Agent surface.** Both record kinds — `crash-report` and `unclean-exit` —
flow through `recent_errors` over IPC and MCP like everything else, and the
MCP tool description names them, so an agent asked why the app quit knows to
look and knows what it is reading.

## Deliberately not in v1

- **No remote reporting.** Nothing leaves the machine. Alchemy is local-first
  and a crash reporter that phones home would be the first thing to break
  that promise. `Copy details` and `Show log` are how a report gets sent, by
  the user, deliberately.
- **No log level setting.** Errors are rare enough that there is nothing to
  tune; a verbosity dial would only ever be found by someone already lost.
- **No conversion of the remaining `eprintln!`s.** The ones that mark silent
  failure were converted. The rest are progress chatter, and moving them into
  an error log would bury the signal this exists to surface.
