import { useEffect, useRef, useSyncExternalStore } from "react";
import { useNow } from "../../lib/format";
import type { TrackerState } from "../../lib/migrate/queries";
import { ago } from "../../lib/migrate/track";
import { Dot, Tooltip, type Tone } from "../ui";

// The tracker runs while a project's overview or Track page is open. Those pages publish its state here, and the
// top menu shows it next to Settings, so the state is in one known place and the page keeps its room.

interface Published {
  /** Which page published it, so a page leaving doesn't clear what the next one put there. */
  owner: symbol;
  projectName: string;
  tracker: TrackerState;
}

let current: Published | null = null;
const listeners = new Set<() => void>();

function set(next: Published | null) {
  current = next;
  listeners.forEach((l) => l());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Shows this project's tracker in the top menu while the calling page is open. Pass null while it isn't running. */
export function usePublishTracker(projectName: string | undefined, tracker: TrackerState | null) {
  const owner = useRef(Symbol("tracker page")).current;
  useEffect(() => {
    if (tracker && projectName) set({ owner, projectName, tracker });
    else if (current?.owner === owner) set(null);
  }, [owner, projectName, tracker]);
  useEffect(() => () => void (current?.owner === owner && set(null)), [owner]);
}

/** Short and long wording for a tracker state: the short fits the top menu, the long goes in its tooltip. */
export function trackerWords(tracker: TrackerState, now: number): { tone: Tone; pulse: boolean; short: string; long: string; readOnly: boolean } {
  const readOnly = tracker.error?.includes("Another N-voke window") ?? false;
  if (readOnly) return { tone: "warn", pulse: false, short: "Read-only", long: "Read-only: another window is working on this project", readOnly };
  if (tracker.error) return { tone: "err", pulse: false, short: "Tracking stopped", long: `Couldn't read: ${tracker.error}`, readOnly };
  if (tracker.starting) return { tone: "accent", pulse: true, short: "Reading the devices…", long: "Reading the devices…", readOnly };
  const text = tracker.last ? `Tracking · read ${ago(tracker.last.at, now)} ago` : "Tracking";
  return { tone: "ok", pulse: false, short: text, long: text, readOnly };
}

/** The tracker's state as a small pill, for the top menu. Nothing while no project is being tracked. */
export function TrackerIndicator() {
  const published = useSyncExternalStore(subscribe, () => current);
  const now = useNow(15_000);
  if (!published) return null;
  const words = trackerWords(published.tracker, now);
  return (
    <Tooltip content={`${published.projectName}: ${words.long}`}>
      <span role="status" className="inline-flex h-7 max-w-64 items-center gap-2 rounded-full border border-border px-3 text-xs whitespace-nowrap text-muted">
        <Dot tone={words.tone} pulse={words.pulse} />
        <span className="truncate">{words.short}</span>
      </span>
    </Tooltip>
  );
}
