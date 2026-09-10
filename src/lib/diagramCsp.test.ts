/**
 * Diagrams under the release Content Security Policy (docs/RFC-diagrams.md).
 *
 * The signed app runs with `script-src 'self'`, which forbids `new Function`
 * and `eval`. The eraser resolver validates with Ajv, which compiles every
 * schema that way — so v0.59.0 drew no diagram at all, while dev builds
 * (no CSP on Vite's page) kept passing. This suite stubs the evaluators
 * out the way the WebView refuses them and runs the real resolver over
 * every harness sample through the entry the app uses.
 *
 * Covered here: prepareForRender, our placement, the resolver's warm-up
 * (library check, schema compile, policy tables) and its full pipeline —
 * validation, normalizing, cross-references, sanitizing, icon inlining,
 * colors. Not covered: the render frame (`@eraserlabs/render/browser`
 * needs a DOM) and the second placement pass on measured sizes;
 * `pnpm run diagram:check-csp` greps the built bundle for those, and the
 * harness renders the same samples in a browser.
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { isDiagramKind, parseDiagram, type DiagramKind } from "./diagramDoc";
import { resolveDiagram } from "./eraserDiagram";
import { Ajv } from "./diagramValidators";
import { TAGS } from "./diagramValidators.gen.js";
import generated from "./diagramValidators.gen.js?raw";

const SAMPLES = import.meta.glob("../../scripts/diagram-harness/samples/*.json", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const refused = (): never => {
  throw new EvalError(
    "Refused to evaluate a string as JavaScript because 'unsafe-eval' is not an allowed source of script (test stand-in for script-src 'self')",
  );
};

const original = {
  Function: globalThis.Function,
  eval: globalThis.eval,
  setTimeout: globalThis.setTimeout,
  setInterval: globalThis.setInterval,
};

beforeAll(() => {
  // `new Function(...)` and `Function(...)` both run the body; instances
  // keep the real prototype so `instanceof Function` and `.call/.bind`
  // on existing functions still work for the code under test.
  const Stub = function Function() {
    refused();
  } as unknown as FunctionConstructor;
  Object.defineProperty(Stub, "prototype", { value: original.Function.prototype });
  globalThis.Function = Stub;
  globalThis.eval = refused as unknown as typeof eval;
  globalThis.setTimeout = ((handler: unknown, ...rest: unknown[]) =>
    typeof handler === "string"
      ? refused()
      : (original.setTimeout as (...args: unknown[]) => unknown)(handler, ...rest)) as typeof setTimeout;
  globalThis.setInterval = ((handler: unknown, ...rest: unknown[]) =>
    typeof handler === "string"
      ? refused()
      : (original.setInterval as (...args: unknown[]) => unknown)(handler, ...rest)) as typeof setInterval;
});

afterAll(() => {
  globalThis.Function = original.Function;
  globalThis.eval = original.eval;
  globalThis.setTimeout = original.setTimeout;
  globalThis.setInterval = original.setInterval;
});

describe("the CSP stand-in", () => {
  it("refuses string evaluation", () => {
    expect(() => new Function("return 1")).toThrow(EvalError);
    expect(() => eval("1")).toThrow(EvalError);
    expect(() => setTimeout("1", 0)).toThrow(EvalError);
  });
});

describe("every harness sample resolves without evaluating code", () => {
  const files = Object.keys(SAMPLES).sort();
  expect(files.length).toBeGreaterThan(0);

  for (const file of files) {
    it(file.replace(/^.*\//, ""), async () => {
      const content = SAMPLES[file];
      const declared = (JSON.parse(content) as { kind?: string }).kind ?? "architecture";
      const kind: DiagramKind = isDiagramKind(declared) ? declared : "architecture";
      const parsed = parseDiagram(content, kind);
      if (!parsed.doc) throw new Error(parsed.error);

      const resolved = await resolveDiagram(parsed.doc, kind);
      expect(resolved.payload.entities.length).toBe(parsed.doc.entities.length);
      expect(resolved.payload.connections.length).toBe(parsed.doc.connections.length);
    });
  }
});

describe("the precompiled validators behave like the compiler's", () => {
  const doc = (entity: Record<string, unknown>) => ({
    direction: "down" as const,
    entities: [{ tag: "Shape", id: "a", texts: [{ text: "A" }], ...entity }],
    connections: [],
  });

  it("drop an unknown property with a warning (additionalProperties + suggestion)", async () => {
    const resolved = await resolveDiagram(doc({ shapee: "cylinder" }), "architecture");
    expect(resolved.warnings).toEqual([expect.stringMatching(/Unknown property "shapee".*\(shape\)/)]);
    expect(resolved.payload.entities[0].props).not.toHaveProperty("shapee");
  });

  it("fill schema defaults (useDefaults)", async () => {
    const resolved = await resolveDiagram(doc({}), "architecture");
    expect(resolved.payload.entities[0]).toMatchObject({
      props: { shape: "rectangle", vAlign: "middle" },
    });
  });

  it("report every violation, with the allowed values (allErrors + enum)", async () => {
    await expect(
      resolveDiagram(doc({ shape: "blob", vAlign: "sideways" }), "architecture"),
    ).rejects.toThrow(/Invalid value "blob"[\s\S]*Invalid value "sideways"[\s\S]*expected one of/);
  });
});

describe("the precompiled validators", () => {
  it("cover every tag in the stock library", async () => {
    const { stockLibrary } = await import("@eraserlabs/diagrams/library");
    for (const tag of Object.keys(stockLibrary.schemas)) {
      expect(TAGS, `tag ${tag} — run pnpm run diagram:validators`).toHaveProperty(tag);
    }
    expect(TAGS).toHaveProperty("tag-schema");
  });

  it("contain no code evaluation", () => {
    expect(generated).not.toMatch(/new Function\b/);
    expect(generated).not.toMatch(/(^|[^\w$.])eval\(/);
    expect(generated).not.toMatch(/Error compiling schema/);
  });

  it("refuse a schema they were not built from, naming the fix", () => {
    const ajv = new Ajv({ allErrors: true });
    expect(() => ajv.compile({ type: "object", properties: { tag: { const: "Nope" } } })).toThrow(
      /no precompiled diagram validator for schema "Nope".*diagram:validators/,
    );
  });
});
