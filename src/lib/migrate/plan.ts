import type { Blocker, PlanRow, Warning } from "./bindings";

/** Where a blocked device is fixed. */
export type FixIn = "map" | "prepare" | null;

export interface BlockerWords {
  /** The short reason in the table. */
  short: string;
  /** What to do about it. */
  text: string;
  fix: FixIn;
}

/**
 * The only place the page words a blocker. The core's messages name terminal commands, so the page has its own;
 * the record is keyed by the generated `Blocker`, so a reason added in Rust fails the build until it has words.
 */
export const BLOCKER_WORDS: Record<Blocker, BlockerWords> = {
  notWindows: { short: "Not Windows", text: "Only Windows agents can be migrated. Export the list for manual follow-up.", fix: null },
  unmapped: { short: "Not mapped", text: "Its customer or site has no destination. Map it, or leave it out.", fix: "map" },
  ambiguousMapping: { short: "Several fit", text: "Its customer or site fits more than one destination. Choose one in Map.", fix: "map" },
  tokenMissing: { short: "No token", text: "The destination has no registration token. Create it in Map, under Tokens.", fix: "map" },
  tokenExpired: { short: "Token expired", text: "The destination's registration token has expired. Generate a new one in N-central.", fix: null },
  tokenNotChecked: { short: "Token not read", text: "The destination's registration token couldn't be read. Check again.", fix: "map" },
  restScriptMissing: { short: "Script not uploaded", text: "This device can only be reached through N-central, and the script isn't in that server's repository. Upload it in Prepare.", fix: "prepare" },
  noEcoversePath: { short: "Not in Ecoverse", text: "Ecoverse can't reach this device. Use the N-central path.", fix: "prepare" },
  unreachable: { short: "Can't reach", text: "No agent is reachable and it hasn't checked in for a long time. It is picked up if it comes back; export the list for on-site follow-up.", fix: null },
};

export const WARNING_WORDS: Record<Warning, string> = {
  tokenExpiring: "The destination's token ends soon: launches after that date fail.",
  tokenExpiryUnknown: "The destination's token expiry couldn't be read.",
  queuedUntilOnline: "Offline now: the launch is queued and runs when it reconnects.",
  checkinUnknown: "The server gave no usable check-in time, so reachability is a guess.",
  siteNotMapped: "Its site isn't mapped, so it goes to the customer instead.",
  pathOverridden: "The delivery path was changed from the suggested one.",
};

/** Whether the table's note for a row is a problem (blocked), something to know, or nothing. */
export function rowNote(r: PlanRow): { tone: "err" | "warn" | "ok" | "muted"; text: string } {
  if (r.leftOut) return { tone: "muted", text: r.leftOutByChoice ? "Left out by you" : "Left out" };
  if (r.blockers.length) return { tone: "err", text: r.blockers.map((b) => BLOCKER_WORDS[b].short).join(", ") };
  if (r.warnings.length) return { tone: "warn", text: r.warnings.map((w) => WARNING_WORDS[w]).join(" ") };
  return { tone: "ok", text: "Ready" };
}
