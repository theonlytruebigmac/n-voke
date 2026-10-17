import { useSyncExternalStore } from "react";
import { getTasks } from "./api";
import { capOutput, retain } from "./retention";
import { purgeLegacyStorage, storeGet, storeRemove, storeSet, takeLegacy } from "./secureStore";
import { applySnapshot, isDone, maxWaitSeconds, type JobTask } from "./taskState";
import type { Execution, Script } from "./types";

const POLL_INTERVAL_MS = 5000;
const MAX_POLL_FAILURES = 5;
/** Tasks fetched per request (aliased into one GraphQL query). */
const POLL_BATCH_SIZE = 20;
/** Name in the encrypted store (see secureStore.ts). */
const STORE_NAME = "jobs";
/** Plain-text localStorage keys used by earlier versions; migrated once, then deleted. */
const LEGACY_JOBS_KEY = "jobs.v1";
const LEGACY_RUNS_KEY = "runs.v1";
const MAX_JOBS = 50;
const SAVE_DELAY_MS = 1000;

/** One script started on one or more devices. */
export interface Job {
  id: string;
  name: string;
  scriptId: string | null;
  /** Full script (with input declarations), so "Run again" can pre-fill the Run page. */
  script?: Script | null;
  scriptName: string;
  orgId: string | null;
  orgName: string | null;
  /** Human summary of what was targeted, e.g. "PatchGroupTwo + 1 device". */
  targetLabel: string;
  createdAt: string;
  /** Script timeout used for the run; 0 when unknown (falls back to the API max). */
  timeoutSeconds: number;
  /** Per-device problems reported when the run was created. */
  warnings?: string[];
  tasks: JobTask[];
}

export function newTask(taskId: string, assetId: string | null, deviceName: string, init: Partial<JobTask> = {}): JobTask {
  const t: JobTask = {
    taskId,
    assetId,
    deviceName,
    taskStatus: "IN_PROGRESS",
    execution: null,
    watching: true,
    watchStartedAt: Date.now(),
    pollFailures: 0,
    lastError: null,
    stopReason: null,
    ...init,
  };
  if (isDone(t)) t.watching = false;
  return t;
}

/** Runs saved by the first version of the app become single-device jobs. */
function fromLegacyRuns(raw: string | null): Job[] {
  try {
    const legacy = JSON.parse(raw ?? "[]") as {
      taskId: string;
      taskName: string;
      scriptName: string;
      deviceName: string;
      orgName: string | null;
      createdAt: string;
      timeoutSeconds: number;
      taskStatus: string | null;
      execution: Execution | null;
      watching: boolean;
    }[];
    return legacy.map((r) => ({
      id: r.taskId,
      name: r.taskName,
      scriptId: null,
      scriptName: r.scriptName,
      orgId: null,
      orgName: r.orgName,
      targetLabel: r.deviceName,
      createdAt: r.createdAt,
      timeoutSeconds: r.timeoutSeconds,
      tasks: [newTask(r.taskId, null, r.deviceName, { taskStatus: r.taskStatus, execution: r.execution, watching: r.watching })],
    }));
  } catch {
    return [];
  }
}

function parseJobs(raw: string | null): Job[] {
  try {
    return raw ? (JSON.parse(raw) as Job[]) : [];
  } catch {
    return [];
  }
}

/**
 * Tasks still being watched when the app closed get a fresh watch window, and
 * unfinished tasks that stopped because of errors (not by choice) are picked up again.
 */
function rewatch(list: Job[]): Job[] {
  return list.map((j) => ({
    ...j,
    tasks: j.tasks.map((t) =>
      t.watching || (t.stopReason === "errors" && !isDone(t))
        ? { ...t, watching: true, stopReason: null, lastError: null, watchStartedAt: Date.now(), pollFailures: 0 }
        : t,
    ),
  }));
}

let jobs: Job[] = [];
let historyDays = 30;
let hydrated: Promise<void> | null = null;
/** True once saved history has been read; until then nothing is written (so it can't be overwritten). */
let loaded = false;
let saveTimer: ReturnType<typeof setTimeout> | undefined;
const listeners = new Set<() => void>();

/**
 * Loads saved history from the encrypted store (migrating and deleting any
 * plain-text copy from older versions). Call once at startup; safe to repeat.
 */
export function initJobs(days: number): Promise<void> {
  historyDays = days;
  hydrated ??= (async () => {
    const stored = parseJobs(await storeGet(STORE_NAME));
    const legacy = [...parseJobs(takeLegacy(LEGACY_JOBS_KEY)), ...fromLegacyRuns(takeLegacy(LEGACY_RUNS_KEY))];
    const byId = new Map<string, Job>();
    // Jobs started before loading finished win over saved copies.
    for (const j of [...rewatch([...stored, ...legacy]), ...jobs]) byId.set(j.id, j);
    jobs = retain([...byId.values()].sort((a, b) => b.createdAt.localeCompare(a.createdAt)), Infinity, Date.now(), MAX_JOBS);
    loaded = true;
    emit();
    // Get any migrated history onto disk encrypted first, then remove the old plain-text copy.
    await save();
    await purgeLegacyStorage();
  })();
  return hydrated;
}

