import { describe, expect, it } from "vitest";
import type { ProjectOptions } from "./bindings";
import { parseOptions, sameOptions, toForm } from "./options";

const defaults: ProjectOptions = { reviewDays: 7, cleanMode: "full", taskRepeatMinutes: 15, taskExpiryDays: 14, waveSize: 50, retireSourceRecords: false, delivery: "ecoverseFirst" };

describe("parseOptions", () => {
  it("round-trips the saved options", () => {
    const parsed = parseOptions(toForm(defaults));
    expect(parsed).toEqual({ ok: true, options: defaults });
  });

  it("accepts a review period of zero and the edges of every range", () => {
    const form = { ...toForm(defaults), reviewDays: "0", waveSize: "500", taskRepeatMinutes: "5", taskExpiryDays: "90" };
    expect(parseOptions(form).ok).toBe(true);
  });

  it("names each field that is out of range or not a whole number, with its limits", () => {
    const parsed = parseOptions({ ...toForm(defaults), reviewDays: "1000", waveSize: "0", taskRepeatMinutes: "4.5", taskExpiryDays: "" });
    expect(parsed.ok).toBe(false);
    if (parsed.ok) return;
    expect(parsed.errors.reviewDays).toContain("0 to 999 days");
    expect(parsed.errors.waveSize).toContain("1 to 500 devices");
    expect(parsed.errors.taskRepeatMinutes).toContain("5 to 1440 minutes");
    expect(parsed.errors.taskExpiryDays).toContain("1 to 90 days");
  });

  it("trims what was typed", () => {
    const parsed = parseOptions({ ...toForm(defaults), waveSize: " 25 " });
    expect(parsed.ok && parsed.options.waveSize).toBe(25);
  });
});

describe("sameOptions", () => {
  it("tells changed options from unchanged ones", () => {
    expect(sameOptions(defaults, { ...defaults })).toBe(true);
    expect(sameOptions(defaults, { ...defaults, cleanMode: "none" })).toBe(false);
  });
});
