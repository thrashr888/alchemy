import { describe, expect, it } from "vitest";
import rag from "../../src-tauri/src/rag.rs?raw";
import {
  DIAGRAM_KINDS,
  KIND_TAGS,
  diagramSource,
  parseDiagram,
  prepareForRender,
  type DiagramKind,
} from "./diagramDoc";

const ARCHITECTURE = {
  title: "Shop",
  direction: "right",
  entities: [
    { tag: "Group", id: "backend", title: { text: "Backend" } },
    { tag: "Icon", id: "db", icon: "postgres", texts: [{ text: "Postgres" }], containerId: "backend" },
    { tag: "Shape", id: "api", texts: [{ text: "API" }], containerId: "backend", x: 40, y: 40 },
  ],
  connections: [{ from: "api", to: "db", label: "SQL" }],
};

/** One valid document per kind, in the vocabulary its prompt teaches. */
const GOOD: Record<DiagramKind, { entities: unknown[]; connections?: unknown[] }> = {
  architecture: ARCHITECTURE,
  process: {
    entities: [
      { tag: "Pool", id: "acme", title: { text: "Acme" } },
      { tag: "Lane", id: "customer", title: { text: "Customer" }, containerId: "acme" },
      { tag: "Lane", id: "support", title: { text: "Support" }, containerId: "acme" },
      { tag: "Event", id: "start", texts: [{ text: "Ticket opened" }], containerId: "customer" },
      { tag: "Activity", id: "triage", texts: [{ text: "Triage" }], containerId: "support" },
      { tag: "Gateway", id: "known", texts: [{ text: "Known?" }], containerId: "support" },
      { tag: "Event", id: "end", texts: [{ text: "Closed" }], color: "red", containerId: "customer" },
      { tag: "Textbox", id: "n1", text: "SLA 4h" },
    ],
    connections: [
      { from: "start", to: "triage" },
      { from: "triage", to: "known" },
      { from: "known", to: "end", label: "yes" },
    ],
  },
  data_model: {
    entities: [
      { tag: "Group", id: "billing", title: { text: "Billing" } },
      {
        tag: "DatabaseTable",
        id: "orders",
        label: "orders",
        fields: [
          { name: "id", type: "uuid", meta: "PK" },
          { name: "customer_id", type: "uuid", meta: "FK" },
        ],
        containerId: "billing",
      },
      { tag: "DatabaseTable", id: "customers", label: "customers", fields: [{ name: "id" }] },
      { tag: "Textbox", id: "n1", text: "Soft deletes" },
    ],
    connections: [
      { from: "orders", to: "customers", label: "n..1" },
      { tag: "DatabaseRelationship", from: "customers", to: "orders", relType: "one-to-many" },
    ],
  },
  relationship: {
    entities: [
      { tag: "Group", id: "alex", title: { text: "Alexandria" } },
      { tag: "Icon", id: "zosimos", icon: "user", texts: [{ text: "Zosimos" }], containerId: "alex" },
      { tag: "Shape", id: "cheirokmeta", shape: "document", texts: [{ text: "Cheirokmeta" }] },
      { tag: "Textbox", id: "n1", text: "Dates disputed" },
    ],
    connections: [{ from: "zosimos", to: "cheirokmeta", label: "wrote" }],
  },
  journey: {
    entities: [
      { tag: "Event", id: "start", texts: [{ text: "Needs a stand" }] },
      { tag: "Group", id: "discover", title: { text: "1. Discover" } },
      { tag: "Shape", id: "search", texts: [{ text: "Searches" }], containerId: "discover" },
      { tag: "Textbox", id: "p1", text: "Frustrated: prices hidden", containerId: "discover" },
      { tag: "Group", id: "buy", title: { text: "2. Buy" } },
      { tag: "Shape", id: "checkout", texts: [{ text: "Checks out" }], containerId: "buy" },
    ],
    connections: [
      { from: "start", to: "search" },
      { from: "search", to: "checkout" },
    ],
  },
};

/** For each kind, one stock tag another kind owns. */
const WRONG: Record<DiagramKind, string> = {
  architecture: "DatabaseTable",
  process: "Shape",
  data_model: "Icon",
  relationship: "Activity",
  journey: "DatabaseTable",
};

describe("diagramSource", () => {
  it("unwraps a fence and trims prose around the object", () => {
    const wrapped = `Here is the diagram:\n\n\`\`\`json\n${JSON.stringify(ARCHITECTURE)}\n\`\`\`\nHope this helps.`;
    expect(JSON.parse(diagramSource(wrapped))).toEqual(ARCHITECTURE);
    const bare = `Sure! ${JSON.stringify(ARCHITECTURE)} Done.`;
    expect(JSON.parse(diagramSource(bare))).toEqual(ARCHITECTURE);
  });
});