/** Applies a new retention setting (days; 0 = don't save history). */
export function setHistoryDays(days: number) {
  historyDays = days;
  emit();
}

/** Forgets all jobs (after the user clears local data). Jobs still running keep being watched. */
export function clearFinishedJobs() {
  jobs = jobs.filter(isJobActive);
  emit();
}

/** Writes the retained history, encrypted. Nothing is written when history isn't saved. */
async function writeHistory(): Promise<void> {
  if (historyDays <= 0) return storeRemove(STORE_NAME);
  const keep = retain(jobs, historyDays, Date.now(), MAX_JOBS).map(capOutput);
  return storeSet(STORE_NAME, JSON.stringify(keep));
}

let saving: Promise<void> = Promise.resolve();

/** One write at a time, so a slow write can't land after (and undo) a newer one. */
function save(): Promise<void> {
  saving = saving.then(writeHistory, writeHistory);
  return saving;
}

function emit() {
  listeners.forEach((l) => l());
  if (!loaded) return;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => void save(), SAVE_DELAY_MS);
}

function updateTasks(jobId: string, fn: (t: JobTask) => JobTask) {
  jobs = jobs.map((j) => (j.id === jobId ? { ...j, tasks: j.tasks.map(fn) } : j));
  emit();
}

export const jobStore = {
  subscribe(listener: () => void) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
  getSnapshot: () => jobs,

  add(job: Job) {
    // Past the limit, older finished jobs make way; ones still being watched stay.
    jobs = [job, ...jobs.filter((j) => j.id !== job.id)].filter((j, i) => i < MAX_JOBS || isJobActive(j));
    emit();
    void pollNow();
  },

  /** Resume watching every unfinished task in a job (or just one). */
  resume(jobId: string, taskId?: string) {
    updateTasks(jobId, (t) =>
      (!taskId || t.taskId === taskId) && !isDone(t)
        ? { ...t, watching: true, watchStartedAt: Date.now(), pollFailures: 0, lastError: null, stopReason: null }
        : t,
    );
    void pollNow();
  },

  stop(jobId: string) {
    updateTasks(jobId, (t) => (t.watching ? { ...t, watching: false, stopReason: "manual" } : t));
  },

  remove(jobId: string) {
    jobs = jobs.filter((j) => j.id !== jobId);
    emit();
  },
};

export function useJobs(): Job[] {
  return useSyncExternalStore(jobStore.subscribe, jobStore.getSnapshot);
}

export function isJobActive(job: Job): boolean {
  return job.tasks.some((t) => t.watching);
}

let polling = false;

async function pollNow() {
  if (polling) return;
  polling = true;
  try {
    const now = Date.now();
    // Expire tasks whose watch window has passed.
    for (const job of jobs) {
      const limit = maxWaitSeconds(job.timeoutSeconds) * 1000;
      if (job.tasks.some((t) => t.watching && now - t.watchStartedAt >= limit)) {
        updateTasks(job.id, (t) => (t.watching && now - t.watchStartedAt >= limit ? { ...t, watching: false, stopReason: "timeout" } : t));
      }
    }

    const watched = jobs.flatMap((j) => j.tasks.filter((t) => t.watching).map((t) => ({ jobId: j.id, taskId: t.taskId })));
    for (let i = 0; i < watched.length; i += POLL_BATCH_SIZE) {
      const batch = watched.slice(i, i + POLL_BATCH_SIZE);
      const ids = new Set(batch.map((b) => b.taskId));
      try {
        const { tasks: snapshots, errors } = await getTasks([...ids]);
        const partial = errors.length ? errors.join("; ") : null;
        jobs = jobs.map((j) => ({
          ...j,
          tasks: j.tasks.map((t) => {
            if (!ids.has(t.taskId) || !t.watching) return t;
            const snap = snapshots.get(t.taskId);
            if (!snap) {
              // Can be transient right after creation; give up only after repeated misses.
              const failures = t.pollFailures + 1;
              const giveUp = failures >= MAX_POLL_FAILURES;
              return { ...t, pollFailures: failures, lastError: "The task wasn't found.", watching: !giveUp, stopReason: giveUp ? "errors" : t.stopReason };
            }
            // Partial errors (e.g. output not readable yet) are shown but don't stop polling.
            return { ...applySnapshot(t, snap), lastError: partial };
          }),
        }));
      } catch (e) {
        const message = (e as Error).message;
        jobs = jobs.map((j) => ({
          ...j,
          tasks: j.tasks.map((t) => {
            if (!ids.has(t.taskId) || !t.watching) return t;
            const failures = t.pollFailures + 1;
            const giveUp = failures >= MAX_POLL_FAILURES;
            return { ...t, pollFailures: failures, lastError: message, watching: !giveUp, stopReason: giveUp ? "errors" : t.stopReason };
          }),
        }));
      }
      emit();
    }
  } finally {
    polling = false;
  }
}

setInterval(() => void pollNow(), POLL_INTERVAL_MS);
