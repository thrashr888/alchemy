import { useEffect, useState } from "react";
import { useStore } from "@/lib/store";
import { mermaidConfig, sanitizeSvg } from "@/lib/mermaid";

/**
 * A ```mermaid fenced block, rendered as a diagram
 * (docs/RFC-document-surface.md phase 5).
 *
 * Three things shape this component:
 *
 *  - **Mermaid is lazy.** The library is ~2MB parsed and most documents have
 *    no diagram in them, so it loads on first sight of one and never before.
 *    Vite code-splits it out of the main bundle on the dynamic import.
 *  - **Diagram source is untrusted.** Source content is fetched from the open
 *    web and notes can be written by agents, so this is defended twice:
 *    mermaid runs at `securityLevel: "strict"` (no click bindings, no inline
 *    HTML labels), AND the SVG it returns goes through DOMPurify before it is
 *    injected. Mermaid bundles DOMPurify and may well sanitize already, but
 *    that is not a documented guarantee of `render()`, and this is the one
 *    place in the app that injects markup built from source text — the same
 *    bargain the markdown renderer makes with rehype-sanitize.
 *  - **Diagrams follow the theme.** No hex belongs in a component (DESIGN.md),
 *    so the palette is read off the live CSS custom properties and handed to
 *    mermaid as themeVariables. Re-rendered whenever the theme changes.
 *
 * A diagram that will not parse falls back to the code block it came from —
 * a half-written diagram in a document is a rendering problem, never a
 * reason to hide the author's text.
 */

/** Module-scoped so the library is fetched and configured once per run, not
 *  once per diagram — a document with twelve diagrams should pay once. */
let mermaidPromise: Promise<typeof import("mermaid").default> | null = null;
function loadMermaid() {
  mermaidPromise ??= import("mermaid").then((m) => m.default);
  return mermaidPromise;
}

/** Ids must be unique per render — mermaid keys internal state on them. */
let diagramSeq = 0;

/** Render `code` to sanitized SVG, re-running on theme changes. `error` holds
 *  mermaid's own parse message so callers can show it (the UML viewer does);
 *  MermaidBlock only cares that it failed. Both null while loading. */
export function useMermaidSvg(code: string) {
  const theme = useStore((s) => s.theme);
  const [svg, setSvg] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let stale = false;
    setError(null);
    void loadMermaid()
      .then(async (mermaid) => {
        mermaid.initialize(mermaidConfig());
        const id = `mermaid-${++diagramSeq}`;
        // `parse` throws on a malformed diagram without touching the DOM,
        // which is what lets the fallback stay clean.
        await mermaid.parse(code);
        const { svg: out } = await mermaid.render(id, code);
        if (!stale) setSvg(sanitizeSvg(out));
      })
      .catch((e) => {
        if (!stale) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      stale = true;
    };
    // `theme` is a dependency on purpose: a theme switch re-reads the tokens
    // and re-renders every diagram on screen.
  }, [code, theme]);

  return { svg, error };
}

export function MermaidBlock({ code }: { code: string }) {
  const { svg, error } = useMermaidSvg(code);
  const failed = error !== null;

  if (failed || (!svg && !code.trim())) {
    return (
      <pre>
        <code className="language-mermaid">{code}</code>
      </pre>
    );
  }

  if (!svg) {
    // Hold the slot rather than collapsing the document while it loads.
    return (
      <div
        className="my-3 h-24 rounded-md border border-border bg-surface-2/40"
        aria-busy="true"
      />
    );
  }

  return (
    <div
      // Wide diagrams scroll inside their own box; the document must never
      // scroll sideways.
      className="my-3 overflow-x-auto rounded-md border border-border bg-surface-2/40 p-3 [&_svg]:mx-auto [&_svg]:h-auto [&_svg]:max-w-full"
      // Sanitized above with DOMPurify before it ever reaches here.
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
