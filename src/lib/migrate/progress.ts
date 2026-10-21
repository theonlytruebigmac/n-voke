import type { ProjectSummary, Stage } from "./bindings";

/** Stages that count as done for the project's progress bar: healthy, or past it, or taken out of the count. */
const DONE: Stage[] = ["healthy", "cleanedUp", "retired", "resolved"];

export interface Progress {
  /** Devices counted for progress: everything except those left out of the project. */
  total: number;
  done: number;
  /** 0 to 1; 0 for a project with nothing to move. */
  fraction: number;
}

/**
 * The list's progress bar: devices that are healthy or beyond, out of the devices that are being moved
 * (docs/migration/11-gui-design.md, open question 5). Counts come from the core's stage counts.
 */
export function projectProgress(p: Pick<ProjectSummary, "devices" | "stageCounts"> & { ecoverseWaiting?: number }): Progress {
  const left = p.stageCounts.excluded ?? 0;
  const total = Math.max(0, p.devices - left);
  // A healthy device whose Ecoverse isn't receiving data yet is registered but not fully working, so it isn't done.
  const done = Math.max(0, DONE.reduce((sum, stage) => sum + (p.stageCounts[stage] ?? 0), 0) - (p.ecoverseWaiting ?? 0));
  return { total, done, fraction: total === 0 ? 0 : Math.min(1, done / total) };
}

const FINISHED_OR_WORKING: ReadonlySet<string> = new Set(["healthy", "cleanedUp", "retired", "resolved", "excluded"]);

/**
 * Everything that is being moved has arrived and is fully working: no device is planned, moving or needing the person,
 * and none is waiting for Ecoverse's data. What is left is Finish (the report and tidying up).
 */
export function readyToFinish(p: Pick<ProjectSummary, "devices" | "stageCounts"> & { ecoverseWaiting?: number }): boolean {
  const left = p.stageCounts.excluded ?? 0;
  if (p.devices - left <= 0) return false;
  if ((p.ecoverseWaiting ?? 0) > 0) return false;
  return Object.entries(p.stageCounts).every(([stage, count]) => !count || FINISHED_OR_WORKING.has(stage));
}
