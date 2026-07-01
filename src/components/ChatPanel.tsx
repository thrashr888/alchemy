import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useStore } from "@/lib/store";
import { Button, Textarea } from "./ui";
import { Markdown } from "./Markdown";
import { cn } from "@/lib/utils";
import { DitherBackground } from "./DitherBackground";
import type { Citation, Message } from "@/lib/types";
import {
  ArrowUp,
  Eraser,
  Quote,
  Sparkles,
  MessageSquare,
  Telescope,
  Check,
  Copy,
  NotebookPen,
} from "lucide-react";

export function ChatPanel() {
  const currentId = useStore((s) => s.currentId);
  const messages = useStore((s) => s.messages);
  const sources = useStore((s) => s.sources);
  const sending = useStore((s) => s.sending);
  const streamingText = useStore((s) => s.streamingText);
  const steps = useStore((s) => s.steps);
  const agentMode = useStore((s) => s.agentMode);
  const toggleAgentMode = useStore((s) => s.toggleAgentMode);
  const send = useStore((s) => s.sendMessage);
  const clearChat = useStore((s) => s.clearChat);
  const appendToken = useStore((s) => s.appendToken);
  const appendStep = useStore((s) => s.appendStep);

  const [draft, setDraft] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);

  // Subscribe once to streaming tokens + agent progress steps from the backend.
  useEffect(() => {
    const unToken = listen<{ content: string }>("chat://token", (e) => {
      appendToken(e.payload.content);
    });
    const unStep = listen<{ label: string }>("chat://step", (e) => {
      appendStep(e.payload.label);
    });
    return () => {
      unToken.then((fn) => fn());
      unStep.then((fn) => fn());
    };
  }, [appendToken, appendStep]);

  // Autoscroll on new content.
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, streamingText, steps]);

  const canChat = !!currentId && sources.length > 0;

  function submit() {
    const text = draft.trim();
    if (!text || sending || !canChat) return;
    setDraft("");
    void send(text);
  }

  return (
    <div className="flex h-full flex-1 flex-col bg-background min-w-0">
      <div className="flex items-center px-5 h-12 border-b border-border">
        <MessageSquare className="h-4 w-4 text-muted-foreground" />
        <span className="ml-2 text-[13px] font-semibold">Chat</span>
        {messages.length > 0 && (
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto"
            onClick={() => confirm("Clear this conversation?") && clearChat()}
          >
            <Eraser className="h-3.5 w-3.5" />
            Clear
          </Button>
        )}
      </div>

      <div ref={scrollRef} className="flex-1 overflow-y-auto">
        <div className="mx-auto flex max-w-[720px] flex-col gap-6 px-5 py-6">
          {messages.length === 0 && !sending ? (
            <ChatEmpty hasNotebook={!!currentId} hasSources={sources.length > 0} />
          ) : (
            messages.map((m) => <ChatMessage key={m.id} message={m} />)
          )}

          {sending && (
            <div className="flex flex-col gap-2">
              <RoleLabel role="assistant" />
              {steps.length > 0 && <StepTrail steps={steps} done={!!streamingText} />}
              {streamingText ? (
                <Markdown>{streamingText}</Markdown>
              ) : (
                steps.length === 0 && <ThinkingDots />
              )}
            </div>
          )}
        </div>
      </div>

      <div className="px-5 pb-5 pt-2">
        <div className="mx-auto max-w-[720px]">
          <div
            className={cn(
              "rounded-lg border border-border-strong bg-surface p-2.5 shadow-md transition-colors",
              "focus-within:border-ring/60",
            )}
          >
            <Textarea
              rows={1}
              className="border-0 bg-transparent focus:ring-0 min-h-[24px] max-h-[180px] px-1.5 py-1"
              placeholder={
                canChat
                  ? "Ask anything about your sources…"
                  : currentId
                    ? "Add a source to start chatting"
                    : "Select or create a notebook"
              }
              value={draft}
              disabled={!canChat || sending}
              onChange={(e) => {
                setDraft(e.target.value);
                e.target.style.height = "auto";
                e.target.style.height = `${Math.min(e.target.scrollHeight, 180)}px`;
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  submit();
                }
              }}
            />
            <div className="flex items-center justify-between px-1.5 pt-1">
              <button
                onClick={toggleAgentMode}
                title="Agentic mode: the model plans multiple searches over your sources before answering"
                className={cn(
                  "inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-[11px] transition-colors",
                  agentMode
                    ? "border-primary/50 bg-primary/15 text-citation"
                    : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
                )}
              >
                <Telescope className="h-3 w-3" />
                {agentMode ? "Deep research: on" : "Deep research: off"}
              </button>
              <Button
                variant="primary"
                size="icon"
                onClick={submit}
                disabled={!draft.trim() || sending || !canChat}
                title="Send"
              >
                <ArrowUp className="h-4 w-4" />
              </Button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function ChatMessage({ message }: { message: Message }) {
  if (message.role === "user") {
    return (
      <div className="flex flex-col items-end gap-1">
        <div className="max-w-[85%] rounded-lg rounded-br-sm bg-surface-2 px-3.5 py-2 text-[13.5px] selectable border border-border">
          {message.content}
        </div>
      </div>
    );
  }
  return (
    <div className="group flex flex-col gap-2">
      <RoleLabel role="assistant" />
      <Markdown>{message.content}</Markdown>
      {message.citations.length > 0 && <Citations citations={message.citations} />}
      <MessageActions content={message.content} />
    </div>
  );
}

