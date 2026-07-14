import { invoke } from "@tauri-apps/api/core";
import { Cause, Duration, Effect, Schedule } from "effect";
import { describe, IpcError, TimeoutError, type AppError } from "./errors";
import type {
  AiConfig,
  ChatConfig,
  ConnectorStatus,
  CorpusStats,
  FolderScan,
  KokoroStatus,
  MacCollection,
  McpStatus,
  Message,
  MetaAnswer,
  ModelHealth,
  ModelStat,
  Note,
  NoteKind,
  Notebook,
  ReportSchedule,
  SearchHit,
  Source,
  Template,
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
    Effect.retry({
      schedule: retryPolicy,
      while: (e: AppError) => e._tag === "IpcError",
    }),
  );

/** Quick mutation (DB write): short timeout, no retry (avoid double writes). */
const cmd = <T>(command: string, args?: Record<string, unknown>) =>
  invokeRaw<T>(command, args).pipe(
    Effect.timeoutFail({
      duration: Duration.seconds(30),
      onTimeout: () => new TimeoutError({ command }),
    }),
  );

/** Fast probe (gateway checks): one attempt, short timeout, no retry. */
const probe = <T>(command: string, args?: Record<string, unknown>) =>
  invokeRaw<T>(command, args).pipe(
    Effect.timeoutFail({
      duration: Duration.seconds(15),
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

/** Marathon op (a 20-minute episode scripts + synthesizes for a long time):
 *  the ceiling exists only to catch a truly wedged backend. */
const slow = <T>(command: string, args?: Record<string, unknown>) =>
  invokeRaw<T>(command, args).pipe(
    Effect.timeoutFail({
      duration: Duration.minutes(60),
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
  createNotebook: (title: string) =>
    run(cmd<Notebook>("create_notebook", { title })),
  renameNotebook: (id: string, title: string) =>
    run(cmd<void>("rename_notebook", { id, title })),
  setNotebookColor: (id: string, color: string) =>
    run(cmd<void>("set_notebook_color", { id, color })),
  deleteNotebook: (id: string) => run(cmd<void>("delete_notebook", { id })),

  // Sources
  listSources: (notebookId: string) =>
    run(query<Source[]>("list_sources", { notebookId })),
  addSourceFile: (notebookId: string, path: string) =>
    run(ai<Source>("add_source_file", { notebookId, path })),
  addSourceFolder: (notebookId: string, path: string) =>
    run(slow<Source>("add_source_folder", { notebookId, path })),
  resyncSources: () => run(slow<FolderScan>("resync_sources")),
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
  deleteSource: (sourceId: string) =>
    run(cmd<void>("delete_source", { sourceId })),

  // Mac providers (Calendar, Reminders, Apple Notes via cider)
  macAvailable: () => run(query<boolean>("mac_available")),
  macConnect: (provider: string) => run(cmd<void>("mac_connect", { provider })),
  listMacCollections: (provider: string) =>
    run(query<MacCollection[]>("list_mac_collections", { provider })),
  addSourceMac: (
    notebookId: string,
    provider: string,
    collection: string,
    label: string,
  ) =>
    run(
      ai<Source>("add_source_mac", { notebookId, provider, collection, label }),
    ),
  macNoteBody: (sourceId: string) =>
    run(query<string>("mac_note_body", { sourceId })),
  updateMacNote: (sourceId: string, body: string) =>
    run(ai<Source>("update_mac_note", { sourceId, body })),
  addMacReminder: (sourceId: string, title: string, notes?: string) =>
    run(
      ai<Source>("add_mac_reminder", { sourceId, title, notes: notes ?? null }),
    ),

  // OS integrations (deep links, tray, Services, Spotlight)
  integrationsReady: () => run(cmd<string[]>("integrations_ready")),
  locateNote: (noteId: string) =>
    run(query<string | null>("locate_note", { noteId })),

  // Chat
  listMessages: (notebookId: string) =>
    run(query<Message[]>("list_messages", { notebookId })),
  sendMessage: (
    notebookId: string,
    content: string,
    config: ChatConfig,
    sourceIds?: string[] | null,
  ) =>
    run(
      ai<Message>("send_message", { notebookId, content, config, sourceIds }),
    ),
  sendMessageAgentic: (
    notebookId: string,
    content: string,
    config: ChatConfig,
    sourceIds?: string[] | null,
  ) =>
    run(
      ai<Message>("send_message_agentic", {
        notebookId,
        content,
        config,
        sourceIds,
      }),
    ),
  cancelGeneration: (scope?: "chat" | "artifact" | "tts" | "meta") =>
    run(cmd<void>("cancel_generation", { scope })),
  suggestFollowups: (notebookId: string) =>
    run(query<string[]>("suggest_followups", { notebookId })),
  generateNotebookSummary: (notebookId: string) =>
    run(ai<string>("generate_notebook_summary", { notebookId })),
  clearChat: (notebookId: string) =>
    run(cmd<void>("clear_chat", { notebookId })),
  addNoteToChat: (noteId: string) =>
    run(cmd<Message>("add_note_to_chat", { noteId })),

  // Notes & artifacts
  listNotes: (notebookId: string) =>
    run(query<Note[]>("list_notes", { notebookId })),
  listRecentNotes: (limit = 6) =>
    run(query<Note[]>("list_recent_notes", { limit })),
  listRecentReports: (limit = 10) =>
    run(query<Note[]>("list_recent_reports", { limit })),
  corpusStats: () => run(query<CorpusStats>("corpus_stats")),
  newWindow: (notebookId?: string, noteId?: string) =>
    run(cmd<void>("new_window", { notebookId, noteId })),
  exportNotebookOkf: (notebookId: string, destDir: string) =>
    run(cmd<string>("export_notebook_okf", { notebookId, destDir })),
  rebuildAppMenu: () => run(cmd<void>("rebuild_app_menu")),
  fixTrafficLights: () => run(cmd<void>("fix_traffic_lights")),
  getAudioPath: (noteId: string) =>
    run(query<string | null>("get_audio_path", { noteId })),
  exportAudio: (noteId: string, dest: string) =>
    run(cmd<void>("export_audio", { noteId, dest })),
  kokoroStatus: () => run(query<KokoroStatus>("kokoro_status")),
  setupKokoro: () => run(slow<KokoroStatus>("setup_kokoro")),
  removeKokoro: () => run(cmd<KokoroStatus>("remove_kokoro")),
  exportNotebookOkfZip: (notebookId: string, destPath: string) =>
    run(slow<string>("export_notebook_okf_zip", { notebookId, destPath })),
  importNotebookOkf: (path: string, notebookId?: string | null) =>
    run(
      slow<Notebook>("import_notebook_okf", {
        path,
        notebookId: notebookId ?? null,
      }),
    ),
  searchEverything: (q: string) =>
    run(query<SearchHit[]>("search_everything", { query: q })),
  askEverything: (
    question: string,
    history: { role: string; content: string }[],
  ) => run(ai<MetaAnswer>("ask_everything", { question, history })),
  createNote: (notebookId: string, title: string, content: string) =>
    run(cmd<Note>("create_note", { notebookId, title, content })),
  updateNote: (id: string, title: string, content: string) =>
    run(cmd<void>("update_note", { id, title, content })),
  deleteNote: (id: string) => run(cmd<void>("delete_note", { id })),
  convertNoteToSource: (noteId: string) =>
    run(ai<Source>("convert_note_to_source", { noteId })),
  generateArtifact: (
    notebookId: string,
    kind: NoteKind,
    prompt?: string,
    sourceIds?: string[] | null,
  ) =>
    run(
      slow<Note>("generate_artifact", {
        notebookId,
        kind,
        prompt: prompt ?? "",
        sourceIds,
      }),
    ),
  rebuildNote: (
    noteId: string,
    notebookId: string,
    kind: NoteKind,
    prompt: string,
  ) => run(slow<Note>("rebuild_note", { noteId, notebookId, kind, prompt })),

  // Templates (custom generators in ~/Documents/Alchemy/templates)
  listTemplates: () => run(query<Template[]>("list_templates")),
  openTemplatesFolder: () => run(cmd<void>("open_templates_folder")),
  installDefaultTemplates: () => run(cmd<number>("install_default_templates")),

  // Reports
  listReportSchedules: (notebookId: string) =>
    run(query<ReportSchedule[]>("list_report_schedules", { notebookId })),
  listAllReportSchedules: () =>
    run(query<ReportSchedule[]>("list_all_report_schedules")),
  createReportSchedule: (
    notebookId: string,
    name: string,
    kind: string,
    prompt: string,
    intervalSecs: number,
  ) =>
    run(
      cmd<ReportSchedule>("create_report_schedule", {
        notebookId,
        name,
        kind,
        prompt,
        intervalSecs,
      }),
    ),
  updateReportSchedule: (
    id: string,
    name: string,
    kind: string,
    prompt: string,
    intervalSecs: number,
    enabled: boolean,
  ) =>
    run(
      cmd<void>("update_report_schedule", {
        id,
        name,
        kind,
        prompt,
        intervalSecs,
        enabled,
      }),
    ),
  deleteReportSchedule: (id: string) =>
    run(cmd<void>("delete_report_schedule", { id })),
  runReport: (scheduleId: string) =>
    run(ai<Note>("run_report", { scheduleId })),

  // Settings / health
  getAiConfig: () => run(query<AiConfig>("get_ai_config")),
  setAiConfig: (config: AiConfig) =>
    run(cmd<void>("set_ai_config", { config })),
  listModels: () => run(query<string[]>("list_models")),
  listGatewayModels: (baseUrl: string, apiKey: string) =>
    run(probe<string[]>("list_gateway_models", { baseUrl, apiKey })),
  checkOllama: () => run(query<boolean>("check_ollama")),
  checkModels: () => run(query<ModelHealth>("check_models")),
  getModelStats: () => run(query<ModelStat[]>("get_model_stats")),

  // Agent access (MCP)
  mcpStatus: () => run(query<McpStatus>("mcp_status")),
  listAgentConnectors: () =>
    run(query<ConnectorStatus[]>("list_agent_connectors")),
  connectAgent: (id: string) =>
    run(cmd<ConnectorStatus>("connect_agent", { id })),
};
