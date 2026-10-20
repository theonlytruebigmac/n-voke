import { describe, expect, it } from "vitest";
import { STAGE_WORDS } from "./words";

describe("STAGE_WORDS", () => {
  const entries = Object.entries(STAGE_WORDS);

  it("gives every stage a word and a sentence", () => {
    expect(entries.length).toBe(16);
    for (const [stage, w] of entries) {
      expect(w.short.trim(), stage).not.toBe("");
      expect(w.long.trim(), stage).not.toBe("");
    }
  });

  it("uses a different word for each stage, so a table never reads ambiguously", () => {
    const words = entries.map(([, w]) => w.short);
    expect(new Set(words).size).toBe(words.length);
  });

  it("only pulses for stages where something is happening now", () => {
    const pulsing = entries.filter(([, w]) => w.pulse).map(([s]) => s).sort();
    expect(pulsing).toEqual(["agentRemoved", "launched", "registered", "staged"]);
  });

  it("colours problems amber or red and finished work green", () => {
    for (const stage of ["unreachable", "launchFailed", "lost"]) expect(STAGE_WORDS[stage as keyof typeof STAGE_WORDS].tone).toBe("err");
    for (const stage of ["notPickedUp", "stuckStaged", "wrongPlace"]) expect(STAGE_WORDS[stage as keyof typeof STAGE_WORDS].tone).toBe("warn");
    for (const stage of ["healthy", "cleanedUp", "retired"]) expect(STAGE_WORDS[stage as keyof typeof STAGE_WORDS].tone).toBe("ok");
  });
});
