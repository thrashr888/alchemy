import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import { api } from "@/lib/api";
import type { ActivityItem } from "@/lib/types";
import { cn } from "@/lib/utils";
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
 * leaves behind. Idle draws nothing at all — but it still occupies its slot,
 * so starting a model never shoves the rest of the toolbar sideways.
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

/** At most this many glyphs are drawn. The card and the accessible name
 *  still count every call; the strip is a status light, not a census, and
 *  its width has to be a constant the toolbar can reserve. */
const MAX_GLYPHS = 3;
/** 14px glyphs, 6px apart (`gap-1.5`): 3 × 14 + 2 × 6. The slot is always
 *  this wide, running or idle. */
const SLOT_PX = MAX_GLYPHS * 14 + (MAX_GLYPHS - 1) * 6;
/** Matches `duration-200` below: how long the glyphs stay mounted after the
 *  last call ends, so they fade instead of blinking out. */
const FADE_MS = 200;

export function InferenceActivity() {
  const [items, setItems] = useState<ActivityItem[]>([]);
  // The glyphs outlive the work by one fade. Unmounting them on the same
  // tick would end the fade before it drew, and keeping them forever would
  // keep a stale hover card alive (see below).
  const [lingering, setLingering] = useState<ActivityItem[]>([]);

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

  useEffect(() => {
    if (items.length > 0) {
      setLingering(items);
      return;
    }
    const t = setTimeout(() => setLingering([]), FADE_MS);
    return () => clearTimeout(t);
  }, [items]);

  // A fixed slot, not a mount: this sat between the centered mode tabs and
  // the DEV pill, so every model that started or finished shoved the search
  // field, the toggles and the tabs sideways. The box is always here and
  // always this wide; only its opacity moves. Hidden it is out of the
  // accessibility tree and out of the focus order — `visibility: hidden`,
  // not just transparent.
  const running = items.length > 0;
  return (
    <div
      style={{ width: SLOT_PX }}
      aria-hidden={running ? undefined : true}
      className={cn(
        "flex h-6 shrink-0 items-center justify-end transition-opacity duration-200",
        !running && "pointer-events-none opacity-0",
        lingering.length === 0 && "invisible",
      )}
    >
      {lingering.length > 0 && <ActiveInferenceActivity items={lingering} />}
    </div>
  );
}

// Mount hover state only while activity exists. Returning null from the same
// component preserves a stale card and its reveal timer across idle periods.
function ActiveInferenceActivity({ items }: { items: ActivityItem[] }) {
  const { show, hide, update, card } = useHoverCard("left");

  // One glyph per engine family in flight, never one per call: eight parallel
  // embed calls are one machine working, not eight. Anything we didn't draw
  // a glyph for (a gateway, an agent CLI) borrows the meter — it reads as
  // "busy", which is what it is — but keeps its own name. Three glyphs is
  // where the strip stops drawing; past that the name and the card carry the
  // rest, and the slot keeps a width the toolbar can hold open.
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

  const details = {
    title,
    layout: "stacked" as const,
    meta: items.map((i) => ({
      label: i.label || "Working",
      value: i.model || providerName(i.kind),
    })),
  };
  // The pointer can rest on the glyph through several calls; the card
  // follows the list instead of freezing on whatever was running at entry.
  useEffect(() => {
    update(details);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items]);

  return (
    <>
      <span
        className="flex items-center gap-1.5 text-muted-foreground"
        aria-label={
          items.length === 1
            ? `${names[0]} is running ${items[0].label || "a model"}`
            : `${items.length} model calls running on ${names.join(" and ")}`
        }
        tabIndex={0}
        onMouseEnter={(e) => show(e, details)}
        onMouseLeave={hide}
        onFocus={(e) => show(e, details)}
        onBlur={hide}
        onKeyDown={(e) => {
          if (e.key === "Escape") hide();
        }}
      >
        {kinds.slice(0, MAX_GLYPHS).map((kind) => (
          <span key={kind} className="flex items-center">
            <ProviderGlyph kind={kind} />
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
