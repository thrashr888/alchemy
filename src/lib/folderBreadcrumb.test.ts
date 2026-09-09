import { describe, expect, it } from "vitest";
import { folderBreadcrumb } from "./utils";

describe("folderBreadcrumb", () => {
  it("names an iCloud app container the way Finder does", () => {
    expect(
      folderBreadcrumb(
        "/Users/paul/Library/Mobile Documents/iCloud~com~thrashr888~alchemy/Documents",
      ),
    ).toBe("iCloud Drive › Alchemy › Documents");
  });
  it("drops the container for plain iCloud Drive", () => {
    expect(
      folderBreadcrumb(
        "/Users/paul/Library/Mobile Documents/com~apple~CloudDocs/Notebooks",
      ),
    ).toBe("iCloud Drive › Notebooks");
  });
  it("names File Provider mounts by provider", () => {
    expect(
      folderBreadcrumb(
        "/Users/paul/Library/CloudStorage/Dropbox-Personal/Alchemy",
      ),
    ).toBe("Dropbox › Alchemy");
  });
  it("shortens a plain path to ~", () => {
    expect(folderBreadcrumb("/Users/paul/Documents/Alchemy")).toBe(
      "~/Documents/Alchemy",
    );
  });
});
