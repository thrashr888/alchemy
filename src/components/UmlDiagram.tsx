import { useEffect, useMemo, useState } from "react";
import { Code2, Image as ImageIcon } from "lucide-react";
import { useMermaidSvg } from "./MermaidBlock";
import { PanCanvas } from "./MindMap";
import { PrintPortal } from "./printExport";

/**
 * Native UML viewer for the `uml` artifact.
 *
 * The generator emits bare Mermaid source (one `classDiagram`,
 * `sequenceDiagram`, `stateDiagram-v2`, `erDiagram` or `flowchart`), so the
 * whole note IS the diagram — unlike a ```mermaid fence inside a document,
 * which MermaidBlock handles inline. That difference is what this component
 * is for: the diagram gets the pane, an infinite pan/zoom canvas (UML grows
 * wider than any column), and a source view for reading or copying the
 * model out.
 *
 * A model that will not parse shows the source with mermaid's own error
 * rather than an empty box — the generator's text is still the useful part,
 * and a diagram is easier to hand-fix than to regenerate.
 */

/** Diagram-type keyword → the UML name a reader knows it by. */
const UML_TYPES: [RegExp, string][] = [
  [/^classDiagram/, "Class diagram"],
  [/^sequenceDiagram/, "Sequence diagram"],
  [/^stateDiagram(-v2)?/, "State diagram"],
  [/^erDiagram/, "Entity relationship"],
  [/^(flowchart|graph)\b/, "Component diagram"],
  [/^journey/, "User journey"],
  [/^C4(Context|Container|Component|Dynamic)/, "C4 model"],
];

/** The note's Mermaid source: models sometimes wrap it in a fence despite
 *  the instruction, and a stray prose preamble should not break the parse. */
export function umlSource(content: string): string {
  const fenced = /```(?:mermaid)?\s*\n([\s\S]*?)```/.exec(content);
  const body = (fenced ? fenced[1] : content).trim();
  const lines = body.split("\n");
  const start = lines.findIndex((l) => UML_TYPES.some(([re]) => re.test(l.trim())));
  return (start > 0 ? lines.slice(start).join("\n") : body).trim();
}

/** What kind of UML this is, for the chip beside the diagram. */
export function umlKindLabel(code: string): string {
  const first = code.trimStart().split("\n")[0]?.trim() ?? "";
  return UML_TYPES.find(([re]) => re.test(first))?.[1] ?? "Diagram";
}

/** The model as text — readable, selectable, and copyable straight out. */
function UmlSource({ code }: { code: string }) {
  return (
    <pre className="selectable whitespace-pre rounded-md border border-border bg-surface-2/40 p-4 font-mono text-caption leading-relaxed text-foreground/90">
      {code}
    </pre>
  );
}

export function UmlDiagram({ content }: { content: string }) {
  const code = useMemo(() => umlSource(content), [content]);
  const [showSource, setShowSource] = useState(false);
  const { svg, error } = useMermaidSvg(code);

  // A broken model is a source-reading problem: show the text and say why.
  if (error !== null) {
    return (
      <div className="flex h-full min-h-0 flex-col gap-3">
        <p className="shrink-0 text-caption text-destructive">
          This diagram doesn’t parse: {error.split("\n")[0]}
        </p>
        <div className="min-h-0 flex-1 overflow-auto">
          <UmlSource code={code} />
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex shrink-0 items-center gap-2">
        <span className="rounded-full border border-border px-2 py-0.5 text-micro uppercase tracking-wide text-muted-foreground">
          {umlKindLabel(code)}
        </span>
        <button
          type="button"
          onClick={() => setShowSource((s) => !s)}
          title={showSource ? "Show the diagram" : "Show the Mermaid source"}
          aria-pressed={showSource}
          className="ml-auto flex items-center gap-1.5 rounded px-2 py-1 text-caption text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
        >
          {showSource ? (
            <>
              <ImageIcon aria-hidden className="h-3.5 w-3.5" />
              Diagram
            </>
          ) : (
            <>
              <Code2 aria-hidden className="h-3.5 w-3.5" />
              Source
            </>
          )}
        </button>
      </div>
      {showSource ? (
        <div className="min-h-0 flex-1 overflow-auto">
          <UmlSource code={code} />
        </div>
      ) : !svg ? (
        <div
          className="min-h-0 flex-1 rounded-md border border-border bg-surface-2/40"
          aria-busy="true"
        />
      ) : (
        <div className="min-h-0 flex-1 rounded-md border border-border bg-surface-2/30">
          <PanCanvas>
            <div
              className="p-6 [&_svg]:h-auto [&_svg]:max-w-none"
              // Sanitized in useMermaidSvg before it ever reaches here.
              dangerouslySetInnerHTML={{ __html: svg }}
            />
          </PanCanvas>
        </div>
      )}
    </div>
  );
}

/**
 * Print sheet for PDF/PNG export: the diagram at natural size, no pan/zoom
 * viewport to crop it. Mermaid's SVG carries its own theme colors, which on
 * a dark theme means light-on-dark ink — so the sheet keeps a white ground
 * behind it rather than pretending the export is a document.
 */
export function PrintUml({
  content,
  onReady,
}: {
  content: string;
  /** Fires once the sheet has settled — a rendered diagram or a parse
   *  failure. The export window waits for it before printing. */
  onReady?: () => void;
}) {
  const code = useMemo(() => umlSource(content), [content]);
  const { svg, error } = useMermaidSvg(code);
  const settled = !!svg || error !== null;
  useEffect(() => {
    if (settled) onReady?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settled]);
  return (
    <PrintPortal pageCss="@page { size: auto; margin: 16mm; }">
      <div
        style={{
          background: "#fff",
          WebkitPrintColorAdjust: "exact",
          display: "flex",
          justifyContent: "center",
        }}
      >
        {svg ? (
          <div
            style={{ maxWidth: 620 }}
            className="[&_svg]:h-auto [&_svg]:max-w-full"
            // Sanitized in useMermaidSvg before it ever reaches here.
            dangerouslySetInnerHTML={{ __html: svg }}
          />
        ) : (
          // Mermaid hasn't answered yet (or won't): the source is still the
          // model, and a blank sheet would be worse than a printed listing.
          <pre style={{ color: "#111", fontSize: 11, maxWidth: 620 }}>{code}</pre>
        )}
      </div>
    </PrintPortal>
  );
}
