import { describe, expect, it } from "vitest";
import type { PlanRow } from "./bindings";
import { BLOCKER_WORDS, rowNote } from "./plan";

const row = (name: string, over: Partial<PlanRow> = {}): PlanRow => ({
  deviceId: name,
  name,
  customer: "Acme",
  site: null,
  destination: "Acme",
  path: "rest",
  blockers: [],
  warnings: [],
  ready: true,
  leftOut: false,
  leftOutByChoice: false,
  pathChosen: false,
  stage: null,
  ...over,
});

const rows = [
  row("PC-1"),
  row("PC-2", { customer: "Zeta", destination: null, ready: false, blockers: ["unmapped", "tokenMissing"], path: null }),
  row("MAC-3", { ready: false, blockers: ["notWindows"], path: null }),
  row("PC-4", { leftOut: true, ready: false }),
  row("PC-5", { warnings: ["queuedUntilOnline"] }),
];

describe("rowNote", () => {
  it("says ready, a warning, the blockers, or left out", () => {
    expect(rowNote(rows[0])).toEqual({ tone: "ok", text: "Ready" });
    expect(rowNote(rows[4]).tone).toBe("warn");
    expect(rowNote(rows[1])).toEqual({ tone: "err", text: "Not mapped, No token" });
    expect(rowNote(rows[3])).toEqual({ tone: "muted", text: "Left out" });
    expect(rowNote(row("PC-6", { leftOut: true, leftOutByChoice: true, ready: false }))).toEqual({ tone: "muted", text: "Left out by you" });
  });
});

describe("BLOCKER_WORDS", () => {
  it("sends a mapping or token problem to Map and a missing script to Prepare", () => {
    expect(BLOCKER_WORDS.unmapped.fix).toBe("map");
    expect(BLOCKER_WORDS.tokenMissing.fix).toBe("map");
    expect(BLOCKER_WORDS.restScriptMissing.fix).toBe("prepare");
    expect(BLOCKER_WORDS.notWindows.fix).toBeNull();
  });
});
