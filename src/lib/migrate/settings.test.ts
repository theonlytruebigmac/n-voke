import { describe, expect, it } from "vitest";
import type { MigrationSettings } from "./bindings";
import { parseSettings, sameSettings, toSettingsForm } from "./settings";

const base: MigrationSettings = {
  pollSeconds: 60,
  notPickedUpMinutes: 30,
  stuckStagedMinutes: 90,
  lostMinutes: 240,
  ecoverseNotMovedMinutes: 75,
  healthyCheckinMinutes: 10,
  agentQuietMinutes: 10,
  autoProbeMinutes: 30,
  defaults: { reviewDays: 7, cleanMode: "full", taskRepeatMinutes: 15, taskExpiryDays: 7, waveSize: 25, retireSourceRecords: false, delivery: "ecoverseFirst" },
};

describe("migration settings form", () => {
  it("round-trips the saved settings unchanged", () => {
    const parsed = parseSettings(toSettingsForm(base), base);
    expect(parsed.ok && parsed.settings).toEqual(base);
    expect(parsed.ok && sameSettings(parsed.settings, base)).toBe(true);
  });

  it("names the limit that a tracking number breaks", () => {
    const form = toSettingsForm(base);
    form.tracking.pollSeconds = "5";
    form.tracking.lostMinutes = "soon";
    const parsed = parseSettings(form, base);
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) {
      expect(parsed.errors.pollSeconds).toContain("from 15 to 900 seconds");
      expect(parsed.errors.lostMinutes).toContain("whole number");
    }
  });

  it("refuses a default option out of range and keeps the rest", () => {
    const form = toSettingsForm(base);
    form.defaults.waveSize = "0";
    const parsed = parseSettings(form, base);
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.optionErrors.waveSize).toContain("1 to 500");
  });

  it("sees a change", () => {
    const form = toSettingsForm(base);
    form.tracking.pollSeconds = "30";
    const parsed = parseSettings(form, base);
    expect(parsed.ok && sameSettings(parsed.settings, base)).toBe(false);
  });
});
