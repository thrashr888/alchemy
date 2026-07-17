import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, FileDown, Maximize2, X } from "lucide-react";
import { Markdown } from "./Markdown";
import { PrintPortal, usePrintExport } from "./printExport";
import { THEMES as APP_THEMES, DEFAULT_THEME as APP_DEFAULT_THEME } from "@/lib/themes";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";

/**
 * Native slide-deck renderer. The generator emits Marp-style markdown — a
 * front-matter block choosing a color theme and font, then slides separated
 * by `---` lines (see rag::artifact_spec). Falls back to Markdown when the
 * content doesn't split into slides, so a deck never arrives broken.
 *
 * Slides are laid out at a fixed 960×540 design resolution and scaled with a
 * CSS transform to fit whatever box they're shown in (modal, note window,
 * fullscreen, print page) — one set of type sizes, no reflow, no nested
 * scrollbars, aspect ratio always preserved. Deck palettes are the app's own
 * UI themes (each theme's tokens mapped onto slide vars), so decks share
 * Alchemy's design language without inheriting whatever theme the app
 * happens to be in. The user can switch theme and font from the controls —
 * the choice is written back into the note's front-matter, so it persists
 * and survives Rebuild prompts. Layouts are inferred from content, not
 * declared, so small models stay reliable.
 */

const SLIDE_W = 960;
const SLIDE_H = 540;

export interface DeckStyle {
  theme: string;
  font: string;
}

export interface Deck extends DeckStyle {
  slides: string[];
}

/** Font pairings (system stacks — nothing to download). */
export const FONTS: Record<string, { label: string; heading: string; body: string }> = {
  sans: {
    label: "Sans",
    heading: "system-ui, -apple-system, 'Segoe UI', sans-serif",
    body: "system-ui, -apple-system, 'Segoe UI', sans-serif",
  },
  serif: {
    label: "Serif",
    heading: "ui-serif, 'New York', Georgia, 'Times New Roman', serif",
    body: "ui-serif, 'New York', Georgia, 'Times New Roman', serif",
  },
  mono: {
    label: "Mono",
    heading: "ui-monospace, 'SF Mono', Menlo, Consolas, monospace",
    body: "system-ui, -apple-system, 'Segoe UI', sans-serif",
  },
  rounded: {
    label: "Rounded",
    heading: "ui-rounded, 'SF Pro Rounded', system-ui, sans-serif",
    body: "system-ui, -apple-system, 'Segoe UI', sans-serif",
  },
};
const DEFAULT_FONT = "sans";

/** Slide palette derived from an app UI theme's design tokens. */
function palette(themeId: string) {
  const t = APP_THEMES[themeId] ?? APP_THEMES[APP_DEFAULT_THEME];
  return {
    bg: t.vars.background,
    fg: t.vars.foreground,
    muted: t.vars["muted-foreground"],
    accent: t.vars.citation,
  };
}

/** Pre-app-theme deck themes from earlier generations map to the closest. */
const LEGACY_THEMES: Record<string, string> = {
  paper: "sepia",
  ocean: "nord",
  forest: "gruvbox",
  ember: "synthwave",
};

function normalizeTheme(name: string | null): string | null {
  if (!name) return null;
  const id = name.toLowerCase();
  if (APP_THEMES[id]) return id;
  return LEGACY_THEMES[id] ?? null;
}

/** Parse Marp-style markdown into a styled deck; null when it isn't one. */
export function parseDeck(md: string, fallbackTheme: string): Deck | null {
  let theme: string | null = null;
  let font = DEFAULT_FONT;
  const slides: string[] = [];
  for (const block of md.split(/^\s*-{3,}\s*$/m)) {
    const text = block.trim();
    if (!text) continue;
    // A front-matter block is only `key: value` lines — take its style keys
    // and keep it out of the slide list.
    if (text.split("\n").every((l) => /^[\w-]+:\s*\S.*$/.test(l.trim()))) {
      const tm = /^theme:\s*(\S+)/im.exec(text);
      if (tm) theme = normalizeTheme(tm[1]) ?? theme;
      const fm = /^font:\s*(\S+)/im.exec(text);
      if (fm && FONTS[fm[1].toLowerCase()]) font = fm[1].toLowerCase();
      continue;
    }
    slides.push(text);
  }
  if (slides.length < 2) return null;
  return { theme: theme ?? normalizeTheme(fallbackTheme) ?? APP_DEFAULT_THEME, font, slides };
}

