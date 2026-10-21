import { describe, expect, it } from "vitest";
import type { TrackRow } from "./bindings";
import { ACTION_WORDS, GROUPS, ago, filterTrack, groupCounts, groupOf, needsYou } from "./track";

const row = (name: string, stage: TrackRow["stage"], over: Partial<TrackRow> = {}): TrackRow => ({
  index: 0,
  name,
  customer: "Acme",
  destination: "Acme",
  wave: 1,
  stage,
  since: "2026-10-20T14:00:00Z",
  path: "rest",
  needsMatch: false,
  lastError: null,
  ecoverse: null,
  waitingForEcoverse: false,
  ...over,
});

describe("groups", () => {
  it("put every stage in exactly one group", () => {
    const all = GROUPS.flatMap((g) => g.stages);
    expect(new Set(all).size).toBe(all.length);
    expect(all).toHaveLength(15); // the 16 stages but excluded, which isn't tracked
    expect(GROUPS.find((g) => g.id === "waiting")?.stages).toEqual([]); // derived: it is a healthy device waiting for Ecoverse
  });

  it("count devices by group and leave out the ones taken out of the project", () => {
    const counts = groupCounts({ planned: 4, healthy: 3, cleanedUp: 1, retired: 2, lost: 1, notPickedUp: 1, excluded: 9 });
    expect(Object.fromEntries(counts.map((c) => [c.id, c.count]))).toMatchObject({ planned: 4, healthy: 3, done: 3, needsYou: 2, launched: 0 });
  });

  it("move healthy devices that are waiting for Ecoverse out of Healthy and into their own group", () => {
    const counts = Object.fromEntries(groupCounts({ healthy: 5 }, 2).map((c) => [c.id, c.count]));
    expect(counts).toMatchObject({ healthy: 3, waiting: 2 });
    expect(Object.fromEntries(groupCounts({ healthy: 1 }, 5).map((c) => [c.id, c.count])).healthy).toBe(0);
    expect(groupOf("healthy", true)).toBe("waiting");
    expect(groupOf("healthy", false)).toBe("healthy");
    expect(groupOf("lost", true)).toBe("needsYou");
  });

  it("read a stage's group", () => {
    expect(groupOf("agentRemoved")).toBe("moving");
    expect(groupOf("excluded")).toBeNull();
  });
});

describe("filterTrack", () => {
  const rows = [row("PC-1", "healthy"), row("PC-2", "lost", { wave: 2, customer: "Zeta" }), row("PC-3", "healthy", { needsMatch: true })];
  const all = { group: null, wave: null, customer: null, text: "" } as const;

  it("narrows by group, wave, customer and text", () => {
    expect(filterTrack(rows, { ...all, group: "healthy" }).map((r) => r.name)).toEqual(["PC-1", "PC-3"]);
    expect(filterTrack(rows, { ...all, wave: 2 }).map((r) => r.name)).toEqual(["PC-2"]);
    expect(filterTrack(rows, { ...all, customer: "Zeta" }).map((r) => r.name)).toEqual(["PC-2"]);
    expect(filterTrack(rows, { ...all, text: "pc-3" }).map((r) => r.name)).toEqual(["PC-3"]);
  });

  it("filters a healthy device by whether Ecoverse is receiving its data", () => {
    const mixed = [row("A", "healthy"), row("B", "healthy", { waitingForEcoverse: true, ecoverse: "waitingForData" })];
    expect(filterTrack(mixed, { ...all, group: "healthy" }).map((r) => r.name)).toEqual(["A"]);
    expect(filterTrack(mixed, { ...all, group: "waiting" }).map((r) => r.name)).toEqual(["B"]);
  });

  it("counts a device with a match to choose under needs you", () => {
    expect(filterTrack(rows, { ...all, group: "needsYou" }).map((r) => r.name)).toEqual(["PC-2", "PC-3"]);
    expect(needsYou(rows[2])).toBe(true);
    expect(needsYou(rows[0])).toBe(false);
  });
});

describe("ago", () => {
  const at = Date.parse("2026-10-20T14:00:00Z");
  it("reads minutes, hours and days", () => {
    expect(ago("2026-10-20T14:00:00Z", at + 20_000)).toBe("under a minute");
    expect(ago("2026-10-20T14:00:00Z", at + 34 * 60_000)).toBe("34 min");
    expect(ago("2026-10-20T14:00:00Z", at + 130 * 60_000)).toBe("2 h");
    expect(ago("2026-10-20T14:00:00Z", at + 3 * 86_400_000)).toBe("3 d");
    expect(ago("nonsense", at)).toBe("");
  });
});

describe("ACTION_WORDS", () => {
  it("asks before anything that changes a device and not before a probe", () => {
    expect(ACTION_WORDS.probe.confirm).toBeNull();
    expect(ACTION_WORDS.cleanup.confirm).toContain("can't be undone");
    expect(ACTION_WORDS.markResolved.confirm).not.toBeNull();
  });
});
