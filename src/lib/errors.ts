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

/** Render an AppError (or anything) as a human-friendly message for the UI. */
export function describe(error: unknown): string {
  if (error instanceof TimeoutError) {
    return `"${error.command}" timed out. Is Ollama running and the model loaded?`;
  }
  if (error instanceof IpcError) {
    return error.message;
  }
  if (error instanceof Error) return error.message;
  return String(error);
}
