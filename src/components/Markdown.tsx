import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Citation } from "@/lib/types";

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

export function Markdown({
  children,
  citations,
  onCitation,
}: {
  children: string;
  /** When present, inline [n] markers become clickable citation chips. */
  citations?: Citation[];
  onCitation?: (citation: Citation) => void;
}) {
  const interactive = !!citations?.length && !!onCitation;
  return (
    <div className="prose">
      <ReactMarkdown
        remarkPlugins={interactive ? [remarkGfm, remarkCitations(citations.length)] : [remarkGfm]}
        components={
          interactive
            ? {
                a: ({ href, children: linkChildren, ...props }) => {
                  const n = href?.startsWith("#cite-") ? Number(href.slice(6)) : NaN;
                  const cite = Number.isInteger(n) ? citations[n - 1] : undefined;
                  if (!cite) return <a href={href} {...props}>{linkChildren}</a>;
                  return (
                    <button
                      onClick={() => onCitation(cite)}
                      title={`${cite.sourceTitle} — “${cite.snippet.slice(0, 120)}…”`}
                      className="mx-0.5 inline-flex h-[18px] min-w-[18px] translate-y-[-2px] cursor-pointer items-center justify-center rounded bg-primary/15 px-1 align-baseline text-[11px] font-semibold text-citation transition-colors hover:bg-primary/30"
                    >
                      {linkChildren}
                    </button>
                  );
                },
              }
            : undefined
        }
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}
