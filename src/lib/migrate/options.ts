import type { CleanMode, Delivery, ProjectOptions } from "./bindings";

// The options form keeps numbers as text while they are typed. The core checks the ranges again; these only stop
// obvious mistakes before a round trip and say what the limit is.

export interface OptionsForm {
  reviewDays: string;
  cleanMode: CleanMode;
  taskRepeatMinutes: string;
  taskExpiryDays: string;
  waveSize: string;
  retireSourceRecords: boolean;
  delivery: Delivery;
}

export const toForm = (o: ProjectOptions): OptionsForm => ({
  reviewDays: String(o.reviewDays),
  cleanMode: o.cleanMode,
  taskRepeatMinutes: String(o.taskRepeatMinutes),
  taskExpiryDays: String(o.taskExpiryDays),
  waveSize: String(o.waveSize),
  retireSourceRecords: o.retireSourceRecords,
  delivery: o.delivery,
});

export const LIMITS = {
  reviewDays: { low: 0, high: 999, label: "The review period", unit: "days" },
  waveSize: { low: 1, high: 500, label: "The wave size", unit: "devices" },
  taskRepeatMinutes: { low: 5, high: 1440, label: "How often the device retries", unit: "minutes" },
  taskExpiryDays: { low: 1, high: 90, label: "How long the device keeps retrying", unit: "days" },
} as const;

type Numeric = keyof typeof LIMITS;
export type OptionsResult = { ok: true; options: ProjectOptions } | { ok: false; errors: Partial<Record<Numeric, string>> };

export function parseOptions(form: OptionsForm): OptionsResult {
  const errors: Partial<Record<Numeric, string>> = {};
  const numbers = {} as Record<Numeric, number>;
  for (const key of Object.keys(LIMITS) as Numeric[]) {
    const { low, high, label, unit } = LIMITS[key];
    const text = form[key].trim();
    const value = Number(text);
    if (text === "" || !Number.isInteger(value) || value < low || value > high) errors[key] = `${label} must be a whole number from ${low} to ${high} ${unit}.`;
    else numbers[key] = value;
  }
  if (Object.keys(errors).length) return { ok: false, errors };
  return { ok: true, options: { ...numbers, cleanMode: form.cleanMode, retireSourceRecords: form.retireSourceRecords, delivery: form.delivery } };
}

export const sameOptions = (a: ProjectOptions, b: ProjectOptions) =>
  a.reviewDays === b.reviewDays &&
  a.cleanMode === b.cleanMode &&
  a.taskRepeatMinutes === b.taskRepeatMinutes &&
  a.taskExpiryDays === b.taskExpiryDays &&
  a.waveSize === b.waveSize &&
  a.retireSourceRecords === b.retireSourceRecords &&
  a.delivery === b.delivery;
