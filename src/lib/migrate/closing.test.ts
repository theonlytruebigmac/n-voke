import { describe, expect, it } from "vitest";
import type { FinishView } from "./bindings";
import { unfinishedParts, whatKeepsItOpen } from "./closing";

const view = (over: Partial<FinishView> = {}): FinishView => ({
  total: 3,
  healthy: 0,
  cleanedUp: 0,
  retired: 0,
  resolved: 0,
  moving: 0,
  waiting: 0,
  needsYou: 0,
  closed: false,
  closedAt: null,
  reopenedAt: null,
  unfinished: 0,
  toRetire: 0,
  nextReviewEnds: null,
  reviewDays: 1,
  retireSourceRecords: false,
  cleanupDevices: [],
  waitingForEcoverse: [],
  ...over,
});

describe("what keeps a project open", () => {
  it("names the old records still to retire, with when the first review period ends", () => {
    const text = whatKeepsItOpen(view({ cleanedUp: 3, toRetire: 3, unfinished: 3, nextReviewEnds: "2026-10-24T12:00:00Z", retireSourceRecords: true }));
    expect(text).toMatch(/^It closes by itself once every device is finished\. Left: 3 old records to retire \(the first review period ends .+\)\.$/);
  });

  it("lists everything left in the order it is dealt with", () => {
    expect(unfinishedParts(view({ needsYou: 1, moving: 2, waiting: 4, healthy: 1, toRetire: 2 }))).toEqual(["1 needs you", "2 still moving", "4 not launched yet", "1 healthy device to clean up", "2 old records to retire"]);
    expect(whatKeepsItOpen(view({ moving: 2, healthy: 5 }))).toBe("It closes by itself once every device is finished. Left: 2 still moving and 5 healthy devices to clean up.");
  });

  it("doesn't promise to close by itself once it was reopened", () => {
    expect(whatKeepsItOpen(view({ reopenedAt: "2026-10-24T12:00:00Z" }))).toBe("You reopened it with every device finished, so it stays open until you close it.");
    expect(whatKeepsItOpen(view({ reopenedAt: "2026-10-24T12:00:00Z", moving: 1 }))).toBe("It stays open until you close it, since you reopened it. Left: 1 still moving.");
  });

  it("says so when there are no devices yet", () => {
    expect(whatKeepsItOpen(view({ total: 0 }))).toBe("There are no devices in this project yet.");
  });
});
