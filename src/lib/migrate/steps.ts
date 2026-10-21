import type { PrepareView, ProjectSummary } from "./bindings";
import { readyToFinish } from "./progress";

export type StepId = "connect" | "map" | "prepare" | "plan" | "launch" | "track" | "finish";

export const STEPS: { id: StepId; label: string }[] = [
  { id: "connect", label: "Connect" },
  { id: "map", label: "Map" },
  { id: "prepare", label: "Prepare" },
  { id: "plan", label: "Plan" },
  { id: "launch", label: "Launch" },
  { id: "track", label: "Track" },
  { id: "finish", label: "Finish" },
];

/** `done` is a green check; `partial` is started but not finished (some waves launched, others waiting). */
export type StepMark = "done" | "partial";

export interface StepStatus {
  marks: Partial<Record<StepId, StepMark>>;
  /** A few words on why a step is marked as it is, for the rail's tooltip. */
  notes: Partial<Record<StepId, string>>;
}

type Summary = Pick<ProjectSummary, "devices" | "stageCounts"> & { ecoverseWaiting?: number; ecoverseChecked?: boolean; closed?: boolean };

/** The endpoint script is where the project needs it: in Ecoverse when Ecoverse is used, otherwise in the repository. */
export function scriptsReady(view: Pick<PrepareView, "ecoverse" | "rest">): boolean {
  return view.ecoverse.notUsed ? view.rest.state === "current" : view.ecoverse.state === "current";
}

/**
 * Which steps get a check. A step is only done when all of its work is, so with several waves Launch has no check
 * until every wave has gone, and Track none until everything that moved is working. Plan and the steps before it
 * are done once devices have been added, since adding them needed both servers, a mapping and a plan.
 */
export function stepStatus(p: Summary | undefined, prepare: Pick<PrepareView, "ecoverse" | "rest"> | undefined): StepStatus {
  const marks: StepStatus["marks"] = {};
  const notes: StepStatus["notes"] = {};
  if (!p) return { marks, notes };

  const active = p.devices - (p.stageCounts.excluded ?? 0);
  const waiting = p.stageCounts.planned ?? 0;
  const launched = Math.max(0, active - waiting);
  const hasDevices = active > 0;

  if (hasDevices) {
    for (const step of ["connect", "map", "plan"] as const) marks[step] = "done";
    notes.plan = `${active} device${active === 1 ? "" : "s"} in the project`;
  }
  if ((prepare && scriptsReady(prepare)) || launched > 0) marks.prepare = "done";

  if (hasDevices && waiting === 0) {
    marks.launch = "done";
    notes.launch = "Every device has been launched";
  } else if (launched > 0) {
    marks.launch = "partial";
    notes.launch = `${launched} of ${active} devices launched; ${waiting} still waiting`;
  }

  if (readyToFinish(p)) {
    marks.track = "done";
    notes.track = p.ecoverseChecked === false ? "Everything has moved (Ecoverse was not checked)" : "Everything has moved and is working";
  }
  if (p.closed) {
    marks.finish = "done";
    notes.finish = "Closed";
  }
  return { marks, notes };
}
