import type { InputType, InputValue } from "./types";

/**
 * Converts a form value into the API's typed value object. Throws an Error
 * whose message is a short reason when the value doesn't fit the type.
 */
export function toInputValue(type: InputType, raw: string | boolean): InputValue {
  const text = typeof raw === "string" ? raw.trim() : String(raw);

  switch (type) {
    case "BOOLEAN": {
      if (typeof raw === "boolean") return { booleanValue: raw };
      if (/^(true|yes|y|1)$/i.test(text)) return { booleanValue: true };
      if (/^(false|no|n|0)$/i.test(text)) return { booleanValue: false };
      throw new Error("expected true or false");
    }
    case "INT": {
      const n = Number(text);
      if (/^[+-]?\d+$/.test(text) && Number.isSafeInteger(n) && n >= -2147483648 && n <= 2147483647) {
        return { intValue: n };
      }
      throw new Error("expected a whole number");
    }
    case "FLOAT": {
      const n = Number(text);
      if (text !== "" && Number.isFinite(n)) return { floatValue: n };
      throw new Error("expected a number");
    }
    case "DATE_TIME": {
      // A value without an offset (e.g. from <input type="datetime-local">)
      // is local time, per the JS date-time parsing rules.
      const ms = Date.parse(text);
      if (text === "" || Number.isNaN(ms)) throw new Error("expected a date/time, e.g. 2026-10-17T13:00:00Z");
      return { dateTimeValue: new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z") };
    }
    default:
      return { stringValue: typeof raw === "string" ? raw : String(raw) };
  }
}

/** Returns the reason a value is invalid, or null when it's fine (or blank). */
export function validateInput(type: InputType, raw: string | boolean | undefined): string | null {
  if (raw === undefined || raw === "") return null;
  try {
    toInputValue(type, raw);
    return null;
  } catch (e) {
    return (e as Error).message;
  }
}
