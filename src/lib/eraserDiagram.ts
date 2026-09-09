/**
 * Eraser diagrams, rendered in the WebView with eraser-diagrams
 * (docs/RFC-diagrams.md) and no Chromium: `@eraserlabs/resolve` validates
 * the document against the stock template library and inlines icons,
 * `@eraserlabs/render/browser` — running in a same-origin iframe, see
 * `diagramFrame.ts` — fills, measures, routes, and applies it, and the
 * serialized scene comes back here as positioned HTML plus its stylesheet.
 *
 * Placement is ours (`diagramLayout.ts`): the document carries no
 * coordinates, so this runs the render twice — once on estimated sizes to
 * learn what the components actually measure, once on those measurements
 * to place them for real. Both passes take tens of milliseconds.
 *
 * This module is imported lazily: it carries the resolver, the template
 * library, and the vendored icon set, none of which a notebook without a
 * diagram should pay for.
 */
import type { EraserBrowserApi, ElementMeasure } from "@eraserlabs/render/browser";
import type { Issue, Resolver } from "@eraserlabs/resolve";
import {
  CONTAINER_TAGS,
  prepareForRender,
  type DiagramDoc,
  type DiagramEntity,
  type DiagramKind,
} from "./diagramDoc";
import { estimateSize, placeNodes, type LayoutNode } from "./diagramLayout";

export interface RenderedDiagram {
  /** `#eraser-scene` as HTML: positioned wrappers, filled templates, inline SVGs. */
  scene: string;
  /** The stylesheets the scene needs — font role vars, base, scoped templates. */
  css: string;
  width: number;
  height: number;
  /** Resolver warnings: unknown icons, dropped props, sanitized content. */
  warnings: string[];
}

/** Icon name → raw SVG, for the curated set the app ships. The resolver
 *  sanitizes and normalizes each one on first use and caches it. */
