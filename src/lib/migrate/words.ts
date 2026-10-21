import type { EcoState, Stage } from "./bindings";

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

export interface StageContext {
  /** How long it has been in the stage, as `ago` words ("34 min"). */
  since: string;
  destination?: string | null;
  reason?: string | null;
  /** Healthy in N-central, but Ecoverse isn't receiving its data yet. */
  waitingForEcoverse?: boolean;
}

/** The sentence for a stage with its blanks filled in; a blank with nothing to fill it is left out of the sentence. */
export function describeStage(stage: Stage, ctx: StageContext): string {
  if (stage === "healthy" && ctx.waitingForEcoverse) return `Checking in to ${ctx.destination ?? "the new server"}, but Ecoverse isn't receiving its data yet, so it isn't fully working.`;
  return STAGE_WORDS[stage].long
    .replace("Launched {n} and", ctx.since ? `Launched ${ctx.since} ago and` : "Launched a while ago and")
    .replace("Staged {n} ago but", ctx.since ? `Staged ${ctx.since} ago but` : "Staged a while ago but")
    .replace("after {n}", ctx.since ? `after ${ctx.since}` : "after a long wait")
    .replace("{destination}", ctx.destination ?? "the new server")
    .replace("{actual}", "another customer or site")
    .replace("{expected}", ctx.destination ?? "the mapped destination")
    .replace("{reason}", ctx.reason?.trim() ? ctx.reason.trim().replace(/\.$/, "") : "no reason was given");
}

export interface EcoWords {
  short: string;
  tone: Tone;
  long: string;
}

/**
 * What the destination's Ecoverse shows for a device that has moved. "Healthy" in N-central only means the Windows
 * Agent checks in: the device is not fully working until Ecoverse is receiving its asset data (name, serial number,
 * addresses), which is also when scripts can run on it. `unread` is a device the tracker hasn't read Ecoverse for yet.
 */
export const ECO_WORDS: Record<EcoState | "unread", EcoWords> = {
  reporting: { short: "Ecoverse data received", tone: "ok", long: "The Ecoverse agent is sending asset data (name, serial number, addresses), so the device is fully working there." },
  waitingForData: {
    short: "Waiting for Ecoverse data",
    tone: "warn",
    long: "Registered with Ecoverse and shown there, but no asset data has arrived yet. Until it does the device isn't fully working: its name, serial number and addresses are unknown and scripts can't run on it. This commonly takes 20 to 60 minutes.",
  },
  notSeen: { short: "Not in Ecoverse yet", tone: "warn", long: "The new Ecoverse agent is still setting up, and the device isn't in the destination's Ecoverse yet. This commonly takes 20 to 60 minutes and doesn't block the move." },
  notLinked: { short: "Not in Ecoverse", tone: "muted", long: "The destination isn't linked to Ecoverse, so the device isn't expected there." },
  unread: { short: "Ecoverse not read yet", tone: "muted", long: "Ecoverse hasn't been read for this device yet. It is read on each pass." },
};

export const ecoWords = (state: EcoState | null): EcoWords => ECO_WORDS[state ?? "unread"];

export interface StageView {
  short: string;
  tone: Tone;
  pulse: boolean;
}

/**
 * How a stage reads in a table. A healthy device whose Ecoverse isn't receiving data yet reads "Checking in", not
 * "Healthy": it is registered, but not fully working.
 */
export function stageView(stage: Stage, waitingForEcoverse: boolean): StageView {
  if (stage === "healthy" && waitingForEcoverse) return { short: "Checking in", tone: "accent", pulse: true };
  const { short, tone, pulse } = STAGE_WORDS[stage];
  return { short, tone, pulse };
}
