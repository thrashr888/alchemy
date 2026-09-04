/**
 * Typed errors for the IPC/data layer. `IpcError` wraps any failure crossing the
 * Tauri boundary; `TimeoutError` is raised when a call exceeds its budget.
 */
export class IpcError extends Error {
  readonly _tag = "IpcError" as const;
  readonly command: string;

  constructor({ command, message }: { command: string; message: string }) {
    super(message);
    this.name = "IpcError";
    this.command = command;
  }
}

export class TimeoutError extends Error {
  readonly _tag = "TimeoutError" as const;
  readonly command: string;

  constructor({ command }: { command: string }) {
    super(`${command} timed out`);
    this.name = "TimeoutError";
    this.command = command;
  }
}

export type AppError = IpcError | TimeoutError;

/** Human names for the commands a user is likely to see time out; anything
 *  unmapped falls back to "The request" rather than leaking the IPC name. */
const ACTIVITIES: Record<string, string> = {
  chat: "Answering your question",
  ask_everything: "Searching your notebooks",
  generate_artifact: "Generating the document",
  generate_notebook_summary: "Summarizing the notebook",
  rebuild_note: "Rebuilding the note",
  export_audio: "Exporting the audio",
  add_source_file: "Importing the file",
  add_source_folder: "Importing the folder",
  add_source_url: "Importing the page",
  add_source_text: "Importing the text",
  add_source_mac: "Importing the source",
  import_notebook_okf: "Importing the notebook",
};

/** Commands that actually wait on a model, and so deserve the "is Ollama
 *  running?" hint when they time out. Everything else is local work. */
const MODEL_BOUND = new Set([
  "send_message",
  "generate",
  "generate_artifact",
  "run_report",
  "reembed_all",
  "ask_everything",
  "second_look",
  "run_second_look",
  "deep_research",
]);

/** Render an AppError (or anything) as a human-friendly message for the UI. */
export function describe(error: unknown): string {
  if (error instanceof TimeoutError) {
    const what = ACTIVITIES[error.command] ?? "The request";
    // Only blame the model when a model was involved. Every command shares
    // this path, so a slow database read used to arrive as advice about
    // Ollama — which sent people to check something that was never wrong.
    const modelBound = MODEL_BOUND.has(error.command);
    return modelBound
      ? `${what} timed out. If you're using Ollama, check that it's running and the model is loaded.`
      : `${what} timed out. It may just be busy — try again.`;
  }
  const message = error instanceof Error ? error.message : String(error);
  if (/too many open files|\bos error (?:23|24)\b|\b(?:EMFILE|ENFILE)\b/i.test(message)) {
    return "Alchemy reached the system's open-file limit. Let current imports finish, then try again. If it keeps happening, restart Alchemy.";
  }
  // Lance errors can include dependency source paths and line numbers.
  // Keep those in diagnostics rather than showing a Rust build path in a
  // source card. Other errors retain their specific recovery advice.
  if (/\bLanceError\b|\.cargo[\\/]registry[\\/]src[\\/]/i.test(message)) {
    return "Alchemy couldn't read or update its local database. Try again. If it keeps happening, restart Alchemy.";
  }
  return message;
}
