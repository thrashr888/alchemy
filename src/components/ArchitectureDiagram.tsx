import { useEffect, useMemo, useRef, useState } from "react";
import { Code2, Image as ImageIcon, TriangleAlert } from "lucide-react";
import {
  architectureSource,
  formatArchitecture,
  parseArchitecture,
  type ArchDoc,
} from "@/lib/architectureDoc";
import { ensureDocumentDiagramFonts } from "@/lib/diagramFonts";
import type { RenderedDiagram } from "@/lib/eraserDiagram";
import { PanCanvas } from "./MindMap";
import { PrintPortal } from "./printExport";

/**
 * Native viewer for the `architecture` artifact (docs/RFC-diagrams.md).
 *
 * The generator emits an eraser-diagrams document as JSON — groups,
 * components with technology icons, labeled connections — and the note IS
 * that document. This renders it through `lib/eraserDiagram.ts` (placement,
 * resolve, in-WebView render) and shows the scene on an infinite pan/zoom
 * canvas, with a source view for reading or copying the JSON out.
 *
 * A document that will not render shows the JSON with the resolver's own
 * message rather than an empty box: the text is still the model, and a
 * misnamed icon or a dangling `containerId` is easier to fix than to
 * regenerate.
 */

interface SceneState {
  rendered: RenderedDiagram | null;
  error: string | null;
}

function useArchitectureScene(content: string): SceneState & { doc?: ArchDoc; source: string } {
  const parsed = useMemo(() => parseArchitecture(content), [content]);
  const source = useMemo(
    () => (parsed.doc ? formatArchitecture(parsed.doc) : architectureSource(content)),
    [parsed, content],
  );
  const [state, setState] = useState<SceneState>({ rendered: null, error: null });

  useEffect(() => {
    let stale = false;
    if (!parsed.doc) {
      setState({ rendered: null, error: parsed.error });
      return;
    }
    const doc = parsed.doc;
    setState({ rendered: null, error: null });
    void import("@/lib/eraserDiagram")
      .then((m) => m.renderArchitecture(doc))
      .then(
        (rendered) => {
          if (!stale) setState({ rendered, error: null });
        },
        (e: unknown) => {
          if (!stale)
            setState({ rendered: null, error: e instanceof Error ? e.message : String(e) });
        },
      );
    return () => {
      stale = true;
    };
  }, [parsed]);

  return { ...state, doc: parsed.doc, source };
}

/**
 * The scene in a shadow root: eraser's stylesheet is global by design
 * (`*{box-sizing}`, a rule on every `svg[stroke='currentColor']`), so it
 * must not reach the app's own DOM. Fonts are the one thing a shadow tree
 * cannot declare for itself; the document gets those once.
 */
function Scene({ rendered, scale = 1 }: { rendered: RenderedDiagram; scale?: number }) {
  const host = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ensureDocumentDiagramFonts();
    const el = host.current;
    if (!el) return;
    const root = el.shadowRoot ?? el.attachShadow({ mode: "open" });
    // Eraser paints for paper: its palette is ink on white. The ground is
    // part of the diagram's own ink, like the theme colors inside a Mermaid
    // SVG, not an app surface — which is why it is not a token.
    root.innerHTML = `<style>${rendered.css}.paper{background:#fff;display:inline-block;transform-origin:0 0}</style><div class="paper" style="transform:scale(${scale})">${rendered.scene}</div>`;
  }, [rendered, scale]);
  return (
    <div
      ref={host}
      style={{ width: rendered.width * scale, height: rendered.height * scale }}
      className="overflow-hidden rounded-md"
    />
  );
}

/** The document as text — readable, selectable, and copyable straight out. */
function ArchitectureSource({ source }: { source: string }) {
  return (
    <pre className="selectable whitespace-pre rounded-md border border-border bg-surface-2/40 p-4 font-mono text-caption leading-relaxed text-foreground/90">
      {source}
    </pre>
  );
}

export function ArchitectureDiagram({ content }: { content: string }) {
  const { doc, source, rendered, error } = useArchitectureScene(content);
  const [showSource, setShowSource] = useState(false);
  const [showWarnings, setShowWarnings] = useState(false);

  if (error !== null) {
    return (
      <div className="flex h-full min-h-0 flex-col gap-3">
        <p className="shrink-0 whitespace-pre-line text-caption text-destructive">
          This diagram doesn’t render: {error}
        </p>
        <div className="min-h-0 flex-1 overflow-auto">
          <ArchitectureSource source={source} />
        </div>
      </div>
    );
  }

  const warnings = rendered?.warnings ?? [];
  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex shrink-0 items-center gap-2">
        <span className="rounded-full border border-border px-2 py-0.5 text-micro uppercase tracking-wide text-muted-foreground">
          {doc?.title ? doc.title : "Architecture"}
        </span>
        {warnings.length > 0 && (
          <button
            type="button"
            onClick={() => setShowWarnings((s) => !s)}
            aria-pressed={showWarnings}
            title="The renderer had notes on this document"
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-micro text-muted-foreground hover:bg-surface-2 hover:text-foreground"
          >
            <TriangleAlert aria-hidden className="h-3 w-3" />
            {warnings.length}
          </button>
        )}
        <button
          type="button"
          onClick={() => setShowSource((s) => !s)}
          title={showSource ? "Show the diagram" : "Show the diagram JSON"}
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
      {showWarnings && warnings.length > 0 && (
        <ul className="shrink-0 list-disc pl-5 text-caption text-muted-foreground">
          {warnings.map((w) => (
            <li key={w}>{w}</li>
          ))}
        </ul>
      )}
      {showSource ? (
        <div className="min-h-0 flex-1 overflow-auto">
          <ArchitectureSource source={source} />
        </div>
      ) : !rendered ? (
        <div
          className="min-h-0 flex-1 rounded-md border border-border bg-surface-2/40"
          aria-busy="true"
        />
      ) : (
        <div className="min-h-0 flex-1 rounded-md border border-border bg-surface-2/30">
          <PanCanvas>
            <div className="p-6">
              <Scene rendered={rendered} />
            </div>
          </PanCanvas>
        </div>
      )}
    </div>
  );
}

/**
 * Print sheet for PDF/PNG export: the whole scene scaled to the page width,
 * never a panned viewport crop. The diagram is ink on paper already, so the
 * sheet keeps a white ground like the UML sheet does.
 */
export function PrintArchitecture({
  content,
  onReady,
}: {
  content: string;
  /** Fires once the sheet has settled — a rendered scene or a failure. The
   *  export window waits for it before printing. */
  onReady?: () => void;
}) {
  const { source, rendered, error } = useArchitectureScene(content);
  const settled = !!rendered || error !== null;
  useEffect(() => {
    if (settled) onReady?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settled]);
  const scale = rendered ? Math.min(1, 620 / rendered.width) : 1;
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
        {rendered ? (
          <Scene rendered={rendered} scale={scale} />
        ) : (
          // Not rendered (or never will be): the JSON is still the model,
          // and a blank sheet would be worse than a printed listing.
          <pre style={{ color: "#111", fontSize: 11, maxWidth: 620, whiteSpace: "pre-wrap" }}>
            {source}
          </pre>
        )}
      </div>
    </PrintPortal>
  );
}
