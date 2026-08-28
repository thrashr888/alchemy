import { invoke } from "@tauri-apps/api/core";
import { Cause, Duration, Effect, Schedule } from "effect";
import { describe, IpcError, TimeoutError, type AppError } from "./errors";
import { report } from "./diagnostics";
import type {
  ProviderModels,
  AcpAgentInfo,
  ActivityStats,
  Citation,
  AiConfig,
  BuildInfo,
  ChatConfig,
  CloudFolder,
  ConnectorStatus,
  FolderScan,
  GrepHit,
  HomeActivity,
  HygieneIssue,
  KokoroStatus,
  MacCollection,
  MacFileHit,
  McpStatus,
  Message,
  MessagePage,
  MetaAnswer,
  MetaCitation,
  MetaThread,
  MetaTurn,
  ModelHealth,
  ModelStat,
  CardFact,
  LedgerAnchor,
  LedgerEntry,
  RegistryCard,
  NightShiftStatus,
  Note,
  NoteKind,
  Notebook,
  NotebookGraph,
  NotebookSuggestion,
  ReportSchedule,
  SearchHit,
  Source,
  SnapshotStatus,
  SourceEvent,
  SuggestOutcome,
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

/** Run an Effect to a Promise, rejecting with a clean, user-friendly Error.
 *
 *  Every backend failure the app ever surfaces passes through here, after
 *  retries and timeouts have had their say — which makes it the one place
 *  worth logging from (docs/RFC-diagnostics.md). Without it, a command that
 *  fails becomes a toast the user reads once and we never see. */
async function run<A>(effect: Effect.Effect<A, AppError>): Promise<A> {
  const exit = await Effect.runPromiseExit(effect);
  if (exit._tag === "Success") return exit.value;
  const failure = Cause.squash(exit.cause);
  const message = describe(failure);
  const error = failure as Partial<AppError>;
  report("error", "ipc", message, undefined, {
    command: error?.command ?? "unknown",
    failure: error?._tag ?? "Unknown",
  });
  throw new Error(message);
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
  setNotebookIcon: (id: string, icon: string) =>
    run(cmd<void>("set_notebook_icon", { id, icon })),
  deleteNotebook: (id: string) => run(cmd<void>("delete_notebook", { id })),
  setNotebookStatus: (id: string, status: "" | "archived") =>
    run(cmd<void>("set_notebook_status", { id, status })),

  // Sources
  listSources: (notebookId: string) =>
    run(query<Source[]>("list_sources", { notebookId })),
  /** Base64 PNG for gallery cards (PDF first page / image file); "" = none. */
  sourceThumbnail: (sourceId: string) =>
    run(query<string>("source_thumbnail", { sourceId })),
  /** Batched opening-lines snippets for gallery cards. */
  sourceSnippets: (sourceIds: string[], maxChars?: number) =>
    run(
      query<Record<string, string>>("source_snippets", {
        sourceIds,
        maxChars,
      }),
    ),
  /** Stamp lead images onto pre-gallery URL sources; returns how many gained one. */
  backfillSourceImages: (notebookId: string) =>
    run(slow<number>("backfill_source_images", { notebookId })),
  addSourceFile: (notebookId: string, path: string) =>
    run(ai<Source>("add_source_file", { notebookId, path })),
  addSourceFolder: (notebookId: string, path: string) =>
    run(slow<Source>("add_source_folder", { notebookId, path })),
  /** Cloud-storage sync roots detected on this machine, for folder quick-picks. */
  listCloudFolders: () => run(query<CloudFolder[]>("list_cloud_folders")),
  /** Live Spotlight search over local files (probe: one attempt, no retry —
   *  the caller debounces and cancels stale queries itself). */
  searchMacFiles: (q: string, limit?: number) =>
    run(
      probe<MacFileHit[]>("search_mac_files", { query: q, limit: limit ?? null }),
    ),
  /** No id = whole corpus (the scheduler's tick); an id scopes the sweep. */
  resyncSources: (notebookId?: string) =>
    run(slow<FolderScan>("resync_sources", { notebookId })),
  providerReadiness: () =>
    run(
      ai<{ id: string; ready: boolean; detail: string }[]>(
        "provider_readiness",
        {},
      ),
    ),
  /** One provider's readiness, by id — Settings → Models probes each row
   *  independently so a slow/unreachable provider can't stall the others.
   *  Uses the fast 15s probe budget: a wedged backend flips the row to an
   *  error state promptly instead of spinning behind the 10-minute ceiling. */
  providerReadinessOne: (providerId: string) =>
    run(
      probe<{ id: string; ready: boolean; detail: string }>(
        "provider_readiness_one",
        { providerId },
      ),
    ),
  agentCliStatus: () =>
    run(ai<{ id: string; installed: boolean; detail: string }[]>("agent_cli_status", {})),
  addSourceUrl: (notebookId: string, url: string, include?: string) =>
    run(ai<Source>("add_source_url", { notebookId, url, include })),
  setChildEmbedded: (sourceId: string, embed: boolean) =>
    run(ai<Source>("set_child_embedded", { sourceId, embed })),
  addSourceText: (notebookId: string, title: string, text: string) =>
    run(ai<Source>("add_source_text", { notebookId, title, text })),
  updateSourceText: (sourceId: string, title: string, text: string) =>
    run(ai<Source>("update_source_text", { sourceId, title, text })),
  /** Set a source's tags (free text; the backend normalizes: strips "#",
   *  lowercases, dedupes). Returns the updated source, content stripped. */
  setSourceTags: (sourceId: string, tags: string) =>
    run(cmd<Source>("set_source_tags", { sourceId, tags })),
  /** Set the user annotation on a source (empty clears). Re-embeds the
   *  annotation for retrieval, hence the ai budget. */
  setSourceNote: (sourceId: string, note: string) =>
    run(ai<Source>("set_source_note", { sourceId, note })),
  refreshSourceUrl: (sourceId: string) =>
    run(ai<Source>("refresh_source_url", { sourceId })),
  /** Batch refresh (RFC-multi-select): returns immediately; the backend
   *  refreshes sequentially and emits one sources://changed at the end. */
  refreshSources: (notebookId: string, sourceIds: string[]) =>
    run(ai<void>("refresh_sources", { notebookId, sourceIds })),
  /** Batch delete (RFC-multi-select): one bulk operation; selected
   *  folder-like parents take their children along. */
  deleteSources: (notebookId: string, sourceIds: string[]) =>
    run(cmd<void>("delete_sources", { notebookId, sourceIds })),
  /** One tag string applied to a whole selection (RFC-multi-select). */
  setSourcesTags: (sourceIds: string[], tags: string) =>
    run(cmd<void>("set_sources_tags", { sourceIds, tags })),
  /** Hygiene classification for a notebook (RFC-source-hygiene). */
  sourceHygiene: (notebookId: string) =>
    run(query<HygieneIssue[]>("source_hygiene", { notebookId })),
  /** "Keep" an unreachable source: clear its strikes, restart the cadence. */
  hygieneKeep: (sourceId: string) =>
    run(cmd<void>("hygiene_keep", { sourceId })),
  getSourceContent: (sourceId: string) =>
    run(query<string>("get_source_content", { sourceId })),
  setWindowGlass: (enabled: boolean, dark: boolean, pinned: boolean) =>
    run(query<void>("set_window_glass", { enabled, dark, pinned })),
  liveViewOpen: (url: string, r: { x: number; y: number; w: number; h: number }) =>
    run(query<void>("live_view_open", { url, ...r })),
  liveViewBounds: (r: { x: number; y: number; w: number; h: number }) =>
    run(query<void>("live_view_bounds", r)),
  liveViewVisible: (visible: boolean) =>
    run(query<void>("live_view_visible", { visible })),
  liveViewClose: () => run(query<void>("live_view_close")),
  relatedPassages: (notebookId: string, text: string, limit?: number) =>
    run(query<Citation[]>("related_passages", { notebookId, text, limit })),
  sourceBacklinks: (sourceId: string) =>
    run(query<{ kind: "source" | "note"; id: string; title: string }[]>(
      "source_backlinks",
      { sourceId },
    )),
  /** The whole notebook as a link graph — one pass, unlike per-source
   *  backlinks. Backs the gallery's graph view. */
  notebookGraph: (notebookId: string) =>
    run(query<NotebookGraph>("notebook_graph", { notebookId })),
  reembedAll: () => run(ai<number>("reembed_all")),
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
  openPrivacySettings: () => run(cmd<void>("open_privacy_settings")),
  macNoteBody: (sourceId: string) =>
    run(query<string>("mac_note_body", { sourceId })),
  updateMacNote: (sourceId: string, body: string) =>
    run(ai<Source>("update_mac_note", { sourceId, body })),
  /** Check off a reminder by its id — titles repeat, ids don't. */
  completeMacReminder: (sourceId: string, reminderId: string) =>
    run(ai<Source>("complete_mac_reminder", { sourceId, reminderId })),
  addMacReminder: (sourceId: string, title: string, notes?: string) =>
    run(
      ai<Source>("add_mac_reminder", { sourceId, title, notes: notes ?? null }),
    ),

  // OS integrations (deep links, tray, Services, Spotlight)
  integrationsReady: () => run(cmd<string[]>("integrations_ready")),
  locateNote: (noteId: string) =>
    run(query<string | null>("locate_note", { noteId })),

  // Chat
  listMessagesPage: (
    notebookId: string,
    before?: { createdAt: number; id: string },
    limit = 80,
  ) =>
    run(
      query<MessagePage>("list_messages_page", {
        notebookId,
        beforeAt: before?.createdAt ?? null,
        beforeId: before?.id ?? null,
        limit,
      }),
    ),
  sendMessage: (
    notebookId: string,
    content: string,
    config: ChatConfig,
    sourceIds?: string[] | null,
    providerOverride?: string | null,
  ) =>
    run(
      ai<Message>("send_message", {
        notebookId,
        content,
        config,
        sourceIds,
        providerOverride: providerOverride ?? null,
      }),
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
  openInTerminal: (command: string) =>
    run(cmd<void>("open_in_terminal", { command })),
  /** Start Ollama detached from Alchemy: the Mac app when it's installed,
   *  else a backgrounded `ollama serve`. Resolves to "app" or "cli". */
  startOllama: () => run(cmd<string>("start_ollama")),
  /** Apply one settings-tool change from an error-row fix button
   *  (RFC-self-resolve): same allowlist as the chat `settings` tool; the
   *  returned Message is the tool row echoed into the transcript. */
  applySettingsFix: (notebookId: string, field: string, value: string) =>
    run(cmd<Message>("apply_settings_fix", { notebookId, field, value })),
  /** Confirmed connect from the transcript's confirm-click — the only
   *  chat-side path that writes an agent client's config; the returned
   *  Message is the tool row naming the file touched. */
  applyConnectFix: (notebookId: string, clientId: string) =>
    run(ai<Message>("apply_connect_fix", { notebookId, clientId })),
  /** Which notebook an incoming source belongs in. A bare `url` is fetched
   *  and extracted first, so this can take a second. */
  suggestNotebook: (input: { title?: string; text?: string; url?: string }) =>
    run(
      cmd<NotebookSuggestion>("suggest_notebook", {
        title: input.title ?? "",
        text: input.text ?? "",
        url: input.url ?? "",
      }),
    ),
  /** Page count of a PDF on disk; 0 when it can't be read. */
  pdfPageCount: (path: string) => run(cmd<number>("pdf_page_count", { path })),
  /** One rendered PDF page (1-indexed) as a `data:` PNG URL. */
  pdfPageImage: (path: string, page: number, width: number) =>
    run(cmd<string>("pdf_page_image", { path, page, width })),
  /** Local path for a PDF's bytes — downloads and caches URL-backed PDFs the
   *  first time, so page view works for them too. */
  pdfLocalPath: (sourceId: string) =>
    run(cmd<string>("pdf_local_path", { sourceId })),
  /** Validate a Notion integration token; resolves to the workspace label. */
  notionCheck: (token: string) => run(cmd<string>("notion_check", { token })),
  deleteMessage: (messageId: string) =>
    run(cmd<void>("delete_message", { messageId })),
  cancelGeneration: (scope?: "chat" | "artifact" | "tts" | "meta") =>
    run(cmd<void>("cancel_generation", { scope })),
  suggestFollowups: (notebookId: string) =>
    run(query<string[]>("suggest_followups", { notebookId })),
  generateNotebookSummary: (notebookId: string) =>
    run(ai<string>("generate_notebook_summary", { notebookId })),
  generateEpigraph: (mood: string) =>
    run(ai<string>("generate_epigraph", { mood })),
  clearChat: (notebookId: string) =>
    run(cmd<void>("clear_chat", { notebookId })),
  addNoteToChat: (noteId: string) =>
    run(cmd<Message>("add_note_to_chat", { noteId })),

  // Notes & artifacts
  listNotes: (notebookId: string) =>
    run(query<Note[]>("list_notes", { notebookId })),
  activityStats: () => run(query<ActivityStats>("activity_stats")),
  homeActivity: () => run(query<HomeActivity>("home_activity")),
  newWindow: (notebookId?: string, noteId?: string) =>
    run(cmd<void>("new_window", { notebookId, noteId })),
  rebuildAppMenu: () => run(cmd<void>("rebuild_app_menu")),
  fixTrafficLights: () => run(cmd<void>("fix_traffic_lights")),
  getAudioPath: (noteId: string) =>
    run(query<string | null>("get_audio_path", { noteId })),
  exportAudio: (noteId: string, dest: string) =>
    run(cmd<void>("export_audio", { noteId, dest })),
  exportNote: (noteId: string, format: string, dest: string) =>
    run(slow<string>("export_note", { noteId, format, dest })),
  kokoroStatus: () => run(query<KokoroStatus>("kokoro_status")),
  setupKokoro: () => run(slow<KokoroStatus>("setup_kokoro")),
  removeKokoro: () => run(cmd<KokoroStatus>("remove_kokoro")),
  /** Push the frontend-owned menu lists (themes, studio generators) into
   *  the native menu; re-called on theme change to move the selection dot. */
  fillMenuLists: (
    themes: [string, string][],
    generators: [string, string][],
    currentTheme: string,
  ) =>
    run(cmd<void>("fill_menu_lists", { themes, generators, currentTheme })),
  /** Settings → Shortcuts rows from the menu's command registry. */
  listShortcuts: () =>
    run(
      query<{ keys: string; label: string; context: string }[]>(
        "list_shortcuts",
        {},
      ),
    ),
  exportNotebookOkfZip: (notebookId: string, destPath: string) =>
    run(slow<string>("export_notebook_okf_zip", { notebookId, destPath })),
  probeOkf: (path: string) => run(query<boolean>("probe_okf", { path })),
  importNotebookOkf: (path: string, notebookId?: string | null) =>
    run(
      slow<Notebook>("import_notebook_okf", {
        path,
        notebookId: notebookId ?? null,
      }),
    ),
  searchEverything: (q: string) =>
    run(query<SearchHit[]>("search_everything", { query: q })),
  /** `/grep` in the composer: in-process ripgrep over the notebook's repo- and
   *  folder-backed files. No model call; hits render as a local chat message. */
  grepSources: (notebookId: string, pattern: string, maxResults?: number) =>
    run(
      cmd<GrepHit[]>("grep_sources", {
        notebookId,
        pattern,
        maxResults: maxResults ?? null,
      }),
    ),
  askEverything: (
    question: string,
    history: { role: string; content: string }[],
    /** Deep search: 3× retrieval pool + model rerank. Omit for the smart
     *  default (on for gateway models, off for local). */
    deep?: boolean,
    /** Home chat's own style and length. Omitted (⌘K's glance mode) means the
     *  default prompt, unchanged. */
    config?: ChatConfig | null,
  ) =>
    run(
      ai<MetaAnswer>("ask_everything", {
        question,
        history,
        deep: deep ?? null,
        config: config ?? null,
      }),
    ),
  // Home chat threads — the persisted side of ask_everything.
  listMetaThreads: () => run(query<MetaThread[]>("list_meta_threads")),
  listMetaTurns: (threadId: string) =>
    run(query<MetaTurn[]>("list_meta_turns", { threadId })),
  addMetaTurn: (
    threadId: string,
    role: "user" | "assistant",
    content: string,
    citations: MetaCitation[],
    kind: MetaTurn["kind"],
  ) =>
    run(
      cmd<MetaTurn>("add_meta_turn", {
        threadId,
        role,
        content,
        citations,
        kind,
      }),
    ),
  deleteMetaThread: (threadId: string) =>
    run(cmd<void>("delete_meta_thread", { threadId })),
  createNote: (notebookId: string, title: string, content: string) =>
    run(cmd<Note>("create_note", { notebookId, title, content })),
  /** Undo half of the note-delete toast: re-insert with kind/prompt intact. */
  restoreNote: (n: Note) =>
    run(
      cmd<Note>("restore_note", {
        note: {
          notebookId: n.notebookId,
          title: n.title,
          content: n.content,
          kind: n.kind,
          prompt: n.prompt,
          origin: n.origin,
          status: n.status,
        },
      }),
    ),
  updateNote: (id: string, title: string, content: string) =>
    run(cmd<void>("update_note", { id, title, content })),
  deleteNotes: (ids: string[]) => run(cmd<void>("delete_notes", { ids })),
  /** Fire-and-forget read counter for the note curator (RFC-note-curator). */
  noteOpened: (id: string) => run(cmd<void>("note_opened", { id })),
  /** Version, commit, and dev/release profile for Settings → About. */
  buildInfo: () => run(cmd<BuildInfo>("build_info", {})),
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
  saveTemplate: (id: string | null, name: string, description: string, prompt: string) =>
    run(cmd<Template>("save_template", { id, name, description, prompt })),
  deleteTemplate: (id: string) => run(cmd<void>("delete_template", { id })),

  // The Ledger
  listLedger: (notebookId: string) =>
    run(query<LedgerEntry[]>("list_ledger", { notebookId })),
  addLedgerEntry: (
    notebookId: string,
    kind: string,
    text: string,
    why?: string,
    anchors?: LedgerAnchor[],
  ) =>
    run(
      cmd<LedgerEntry>("add_ledger_entry", {
        notebookId,
        kind,
        text,
        why,
        anchors,
      }),
    ),
  updateLedgerEntry: (
    id: string,
    patch: { text?: string; why?: string; status?: string },
  ) => run(cmd<LedgerEntry>("update_ledger_entry", { id, ...patch })),
  deleteLedgerEntry: (id: string) =>
    run(cmd<void>("delete_ledger_entry", { id })),

  // The Registry — corpus-scoped, so no notebookId anywhere here.
  listRegistry: () => run(query<RegistryCard[]>("list_registry", {})),
  addRegistryCard: (
    kind: string,
    name: string,
    identifiers?: string,
    note?: string,
    facts?: CardFact[],
  ) =>
    run(
      cmd<RegistryCard>("add_registry_card", {
        kind,
        name,
        identifiers,
        note,
        facts,
      }),
    ),
  updateRegistryCard: (
    id: string,
    patch: {
      name?: string;
      identifiers?: string;
      note?: string;
      facts?: CardFact[];
    },
  ) => run(cmd<RegistryCard>("update_registry_card", { id, ...patch })),
  deleteRegistryCard: (id: string) =>
    run(cmd<void>("delete_registry_card", { id })),
  attachSourceToCard: (cardId: string, sourceId: string, status?: string) =>
    run(
      cmd<RegistryCard>("attach_source_to_card", { cardId, sourceId, status }),
    ),
  /** status: "confirmed" | "rejected" | "remove". */
  setAttachmentStatus: (cardId: string, sourceId: string, status: string) =>
    run(
      cmd<RegistryCard>("set_attachment_status", { cardId, sourceId, status }),
    ),
  /** origin: "" confirms a suggestion, "dismissed" turns it down. */
  setCardOrigin: (id: string, origin: string) =>
    run(cmd<RegistryCard>("set_card_origin", { id, origin })),
  /** Rule suggestions in bulk; same origin contract as setCardOrigin.
   *  `onlyRecommended` limits the verdict to the triage pass's picks. */
  ruleAllSuggested: (origin: string, onlyRecommended?: boolean) =>
    run(cmd<number>("rule_all_suggested", { origin, onlyRecommended })),
  /** Run the card suggester now. Omit notebookId to read every notebook —
   *  the Registry's own button is corpus-scoped. Triage follows in the
   *  background and lands on a registry bump. */
  suggestCardsNow: (notebookId?: string) =>
    run(cmd<SuggestOutcome>("suggest_cards_now", { notebookId })),
  cardsForSource: (sourceId: string) =>
    run(query<RegistryCard[]>("cards_for_source", { sourceId })),
  rematchRegistry: (notebookId: string) =>
    run(cmd<number>("rematch_registry", { notebookId })),

  // Night Shift (the Home Staff section)
  listSourceEvents: (hours = 24) =>
    run(query<SourceEvent[]>("list_source_events", { hours })),
  nightShiftStatus: () => run(query<NightShiftStatus>("night_shift_status")),
  snapshotStatus: () => run(query<SnapshotStatus>("snapshot_status")),
  snapshotNow: () => run(cmd<SnapshotStatus>("snapshot_now")),
  restoreSnapshot: () => run(cmd<string>("restore_snapshot")),
  toggleNightShiftPause: () => run(cmd<boolean>("toggle_night_shift_pause")),

  // Reports
  listReportSchedules: (notebookId: string) =>
    run(query<ReportSchedule[]>("list_report_schedules", { notebookId })),
  createReportSchedule: (
    notebookId: string,
    name: string,
    kind: string,
    prompt: string,
    trigger: string,
    intervalSecs: number,
  ) =>
    run(
      cmd<ReportSchedule>("create_report_schedule", {
        notebookId,
        name,
        kind,
        prompt,
        trigger,
        intervalSecs,
      }),
    ),
  updateReportSchedule: (
    id: string,
    name: string,
    kind: string,
    prompt: string,
    trigger: string,
    intervalSecs: number,
    enabled: boolean,
  ) =>
    run(
      cmd<void>("update_report_schedule", {
        id,
        name,
        kind,
        prompt,
        trigger,
        intervalSecs,
        enabled,
      }),
    ),
  deleteReportSchedule: (id: string) =>
    run(cmd<void>("delete_report_schedule", { id })),
  runReport: (scheduleId: string) =>
    run(ai<Note>("run_report", { scheduleId })),
  runSecondLook: (noteId: string) =>
    run(cmd<void>("run_second_look", { noteId })),

  // Settings / health
  getAiConfig: () => run(query<AiConfig>("get_ai_config")),
  setAiConfig: (config: AiConfig) =>
    run(cmd<void>("set_ai_config", { config })),
  /** Desktop notification; the backend applies the gates ("Show
   *  notifications" + quiet-while-focused) so every path shares them. */
  sendNotification: (title: string, body: string) =>
    run(cmd<void>("send_notification", { title, body })),
  listModels: () => run(query<string[]>("list_models")),
  listGatewayModels: (baseUrl: string, apiKey: string) =>
    run(probe<string[]>("list_gateway_models", { baseUrl, apiKey })),
  /** One provider's model choices for the composer picker. Never throws for a
   *  provider that simply has no catalogue — the list comes back empty. */
  providerModels: (providerId: string) =>
    run(probe<ProviderModels>("provider_models", { providerId })),
  checkOllama: () => run(query<boolean>("check_ollama")),
  checkModels: () => run(query<ModelHealth>("check_models")),
  getModelStats: () => run(query<ModelStat[]>("get_model_stats")),

  // Hosted agents (ACP)
  acpAgents: () => run(query<AcpAgentInfo[]>("acp_agents")),
  /** Opens a throwaway session to prove the agent is signed in and working.
   *  Spawns a subprocess like acpStart, so it takes the long AI timeout. */
  acpCheck: (agentId: string) => run(ai<void>("acp_check", { agentId })),
  /** The running session's agent id for a notebook, or null. */
  acpStatus: (notebookId: string) =>
    run(query<string | null>("acp_status", { notebookId })),
  /** Spawns the agent subprocess — npx-backed adapters can take a while on
   *  first run, so this gets the long AI timeout, not the 30s command one. */
  acpStart: (notebookId: string, agentId: string, resume?: string | null) =>
    run(ai<void>("acp_start", { notebookId, agentId, resume: resume ?? null })),
  acpPrompt: (notebookId: string, text: string) =>
    run(cmd<void>("acp_prompt", { notebookId, text })),
  acpCancel: (notebookId: string) =>
    run(cmd<void>("acp_cancel", { notebookId })),
  acpStop: (notebookId: string) => run(cmd<void>("acp_stop", { notebookId })),
  acpPermission: (
    notebookId: string,
    requestId: string,
    optionId: string | null,
  ) => run(cmd<void>("acp_permission", { notebookId, requestId, optionId })),

  // Agent access (MCP)
  mcpStatus: () => run(query<McpStatus>("mcp_status")),
  listAgentConnectors: () =>
    run(query<ConnectorStatus[]>("list_agent_connectors")),
  connectAgent: (id: string) =>
    run(cmd<ConnectorStatus>("connect_agent", { id })),
};