const ICONS = import.meta.glob("../assets/diagrams/icons/*.svg", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/** Every icon name the renderer can draw without the network. */
export const ICON_NAMES: readonly string[] = Object.keys(ICONS)
  .map((p) => p.replace(/^.*\//, "").replace(/\.svg$/, ""))
  .sort();

let resolverPromise: Promise<Resolver> | undefined;

function getResolver(): Promise<Resolver> {
  resolverPromise ??= (async () => {
    const [{ createResolver }, { stockLibrary }, { stockNormalizers }, { normalizeFetchedIcon }] =
      await Promise.all([
        import("@eraserlabs/resolve"),
        import("@eraserlabs/diagrams/library"),
        import("@eraserlabs/diagrams/normalizers"),
        import("@eraserlabs/diagrams/svg-transforms"),
      ]);
    return createResolver({
      library: stockLibrary,
      normalizers: stockNormalizers,
      iconLoader: async (name) => {
        const raw = ICONS[`../assets/diagrams/icons/${name}.svg`];
        if (!raw) throw new Error(`no icon named "${name}"`);
        return normalizeFetchedIcon(raw, name);
      },
      // A misnamed icon draws a placeholder glyph and warns; the diagram
      // still renders and the warning says what to fix.
      onUnknownIcon: "placeholder",
    });
  })();
  return resolverPromise;
}

interface Frame {
  api: EraserBrowserApi;
  document: Document;
}

let framePromise: Promise<Frame> | undefined;

/** The render iframe, created once per document and set up with the stock library. */
function getFrame(): Promise<Frame> {
  framePromise ??= new Promise<Frame>((resolve, reject) => {
    const iframe = document.createElement("iframe");
    iframe.setAttribute("aria-hidden", "true");
    iframe.tabIndex = -1;
    // Off-screen but laid out at a real size: measurement needs a viewport,
    // and `visibility:hidden` keeps layout while hiding paint.
    iframe.style.cssText =
      "position:fixed;left:-10000px;top:0;width:2400px;height:2400px;visibility:hidden;pointer-events:none;border:0";
    // Root-relative: the page is a Vite entry beside index.html, and every
    // window (main, note, export, the harness) serves from the same root.
    iframe.src = new URL("/diagram-frame.html", document.baseURI).href;
    iframe.addEventListener("load", () => {
      void (async () => {
        const win = iframe.contentWindow;
        const boot = win?.__alchemyDiagramFrame;
        if (!win || !boot) throw new Error("the diagram frame did not boot");
        await boot.ready;
        const resolver = await getResolver();
        const { buildRenderPageSetup } = await import("@eraserlabs/diagrams/library");
        win.__eraser.setup(buildRenderPageSetup(resolver.library));
        return { api: win.__eraser, document: win.document };
      })().then(resolve, reject);
    });
    iframe.addEventListener("error", () =>
      reject(new Error("the diagram frame failed to load")),
    );
    document.body.appendChild(iframe);
  });
  framePromise.catch(() => {
    framePromise = undefined; // let the next render try again
  });
  return framePromise;
}

// The frame holds one scene at a time, so renders run one after another.
let chain: Promise<unknown> = Promise.resolve();
function queued<T>(task: () => Promise<T>): Promise<T> {
  const next = chain.then(task, task);
  chain = next.catch(() => undefined);
  return next;
}

type Size = { width: number; height: number };

/** Leaves that take the estimate as an authored minimum width. */
const SIZED_TAGS: ReadonlySet<string> = new Set(["Shape", "Activity", "Textbox"]);

/**
 * What each kind asks of the layout, per container. A swimlane (Lane, or
 * a Pool holding steps directly) runs its steps across the scene's flow
 * and shares columns with every other lane; a Pool holding lanes stacks
 * them; a journey stage is a column whose members stack in the order the
 * document lists them.
 */
function layoutHints(
  entity: DiagramEntity,
  doc: DiagramDoc,
  kind: DiagramKind,
): Pick<LayoutNode, "flow" | "band" | "lane" | "sequence"> {
  const across = doc.direction === "down" ? "right" : "down";
  const holdsLanes = doc.entities.some(
    (e) => e.containerId === entity.id && (e.tag === "Lane" || e.tag === "Pool"),
  );
  switch (entity.tag) {
    case "Lane":
    case "Pool":
      return holdsLanes ? { band: true } : { flow: across, band: true, lane: true };
    case "Group":
      return kind === "journey" ? { flow: across, sequence: true } : {};
    default:
      return {};
  }
}

/** The document with coordinates: our placement written onto eraser's entities. */
function placed(doc: DiagramDoc, kind: DiagramKind, sizeOf: (entity: DiagramEntity) => Size) {
  const nodes: LayoutNode[] = doc.entities.map((entity) => ({
    id: entity.id,
    containerId: entity.containerId ?? null,
    container: CONTAINER_TAGS.has(entity.tag),
    ...layoutHints(entity, doc, kind),
    ...sizeOf(entity),
  }));
  const edges = doc.connections.map((c) => ({
    from: c.from,
    to: c.to,
    labeled: typeof c.label === "string" && c.label.trim() !== "",
  }));
  return placeNodes(nodes, edges, {
    direction: doc.direction,
    // Journey stages share a top edge; everything else centers on its rank.
    ...(kind === "journey" ? { align: "start" as const } : {}),
  });
}

function describe(issue: Issue): string {
  const where = issue.elementId ? `${issue.tag ?? "element"} "${issue.elementId}"` : issue.path;
  return `${where}: ${issue.message}${issue.suggestion ? ` (${issue.suggestion})` : ""}`;
}

/**
 * Render a document to a scene. Throws with the resolver's errors when the
 * document is not a diagram eraser can draw; the caller shows the JSON and
 * the message, since the text is still the useful part.
 */
export function renderDiagram(source: DiagramDoc, kind: DiagramKind): Promise<RenderedDiagram> {
  return queued(async () => {
    const resolver = await getResolver();
    const frame = await getFrame();
    const doc = prepareForRender(source, kind);

    // Pass 1: estimated sizes, so the resolver has the coordinates its
    // schemas require and the renderer has something to measure.
    const first = placed(doc, kind, estimateSize);
    const authored = {
      ...(doc.title ? { title: doc.title } : {}),
      entities: doc.entities.map((entity) => {
        const box = first.boxes.get(entity.id);
        // Containers are sized around their members; Shapes, Activities,
        // and Textboxes get the estimate as an authored minimum, or eraser
        // wraps their text at 100px. Icons, events, and tables size
        // themselves.
        const sized = CONTAINER_TAGS.has(entity.tag) || SIZED_TAGS.has(entity.tag);
        return {
          ...entity,
          x: box?.x ?? 0,
          y: box?.y ?? 0,
          ...(sized && box ? { width: box.width, height: box.height } : {}),
        };
      }),
      connections: doc.connections,
    };
    const resolved = await resolver.resolve(authored);
    if (!resolved.ok || !resolved.entities || !resolved.connections) {
      throw new Error(resolved.errors.map(describe).join("\n"));
    }
    const payload = {
      entities: resolved.entities,
      connections: resolved.connections,
      icons: resolved.icons ?? {},
    };
    const measured = await frame.api.run(payload);

    // Pass 2: the boxes the browser actually produced. A container's own
    // measure includes its pass-1 members, so it contributes only its
    // intrinsic (title) size and the layout re-derives the rest. A leaf
    // reserves its ink, not just its routable body: an Icon's caption
    // hangs below the glyph box, and a group sized to bodies alone would
    // cut it off. Ink that starts left of (or above) the body — an Event's
    // caption, centered under a 56px disc — widens the box and shifts the
    // body inside it, so the caption never runs over a lane's title band.
    const byId = new Map<string, ElementMeasure>(measured.measures.map((m) => [m.id, m]));
    const shift = new Map<string, { x: number; y: number }>();
    const second = placed(doc, kind, (entity) => {
      const measure = byId.get(entity.id);
      if (!measure) return estimateSize(entity);
      if (CONTAINER_TAGS.has(entity.tag)) {
        return { width: Math.ceil(measure.intrinsic.width), height: Math.ceil(measure.intrinsic.height) };
      }
      const body = measure.body ?? measure.intrinsic;
      const ink = measure.ink;
      const offset = { x: Math.max(0, -ink.x), y: Math.max(0, -ink.y) };
      shift.set(entity.id, offset);
      return {
        width: Math.ceil(offset.x + Math.max(body.width, ink.x + ink.width)),
        height: Math.ceil(offset.y + Math.max(body.height, ink.y + ink.height)),
      };
    });
    for (const entity of payload.entities) {
      const box = second.boxes.get(entity.id);
      if (!box) continue;
      const offset = shift.get(entity.id) ?? { x: 0, y: 0 };
      entity.x = box.x + offset.x;
      entity.y = box.y + offset.y;
      if (CONTAINER_TAGS.has(entity.tag)) {
        entity.width = box.width;
        entity.height = box.height;
      }
    }
    await frame.api.run(payload);

    const { scene, css } = frame.api.serialize();
    const element = frame.document.getElementById("eraser-scene");
    return {
      scene,
      css,
      width: Math.ceil(parseFloat(element?.style.width ?? "") || second.width),
      height: Math.ceil(parseFloat(element?.style.height ?? "") || second.height),
      warnings: resolved.warnings.map(describe),
    };
  });
}
