import { isDiagramKind, parseDiagram } from "@/lib/diagramDoc";
import { ensureDocumentDiagramFonts } from "@/lib/diagramFonts";
import { renderDiagram } from "@/lib/eraserDiagram";

// One viewer, repeatedly replaced: the memory test must not itself retain
// every scene as the visual contact-sheet harness intentionally does.
const files = import.meta.glob("./samples/*.json", { query: "?raw", import: "default" });
const docs = await Promise.all(Object.entries(files).sort().map(async ([name, load]) => {
  const content = await load() as string;
  const rawKind = JSON.parse(content).kind ?? "architecture";
  const kind = isDiagramKind(rawKind) ? rawKind : "architecture";
  const parsed = parseDiagram(content, kind);
  if (!parsed.doc) throw new Error(`${name}: ${parsed.error}`);
  return { doc: parsed.doc, kind };
}));
ensureDocumentDiagramFonts();
const view = document.getElementById("view")!;

const harness = {
  async draw(i: number) {
    const { doc, kind } = docs[i % docs.length];
    const started = performance.now();
    const rendered = await renderDiagram(doc, kind);
    const host = document.createElement("div");
    host.attachShadow({ mode: "open" }).innerHTML = `<style>${rendered.css}</style>${rendered.scene}`;
    view.replaceChildren(host);
    await document.fonts.ready;
    return { ms: performance.now() - started, bytes: rendered.scene.length + rendered.css.length };
  },
  clearScene() { view.replaceChildren(); },
  async cancelledBurst() {
    const jobs = docs.map(({ doc, kind }) => {
      const abort = new AbortController();
      const job = renderDiagram(doc, kind, abort.signal).then(
        () => "unexpected render",
        (error: unknown) => error instanceof DOMException ? error.name : String(error),
      );
      abort.abort();
      return job;
    });
    return Promise.all(jobs);
  },
};
Object.assign(window, { diagramMemory: harness });
