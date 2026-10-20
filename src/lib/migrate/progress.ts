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
export function projectProgress(p: Pick<ProjectSummary, "devices" | "stageCounts">): Progress {
  const left = p.stageCounts.excluded ?? 0;
  const total = Math.max(0, p.devices - left);
  const done = DONE.reduce((sum, stage) => sum + (p.stageCounts[stage] ?? 0), 0);
  return { total, done, fraction: total === 0 ? 0 : Math.min(1, done / total) };
}
