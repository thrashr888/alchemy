import { useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronLeft, ChevronRight, FileDown, RotateCcw, X } from "lucide-react";
import { Markdown } from "./Markdown";
import { PrintPortal, usePrintExport } from "./printExport";
import { cn } from "@/lib/utils";

/**
 * Native flashcard renderer with Leitner-style spaced repetition. The
 * generator emits `**Front:** / **Back:**` pairs separated by `---` lines
 * (see rag::artifact_spec); we parse that and do the flipping here. Falls
 * back to Markdown when the content doesn't parse, so a deck never arrives
 * broken.
 *
 * Review state is a Leitner box per card (0-4, intervals below) persisted in
 * localStorage keyed by note id + a hash of the card front — regenerating a
 * deck keeps the schedule of unchanged cards. Grading is pass/fail ("Missed
 * it" resets to box 0 / due now; "Got it" moves up a box), which is the part
 * of spaced repetition that carries the effect: active recall, self-grading,
 * and increasing intervals for known material.
 */

export interface Card {
  front: string;
  back: string;
}

/** Parse the flashcard spec; null when it isn't a usable deck. */
export function parseCards(md: string): Card[] | null {
  const cards: Card[] = [];
  for (const block of md.split(/^\s*-{3,}\s*$/m)) {
    const m = /\*\*Front:\*\*\s*([\s\S]*?)\*\*Back:\*\*\s*([\s\S]*)/.exec(block);
    if (!m) continue;
    const front = m[1].trim();
    const back = m[2].trim();
    if (front && back) cards.push({ front, back });
  }
  return cards.length >= 2 ? cards : null;
}

/** Review intervals per Leitner box, in days. Box 0 is "due immediately". */
const BOX_DAYS = [0, 1, 3, 7, 21];
const DAY_MS = 24 * 60 * 60 * 1000;

interface Review {
  box: number;
  due: number;
}

/** Stable per-card key: djb2 over the front, so edits to a back keep state. */
function cardKey(front: string): string {
  let h = 5381;
  for (let i = 0; i < front.length; i++) h = ((h << 5) + h + front.charCodeAt(i)) | 0;
  return (h >>> 0).toString(36);
}

function loadReviews(noteId: string): Record<string, Review> {
  try {
    return JSON.parse(localStorage.getItem(`alchemy:flashcards:${noteId}`) ?? "{}");
  } catch {
    return {};
  }
}

function saveReviews(noteId: string, reviews: Record<string, Review>) {
  try {
    localStorage.setItem(`alchemy:flashcards:${noteId}`, JSON.stringify(reviews));
  } catch {
    // Storage full or unavailable — the session still works, it just won't persist.
  }
}

export function Flashcards({ content, noteId }: { content: string; noteId?: string }) {
  const cards = parseCards(content);
  if (!cards) return <Markdown>{content}</Markdown>;
  return <Deck cards={cards} noteId={noteId} />;
}

/** Cards as a printable study sheet — front bold, back beneath, no page
 *  breaks inside a card. */
function PrintCards({ cards }: { cards: Card[] }) {
  return (
    <PrintPortal pageCss="@page { size: auto; margin: 16mm; }">
      <div style={{ color: "#111", fontFamily: "system-ui, sans-serif", fontSize: 12 }}>
        {cards.map((card, i) => (
          <div key={i} className="print-card" style={{ padding: "10px 0", borderBottom: "1px solid #ccc" }}>
            <div style={{ fontWeight: 600, marginBottom: 4 }}>
              {i + 1}. {card.front}
            </div>
            <div>{card.back}</div>
          </div>
        ))}
      </div>
    </PrintPortal>
  );
}

