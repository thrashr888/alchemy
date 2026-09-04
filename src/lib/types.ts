// Mirrors the serde models in src-tauri/src/models.rs (camelCase).

export type ToastKind = "success" | "error" | "info";
export interface Toast {
  id: string;
  kind: ToastKind;
  message: string;
  /** Present → the toast body is clickable (e.g. "update available" opens
   *  Settings). Clicking dismisses the toast too. */
  onClick?: () => void;
}

export interface Notebook {
  id: string;
  title: string;
  /** Per-notebook web-search opt-in (Grow's Firecrawl tier). */
  growthWeb?: boolean;
  createdAt: number;
  updatedAt: number;
  color: string;
  /** Lucide icon slug, "" → default book. See lib/notebookIcons.ts. */
  icon: string;
  /** "" (active) | "archived" | "system". Archived notebooks leave the main
   *  grid; system notebooks (Briefs) never appear on the shelf at all but
   *  work like any other notebook when opened. */
  status: "" | "archived" | "system";
  sourceCount: number;
  /** Deliberate notes, excluding reports. */
  noteCount: number;
  /** Report-kind notes (scheduled runs, briefs). */
  reportCount: number;
}

/** One document in the notebook link graph. */
export interface GraphNode {
  id: string;
  kind: "source" | "note";
  title: string;
  sourceType: string;
  /** Inbound + outbound edges, used to size the node. */
  degree: number;
}

/** The notebook as a link graph (RFC-document-surface phase 5). */
export interface NotebookGraph {
  nodes: GraphNode[];
  edges: { from: string; to: string }[];
}

/** Where an unfiled source should go — the auto-notebooking answer. */
export interface NotebookSuggestion {
  /** Empty when proposing a new notebook; `title` is then the proposal. */
  notebookId: string;
  title: string;
  isNew: boolean;
}

/** Where a notebook keeps itself on disk as an OKF bundle (RFC-okf-live §5.1).
 *  Machine-local: the path lives in a sidecar, not on the notebook row. */
export interface OkfBinding {
  path: string;
  /** Epoch ms of the last write; 0 until the seed pass lands. */
  lastWriteAt: number;
}

/** Whether the Notebooks folder can move into the app's own iCloud container,
 *  and what would move (RFC-okf-live §5.7, stage two). */
export interface IcloudMoveOffer {
  available: boolean;
  from: string;
  to: string;
  count: number;
  /** Bundles in the old folder no notebook here is bound to — the starters
   *  and their copies. They move too, so the banner can promise the whole
   *  folder rather than half of it. */
  others: number;
}

/** What one OKF concept says about its own standing (RFC-okf-live §4), read
 *  from the file's frontmatter at scan time and keyed by source id. */
export interface OkfLifecycle {
  /** "" (current) | "draft" | "deprecated" */
  status: string;
  /** Epoch ms after which the concept is stale; 0 when it names no expiry. */
  staleAfter: number;
  /** "" unverified | "machine" confirmed | "human" reviewed */
  trust: string;
}

