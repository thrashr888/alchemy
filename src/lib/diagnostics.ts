import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { relaunch } from "@tauri-apps/plugin-process";

/**
 * Front-end half of the diagnostics path (docs/RFC-diagnostics.md). Every
 * failure the webview can see — a render crash, an unhandled rejection, a
 * command that came back an error — goes to the same
 * `~/Library/Logs/com.thrashr888.alchemy/alchemy.log` the backend writes, so
 * one file answers "what went wrong" no matter which side broke.
 *
 * The hard rule here is that reporting an error must never itself produce
 * one. Everything below is fire-and-forget, guarded against re-entry, and
 * throttled — an error thrown on every render must not become a flood of IPC
 * calls that wedges the app it was trying to describe.
 */

export type DiagnosticLevel = "info" | "warn" | "error" | "fatal";

export interface DiagnosticRecord {
  ts: number;
  time: string;
  level: DiagnosticLevel;
  origin: "rust" | "js";
  kind: string;
  message: string;
  detail?: string;
  context?: Record<string, unknown>;
  repeated?: number;
}

export interface DiagnosticsReport {
  summary: {
    path: string;
    retained: number;
    fatal: number;
    error: number;
    warn: number;
    info: number;
    fatalsThisSession: number;
  };
  records: DiagnosticRecord[];
}

/** Set while a report is in flight, so a failure inside reporting can't recurse. */
let reporting = false;

/** (kind + message) -> count within the current window, mirroring the backend throttle. */
const seen = new Map<string, number>();
let windowStart = Date.now();
const WINDOW_MS = 60_000;
const DUP_LIMIT = 3;

function admit(key: string): boolean {
  const now = Date.now();
  if (now - windowStart > WINDOW_MS) {
    windowStart = now;
    seen.clear();
  }
  const count = (seen.get(key) ?? 0) + 1;
  seen.set(key, count);
  return count <= DUP_LIMIT;
}

/**
 * Record a front-end failure. Never throws and never returns a rejected
 * promise: callers use it from inside catch blocks, where a second failure
 * has nowhere to go.
 */
export function report(
  level: DiagnosticLevel,
  kind: string,
  message: string,
  detail?: string,
  context?: Record<string, unknown>,
): void {
  if (reporting) return;
  if (!admit(`${kind}${message}`)) return;
  // The browser dev build has no backend to report to; the console is the
  // only sink there, and it is the one the developer is already watching.
  if (!isTauri()) {
    console.error(`[${level}] ${kind}: ${message}`, detail ?? "", context ?? "");
    return;
  }
  reporting = true;
  void invoke("log_client_error", { level, kind, message, detail, context })
    .catch(() => {
      // The log call itself failed — the console is all that's left.
      console.error(`[${level}] ${kind}: ${message}`, detail ?? "");
    })
    .finally(() => {
      reporting = false;
    });
}

/** Pull the human-readable pieces out of whatever was thrown. */
export function describeThrown(value: unknown): {
  message: string;
  detail?: string;
} {
  if (value instanceof Error) {
    return { message: value.message || value.name, detail: value.stack };
  }
  if (typeof value === "string") return { message: value };
  try {
    return { message: JSON.stringify(value) };
  } catch {
    return { message: String(value) };
  }
}

// ---- Fatal state ------------------------------------------------------------

/**
 * A fatal is anything that leaves the app unable to carry on: a React tree
 * that threw on render, or a backend panic the Rust side raised. The UI shows
 * a restart affordance rather than leaving the user with a dead window — the
 * one thing worse than crashing is crashing quietly.
 */
export interface FatalState {
  origin: "rust" | "js";
  kind: string;
  message: string;
  detail?: string;
}

type FatalListener = (fatal: FatalState) => void;
const fatalListeners = new Set<FatalListener>();
let currentFatal: FatalState | null = null;

export function getFatal(): FatalState | null {
  return currentFatal;
}

