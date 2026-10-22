import type { DeviceUfa, EcoState, Stage, UfaState } from "./bindings";

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
    long: "Registered with Ecoverse and shown there, but no asset data has arrived yet. Until it does the device isn't fully working: its name, serial number and addresses are unknown and scripts can't run on it. This commonly takes 20 to 60 minutes. If the agent's first attempt to fetch its setup fails, it tries again at its next hourly check, so it can take up to about an hour and a half.",
  },
  notSeen: { short: "Not in Ecoverse yet", tone: "warn", long: "The new Ecoverse agent is still setting up, and the device isn't in the destination's Ecoverse yet. This commonly takes 20 to 60 minutes and doesn't block the move. If its first attempt fails it tries again at its next hourly check, so it can take up to about an hour and a half." },
  notLinked: {
    short: "Ecoverse not checked",
    tone: "muted",
    long: "This project says the destination isn't linked to Ecoverse, so Ecoverse is not looked at and nothing is known about this device there. If the destination is linked, say so in Prepare and the next pass checks whether Ecoverse is receiving its data.",
  },
  unread: { short: "Ecoverse not read yet", tone: "muted", long: "Ecoverse hasn't been read for this device yet. It is read on each pass." },
};

export const ecoWords = (state: EcoState | null): EcoWords => ECO_WORDS[state ?? "unread"];

/**
 * What the Ecoverse agent itself is doing on the device, from the installer's watcher (see
 * docs/migration/05-endpoint-scripts.md, "UFA sync watchdog"). It is the device's own account, as of the last Probe,
 * so it can say why Ecoverse has no data yet and what the installer is doing about it. Keyed by the generated
 * `UfaState`, so a state added in Rust fails the build until it has words.
 */
export const UFA_WORDS: Record<UfaState, EcoWords> = {
  notInstalled: { short: "Not installed yet", tone: "muted", long: "The Windows Agent installs the Ecoverse agent a few minutes after itself." },
  notProvisioned: { short: "Setting up", tone: "accent", long: "The Ecoverse agent is installed and registering with the destination." },
  waiting: { short: "Waiting for data", tone: "accent", long: "The Ecoverse agent is running and no problem was seen. Its data usually arrives within an hour of the install." },
  synced: { short: "Sending data", tone: "ok", long: "The Ecoverse agent received its components and is sending the device's data." },
  stalled: {
    short: "Needs a restart",
    tone: "warn",
    long: "The agent's first fetch of its setup timed out and it retries only hourly. The installer restarts the agent for it, at most twice, 15 minutes apart.",
  },
  blocked: {
    short: "Can't reach the Ecoverse login service",
    tone: "warn",
    long: "The Ecoverse login service isn't answering the agent. This often clears by itself, so the installer watches it first. If it lasts, the installer restarts the agent after 30 minutes and does a one-time repair after 60 minutes if that didn't help. If it keeps happening, check that the device can reach the Ecoverse login service (an auth host under system-monitor.com) on port 443, with no TLS inspection in the way.",
  },
  rejected: {
    short: "Login rejected",
    tone: "err",
    long: "The cloud rejects the agent's login (400 SSL certificate error). A restart doesn't help. The installer does a one-time repair after 30 minutes. If it is still refused after that, check TLS inspection, proxy and firewall on the device's network path, and report it to N-able.",
  },
  denied: {
    short: "Downloads refused",
    tone: "err",
    long: "The agent can log in, but the cloud refuses its component downloads (403). A restart doesn't help. The installer does a one-time repair after 30 minutes.",
  },
  noDeviceRecord: {
    short: "No device record in Ecoverse",
    tone: "err",
    long: "The Ecoverse agent logs in and its components run, but the cloud has no device record for this agent (the components log \"failed to get goat NRI\"), so nothing they collect can be sent and Ecoverse shows the device with no data. A restart or the repair doesn't help, and the installer tries neither. It has to be fixed on the Ecoverse side: check that the customer or site the device registered under exists in the destination's Ecoverse tenant and is linked to N-central, and that other devices there show data. If they look right, send N-able the agent's logs (collect-logs.ps1 gathers them).",
  },
  broken: {
    short: "Registered but broken",
    tone: "err",
    long: "The Ecoverse agent is registered on the device, but its program is missing and it has no service. The Windows Agent can then only try to remove it, which needs that program, and never installs a new one. The installer can't fix this. Remove the registration (or restore the agent's program and remove it properly), then restart the Windows Agent services. The troubleshooting guide has the steps.",
  },
  unknown: { short: "No agent log", tone: "muted", long: "The device has no log from the Ecoverse agent to read yet." },
};

