import { describe, expect, it } from "vitest";
import { applySnapshot, countPhases, finalStatus, groupHeadline, groupResults, outcomeLabel, taskOutcome, taskPhase, type JobTask } from "./taskState";
import type { Execution } from "./types";

function task(deviceName: string, overrides: Partial<JobTask> = {}): JobTask {
  return {
    taskId: `t-${deviceName}`,
    assetId: `a-${deviceName}`,
    deviceName,
    taskStatus: "IN_PROGRESS",
    execution: null,
    watching: true,
    watchStartedAt: 0,
    pollFailures: 0,
    lastError: null,
    stopReason: null,
    ...overrides,
  };
}

function finished(deviceName: string, status: string, stdOutput: string, errorMessage: string | null = null): JobTask {
  const execution: Execution = {
    id: `e-${deviceName}`,
    status,
    exitCode: null, // N-able never fills this in for script runs
    errorMessage,
    startedAt: null,
    updatedAt: null,
    durationMilliseconds: 1000,
    output: { runAs: null, stdOutput, stdError: null, outputs: null },
  };
  return task(deviceName, { taskStatus: status, execution, watching: false });
}

describe("taskPhase", () => {
  it("distinguishes waiting, running, finished and unwatched tasks", () => {
    expect(taskPhase(task("a"))).toBe("queued");
    expect(taskPhase(task("a", { execution: { ...finished("a", "RUNNING", "").execution!, status: "RUNNING" } }))).toBe("running");
    expect(taskPhase(finished("a", "SUCCEEDED", "ok"))).toBe("succeeded");
    expect(taskPhase(finished("a", "FAILED", ""))).toBe("failed");
    expect(taskPhase(finished("a", "FAILED", "report", "something went wrong while parsing the PowerShell output"))).toBe("nonZero");
    expect(taskPhase(task("a", { watching: false }))).toBe("stopped");
  });

  it("treats a task that ended without an execution as finished", () => {
    expect(taskPhase(task("a", { taskStatus: "EXPIRED" }))).toBe("failed");
  });
});

describe("applySnapshot", () => {
  it("stops watching once the execution finishes", () => {
    const done = finished("a", "SUCCEEDED", "ok");
    const next = applySnapshot(task("a"), {
      id: "t-a",
      name: "run",
      status: "SUCCEEDED",
      createdAt: null,
      executions: { nodes: [done.execution] },
    });
    expect(next.watching).toBe(false);
    expect(next.execution?.status).toBe("SUCCEEDED");
  });

  const snapshot = (taskStatus: string, executionStatus: string) => ({
    id: "t-a",
    status: taskStatus,
    createdAt: null,
    executions: { nodes: [{ ...finished("a", executionStatus, "").execution!, output: null }] },
  });

  it("stops watching a task cancelled before the device started it", () => {
    const next = applySnapshot(task("a"), snapshot("CANCELLED", "PENDING"));
    expect(next.watching).toBe(false);
    expect(taskPhase(next)).toBe("failed");
    expect(finalStatus(next)).toBe("CANCELLED");
    expect(outcomeLabel(taskOutcome(next)!)).toBe("Cancelled");
  });

  it("stops watching a task cancelled while its script was running", () => {
    expect(applySnapshot(task("a"), snapshot("CANCELED", "RUNNING")).watching).toBe(false);
  });

  it("keeps watching when the task ends before its running execution reports back", () => {
    const next = applySnapshot(task("a"), snapshot("COMPLETED", "RUNNING"));
    expect(next.watching).toBe(true);
    expect(taskPhase(next)).toBe("running");
  });

  it("uses the execution's result once it has one", () => {
    const next = applySnapshot(task("a"), snapshot("COMPLETED", "SUCCEEDED"));
    expect(finalStatus(next)).toBe("SUCCEEDED");
    expect(taskPhase(next)).toBe("succeeded");
  });
});

describe("groupResults", () => {
  it("groups identical outputs, ignoring line endings and the device's own name", () => {
    const groups = groupResults([
      finished("PC-1", "SUCCEEDED", "PC-1: all good\r\n"),
      finished("PC-2", "SUCCEEDED", "PC-2: all good"),
      finished("PC-3", "FAILED", "TPM missing", "script failed"),
      finished("PC-4", "SUCCEEDED", "PC-4: all good  "),
    ]);
    expect(groups).toHaveLength(2);
    expect(groups[0].tasks.map((t) => t.deviceName)).toEqual(["PC-1", "PC-2", "PC-4"]);
    expect(groups[0].stdout).toBe("{device}: all good");
    expect(groups[1].status).toBe("FAILED");
  });

  it("lists unfinished groups before finished ones", () => {
    const groups = groupResults([finished("PC-1", "SUCCEEDED", "ok"), task("PC-2"), finished("PC-3", "SUCCEEDED", "ok")]);
    expect(groups.map((g) => g.phase)).toEqual(["queued", "succeeded"]);
  });
});

describe("groupHeadline", () => {
  it("skips separator lines and prefers the error message", () => {
    const [g] = groupResults([finished("PC-1", "FAILED", "-----\nFail - TPM not ready", null)]);
    expect(groupHeadline(g)).toBe("Fail - TPM not ready");
    const [g2] = groupResults([finished("PC-1", "FAILED", "", "could not start the script")]);
    expect(groupHeadline(g2)).toBe("could not start the script");
  });
});

describe("countPhases", () => {
  it("counts each phase", () => {
    expect(countPhases([task("a"), finished("b", "SUCCEEDED", ""), finished("c", "FAILED", "")])).toEqual({
      queued: 1,
      running: 0,
      succeeded: 1,
      nonZero: 0,
      failed: 1,
      stopped: 0,
    });
  });
});

describe("taskOutcome", () => {
  const PARSE = "something went wrong while parsing the PowerShell output";

  it("treats N-able's PowerShell parse error with output as a script that ran and exited non-zero", () => {
    const o = taskOutcome(finished("PC", "FAILED", "Fail - Bitlocker Key Dependancys Not Met", PARSE))!;
    expect(o).toEqual({ kind: "nonZeroExit", ran: true, exitCode: null, hasOutput: true });
    expect(outcomeLabel(o)).toBe("Non-zero exit");
  });

  it("recovers the exit code from N-able's error text when exitCode is null", () => {
    const o = taskOutcome(finished("PC", "FAILED", "", "error trying to run script: exit status 1001"))!;
    expect(o).toMatchObject({ kind: "nonZeroExit", ran: true, exitCode: 1001, hasOutput: false });
    expect(outcomeLabel(o)).toBe("Exit 1001");
  });

  it("recognises passes, timeouts and runs that never started", () => {
    expect(taskOutcome(finished("PC", "SUCCEEDED", "ok"))!.kind).toBe("passed");
    expect(taskOutcome(finished("PC", "FAILED", "", "timed out"))!.kind).toBe("timedOut");
    expect(taskOutcome(task("PC", { taskStatus: "EXPIRED", watching: false }))).toMatchObject({ kind: "error", ran: false });
  });

  it("returns null while the task is unfinished", () => {
    expect(taskOutcome(task("PC"))).toBeNull();
  });

  it("uses the script's output, not N-able's message, as the headline for non-zero exits", () => {
    const [g] = groupResults([finished("PC", "FAILED", "Fail - TPM not ready\nUEFI - False", PARSE)]);
    expect(groupHeadline(g)).toBe("Fail - TPM not ready");
  });
});