export interface Source {
  id: string;
  notebookId: string;
  title: string;
  sourceType:
    | "pdf"
    | "text"
    | "markdown"
    | "html"
    | "url"
    | "image"
    | "folder"
    | "mac"
    | "code"
    | "git"
    | "notion"
    | "obsidian"
    | "okf"
    | "feed";
  url: string;
  content: string;
  /** "placeholder" = cloud-sync file not downloaded yet; listed, not embedded.
   *  "processing" = landed and readable; chunks/embeddings still indexing in
   *  the background (RFC-import-pipeline §2) — flips to "ready" via events. */
  status: "ready" | "error" | "placeholder" | "processing";
  error: string;
  charCount: number;
  chunkCount: number;
  createdAt: number;
  /** Id of the folder source this file belongs to; empty for top-level. */
  parentId: string;
  /** File mtime (unix millis) recorded at ingest for folder children. */
  mtime: number;
  /** Embedded document authorship (PDF /Author, Office dc:creator, EXIF
   *  Artist) captured at ingest; empty when the format carries none. */
  author: string;
  /** Gallery lead image (og:image) for url sources.
   *  "" = unknown, "-" = checked and the page has none. */
  imageUrl: string;
  /** User tags: space-separated normalized tokens (lowercase, no "#").
   *  Fold into routing and the chat manifest (RFC-source-tags). */
  tags: string;
  /** The user's one annotation on this source ("why I saved this");
   *  indexed for retrieval as their own judgment. */
  note: string;
  /** When the content was last successfully ingested from its origin
   *  (unix millis) — hygiene's freshness signal (RFC-source-hygiene). */
  fetchedAt: number;
  /** Consecutive background refresh failures; reset on success. At 3 the
   *  source is flagged "unreachable" instead of being retried forever. */
  fetchFailures: number;
}

/** One flagged source or note from the hygiene check (RFC-source-hygiene).
 *  "unreachable" | "missing-file" | "duplicate" | "husk" | "empty-note" are
 *  proposed removals (never automatic); "stale" is informational — the
 *  background sweep re-fetches those itself. */
export interface HygieneIssue {
  /** Which table `sourceId` points into. Notes have nothing to re-fetch, so
   *  the review offers them Keep and Remove only. */
  kind: "source" | "note";
  /** The flagged object's id — a note id when `kind` is "note". */
  sourceId: string;
  title: string;
  bucket:
    | "unreachable"
    | "missing-file"
    | "duplicate"
    | "husk"
    | "empty-note"
    | "stale";
  detail: string;
  /** For "duplicate": the id of the copy being kept — the oldest of the
   *  group. "" for every other bucket. */
  keeperId: string;
}

/** One model call in flight (inference::activity). The title-bar indicator
 *  reads these; agents get the same list from the `settings` MCP tool with
 *  op "activity". */
export interface ActivityItem {
  id: number;
  /** ollama | fm | gateway | agent-cli | builtin. */
  kind: string;
  /** What it is for, in the user's words. "" when the caller set no scope. */
  label: string;
  model: string;
  startedAt: number;
}

/** One pickable Mac-provider item (a calendar range, reminders list, note…). */
export interface MacCollection {
  id: string;
  label: string;
  detail: string;
}

/** A detected cloud-storage sync root the "Add folder" picker can open into. */
export interface CloudFolder {
  /** Stable machine key: google_drive | onedrive | box | dropbox | icloud. */
  provider: string;
  /** Display name, e.g. "Google Drive". */
  label: string;
  /** Account or tenant this root belongs to ("Personal", "Contoso Ltd",
   *  "me@gmail.com"); empty when the root says nothing past the provider. */
  name: string;
  /** Absolute path to the sync root on disk. */
  path: string;
}

/** One Spotlight hit from the Add Source → "Search your Mac" step (mirrors
 *  `FileHit` in src-tauri/src/filesearch.rs). Only `ingestible` rows (and
 *  every folder) can be added. */
export interface MacFileHit {
  name: string;
  path: string;
  /** Lowercased extension without the dot ("" for folders / extensionless). */
  ext: string;
  /** Human kind for the row chip: "Folder", "PDF", "Code", … */
  kind: string;
  isDir: boolean;
  size: number;
  /** mtime in unix millis (0 when unknown). */
  mtime: number;
  ingestible: boolean;
}

/** Tally of what a folder rescan changed. */
export interface FolderScan {
  added: number;
  updated: number;
  removed: number;
  failed: number;
}

