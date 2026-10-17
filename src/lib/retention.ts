import type { JobTask } from "./taskState";

/** Stored output per device is capped so one chatty script can't bloat the history. */
export const MAX_STORED_OUTPUT = 64 * 1024;
const TRUNCATED = "\n… (output truncated in saved history)";

interface RetainableJob {
  createdAt: string;
  tasks: JobTask[];
}

/**
 * Jobs to keep under a retention of `days` (0 = keep nothing on disk). Jobs
 * still being watched are always kept, whatever their age.
 */
export function retain<T extends RetainableJob>(jobs: T[], days: number, now = Date.now(), max = 50): T[] {
  const watched = (j: T) => j.tasks.some((t) => t.watching);
  const cutoff = days <= 0 ? Infinity : now - days * 24 * 60 * 60 * 1000;
  // `max` caps finished jobs only; dropping a watched one would lose it on restart.
  return jobs.filter((j) => watched(j) || new Date(j.createdAt).getTime() >= cutoff).filter((j, i) => i < max || watched(j));
}

function cap(text: string | null | undefined): string | null | undefined {
  return text && text.length > MAX_STORED_OUTPUT ? text.slice(0, MAX_STORED_OUTPUT) + TRUNCATED : text;
}

/** A copy of the job with each device's output capped for storage. */
export function capOutput<T extends RetainableJob>(job: T): T {
  return {
    ...job,
    tasks: job.tasks.map((t) =>
      t.execution?.output
        ? { ...t, execution: { ...t.execution, output: { ...t.execution.output, stdOutput: cap(t.execution.output.stdOutput) ?? null, stdError: cap(t.execution.output.stdError) ?? null } } }
        : t,
    ),
  };
}