describe("parseDiagram", () => {
  it("keeps eraser's vocabulary and drops the model's coordinates", () => {
    const parsed = parseDiagram(JSON.stringify(ARCHITECTURE), "architecture");
    expect(parsed.error).toBeUndefined();
    const doc = parsed.doc!;
    expect(doc.direction).toBe("right");
    expect(doc.title).toBe("Shop");
    expect(doc.entities[2]).toEqual({
      tag: "Shape",
      id: "api",
      texts: [{ text: "API" }],
      containerId: "backend",
    });
    expect(doc.connections).toEqual(ARCHITECTURE.connections);
  });

  it("defaults the direction per kind and a missing connections list", () => {
    const one = parseDiagram('{"entities":[{"tag":"Shape","id":"a"}]}', "architecture");
    expect(one.doc?.direction).toBe("down");
    expect(one.doc?.connections).toEqual([]);
    expect(parseDiagram('{"entities":[]}', "journey").doc?.direction).toBe("right");
    expect(parseDiagram('{"entities":[]}', "process").doc?.direction).toBe("down");
    expect(
      parseDiagram('{"entities":[],"direction":"sideways"}', "data_model").doc?.direction,
    ).toBe("right");
  });

  it("names what is wrong", () => {
    expect(parseDiagram("not json", "architecture").error).toMatch(/not valid JSON/);
    expect(parseDiagram("[]", "architecture").error).toMatch(/JSON object/);
    expect(parseDiagram('{"entities":[{"tag":"Shape"}]}', "architecture").error).toMatch(
      /entity 0 .* "id"/,
    );
    expect(
      parseDiagram('{"entities":[],"connections":[{"from":"a"}]}', "architecture").error,
    ).toMatch(/connection 0/);
  });

  describe.each(DIAGRAM_KINDS)("%s", (kind) => {
    it("accepts a document in its own vocabulary", () => {
      const parsed = parseDiagram(JSON.stringify(GOOD[kind]), kind);
      expect(parsed.error).toBeUndefined();
      expect(parsed.doc?.entities.length).toBe(GOOD[kind].entities.length);
    });

    it("rejects a tag another kind owns, naming the entity and its own tags", () => {
      const wrong = { entities: [{ tag: WRONG[kind], id: "stray" }] };
      const parsed = parseDiagram(JSON.stringify(wrong), kind);
      expect(parsed.error).toMatch(new RegExp(`"stray" has tag ${WRONG[kind]}`));
      for (const tag of KIND_TAGS[kind].entities) expect(parsed.error).toContain(tag);
      expect(parseDiagram('{"entities":[{"tag":"Nonsense","id":"x"}]}', kind).error).toMatch(
        /Nonsense/,
      );
    });

    it("rejects a connection tag it does not use", () => {
      const doc = { entities: [], connections: [{ tag: "Bogus", from: "a", to: "b" }] };
      expect(parseDiagram(JSON.stringify(doc), kind).error).toMatch(/connection 0 has tag Bogus/);
    });
  });
});

describe("a relationship map's notes", () => {
  it("drops a connection into a Textbox with a warning and keeps the rest", () => {
    const doc = {
      entities: [
        { tag: "Icon", id: "zosimos", icon: "user", texts: [{ text: "Zosimos" }] },
        { tag: "Shape", id: "cheirokmeta", shape: "document", texts: [{ text: "Cheirokmeta" }] },
        { tag: "Textbox", id: "n1", text: "Dates disputed" },
      ],
      connections: [
        { from: "zosimos", to: "cheirokmeta", label: "wrote" },
        { from: "cheirokmeta", to: "n1", label: "see" },
        { from: "n1", to: "zosimos" },
      ],
    };
    const parsed = parseDiagram(JSON.stringify(doc), "relationship");
    expect(parsed.error).toBeUndefined();
    expect(parsed.doc?.connections).toEqual([doc.connections[0]]);
    expect(parsed.warnings).toHaveLength(2);
    expect(parsed.warnings?.[0]).toMatch(/"cheirokmeta" → "n1" dropped: "n1" is a Textbox/);
    expect(parsed.warnings?.[1]).toMatch(/"n1" → "zosimos" dropped/);
    // Other kinds keep their Textbox lines: an architecture note may point
    // at the component it annotates.
    const arch = parseDiagram(JSON.stringify({ ...doc, entities: doc.entities }), "architecture");
    expect(arch.doc?.connections).toHaveLength(3);
    expect(arch.warnings).toEqual([]);
  });

  it("defaults a relationship map to run right", () => {
    expect(parseDiagram('{"entities":[]}', "relationship").doc?.direction).toBe("right");
  });
});