export interface Citation {
  chunkId: string;
  /** Empty when the passage came from a note (see noteId). */
  sourceId: string;
  /** Title of the source — or of the note for note passages. */
  sourceTitle: string;
  /** On-disk path of the source's original file; empty for web/mac sources,
   *  note passages, and citations persisted before this field existed. */
  sourcePath?: string;
  /** Non-empty when the passage came from a note: the note's id. */
  noteId: string;
  /**
   * True for source-gist rows (distilled overview evidence, RFC-infinite-
   * context §1). Optional: citations persisted before gists existed lack it.
   */
  gist?: boolean;
  /** "title › chapter › section" for the passage; absent on older rows. */
  section?: string;
  /**
   * True for the user's own per-source annotation (RFC-source-tags);
   * sourceId names the annotated source. Optional: older citations lack it.
   */
  snote?: boolean;
  ordinal: number;
  snippet: string;
  distance: number;
}

export interface Message {
  id: string;
  notebookId: string;
  role: "user" | "assistant";
  content: string;
  citations: Citation[];
  /** "chat" for real answers, "tool" for tool confirmations. */
  kind: "chat" | "tool" | "error";
  /** Provider attribution caption ("Claude Code · $0.04"); empty for user
   *  turns and pre-existing rows. */
  model: string;
  createdAt: number;
}

export interface MessagePage {
  messages: Message[];
  hasMore: boolean;
}

export type NoteKind =
  | "note"
  | "summary"
  | "faq"
  | "study_guide"
  | "briefing"
  | "timeline"
  | "insights"
  | "flashcards"
  | "quiz"
  | "audio_overview"
  | "mind_map"
  | "uml"
  | "slide_deck"
  | "infographic"
  | "data_table"
  | "round_table"
  | "problems"
  | "evidence"
  | "prd"
  | "prfaq"
  | "rfc"
  | "skill"
  | "report"
  | "template";

export interface Note {
  id: string;
  notebookId: string;
  title: string;
  content: string;
  kind: NoteKind;
  prompt: string;
  /** "" for deliberate notes, "auto" for chat-created evidence records.
   *  Editing an auto note flips it to "" (user-owned). */
  origin: string;
  /** Curator state for auto notes: "" | "stale" (dimmed) | "archived"
   *  (out of retrieval, collapsed). Use or an edit revives. */
  status: string;
  createdAt: number;
  updatedAt: number;
}

/** Which build a window belongs to (Settings → About) — dev and the
 *  installed app share a data dir and look identical. */
export interface BuildInfo {
  version: string;
  commit: string;
  /** "dev" (tauri dev) | "release" (installed app). */
  profile: string;
}

/** One growth proposal (RFC-living-notebook Pillar 2): a URL the
 *  notebook's own sources keep pointing at, ranked against standing
 *  queries mined from thin retrievals. Nothing fetches until Add. */
/** The Grow surface payload: what the notebook is hungry for, plus the
 *  free tiers' proposals (local Spotlight + links mined from sources). */
export interface GrowthOverview {
  queries: string[];
  proposals: GrowthProposal[];
}

/** The open-web tier's result (Firecrawl keyless search, metered). */
export interface GrowthWebSearch {
  proposals: GrowthProposal[];
  creditsThisMonth: number;
  capped: boolean;
  /** Days between fresh searches at the current budget pace. */
  refreshEveryDays: number;
}

export interface GrowthProposal {
  /** "web" (link mined from sources) | "local" (on-disk path) |
   *  "search" (found on the open web via Firecrawl) | "feed" (a feed one
   *  of the notebook's pages advertised — docs/RFC-events.md §2). */
  kind: "web" | "local" | "search" | "feed";
  url: string;
  /** Best anchor text seen for the link, or the file's name. */
  anchor: string;
  mentions: number;
  sourceCount: number;
  matchedQuery: string;
  score: number;
}

/** One retirement candidate (RFC-living-notebook Pillar 3): old enough to
 *  have had its chance, never once cited. Proposal only. */
export interface RetireProposal {
  sourceId: string;
  title: string;
  ageDays: number;
  charCount: number;
}

/** One proposed tag merge (RFC-living-notebook phase 5): plural/singular
 *  or separator variants of the same word. Proposal only. */
