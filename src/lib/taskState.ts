import type { Execution, TaskSnapshot } from "./types";

/** Statuses that mean "not finished yet" for both tasks and executions. */
const PENDING_STATUSES = new Set(["IN_PROGRESS", "PENDING", "QUEUED", "SCHEDULED", "CREATED", "RUNNING", "STARTED"]);
const PICKUP_GRACE_SECONDS = 300;
const API_MAX_TIMEOUT_SECONDS = 3600;

/** One device's task within a job. */
export interface JobTask {
  taskId: string;
  assetId: string | null;
  deviceName: string;
  taskStatus: string | null;
  execution: Execution | null;
  watching: boolean;
  /** When the current watch window began (ms epoch); reset on resume. */
  watchStartedAt: number;
  pollFailures: number;
  lastError: string | null;
  /** Why watching stopped without a final result, if it did. */
  stopReason: "timeout" | "errors" | "manual" | null;
}

/**
 * "nonZero": the script ran but exited with a non-zero code, which N-able
 * records as Failed. "failed": it didn't run or didn't finish (error, timeout).
 */
export type TaskPhase = "queued" | "running" | "succeeded" | "nonZero" | "failed" | "stopped";

export function isFinished(phase: TaskPhase): boolean {
  return phase === "succeeded" || phase === "nonZero" || phase === "failed";
}
export type Tone = "ok" | "err" | "warn" | "accent" | "muted";

/** Execution statuses that mean the device hasn't started the script yet. */
const NOT_STARTED_STATUSES = new Set(["PENDING", "QUEUED", "SCHEDULED", "CREATED"]);
const CANCELLED = /CANCEL/i;

const ended = (status: string | null | undefined) => !!status && !PENDING_STATUSES.has(status);

/**
 * Finished when the execution finishes, or when the task itself ends (e.g.
 * cancelled or expired in N-able) while its execution never started, so no
 * output is coming. A task that ends while its script is still running is
 * watched until the execution catches up, unless it was cancelled.
 */
export function isDone(t: Pick<JobTask, "execution" | "taskStatus">): boolean {
  if (ended(t.execution?.status)) return true;
  if (!ended(t.taskStatus)) return false;
  return !t.execution || NOT_STARTED_STATUSES.has(t.execution.status) || CANCELLED.test(t.taskStatus!);
}

/** The status a finished task ended with: the execution's, unless the task ended first. */
export function finalStatus(t: Pick<JobTask, "execution" | "taskStatus">): string | null {
  return ended(t.execution?.status) ? t.execution!.status : (t.taskStatus ?? t.execution?.status ?? null);
}

export function taskPhase(t: JobTask): TaskPhase {
  if (isDone(t)) {
    if (finalStatus(t) === "SUCCEEDED") return "succeeded";
    return taskOutcome(t)?.kind === "nonZeroExit" ? "nonZero" : "failed";
  }
  if (!t.watching) return "stopped";
  return t.execution ? "running" : "queued";
}

export const PHASE_TONE: Record<TaskPhase, Tone> = {
  queued: "accent",
  running: "accent",
  succeeded: "ok",
  nonZero: "warn",
  failed: "err",
  stopped: "muted",
};

export const PHASE_LABEL: Record<TaskPhase, string> = {
  queued: "Waiting for device",
  running: "Running",
  succeeded: "Succeeded",
  nonZero: "Non-zero exit",
  failed: "Didn't run",
  stopped: "Not watching",
};

export function maxWaitSeconds(timeoutSeconds: number): number {
  return (timeoutSeconds > 0 ? timeoutSeconds : API_MAX_TIMEOUT_SECONDS) + PICKUP_GRACE_SECONDS;
}

/** Applies a fresh task snapshot from the API to a stored task. */
export function applySnapshot(t: JobTask, snapshot: TaskSnapshot): JobTask {
  const execution = snapshot.executions?.nodes.find((n): n is Execution => !!n) ?? null;
  const next = { ...t, taskStatus: snapshot.status, execution, pollFailures: 0 };
  return { ...next, watching: t.watching && !isDone(next) };
}

/**
 * How a finished task ended, from the script's point of view. N-able marks a
 * script Failed whenever it exits with a non-zero code, even when it ran
 * normally and produced its report, and it never fills in `exitCode` for
 * script runs; the code only sometimes appears in the error text.
 */
export type OutcomeKind = "passed" | "nonZeroExit" | "timedOut" | "cancelled" | "error";

export interface Outcome {
  kind: OutcomeKind;
  /** The script started and ran (as opposed to never running). */
  ran: boolean;
  /** Exit code if N-able reported it anywhere; null when unknown. */
  exitCode: number | null;
  hasOutput: boolean;
}

/** N-able's message when a PowerShell script exits non-zero and its output was captured. */
const PARSE_ERROR = /parsing the powershell output/i;
const EXIT_STATUS = /exit (?:status|code)[:\s]+(-?\d+)/i;

