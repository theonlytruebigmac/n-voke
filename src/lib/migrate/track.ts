import type { DeviceAction, ProjectSummary, Stage, TrackRow } from "./bindings";

/** The stage strip's groups: the 16 stages, read in eight (docs/migration/11-gui-design.md, Track). */
export type GroupId = "planned" | "launched" | "staged" | "moving" | "onNew" | "waiting" | "healthy" | "done" | "needsYou";

export const GROUPS: { id: GroupId; label: string; stages: Stage[] }[] = [
  { id: "planned", label: "Planned", stages: ["planned"] },
  { id: "launched", label: "Launched", stages: ["launched"] },
  { id: "staged", label: "Staged", stages: ["staged"] },
  { id: "moving", label: "Moving", stages: ["agentRemoved"] },
  { id: "onNew", label: "On new server", stages: ["registered"] },
  // Healthy in N-central, but Ecoverse isn't receiving its data yet. A device is in this group or in Healthy, never both.
  { id: "waiting", label: "Waiting for Ecoverse", stages: [] },
  { id: "healthy", label: "Healthy", stages: ["healthy"] },
  { id: "done", label: "Done", stages: ["cleanedUp", "retired", "resolved"] },
  { id: "needsYou", label: "Needs you", stages: ["unreachable", "notPickedUp", "launchFailed", "stuckStaged", "lost", "wrongPlace"] },
];

export const NEEDS_YOU: ReadonlySet<Stage> = new Set(GROUPS.find((g) => g.id === "needsYou")?.stages);

export function groupOf(stage: Stage, waitingForEcoverse = false): GroupId | null {
  if (stage === "healthy" && waitingForEcoverse) return "waiting";
  return GROUPS.find((g) => g.stages.includes(stage))?.id ?? null;
}

/** `ecoverseWaiting` healthy devices are counted under Waiting for Ecoverse instead of Healthy. */
export function groupCounts(stageCounts: ProjectSummary["stageCounts"], ecoverseWaiting = 0): { id: GroupId; label: string; count: number }[] {
  return GROUPS.map((g) => {
    const count = g.stages.reduce((sum, s) => sum + (stageCounts[s] ?? 0), 0);
    if (g.id === "waiting") return { id: g.id, label: g.label, count: ecoverseWaiting };
    if (g.id === "healthy") return { id: g.id, label: g.label, count: Math.max(0, count - ecoverseWaiting) };
    return { id: g.id, label: g.label, count };
  });
}

/** Rows that need the person: in a needs-you stage, or a destination match to choose. */
export const needsYou = (r: TrackRow) => NEEDS_YOU.has(r.stage) || r.needsMatch;

export interface TrackFilter {
  group: GroupId | null;
  wave: number | null;
  customer: string | null;
  text: string;
}

export function filterTrack(rows: TrackRow[], f: TrackFilter): TrackRow[] {
  const term = f.text.trim().toLowerCase();
  return rows.filter((r) => {
    if (f.group !== null && groupOf(r.stage, r.waitingForEcoverse) !== f.group && !(f.group === "needsYou" && r.needsMatch)) return false;
    if (f.wave !== null && r.wave !== f.wave) return false;
    if (f.customer !== null && r.customer !== f.customer) return false;
    return !term || r.name.toLowerCase().includes(term) || r.customer.toLowerCase().includes(term);
  });
}

/** How long ago, in the words the page uses ("34 min", "2 h", "3 d"). */
export function ago(iso: string, now: number): string {
  const minutes = Math.max(0, Math.round((now - Date.parse(iso)) / 60000));
  if (Number.isNaN(minutes)) return "";
  if (minutes < 1) return "under a minute";
  if (minutes < 60) return `${minutes} min`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)} h`;
  return `${Math.floor(minutes / 1440)} d`;
}

export const ACTION_WORDS: Record<DeviceAction, { label: string; confirm: string | null }> = {
  probe: { label: "Probe", confirm: null },
  cleanup: { label: "Run cleanup now", confirm: "This removes what the migration left on the device. It can't be undone." },
  retry: { label: "Retry", confirm: "The device goes back to Planned, then you launch it again." },
  retryOtherPath: { label: "Retry on the other path", confirm: "The device goes back to Planned on the other delivery path (Ecoverse or N-central), then you launch it again." },
  markResolved: { label: "Mark resolved", confirm: "The device stops being tracked, as fixed outside N-voke." },
  chooseMatch: { label: "Choose match", confirm: null },
};
