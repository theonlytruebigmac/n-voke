import { describe, expect, it } from "vitest";
import type { MapDetails, MapRow, MapView, UnitRef } from "./bindings";
import { applyChoice, mapCounts, missingTokens, tokenCell, visibleRows } from "./map";

const unit = (id: string, name: string, kind: "customer" | "site" = "customer"): UnitRef => ({ id, name, kind, parentName: null });
const row = (sourceId: string, sourceName: string, over: Partial<MapRow> = {}): MapRow => ({
  sourceId,
  sourceName,
  kind: "customer",
  parentId: null,
  dest: null,
  how: null,
  status: "mapped",
  candidates: [],
  ...over,
});

const rows: MapRow[] = [
  row("1", "Acme", { dest: unit("11", "Acme"), how: "exact" }),
  row("2", "HQ", { kind: "site", parentId: "1", dest: unit("21", "HQ", "site"), how: "exact" }),
  row("3", "Annex", { kind: "site", parentId: "1", status: "unmatched" }),
  row("4", "Zeta", { status: "ambiguous", candidates: [unit("41", "Zeta"), unit("42", "Zeta")] }),
  row("5", "Old Dept", { status: "leftOut", how: "chosen" }),
  row("6", "Beta", { dest: unit("61", "Beta"), how: "similar" }),
];

describe("mapCounts", () => {
  it("counts matched, needing a choice (ambiguous or unmatched) and left out", () => {
    expect(mapCounts(rows)).toEqual({ mapped: 3, needsChoice: 2, leftOut: 1 });
  });
});

describe("applyChoice", () => {
  const view: MapView = { sourceLabel: "Old", destLabel: "New", rows, destUnits: [unit("41", "Zeta")] };

  it("shows a chosen destination at once, as the operator's", () => {
    const next = applyChoice(view, "4", { kind: "mapTo", destId: "41" });
    expect(next.rows.find((r) => r.sourceId === "4")).toMatchObject({ status: "mapped", how: "chosen", candidates: [], dest: { id: "41" } });
    expect(next.rows.find((r) => r.sourceId === "1")).toBe(rows[0]);
  });

  it("shows a left-out row as the operator's choice and keeps no destination", () => {
    const next = applyChoice(view, "1", { kind: "exclude" });
    expect(next.rows[0]).toMatchObject({ status: "leftOut", how: "chosen" });
  });

  it("ignores a destination that isn't on offer", () => {
    expect(applyChoice(view, "4", { kind: "mapTo", destId: "999" })).toBe(view);
  });
});

describe("visibleRows", () => {
  const ids = (list: MapRow[]) => list.map((r) => r.sourceId);

  it("shows everything without a filter", () => {
    expect(ids(visibleRows(rows, "", false))).toEqual(["1", "2", "3", "4", "5", "6"]);
  });

  it("keeps only rows that need a choice, with the customer of a site for context", () => {
    expect(ids(visibleRows(rows, "", true))).toEqual(["1", "3", "4"]);
  });

  it("brings a customer's sites when the customer matches, and the customer when a site matches", () => {
    expect(ids(visibleRows(rows, "acme", false))).toEqual(["1", "2", "3"]);
    expect(ids(visibleRows(rows, "annex", false))).toEqual(["1", "3"]);
  });

  it("matches the destination's name too, ignoring case", () => {
    expect(ids(visibleRows(rows, "BETA", false))).toEqual(["6"]);
  });
});

describe("tokenCell", () => {
  const details: MapDetails = { devices: {}, tokens: [{ destId: "11", destName: "Acme", state: "missing", expires: null }], warnings: [] };

  it("says nothing for a row that has no destination or no devices going there", () => {
    expect(tokenCell(rows[3], details, false)).toEqual({ kind: "none" });
    expect(tokenCell(rows[5], details, false)).toEqual({ kind: "none" });
  });

  it("shows the destination's token, and loading while the details are read", () => {
    expect(tokenCell(rows[0], details, false)).toEqual({ kind: "state", state: "missing", expires: null });
    expect(tokenCell(rows[0], undefined, true)).toEqual({ kind: "loading" });
    expect(tokenCell(rows[0], undefined, false)).toEqual({ kind: "unread" });
  });

  it("lists only the missing tokens to generate", () => {
    expect(missingTokens(details).map((t) => t.destName)).toEqual(["Acme"]);
    expect(missingTokens(undefined)).toEqual([]);
  });
});
