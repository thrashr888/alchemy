import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import { api } from "@/lib/api";
import type { ActivityItem } from "@/lib/types";
import { useHoverCard } from "./ui";

/**
 * The one place the app says a model is working — top right of the title
 * bar, in every window and on every screen.
 *
 * It replaces a glyph that sat on the answering chat row: too small a place
 * for something that isn't only about chat. Scheduled reports, queued
 * generations, background sweeps and indexing all spend the machine, and
 * before this they spent it invisibly. One indicator, everything in flight.
 *
 * Shape, not color (DESIGN.md): Ollama gets three uneven uprights — a meter,
 * a machine of yours doing work — and Apple Foundation Models a sweeping
 * ring, the system's own. Both are drawn at the 14px icon floor, both hold
 * up as a still frame, and a still frame is what `prefers-reduced-motion`
 * leaves behind. Idle draws nothing at all.
 *
 * The glyph alone said "a model is working" but never which one, and on a
 * Mac that answers from two engines that is the interesting half. Each
 * glyph now carries its provider's name in the title bar's own caption
 * gray — still no color, still nothing at all when nothing is running.
 */

/** Provider family (see inference/activity.rs) to the word for it. */
const PROVIDER: Record<string, string> = {
  ollama: "Ollama",
  fm: "Apple",
  gateway: "Gateway",
  "agent-cli": "Agent",
  builtin: "Built-in",
};

const providerName = (kind: string) => PROVIDER[kind] ?? "Model";

export function InferenceActivity() {
  const [items, setItems] = useState<ActivityItem[]>([]);
  const { show, hide, card } = useHoverCard("left");

  useEffect(() => {
    void api
      .inferenceActivity()
      .then(setItems)
      .catch(() => undefined);
    const off = listen<ActivityItem[]>("inference://activity", (e) =>
      setItems(e.payload),
    );
    return () => {
      void off.then((f) => f());
    };
  }, []);

  if (items.length === 0) return null;

  // One glyph per engine family in flight, never one per call: eight parallel
  // embed calls are one machine working, not eight. Anything we didn't draw
  // a glyph for (a gateway, an agent CLI) borrows the meter — it reads as
  // "busy", which is what it is — but keeps its own name.
  const kinds = [...new Set(items.map((i) => i.kind))];
  const names = kinds.map(providerName);
  // The model earns a place in the card's title only when there is one
  // model to name; otherwise the rows below carry them.
  const models = [...new Set(items.map((i) => i.model).filter(Boolean))];
  const title = [
    names.join(" · "),
    models.length === 1 ? models[0] : null,
    items.length > 1 ? String(items.length) : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <>
      <span
        className="flex items-center gap-1.5 text-muted-foreground"
        aria-label={
          items.length === 1
            ? `${names[0]} is running ${items[0].label || "a model"}`
            : `${items.length} model calls running on ${names.join(" and ")}`
        }
        onMouseEnter={(e) =>
          show(e, {
            title,
            meta: items.map((i) => ({
              label: i.label || "Working",
              value: i.model || providerName(i.kind),
            })),
          })
        }
        onMouseLeave={hide}
      >
        {kinds.map((kind) => (
          <span key={kind} className="flex items-center gap-1">
            <ProviderGlyph kind={kind} />
            <span className="text-caption">{providerName(kind)}</span>
          </span>
        ))}
      </span>
      {card}
    </>
  );
}

function ProviderGlyph({ kind }: { kind: string }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 12 12"
      className="h-3.5 w-3.5"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.25"
      strokeLinecap="round"
    >
      {kind === "fm" ? (
        <>
          <circle cx="6" cy="6" r="4" opacity="0.3" />
          <path d="M6 2a4 4 0 0 1 4 4" className="provider-arc" />
        </>
      ) : (
        ["M3 8.5V5", "M6 9.5V2.5", "M9 8V4"].map((d, i) => (
          <path
            key={d}
            d={d}
            className="provider-bar"
            style={{ animationDelay: `${i * 0.16}s` }}
          />
        ))
      )}
    </svg>
  );
}
