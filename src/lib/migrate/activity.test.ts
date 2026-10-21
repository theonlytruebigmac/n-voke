import { describe, expect, it } from "vitest";
import type { LaunchReport } from "./bindings";
import { jobsFromLaunch } from "./activity";

const report = (activity: LaunchReport["activity"]): LaunchReport => ({ launchedEcoverse: 2, launchedRest: 0, fellBack: 0, heldBack: 0, failed: [], stopped: null, activity });

describe("jobsFromLaunch", () => {
  it("makes one watched job per Ecoverse run, with a task per device", () => {
    const jobs = jobsFromLaunch(
      report([{ name: "N-voke migration: launch wave 1", scriptId: "s1", scriptName: "N-voke migration", targetLabel: "wave 1", timeoutSeconds: 900, tasks: [["t1", "a1", "PC-1"], ["t2", "", "PC-2"]] }]),
      new Date("2026-10-20T20:00:00Z"),
      () => "job-1",
    );
    expect(jobs).toHaveLength(1);
    expect(jobs[0]).toMatchObject({ id: "job-1", name: "N-voke migration: launch wave 1", scriptId: "s1", timeoutSeconds: 900, createdAt: "2026-10-20T20:00:00.000Z" });
    expect(jobs[0].tasks.map((t) => [t.taskId, t.assetId, t.deviceName, t.watching])).toEqual([["t1", "a1", "PC-1", true], ["t2", null, "PC-2", true]]);
  });

  it("makes nothing for a launch Activity can't watch", () => {
    expect(jobsFromLaunch(report([]))).toEqual([]);
  });
});
