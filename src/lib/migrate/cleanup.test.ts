import { describe, expect, it } from "vitest";
import type { CleanupDevice } from "./bindings";
import { cleanupScope, waveGroups } from "./cleanup";

const dev = (deviceId: string, wave: number | null, waveLabel: string | null = null): CleanupDevice => ({ deviceId, name: `PC-${deviceId}`, wave, waveLabel });
const devices = [dev("1", 2, "Friday"), dev("2", 1, "Pilot"), dev("3", null), dev("4", 1, "Pilot")];

describe("cleanup scope", () => {
  it("groups by wave in wave order with the unassigned last", () => {
    expect(waveGroups(devices).map((g) => [g.label, g.ids])).toEqual([["Pilot", ["2", "4"]], ["Friday", ["1"]], ["No wave", ["3"]]]);
  });

  it("sends nothing special when everything is ticked, and the ticked ids otherwise", () => {
    expect(cleanupScope(devices, new Set(["1", "2", "3", "4"]))).toBeNull();
    expect(cleanupScope(devices, new Set(["2", "4"]))).toEqual(["2", "4"]);
    expect(cleanupScope(devices, new Set())).toEqual([]);
  });
});
