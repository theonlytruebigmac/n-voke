import { describe, expect, it } from "vitest";
import { ACTION_WORDS, GROUP_LABELS, ago } from "./track";

describe("GROUP_LABELS", () => {
  it("name every group the core counts, with the strip's order kept by the core", () => {
    expect(Object.keys(GROUP_LABELS).sort()).toEqual(["done", "healthy", "launched", "moving", "needsYou", "onNew", "planned", "staged", "waiting"]);
    expect(GROUP_LABELS.waiting).toBe("Waiting for Ecoverse");
  });
});

describe("ago", () => {
  const at = Date.parse("2026-10-20T14:00:00Z");
  it("reads minutes, hours and days", () => {
    expect(ago("2026-10-20T14:00:00Z", at + 20_000)).toBe("under a minute");
    expect(ago("2026-10-20T14:00:00Z", at + 34 * 60_000)).toBe("34 min");
    expect(ago("2026-10-20T14:00:00Z", at + 130 * 60_000)).toBe("2 h");
    expect(ago("2026-10-20T14:00:00Z", at + 3 * 86_400_000)).toBe("3 d");
    expect(ago("nonsense", at)).toBe("");
  });
});

describe("ACTION_WORDS", () => {
  it("asks before anything that changes a device and not before a probe", () => {
    expect(ACTION_WORDS.probe.confirm).toBeNull();
    expect(ACTION_WORDS.cleanup.confirm).toContain("can't be undone");
    expect(ACTION_WORDS.markResolved.confirm).not.toBeNull();
  });
});
