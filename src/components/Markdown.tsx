import React, { memo, useMemo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import { openUrl } from "@tauri-apps/plugin-opener";
import { MermaidBlock } from "./MermaidBlock";

/**
 * GitHub-flavored markdown allows a subset of inline HTML (<details>,
 * <summary>, <kbd>, <sup>…). rehype-raw parses it and rehype-sanitize clamps
 * it to GitHub's own allowlist — source content is fetched from the open web,
 * so nothing executable may pass.
 */
const REHYPE_PLUGINS = [
  rehypeRaw,
  [rehypeSanitize, defaultSchema],
] as import("react-markdown").Options["rehypePlugins"];

/** ```mermaid fences render as diagrams; every other fence stays a code
 *  block. Shared by both renderer configurations below. */
function CodeBlock({
  className,
  children,
  ...props
}: React.HTMLAttributes<HTMLElement>) {
  if (className === "language-mermaid") {
    return <MermaidBlock code={String(children).replace(/\n$/, "")} />;
  }
  return (
    <code className={className} {...props}>
      {children}
    </code>
  );
}

/** External links must open in the system browser, not navigate the webview. */
function ExternalLink({
  href,
  children,
  ...props
}: React.AnchorHTMLAttributes<HTMLAnchorElement>) {
  const external = /^(https?|mailto):/.test(href ?? "");
  return (
    <a
      href={href}
      {...props}
      onClick={
        external
          ? (e) => {
              e.preventDefault();
              void openUrl(href!);
            }
          : undefined
      }
    >
      {children}
    </a>
  );
}

/**
 * Turn `[n]` citation markers in text nodes into `#cite-n` links so the `a`
 * renderer below can make them clickable chips. Walks the mdast tree directly
 * (plain objects) to avoid pulling in unist utilities.
 */
function remarkCitations(maxN: number) {
  interface Node {
    type: string;
    value?: string;
    url?: string;
    children?: Node[];
  }
  const split = (value: string): Node[] => {
    const out: Node[] = [];
    let last = 0;
    for (const m of value.matchAll(/\[(\d{1,2})\]/g)) {
      const n = Number(m[1]);
      if (n < 1 || n > maxN) continue;
      if (m.index > last) out.push({ type: "text", value: value.slice(last, m.index) });
      out.push({
        type: "link",
        url: `#cite-${n}`,
        children: [{ type: "text", value: String(n) }],
      });
      last = m.index + m[0].length;
    }
    if (out.length === 0) return [{ type: "text", value }];
    if (last < value.length) out.push({ type: "text", value: value.slice(last) });
    return out;
  };
  const visit = (node: Node) => {
    if (!node.children) return;
    node.children = node.children.flatMap((child) => {
      if (child.type === "text" && child.value) return split(child.value);
      // Don't rewrite text inside real links or code.
      if (child.type !== "link" && child.type !== "inlineCode" && child.type !== "code") {
        visit(child);
      }
      return [child];
    });
  };
  return () => (tree: Node) => visit(tree);
}

/**
 * Turn Obsidian-style `[[wikilinks]]` in text nodes into ordinary relative
 * links (`[[Note#h|alias]]` → `<a href="Note.md">alias</a>`) so the reader's
 * in-corpus link routing can hop between vault notes. Same mdast walk as
 * remarkCitations; only enabled for document bodies (the `wikilinks` prop).
 */
function remarkWikilinks() {
  interface Node {
    type: string;
    value?: string;
    url?: string;
    children?: Node[];
  }
  const split = (value: string): Node[] => {
    const out: Node[] = [];
    let last = 0;
    for (const m of value.matchAll(/\[\[([^\][|#]+)(?:#([^\][|]*))?(?:\|([^\][]*))?\]\]/g)) {
      const target = m[1].trim();
      if (!target) continue;
      if (m.index > last) out.push({ type: "text", value: value.slice(last, m.index) });
      const display = m[3]?.trim() || (m[2] ? `${target} › ${m[2].trim()}` : target);
      const href = /\.[a-z0-9]{1,5}$/i.test(target) ? target : `${target}.md`;
      out.push({ type: "link", url: href, children: [{ type: "text", value: display }] });
      last = m.index + m[0].length;
    }
    if (out.length === 0) return [{ type: "text", value }];
    if (last < value.length) out.push({ type: "text", value: value.slice(last) });
    return out;
  };
  const visit = (node: Node) => {
    if (!node.children) return;
    node.children = node.children.flatMap((child) => {
      if (child.type === "text" && child.value?.includes("[[")) return split(child.value);
      if (child.type !== "link" && child.type !== "inlineCode" && child.type !== "code") {
        visit(child);
      }
      return [child];
    });
  };
  return () => (tree: Node) => visit(tree);
}

/** A wide table scrolls inside its own hairline frame instead of stretching
 *  the whole chat/note column sideways (styling in index.css .table-wrap). */
function ScrollableTable({
  node: _node,
  ...props
}: React.TableHTMLAttributes<HTMLTableElement> & { node?: unknown }) {
  return (
    <div className="table-wrap">
      <table {...props} />
    </div>
  );
}

/** Every string inside a rendered element tree, concatenated — how the
 *  table components below read a cell without touching the DOM. */
function textOf(node: React.ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (React.isValidElement<{ children?: React.ReactNode }>(node))
    return textOf(node.props.children);
  return "";
}

/** A cell that holds a bare figure — money, percent, count — sits flush
 *  right with tabular numerals (index.css .cell-num), the way every decent
 *  data table sets numbers. */
const NUMERIC_CELL = /^[$€£+\-(]?[\d,]+(\.\d+)?\)?%?$/;
function SmartTd({
  node: _node,
  children,
  ...props
}: React.TdHTMLAttributes<HTMLTableCellElement> & { node?: unknown }) {
  const text = textOf(children).trim();
  const numeric = text !== "" && NUMERIC_CELL.test(text);
  return (
    <td {...props} className={numeric ? "cell-num" : undefined}>
      {children}
    </td>
  );
}

/** Spreadsheet exports open with a BLANK header row (their first line is a
 *  title, not column names) — GFM needs the row, readers don't. Render
 *  nothing when no header cell has text. */
function SmartThead({
  node: _node,
  children,
  ...props
}: React.HTMLAttributes<HTMLTableSectionElement> & { node?: unknown }) {
  if (textOf(children).trim() === "") return null;
  return <thead {...props}>{children}</thead>;
}

/** Section-title rows — one label, then a run of empty cells, ubiquitous in
 *  spreadsheet exports — read as the subheads they are (index.css
 *  .tr-section) instead of data rows full of holes. */
function SmartTr({
  node: _node,
  children,
  ...props
}: React.HTMLAttributes<HTMLTableRowElement> & { node?: unknown }) {
  const cells = React.Children.toArray(children).filter(React.isValidElement);
  const texts = cells.map((c) => textOf(c).trim());
  const isSection =
    texts.length >= 3 &&
    texts[0] !== "" &&
    texts.slice(1).every((t) => t === "");
  return (
    <tr {...props} className={isSection ? "tr-section" : undefined}>
      {children}
    </tr>
  );
}

/** The static component map for non-interactive renders, hoisted so memo'd
 *  consumers see a stable identity. */
const PLAIN_COMPONENTS = {
  table: ScrollableTable,
  thead: SmartThead,
  tr: SmartTr,
  td: SmartTd,
  code: CodeBlock,
  a: ExternalLink,
};

/** Chat citations carry `sourceTitle`; meta-chat citations carry `title` —
 *  the chip works with either, and `citationLabel` overrides the tooltip.
 *
 *  Memoized (export below): this sits on every hot text path — the reader's
 *  rich view, the chat transcript, streaming previews — and an unmemoized
 *  render is a full remark+rehype parse of the whole string. */
function MarkdownInner<C extends { snippet: string }>({
  children,
  citations,
  onCitation,
  citationLabel,
  wikilinks,
}: {
  children: string;
  /** When present, inline [n] markers become clickable citation chips. */
  citations?: C[];
  onCitation?: (citation: C) => void;
  citationLabel?: (citation: C) => string;
  /** Render `[[wikilinks]]` as relative links (document bodies only). */
  wikilinks?: boolean;
}) {
  const interactive = !!citations?.length && !!onCitation;
  const label =
    citationLabel ??
    ((c: C) => {
      const t = c as { sourceTitle?: string; title?: string };
      return t.sourceTitle ?? t.title ?? "";
    });
  const remarkPlugins = useMemo(
    () => [
      remarkGfm,
      ...(interactive ? [remarkCitations(citations?.length ?? 0)] : []),
      ...(wikilinks ? [remarkWikilinks()] : []),
    ],
    [interactive, citations?.length, wikilinks],
  );
  return (
    <div className="prose">
      <ReactMarkdown
        remarkPlugins={remarkPlugins}
        rehypePlugins={REHYPE_PLUGINS}
        components={
          interactive
            ? {
                table: ScrollableTable,
                thead: SmartThead,
                tr: SmartTr,
                td: SmartTd,
                code: CodeBlock,
                a: ({ href, children: linkChildren, ...props }) => {
                  const n = href?.startsWith("#cite-") ? Number(href.slice(6)) : NaN;
                  const cite = Number.isInteger(n) ? citations[n - 1] : undefined;
                  if (!cite)
                    return (
                      <ExternalLink href={href} {...props}>
                        {linkChildren}
                      </ExternalLink>
                    );
                  return (
                    <button
                      onClick={() => onCitation(cite)}
                      title={`${label(cite)} — “${cite.snippet.slice(0, 120)}…”`}
                      // Read mid-sentence, a bare "[3], button" says nothing
                      // about where the claim came from — name the source.
                      aria-label={`Citation ${n}, ${label(cite)}`}
                      className="mx-0.5 inline-flex h-[18px] min-w-[18px] translate-y-[-2px] cursor-pointer items-center justify-center rounded bg-primary/15 px-1 align-baseline text-micro font-semibold text-citation transition-colors hover:bg-primary/30"
                    >
                      {linkChildren}
                    </button>
                  );
                },
              }
            : PLAIN_COMPONENTS
        }
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}

/** Memoized export; the cast keeps the generic citation typing that
 *  `memo` would otherwise erase. */
export const Markdown = memo(MarkdownInner) as typeof MarkdownInner;
