import { describe, expect, it } from "vitest";
import { parseGenerateIntent } from "./slashCommands";

describe("parseGenerateIntent", () => {
  it("reads a generate sentence as the generator plus its instructions", () => {
    const p = parseGenerateIntent(
      "generate a slide deck based on TEC-4577 @TEC-4577 — Story outline v2",
    );
    expect(p?.cmd.name).toBe("slide_deck");
    expect(p?.arg).toBe("TEC-4577 @TEC-4577 — Story outline v2");
  });

  it("accepts the spoken names and other verbs", () => {
    expect(parseGenerateIntent("make me a deck about Q3")?.cmd.name).toBe("slide_deck");
    expect(parseGenerateIntent("Write a podcast on the merger")?.cmd.name).toBe(
      "audio_overview",
    );
    expect(parseGenerateIntent("create a study guide")?.cmd.name).toBe("study_guide");
    expect(parseGenerateIntent("please build an FAQ for onboarding")?.cmd.name).toBe("faq");
  });

  it("drops the joining word and keeps the rest verbatim", () => {
    expect(parseGenerateIntent("generate a summary of the tax sources")?.arg).toBe(
      "the tax sources",
    );
    expect(parseGenerateIntent("generate a summary")?.arg).toBe("");
  });

  it("leaves questions and unrelated sentences alone", () => {
    expect(parseGenerateIntent("what would a slide deck need?")).toBeNull();
    expect(parseGenerateIntent("generate revenue ideas")).toBeNull();
    expect(parseGenerateIntent("make a decking plan")).toBeNull();
    expect(parseGenerateIntent("/slide_deck x")).toBeNull();
  });
});