export interface TagMergeProposal {
  from: string;
  to: string;
  fromCount: number;
  toCount: number;
}

/** One GitHub release, read live for Settings → About's What's new. */
export interface ReleaseNote {
  version: string;
  name: string;
  /** Release body, GitHub-flavored Markdown. */
  body: string;
  publishedAt: string;
  url: string;
}

/** A custom Studio generator: one ~/Documents/Alchemy/templates/*.md file. */
export interface Template {
  /** Filename stem, e.g. "swot-analysis". */
  id: string;
  name: string;
  description: string;
  /** Generation instruction (file body), run over the notebook's sources. */
  prompt: string;
}

/** Aggregate corpus totals for the home page. */
export interface CorpusStats {
  sources: number;
  chars: number;
  notes: number;
  ledger: number;
}

/** One local calendar day of activity (Settings → Activity; Rust activity.rs). */
export interface ActivityDay {
  /** Local date, "YYYY-MM-DD". */
  date: string;
  messages: number;
  sources: number;
  notes: number;
  retrievals: number;
}

/** A labeled count for the "most used" lists. */
export interface ActivityCount {
  label: string;
  count: number;
}

/** Everything Settings → Activity renders, aggregated read-time in Rust. */
export interface ActivityStats {
  /** Ascending by date; sparse — active days only. */
  days: ActivityDay[];
  totalMessages: number;
  totalUserMessages: number;
  totalSources: number;
  totalNotes: number;
  totalNotebooks: number;
  /** Retained trace history only (~months), not lifetime. */
  totalRetrievals: number;
  corpusChars: number;
  assistantWords: number;
  activeDays: number;
  currentStreak: number;
  longestStreak: number;
  /** Local hour 0–23; -1 with no messages. */
  peakHour: number;
  favoriteModel: string;
  models: ActivityCount[];
  notebooks: ActivityCount[];
  sourceTypes: ActivityCount[];
  firstActivityAt: number;
  /** What background runs cost over the receipts window (30 days), in
   *  millionths of a dollar. Background work only, and 0 when every run was
   *  local — local models are free, not unrecorded. */
  backgroundCostMicros: number;
  /** Measured output tokens across every recorded generation, lifetime. */
  tokensGenerated: number;
}

/** One exact-match window from the `/grep` chat command (Rust grep_sources). */
export interface GrepHit {
  sourceId: string;
  sourceTitle: string;
  /** Absolute file path of the repo-/folder-backed child source. */
  path: string;
  /** 1-based line where the window begins. */
  line: number;
  /** The matching line window, real lines joined with whitespace intact. */
  window: string;
}

/** One global-search result (command menu). */
export interface SearchHit {
  kind: "source" | "note" | "content" | "card" | "ledger";
  notebookId: string;
  /** Source id for source/content hits, note id for notes, card id for
   *  registry cards (which carry no notebookId — they're corpus-scoped),
   *  entry id for ledger rows. */
  id: string;
  title: string;
  snippet: string;
}

/** Podcast voice model (Kokoro) readiness. */
export interface KokoroStatus {
  downloaded: boolean;
  /** A test synthesis succeeded — the Audio Overview generator may show. */
  verified: boolean;
}

export interface ModelStatus {
  name: string;
  installed: boolean;
  working: boolean;
  detail: string;
}

export interface ModelHealth {
  reachable: boolean;
  chat: ModelStatus;
  embed: ModelStatus;
  vision: ModelStatus;
}

export interface ModelStat {
  name: string;
  lastTokensPerSec: number;
  avgTokensPerSec: number;
  samples: number;
  /** How many chats contributed a TTFT measurement (its own count —
   *  throughput and TTFT are recorded separately). */
  ttftSamples: number;
  /** Chat time-to-first-token, wall-clock ms (0 = never measured). */
  lastTtftMs: number;
  avgTtftMs: number;
}