export function taskOutcome(t: Pick<JobTask, "execution" | "taskStatus">): Outcome | null {
  if (!isDone(t)) return null;
  const exec = t.execution;
  const message = exec?.errorMessage ?? "";
  const out = exec?.output;
  const hasOutput = !!(out?.stdOutput?.trim() || out?.stdError?.trim() || out?.outputs?.length);
  const reported = exec?.exitCode ?? (EXIT_STATUS.test(message) ? Number(message.match(EXIT_STATUS)![1]) : null);

  if (finalStatus(t) === "SUCCEEDED") return { kind: "passed", ran: true, exitCode: reported ?? 0, hasOutput };
  if (CANCELLED.test(finalStatus(t) ?? "")) return { kind: "cancelled", ran: hasOutput, exitCode: reported, hasOutput };
  if (/timed out/i.test(message) || finalStatus(t) === "TIMED_OUT") return { kind: "timedOut", ran: !!exec, exitCode: reported, hasOutput };
  if ((reported !== null && reported !== 0) || PARSE_ERROR.test(message)) {
    return { kind: "nonZeroExit", ran: true, exitCode: reported, hasOutput };
  }
  // Failed without a recognisable reason: if it produced output it still ran.
  return { kind: hasOutput ? "nonZeroExit" : "error", ran: hasOutput, exitCode: reported, hasOutput };
}

/** Short label for a finished task, e.g. "Failed · exit 1001". */
export function outcomeLabel(o: Outcome): string {
  switch (o.kind) {
    case "passed":
      return "Succeeded";
    case "nonZeroExit":
      return o.exitCode !== null ? `Exit ${o.exitCode}` : "Non-zero exit";
    case "timedOut":
      return "Timed out";
    case "cancelled":
      return "Cancelled";
    default:
      return "Didn't run";
  }
}

export type PhaseCounts = Record<TaskPhase, number>;

export function countPhases(tasks: JobTask[]): PhaseCounts {
  const counts: PhaseCounts = { queued: 0, running: 0, succeeded: 0, nonZero: 0, failed: 0, stopped: 0 };
  for (const t of tasks) counts[taskPhase(t)]++;
  return counts;
}

export interface ResultGroup {
  key: string;
  phase: TaskPhase;
  /** Final status for finished groups (e.g. FAILED, TIMED_OUT). */
  status: string | null;
  /** Representative output, with the device's own name replaced by {device}. */
  stdout: string;
  stderr: string;
  errorMessage: string;
  tasks: JobTask[];
}

function normalize(text: string | null | undefined, deviceName: string): string {
  if (!text) return "";
  let out = text.replace(/\r\n?/g, "\n").replace(/[ \t]+$/gm, "").trim();
  if (deviceName) out = out.split(deviceName).join("{device}");
  return out;
}

/**
 * Groups tasks by outcome so many devices with identical results read as one
 * line. Output is compared after normalizing line endings, trailing spaces
 * and the device's own name (scripts often echo the hostname).
 * Unfinished groups come first, then finished groups by size.
 */
export function groupResults(tasks: JobTask[]): ResultGroup[] {
  const groups = new Map<string, ResultGroup>();
  for (const t of tasks) {
    const phase = taskPhase(t);
    const done = isFinished(phase);
    const out = t.execution?.output;
    const stdout = done ? normalize(out?.stdOutput, t.deviceName) : "";
    const stderr = done ? normalize(out?.stdError, t.deviceName) : "";
    const errorMessage = done ? normalize(t.execution?.errorMessage, t.deviceName) : "";
    const status = done ? finalStatus(t) : null;
    const key = done ? [status, errorMessage, stdout, stderr].join("\u0000") : phase;

    const group = groups.get(key);
    if (group) group.tasks.push(t);
    else groups.set(key, { key, phase, status, stdout, stderr, errorMessage, tasks: [t] });
  }
  const order: TaskPhase[] = ["running", "queued", "stopped"];
  return [...groups.values()].sort((a, b) => {
    const ai = order.indexOf(a.phase);
    const bi = order.indexOf(b.phase);
    if (ai !== -1 || bi !== -1) return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
    return b.tasks.length - a.tasks.length;
  });
}

/**
 * First meaningful line of a group's output, for its heading. When the script
 * ran and only its exit code failed it, the script's own output says more
 * than N-able's error message.
 */
export function groupHeadline(g: ResultGroup): string {
  const outcome = taskOutcome(g.tasks[0]);
  const text = outcome?.kind === "nonZeroExit" && g.stdout ? g.stdout : g.errorMessage || g.stderr || g.stdout;
  const line = text.split("\n").find((l) => l.trim() && !/^[-=_*\s]+$/.test(l));
  return line ? line.trim() : "No output";
}
