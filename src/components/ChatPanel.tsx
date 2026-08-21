import { Fragment, memo, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Button, CardAction, Textarea, useConfirm } from "./ui";
import { Markdown } from "./Markdown";
import { cn, chatReadingClass, fmtDateTime, isWebUrl, relativeTime } from "@/lib/utils";
import { DitherBackground } from "./DitherBackground";
import { AlchemySymbol } from "./AlchemyHero";
import { DEFAULT_VERBS, THEMES, resolveThemeId } from "@/lib/themes";
import { FALLBACK_EPIGRAPHS, generatedEpigraph } from "@/lib/epigraph";
import {
  parseSlash,
  slashFilter,
  slashNorm,
  type SlashCommandMeta,
} from "@/lib/slashCommands";
import type {
  Citation,
  GrepHit,
  Message,
  NoteKind,
  ProviderEntry,
  ProviderModels,
  Source,
} from "@/lib/types";
import {
  MessageSquare,
  Wrench,
  ArrowDown,
  ArrowUp,
  Square,
  Eraser,
  Quote,
  StickyNote,
  Sparkles,
  Telescope,
  Check,
  Copy,
  NotebookPen,
  RefreshCw,
  CornerDownRight,
  ExternalLink,
  FileText,
  SlidersHorizontal,
  ChevronDown,
  AlertTriangle,
} from "lucide-react";

/** Composer autosize ceiling — past this the textarea scrolls instead. */
const COMPOSER_MAX_H = 180;

/** Fuzzy match for the @ picker: every query character (spaces ignored) must
 *  appear in order in the title. Substring hits outrank scattered ones, and
 *  word-start hits outrank mid-word — so "q3 rep" finds "Q3 Sales Report"
 *  but garbage trailing text stops matching and closes the picker. Returns
 *  null for no match; higher is better. */
function fuzzyScore(query: string, title: string): number | null {
  const t = title.toLowerCase();
  if (query === "") return 0;
  // Whole-query substring: strongest signal, earlier is better.
  const sub = t.indexOf(query);
  if (sub >= 0) return 1000 - sub;
  // Subsequence walk over non-space chars, rewarding word starts and runs.
  let ti = 0;
  let score = 0;
  let run = 0;
  for (const ch of query) {
    if (ch === " ") continue;
    let found = -1;
    for (let i = ti; i < t.length; i++) {
      if (t[i] === ch) {
        found = i;
        break;
      }
    }
    if (found < 0) return null;
    const wordStart = found === 0 || t[found - 1] === " ";
    run = found === ti ? run + 1 : 1;
    score += (wordStart ? 10 : 1) + run;
    ti = found + 1;
  }
  return score;
}