describe("prepareForRender", () => {
  it("turns a data model's cardinality labels into crow's feet and leaves the rest", () => {
    const doc = parseDiagram(JSON.stringify(GOOD.data_model), "data_model").doc!;
    const ready = prepareForRender(doc, "data_model");
    expect(ready.connections[0]).toEqual({
      from: "orders",
      to: "customers",
      label: "n..1",
      tag: "DatabaseRelationship",
      relType: "many-to-one",
    });
    expect(ready.connections[1].relType).toBe("one-to-many");
    // The stored document is untouched.
    expect(doc.connections[0].tag).toBeUndefined();
    const plain = { entities: [], connections: [{ from: "a", to: "b", label: "has" }] };
    const kept = prepareForRender(
      parseDiagram(JSON.stringify(plain), "data_model").doc!,
      "data_model",
    );
    expect(kept.connections[0].tag).toBeUndefined();
    expect(prepareForRender(doc, "architecture").connections[0].tag).toBeUndefined();
  });

  it("reads the cardinality spellings the prompt allows", () => {
    const rel = (label: string) =>
      prepareForRender(
        { direction: "right", entities: [], connections: [{ from: "a", to: "b", label }] },
        "data_model",
      ).connections[0].relType;
    expect(rel("1..1")).toBe("one-to-one");
    expect(rel("1..n")).toBe("one-to-many");
    expect(rel("n..n")).toBe("many-to-many");
    expect(rel("many-to-one")).toBe("many-to-one");
    expect(rel("1:n")).toBe("one-to-many");
  });
});

/** A Rust string literal's text, with `\`-continuations joined. */
function rustLiteral(name: string): string {
  const m = new RegExp(`${name}: &str = "([^;]+)";`).exec(rag);
  expect(m, `rag.rs declares ${name}`).toBeTruthy();
  return m![1].replace(/\\\n\s*/g, "").replace(/"/g, "");
}

describe("the generator's vocabulary", () => {
  it("names the same tags per kind as the parser admits", () => {
    // rag.rs DIAGRAM_TAGS: ("kind", "Entity Tags", "Connection Tags") per kind.
    const block = /DIAGRAM_TAGS: &\[\(&str, &str, &str\)\] = &\[([\s\S]*?)\n\];/.exec(rag)?.[1];
    expect(block, "rag.rs declares DIAGRAM_TAGS").toBeTruthy();
    const declared = new Map<string, { entities: string[]; connections: string[] }>();
    for (const m of block!.matchAll(/\(\s*"([a-z_]+)",\s*"([^"]+)",\s*"([^"]+)",?\s*\)/g)) {
      declared.set(m[1], { entities: m[2].split(/\s+/), connections: m[3].split(/\s+/) });
    }
    expect([...declared.keys()].sort()).toEqual([...DIAGRAM_KINDS].sort());
    for (const kind of DIAGRAM_KINDS) {
      expect(declared.get(kind)?.entities.sort()).toEqual([...KIND_TAGS[kind].entities].sort());
      expect(declared.get(kind)?.connections.sort()).toEqual(
        [...KIND_TAGS[kind].connections].sort(),
      );
    }
  });

  it("matches the icons the app ships", () => {
    // The prompt in rag.rs lists the icons a model may use; the renderer
    // resolves them from src/assets/diagrams/icons. One set, two places.
    const prompt = new Set(rustLiteral("DIAGRAM_ICONS").split(/\s+/).filter(Boolean));
    const shipped = new Set(
      Object.keys(import.meta.glob("../assets/diagrams/icons/*.svg")).map((p) =>
        p.replace(/^.*\//, "").replace(/\.svg$/, ""),
      ),
    );
    expect([...prompt].sort()).toEqual([...shipped].sort());
  });

  it("names icons the renderer has when a prompt suggests one by name", () => {
    // The relationship prompt names icons for people, organizations,
    // places, and projects.
    const shipped = new Set(rustLiteral("DIAGRAM_ICONS").split(/\s+/));
    for (const icon of ["user", "users", "building", "globe", "folder"])
      expect(shipped.has(icon), icon).toBe(true);
  });
});