function Deck({ cards, noteId }: { cards: Card[]; noteId?: string }) {
  const { printing, exportPdf } = usePrintExport({ suggestedName: "Flashcards" });
  const rootRef = useRef<HTMLDivElement>(null);
  const [reviews, setReviews] = useState<Record<string, Review>>(() =>
    noteId ? loadReviews(noteId) : {},
  );
  // Due-first study order, computed once per session so grading a card
  // doesn't reshuffle the deck under you.
  const order = useMemo(() => {
    const now = Date.now();
    const due = (c: Card) => (reviews[cardKey(c.front)]?.due ?? 0) <= now;
    return [...cards.filter(due), ...cards.filter((c) => !due(c))];
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cards]);
  const dueCount = useMemo(() => {
    const now = Date.now();
    return cards.filter((c) => (reviews[cardKey(c.front)]?.due ?? 0) <= now).length;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cards, reviews]);

  const [queue, setQueue] = useState<Card[]>(order);
  const [index, setIndex] = useState(0);
  const [flipped, setFlipped] = useState(false);
  const [missed, setMissed] = useState<Card[]>([]);
  const [gotIt, setGotIt] = useState(0);

  useEffect(() => {
    rootRef.current?.focus();
  }, []);

  const grade = (pass: boolean) => {
    const card = queue[index];
    if (!card) return;
    if (noteId) {
      const key = cardKey(card.front);
      const prior = reviews[key]?.box ?? 0;
      const box = pass ? Math.min(prior + 1, BOX_DAYS.length - 1) : 0;
      const next = {
        ...reviews,
        [key]: { box, due: Date.now() + BOX_DAYS[box] * DAY_MS },
      };
      setReviews(next);
      saveReviews(noteId, next);
    }
    if (pass) setGotIt((n) => n + 1);
    else setMissed((m) => [...m, card]);
    setIndex((i) => i + 1);
    setFlipped(false);
  };

  const restart = (subset?: Card[]) => {
    setQueue(subset ?? order);
    setIndex(0);
    setFlipped(false);
    setMissed([]);
    setGotIt(0);
  };

  const go = (dir: 1 | -1) => {
    setIndex((current) => {
      const next = current + dir;
      return next < 0 || next >= queue.length ? current : next;
    });
    setFlipped(false);
  };

  const card = queue[index];
  const done = index >= queue.length;

  return (
    <div
      ref={rootRef}
      tabIndex={0}
      className="flex flex-col gap-3 outline-none"
      onKeyDown={(e) => {
        if (done) return;
        if (e.key === "ArrowRight") go(1);
        else if (e.key === "ArrowLeft") go(-1);
        else if (e.key === " " || e.key === "Enter") {
          e.preventDefault();
          setFlipped((f) => !f);
        } else if (flipped && e.key === "1") grade(false);
        else if (flipped && e.key === "2") grade(true);
      }}
    >
      <div className="flex items-center gap-2 text-micro text-subtle-foreground">
        <span className="tabular-nums">
          {dueCount} of {cards.length} due for review
        </span>
        {(gotIt > 0 || missed.length > 0) && (
          <span className="tabular-nums">
            · this session: {gotIt} got it, {missed.length} missed
          </span>
        )}
        <button
          type="button"
          onClick={exportPdf}
          disabled={printing}
          title="Print / save all cards as PDF"
          className="ml-auto inline-flex items-center gap-1 rounded px-1.5 py-0.5 transition-colors hover:text-foreground disabled:opacity-50"
        >
          <FileDown className="h-3 w-3" />
          PDF
        </button>
      </div>
      {printing && <PrintCards cards={cards} />}
      {done ? (
        <div className="flex min-h-[220px] flex-col items-center justify-center gap-3 rounded-xl border border-border bg-surface-2 px-8 py-6 text-center">
          <span className="text-section font-medium text-foreground">
            Session complete — {gotIt} got it, {missed.length} missed
          </span>
          <span className="text-caption text-muted-foreground">
            Missed cards come back immediately; the rest return on a 1 / 3 / 7 / 21
            day schedule.
          </span>
          <div className="flex gap-2">
            {missed.length > 0 && (
              <button
                type="button"
                onClick={() => restart(missed)}
                className="rounded-md border border-border-strong bg-elevated px-3 py-1.5 text-caption text-foreground transition-colors hover:border-ring/50"
              >
                Review {missed.length} missed
              </button>
            )}
            <button
              type="button"
              onClick={() => restart()}
              className="inline-flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-caption text-muted-foreground transition-colors hover:text-foreground"
            >
              <RotateCcw className="h-3 w-3" />
              Start over
            </button>
          </div>
        </div>
      ) : (
        <>
          <button
            type="button"
            onClick={() => setFlipped((f) => !f)}
            aria-label={flipped ? "Show front" : "Show back"}
            className={cn(
              "flex min-h-[220px] w-full flex-col items-center justify-center gap-3 rounded-xl border px-8 py-6 text-center transition-colors",
              flipped
                ? "border-border bg-surface-2"
                : "border-border-strong bg-elevated hover:border-ring/50",
            )}
          >
            <span className="text-badge font-medium uppercase tracking-wider text-subtle-foreground">
              {flipped ? "Back" : "Front"}
            </span>
            {flipped ? (
              <>
                <span className="text-body text-muted-foreground">{card.front}</span>
                <span className="text-section leading-relaxed text-foreground">
                  {card.back}
                </span>
              </>
            ) : (
              <span className="text-[1.0625rem] font-medium leading-snug text-foreground">
                {card.front}
              </span>
            )}
            {!flipped && (
              <span className="text-micro text-subtle-foreground">
                Recall the answer, then click to reveal
              </span>
            )}
          </button>
          {flipped ? (
            <div className="flex items-center justify-center gap-2">
              <button
                type="button"
                onClick={() => grade(false)}
                className="inline-flex items-center gap-1.5 rounded-md border border-destructive/50 px-3 py-1.5 text-caption text-destructive transition-colors hover:bg-destructive/10"
              >
                <X className="h-3.5 w-3.5" />
                Missed it
                <kbd className="text-badge text-subtle-foreground">1</kbd>
              </button>
              <button
                type="button"
                onClick={() => grade(true)}
                className="inline-flex items-center gap-1.5 rounded-md border border-success/50 px-3 py-1.5 text-caption text-success transition-colors hover:bg-success/10"
              >
                <Check className="h-3.5 w-3.5" />
                Got it
                <kbd className="text-badge text-subtle-foreground">2</kbd>
              </button>
            </div>
          ) : (
            <div className="flex items-center justify-center gap-3">
              <button
                type="button"
                onClick={() => go(-1)}
                disabled={index === 0}
                aria-label="Previous card"
                className="rounded-md border border-border p-1.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-40"
              >
                <ChevronLeft className="h-4 w-4" />
              </button>
              <span className="min-w-16 text-center text-caption tabular-nums text-muted-foreground">
                {index + 1} / {queue.length}
              </span>
              <button
                type="button"
                onClick={() => go(1)}
                disabled={index === queue.length - 1}
                aria-label="Next card"
                className="rounded-md border border-border p-1.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-40"
              >
                <ChevronRight className="h-4 w-4" />
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}