export function ChatPanel() {
  const currentId = useStore((s) => s.currentId);
  const messages = useStore((s) => s.messages);
  const messagesHasMore = useStore((s) => s.messagesHasMore);
  const messagesLoadingOlder = useStore((s) => s.messagesLoadingOlder);
  const loadOlderMessages = useStore((s) => s.loadOlderMessages);
  const sources = useStore((s) => s.sources);
  const sending = useStore((s) => s.sending);
  const streamingText = useStore((s) => s.streamingText);
  const steps = useStore((s) => s.steps);
  const waiting = useStore((s) => s.waiting);
  const agentMode = useStore((s) => s.agentMode);
  const toggleAgentMode = useStore((s) => s.toggleAgentMode);
  const send = useStore((s) => s.sendMessage);
  const cancelGeneration = useStore((s) => s.cancelGeneration);
  const reading = useStore((s) => s.reading);
  const clearChat = useStore((s) => s.clearChat);
  const appendToken = useStore((s) => s.appendToken);
  const appendStep = useStore((s) => s.appendStep);
  const theme = useStore((s) => s.theme);
  // Under glass the material is the ambience — the shader must not mount
  // (display:none alone leaves its rAF/WebGL loop running invisibly).
  const glassOn = useStore((s) => s.reading.glass);
  const followups = useStore((s) => s.followups);
  const summary = useStore((s) => s.summary);
  const summaryLoading = useStore((s) => s.summaryLoading);
  const refreshSummary = useStore((s) => s.refreshSummary);

  // In-progress text survives refreshes and restarts: mirrored to
  // localStorage per notebook. Restored lazily at first render (an effect
  // restore breaks under StrictMode — the replayed effects run the empty
  // save before the second restore, wiping the key), with `draftNb`
  // stamping which notebook the current draft state belongs to so the
  // mirror never writes one notebook's text under another's key.
  const [draft, setDraft] = useState(() =>
    currentId ? (localStorage.getItem(`chatDraft:${currentId}`) ?? "") : "",
  );
  const draftNb = useRef<string | null>(currentId);
  // Slash-command picker (see the composer below): index of the highlighted
  // row, and whether Esc/blur has dismissed the menu for the current draft.
  const [slashSel, setSlashSel] = useState(0);
  const [slashDismissed, setSlashDismissed] = useState(false);
  // @ mentions: picked source/note handles for THIS message. The text keeps
  // reading naturally ("what does @Q3 Report say?") while the recorded ids
  // narrow retrieval for the one send. A mention whose "@Title" text is
  // edited out of the draft is dropped at send time.
  const [mentions, setMentions] = useState<
    { id: string; kind: "source" | "note"; title: string }[]
  >([]);
  const [mentionSel, setMentionSel] = useState(0);
  const [mentionDismissed, setMentionDismissed] = useState(false);
  const notes = useStore((s) => s.notes);
  const failedInput = useStore((s) => s.failedInput);
  const { confirm, dialog: confirmDialog } = useConfirm();
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // Restore the saved draft for this notebook (refresh/restart survival),
  // then keep the mirror current. Restore runs first and stamps `draftNb`;
  // the save effect below refuses to write until the stamp matches, so a
  // notebook switch can't leak the previous notebook's text.
  // Notebook switch while mounted: swap in the new notebook's saved draft.
  // Skip when the stamp already matches (initial render restored it, and
  // StrictMode replays this effect with the stamp intact).
  useEffect(() => {
    if (draftNb.current === currentId) return;
    draftNb.current = currentId;
    setDraft(
      currentId ? (localStorage.getItem(`chatDraft:${currentId}`) ?? "") : "",
    );
  }, [currentId]);
  useEffect(() => {
    if (!currentId || draftNb.current !== currentId) return;
    if (draft) localStorage.setItem(`chatDraft:${currentId}`, draft);
    else localStorage.removeItem(`chatDraft:${currentId}`);
  }, [draft, currentId]);

  // A failed send hands its text back — restore it into the composer so the
  // user can retry without retyping.
  useEffect(() => {
    if (failedInput) {
      setDraft((d) => d || failedInput);
      useStore.setState({ failedInput: null });
    }
  }, [failedInput]);

  // Another surface (the source reader's "Ask about this") staged text for
  // the composer — load it and focus so the user can finish their question.
  // The empty string is a focus-only request ("Ask about this source" scoped
  // retrieval and wants the caret here, without touching the draft).
  const pendingInput = useStore((s) => s.pendingInput);
  useEffect(() => {
    if (pendingInput === null) return;
    if (pendingInput) setDraft(pendingInput);
    useStore.setState({ pendingInput: null });
    // Focus after the surface that staged the text (a modal) has closed.
    setTimeout(() => {
      const el = inputRef.current;
      if (!el) return;
      el.focus();
      el.selectionStart = el.selectionEnd = el.value.length;
    }, 0);
  }, [pendingInput]);

  // Autosize from the draft, not from onChange: sending, a slash reset, a
  // follow-up click and retry-after-failure all set the text programmatically,
  // so keying off the value is what makes the box shrink back as well as grow.
  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_H)}px`;
  }, [draft]);

  // Subscribe once to streaming tokens + agent progress steps from the backend.
  // Events broadcast to every window — only the one with a send in flight
  // should accumulate them.
  useEffect(() => {
    const unToken = listen<{ content: string }>("chat://token", (e) => {
      if (useStore.getState().sending) appendToken(e.payload.content);
    });
    const unStep = listen<{ label: string; transient: boolean }>(
      "chat://step",
      (e) => {
        if (useStore.getState().sending)
          appendStep(e.payload.label, e.payload.transient);
      },
    );
    // Verify-and-repair swaps a revised answer under the same message id
    // (backend spawn_answer_verify). Events reach every window — apply
    // only when the message is in this window's transcript.
    const unRevised = listen<{ id: string; content: string }>(
      "chat://revised",
      (e) => {
        const { messages } = useStore.getState();
        if (!messages.some((m) => m.id === e.payload.id)) return;
        useStore.setState({
          messages: messages.map((m) =>
            m.id === e.payload.id ? { ...m, content: e.payload.content } : m,
          ),
        });
      },
    );
    return () => {
      unToken.then((fn) => fn());
      unStep.then((fn) => fn());
      unRevised.then((fn) => fn());
    };
  }, [appendToken, appendStep]);

  // "Focus the chat composer" command from the Cmd+K menu.
  useEffect(() => {
    const onFocus = () => inputRef.current?.focus();
    window.addEventListener("nb:focus-composer", onFocus);
    return () => window.removeEventListener("nb:focus-composer", onFocus);
  }, []);

  // Jump straight to the latest message when a notebook's chat first loads —
  // the near-bottom guard below would otherwise leave us stuck at the top.
  const initialScrollDone = useRef(false);
  useEffect(() => {
    initialScrollDone.current = false;
  }, [currentId]);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || initialScrollDone.current || messages.length === 0) return;
    el.scrollTop = el.scrollHeight;
    initialScrollDone.current = true;
  }, [messages, currentId]);

  // Autoscroll on new content — but only when the user is already near the
  // bottom, so scrolling up to re-read mid-stream isn't yanked back down.
  // `atBottom` also drives the "jump to latest" pill when content arrives
  // off-screen.
  const [atBottom, setAtBottom] = useState(true);
  const updateAtBottom = () => {
    const el = scrollRef.current;
    if (!el) return;
    setAtBottom(el.scrollHeight - el.scrollTop - el.clientHeight < 120);
  };
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
    if (nearBottom) el.scrollTo({ top: el.scrollHeight });
    setAtBottom(nearBottom);
  }, [messages, streamingText, steps]);

  // Sending your own message always jumps to it, even from deep in history —
  // the near-bottom guard is for incoming content, not your own action.
  useEffect(() => {
    if (!sending) return;
    const el = scrollRef.current;
    el?.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }, [sending]);

  const jumpToLatest = () => {
    const el = scrollRef.current;
    el?.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  };

  const loadEarlier = async () => {
    const el = scrollRef.current;
    const height = el?.scrollHeight ?? 0;
    const top = el?.scrollTop ?? 0;
    await loadOlderMessages();
    // Prepending history must not move the passage the user was reading.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (el) el.scrollTop = top + (el.scrollHeight - height);
      });
    });
  };

  const canChat = !!currentId && sources.length > 0;
  const isBlank = messages.length === 0 && !sending;

  function submit() {
    const text = draft.trim();
    if (!text || sending || !canChat) return;
    // A typed "/command args" runs the command; an unknown slash falls through
    // and sends as a normal message.
    if (text.startsWith("/")) {
      const parsed = parseSlash(text);
      if (parsed) {
        resetComposer();
        void runSlash(parsed.cmd, parsed.arg);
        return;
      }
    }
    // @ mentions narrow retrieval to exactly what was named: mentioned
    // sources by id, folders as their ready children, notes via their
    // prefixed chunk-owner ids. No mentions → the checkbox selection rules.
    let override: string[] | undefined;
    if (activeMentions.length) {
      const ids = new Set<string>();
      for (const m of activeMentions) {
        if (m.kind === "note") {
          ids.add(`note:${m.id}`);
        } else if (sources.find((s) => s.id === m.id)?.sourceType === "folder") {
          for (const c of sources) {
            if (c.parentId === m.id && c.status === "ready") ids.add(c.id);
          }
        } else {
          ids.add(m.id);
        }
      }
      override = [...ids];
    }
    resetComposer();
    void send(text, override);
  }

  // The picker is a pure command chooser: it's active only while the (space-
  // free) name is being typed as the first characters. A space commits the
  // choice and switches the composer to argument entry, closing the picker.
  const slashName =
    draft.startsWith("/") && !draft.includes("\n") && !/\s/.test(draft.slice(1))
      ? draft.slice(1)
      : null;
  const slashOpen = slashName !== null && !slashDismissed;
  const slashResults = useMemo(
    () => (slashName === null ? [] : slashFilter(slashName)),
    [slashName],
  );
  useEffect(() => {
    setSlashSel((i) => Math.min(i, Math.max(0, slashResults.length - 1)));
  }, [slashResults.length]);

  // The active "@query" token: at the end of the draft (the same type-at-the-
  // caret simplicity as the slash picker) and never mid-word — "user@host"
  // must not open a picker. Spaces are allowed inside the query ("@Q3 sales
  // report"), so the token runs from the last standalone "@" to the caret;
  // fuzzy matching plus the no-results guard is what closes the picker once
  // the trailing text stops looking like a title.
  const mentionMatch = /(^|\s)@([^\s@\n][^@\n]{0,59})?$/.exec(draft);
  const mentionQuery = mentionMatch ? (mentionMatch[2] ?? "") : null;
  const mentionResults = useMemo(() => {
    if (mentionQuery === null) return [];
    const q = mentionQuery.toLowerCase().trim();
    // A title already committed to the draft is done — offering it again
    // would hold the picker open forever after a pick (the "@Title " text
    // itself matches its own title).
    const picked = new Set(
      mentions
        .filter((m) => draft.includes(`@${m.title}`))
        .map((m) => `${m.kind}:${m.id}`),
    );
    // Sources first (folders included — a folder expands to its ready
    // children at send), then notes; capped so a big repo can't flood it.
    const rank = <T,>(
      items: T[],
      title: (x: T) => string,
      keep: (x: T) => boolean,
      cap: number,
    ) =>
      items
        .flatMap((x) => {
          if (!keep(x)) return [];
          const score = fuzzyScore(q, title(x));
          return score === null ? [] : [{ x, score }];
        })
        .sort((a, b) => b.score - a.score)
        .slice(0, cap)
        .map(({ x }) => x);
    const srcs = rank(
      sources,
      (s) => s.title,
      (s) => s.status === "ready" && !picked.has(`source:${s.id}`),
      6,
    ).map((s) => ({ id: s.id, kind: "source" as const, title: s.title }));
    const nts = rank(
      notes,
      (n) => n.title,
      (n) => !picked.has(`note:${n.id}`),
      4,
    ).map((n) => ({ id: n.id, kind: "note" as const, title: n.title }));
    return [...srcs, ...nts];
  }, [mentionQuery, sources, notes, mentions, draft]);
  const mentionOpen =
    mentionQuery !== null && !mentionDismissed && mentionResults.length > 0;
  useEffect(() => {
    setMentionSel((i) => Math.min(i, Math.max(0, mentionResults.length - 1)));
  }, [mentionResults.length]);

  function pickMention(m: { id: string; kind: "source" | "note"; title: string }) {
    // Replace the trailing "@query" with the canonical "@Title " token; the
    // recorded id — not the text — is what narrows retrieval.
    const next = draft.replace(/@(?:[^\s@\n][^@\n]{0,59})?$/, `@${m.title} `);
    setDraft(next);
    setMentions((cur) => (cur.some((x) => x.id === m.id) ? cur : [...cur, m]));
    setMentionSel(0);
    requestAnimationFrame(() => {
      const el = inputRef.current;
      if (!el) return;
      el.focus();
      el.selectionStart = el.selectionEnd = next.length;
    });
  }

  /** Mentions whose "@Title" text still survives in the draft. */
  const activeMentions = mentions.filter((m) => draft.includes(`@${m.title}`));

  function resetComposer() {
    setDraft("");
    setSlashSel(0);
    setSlashDismissed(false);
    setMentions([]);
    setMentionSel(0);
    setMentionDismissed(false);
  }

  // Tab, or Enter on an argument-required command: drop the canonical name into
  // the composer with a trailing space and keep focus so the user can type
  // arguments (the space closes the picker on its own).
  function completeSlash(c: SlashCommandMeta) {
    const next = `/${c.name} `;
    setDraft(next);
    setSlashSel(0);
    setSlashDismissed(false);
    requestAnimationFrame(() => {
      const el = inputRef.current;
      if (!el) return;
      el.focus();
      el.selectionStart = el.selectionEnd = next.length;
    });
  }

  // Enter/click on a picker row: complete arg-required commands, run the rest.
  function activateSlash(c: SlashCommandMeta) {
    if (c.argRequired) {
      completeSlash(c);
      return;
    }
    void runSlash(c, "");
    resetComposer();
  }

  async function runGrep(pattern: string) {
    const s = useStore.getState();
    const id = s.currentId;
    if (!id) return;
    let hits: GrepHit[];
    try {
      hits = await api.grepSources(id, pattern);
    } catch (e) {
      s.pushToast("error", e instanceof Error ? e.message : String(e));
      return;
    }
    // A local, non-LLM transcript row — never persisted, so it stays out of the
    // model's history and simply shows the matches inline.
    const msg: Message = {
      id: `grep-${Date.now()}`,
      notebookId: id,
      role: "assistant",
      content: renderGrepMarkdown(pattern, hits),
      citations: [],
      kind: "chat",
      model: `grep · ${hits.length} ${hits.length === 1 ? "match" : "matches"}`,
      createdAt: Date.now(),
    };
    useStore.setState((st) => ({ messages: [...st.messages, msg] }));
  }

  async function runSlash(c: SlashCommandMeta, rawArg: string) {
    const s = useStore.getState();
    const arg = rawArg.trim();

    // Generators (any non-Actions family) map name → note kind; trailing text
    // becomes optional custom instructions for the generation.
    if (c.family !== "Actions") {
      if (c.name === "audio_overview" && !s.kokoroStatus?.verified) {
        s.pushToast(
          "info",
          "Audio Overview needs its voices. Set them up in Settings → Studio.",
        );
        return;
      }
      void s.generateArtifact(c.name as NoteKind, arg || undefined);
      return;
    }

    switch (c.name) {
      case "add": {
        if (!isWebUrl(arg)) {
          s.pushToast("error", "Add needs a web URL, e.g. /add https://example.com");
          return;
        }
        void s.addSourceUrl(arg);
        s.pushToast("info", "Adding source…");
        return;
      }
      case "model": {
        const cfg = s.aiConfig;
        if (!cfg) return;
        const q = slashNorm(arg);
        const providers = cfg.providers;
        const hit =
          providers.find((p) => slashNorm(p.label) === q || slashNorm(p.id) === q) ??
          providers.find(
            (p) =>
              slashNorm(p.label).includes(q) ||
              slashNorm(p.id).includes(q) ||
              slashNorm(p.chatModel).includes(q),
          );
        if (!hit) {
          s.pushToast("error", `No model matches “${arg}”`);
          return;
        }
        void s.saveAiConfig({ ...cfg, chatProvider: hit.id });
        s.pushToast("success", `Answering with ${hit.label}`);
        return;
      }
      case "research": {
        const want = arg.toLowerCase();
        const on = s.agentMode;
        const target = want === "on" ? true : want === "off" ? false : !on;
        if (on !== target) s.toggleAgentMode();
        s.pushToast("info", `Deep research: ${target ? "on" : "off"}`);
        return;
      }
      case "grep": {
        if (!arg) {
          s.pushToast("error", "Grep needs a pattern, e.g. /grep TODO");
          return;
        }
        void runGrep(arg);
        return;
      }
      case "note": {
        if (arg) {
          void s.createNote(noteTitleFrom(arg), arg);
          s.pushToast("success", "Note saved");
          return;
        }
        const last = [...s.messages]
          .reverse()
          .find((m) => m.role === "assistant" && m.kind === "chat");
        if (!last) {
          s.pushToast("info", "No answer to save yet");
          return;
        }
        void s.createNote(
          noteTitleFrom(last.content),
          noteContentFrom(last.content, last.citations, s.sources),
        );
        s.pushToast("success", "Saved the last answer as a note");
        return;
      }
      case "template": {
        // Deterministic and instant — no model call. The argument becomes
        // the generation prompt, the first words become a working name, and
        // the editor opens for refinement. Asking the chat in prose ("make
        // me a generator that…") is the model-composed route instead.
        const name = arg
          ? arg.split(/\s+/).slice(0, 5).join(" ").replace(/[.,;:!?]+$/, "")
          : "New template";
        const prompt =
          arg ||
          "Describe what this generator should produce from the notebook's sources.";
        void (async () => {
          try {
            const t = await api.saveTemplate(null, name, "", prompt);
            await s.refreshTemplates();
            s.openInReader({ type: "template", id: t.id });
          } catch (e) {
            s.pushToast("error", e instanceof Error ? e.message : String(e));
          }
        })();
        return;
      }
      case "report": {
        const scheds = s.reportSchedules;
        if (scheds.length === 1) {
          void s.runReportNow(scheds[0].id);
          s.pushToast("info", `Running “${scheds[0].name}”…`);
          return;
        }
        if (!s.studioOpen) s.toggleStudio();
        s.pushToast(
          "info",
          scheds.length === 0
            ? "No reports yet — schedule one in Studio → Reports"
            : "Choose a report to run in Studio → Reports",
        );
        return;
      }
      case "clear": {
        const ok = await confirm({
          title: "Clear this conversation?",
          confirmLabel: "Clear",
          danger: true,
        });
        if (ok) void s.clearChat();
        return;
      }
    }
  }

  return (
    <div className="relative flex h-full flex-1 flex-col min-w-0">
      {isBlank && !glassOn && (
        <>
          <div className="glass-mist pointer-events-none absolute inset-0 z-0">
            <DitherBackground themeKey={theme} />
          </div>
          <div className="chat-mist-fade glass-mist pointer-events-none absolute inset-0 z-0" />
        </>
      )}
      <div className="relative z-10 flex items-center px-5 h-12 border-b border-border">
        <MessageSquare className="h-4 w-4 text-muted-foreground" />
        <span className="ml-2 text-caption font-semibold uppercase tracking-wide text-muted-foreground">
          Chat
        </span>
        <div className="ml-auto flex items-center gap-1">
          {messages.length > 0 && (
            <Button
              variant="ghost"
              size="sm"
              onClick={async () => {
                if (await confirm({ title: "Clear this conversation?", confirmLabel: "Clear", danger: true }))
                  clearChat();
              }}
            >
              <Eraser className="h-3.5 w-3.5" />
              Clear
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon"
            onClick={() => useStore.getState().openSettings("chat")}
            title="Chat settings (style, length, custom prompt)"
            aria-label="Chat settings"
          >
            <SlidersHorizontal className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      <div ref={scrollRef} onScroll={updateAtBottom} className="relative z-10 flex-1 overflow-y-auto">
        <div className={cn("mx-auto flex max-w-[720px] flex-col gap-6 px-5 py-6", chatReadingClass(reading))}>
          {canChat && (
            <SummaryBanner
              summary={summary}
              loading={summaryLoading}
              // Show the summary from the top — immediately on click (the
              // loading banner already grows the top of the transcript, and
              // scroll anchoring would pin the bottom and cut it off) and
              // again when the text lands and the banner grows tall.
              onRefresh={() => {
                scrollRef.current?.scrollTo({ top: 0 });
                void refreshSummary().then(() => {
                  scrollRef.current?.scrollTo({ top: 0 });
                });
              }}
              centered={isBlank}
            />
          )}

          {/* The sigil: full-size welcome on a truly blank notebook; once a
              summary exists it stays as a compact emblem between the summary
              and the start of the thread. */}
          {(isBlank || (canChat && !!summary)) && (
            <ChatHero
              hasNotebook={!!currentId}
              hasSources={sources.length > 0}
              compact={canChat && !!summary}
            />
          )}

          {messagesHasMore && (
            <div className="flex justify-center">
              <Button
                variant="ghost"
                size="sm"
                loading={messagesLoadingOlder}
                onClick={() => void loadEarlier()}
              >
                Load earlier messages
              </Button>
            </div>
          )}

          {messages.map((m) => (
            <ChatMessage key={m.id} message={m} />
          ))}

          {sending && (
            <div className="flex flex-col gap-2">
              <RoleLabel role="assistant" />
              {steps.length > 0 && (
                <StepTrail
                  steps={steps}
                  waiting={waiting}
                  done={!!streamingText}
                />
              )}
              {streamingText ? (
                <Markdown>{streamingText}</Markdown>
              ) : (
                steps.length === 0 && <ThinkingDots />
              )}
            </div>
          )}

          {!sending && followups.length > 0 && messages.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <span className="text-micro font-medium uppercase tracking-wide text-subtle-foreground">
                Suggested follow-ups
              </span>
              {followups.map((q, i) => (
                <button
                  key={i}
                  onClick={() => {
                    // Fill the composer instead of firing immediately — the
                    // user can tweak the question, or just hit Enter.
                    setDraft(q);
                    inputRef.current?.focus();
                  }}
                  className="flex items-start gap-2 rounded-lg border border-border bg-surface/60 px-3 py-2 text-left text-body text-foreground/90 transition-colors hover:border-border-strong hover:bg-surface-2"
                >
                  <CornerDownRight className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  {q}
                </button>
              ))}
            </div>
          )}
        </div>

        {!atBottom && !isBlank && (
          <div className="pointer-events-none sticky bottom-3 z-20 flex justify-center">
            <button
              onClick={jumpToLatest}
              className={cn(
                "pointer-events-auto flex h-7 items-center gap-1.5 rounded-full border border-border-strong",
                "bg-elevated/95 px-3 text-micro font-medium text-muted-foreground shadow-lg backdrop-blur",
                "transition-colors hover:text-foreground",
              )}
            >
              <ArrowDown className="h-3 w-3" />
              {sending ? "New content below" : "Jump to latest"}
            </button>
          </div>
        )}
      </div>

      <div className="relative z-10 px-5 pb-5 pt-2">
        <div className="mx-auto max-w-[720px]">
          <div
            className={cn(
              "relative rounded-lg border border-border-strong bg-surface p-2.5 shadow-md transition-colors",
              "focus-within:border-ring/60",
            )}
          >
            {slashOpen && (
              <SlashPicker
                results={slashResults}
                selected={slashSel}
                onHover={setSlashSel}
                onPick={activateSlash}
              />
            )}
            {mentionOpen && !slashOpen && (
              <MentionPicker
                results={mentionResults}
                selected={mentionSel}
                onHover={setMentionSel}
                onPick={pickMention}
              />
            )}
            <Textarea
              ref={inputRef}
              rows={1}
              className="border-0 bg-transparent focus:ring-0 min-h-[24px] max-h-[180px] px-1.5 py-1"
              placeholder={
                canChat
                  ? "Ask anything — / for commands, @ to ask about one source…"
                  : currentId
                    ? "Add a source to start chatting"
                    : "Select or create a notebook"
              }
              value={draft}
              disabled={!canChat}
              role="combobox"
              aria-expanded={slashOpen}
              aria-controls={slashOpen ? "slash-picker" : undefined}
              aria-activedescendant={
                slashOpen && slashResults[slashSel]
                  ? `slash-${slashResults[slashSel].name}`
                  : undefined
              }
              onChange={(e) => {
                setDraft(e.target.value);
                // Any edit re-opens the picker (Esc/blur only dismiss the
                // current text) and resets the highlight to the top match.
                setSlashDismissed(false);
                setSlashSel(0);
                setMentionDismissed(false);
              }}
              // Clicking outside (Send button, model pill, transcript) closes
              // the pickers; row clicks use onMouseDown+preventDefault so
              // focus never leaves and this doesn't fire.
              onBlur={() => {
                setSlashDismissed(true);
                setMentionDismissed(true);
              }}
              onKeyDown={(e) => {
                // isComposing: don't act mid-IME-composition (CJK input).
                if (e.nativeEvent.isComposing) return;
                if (slashOpen) {
                  const c = slashResults[slashSel];
                  if (e.key === "ArrowDown") {
                    e.preventDefault();
                    setSlashSel((i) =>
                      slashResults.length ? (i + 1) % slashResults.length : 0,
                    );
                    return;
                  }
                  if (e.key === "ArrowUp") {
                    e.preventDefault();
                    setSlashSel((i) =>
                      slashResults.length
                        ? (i - 1 + slashResults.length) % slashResults.length
                        : 0,
                    );
                    return;
                  }
                  if (e.key === "Escape") {
                    e.preventDefault();
                    e.stopPropagation();
                    setSlashDismissed(true);
                    return;
                  }
                  if (e.key === "Tab") {
                    e.preventDefault();
                    if (c) completeSlash(c);
                    return;
                  }
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    if (c) activateSlash(c);
                    else submit(); // no match → send as a plain message
                    return;
                  }
                  // Other keys type through and re-filter the picker.
                  return;
                }
                if (mentionOpen) {
                  const m = mentionResults[mentionSel];
                  if (e.key === "ArrowDown") {
                    e.preventDefault();
                    setMentionSel((i) =>
                      mentionResults.length ? (i + 1) % mentionResults.length : 0,
                    );
                    return;
                  }
                  if (e.key === "ArrowUp") {
                    e.preventDefault();
                    setMentionSel((i) =>
                      mentionResults.length
                        ? (i - 1 + mentionResults.length) % mentionResults.length
                        : 0,
                    );
                    return;
                  }
                  if (e.key === "Escape") {
                    e.preventDefault();
                    e.stopPropagation();
                    setMentionDismissed(true);
                    return;
                  }
                  if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
                    e.preventDefault();
                    if (m) pickMention(m);
                    return;
                  }
                  return;
                }
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  submit();
                }
              }}
            />
            <div className="flex items-center gap-1.5 px-1.5 pt-1">
              <button
                onClick={toggleAgentMode}
                title="Deep research: several searches over your sources before answering"
                className={cn(
                  "inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-micro transition-colors",
                  agentMode
                    ? "border-primary/50 bg-primary/15 text-citation"
                    : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
                )}
              >
                <Telescope className="h-3 w-3" />
                {agentMode ? "Deep research: on" : "Deep research: off"}
              </button>
              <ModelPill />
              {activeMentions.length > 0 && (
                <span
                  className="min-w-0 truncate text-micro text-subtle-foreground"
                  title={activeMentions.map((m) => m.title).join(", ")}
                >
                  Searching only:{" "}
                  {activeMentions.map((m) => m.title).join(", ")}
                </span>
              )}
              <span className="flex-1" />
              {sending ? (
                <Button
                  variant="secondary"
                  size="icon"
                  onClick={() => cancelGeneration("chat")}
                  title="Stop"
                  aria-label="Stop generating"
                >
                  <Square className="h-3.5 w-3.5" />
                </Button>
              ) : (
                <Button
                  variant="primary"
                  size="icon"
                  onClick={submit}
                  disabled={!draft.trim() || !canChat}
                  title="Send"
                  aria-label="Send message"
                >
                  <ArrowUp className="h-4 w-4" />
                </Button>
              )}
            </div>
          </div>
        </div>
      </div>

      {confirmDialog}
    </div>
  );
}

function SummaryBanner({
  summary,
  loading,
  onRefresh,
  centered,
}: {
  summary: string;
  loading: boolean;
  onRefresh: () => void;
  /** Blank notebook: the chip sits under the centered hero, so center it. */
  centered?: boolean;
}) {
  if (!summary && !loading) {
    return (
      <button
        onClick={onRefresh}
        className={cn(
          "rounded-lg border border-dashed border-border-strong bg-surface/50 px-3 py-1.5 text-caption text-muted-foreground transition-colors hover:text-foreground",
          centered ? "self-center" : "self-start",
        )}
      >
        <Sparkles className="mr-1.5 inline h-3 w-3" />
        Generate notebook summary
      </button>
    );
  }
  return (
    <div className="rounded-lg border border-border bg-surface/60 p-3.5">
      <div className="mb-1 flex items-center justify-between">
        <span className="text-micro font-medium uppercase tracking-wide text-subtle-foreground">
          Notebook summary
        </span>
        <button
          onClick={onRefresh}
          className="text-muted-foreground transition-colors hover:text-foreground"
          title="Regenerate summary"
        >
          <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
        </button>
      </div>
      {loading && !summary ? (
        <div className="text-body text-muted-foreground">Summarizing sources…</div>
      ) : (
        <div className="text-body leading-relaxed text-foreground/90 selectable">
          {/* Single newlines become markdown hard breaks so the model's line
              breaks survive; double newlines stay paragraphs. */}
          <Markdown>{summary.replace(/\n(?!\n)/g, "  \n")}</Markdown>
        </div>
      )}
    </div>
  );
}

/** Resend the question an error row failed to answer: both rows leave the
 *  transcript and the normal send pipeline owns the fresh attempt. With a
 *  providerOverride, the rerun answers on that engine — config untouched
 *  (RFC-self-resolve phase 4). Never loops: one click, one resend. */
async function resendFailed(message: Message, providerOverride?: string) {
  const msgs = useStore.getState().messages;
  const i = msgs.findIndex((m) => m.id === message.id);
  const prevUser = msgs
    .slice(0, Math.max(i, 0))
    .reverse()
    .find((m) => m.role === "user");
  if (!prevUser || useStore.getState().sending) return;
  await api.deleteMessage(message.id);
  await api.deleteMessage(prevUser.id);
  useStore.setState({
    messages: msgs.filter((m) => m.id !== message.id && m.id !== prevUser.id),
  });
  void useStore
    .getState()
    .sendMessage(prevUser.content, undefined, providerOverride);
}

/** Display name for a provider id in fix buttons; falls back to the id. */
function providerLabelFor(providerId: string): string {
  const p = useStore
    .getState()
    .aiConfig?.providers.find((x) => x.id === providerId);
  return p?.label || providerId;
}

/** "Answer with Ollama / Apple Intelligence" on the latest error row
 *  (RFC-self-resolve phase 4): one-click rerun of this question on a local
 *  engine, offered only when that engine is actually alive (same readiness
 *  probe the provider pill uses) and isn't the provider that just failed. */
function FallbackOffers({ message }: { message: Message }) {
  const aiConfig = useStore((s) => s.aiConfig);
  const chatProvider = aiConfig?.chatProvider;
  const [alive, setAlive] = useState<{ id: string; label: string }[]>([]);
  useEffect(() => {
    if (!aiConfig) return;
    let live = true;
    const candidates = aiConfig.providers.filter(
      (p) => (p.kind === "ollama" || p.kind === "fm") && p.id !== chatProvider,
    );
    void Promise.all(
      candidates.map(async (p) => {
        try {
          const r = await api.providerReadinessOne(p.id);
          return r.ready ? p : null;
        } catch {
          return null;
        }
      }),
    ).then((rs) => {
      if (!live) return;
      setAlive(
        rs
          .filter((p): p is NonNullable<typeof p> => p !== null)
          .map((p) => ({
            id: p.id,
            label: p.kind === "fm" ? "Apple Intelligence" : p.label,
          })),
      );
    });
    return () => {
      live = false;
    };
    // Re-probe only when the failing provider changes, not per render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chatProvider]);
  return (
    <>
      {alive.map((p) => (
        <Button
          key={p.id}
          variant="ghost"
          size="sm"
          onClick={() => void resendFailed(message, p.id)}
        >
          Answer with {p.label}
        </Button>
      ))}
    </>
  );
}

/** The transcript's literal button grammars, shared by tool rows and error
 *  rows (RFC-self-resolve + RFC-conversational-setup): a Terminal launch
 *  (backend-allowlisted), a Settings-tab jump, a provider switch applied
 *  through the settings tool, and the connect confirm-click — the ONLY
 *  chat-side path that writes an agent client's config. */
function GrammarActions({ content }: { content: string }) {
  const fixCmd = /Fix: open Terminal, run `([^`]+)`/.exec(content)?.[1];
  const settingsTab = /Settings → (Models|General|Sources|Studio|Agents)/.exec(
    content,
  )?.[1]?.toLowerCase();
  const switchFix = /Fix: switch (chat|studio) to provider `([^`]+)`/.exec(
    content,
  );
  const connectFix = /Confirm: connect agent `([^`]+)` \(([^)]+)\)/.exec(
    content,
  );
  const appendToolRow = (toolRow: Message) =>
    useStore.setState({
      messages: [...useStore.getState().messages, toolRow],
    });
  const applySwitch = async (role: string, providerId: string) => {
    const nb = useStore.getState().currentId;
    if (!nb) return;
    try {
      appendToolRow(
        await api.applySettingsFix(
          nb,
          role === "studio" ? "studioProvider" : "chatProvider",
          providerId,
        ),
      );
      useStore.setState({ aiConfig: await api.getAiConfig() });
    } catch (e) {
      useStore
        .getState()
        .pushToast("error", e instanceof Error ? e.message : String(e));
    }
  };
  const applyConnect = async (clientId: string) => {
    const nb = useStore.getState().currentId;
    if (!nb) return;
    try {
      appendToolRow(await api.applyConnectFix(nb, clientId));
    } catch (e) {
      useStore
        .getState()
        .pushToast("error", e instanceof Error ? e.message : String(e));
    }
  };
  if (!fixCmd && !settingsTab && !switchFix && !connectFix) return null;
  return (
    <>
      {fixCmd && (
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void api.openInTerminal(fixCmd)}
        >
          <ExternalLink className="h-3.5 w-3.5" />
          Open Terminal: {fixCmd}
        </Button>
      )}
      {switchFix && (
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void applySwitch(switchFix[1], switchFix[2])}
        >
          Switch {switchFix[1]} to {providerLabelFor(switchFix[2])}
        </Button>
      )}
      {connectFix && (
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void applyConnect(connectFix[1])}
        >
          Connect {connectFix[2]}
        </Button>
      )}
      {settingsTab && (
        <Button
          variant="ghost"
          size="sm"
          onClick={() => useStore.getState().openSettings(settingsTab)}
        >
          {settingsTab === "models" ? "Model settings…" : "Open Settings"}
        </Button>
      )}
    </>
  );
}

