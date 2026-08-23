import { useEffect, useState } from "react";
import { FileDown } from "lucide-react";
import { Markdown } from "./Markdown";
import { PrintPortal, usePrintExport } from "./printExport";

/**
 * Native infographic renderer. The generator emits a rigid markdown shape —
 * `# title`, an optional hook, then `##` sections whose bodies are stat
 * lines, a 2-column numeric table, bullets, a quote, or a paragraph (see
 * rag::artifact_spec) — and this component infers each block's visual from
 * its shape, mirroring SlideDeck's layout inference: the model never
 * declares layout, so small models stay reliable. Falls back to plain
 * Markdown whenever the content doesn't parse, so a note never arrives
 * broken.
 *
 * The poster is a single centered scrolling column styled entirely by the
 * app's semantic tokens, so it follows every theme. Unlike app chrome, a
 * poster is allowed to lean into color: it cycles the five theme-managed
 * data hues (primary + the artifact-family tokens) through tinted washes,
 * ring gauges, and bar fills — no hex anywhere, no border-accents, and the
 * whole thing re-tints with the theme.
 */

/** The poster palette: theme-managed hues, cycled per tile / bar / section. */
const HUES = [
  "var(--primary)",
  "var(--artifact-generate)",
  "var(--artifact-learning)",
  "var(--artifact-documents)",
  "var(--artifact-template)",
];
const hue = (i: number) => HUES[i % HUES.length];
/** A tinted wash of a palette hue (tonal fill, stays legible in any theme). */
const wash = (color: string, pct = 9) =>
  `color-mix(in srgb, ${color} ${pct}%, transparent)`;

/** "42%" | "87.5 %" → 0-100 for the ring gauge; NaN when not a percent. */
function percentOf(value: string): number {
  const m = /^-?(\d+(?:\.\d+)?)\s*%$/.exec(value.trim());
  return m ? Math.min(parseFloat(m[1]), 100) : NaN;
}

export type InfographicBlock =
  | { type: "stats"; items: { value: string; label: string }[] }
  | { type: "bars"; rows: { label: string; value: number; display: string }[] }
  | { type: "funnel"; rows: { label: string; value: number; display: string }[] }
  | { type: "timeline"; items: { date: string; text: string }[] }
  | { type: "compare"; sides: { label: string; items: string[] }[] }
  | { type: "facts"; items: string[] }
  | { type: "quote"; text: string; attribution?: string }
  | { type: "prose"; text: string };

export interface InfographicSection {
  heading: string;
  blocks: InfographicBlock[];
}

export interface InfographicDoc {
  title: string;
  hook: InfographicBlock[];
  sections: InfographicSection[];
}

/** `**<value>** — <label>` (any dash or colon separator survives models). */
const STAT_RE = /^\*\*(.+?)\*\*\s*[—–:-]+\s*(.+)$/;

/** `2019 — text` | `Q3 2025 — text` | `March 2024 — text`: a dated bullet.
 *  A bullet run is a timeline only when EVERY item matches (ranking lists
 *  and ordinary facts must never get a rail). */
const TIMELINE_RE = /^((?:Q[1-4]\s+)?\d{4}|[A-Z][a-z]{2,9}\.?\s+\d{4}|\d{4}\s*[–-]\s*\d{2,4})\s*[—–:-]+\s*(.+)$/;

/** First number in a table cell, commas tolerated ("1,204 ms" → 1204). */
function cellNumber(display: string): number {
  const m = /-?\d[\d,]*(?:\.\d+)?/.exec(display);
  return m ? parseFloat(m[0].replace(/,/g, "")) : NaN;
}

/** A GFM table becomes bars when every data row's second cell is numeric —
 *  or a funnel when its first header cell is `Stage` (the spec's explicit
 *  marker: decreasing values alone would misread ranking tables as funnels).
 *  Anything else stays prose (Markdown renders the table as-is). */
function tableBlock(lines: string[]): InfographicBlock {
  const rows = lines
    .map((line) =>
      line
        .trim()
        .replace(/^\|/, "")
        .replace(/\|$/, "")
        .split("|")
        .map((cell) => cell.trim()),
    )
    .filter((cells) => !cells.every((cell) => /^:?-{2,}:?$/.test(cell) || cell === ""));
  const data = rows.slice(1); // drop the header row
  const bars: { label: string; value: number; display: string }[] = [];
  for (const cells of data) {
    if (cells.length < 2) return { type: "prose", text: lines.join("\n") };
    const value = cellNumber(cells[1]);
    if (!Number.isFinite(value)) return { type: "prose", text: lines.join("\n") };
    bars.push({ label: cells[0], value, display: cells[1] });
  }
  if (bars.length < 2) return { type: "prose", text: lines.join("\n") };
  const isFunnel = rows[0]?.[0]?.toLowerCase() === "stage";
  return { type: isFunnel ? "funnel" : "bars", rows: bars };
}

