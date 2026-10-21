import { describe, expect, it } from "vitest";
import { ECO_WORDS, STAGE_WORDS, describeStage, ecoWords, eventWords, movedBanner, stageView } from "./words";

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

describe("describeStage", () => {
  it("fills the blanks and never leaves a brace in a sentence", () => {
    for (const stage of Object.keys(STAGE_WORDS) as (keyof typeof STAGE_WORDS)[]) {
      expect(describeStage(stage, { since: "34 min", destination: "Acme", reason: "No token." })).not.toMatch(/[{}]/);
      expect(describeStage(stage, { since: "" })).not.toMatch(/[{}]/);
    }
  });

  it("says how long and why where the words ask for it", () => {
    expect(describeStage("notPickedUp", { since: "34 min" })).toContain("Launched 34 min ago and still not running");
    expect(describeStage("launchFailed", { since: "", reason: "The token expired." })).toContain("refused: The token expired.");
    expect(describeStage("healthy", { since: "", destination: "Acme HQ" })).toBe("Checking in to Acme HQ.");
  });
});

describe("a healthy device and Ecoverse", () => {
  it("reads Healthy only once Ecoverse has data, and Checking in until then", () => {
    expect(stageView("healthy", false).short).toBe("Healthy");
    expect(stageView("healthy", true)).toMatchObject({ short: "Checking in", tone: "accent" });
    expect(stageView("lost", true).short).toBe("Lost");
  });

  it("says it isn't fully working while waiting, and what that means", () => {
    expect(describeStage("healthy", { since: "", destination: "Acme HQ", waitingForEcoverse: true })).toContain("Ecoverse isn't receiving its data yet");
    expect(ECO_WORDS.waitingForData.long).toContain("scripts can't run");
    for (const state of ["waitingForData", "notSeen"] as const) expect(ECO_WORDS[state].long).toContain("hourly check");
  });

  it("has words for every state, and for one not read yet", () => {
    for (const state of ["reporting", "waitingForData", "notSeen", "notLinked"] as const) expect(ecoWords(state).short).not.toBe("");
    expect(ecoWords(null).short).toBe("Ecoverse not read yet");
  });

  it("reads a stage change as the stage and why, and other events by their kind", () => {
    expect(eventWords("stage", "AgentRemoved: the old agent has stopped")).toEqual({ title: STAGE_WORDS.agentRemoved.short, body: "the old agent has stopped" });
    expect(eventWords("stage", "Planned")).toEqual({ title: STAGE_WORDS.planned.short, body: "" });
    expect(eventWords("poll", "seen")).toEqual({ title: "Checked", body: "seen" });
    expect(eventWords("custom", "x").title).toBe("Custom");
  });

  it("only says Ecoverse is receiving a device's data when Ecoverse was checked", () => {
    expect(movedBanner(true).rest).toContain("Ecoverse is receiving its data");
    const unchecked = movedBanner(false);
    expect(`${unchecked.lead} ${unchecked.rest}`).not.toMatch(/receiving its data|and is working|fully working/);
    expect(unchecked.rest).toContain("not checked");
    expect(ECO_WORDS.notLinked.short).toBe("Ecoverse not checked");
    expect(ECO_WORDS.notLinked.long).toContain("not looked at");
  });
});
