import { Data } from "effect";

/**
 * Typed errors for the IPC/data layer. `IpcError` wraps any failure crossing the
 * Tauri boundary; `TimeoutError` is raised when a call exceeds its budget.
 */
export class IpcError extends Data.TaggedError("IpcError")<{
  command: string;
  message: string;
}> {}

export class TimeoutError extends Data.TaggedError("TimeoutError")<{
  command: string;
}> {}

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
  if (error instanceof IpcError) {
    return error.message;
  }
  if (error instanceof Error) return error.message;
  return String(error);
}