/** Exactly two `### name` subheadings, each followed only by bullets,
 *  make a head-to-head comparison; any other use of `###` falls through
 *  to the ordinary line-shape parser. */
function compareBlock(lines: string[]): InfographicBlock | null {
  const sides: { label: string; items: string[] }[] = [];
  for (const raw of lines) {
    const t = raw.trim();
    if (!t) continue;
    const h3 = /^###\s+(.+)$/.exec(t);
    if (h3) {
      sides.push({ label: h3[1].trim(), items: [] });
      continue;
    }
    if (!/^[-*•]\s+/.test(t) || sides.length === 0) return null;
    sides[sides.length - 1].items.push(t.replace(/^[-*•]\s+/, "").trim());
  }
  if (sides.length !== 2 || sides.some((side) => side.items.length === 0))
    return null;
  return { type: "compare", sides };
}

type Shape = "stat" | "table" | "quote" | "bullet" | "text";

function shapeOf(line: string): Shape {
  const t = line.trim();
  if (STAT_RE.test(t)) return "stat";
  if (/^\|.*\|/.test(t)) return "table";
  if (/^>/.test(t)) return "quote";
  if (/^[-*•]\s+/.test(t)) return "bullet";
  return "text";
}

/** Group a section body into typed blocks by line shape. */
function parseBlocks(lines: string[]): InfographicBlock[] {
  // A body that is exactly two ###-led bullet groups is a comparison.
  const compare = lines.some((line) => /^###\s+/.test(line.trim()))
    ? compareBlock(lines)
    : null;
  if (compare) return [compare];
  const blocks: InfographicBlock[] = [];
  let i = 0;
  while (i < lines.length) {
    if (!lines[i].trim()) {
      i++;
      continue;
    }
    const shape = shapeOf(lines[i]);
    const run: string[] = [];
    while (i < lines.length) {
      const t = lines[i].trim();
      if (!t) {
        // A blank only ends the run when the next content changes shape —
        // models like to air out stat lines and bullets.
        let j = i + 1;
        while (j < lines.length && !lines[j].trim()) j++;
        if (j >= lines.length || shapeOf(lines[j]) !== shape) break;
        i = j;
        continue;
      }
      if (shapeOf(lines[i]) !== shape) break;
      run.push(t);
      i++;
    }
    switch (shape) {
      case "stat":
        blocks.push({
          type: "stats",
          items: run.map((line) => {
            const m = STAT_RE.exec(line)!;
            return { value: m[1].trim(), label: m[2].trim() };
          }),
        });
        break;
      case "table":
        blocks.push(tableBlock(run));
        break;
      case "quote": {
        const text = run.map((line) => line.replace(/^>\s?/, "")).join(" ").trim();
        // Attribution: a short trailing `— name` line after the blockquote.
        let attribution: string | undefined;
        let j = i;
        while (j < lines.length && !lines[j].trim()) j++;
        const next = lines[j]?.trim() ?? "";
        if (/^[—–]\s*\S/.test(next) && next.length <= 80) {
          attribution = next.replace(/^[—–]\s*/, "");
          i = j + 1;
        }
        if (text) blocks.push({ type: "quote", text, attribution });
        break;
      }
      case "bullet": {
        const items = run.map((line) => line.replace(/^[-*•]\s+/, "").trim());
        const dated = items.map((item) => TIMELINE_RE.exec(item));
        if (items.length >= 3 && dated.every(Boolean)) {
          blocks.push({
            type: "timeline",
            items: dated.map((m) => ({ date: m![1].trim(), text: m![2].trim() })),
          });
        } else {
          blocks.push({ type: "facts", items });
        }
        break;
      }
      default:
        blocks.push({ type: "prose", text: run.join("\n") });
    }
  }
  return blocks;
}

/** Parse the infographic spec; null unless a title + 2 real sections emerge. */
export function parseInfographic(md: string): InfographicDoc | null {
  const lines = md.replace(/\r\n/g, "\n").split("\n");
  let i = 0;
  while (i < lines.length && !lines[i].trim()) i++;
  const h1 = /^#\s+(.+)$/.exec(lines[i]?.trim() ?? "");
  if (!h1) return null;
  const title = h1[1].trim();
  i++;

  const hookLines: string[] = [];
  const sections: { heading: string; body: string[] }[] = [];
  for (; i < lines.length; i++) {
    const h2 = /^##\s+(.+)$/.exec(lines[i].trim());
    if (h2 && !/^###/.test(lines[i].trim())) {
      sections.push({ heading: h2[1].trim(), body: [] });
    } else if (sections.length === 0) {
      hookLines.push(lines[i]);
    } else {
      sections[sections.length - 1].body.push(lines[i]);
    }
  }

  const parsed = sections
    .map((section) => ({ heading: section.heading, blocks: parseBlocks(section.body) }))
    .filter((section) => section.blocks.length > 0);
  if (!title || parsed.length < 2) return null;
  return { title, hook: parseBlocks(hookLines), sections: parsed };
}

/** Minimal inline markdown for labels and facts — bold, italic, code. */
function Inline({ text }: { text: string }) {
  const parts: React.ReactNode[] = [];
  const re = /\*\*([^*]+)\*\*|\*([^*]+)\*|`([^`]+)`/g;
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(re)) {
    if (m.index > last) parts.push(text.slice(last, m.index));
    if (m[1] !== undefined) parts.push(<strong key={key++}>{m[1]}</strong>);
    else if (m[2] !== undefined) parts.push(<em key={key++}>{m[2]}</em>);
    else parts.push(<code key={key++}>{m[3]}</code>);
    last = m.index + m[0].length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return <>{parts}</>;
}

/** Ring gauge for a percentage stat: conic fill over a washed track. */
function Ring({ pct, color }: { pct: number; color: string }) {
  return (
    <div
      aria-hidden
      className="grid h-[72px] w-[72px] place-items-center rounded-full"
      style={{
        background: `conic-gradient(${color} ${pct * 3.6}deg, ${wash(color, 16)} 0)`,
      }}
    >
      <div className="grid h-[54px] w-[54px] place-items-center rounded-full bg-surface" />
    </div>
  );
}

function BlockView({
  block,
  grown,
}: {
  block: InfographicBlock;
  /** Bars/rings animate from zero on mount; the print sheet renders static. */
  grown: boolean;
}) {
  switch (block.type) {
    case "stats":
      return (
        <div
          className="grid gap-2.5"
          style={{ gridTemplateColumns: "repeat(auto-fit, minmax(150px, 1fr))" }}
        >
          {block.items.map((item, i) => {
            const color = hue(i);
            const pct = percentOf(item.value);
            return (
              <div
                key={i}
                className="relative flex flex-col justify-between gap-2 overflow-hidden rounded-xl px-4 py-4"
                style={{ background: wash(color) }}
              >
                {Number.isFinite(pct) ? (
                  <div className="relative">
                    <Ring pct={grown ? pct : 0} color={color} />
                    <div
                      className="absolute inset-0 grid h-[72px] w-[72px] place-items-center text-[0.9375rem] font-bold tabular-nums"
                      style={{ color }}
                    >
                      <Inline text={item.value} />
                    </div>
                  </div>
                ) : (
                  <div
                    className="text-[2.125rem] font-bold leading-none tracking-tight tabular-nums"
                    style={{ color }}
                  >
                    <Inline text={item.value} />
                  </div>
                )}
                <div className="text-caption font-medium leading-snug text-foreground/80">
                  <Inline text={item.label} />
                </div>
              </div>
            );
          })}
        </div>
      );
    case "bars": {
      const max = Math.max(...block.rows.map((row) => row.value), 0);
      return (
        <div className="flex flex-col gap-3">
          {block.rows.map((row, i) => {
            const color = hue(i);
            const width = max > 0 ? Math.max((row.value / max) * 100, 2) : 0;
            return (
              <div key={i}>
                <div className="mb-1 flex items-baseline justify-between gap-3">
                  <div className="truncate text-caption font-medium" title={row.label}>
                    <Inline text={row.label} />
                  </div>
                  <div
                    className="shrink-0 text-caption font-bold tabular-nums"
                    style={{ color }}
                  >
                    {row.display}
                  </div>
                </div>
                <div
                  className="h-3.5 overflow-hidden rounded-full"
                  style={{ background: wash(color, 12) }}
                  aria-hidden
                >
                  <div
                    className="h-full rounded-full transition-[width] duration-200 ease-out"
                    style={{
                      width: `${grown ? width : 0}%`,
                      background: `linear-gradient(90deg, color-mix(in srgb, ${color} 70%, transparent), ${color})`,
                    }}
                  />
                </div>
              </div>
            );
          })}
        </div>
      );
    }
    case "funnel": {
      const max = Math.max(...block.rows.map((row) => row.value), 0);
      return (
        <div className="flex flex-col items-center gap-1.5">
          {block.rows.map((row, i) => {
            const color = hue(i);
            const width = max > 0 ? Math.max((row.value / max) * 100, 18) : 18;
            return (
              <div
                key={i}
                className="flex items-center justify-between gap-3 rounded-lg px-4 py-2 transition-[width] duration-200 ease-out"
                style={{
                  width: `${grown ? width : 18}%`,
                  minWidth: "fit-content",
                  background: wash(color, 16),
                }}
              >
                <span className="truncate text-caption font-medium" title={row.label}>
                  <Inline text={row.label} />
                </span>
                <span
                  className="shrink-0 text-caption font-bold tabular-nums"
                  style={{ color }}
                >
                  {row.display}
                </span>
              </div>
            );
          })}
        </div>
      );
    }
    case "timeline":
      return (
        <div className="flex flex-col">
          {block.items.map((item, i) => {
            const color = hue(i);
            return (
              <div key={i} className="grid grid-cols-[7rem_auto_1fr] gap-x-3">
                <div
                  className="pt-0.5 text-right text-caption font-bold tabular-nums"
                  style={{ color }}
                >
                  {item.date}
                </div>
                {/* The rail: a dot per event, a hairline thread between. */}
                <div className="flex flex-col items-center" aria-hidden>
                  <span
                    className="mt-1.5 h-2.5 w-2.5 shrink-0 rounded-full"
                    style={{ background: color }}
                  />
                  {i < block.items.length - 1 && (
                    <span className="w-px flex-1 bg-border-strong" />
                  )}
                </div>
                <div className="pb-4 text-body leading-relaxed">
                  <Inline text={item.text} />
                </div>
              </div>
            );
          })}
        </div>
      );
    case "compare":
      return (
        <div className="grid grid-cols-[1fr_auto_1fr] items-stretch gap-2.5">
          {block.sides.map((side, i) => {
            const color = hue(i * 2); // skip a hue so the two sides contrast
            return (
              <div
                key={side.label}
                className="flex flex-col gap-2.5 rounded-xl px-4 py-3.5"
                style={{ background: wash(color, 8), order: i * 2 }}
              >
                <div
                  className="text-[0.9375rem] font-bold tracking-tight"
                  style={{ color }}
                >
                  <Inline text={side.label} />
                </div>
                <ul className="flex flex-col gap-1.5 text-caption leading-relaxed">
                  {side.items.map((item, j) => (
                    <li key={j} className="flex gap-2">
                      <span
                        aria-hidden
                        className="mt-[7px] h-1 w-1 shrink-0 rounded-full"
                        style={{ background: color }}
                      />
                      <span>
                        <Inline text={item} />
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            );
          })}
          <div className="grid place-items-center" style={{ order: 1 }} aria-hidden>
            <span className="rounded-full bg-surface-2 px-2 py-1 text-micro font-bold uppercase text-muted-foreground">
              vs
            </span>
          </div>
        </div>
      );
    case "facts":
      return (
        <ul className="flex flex-col gap-2">
          {block.items.map((item, i) => {
            const color = hue(i);
            return (
              <li
                key={i}
                className="flex items-start gap-3 rounded-xl px-3.5 py-3 text-body leading-relaxed"
                style={{ background: wash(color, 6) }}
              >
                <span
                  className="mt-0.5 grid h-6 w-6 shrink-0 place-items-center rounded-full text-micro font-bold"
                  style={{ background: wash(color, 18), color }}
                >
                  {i + 1}
                </span>
                <span>
                  <Inline text={item} />
                </span>
              </li>
            );
          })}
        </ul>
      );
    case "quote":
      return (
        <figure
          className="rounded-xl px-7 py-6 text-center"
          style={{ background: wash(HUES[0], 7) }}
        >
          <div
            aria-hidden
            className="mx-auto mb-1 font-serif text-[2.5rem] leading-none"
            style={{ color: HUES[0] }}
          >
            &ldquo;
          </div>
          <blockquote className="text-[1.125rem] font-medium leading-relaxed">
            <Inline text={block.text} />
          </blockquote>
          {block.attribution && (
            <figcaption className="mt-2.5 text-caption text-muted-foreground">
              — <Inline text={block.attribution} />
            </figcaption>
          )}
        </figure>
      );
    default:
      return <Markdown>{block.text}</Markdown>;
  }
}

export function Infographic({ content, title }: { content: string; title?: string }) {
  const doc = parseInfographic(content);
  if (!doc) return <Markdown>{content}</Markdown>;
  return <Poster doc={doc} exportName={title ?? doc.title} />;
}

function Poster({ doc, exportName }: { doc: InfographicDoc; exportName: string }) {
  const { printing, exportPdf } = usePrintExport({ suggestedName: exportName });
  // Flip after first paint so bars sweep from zero to their widths.
  const [grown, setGrown] = useState(false);
  useEffect(() => {
    const id = requestAnimationFrame(() => setGrown(true));
    return () => cancelAnimationFrame(id);
  }, []);
  return (
    <div className="mx-auto w-full max-w-[680px]">
      {/* Poster ribbon: the five data hues announce "infographic" up top. */}
      <div
        aria-hidden
        className="mb-5 h-1 rounded-full"
        style={{ background: `linear-gradient(90deg, ${HUES.join(", ")})` }}
      />
      <div className="flex items-start justify-between gap-3">
        <h1 className="text-[2rem] font-bold leading-[1.15] tracking-tight">
          {doc.title}
        </h1>
        <button
          type="button"
          onClick={exportPdf}
          disabled={printing}
          aria-label="Export infographic as PDF"
          title="Print / save the infographic as PDF"
          className="mt-1 inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-caption text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50"
        >
          <FileDown className="h-3.5 w-3.5" />
          PDF
        </button>
      </div>
      {doc.hook.length > 0 && (
        <div className="mt-3 text-[1.0625rem] leading-relaxed text-muted-foreground">
          {doc.hook.map((block, i) => (
            <BlockView key={i} block={block} grown={grown} />
          ))}
        </div>
      )}
      <div className="mt-6 flex flex-col gap-8">
        {doc.sections.map((section, i) => {
          const color = hue(i);
          return (
            <section key={i}>
              <h2 className="mb-3.5 flex items-center gap-2.5">
                <span
                  aria-hidden
                  className="grid h-7 w-7 shrink-0 place-items-center rounded-lg text-caption font-bold tabular-nums"
                  style={{ background: wash(color, 16), color }}
                >
                  {String(i + 1).padStart(2, "0")}
                </span>
                <span className="text-[0.9375rem] font-semibold tracking-tight">
                  {section.heading}
                </span>
              </h2>
              <div className="flex flex-col gap-3">
                {section.blocks.map((block, j) => (
                  <BlockView key={j} block={block} grown={grown} />
                ))}
              </div>
            </section>
          );
        })}
      </div>
      {printing && <PrintInfographic doc={doc} />}
    </div>
  );
}

/** Strip inline markdown for the fixed-ink print sheet. */
function plain(text: string): string {
  return text
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/\*([^*]+)\*/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1");
}

/**
 * Portrait print sheet. Print is a document, not chrome (DESIGN.md §3): like
 * the flashcards study sheet it uses fixed paper ink — near-black on white,
 * gray bars — instead of the screen theme, which could be light-on-dark.
 */
export function PrintInfographic({ doc }: { doc: InfographicDoc }) {
  const muted = { color: "#555" };
  const card: React.CSSProperties = {
    border: "1px solid #ddd",
    borderRadius: 8,
    padding: "8px 12px",
  };
  const printBlock = (block: InfographicBlock, key: number) => {
    switch (block.type) {
      case "stats":
        return (
          <div key={key} style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
            {block.items.map((item, i) => (
              <div key={i} style={{ ...card, flex: "1 1 130px" }}>
                <div style={{ fontSize: 22, fontWeight: 650, letterSpacing: "-0.01em" }}>
                  {plain(item.value)}
                </div>
                <div style={{ fontSize: 10.5, marginTop: 3, ...muted }}>
                  {plain(item.label)}
                </div>
              </div>
            ))}
          </div>
        );
      case "bars": {
        const max = Math.max(...block.rows.map((row) => row.value), 0);
        return (
          <div key={key} style={{ display: "flex", flexDirection: "column", gap: 5 }}>
            {block.rows.map((row, i) => (
              <div key={i} style={{ display: "flex", alignItems: "center", gap: 10 }}>
                <div style={{ width: 130, fontSize: 10.5, ...muted }}>
                  {plain(row.label)}
                </div>
                <div style={{ flex: 1 }}>
                  <div
                    style={{
                      height: 11,
                      borderRadius: 2,
                      background: "#c9c9c9",
                      width: `${max > 0 ? Math.max((row.value / max) * 100, 1.5) : 0}%`,
                    }}
                  />
                </div>
                <div style={{ fontSize: 10.5, fontVariantNumeric: "tabular-nums" }}>
                  {row.display}
                </div>
              </div>
            ))}
          </div>
        );
      }
      case "funnel": {
        const max = Math.max(...block.rows.map((row) => row.value), 0);
        return (
          <div
            key={key}
            style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 4 }}
          >
            {block.rows.map((row, i) => (
              <div
                key={i}
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  gap: 12,
                  padding: "4px 12px",
                  borderRadius: 6,
                  background: "#e4e4e4",
                  width: `${max > 0 ? Math.max((row.value / max) * 100, 18) : 18}%`,
                  minWidth: "fit-content",
                  fontSize: 10.5,
                }}
              >
                <span>{plain(row.label)}</span>
                <span style={{ fontWeight: 650, fontVariantNumeric: "tabular-nums" }}>
                  {row.display}
                </span>
              </div>
            ))}
          </div>
        );
      }
      case "timeline":
        return (
          <div key={key} style={{ display: "flex", flexDirection: "column", gap: 5 }}>
            {block.items.map((item, i) => (
              <div key={i} style={{ display: "flex", gap: 10, fontSize: 11 }}>
                <span
                  style={{
                    width: 90,
                    textAlign: "right",
                    fontWeight: 650,
                    fontVariantNumeric: "tabular-nums",
                    flexShrink: 0,
                  }}
                >
                  {item.date}
                </span>
                <span>{plain(item.text)}</span>
              </div>
            ))}
          </div>
        );
      case "compare":
        return (
          <div key={key} style={{ display: "flex", gap: 8 }}>
            {block.sides.map((side) => (
              <div key={side.label} style={{ ...card, flex: 1 }}>
                <div style={{ fontWeight: 650, fontSize: 11.5, marginBottom: 4 }}>
                  {plain(side.label)}
                </div>
                {side.items.map((item, j) => (
                  <div key={j} style={{ fontSize: 10.5, marginTop: 2 }}>
                    • {plain(item)}
                  </div>
                ))}
              </div>
            ))}
          </div>
        );
      case "facts":
        return (
          <div key={key} style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            {block.items.map((item, i) => (
              <div key={i} style={card}>
                {plain(item)}
              </div>
            ))}
          </div>
        );
      case "quote":
        return (
          <div key={key} style={{ ...card, textAlign: "center", padding: "12px 20px" }}>
            <div style={{ fontSize: 14, fontWeight: 500 }}>{plain(block.text)}</div>
            {block.attribution && (
              <div style={{ fontSize: 10.5, marginTop: 4, ...muted }}>
                — {plain(block.attribution)}
              </div>
            )}
          </div>
        );
      default:
        return (
          <div key={key} style={{ fontSize: 12, lineHeight: 1.55 }}>
            {plain(block.text)}
          </div>
        );
    }
  };
  return (
    <PrintPortal pageCss="@page { size: auto; margin: 16mm; }">
      <div
        style={{
          color: "#111",
          fontFamily: "system-ui, sans-serif",
          fontSize: 12,
          maxWidth: 620,
          margin: "0 auto",
          // The bar/funnel fills are plain CSS backgrounds; WebKit drops
          // backgrounds from print output unless told to keep the ink.
          WebkitPrintColorAdjust: "exact",
          printColorAdjust: "exact",
        }}
      >
        <div style={{ fontSize: 26, fontWeight: 650, letterSpacing: "-0.015em" }}>
          {doc.title}
        </div>
        {doc.hook.map((block, i) => (
          <div key={i} style={{ marginTop: 6, fontSize: 13, ...muted }}>
            {printBlock(block, i)}
          </div>
        ))}
        {doc.sections.map((section, i) => (
          <div
            key={i}
            className="print-card"
            style={{ borderTop: "1px solid #ddd", marginTop: 14, paddingTop: 12 }}
          >
            <div
              style={{
                fontSize: 9.5,
                fontWeight: 600,
                textTransform: "uppercase",
                letterSpacing: "0.08em",
                marginBottom: 8,
                ...muted,
              }}
            >
              {section.heading}
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              {section.blocks.map((block, j) => printBlock(block, j))}
            </div>
          </div>
        ))}
      </div>
    </PrintPortal>
  );
}