/** The words for the agent's state; after the one-time repair, a missing agent is being put back, not just late. */
export function ufaWords(ufa: Pick<DeviceUfa, "state" | "repairedAt" | "redeployFiles"> & { removalFailures?: number | null; removalError?: number | null }): EcoWords {
  const missing = ufa.state === "notInstalled" || ufa.state === "notProvisioned";
  const files = ufa.redeployFiles ?? null;
  const progress = files === null ? "" : files > 0 ? ` ${files} file${files === 1 ? "" : "s"} written to its folder since the last restart: it is being put back.` : " Nothing new in its folder since the last restart yet.";
  if (ufa.repairedAt && missing) {
    return {
      short: "Being redeployed",
      tone: "accent",
      long: `The installer removed the Ecoverse agent to repair it and restarted the Windows Agent services. The Windows Agent should now put it back, which usually takes 5 to 15 minutes.${progress}`,
    };
  }
  if (ufa.state === "broken" && ufa.removalFailures) {
    const failures = `Windows Installer logged ${ufa.removalFailures} failed ${ufa.removalFailures === 1 ? "removal" : "removals"}${ufa.removalError ? `, the last with MSI error ${ufa.removalError}` : ""}.`;
    return { ...UFA_WORDS.broken, long: `${UFA_WORDS.broken.long} ${failures}` };
  }
  return UFA_WORDS[ufa.state];
}

const UFA_STEP_WORDS: Record<string, string> = {
  "ufa-sync-restart": "Restarted the Ecoverse agent",
  "ufa-repair": "Repaired the Ecoverse agent (removed it, then restarted the Windows Agent services)",
  "ufa-repair-restart": "Restarted the Windows Agent services again",
  "ufa-registration-broken": "Found the Ecoverse agent registered with its program missing",
};

/** A step the installer took, as a title; an unknown one reads as its own name. */
export const ufaStepTitle = (event: string): string => UFA_STEP_WORDS[event] ?? event;

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

const EVENT_KINDS: Record<string, string> = { poll: "Checked", status: "From the device", note: "Note", match: "Match", path: "Path", output: "Output", launch: "Launch", action: "Action" };

/**
 * A timeline entry as a title and a body. A stage change reads as the stage it entered ("Lost") with why underneath;
 * the core records it as `Stage` or `Stage: why`.
 */
export function eventWords(kind: string, detail: string): { title: string; body: string } {
  if (kind === "stage") {
    const [name, ...rest] = detail.split(": ");
    const stage = (name.charAt(0).toLowerCase() + name.slice(1)) as Stage;
    const known = STAGE_WORDS[stage];
    if (known) return { title: known.short, body: rest.join(": ") };
  }
  return { title: EVENT_KINDS[kind] ?? kind.charAt(0).toUpperCase() + kind.slice(1), body: detail };
}

/**
 * What the "everything has moved" banner may claim. Ecoverse is only mentioned as receiving data when it was checked:
 * a project whose destination isn't linked never reads it, so "healthy" there means only that the agent checks in.
 */
export function movedBanner(ecoverseChecked: boolean): { lead: string; rest: string } {
  return ecoverseChecked
    ? { lead: "Everything has moved and is working.", rest: "Each device is healthy and Ecoverse is receiving its data. What's left is in Finish: a report, and tidying up the migration's records." }
    : { lead: "Everything has moved.", rest: "Each device is healthy in N-central. Ecoverse was not checked, because this project says the destination isn't linked to Ecoverse. If it is, say so in Prepare and the next pass checks whether Ecoverse is receiving each device's data." };
}