/** Rewrite (or insert) the front-matter style keys, preserving the slides. */
export function withFrontMatter(md: string, style: DeckStyle): string {
  const fm = `---\ntheme: ${style.theme}\nfont: ${style.font}\n---`;
  const lead = /^\s*-{3,}[ \t]*\n([\s\S]*?)\n-{3,}[ \t]*\n/.exec(md);
  const isFrontMatter =
    lead && lead[1].split("\n").every((l) => !l.trim() || /^[\w-]+:\s*\S/.test(l.trim()));
  const body = isFrontMatter ? md.slice(lead[0].length) : md;
  return `${fm}\n\n${body.trimStart()}`;
}

export type SlideLayout = "title" | "section" | "quote" | "statement" | "table" | "bullets";

/** Infer a slide's layout from its shape (see module comment). */
export function slideLayout(md: string): SlideLayout {
  const lines = md.split("\n").filter((l) => l.trim());
  const body = lines.filter((l) => !/^#{1,3}\s/.test(l.trim()));
  if (/^#\s/.test(lines[0] ?? "")) return "title";
  if (lines.length > 0 && body.length === 0) return "section";
  if (body.some((l) => /^>/.test(l.trim()))) return "quote";
  if (body.some((l) => /^\|.*\|/.test(l.trim()))) return "table";
  const bullets = body.filter((l) => /^[-*•]|\d+\./.test(l.trim()));
  if (bullets.length === 0 && body.length === 1 && body[0].length <= 140) return "statement";
  return "bullets";
}

/** Scale factor that fits the 960×540 design box inside the observed element. */
function useFitScale(ref: React.RefObject<HTMLElement | null>): number {
  const [scale, setScale] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      setScale(Math.min(r.width / SLIDE_W, r.height / SLIDE_H));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return scale;
}

/** Vertical space inside the slide box once .slide-surface padding is off. */
const SLIDE_PAD_Y = 96;

function styleVars(style: DeckStyle): React.CSSProperties {
  const p = palette(style.theme);
  const f = FONTS[style.font] ?? FONTS[DEFAULT_FONT];
  return {
    "--slide-bg": p.bg,
    "--slide-fg": p.fg,
    "--slide-muted": p.muted,
    "--slide-accent": p.accent,
    "--slide-font-heading": f.heading,
    "--slide-font-body": f.body,
  } as React.CSSProperties;
}

/** One slide at design resolution, scaled to fit its parent box. */
function SlideCanvas({ slide, style }: { slide: string; style: DeckStyle }) {
  const areaRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const scale = useFitScale(areaRef);

  // PowerPoint-style autofit: models overshoot word budgets, and a fixed
  // design box would clip the tail — zoom the content down (never up) until
  // it fits the 540px canvas. `scale` is a dep because the content only
  // mounts once the box has been measured (scale > 0); without it, a canvas
  // that opens directly on a dense slide would never get fitted.
  useLayoutEffect(() => {
    const el = contentRef.current;
    if (!el) return;
    el.style.setProperty("zoom", "1");
    const avail = SLIDE_H - SLIDE_PAD_Y;
    const height = el.scrollHeight;
    if (height > avail) {
      el.style.setProperty("zoom", String(Math.max(0.5, avail / height)));
    }
  }, [slide, scale]);

  return (
    <div ref={areaRef} className="flex h-full w-full items-center justify-center">
      {scale > 0 && (
        <div style={{ width: SLIDE_W * scale, height: SLIDE_H * scale }}>
          <div
            className={cn("slide-surface", `slide-layout-${slideLayout(slide)}`)}
            style={{
              width: SLIDE_W,
              height: SLIDE_H,
              transform: `scale(${scale})`,
              transformOrigin: "top left",
              ...styleVars(style),
            }}
          >
            <div ref={contentRef}>
              <Markdown>{slide}</Markdown>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/** All slides as fixed 960×540 print pages (one per @page, see index.css). */
function PrintDeck({ deck }: { deck: Deck }) {
  return (
    <PrintPortal pageCss="@page { size: 960px 540px; margin: 0; }">
      {deck.slides.map((slide, i) => (
        <div key={i} className="print-slide" style={{ width: SLIDE_W, height: SLIDE_H }}>
          <SlideCanvas slide={slide} style={deck} />
        </div>
      ))}
    </PrintPortal>
  );
}

export function SlideDeck({
  content,
  note,
}: {
  content: string;
  /** When given, theme/font switches persist into the note's front-matter. */
  note?: { id: string; title: string };
}) {
  const appTheme = useStore((s) => s.theme);
  const deck = parseDeck(content, appTheme);
  if (!deck) return <Markdown>{content}</Markdown>;
  return <DeckView deck={deck} content={content} note={note} />;
}

function DeckView({
  deck,
  content,
  note,
}: {
  deck: Deck;
  content: string;
  note?: { id: string; title: string };
}) {
  const { slides } = deck;
  const [index, setIndex] = useState(0);
  const [present, setPresent] = useState(false);
  // Style overrides live in state so switching feels instant; persisting to
  // the note's front-matter follows behind when we know the note.
  const [style, setStyle] = useState<DeckStyle>({ theme: deck.theme, font: deck.font });
  const { printing, exportPdf } = usePrintExport({
    landscape: true,
    suggestedName: note?.title ?? "Slide deck",
  });
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    rootRef.current?.focus();
  }, []);

  const restyle = (next: DeckStyle) => {
    setStyle(next);
    if (note) {
      useStore.getState().updateNote(note.id, note.title, withFrontMatter(content, next));
    }
  };

  const go = (dir: 1 | -1) => {
    setIndex((current) => Math.min(slides.length - 1, Math.max(0, current + dir)));
  };

  const selectClass =
    "h-7 rounded-md border border-border bg-transparent px-1.5 text-[12px] text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60";

  return (
    <div
      ref={rootRef}
      tabIndex={0}
      className="flex h-full min-h-0 flex-col gap-3 outline-none"
      onKeyDown={(e) => {
        if (e.key === "ArrowRight" || e.key === "ArrowDown") {
          e.preventDefault();
          go(1);
        } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
          e.preventDefault();
          go(-1);
        }
      }}
    >
      <div className="min-h-0 flex-1">
        <SlideCanvas slide={slides[index]} style={style} />
      </div>
      <div className="flex shrink-0 flex-wrap items-center justify-center gap-x-2 gap-y-1.5">
        <button
          type="button"
          onClick={() => go(-1)}
          disabled={index === 0}
          aria-label="Previous slide"
          className="rounded-md border border-border p-1.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-40"
        >
          <ChevronLeft className="h-4 w-4" />
        </button>
        <span className="min-w-14 text-center text-[12px] tabular-nums text-muted-foreground">
          {index + 1} / {slides.length}
        </span>
        <button
          type="button"
          onClick={() => go(1)}
          disabled={index === slides.length - 1}
          aria-label="Next slide"
          className="rounded-md border border-border p-1.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-40"
        >
          <ChevronRight className="h-4 w-4" />
        </button>
        <select
          value={style.theme}
          onChange={(e) => restyle({ ...style, theme: e.target.value })}
          aria-label="Deck theme"
          title="Deck color theme"
          className={cn(selectClass, "ml-2 max-w-32")}
        >
          {Object.values(APP_THEMES).map((t) => (
            <option key={t.id} value={t.id}>
              {t.label}
            </option>
          ))}
        </select>
        <select
          value={style.font}
          onChange={(e) => restyle({ ...style, font: e.target.value })}
          aria-label="Deck font"
          title="Deck font"
          className={selectClass}
        >
          {Object.entries(FONTS).map(([id, f]) => (
            <option key={id} value={id}>
              {f.label}
            </option>
          ))}
        </select>
        <button
          type="button"
          onClick={() => setPresent(true)}
          aria-label="Present fullscreen"
          className="ml-2 inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-[12px] text-muted-foreground transition-colors hover:text-foreground"
        >
          <Maximize2 className="h-3.5 w-3.5" />
          Present
        </button>
        <button
          type="button"
          onClick={exportPdf}
          disabled={printing}
          aria-label="Export deck as PDF"
          title="Print / save the whole deck as PDF"
          className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-[12px] text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50"
        >
          <FileDown className="h-3.5 w-3.5" />
          PDF
        </button>
      </div>
      {present && (
        <Presentation
          slides={slides}
          style={style}
          index={index}
          setIndex={setIndex}
          onExit={() => {
            setPresent(false);
            rootRef.current?.focus();
          }}
        />
      )}
      {printing && <PrintDeck deck={{ ...deck, ...style }} />}
    </div>
  );
}

/** Fullscreen presentation overlay. Esc exits (captured before the modal's
 *  own Esc-to-close so the note viewer stays open underneath). */
function Presentation({
  slides,
  style,
  index,
  setIndex,
  onExit,
}: {
  slides: string[];
  style: DeckStyle;
  index: number;
  setIndex: (updater: (i: number) => number) => void;
  onExit: () => void;
}) {
  const overlayRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    overlayRef.current?.focus();
    const go = (dir: 1 | -1) =>
      setIndex((current) => Math.min(slides.length - 1, Math.max(0, current + dir)));
    const onKey = (e: KeyboardEvent) => {
      // stopImmediatePropagation: the Modal's own Esc-to-close also listens
      // on window, and stopPropagation alone can't stop a same-target
      // listener — the presentation must swallow the key entirely.
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        onExit();
      } else if (e.key === "ArrowRight" || e.key === "ArrowDown" || e.key === " ") {
        e.preventDefault();
        e.stopImmediatePropagation();
        go(1);
      } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
        e.preventDefault();
        e.stopImmediatePropagation();
        go(-1);
      }
    };
    // Capture phase: fires before the Modal's window-level Esc handler.
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [slides.length, setIndex, onExit]);

  return (
    <div
      ref={overlayRef}
      tabIndex={-1}
      className="fixed inset-0 z-[80] flex flex-col bg-black outline-none"
    >
      <div className="min-h-0 flex-1 px-10 pb-14 pt-10">
        <SlideCanvas slide={slides[index]} style={style} />
      </div>
      <div className="absolute right-4 top-4">
        <button
          type="button"
          onClick={onExit}
          aria-label="Exit presentation"
          className="rounded-md border border-white/20 p-2 text-white/60 transition-colors hover:text-white"
        >
          <X className="h-4 w-4" />
        </button>
      </div>
      <div className="absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-3">
        <button
          type="button"
          onClick={() => setIndex((i) => Math.max(0, i - 1))}
          disabled={index === 0}
          aria-label="Previous slide"
          className="rounded-md border border-white/20 p-1.5 text-white/60 transition-colors hover:text-white disabled:opacity-30"
        >
          <ChevronLeft className="h-4 w-4" />
        </button>
        <span className="min-w-16 text-center text-[12px] tabular-nums text-white/50">
          {index + 1} / {slides.length}
        </span>
        <button
          type="button"
          onClick={() => setIndex((i) => Math.min(slides.length - 1, i + 1))}
          disabled={index === slides.length - 1}
          aria-label="Next slide"
          className="rounded-md border border-white/20 p-1.5 text-white/60 transition-colors hover:text-white disabled:opacity-30"
        >
          <ChevronRight className="h-4 w-4" />
        </button>
      </div>
    </div>
  );
}
