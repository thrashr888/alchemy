import { useEffect, useState } from "react";
import { ChevronRight, RefreshCw } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "./ui";

type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

/** Parse text that is a JSON document (object or array), else null. Cheap
 *  rejection first: most sources are prose and never reach JSON.parse. */
export function parseJsonDoc(text: string | null): Json | null {
  if (!text) return null;
  const t = text.trimStart();
  if (t[0] !== "{" && t[0] !== "[") return null;
  try {
    return JSON.parse(text) as Json;
  } catch {
    return null;
  }
}

/** Long arrays and objects show this many children, then a "more" row —
 *  SEC submissions carry columns a thousand entries deep. */
const PAGE = 50;

function Scalar({ value }: { value: Json }) {
  if (value === null) return <span className="text-subtle-foreground">null</span>;
  if (typeof value === "string")
    return (
      <span className="text-foreground [overflow-wrap:anywhere]">
        {JSON.stringify(value)}
      </span>
    );
  return <span className="text-primary">{String(value)}</span>;
}

function Node({
  name,
  value,
  depth,
}: {
  name: string | null;
  value: Json;
  depth: number;
}) {
  const container = value !== null && typeof value === "object";
  const [open, setOpen] = useState(depth < 2);
  const [shown, setShown] = useState(PAGE);
  const label =
    name === null ? null : <span className="text-muted-foreground">{name}: </span>;
  if (!container) {
    return (
      <div className="pl-4">
        {label}
        <Scalar value={value} />
      </div>
    );
  }
  const entries: [string, Json][] = Array.isArray(value)
    ? value.map((v, i) => [String(i), v])
    : Object.entries(value);
  const summary = Array.isArray(value)
    ? `[${entries.length}]`
    : `{${entries.length}}`;
  return (
    <div>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-0.5 text-left hover:text-foreground"
        aria-expanded={open}
      >
        <ChevronRight
          className={cn(
            "h-3 w-3 shrink-0 text-subtle-foreground transition-transform",
            open && "rotate-90",
          )}
        />
        {label}
        <span className="text-subtle-foreground">{summary}</span>
      </button>
      {open && (
        <div className="ml-1.5 border-l border-border pl-1">
          {entries.slice(0, shown).map(([k, v]) => (
            <Node key={k} name={k} value={v} depth={depth + 1} />
          ))}
          {entries.length > shown && (
            <button
              type="button"
              onClick={() => setShown((n) => n + PAGE * 4)}
              className="pl-4 text-subtle-foreground hover:text-foreground"
            >
              Show {Math.min(PAGE * 4, entries.length - shown)} more of{" "}
              {entries.length - shown}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** A JSON document as a collapsible tree: two levels open, the rest a
 *  click away, long lists paged. */
export function JsonTree({ value }: { value: Json }) {
  return (
    <div className="selectable font-mono text-caption leading-relaxed">
      <Node name={null} value={value} depth={0} />
    </div>
  );
}

/** The Live view for a JSON source: the document as the server has it now,
 *  fetched through the app (no CORS, no raw one-line webview). */
export function LiveJson({ url }: { url: string }) {
  const [state, setState] = useState<
    | { kind: "loading" }
    | { kind: "ok"; value: Json; at: Date }
    | { kind: "error"; message: string }
  >({ kind: "loading" });
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let on = true;
    setState({ kind: "loading" });
    api
      .fetchJsonLive(url)
      .then((text) => {
        const value = parseJsonDoc(text);
        if (!on) return;
        setState(
          value === null
            ? { kind: "error", message: "The response was not JSON." }
            : { kind: "ok", value, at: new Date() },
        );
      })
      .catch((err: unknown) => {
        if (on) setState({ kind: "error", message: String(err) });
      });
    return () => {
      on = false;
    };
  }, [url, tick]);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex min-w-0 items-center gap-2 border-b border-border px-5 py-2">
        <span
          className="min-w-0 flex-1 truncate text-micro text-subtle-foreground"
          title={url}
        >
          {url}
        </span>
        {state.kind === "ok" && (
          <span className="shrink-0 text-micro tabular-nums text-subtle-foreground">
            Fetched {state.at.toLocaleTimeString()}
          </span>
        )}
        <Button
          variant="ghost"
          size="icon"
          onClick={() => setTick((t) => t + 1)}
          title="Fetch again"
          aria-label="Fetch again"
          disabled={state.kind === "loading"}
        >
          <RefreshCw
            className={cn("h-3.5 w-3.5", state.kind === "loading" && "animate-spin")}
          />
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto px-5 py-4">
        {state.kind === "loading" && (
          <p role="status" className="text-caption text-muted-foreground">
            Fetching…
          </p>
        )}
        {state.kind === "error" && (
          <p className="text-caption text-destructive/80 [overflow-wrap:anywhere]">
            {state.message}
          </p>
        )}
        {state.kind === "ok" && <JsonTree value={state.value} />}
      </div>
    </div>
  );
}