export interface ReportSchedule {
  id: string;
  notebookId: string;
  name: string;
  kind: string;
  prompt: string;
  /** "interval" (clock-fired) or "change" (a standing question — runs when
   *  sources in the notebook change, intervalSecs as the throttle floor). */
  /** "once" is a commission: one job handed to the night, which
   *  retires itself after it runs (docs/RFC-night-shift-area.md §1). */
  trigger: "interval" | "change" | "once";
  /** Epoch ms before which a "once" commission may not start; 0 = next pass. */
  notBefore: number;
  intervalSecs: number;
  enabled: boolean;
  /** Change-trigger filters (docs/RFC-events.md §5): space-separated source
   *  ids and event kinds; empty = any. Ignored unless trigger is "change". */
  watchSources: string;
  watchKinds: string;
  lastRunAt: number;
  createdAt: number;
}

export interface HomeActivity {
  schedules: ReportSchedule[];
  recentNotes: Note[];
  reports: Note[];
  stats: CorpusStats;
}

/** One anchor pinning a ledger entry to verbatim source text. */
export interface LedgerAnchor {
  sourceId: string;
  quote: string;
}

/** One typed ledger row (the Steward's memory): kind-specific lifecycles —
 *  assertion asserted→corroborated|contradicted|stale, fact current→
 *  superseded, decision decided→superseded, question open→answered, log. */
export interface LedgerEntry {
  id: string;
  notebookId: string;
  kind: "assertion" | "fact" | "decision" | "question" | "log";
  text: string;
  why: string;
  status: string;
  /** "" for user/agent rows, "auto" for chat-minted rows. */
  origin: string;
  anchors: LedgerAnchor[];
  createdAt: number;
  updatedAt: number;
}

/** The Registry's kinds (RFC-registry). */
export type CardKind =
  "asset" | "person" | "policy" | "provider" | "project" | "dependency";

/** One key fact on a card — the reader's doc-properties grid shape. */
export interface CardFact {
  label: string;
  value: string;
}

/** What an explicit "suggest cards" ask produced. `reply` carries the
 *  model's raw answer so "it suggested nothing" can be told apart from
 *  "it said something that didn't survive the grounding gate". */
export interface SuggestOutcome {
  created: string[];
  reply: string;
  /** True when another suggest pass already held the single-flight guard
   *  and this ask did nothing. */
  alreadyRunning: boolean;
}

/** One document filed under a card. `matched` is the receipt and is never
 *  empty: the identifier string that matched, "name", or "manual". */
export interface CardAttachment {
  sourceId: string;
  notebookId: string;
  status: "confirmed" | "proposed" | "rejected";
  matched: string;
  at: number;
}

/** One registry card (RFC-registry): a confirmed cast member. Corpus-scoped
 *  — deliberately has no notebookId; its home is derived from where its
 *  documents live. Cards have no lifecycle; their attachments do. */
export interface RegistryCard {
  id: string;
  kind: CardKind;
  name: string;
  /** "" = yours (made or confirmed), "auto" = suggested and awaiting your
   *  verdict, "dismissed" = turned down and remembered so the suggester
   *  never proposes it again. */
  origin: "" | "auto" | "dismissed";
  /** Triage verdict on a still-suggested card: "" = not yet triaged,
   *  "recommended" = the triage pass thinks this one matters, "routine" =
   *  triaged and not singled out. Cleared once the card is ruled on. */
  triage: "" | "recommended" | "routine";
  /** Space-separated normalized tokens (VIN, policy number, serial) — the
   *  strongest auto-attach signal; strong full-name matches attach too,
   *  with a "name matched" receipt. */
  identifiers: string;
  note: string;
  facts: CardFact[];
  attachments: CardAttachment[];
  createdAt: number;
  updatedAt: number;
}

/** One observed source change (watchers, RFC-night-shift): written by the
 *  refresh paths, read by the Brief, the Staff section, and agents. */
