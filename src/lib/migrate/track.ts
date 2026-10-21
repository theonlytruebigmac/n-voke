import type { DeviceAction, TrackGroup } from "./bindings";

/** The stage strip's groups, in the order they are shown. The core decides which group a device is in and counts them. */
export const GROUP_LABELS: Record<TrackGroup, string> = {
  planned: "Planned",
  launched: "Launched",
  staged: "Staged",
  moving: "Moving",
  onNew: "On new server",
  waiting: "Waiting for Ecoverse",
  healthy: "Healthy",
  done: "Done",
  needsYou: "Needs you",
};

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