// Memoized: message objects are stable across renders, and without this
// every streamed token re-rendered (and re-parsed the markdown of) the
// entire transcript, not just the growing tail.
const ChatMessage = memo(function ChatMessage({
  message,
}: {
  message: Message;
}) {
  // Retry only makes sense on the latest exchange — resending an older
  // question would teleport it to the bottom of the transcript.
  const isLast = useStore(
    (s) => s.messages[s.messages.length - 1]?.id === message.id,
  );
  // Tool confirmations are process, not conversation: one quiet gray row,
  // no bubble, no role label — the Claude-desktop "Ran ..." grammar. The
  // settings tool's verbs (pull staging, setup steps, connect confirms)
  // carry the shared button grammars, parsed on the latest row only.
  if (message.kind === "tool") {
    return (
      <div className="flex items-start gap-2 py-0.5 text-caption text-muted-foreground">
        <Wrench className="mt-0.5 h-3 w-3 shrink-0 text-subtle-foreground" />
        {/* pre-line: the settings tool's snapshot reply is multi-line. */}
        <span className="selectable min-w-0 whitespace-pre-line">
          {message.content}
          {isLast && (
            <span className="ml-2 inline-flex flex-wrap items-center gap-2 align-middle">
              <GrammarActions content={message.content} />
            </span>
          )}
        </span>
      </div>
    );
  }
  if (message.role === "user") {
    return (
      <div className="group flex flex-col items-end gap-1">
        {/* wrap-anywhere: a pasted URL has no break opportunities, so without
            it the bubble sizes to the URL and scrolls the transcript. */}
        <div className="max-w-[85%] min-w-0 wrap-anywhere rounded-lg rounded-br-sm bg-surface-2 px-3.5 py-2 text-body selectable border border-border">
          {message.content}
        </div>
        <UserMessageActions message={message} />
      </div>
    );
  }
  // Provider failures are part of the conversation record: a quiet danger
  // wash naming the provider, so an unanswered question is never a mystery.
  // When the advice names a fix, it becomes a button: launch Terminal with
  // the sign-in command (backend allowlists the set), jump to Settings,
  // apply a suggested provider switch through the settings tool, or rerun
  // this question on a live local engine (RFC-self-resolve phases 2–4).
  if (message.kind === "error") {
    return (
      <div className="flex flex-col gap-1.5">
        <RoleLabel role="assistant" />
        <div className="rounded-lg border border-destructive/30 bg-destructive/10 px-3.5 py-2.5">
          <div className="flex items-start gap-2 text-body text-foreground">
            <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" />
            {/* pre-line: a phase-2 diagnosis arrives as extra lines. */}
            <span className="selectable min-w-0 whitespace-pre-line">
              {message.content}
            </span>
          </div>
          <div className="mt-2 flex flex-wrap items-center gap-2">
            {isLast && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => void resendFailed(message)}
              >
                <RefreshCw className="h-3.5 w-3.5" />
                Retry
              </Button>
            )}
            <GrammarActions content={message.content} />
            {isLast && <FallbackOffers message={message} />}
          </div>
          {message.model && (
            <div className="mt-1.5 text-micro text-subtle-foreground">
              {message.model}
            </div>
          )}
        </div>
      </div>
    );
  }
  return (
    <div className="group flex flex-col gap-2">
      <RoleLabel role="assistant" />
      <Markdown
        citations={message.citations}
        onCitation={openCitationTarget}
      >
        {message.content}
      </Markdown>
      {message.citations.length > 0 && <Citations citations={message.citations} />}
      {message.model && (
        <div className="mt-1 text-micro text-subtle-foreground">
          {message.model}
        </div>
      )}
      <MessageActions content={message.content} citations={message.citations} />
    </div>
  );
});

