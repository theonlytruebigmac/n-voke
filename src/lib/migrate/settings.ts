import type { MigrationSettings } from "./bindings";
import { parseOptions, sameOptions, toForm, type OptionsForm } from "./options";

// Like the options form, the numbers are text while typed. The core checks the same ranges again.

export const TRACKING = {
  pollSeconds: { low: 15, high: 900, label: "How often to read the devices", unit: "seconds", hint: "While a project is open. Shorter is faster to notice and heavier on the servers." },
  notPickedUpMinutes: { low: 5, high: 1440, label: "Not picked up after", unit: "minutes", hint: "A launched device that is connected but hasn't started." },
  stuckStagedMinutes: { low: 10, high: 2880, label: "Stuck staged after", unit: "minutes", hint: "Staged, but the old agent still checks in." },
  lostMinutes: { low: 30, high: 10080, label: "Lost after", unit: "minutes", hint: "The old agent is gone and nothing has appeared on the new server." },
  ecoverseNotMovedMinutes: { low: 15, high: 2880, label: "Ecoverse hasn't moved after", unit: "minutes", hint: "Healthy in N-central, but not in the new server's Ecoverse." },
  healthyCheckinMinutes: { low: 2, high: 120, label: "Counts as checking in within", unit: "minutes", hint: "A device that checked in this recently is checking in." },
  agentQuietMinutes: { low: 2, high: 120, label: "Source has gone quiet after", unit: "minutes", hint: "No check-in from the old agent for this long." },
} as const;

export type TrackingKey = keyof typeof TRACKING;
export type TrackingForm = Record<TrackingKey, string>;

export interface SettingsForm {
  tracking: TrackingForm;
  defaults: OptionsForm;
}

export const toSettingsForm = (s: MigrationSettings): SettingsForm => ({
  tracking: Object.fromEntries((Object.keys(TRACKING) as TrackingKey[]).map((k) => [k, String(s[k])])) as TrackingForm,
  defaults: toForm(s.defaults),
});

export type SettingsResult = { ok: true; settings: MigrationSettings } | { ok: false; errors: Partial<Record<TrackingKey, string>>; optionErrors: Partial<Record<string, string>> };

export function parseSettings(form: SettingsForm, base: MigrationSettings): SettingsResult {
  const errors: Partial<Record<TrackingKey, string>> = {};
  const numbers = {} as Record<TrackingKey, number>;
  for (const key of Object.keys(TRACKING) as TrackingKey[]) {
    const { low, high, label, unit } = TRACKING[key];
    const text = form.tracking[key].trim();
    const value = Number(text);
    if (text === "" || !Number.isInteger(value) || value < low || value > high) errors[key] = `${label} must be a whole number from ${low} to ${high} ${unit}.`;
    else numbers[key] = value;
  }
  const options = parseOptions(form.defaults);
  if (Object.keys(errors).length || !options.ok) return { ok: false, errors, optionErrors: options.ok ? {} : options.errors };
  return { ok: true, settings: { ...base, ...numbers, defaults: options.options } };
}

export const sameSettings = (a: MigrationSettings, b: MigrationSettings) =>
  (Object.keys(TRACKING) as TrackingKey[]).every((k) => a[k] === b[k]) && sameOptions(a.defaults, b.defaults);
