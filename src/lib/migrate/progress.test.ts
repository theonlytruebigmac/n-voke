import { describe, expect, it } from "vitest";
import { projectProgress, readyToFinish } from "./progress";

describe("projectProgress", () => {
  it("counts devices that are healthy or beyond, out of those being moved", () => {
    const p = projectProgress({ devices: 10, stageCounts: { healthy: 3, cleanedUp: 2, retired: 1, planned: 4 } });
    expect(p).toEqual({ total: 10, done: 6, fraction: 0.6 });
  });

  it("leaves out the devices taken out of the project", () => {
    const p = projectProgress({ devices: 10, stageCounts: { excluded: 2, healthy: 4, planned: 4 } });
    expect(p.total).toBe(8);
    expect(p.done).toBe(4);
    expect(p.fraction).toBe(0.5);
  });

  it("does not count stages that are still moving, or that need a person", () => {
    const p = projectProgress({ devices: 6, stageCounts: { staged: 1, agentRemoved: 1, registered: 1, lost: 1, notPickedUp: 1, launched: 1 } });
    expect(p.done).toBe(0);
  });

  it("is zero, not NaN, for a project with nothing to move", () => {
    expect(projectProgress({ devices: 0, stageCounts: {} })).toEqual({ total: 0, done: 0, fraction: 0 });
    expect(projectProgress({ devices: 2, stageCounts: { excluded: 2 } }).fraction).toBe(0);
  });

  it("doesn't count a healthy device that Ecoverse isn't receiving data from yet", () => {
    const p = projectProgress({ devices: 10, stageCounts: { healthy: 4, cleanedUp: 2, planned: 4 }, ecoverseWaiting: 3 });
    expect(p).toEqual({ total: 10, done: 3, fraction: 0.3 });
  });
});

describe("readyToFinish", () => {
  it("is true when everything moved is healthy or past it, with Ecoverse receiving data", () => {
    expect(readyToFinish({ devices: 3, stageCounts: { healthy: 3 } })).toBe(true);
    expect(readyToFinish({ devices: 5, stageCounts: { healthy: 2, cleanedUp: 1, resolved: 1, excluded: 1 } })).toBe(true);
  });

  it("is false while anything is planned, moving, needs the person, or waits for Ecoverse's data", () => {
    expect(readyToFinish({ devices: 3, stageCounts: { healthy: 2, planned: 1 } })).toBe(false);
    expect(readyToFinish({ devices: 3, stageCounts: { healthy: 2, staged: 1 } })).toBe(false);
    expect(readyToFinish({ devices: 3, stageCounts: { healthy: 2, lost: 1 } })).toBe(false);
    expect(readyToFinish({ devices: 3, stageCounts: { healthy: 3 }, ecoverseWaiting: 1 })).toBe(false);
  });

  it("is false for a project with nothing to move", () => {
    expect(readyToFinish({ devices: 0, stageCounts: {} })).toBe(false);
    expect(readyToFinish({ devices: 2, stageCounts: { excluded: 2 } })).toBe(false);
  });
});
