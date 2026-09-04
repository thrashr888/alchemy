/* The size strip's hover card: what the notebook is made of, by type
   (alchemy-release-2j9).

   Same pattern as the row hover cards in ui.tsx -- a beat-delayed,
   pointer-events-none portal on the overlay layer -- but the body is a
   segmented bar and a legend rather than label/value rows, so it carries its
   own hook instead of reshaping `useHoverCard` for one caller.

   No color. The app has no per-type tints (source row icons are all
   `muted-foreground`), and DESIGN.md spends color only where it means
   something, so segments are one neutral fill stepped by rank and split by
   hairline gaps. The legend carries the meaning. */
import * as React from "react";
import { createPortal } from "react-dom";
import {
  formatShare,
  sourceBreakdown,
  type BreakdownSlice,
} from "@/lib/sourceBreakdown";
import type { Source } from "@/lib/types";
import { cn, compactNumber } from "@/lib/utils";

const CARD_W = 288;
const CARD_H_EST = 168;

/** Rank ramp: the biggest slice reads strongest. Bounded well above the
 *  hairline gaps so the smallest segment is still a segment, not a smudge. */
function segmentOpacity(i: number, n: number): number {
  if (n <= 1) return 0.85;
  return 0.85 - (i / (n - 1)) * 0.55;
}

function Bar({ slices }: { slices: BreakdownSlice[] }) {
  return (
    <div className="flex h-2 w-full gap-px overflow-hidden rounded-full bg-surface-2">
      {slices.map((s, i) => (
        <div
          key={s.key}
          className="h-full bg-foreground first:rounded-l-full last:rounded-r-full"
          style={{
            width: `${Math.max(1.5, s.share)}%`,
            opacity: segmentOpacity(i, slices.length),
          }}
        />
      ))}
    </div>
  );
}

function Legend({ slices }: { slices: BreakdownSlice[] }) {
  return (
    <div className="mt-2.5 grid grid-cols-2 gap-x-3 gap-y-1">
      {slices.map((s, i) => (
        <div
          key={s.key}
          className="flex min-w-0 items-center gap-1.5 text-caption"
          title={`${s.label}: ${s.count} ${
            s.count === 1 ? "source" : "sources"
          }, ${compactNumber(s.chars)} chars`}
        >
          <span
            aria-hidden
            className="h-1.5 w-1.5 shrink-0 rounded-full bg-foreground"
            style={{ opacity: segmentOpacity(i, slices.length) }}
          />
          <span className="min-w-0 truncate text-muted-foreground">
            {s.label}
          </span>
          <span className="ml-auto shrink-0 tabular-nums text-subtle-foreground">
            {formatShare(s.share)}
          </span>
        </div>
      ))}
    </div>
  );
}

/** One hook per strip. `open`/`close` go on the strip's pointer AND focus
 *  handlers: a keyboard user tabbing to the strip gets the same card, without
 *  the warm-up delay a mouse needs. */
export function useSourceBreakdownCard(sources: readonly Source[]) {
  const [at, setAt] = React.useState<{ top: number; left: number } | null>(
    null,
  );
  const timer = React.useRef<number | undefined>(undefined);

  const place = (el: Element) => {
    const r = el.getBoundingClientRect();
    setAt({
      top: Math.min(r.bottom + 8, window.innerHeight - CARD_H_EST),
      left: Math.min(r.left, window.innerWidth - CARD_W - 8),
    });
  };
  const open = (e: React.MouseEvent<Element> | React.FocusEvent<Element>) => {
    const el = e.currentTarget;
    window.clearTimeout(timer.current);
    // Focus is deliberate; hover is not. Only the pointer waits a beat.
    if (e.type === "focus") place(el);
    else timer.current = window.setTimeout(() => place(el), 450);
  };
  const close = () => {
    window.clearTimeout(timer.current);
    setAt(null);
  };
  React.useEffect(() => () => window.clearTimeout(timer.current), []);
  // Scrolling under the cursor never fires mouseleave; drop the card rather
  // than let it drift away from the strip it describes.
  React.useEffect(() => {
    if (!at) return;
    const drop = () => setAt(null);
    window.addEventListener("scroll", drop, true);
    return () => window.removeEventListener("scroll", drop, true);
  }, [at]);

  const slices = React.useMemo(() => sourceBreakdown(sources), [sources]);

  const card =
    at && slices.length > 0
      ? createPortal(
          <div
            // The reader's live web view is a native child webview and paints
            // above every HTML layer; `data-overlay` is its cue to hide.
            data-overlay=""
            role="presentation"
            className={cn(
              "pointer-events-none fixed z-[100] rounded-lg border border-border-strong",
              // The global prefers-reduced-motion guard in index.css flattens
              // this fade to nothing; no per-component check needed.
              "bg-surface-2 p-3 shadow-2xl animate-in fade-in duration-150",
            )}
            style={{ top: Math.max(8, at.top), left: Math.max(8, at.left), width: CARD_W }}
          >
            <div className="mb-2 flex items-baseline gap-2">
              <span className="text-body font-medium text-foreground">
                Sources by type
              </span>
              <span className="ml-auto shrink-0 text-micro text-subtle-foreground">
                {sources.length}
              </span>
            </div>
            <Bar slices={slices} />
            <Legend slices={slices} />
          </div>,
          document.body,
        )
      : null;

  /** Spread onto the strip: opens on hover and on keyboard focus. */
  const triggerProps = {
    tabIndex: 0,
    onMouseEnter: open,
    onMouseLeave: close,
    onFocus: open,
    onBlur: close,
    "aria-label": `Notebook composition: ${slices
      .map((s) => `${s.label} ${formatShare(s.share)}`)
      .join(", ")}`,
  };

  return { card, triggerProps };
}