export function onFatal(listener: FatalListener): () => void {
  fatalListeners.add(listener);
  return () => fatalListeners.delete(listener);
}

/** Raise the app-level fatal state, logging it on the way through. */
export function raiseFatal(fatal: FatalState, alreadyLogged = false): void {
  if (!alreadyLogged) {
    report("fatal", fatal.kind, fatal.message, fatal.detail, {
      origin: fatal.origin,
    });
  }
  // First fatal wins: later ones are usually fallout from the first, and
  // swapping the banner's text mid-read helps nobody.
  if (currentFatal) return;
  currentFatal = fatal;
  for (const listener of fatalListeners) {
    try {
      listener(fatal);
    } catch {
      // A listener that throws must not stop the others from hearing.
    }
  }
}

/** Restart the app. The escape hatch offered with every fatal. */
export async function restart(): Promise<void> {
  if (!isTauri()) {
    window.location.reload();
    return;
  }
  try {
    await relaunch();
  } catch (err) {
    // Relaunch can fail (an updater mid-swap, a revoked permission). Reload
    // the webview instead: it clears a front-end fatal, which is the common
    // case, and leaves the user somewhere better than a frozen window.
    report("error", "restart", describeThrown(err).message);
    window.location.reload();
  }
}

// ---- Reading back -----------------------------------------------------------

/** Recent records for the UI. Errors and worse by default. */
export async function recentErrors(
  limit = 50,
  minLevel: DiagnosticLevel = "error",
): Promise<DiagnosticsReport | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<DiagnosticsReport>("recent_errors", {
      limit,
      minLevel,
    });
  } catch {
    return null;
  }
}

/** Show the log file in Finder. */
export async function revealLog(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("reveal_log");
  } catch (err) {
    report("warn", "reveal-log", describeThrown(err).message);
  }
}

// ---- Installation -----------------------------------------------------------

let installed = false;

/**
 * Install the global handlers. Called once from `main.tsx`, before React
 * mounts, so an error thrown during the first render is already covered.
 */
export function installDiagnostics(): void {
  if (installed) return;
  installed = true;

  // Synchronous throws that escaped every component boundary.
  window.addEventListener("error", (event) => {
    // Resource load failures (a missing image) surface here too and are not
    // app errors; they carry no `error` object.
    if (!event.error && !event.message) return;
    const { message, detail } = describeThrown(event.error ?? event.message);
    report("error", "window-error", message, detail, {
      source: event.filename,
      line: event.lineno,
      column: event.colno,
    });
  });

  // Rejected promises nobody caught — the most common way a front-end bug
  // stays invisible, since the UI just quietly never updates.
  window.addEventListener("unhandledrejection", (event) => {
    const { message, detail } = describeThrown(event.reason);
    report("error", "unhandled-rejection", message, detail);
  });

  if (!isTauri()) return;

  // A fatal the backend raised. Usually a wedged backend, but a front-end
  // fatal reported through `report()` echoes back here too — so the record's
  // own origin decides the wording, not the direction it arrived from.
  void listen<DiagnosticRecord>("app://fatal", (event) => {
    const record = event.payload;
    raiseFatal(
      {
        origin: record?.origin === "js" ? "js" : "rust",
        kind: record?.kind ?? "panic",
        message: record?.message ?? "The backend stopped responding.",
        detail: record?.detail,
      },
      // The backend already wrote this record; logging it again from here
      // would double every backend fatal in the file.
      true,
    );
  });

  // A window that reloaded past the event still needs to know. Ask once.
  void invoke<DiagnosticRecord | null>("pending_fatal")
    .then((record) => {
      if (!record) return;
      raiseFatal(
        {
          origin: record.origin === "js" ? "js" : "rust",
          kind: record.kind ?? "panic",
          message: record.message,
          detail: record.detail,
        },
        true,
      );
    })
    .catch(() => {
      // Nothing to recover: no answer means no pending fatal worth showing.
    });
}
