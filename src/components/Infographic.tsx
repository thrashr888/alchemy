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
 * app's semantic tokens, so it follows every theme. Per DESIGN.md, blocks
 * are hairline-bordered cards (no tonal fills) and the one accent color
 * appears only where it means something: the bar fills.
 */

export type InfographicBlock =
  | { type: "stats"; items: { value: string; label: string }[] }
  | { type: "bars"; rows: { label: string; value: number; display: string }[] }
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

/** First number in a table cell, commas tolerated ("1,204 ms" → 1204). */
function cellNumber(display: string): number {
  const m = /-?\d[\d,]*(?:\.\d+)?/.exec(display);
  return m ? parseFloat(m[0].replace(/,/g, "")) : NaN;
}

/** A GFM table becomes bars when every data row's second cell is numeric;
 *  anything else stays prose (Markdown renders the table as-is). */
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
  return { type: "bars", rows: bars };
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
      case "bullet":
        blocks.push({
          type: "facts",
          items: run.map((line) => line.replace(/^[-*•]\s+/, "").trim()),
        });
        break;
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

function BlockView({ block }: { block: InfographicBlock }) {
  switch (block.type) {
    case "stats":
      return (
        <div
          className="grid gap-2"
          style={{ gridTemplateColumns: "repeat(auto-fit, minmax(150px, 1fr))" }}
        >
          {block.items.map((item, i) => (
            <div key={i} className="rounded-lg border border-border px-4 py-3.5">
              <div className="text-[1.75rem] font-semibold leading-none tracking-tight tabular-nums">
                <Inline text={item.value} />
              </div>
              <div className="mt-2 text-caption leading-snug text-muted-foreground">
                <Inline text={item.label} />
              </div>
            </div>
          ))}
        </div>
      );
    case "bars": {
      const max = Math.max(...block.rows.map((row) => row.value), 0);
      return (
        <div className="flex flex-col gap-2">
          {block.rows.map((row, i) => (
            <div
              key={i}
              className="grid items-center gap-3"
              style={{ gridTemplateColumns: "minmax(0, 9rem) 1fr auto" }}
            >
              <div
                className="truncate text-caption text-muted-foreground"
                title={row.label}
              >
                <Inline text={row.label} />
              </div>
              <div className="relative h-4" aria-hidden>
                <div
                  className="absolute inset-y-0 left-0 rounded-[3px] bg-primary/30"
                  style={{
                    width: `${max > 0 ? Math.max((row.value / max) * 100, 1.5) : 0}%`,
                  }}
                />
              </div>
              <div className="text-caption tabular-nums text-foreground">
                {row.display}
              </div>
            </div>
          ))}
        </div>
      );
    }
    case "facts":
      return (
        <ul className="flex flex-col gap-2">
          {block.items.map((item, i) => (
            <li
              key={i}
              className="rounded-lg border border-border px-3.5 py-2.5 text-body leading-relaxed"
            >
              <Inline text={item} />
            </li>
          ))}
        </ul>
      );
    case "quote":
      return (
        <figure className="rounded-lg border border-border px-6 py-5 text-center">
          <blockquote className="text-[1.0625rem] font-medium leading-relaxed">
            <Inline text={block.text} />
          </blockquote>
          {block.attribution && (
            <figcaption className="mt-2 text-caption text-muted-foreground">
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
  return (
    <div className="mx-auto w-full max-w-[680px]">
      <div className="flex items-start justify-between gap-3">
        <h1 className="text-[1.625rem] font-semibold leading-tight tracking-tight">
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
        <div className="mt-2 text-[0.9375rem] leading-relaxed text-muted-foreground">
          {doc.hook.map((block, i) => (
            <BlockView key={i} block={block} />
          ))}
        </div>
      )}
      <div className="mt-3 flex flex-col divide-y divide-border border-t border-border">
        {doc.sections.map((section, i) => (
          <section key={i} className="py-5">
            <h2 className="mb-3 text-micro font-medium uppercase tracking-wide text-muted-foreground">
              {section.heading}
            </h2>
            <div className="flex flex-col gap-3">
              {section.blocks.map((block, j) => (
                <BlockView key={j} block={block} />
              ))}
            </div>
          </section>
        ))}
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
function PrintInfographic({ doc }: { doc: InfographicDoc }) {
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
