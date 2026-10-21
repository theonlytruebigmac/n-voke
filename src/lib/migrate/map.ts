import type { MapDetails, MapRow, MapView, MatchHow, Override, TokenRow, TokenState } from "./bindings";

// What the Map step works out from the rows the core sends. The matching itself is in the core; this only counts,
// filters and words what is already decided.

export interface MapCounts {
  mapped: number;
  needsChoice: number;
  leftOut: number;
}

export function mapCounts(rows: MapRow[]): MapCounts {
  const counts: MapCounts = { mapped: 0, needsChoice: 0, leftOut: 0 };
  for (const row of rows) {
    if (row.status === "mapped") counts.mapped++;
    else if (row.status === "leftOut") counts.leftOut++;
    else counts.needsChoice++;
  }
  return counts;
}

export const needsChoice = (row: MapRow) => row.status === "ambiguous" || row.status === "unmatched";

/** The view as it will be once the choice is saved, so the row changes at once. The core keeps the real answer. */
export function applyChoice(view: MapView, sourceId: string, decision: Override): MapView {
  const dest = decision.kind === "mapTo" ? (view.destUnits.find((u) => u.id === decision.destId) ?? null) : null;
  if (decision.kind === "mapTo" && !dest) return view;
  return {
    ...view,
    rows: view.rows.map((row) =>
      row.sourceId !== sourceId
        ? row
        : decision.kind === "exclude"
          ? { ...row, status: "leftOut", how: "chosen", candidates: [] }
          : { ...row, dest, status: "mapped", how: "chosen", candidates: [] },
    ),
  };
}

/**
 * Rows to show. `onlyNeedsChoice` keeps the rows that need a choice plus their customers, for context. The text
 * matches a source or destination name; a customer that matches brings its sites, and a site that matches brings
 * its customer.
 */
export function visibleRows(rows: MapRow[], text: string, onlyNeedsChoice: boolean): MapRow[] {
  const keepByStatus = (list: MapRow[]) => {
    if (!onlyNeedsChoice) return list;
    const parents = new Set(list.filter((r) => r.kind === "site" && needsChoice(r)).map((r) => r.parentId));
    return list.filter((r) => needsChoice(r) || (r.kind === "customer" && parents.has(r.sourceId)));
  };
  const term = text.trim().toLowerCase();
  const byStatus = keepByStatus(rows);
  if (!term) return byStatus;
  const matches = (r: MapRow) => r.sourceName.toLowerCase().includes(term) || (r.dest?.name.toLowerCase().includes(term) ?? false);
  const hit = new Set(byStatus.filter(matches).map((r) => r.sourceId));
  const customersHit = new Set(byStatus.filter((r) => r.kind === "customer" && hit.has(r.sourceId)).map((r) => r.sourceId));
  const parentsOfHits = new Set(byStatus.filter((r) => r.kind === "site" && hit.has(r.sourceId)).map((r) => r.parentId));
  return byStatus.filter((r) => hit.has(r.sourceId) || parentsOfHits.has(r.sourceId) || (r.kind === "site" && r.parentId !== null && customersHit.has(r.parentId)));
}

export type TokenCell =
  | { kind: "loading" }
  /** No device goes to this destination, so its token doesn't matter. */
  | { kind: "none" }
  | { kind: "unread" }
  | { kind: "state"; state: TokenState; expires: string | null };

export function tokenCell(row: MapRow, details: MapDetails | undefined, loading: boolean): TokenCell {
  if (row.status !== "mapped" || !row.dest) return { kind: "none" };
  if (!details) return loading ? { kind: "loading" } : { kind: "unread" };
  const token = details.tokens.find((t) => t.destId === row.dest?.id);
  return token ? { kind: "state", state: token.state, expires: token.expires } : { kind: "none" };
}

export const TOKEN_WORDS: Record<TokenState, { label: string; tone: "ok" | "warn" | "err" }> = {
  valid: { label: "ready", tone: "ok" },
  expiring: { label: "expiring", tone: "warn" },
  expired: { label: "expired", tone: "err" },
  missing: { label: "missing", tone: "warn" },
  unknown: { label: "can't read expiry", tone: "warn" },
};

export const MATCH_WORDS: Record<MatchHow, { label: string; hint: string }> = {
  exact: { label: "exact", hint: "Same name under the customer it was matched to." },
  exactElsewhere: { label: "exact, other parent", hint: "Same name, but under a different customer than expected. Check it." },
  similar: { label: "similar", hint: "The names match once case, punctuation and suffixes like Inc. are ignored. Check it." },
  chosen: { label: "chosen", hint: "You chose this." },
};

export const missingTokens = (details: MapDetails | undefined): TokenRow[] => details?.tokens.filter((t) => t.state === "missing") ?? [];
