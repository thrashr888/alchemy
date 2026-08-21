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

/** Render an AppError (or anything) as a human-friendly message for the UI. */
export function describe(error: unknown): string {
  if (error instanceof TimeoutError) {
    const what = ACTIVITIES[error.command] ?? "The request";
    return `${what} timed out. If you're using Ollama, check that it's running and the model is loaded.`;
  }
  if (error instanceof IpcError) {
    return error.message;
  }
  if (error instanceof Error) return error.message;
  return String(error);
}
