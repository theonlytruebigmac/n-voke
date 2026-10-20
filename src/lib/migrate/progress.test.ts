import { describe, expect, it } from "vitest";
import { projectProgress } from "./progress";

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
});
