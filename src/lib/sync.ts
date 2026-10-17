/**
 * Delta sync for cached collections. N-able has no change feed, but tag and
 * script searches accept `updatedAt: { gt }`, so after one full load the app
 * only asks for what changed. Deletions don't show up in a delta, so the total
 * count is checked too and any mismatch triggers a full reload.
 */

export interface Synced<T> {
  items: T[];
  /** Latest `updatedAt` seen; deltas ask for anything newer. */
  cursor: string | null;
  /** When the last full (not delta) load happened, ms epoch. */
  fullAt: number;
}

/** Re-ask for this much before the cursor, so same-second edits aren't missed. */
const OVERLAP_MS = 2000;

export function maxUpdatedAt(items: { updatedAt?: string | null }[]): string | null {
  let max: string | null = null;
  for (const i of items) if (i.updatedAt && (!max || i.updatedAt > max)) max = i.updatedAt;
  return max;
}

/** Timestamp to send as `updatedAt: { gt }` for the next delta. */
export function deltaSince(cursor: string): string {
  return new Date(new Date(cursor).getTime() - OVERLAP_MS).toISOString();
}

export const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" });

/** Upserts changed items by id; returns a new array sorted by name. */
export function mergeDelta<T extends { id: string; name: string }>(items: T[], changed: T[]): T[] {
  if (!changed.length) return items;
  const map = new Map(items.map((i) => [i.id, i]));
  for (const c of changed) map.set(c.id, c);
  return [...map.values()].sort(byName);
}

/** Case-insensitive substring match on any of the given fields. */
export function matches(term: string, ...fields: (string | null | undefined)[]): boolean {
  const t = term.trim().toLowerCase();
  return !t || fields.some((f) => f?.toLowerCase().includes(t));
}
