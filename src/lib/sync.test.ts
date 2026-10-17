import { describe, expect, it } from "vitest";
import { deltaSince, matches, maxUpdatedAt, mergeDelta } from "./sync";

const tag = (id: string, name: string, updatedAt: string) => ({ id, name, updatedAt });

describe("mergeDelta", () => {
  it("updates changed items, adds new ones and keeps the list sorted", () => {
    const items = [tag("1", "Alpha", "2026-10-01T00:00:00Z"), tag("2", "charlie", "2026-10-01T00:00:00Z")];
    const merged = mergeDelta(items, [tag("2", "Charlie", "2026-10-17T10:00:00Z"), tag("3", "bravo", "2026-10-17T11:00:00Z")]);
    expect(merged.map((t) => t.name)).toEqual(["Alpha", "bravo", "Charlie"]);
    expect(merged.find((t) => t.id === "2")?.updatedAt).toBe("2026-10-17T10:00:00Z");
  });

  it("returns the same array when nothing changed", () => {
    const items = [tag("1", "Alpha", "2026-10-01T00:00:00Z")];
    expect(mergeDelta(items, [])).toBe(items);
  });
});

describe("cursor", () => {
  it("tracks the newest updatedAt and overlaps the next delta slightly", () => {
    const cursor = maxUpdatedAt([tag("1", "a", "2026-10-15T06:56:33Z"), tag("2", "b", "2026-10-16T07:53:11Z"), { updatedAt: null }]);
    expect(cursor).toBe("2026-10-16T07:53:11Z");
    expect(deltaSince(cursor!)).toBe("2026-10-16T07:53:09.000Z");
  });

  it("is null for an empty collection", () => {
    expect(maxUpdatedAt([])).toBeNull();
  });
});

describe("matches", () => {
  it("does case-insensitive substring matching on any field", () => {
    expect(matches("bit", "BitlockerPreChecker", null)).toBe(true);
    expect(matches("usage", "App Usage Tracker")).toBe(true);
    expect(matches("track", "Other", "This will track the use")).toBe(true);
    expect(matches("zzz", "Other")).toBe(false);
    expect(matches("  ", "anything")).toBe(true);
  });
});
