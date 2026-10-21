import { newTask, type Job } from "../jobs";
import type { LaunchReport } from "./bindings";

/** The Ecoverse runs a launch started, as Activity jobs, so each device's output can be watched there. */
export function jobsFromLaunch(report: LaunchReport, now = new Date(), newId: () => string = () => crypto.randomUUID()): Job[] {
  return report.activity.map((run) => ({
    id: newId(),
    name: run.name,
    scriptId: run.scriptId,
    scriptName: run.scriptName,
    orgId: null,
    orgName: null,
    targetLabel: run.targetLabel,
    migrationProject: run.projectId,
    createdAt: now.toISOString(),
    timeoutSeconds: run.timeoutSeconds,
    tasks: run.tasks.map(([taskId, assetId, device]) => newTask(taskId, assetId || null, device)),
  }));
}
