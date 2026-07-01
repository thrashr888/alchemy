import { invoke } from "@tauri-apps/api/core";
import { Cause, Duration, Effect, Schedule } from "effect";
import { describe, IpcError, TimeoutError, type AppError } from "./errors";
import type {
  AiConfig,
  Message,
  ModelHealth,
  ModelStat,
  Note,
  NoteKind,
  Notebook,
  ReportSchedule,
  Source,
} from "./types";

/**
 * Effect powers the data layer: every IPC call is wrapped with a timeout and
 * typed errors, and idempotent reads get bounded retries (Ollama can be flaky
 * on cold starts). The public `api` keeps a plain Promise surface so the store
 * and components don't need to know about Effect.
 */

const invokeRaw = <T>(command: string, args?: Record<string, unknown>) =>
  Effect.tryPromise({
    try: () => invoke<T>(command, args),
    catch: (e) => new IpcError({ command, message: String(e) }),
  });

// Retry transient IPC failures (not timeouts) a couple of times with backoff.
const retryPolicy = Schedule.exponential("300 millis").pipe(
  Schedule.intersect(Schedule.recurs(2)),
);

/** Idempotent read: short timeout + bounded retry. */
const query = <T>(command: string, args?: Record<string, unknown>) =>
  invokeRaw<T>(command, args).pipe(
    Effect.timeoutFail({
      duration: Duration.seconds(30),
      onTimeout: () => new TimeoutError({ command }),
    }),
    Effect.retry({ schedule: retryPolicy, while: (e: AppError) => e._tag === "IpcError" }),
  );

/** Quick mutation (DB write): short timeout, no retry (avoid double writes). */
const cmd = <T>(command: string, args?: Record<string, unknown>) =>
  invokeRaw<T>(command, args).pipe(
    Effect.timeoutFail({
      duration: Duration.seconds(30),
      onTimeout: () => new TimeoutError({ command }),
    }),
  );

/** Long-running AI op (embed / generate / chat): generous timeout, no retry. */
const ai = <T>(command: string, args?: Record<string, unknown>) =>
  invokeRaw<T>(command, args).pipe(
    Effect.timeoutFail({
      duration: Duration.minutes(10),
      onTimeout: () => new TimeoutError({ command }),
    }),
  );

/** Run an Effect to a Promise, rejecting with a clean, user-friendly Error. */
async function run<A>(effect: Effect.Effect<A, AppError>): Promise<A> {
  const exit = await Effect.runPromiseExit(effect);
  if (exit._tag === "Success") return exit.value;
  throw new Error(describe(Cause.squash(exit.cause)));
}

export const api = {
  // Notebooks
  listNotebooks: () => run(query<Notebook[]>("list_notebooks")),
  createNotebook: (title: string) => run(cmd<Notebook>("create_notebook", { title })),
  renameNotebook: (id: string, title: string) => run(cmd<void>("rename_notebook", { id, title })),
  deleteNotebook: (id: string) => run(cmd<void>("delete_notebook", { id })),

  // Sources
  listSources: (notebookId: string) => run(query<Source[]>("list_sources", { notebookId })),
  addSourceFile: (notebookId: string, path: string) =>
    run(ai<Source>("add_source_file", { notebookId, path })),
  addSourceUrl: (notebookId: string, url: string) =>
    run(ai<Source>("add_source_url", { notebookId, url })),
  addSourceText: (notebookId: string, title: string, text: string) =>
    run(ai<Source>("add_source_text", { notebookId, title, text })),
  updateSourceText: (sourceId: string, title: string, text: string) =>
    run(ai<Source>("update_source_text", { sourceId, title, text })),
  refreshSourceUrl: (sourceId: string) =>
    run(ai<Source>("refresh_source_url", { sourceId })),
  getSourceContent: (sourceId: string) =>
    run(query<string>("get_source_content", { sourceId })),
  reembedAll: () => run(ai<number>("reembed_all")),
  deleteSource: (sourceId: string) => run(cmd<void>("delete_source", { sourceId })),

  // Chat
  listMessages: (notebookId: string) => run(query<Message[]>("list_messages", { notebookId })),
  sendMessage: (notebookId: string, content: string) =>
    run(ai<Message>("send_message", { notebookId, content })),
  sendMessageAgentic: (notebookId: string, content: string) =>
    run(ai<Message>("send_message_agentic", { notebookId, content })),
  clearChat: (notebookId: string) => run(cmd<void>("clear_chat", { notebookId })),

  // Notes & artifacts
  listNotes: (notebookId: string) => run(query<Note[]>("list_notes", { notebookId })),
  createNote: (notebookId: string, title: string, content: string) =>
    run(cmd<Note>("create_note", { notebookId, title, content })),
  updateNote: (id: string, notebookId: string, title: string, content: string) =>
    run(cmd<void>("update_note", { id, notebookId, title, content })),
  deleteNote: (id: string) => run(cmd<void>("delete_note", { id })),
  convertNoteToSource: (noteId: string) =>
    run(ai<Source>("convert_note_to_source", { noteId })),
  generateArtifact: (notebookId: string, kind: NoteKind, prompt?: string) =>
    run(ai<Note>("generate_artifact", { notebookId, kind, prompt: prompt ?? "" })),
  rebuildNote: (noteId: string, notebookId: string, kind: NoteKind, prompt: string) =>
    run(ai<Note>("rebuild_note", { noteId, notebookId, kind, prompt })),

  // Reports
  listReportSchedules: (notebookId: string) =>
    run(query<ReportSchedule[]>("list_report_schedules", { notebookId })),
  listAllReportSchedules: () => run(query<ReportSchedule[]>("list_all_report_schedules")),
  createReportSchedule: (notebookId: string, name: string, kind: string, prompt: string, intervalSecs: number) =>
    run(cmd<ReportSchedule>("create_report_schedule", { notebookId, name, kind, prompt, intervalSecs })),
  updateReportSchedule: (id: string, name: string, kind: string, prompt: string, intervalSecs: number, enabled: boolean) =>
    run(cmd<void>("update_report_schedule", { id, name, kind, prompt, intervalSecs, enabled })),
  deleteReportSchedule: (id: string) => run(cmd<void>("delete_report_schedule", { id })),
  runReport: (scheduleId: string) => run(ai<Note>("run_report", { scheduleId })),

  // Settings / health
  getAiConfig: () => run(query<AiConfig>("get_ai_config")),
  setAiConfig: (config: AiConfig) => run(cmd<void>("set_ai_config", { config })),
  listModels: () => run(query<string[]>("list_models")),
  checkOllama: () => run(query<boolean>("check_ollama")),
  checkModels: () => run(query<ModelHealth>("check_models")),
  getModelStats: () => run(query<ModelStat[]>("get_model_stats")),
};
