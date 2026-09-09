import { describe, expect, it } from "vitest";
import rag from "../../src-tauri/src/rag.rs?raw";
import { architectureSource, parseArchitecture } from "./architectureDoc";

const DOC = {
  title: "Shop",
  direction: "right",
  entities: [
    { tag: "Group", id: "backend", title: { text: "Backend" } },
    { tag: "Icon", id: "db", icon: "postgres", texts: [{ text: "Postgres" }], containerId: "backend" },
    { tag: "Shape", id: "api", texts: [{ text: "API" }], containerId: "backend", x: 40, y: 40 },
  ],
  connections: [{ from: "api", to: "db", label: "SQL" }],
};

describe("architectureSource", () => {
  it("unwraps a fence and trims prose around the object", () => {
    const wrapped = `Here is the diagram:\n\n\`\`\`json\n${JSON.stringify(DOC)}\n\`\`\`\nHope this helps.`;
    expect(JSON.parse(architectureSource(wrapped))).toEqual(DOC);
    const bare = `Sure! ${JSON.stringify(DOC)} Done.`;
    expect(JSON.parse(architectureSource(bare))).toEqual(DOC);
  });
});

describe("parseArchitecture", () => {
  it("keeps eraser's vocabulary and drops the model's coordinates", () => {
    const parsed = parseArchitecture(JSON.stringify(DOC));
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
    expect(doc.connections).toEqual(DOC.connections);
  });

  it("defaults the direction and a missing connections list", () => {
    const parsed = parseArchitecture('{"entities":[{"tag":"Shape","id":"a"}]}');
    expect(parsed.doc?.direction).toBe("down");
    expect(parsed.doc?.connections).toEqual([]);
  });

  it("names what is wrong", () => {
    expect(parseArchitecture("not json").error).toMatch(/not valid JSON/);
    expect(parseArchitecture("[]").error).toMatch(/JSON object/);
    expect(parseArchitecture('{"entities":[{"tag":"Shape"}]}').error).toMatch(/entity 0 .* "id"/);
    expect(parseArchitecture('{"entities":[],"connections":[{"from":"a"}]}').error).toMatch(
      /connection 0/,
    );
  });
});

describe("the generator's icon vocabulary", () => {
  it("matches the icons the app ships", () => {
    // The prompt in rag.rs lists the icons a model may use; the renderer
    // resolves them from src/assets/diagrams/icons. One set, two places.
    // The Rust string literal is line-continued with `\`, which drops the
    // backslash, the newline, and the indentation that follows.
    const listed = /ARCHITECTURE_ICONS: &str = "([^;]+)";/.exec(rag)?.[1];
    expect(listed, "rag.rs declares ARCHITECTURE_ICONS").toBeTruthy();
    const prompt = new Set(
      listed!
        .replace(/\\\n\s*/g, "")
        .replace(/"/g, "")
        .split(/\s+/)
        .filter(Boolean),
    );
    const shipped = new Set(
      Object.keys(import.meta.glob("../assets/diagrams/icons/*.svg")).map((p) =>
        p.replace(/^.*\//, "").replace(/\.svg$/, ""),
      ),
    );
    expect([...prompt].sort()).toEqual([...shipped].sort());
  });
});
