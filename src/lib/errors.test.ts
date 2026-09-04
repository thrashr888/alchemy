import { describe as suite, expect, it } from "vitest";
import { describe, IpcError } from "./errors";

suite("resource error descriptions", () => {
  it.each([
    "LanceError(IO): Too many open files (os error 24), /Users/build/.cargo/registry/src/lance/src/io.rs:42",
    "IO error: EMFILE",
    "IO error: ENFILE",
  ])("explains file exhaustion without leaking build paths: %s", (raw) => {
    const message = describe(new IpcError({ command: "list_sources", message: raw }));
    expect(message).toContain("open-file limit");
    expect(message).toContain("restart Alchemy");
    expect(message).not.toContain(".cargo");
    // Persisted Source.error strings use the same display path.
    expect(describe(raw)).toBe(message);
  });

  it("turns other Lance failures into a readable database error", () => {
    expect(describe("LanceError(IO): failed at /build/.cargo/registry/src/lance/io.rs:9"))
      .toContain("local database");
  });

  it("preserves specific non-database errors", () => {
    expect(describe(new Error("This file format is not supported")))
      .toBe("This file format is not supported");
  });
});