/** A feed the app can offer to follow for a source (docs/RFC-events.md §2). */
export interface FeedCandidate {
  url: string;
  /** "Releases", "Page history", "Feed", … */
  label: string;
  /** "page" (advertised by the page) | "host" (the host's shape) |
   *  "well-known" (found at a conventional path). */
  tier: "page" | "host" | "well-known";
}

/** Every SourceEvent.kind a producer writes (docs/RFC-events.md §1). */
export const EVENT_KINDS = [
  "added",
  "updated",
  "removed",
  "unreachable",
  "completed",
  "moved",
] as const;
export type EventKind = (typeof EVENT_KINDS)[number];

export interface SourceEvent {
  id: string;
  notebookId: string;
  sourceId: string;
  sourceTitle: string;
  kind: EventKind | string;
  detail: string;
  /** Capped ±-prefixed line excerpt; empty when nothing textual changed. */
  diff: string;
  at: number;
}

/** Last-snapshot state for the Nightly settings page
 *  (docs/RFC-night-shift-area.md §7). */
export interface SnapshotStatus {
  /** Epoch ms; 0 when no snapshot has been taken. */
  takenAt: number;
  bytes: number;
  path: string;
  storeVersion: number;
}

export interface NightShiftStatus {
  backgroundEnabled: boolean;
  paused: boolean;
}

/** What a provider offers the model picker. `supportsDefault` is false for a
 *  gateway, which has no fallback model of its own — it needs a name. */
export interface ProviderModels {
  models: string[];
  supportsDefault: boolean;
  /** Reasoning-effort levels, cheapest first. Empty = this provider has no
   *  effort control, and the composer hides the pill entirely. */
  efforts: string[];
  /** What "Default" resolves to, when knowable — Ollama falls through to the
   *  app's main model. `null` for a vendor CLI, whose default is its own
   *  business and which won't tell us. */
  defaultModel: string | null;
}

/** One configured inference provider (list entry in Settings → Models). */
export interface ProviderEntry {
  id: string;
  kind: string;
  label: string;
  baseUrl: string;
  apiKey: string;
  chatModel: string;
  /** Reasoning effort; "" = the provider's own default. */
  effort: string;
}

export interface AiConfig {
  providers: ProviderEntry[];
  chatProvider: string;
  studioProvider: string;
  /** Chat backend: "ollama" | "openai" (any OpenAI-compatible gateway). */
  provider: string;
  /** Embedding backend: "ollama" | "builtin". */
  embedder: string;
  baseUrl: string;
  chatModel: string;
  /** Ollama model for the Small role (gists, tags, Weave verdicts, registry
   *  suggestions). Empty = Apple Foundation Models when available, else the
   *  chat provider. */
  smallModel: string;
  embedModel: string;
  visionModel: string;
  openaiBaseUrl: string;
  openaiApiKey: string;
  openaiChatModel: string;
  openaiVisionModel: string;
  /** Who the user is; woven into system prompts so answers fit them. */
  profile: UserProfile;
  /** Embedded MCP server for agent access (localhost streamable HTTP). */
  mcpEnabled: boolean;
  mcpPort: number;
  /** Hosted coding agent the Agent view opens with — an `acpAgents` id, or
   *  empty for "whichever is installed". */
  hostedAgent: string;
  /** Browser-extension clip receiver (localhost; accepts a rendered DOM from
   *  the user's logged-in tab, see docs/RFC-page-capture.md §8). */
  clipEnabled: boolean;
  clipPort: number;
  /** Menu bar extra (tray icon); Settings → General toggles it live. Also
   *  the residency switch: tray on = closing the window leaves Alchemy
   *  running in the menu bar (docs/RFC-night-shift.md). */
  trayEnabled: boolean;
  /** Night Shift master switch: scheduled reports + automatic source
   *  resyncs from the resident scheduler. Off = on-demand only. */
  backgroundEnabled: boolean;
  /** Where notebooks live on disk as OKF bundles (RFC-okf-live §5.7). */
  notebooksDir: string;
  /** Whether a new notebook is kept on disk from the moment it is made. */
  keepOnDisk: boolean;
  /** Whether the one-time "keep your notebooks on disk?" offer was answered. */
  keepOnDiskAsked: boolean;
  /** Whether the one-time "move them into the Alchemy iCloud folder?" offer
   *  was answered (RFC-okf-live §5.7, stage two). */
  icloudMoveAsked: boolean;

