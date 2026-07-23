import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import { openUrl } from "@tauri-apps/plugin-opener";

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

/** A wide table scrolls inside its own container instead of stretching the
 *  whole chat/note column sideways. */
function ScrollableTable({
  node: _node,
  ...props
}: React.TableHTMLAttributes<HTMLTableElement> & { node?: unknown }) {
  return (
    <div className="overflow-x-auto">
      <table {...props} />
    </div>
  );
}

/** Chat citations carry `sourceTitle`; meta-chat citations carry `title` —
 *  the chip works with either, and `citationLabel` overrides the tooltip. */
export function Markdown<C extends { snippet: string }>({
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
  const remarkPlugins = [
    remarkGfm,
    ...(interactive ? [remarkCitations(citations.length)] : []),
    ...(wikilinks ? [remarkWikilinks()] : []),
  ];
  return (
    <div className="prose">
      <ReactMarkdown
        remarkPlugins={remarkPlugins}
        rehypePlugins={REHYPE_PLUGINS}
        components={
          interactive
            ? {
                table: ScrollableTable,
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
                      className="mx-0.5 inline-flex h-[18px] min-w-[18px] translate-y-[-2px] cursor-pointer items-center justify-center rounded bg-primary/15 px-1 align-baseline text-[11px] font-semibold text-citation transition-colors hover:bg-primary/30"
                    >
                      {linkChildren}
                    </button>
                  );
                },
              }
            : { table: ScrollableTable, a: ExternalLink }
        }
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}