function noteTitleFrom(content: string): string {
  const line = content.split("\n").map((l) => l.trim()).find(Boolean) ?? "";
  const clean = line.replace(/^#+\s*/, "").replace(/[*_`>#]/g, "").trim();
  return clean.slice(0, 60) || "Chat response";
}

function MessageActions({ content }: { content: string }) {
  const createNote = useStore((s) => s.createNote);
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
    await createNote(noteTitleFrom(content), content);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  }

  return (
    <div className="flex items-center gap-1 opacity-0 transition group-hover:opacity-100">
      <button
        onClick={copy}
        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-surface-2 hover:text-foreground"
        title="Copy to clipboard"
      >
        {copied ? <Check className="h-3 w-3 text-success" /> : <Copy className="h-3 w-3" />}
        {copied ? "Copied" : "Copy"}
      </button>
      <button
        onClick={save}
        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-surface-2 hover:text-foreground"
        title="Save this response as a note"
      >
        {saved ? <Check className="h-3 w-3 text-success" /> : <NotebookPen className="h-3 w-3" />}
        {saved ? "Saved" : "Save as note"}
      </button>
    </div>
  );
}

function RoleLabel({ role }: { role: "assistant" | "user" }) {
  return (
    <div className="flex items-center gap-1.5 text-[11.5px] font-medium text-muted-foreground">
      <Sparkles className="h-3 w-3 text-primary" />
      {role === "assistant" ? "Assistant" : "You"}
    </div>
  );
}

function Citations({ citations }: { citations: Citation[] }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="mt-1">
      <button
        className="inline-flex items-center gap-1.5 rounded-md border border-border bg-surface px-2 py-1 text-[11px] text-muted-foreground hover:text-foreground hover:border-border-strong transition-colors"
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
              className="rounded-md border border-border bg-surface px-3 py-2"
            >
              <div className="mb-1 flex items-center gap-2 text-[11px]">
                <span className="flex h-4 min-w-4 items-center justify-center rounded bg-primary/15 px-1 font-semibold text-citation">
                  {i + 1}
                </span>
                <span className="font-medium text-foreground/90 truncate">{c.sourceTitle}</span>
                <span className="ml-auto text-subtle-foreground">
                  {(1 - c.distance).toFixed(2)}
                </span>
              </div>
              <p className="text-[12px] leading-relaxed text-muted-foreground line-clamp-4 selectable">
                {c.snippet}
              </p>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function StepTrail({ steps, done }: { steps: string[]; done: boolean }) {
  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border bg-surface/60 px-3 py-2">
      {steps.map((s, i) => {
        const isLast = i === steps.length - 1;
        const spinning = isLast && !done;
        return (
          <div key={i} className="flex items-center gap-2 text-[12px]">
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
    </div>
  );
}

function ThinkingDots() {
  return (
    <div className="flex items-center gap-1 py-1">
      {[0, 1, 2].map((i) => (
        <span
          key={i}
          className="h-1.5 w-1.5 rounded-full bg-muted-foreground"
          style={{ animation: "pulse-dot 1.2s ease-in-out infinite", animationDelay: `${i * 0.18}s` }}
        />
      ))}
    </div>
  );
}

function ChatEmpty({ hasNotebook, hasSources }: { hasNotebook: boolean; hasSources: boolean }) {
  const theme = useStore((s) => s.theme);
  return (
    <div className="relative flex min-h-[440px] flex-col items-center justify-center gap-3 overflow-hidden py-24 text-center">
      <div className="absolute inset-0">
        <DitherBackground themeKey={theme} />
      </div>
      <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_center,transparent_35%,var(--background)_92%)]" />
      <div className="relative z-10 flex h-12 w-12 items-center justify-center rounded-lg bg-primary/15 text-primary">
        <Sparkles className="h-6 w-6" />
      </div>
      <div className="relative z-10 text-[15px] font-semibold">
        {!hasNotebook
          ? "Create a notebook to begin"
          : !hasSources
            ? "Add sources to start a grounded chat"
            : "Ask anything about your sources"}
      </div>
      <p className="relative z-10 max-w-[360px] text-[13px] text-muted-foreground">
        Answers are generated locally with Ollama and cite the exact passages they draw from.
      </p>
    </div>
  );
}