  /** Desktop notifications; in config (not localStorage) so the resident
   *  scheduler can honor it with no window open. */
  showNotifications: boolean;
  /** Quiet-while-focused rule: skip notifications and sound cues while an
   *  Alchemy window is focused. On by default; off = always deliver. */
  quietWhenFocused: boolean;
  /** Weekly LLM consolidation of auto-created evidence notes (note curator
   *  phase 5). On by default — idle-gated, capped, fully recoverable; the
   *  toggle is for cost control. */
  curatorConsolidate: boolean;
  visionProvider: string;
  setupSeen: boolean;
  gitSyncMinutes: number;
  notionToken: string;
  /** Diagnose-and-suggest on unclassified provider errors (RFC-self-resolve
   *  phase 2): one small-model call per unknown failure. On by default; the
   *  toggle is cost control. */
  selfDiagnose: boolean;
  /** Source hygiene sweep (RFC-source-hygiene): background re-fetch of
   *  aging url sources + unreachable flagging. On by default; cost control. */
  sourceHygiene: boolean;
  /** Days before a url source counts as stale and gets re-fetched. */
  hygieneRefreshDays: number;
  /** Per-source distillation sweep. On by default; the toggle is cost
   *  control, and the sweep self-heals either way. */
  sourceGists: boolean;
  /** Overnight effort notch: light | standard | generous (freshness.rs). */
  backgroundBudget: string;
}

/** One passage behind a meta-chat answer: what it is and where it lives. */
export interface MetaCitation {
  /** "card" = a registry card riding as answer context (corpus-scoped:
   *  notebookId is empty and the citation opens the card on Home). */
  kind: "source" | "note" | "card";
  notebookId: string;
  notebookTitle: string;
  /** Source id for source passages; note id for notes; card id for cards. */
  id: string;
  title: string;
  snippet: string;
}

/** Something a Home tool reply asks THIS window to do, because the backend
 *  can neither navigate the webview nor tear down the conversation the
 *  question was asked in. */
export interface MetaEffect {
  kind: "openNotebook" | "deleteChat";
  /** The notebook to open, for "openNotebook"; empty otherwise. */
  notebookId: string;
}

/** A corpus-wide answer (docs/RFC-meta-chat.md). */
export interface MetaAnswer {
  answer: string;
  citations: MetaCitation[];
  /** "chat" — a synthesized answer — or "tool", a command Home carried out
   *  (add a source, open a notebook, change a setting). Absent on answers
   *  from a backend older than the tool router. */
  kind?: MetaTurn["kind"];
  effect?: MetaEffect | null;
}

/** One exchange in Home's corpus-wide conversation, persisted in the
 *  `meta_turns` table (mirrors `models::MetaTurn`). */
export interface MetaTurn {
  id: string;
  threadId: string;
  role: "user" | "assistant";
  content: string;
  citations: MetaCitation[];
  /** "chat" | "stopped" (cut short by Stop/Esc — the partial answer is still
   *  worth keeping) | "error" (a provider failure: rendered as a danger wash
   *  and kept out of the history the model sees) | "tool" (a command Home
   *  carried out: one quiet row, and likewise never model context). */
  kind: "chat" | "stopped" | "error" | "tool";
  /** Which model wrote this answer. Empty on questions, and on answers
   *  stored before the backend recorded it — rendered as nothing. */
  model?: string;
  createdAt: number;
}

/** A Home conversation as the thread list sees it. Derived from its turns —
 *  a thread nobody asked into never existed. */
