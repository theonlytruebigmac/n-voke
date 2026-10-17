import { describe, expect, it } from "vitest";
import { MAX_STORED_OUTPUT, capOutput, retain } from "./retention";
import type { JobTask } from "./taskState";

const DAY = 24 * 60 * 60 * 1000;
const NOW = Date.parse("2026-10-17T12:00:00Z");

function task(watching: boolean, stdOutput: string | null = null): JobTask {
  return {
    taskId: "t",
    assetId: "a",
    deviceName: "PC",
    taskStatus: watching ? "IN_PROGRESS" : "SUCCEEDED",
    execution: stdOutput === null ? null : { id: "e", status: "SUCCEEDED", exitCode: null, errorMessage: null, startedAt: null, updatedAt: null, durationMilliseconds: 1, output: { runAs: null, stdOutput, stdError: null, outputs: null } },
    watching,
    watchStartedAt: 0,
    pollFailures: 0,
    lastError: null,
    stopReason: null,
  };
}

const job = (id: string, ageDays: number, watching = false) => ({ id, createdAt: new Date(NOW - ageDays * DAY).toISOString(), tasks: [task(watching)] });

describe("retain", () => {
  it("drops finished jobs older than the retention window", () => {
    const kept = retain([job("new", 1), job("old", 40), job("edge", 29.9)], 30, NOW);
    expect(kept.map((j) => j.id)).toEqual(["new", "edge"]);
  });

  it("always keeps jobs that are still being watched", () => {
    expect(retain([job("old-but-running", 400, true)], 7, NOW).map((j) => j.id)).toEqual(["old-but-running"]);
  });

  it("caps the number of jobs without dropping ones still being watched", () => {
    const jobs = [job("a", 0), job("b", 0), job("c", 0, true), job("d", 0, true), job("e", 0)];
    expect(retain(jobs, 30, NOW, 2).map((j) => j.id)).toEqual(["a", "b", "c", "d"]);
  });

  it("keeps nothing finished when history isn't saved", () => {
    expect(retain([job("a", 0), job("b", 0, true)], 0, NOW).map((j) => j.id)).toEqual(["b"]);
  });
});

describe("capOutput", () => {
  it("truncates very long output and leaves short output alone", () => {
    const long = "x".repeat(MAX_STORED_OUTPUT + 500);
    const capped = capOutput({ createdAt: "", tasks: [task(false, long), task(false, "short")] });
    expect(capped.tasks[0].execution!.output!.stdOutput!.length).toBeLessThan(long.length);
    expect(capped.tasks[0].execution!.output!.stdOutput).toContain("truncated");
    expect(capped.tasks[1].execution!.output!.stdOutput).toBe("short");
  });
});
