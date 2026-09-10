import { describe, expect, it } from "vitest";
import { splitFrontmatter } from "./utils";

describe("splitFrontmatter", () => {
  it("lifts a leading block into properties and leaves the body", () => {
    const { meta, body } = splitFrontmatter(
      '---\ntitle: "Architecture"\ntags: [markdown]\nalchemy:\n  id: "abc"\n  device: "Mac"\n---\n\n# Heading\n',
    );
    expect(meta).toEqual([
      ["title", "Architecture"],
      ["tags", "[markdown]"],
      ["alchemy", 'id: "abc" device: "Mac"'],
    ]);
    expect(body).toBe("\n# Heading\n");
  });
  it("passes a document without frontmatter through", () => {
    expect(splitFrontmatter("# Plain\n")).toEqual({ meta: [], body: "# Plain\n" });
  });
  it("does not mistake a horizontal rule mid-document for frontmatter", () => {
    const text = "intro\n\n---\n\nmore\n";
    expect(splitFrontmatter(text).body).toBe(text);
  });
});