export interface MetaThread {
  id: string;
  /** What to call it: the small model's short name once one has been
   *  written, the opening question until then. */
  title: string;
  /** The opening question, trimmed to a line — kept so the list can still
   *  show what was actually asked. */
  question: string;
  turnCount: number;
  createdAt: number;
  updatedAt: number;
}

export interface McpStatus {
  running: boolean;
  port: number;
  url: string;
}

/** An ACP-capable agent Alchemy can host (docs/RFC-acp-agents.md). */
export interface AcpAgentInfo {
  id: string;
  label: string;
  available: boolean;
  /** Terminal command that signs this agent in, offered as an auth-failure fix. */
  loginCommand: string;
  /** Shell one-liner that installs it, shown when nothing is installed. */
  installHint: string;
}

/** Lifecycle of a hosted agent session, from the `acp://state` event. */
export interface AcpStateEvent {
  notebookId: string;
  agentId: string;
  state: "starting" | "ready" | "turn" | "idle" | "error" | "stopped";
  detail?: unknown;
}

/** One `session/update` notification, passed through as the ACP schema
 *  shape — `sessionUpdate` discriminates the variant. */
export interface AcpUpdateEvent {
  notebookId: string;
  update: {
    sessionUpdate: string;
    content?: { type: string; text?: string };
    title?: string;
    status?: string;
    toolCallId?: string;
    [key: string]: unknown;
  };
}

/** A permission request awaiting the user's answer. */
export interface AcpPermissionEvent {
  notebookId: string;
  requestId: string;
  toolTitle: string;
  options: { id: string; name: string; kind: string }[];
}

/** One agent client (Claude Code, Codex, …) and its connection state. */
export interface ConnectorStatus {
  id: string;
  name: string;
  installed: boolean;
  configured: boolean;
  /** False = we don't write its config; user copies the snippet. */
  canAuto: boolean;
  supportsSkill: boolean;
  skillInstalled: boolean;
  /** CLI one-liner or config snippet for manual setup. */
  snippet: string;
  /** Where its config lives, e.g. "~/.codex/config.toml". */
  configPath: string;
}

export interface UserProfile {
  name: string;
  profession: string;
  /** Standing instructions, kept in mind across chats and generations. */
  instructions: string;
  /** What the user calls the assistant; empty for no persona. */
  assistantName: string;
}

export interface ChatConfig {
  /** Writing-standard ids mirror rag::CHAT_STYLES in the backend. */
  style:
    | "default"
    | "learning"
    | "friendly"
    | "bffs"
    | "kids"
    | "professional"
    | "scientific"
    | "adhd"
    | "ste100"
    | "govuk"
    | "plain"
    | "gdev"
    | "custom";
  customPrompt: string;
  /** Persisted ids; the UI labels these Concise, Balanced, and Thorough. */
  length: "default" | "longer" | "shorter";
}

export const DEFAULT_CHAT_CONFIG: ChatConfig = {
  style: "default",
  customPrompt: "",
  length: "default",
};

/** Global chat reading preferences (display-only; set in Appearance). */
export interface ReadingPrefs {
  font: "sans" | "serif" | "mono" | "system";
  fontSize: "small" | "medium" | "large";
  textAlign: "natural" | "justified";
  /** Reader: floating table of contents on structured documents. */
  showToc: boolean;
  /** Reader: the related-passages rail beside documents and the editor. */
  showRelated: boolean;
  /** Experimental: window vibrancy behind translucent sidebar chrome. */
  glass: boolean;
  /** Glass opacity level, mirroring macOS's Clear/Tinted styles. */
  glassStyle: "tinted" | "clear";
}

export const DEFAULT_READING_PREFS: ReadingPrefs = {
  font: "sans",
  fontSize: "medium",
  textAlign: "natural",
  showToc: true,
  showRelated: true,
  glass: false,
  glassStyle: "tinted",
};