function noteTitleFrom(content: string): string {
  const line = content.split("\n").map((l) => l.trim()).find(Boolean) ?? "";
  const clean = line.replace(/^#+\s*/, "").replace(/[*_`>#]/g, "").trim();
  return clean.slice(0, 60) || "Chat response";
}

/** A chat answer's [n] markers reference the MESSAGE's citation list, which a
 *  Note doesn't carry — saved bare, they go dead. Resolve them into a Sources
 *  footer at save time, one line per marker number, so the note names its
 *  evidence (and retrieval can find it by source title). */
function noteContentFrom(content: string, citations: Citation[], sources: Source[]): string {
  if (citations.length === 0) return content;
  const lines = citations.map((c, i) => {
    const url = sources.find((x) => x.id === c.sourceId)?.url || "";
    const tail = c.noteId ? " (note)" : isWebUrl(url) ? ` — ${url}` : "";
    return `[${i + 1}] ${c.sourceTitle}${tail}`;
  });
  return `${content}\n\n---\nSources:\n${lines.join("\n")}`;
}

/** `/grep` results as a chat message: a heading, then each hit as its source
 *  title + a shortened path:line locator and a fenced window. Tilde fences so
 *  a window containing ``` (markdown sources) can't break out of the block. */
function renderGrepMarkdown(pattern: string, hits: GrepHit[]): string {
  const head = `Grep \`${pattern}\` — ${hits.length} ${hits.length === 1 ? "match" : "matches"}`;
  if (hits.length === 0) {
    return `${head}\n\nNo matches in this notebook's repo- or folder-backed files.`;
  }
  const blocks = hits.map((h) => {
    const loc = `${h.path.split("/").slice(-3).join("/")}:${h.line}`;
    return `**${h.sourceTitle}** · \`${loc}\`\n\n~~~\n${h.window}\n~~~`;
  });
  return `${head}\n\n${blocks.join("\n\n")}`;
}

/** The composer's slash-command menu — a type-ahead chooser. Focus stays in the
 *  textarea (so typing keeps filtering), rows are selected via
 *  aria-activedescendant, and the ModelPill popover grammar carries the styling.
 *  Rows are grouped by family (Generate / Learning / Documents / Actions). */
/** The @ mention chooser — the slash picker's grammar with sources and notes
 *  as rows. Rows use onMouseDown+preventDefault so composer focus survives. */
function MentionPicker({
  results,
  selected,
  onHover,
  onPick,
}: {
  results: { id: string; kind: "source" | "note"; title: string }[];
  selected: number;
  onHover: (index: number) => void;
  onPick: (m: { id: string; kind: "source" | "note"; title: string }) => void;
}) {
  return (
    <div
      id="mention-picker"
      role="listbox"
      aria-label="Mention a source or note"
      className="menu-glass absolute bottom-full left-0 z-30 mb-1.5 max-h-72 w-[22rem] max-w-[calc(100vw-2.5rem)] overflow-y-auto rounded-md py-1"
    >
      {results.map((m, i) => (
        <Fragment key={`${m.kind}:${m.id}`}>
          {(i === 0 || results[i - 1].kind !== m.kind) && (
            <div className="px-2.5 pb-1 pt-1.5 text-micro font-semibold uppercase tracking-wide text-subtle-foreground">
              {m.kind === "source" ? "Sources" : "Notes"}
            </div>
          )}
          <button
            type="button"
            role="option"
            id={`mention-${m.kind}-${m.id}`}
            aria-selected={i === selected}
            onMouseMove={() => onHover(i)}
            onMouseDown={(e) => {
              e.preventDefault();
              onPick(m);
            }}
            className={cn(
              "flex w-full items-center gap-1.5 px-2.5 py-1.5 text-left text-[0.78125rem]",
              i === selected ? "bg-surface-2 text-foreground" : "text-foreground/85",
            )}
          >
            {m.kind === "source" ? (
              <FileText className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            ) : (
              <NotebookPen className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            )}
            <span className="truncate">{m.title}</span>
          </button>
        </Fragment>
      ))}
    </div>
  );
}

function SlashPicker({
  results,
  selected,
  onHover,
  onPick,
}: {
  results: SlashCommandMeta[];
  selected: number;
  onHover: (index: number) => void;
  onPick: (cmd: SlashCommandMeta) => void;
}) {
  return (
    <div
      id="slash-picker"
      role="listbox"
      aria-label="Slash commands"
      className="menu-glass absolute bottom-full left-0 z-30 mb-1.5 max-h-72 w-[22rem] max-w-[calc(100vw-2.5rem)] overflow-y-auto rounded-md py-1"
    >
      {results.length === 0 ? (
        <div className="px-2.5 py-2 text-caption text-muted-foreground">
          No matching commands — press ↩ to send as a message
        </div>
      ) : (
        results.map((c, i) => (
          <Fragment key={c.name}>
            {(i === 0 || results[i - 1].family !== c.family) && (
              <div className="px-2.5 pb-1 pt-1.5 text-micro font-semibold uppercase tracking-wide text-subtle-foreground">
                {c.family}
              </div>
            )}
            <button
              type="button"
              role="option"
              id={`slash-${c.name}`}
              aria-selected={i === selected}
              onMouseMove={() => onHover(i)}
              onMouseDown={(e) => {
                e.preventDefault(); // keep focus in the textarea
                onPick(c);
              }}
              className={cn(
                "flex w-full items-baseline gap-1.5 px-2.5 py-1.5 text-left text-[0.78125rem]",
                i === selected
                  ? "bg-surface-2 text-foreground"
                  : "text-foreground/85",
              )}
            >
              <span className="shrink-0 font-medium">/{c.name}</span>
              {c.argHint && (
                <span className="shrink-0 text-subtle-foreground">
                  {c.argHint}
                </span>
              )}
              <span className="min-w-0 flex-1 truncate text-muted-foreground">
                — {c.description}
              </span>
            </button>
          </Fragment>
        ))
      )}
      <div className="mx-2 my-1 h-px bg-border" />
      <div className="px-2.5 py-1 text-micro text-subtle-foreground">
        ⇥ complete · ↩ run · esc dismiss
      </div>
    </div>
  );
}

/** Hover row under a user turn: copy, re-run, and when it happened. Re-run is
 *  a rewind — it drops this question and everything after it, then resends —
 *  so it only appears on the last question; re-running an older one would
 *  silently discard the exchanges below it. */
function UserMessageActions({ message }: { message: Message }) {
  const [copied, setCopied] = useState(false);
  const isLastUser = useStore((s) => {
    for (let i = s.messages.length - 1; i >= 0; i--) {
      if (s.messages[i].role === "user") return s.messages[i].id === message.id;
    }
    return false;
  });
  const sending = useStore((s) => s.sending);

  async function copy() {
    try {
      await navigator.clipboard.writeText(message.content);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }
  async function rerun() {
    const st = useStore.getState();
    if (st.sending) return;
    const msgs = st.messages;
    const i = msgs.findIndex((m) => m.id === message.id);
    if (i < 0) return;
    for (const m of msgs.slice(i)) await api.deleteMessage(m.id);
    useStore.setState({ messages: msgs.slice(0, i) });
    void st.sendMessage(message.content);
  }

  // One literal, applied raw to both buttons: passing this through cn() would
  // run it past tailwind-merge, which reads the custom `text-micro` token as a
  // text *color*, and `text-muted-foreground` would then silently drop it.
  const btn =
    "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-micro text-muted-foreground hover:bg-surface-2 hover:text-foreground disabled:opacity-50";
  return (
    <div className="flex items-center gap-1 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
      <span
        className="px-1 text-micro text-subtle-foreground"
        title={fmtDateTime(message.createdAt)}
      >
        {relativeTime(message.createdAt)}
      </span>
      <button onClick={copy} className={btn} title="Copy to clipboard">
        {copied ? (
          <Check className="h-3.5 w-3.5 text-success" />
        ) : (
          <Copy className="h-3.5 w-3.5" />
        )}
        {copied ? "Copied" : "Copy"}
      </button>
      {isLastUser && (
        <button
          onClick={() => void rerun()}
          disabled={sending}
          className={btn}
          title="Ask this again — replaces the answer below"
        >
          <RefreshCw className="h-3.5 w-3.5" />
          Re-run
        </button>
      )}
    </div>
  );
}

function MessageActions({
  content,
  citations,
}: {
  content: string;
  citations: Citation[];
}) {
  const createNote = useStore((s) => s.createNote);
  const sources = useStore((s) => s.sources);
  const [copied, setCopied] = useState(false);
  const [saved, setSaved] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(content);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }
  async function save() {
    await createNote(noteTitleFrom(content), noteContentFrom(content, citations, sources));
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  }

  return (
    <div className="flex items-center gap-1 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100">
      <button
        onClick={copy}
        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-micro text-muted-foreground hover:bg-surface-2 hover:text-foreground"
        title="Copy to clipboard"
      >
        {copied ? <Check className="h-3.5 w-3.5 text-success" /> : <Copy className="h-3.5 w-3.5" />}
        {copied ? "Copied" : "Copy"}
      </button>
      <button
        onClick={save}
        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-micro text-muted-foreground hover:bg-surface-2 hover:text-foreground"
        title="Save this response as a note"
      >
        {saved ? <Check className="h-3.5 w-3.5 text-success" /> : <NotebookPen className="h-3.5 w-3.5" />}
        {saved ? "Saved" : "Save as note"}
      </button>
    </div>
  );
}

function RoleLabel({ role }: { role: "assistant" | "user" }) {
  return (
    <div className="flex items-center gap-1.5 text-micro font-medium text-muted-foreground">
      <Sparkles className="h-3 w-3 text-primary" />
      {role === "assistant" ? "Assistant" : "You"}
    </div>
  );
}

/** A note citation opens the note in Studio (same routing as ⌘K note hits);
 *  a source citation opens the source reader at the passage. */
function openCitationTarget(c: Citation) {
  const s = useStore.getState();
  if (c.noteId) {
    // StudioPanel auto-opens this id once the notebook's notes load.
    useStore.setState({ justCreatedNoteId: c.noteId });
    if (!s.studioOpen) s.toggleStudio();
  } else {
    s.openSourceViewer(c.sourceId, c.sourceTitle, c.snippet);
  }
}

function Citations({ citations }: { citations: Citation[] }) {
  const [open, setOpen] = useState(false);
  const sources = useStore((s) => s.sources);
  // Only web origins get the open-in-browser chip; file paths live in the
  // same field but belong to the source reader's "Show in Finder".
  const urlOf = (sourceId: string) => {
    const url = sources.find((x) => x.id === sourceId)?.url || "";
    return isWebUrl(url) ? url : "";
  };
  return (
    <div className="mt-1">
      <button
        className="inline-flex items-center gap-1.5 rounded-md border border-border bg-surface px-2 py-1 text-micro text-muted-foreground hover:text-foreground hover:border-border-strong transition-colors"
        onClick={() => setOpen((o) => !o)}
      >
        <Quote className="h-3 w-3" />
        {citations.length} {citations.length === 1 ? "citation" : "citations"}
      </button>
      {open && (
        <div className="mt-2 flex flex-col gap-2">
          {citations.map((c, i) => (
            <div
              key={c.chunkId}
              title={c.noteId ? "Open the note in Studio" : "Open in the source, highlighted"}
              className="relative cursor-pointer rounded-md border border-border bg-surface px-3 py-2 text-left transition-colors hover:border-border-strong hover:bg-surface-2"
            >
              <CardAction
                label={`${c.noteId ? "Open note" : "Open source"} ${c.sourceTitle}`}
                onClick={() => openCitationTarget(c)}
              />
              <div className="pointer-events-none relative z-10 mb-1 flex items-center gap-2 text-micro">
                <span className="flex h-4 min-w-4 items-center justify-center rounded bg-primary/15 px-1 font-semibold text-citation">
                  {i + 1}
                </span>
                <span className="font-medium text-foreground/90 truncate">{c.sourceTitle}</span>
                {c.noteId && (
                  <span
                    className="inline-flex shrink-0 items-center gap-1 rounded bg-surface-2 px-1.5 py-0.5 font-medium text-muted-foreground"
                    title="From a note — a saved conclusion, not a source document"
                  >
                    <StickyNote className="h-3 w-3" />
                    note
                  </span>
                )}
                {urlOf(c.sourceId) && (
                  <button
                    className="pointer-events-auto relative z-20 ml-auto shrink-0 rounded p-0.5 text-citation hover:underline"
                    title={`Open ${urlOf(c.sourceId)}`}
                    aria-label={`Open ${urlOf(c.sourceId)} in browser`}
                    onClick={(e) => {
                      e.stopPropagation();
                      void openUrl(urlOf(c.sourceId));
                    }}
                  >
                    <ExternalLink className="h-3.5 w-3.5" />
                  </button>
                )}
              </div>
              <p
                // Stays hit-testable so the text can be selected; plain
                // clicks (no selection) still open the citation target.
                className="pointer-events-auto relative z-10 line-clamp-4 text-caption leading-relaxed text-muted-foreground selectable"
                onClick={() => {
                  if (!window.getSelection()?.toString())
                    openCitationTarget(c);
                }}
              >
                {c.snippet}
              </p>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function StepTrail({
  steps,
  waiting,
  done,
}: {
  steps: string[];
  waiting: string;
  done: boolean;
}) {
  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border bg-surface/60 px-3 py-2">
      {steps.map((s, i) => {
        // The countdown, when there is one, is the thing still running — the
        // last completed step hands its spinner over to it.
        const isLast = i === steps.length - 1 && !waiting;
        const spinning = isLast && !done;
        return (
          <div key={i} className="flex items-center gap-2 text-caption">
            {spinning ? (
              <span
                className="h-2.5 w-2.5 shrink-0 rounded-full border-[1.5px] border-primary border-t-transparent animate-spin"
                aria-hidden
              />
            ) : (
              <Check className="h-3 w-3 shrink-0 text-success" />
            )}
            <span className={cn(spinning ? "text-foreground" : "text-muted-foreground")}>{s}</span>
          </div>
        );
      })}
      {waiting && !done && (
        <div className="flex items-center gap-2 text-caption" aria-live="polite">
          <span
            className="h-2.5 w-2.5 shrink-0 rounded-full border-[1.5px] border-primary border-t-transparent animate-spin"
            aria-hidden
          />
          <span className="text-muted-foreground">{waiting}</span>
        </div>
      )}
    </div>
  );
}

function ThinkingDots() {
  const theme = useStore((s) => s.theme);
  // One verb per thinking session, from the theme's set (see Theme.verbs).
  const [verb] = useState(() => {
    const verbs = THEMES[resolveThemeId(theme)]?.verbs ?? DEFAULT_VERBS;
    return verbs[Math.floor(Math.random() * verbs.length)];
  });
  return (
    <div className="flex items-center gap-2 py-1">
      <span className="text-caption text-muted-foreground">{verb}</span>
      <div className="flex items-center gap-1">
        {[0, 1, 2].map((i) => (
          <span
            key={i}
            className="h-1.5 w-1.5 rounded-full bg-muted-foreground"
            style={{ animation: "pulse-dot 1.2s ease-in-out infinite", animationDelay: `${i * 0.18}s` }}
          />
        ))}
      </div>
    </div>
  );
}

/**
 * The blank-state sigil. Full-size and vertically centered on an empty
 * notebook; `compact` (a summary exists) shrinks it to a small emblem at the
 * top of the chat column. Same element in both states, so the move animates.
 */
function ChatHero({
  hasNotebook,
  hasSources,
  compact,
}: {
  hasNotebook: boolean;
  hasSources: boolean;
  compact: boolean;
}) {
  const theme = useStore((s) => s.theme);
  // The sigil takes on the notebook's color — the transmutation circle is
  // this notebook's mark, not the app's.
  const notebookColor = useStore(
    (s) => s.notebooks.find((n) => n.id === s.currentId)?.color,
  );
  return (
    <div
      className={cn(
        "flex flex-col items-center gap-4 text-center transition-all duration-700",
        compact ? "pt-1" : "min-h-[62vh] justify-center",
      )}
    >
      <AlchemySymbol
        className={cn(
          "transition-all duration-700",
          notebookColor ? "opacity-85" : "text-citation/60",
          compact ? "h-9 w-9" : "h-16 w-16",
        )}
        style={notebookColor ? { color: notebookColor } : undefined}
        strokeWidth={notebookColor ? 1.5 : 1}
        preferred={THEMES[resolveThemeId(theme)]?.sigil}
      />
      {!compact && (
        <>
          <div className="text-section font-semibold text-foreground/90">
            {!hasNotebook
              ? "Create a notebook to begin"
              : !hasSources
                ? "Add sources to chat with citations"
                : "Ask anything about your sources"}
          </div>
          <RotatingQuote theme={theme} />
        </>
      )}
    </div>
  );
}

/** Chosen once per page load — module scope, so remounts don't reshuffle. */
const QUOTE = `“${FALLBACK_EPIGRAPHS[Math.floor(Math.random() * FALLBACK_EPIGRAPHS.length)]}”`;

function RotatingQuote({ theme }: { theme: string }) {
  // One epigraph system for the hero and this slot: the generated daily line
  // (mood-matched to the theme) when cached, else a curated fallback.
  const gen = generatedEpigraph(theme);
  return (
    <p className="max-w-[360px] animate-[quote-fade_0.8s_ease] text-body text-muted-foreground">
      {gen ? `“${gen}”` : QUOTE}
    </p>
  );
}


/** Provider kinds that are a vendor CLI (family B), where a blank model means
 *  "the CLI's own default" rather than "no model". Mirrors the Rust roster in
 *  `inference::AgentKind::id`. */
const AGENT_KINDS = new Set([
  "claude-code",
  "codex",
  "gemini-cli",
  "cursor-cli",
  "opencode",
  "copilot",
  "hermes",
  "bob-shell",
  "prime-agent",
  "pi",
]);
const isAgentProvider = (kind: string) => AGENT_KINDS.has(kind);

/** Composer model controls: provider, then that provider's model, then its
 *  reasoning effort. Three pills rather than one nested menu — a flyout inside
 *  a popover anchored above the composer has nowhere to go, and each list is
 *  short and flat on its own. The Effort pill is absent, not disabled, for a
 *  provider with no effort control (see `ProviderModels.efforts`). */
function ModelPill() {
  const aiConfig = useStore((s) => s.aiConfig);
  const saveAiConfig = useStore((s) => s.saveAiConfig);
  const openSettings = useStore((s) => s.openSettings);
  const [open, setOpen] = useState<null | "provider" | "model" | "effort">(null);
  const [ready, setReady] = useState<
    { id: string; ready: boolean; detail: string }[]
  >([]);
  // What the active provider offers. Fetched when a menu that needs it opens
  // (listing spawns a CLI), and re-fetched when the provider changes.
  const [offer, setOffer] = useState<ProviderModels | null>(null);

  const active = aiConfig?.providers.find((p) => p.id === aiConfig.chatProvider);
  const activeId = active?.id;

  useEffect(() => {
    if (open === "provider") void api.providerReadiness().then(setReady).catch(() => {});
  }, [open]);

  useEffect(() => {
    setOffer(null);
    if (!activeId) return;
    let live = true;
    void api
      .providerModels(activeId)
      .then((m) => live && setOffer(m))
      // A catalogue we couldn't fetch is an empty one: Default and Custom…
      // still work, so the pill is never a dead end.
      .catch(
        () =>
          live &&
          setOffer({
            models: [],
            supportsDefault: true,
            efforts: [],
            defaultModel: null,
          }),
      );
    return () => {
      live = false;
    };
  }, [activeId]);

  if (!aiConfig || !active) return null;

  /** Save a change to the active provider. `keepOpen` is for the effort
   *  slider, which commits as you drag — closing the menu under the pointer
   *  mid-drag would be its own bug. */
  function commit(next: Partial<ProviderEntry>, keepOpen = false) {
    if (!aiConfig || !active) return;
    const id = active.id;
    if (!keepOpen) setOpen(null);
    void saveAiConfig({
      ...aiConfig,
      chatProvider: id,
      providers: aiConfig.providers.map((p) =>
        p.id === id ? { ...p, ...next } : p,
      ),
    });
  }

  const efforts = offer?.efforts ?? [];

  return (
    <span className="inline-flex items-center gap-1">
      <MenuPill
        label={active.label}
        open={open === "provider"}
        onToggle={() => setOpen((o) => (o === "provider" ? null : "provider"))}
        onClose={() => setOpen(null)}
        title="Which provider answers this notebook"
        menuLabel="Answer with"
      >
        {aiConfig.providers.map((p) => {
          const r = ready.find((x) => x.id === p.id);
          const selectable = r ? r.ready : true;
          return (
            <MenuRow
              key={p.id}
              label={p.label}
              selected={aiConfig.chatProvider === p.id}
              disabled={!selectable}
              note={!selectable ? "unavailable" : undefined}
              autoFocus={p.id === aiConfig.chatProvider}
              onPick={() => {
                setOpen(null);
                void saveAiConfig({ ...aiConfig, chatProvider: p.id });
              }}
            />
          );
        })}
        <div className="mx-2 my-1 h-px bg-border" />
        <MenuRow
          label="Model settings…"
          muted
          onPick={() => {
            setOpen(null);
            openSettings("models");
          }}
        />
      </MenuPill>

      {active.kind !== "fm" && (
        <MenuPill
          // Naming the inherited model beats the bare word "Default", which
          // tells the user nothing about what will actually answer. Muted, so
          // "inherited" still reads differently from "chosen".
          label={active.chatModel || offer?.defaultModel || "Default"}
          muted={!active.chatModel}
          open={open === "model"}
          onToggle={() => setOpen((o) => (o === "model" ? null : "model"))}
          onClose={() => setOpen(null)}
          title={
            active.chatModel
              ? `Model for ${active.label}`
              : offer?.defaultModel
                ? `${active.label} default: ${offer.defaultModel}`
                : `Model for ${active.label} — using its own default`
          }
          menuLabel="Models"
        >
          {!offer ? (
            <div className="px-2.5 py-1.5 text-micro text-subtle-foreground">
              reading models…
            </div>
          ) : (
            <>
              {offer.supportsDefault && (
                <MenuRow
                  label="Default"
                  // Name the model when Default resolves to one we know;
                  // otherwise say whose default it is.
                  badge={
                    offer.defaultModel ??
                    (isAgentProvider(active.kind) ? "the CLI's own" : undefined)
                  }
                  selected={!active.chatModel}
                  onPick={() => commit({ chatModel: "" })}
                />
              )}
              {offer.models.map((m: string) => (
                <MenuRow
                  key={m}
                  label={m}
                  selected={active.chatModel === m}
                  onPick={() => commit({ chatModel: m })}
                />
              ))}
              <div className="mx-2 my-1 h-px bg-border" />
              <CustomModelRow
                current={active.chatModel}
                known={offer.models}
                onCommit={(m) => commit({ chatModel: m })}
              />
            </>
          )}
        </MenuPill>
      )}

      {efforts.length > 0 && (
        <MenuPill
          label={active.effort || "Default"}
          muted={!active.effort}
          open={open === "effort"}
          onToggle={() => setOpen((o) => (o === "effort" ? null : "effort"))}
          onClose={() => setOpen(null)}
          title={`Reasoning effort for ${active.label}`}
          menuLabel=""
          wide
        >
          <EffortSlider
            levels={efforts}
            value={active.effort}
            onPick={(e) => commit({ effort: e }, true)}
          />
        </MenuPill>
      )}
    </span>
  );
}

/** A composer pill that opens a popover menu above it. */
function MenuPill({
  label,
  muted,
  open,
  onToggle,
  onClose,
  title,
  menuLabel,
  wide,
  children,
}: {
  label: string;
  muted?: boolean;
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  title: string;
  menuLabel: string;
  wide?: boolean;
  children: React.ReactNode;
}) {
  return (
    <span className="relative">
      <button
        onClick={onToggle}
        aria-expanded={open}
        aria-haspopup="menu"
        title={title}
        className={cn(
          "inline-flex items-center gap-1 rounded-md border border-border bg-surface-2 px-2 py-1 text-micro transition-colors hover:text-foreground",
          muted ? "text-subtle-foreground" : "text-muted-foreground",
        )}
      >
        {label}
        <ChevronDown className="h-3 w-3" />
      </button>
      {open && (
        <>
          <button
            type="button"
            aria-label="Close menu"
            className="fixed inset-0 z-20 cursor-default"
            onClick={onClose}
          />
          <div
            role="menu"
            aria-label={menuLabel || title}
            className={cn(
              "menu-glass absolute bottom-full left-0 z-30 mb-1.5 max-h-[60vh] overflow-y-auto rounded-md py-1",
              wide ? "w-60 px-1" : "min-w-52",
            )}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.stopPropagation();
                onClose();
              }
            }}
          >
            {menuLabel && (
              <div className="px-2.5 py-1 text-micro text-subtle-foreground">
                {menuLabel}
              </div>
            )}
            {children}
          </div>
        </>
      )}
    </span>
  );
}

/** One row in a pill's menu. */
function MenuRow({
  label,
  badge,
  note,
  selected,
  disabled,
  muted,
  autoFocus,
  onPick,
}: {
  label: string;
  badge?: string;
  note?: string;
  selected?: boolean;
  disabled?: boolean;
  muted?: boolean;
  autoFocus?: boolean;
  onPick: () => void;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      autoFocus={autoFocus}
      disabled={disabled}
      onClick={onPick}
      className={cn(
        "flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-[0.78125rem]",
        disabled
          ? "cursor-default text-subtle-foreground"
          : muted
            ? "text-muted-foreground hover:bg-surface-2 hover:text-foreground"
            : "text-foreground hover:bg-surface-2",
      )}
    >
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {badge && (
        <span className="shrink-0 rounded bg-surface-2 px-1.5 py-0.5 text-micro text-subtle-foreground">
          {badge}
        </span>
      )}
      {selected ? (
        <Check className="h-3.5 w-3.5 shrink-0 text-citation" />
      ) : note ? (
        <span className="shrink-0 text-micro">{note}</span>
      ) : null}
    </button>
  );
}

/** A menu row that becomes a text field in place — the model name a provider
 *  will accept is its own vocabulary, so there is nothing to pick from. */
function CustomModelRow({
  current,
  known,
  onCommit,
}: {
  current: string;
  known: string[];
  onCommit: (model: string) => void;
}) {
  const isCustom = !!current && !known.includes(current);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(current);

  if (!editing) {
    return (
      <MenuRow
        label={isCustom ? current : "Custom…"}
        muted={!isCustom}
        selected={isCustom}
        onPick={() => {
          setDraft(current);
          setEditing(true);
        }}
      />
    );
  }
  return (
    <div className="px-2.5 py-1.5">
      <input
        autoFocus
        aria-label="Custom model name"
        value={draft}
        placeholder="model name"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") onCommit(draft.trim());
          if (e.key === "Escape") setEditing(false);
        }}
        onBlur={() => setEditing(false)}
        className="h-6 w-full rounded border border-input bg-surface-2 px-1.5 text-micro text-foreground focus:outline-none focus:border-border-strong"
      />
      <div className="pt-1 text-micro text-subtle-foreground">
        Enter to use · Esc to cancel
      </div>
    </div>
  );
}

/** Effort as a ladder rather than a list: the levels are ordered cheapest to
 *  most thorough, so the trade-off is the control. Stop 0 is the provider's
 *  own default — the state everything ships in.
 *
 *  Draggable, and the fill stops at the thumb: both dots and fill are
 *  positioned as a percentage of the track, inset by the thumb's radius so the
 *  end stops sit ON the ends rather than half off them. */
function EffortSlider({
  levels,
  value,
  onPick,
}: {
  levels: string[];
  value: string;
  onPick: (effort: string) => void;
}) {
  const stops = ["", ...levels];
  const track = useRef<HTMLDivElement | null>(null);
  const [dragging, setDragging] = useState(false);
  const at = Math.max(0, stops.indexOf(value));
  const pct = (i: number) => (i / (stops.length - 1)) * 100;

  /** Nearest stop to a pointer x, in track coordinates. */
  function stopAt(clientX: number) {
    const el = track.current;
    if (!el) return at;
    const r = el.getBoundingClientRect();
    const t = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    return Math.round(t * (stops.length - 1));
  }

  function pickAt(clientX: number) {
    const next = stopAt(clientX);
    if (next !== at) onPick(stops[next]);
  }

  return (
    <div className="flex flex-col gap-2 px-2 py-1.5">
      <div className="flex items-baseline gap-2">
        <span className="text-caption text-subtle-foreground">Effort</span>
        <span className="text-[0.78125rem] capitalize text-foreground">
          {value || "Default"}
        </span>
      </div>
      <div className="flex items-center justify-between text-micro text-subtle-foreground">
        <span>Faster</span>
        <span>Smarter</span>
      </div>
      <div
        role="slider"
        aria-label="Reasoning effort"
        aria-valuemin={0}
        aria-valuemax={stops.length - 1}
        aria-valuenow={at}
        aria-valuetext={value || "Default"}
        tabIndex={0}
        onKeyDown={(e) => {
          const delta =
            e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
          if (!delta) return;
          e.preventDefault();
          const next = Math.min(stops.length - 1, Math.max(0, at + delta));
          if (next !== at) onPick(stops[next]);
        }}
        onPointerDown={(e) => {
          e.preventDefault();
          e.currentTarget.setPointerCapture(e.pointerId);
          setDragging(true);
          pickAt(e.clientX);
        }}
        onPointerMove={(e) => dragging && pickAt(e.clientX)}
        onPointerUp={(e) => {
          e.currentTarget.releasePointerCapture(e.pointerId);
          setDragging(false);
        }}
        onPointerCancel={() => setDragging(false)}
        className="relative h-6 cursor-ew-resize select-none rounded focus:outline-none focus-visible:ring-1 focus-visible:ring-primary"
      >
        {/* Inset by the thumb's radius so stop 0 and the last stop sit fully
            on the track instead of hanging off its ends. */}
        <div ref={track} className="absolute inset-y-0 left-[7px] right-[7px]">
          <div className="absolute top-1/2 h-px w-full -translate-y-1/2 bg-border" />
          <div
            className="absolute top-1/2 h-px -translate-y-1/2 bg-primary"
            style={{ width: `${pct(at)}%` }}
          />
          {stops.map((s, i) => (
            <span
              key={s || "default"}
              title={s || "Default"}
              style={{ left: `${pct(i)}%` }}
              className={cn(
                "absolute top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-full transition-[height,width]",
                i === at
                  ? "h-3.5 w-3.5 bg-foreground"
                  : i < at
                    ? "h-1.5 w-1.5 bg-primary"
                    : "h-1.5 w-1.5 bg-border",
              )}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
