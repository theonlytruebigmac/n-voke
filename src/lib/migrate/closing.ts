import type { FinishView } from "./bindings";

const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

/** "a", "a and b", "a, b and c". */
const listed = (parts: string[]) => (parts.length < 2 ? parts.join("") : `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`);

/**
 * What is still unfinished, in the order a person deals with it, each with its count: what keeps an open project open,
 * and what closing it by hand leaves as it is.
 */
export function unfinishedParts(v: FinishView): string[] {
  const parts: string[] = [];
  if (v.needsYou > 0) parts.push(`${v.needsYou.toLocaleString()} ${v.needsYou === 1 ? "needs" : "need"} you`);
  if (v.moving > 0) parts.push(`${v.moving.toLocaleString()} still moving`);
  if (v.waiting > 0) parts.push(`${v.waiting.toLocaleString()} not launched yet`);
  if (v.healthy > 0) parts.push(`${plural(v.healthy, "healthy device")} to clean up`);
  if (v.toRetire > 0) {
    const ends = v.nextReviewEnds ? ` (the first review period ends ${new Date(v.nextReviewEnds).toLocaleDateString()})` : "";
    parts.push(`${plural(v.toRetire, "old record")} to retire${ends}`);
  }
  return parts;
}

/** The sentence for an open project: it closes by itself (unless it was reopened), and what is left before it does. */
export function whatKeepsItOpen(v: FinishView): string {
  const parts = unfinishedParts(v);
  if (v.reopenedAt) return parts.length === 0 ? "You reopened it with every device finished, so it stays open until you close it." : `It stays open until you close it, since you reopened it. Left: ${listed(parts)}.`;
  if (parts.length === 0) return v.total === 0 ? "There are no devices in this project yet." : "It closes by itself once every device is finished.";
  return `It closes by itself once every device is finished. Left: ${listed(parts)}.`;
}
