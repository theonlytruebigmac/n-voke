import type { Stage } from "./bindings";

/** How the existing `Status` and `Dot` components colour a state. */
export type Tone = "ok" | "warn" | "err" | "accent" | "muted";

export interface StageWords {
  /** The word in tables. */
  short: string;
  tone: Tone;
  /** A moving dot: something is happening right now. */
  pulse: boolean;
  /** What is happening and what to do, for the device sheet. `{n}`, `{reason}` and the like are filled in by the caller. */
  long: string;
}

/**
 * The only place the page decides how a stage reads (docs/migration/11-gui-design.md, "Words"). It is wording
 * and colour, not rules: which stage a device is in, and which actions apply, come from the core. The record is
 * keyed by the generated `Stage`, so a stage added in Rust fails the build until it has words.
 */
export const STAGE_WORDS: Record<Stage, StageWords> = {
  planned: { short: "Planned", tone: "muted", pulse: false, long: "Ready to launch." },
  unreachable: {
    short: "Can't reach",
    tone: "err",
    pulse: false,
    long: "No agent can reach it now. It is picked up if it comes back; export the list for on-site follow-up.",
  },
  launched: { short: "Launched", tone: "accent", pulse: true, long: "The launch was accepted. Waiting for the device to start it." },
  notPickedUp: {
    short: "Not picked up",
    tone: "warn",
    pulse: false,
    long: "Launched {n} and still not running. It may be offline or busy. Cancel and relaunch, or use the other path.",
  },
  launchFailed: { short: "Launch failed", tone: "err", pulse: false, long: "The launch was refused: {reason}. Retry, or retry on the other path." },
  staged: { short: "Staged", tone: "accent", pulse: true, long: "The script ran and scheduled the install. It starts within a couple of minutes." },
  stuckStaged: { short: "Stuck", tone: "warn", pulse: false, long: "Staged {n} ago but the old agent is still checking in. Probe it to see why." },
  agentRemoved: { short: "Moving", tone: "accent", pulse: true, long: "The old agent is gone and the new one is installing. This takes about 10 to 15 minutes." },
  lost: { short: "Lost", tone: "err", pulse: false, long: "The old agent is gone and nothing appeared on the new server after {n}. Probe it; plan on-site follow-up." },
  registered: { short: "On the new server", tone: "accent", pulse: true, long: "Found on {destination}. Waiting for its first check-in." },
  wrongPlace: { short: "Wrong place", tone: "warn", pulse: false, long: "Registered under {actual}, not {expected}. Fix the mapping or leave it." },
  healthy: { short: "Healthy", tone: "ok", pulse: false, long: "Checking in to {destination}." },
  cleanedUp: { short: "Cleaned up", tone: "ok", pulse: false, long: "Its migration record is removed." },
  retired: { short: "Done", tone: "ok", pulse: false, long: "Finished, and the old record is retired." },
  excluded: { short: "Left out", tone: "muted", pulse: false, long: "Taken out of the project." },
  resolved: { short: "Resolved", tone: "muted", pulse: false, long: "Fixed outside N-voke." },
};
