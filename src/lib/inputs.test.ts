import { describe, expect, it } from "vitest";
import { toInputValue, validateInput } from "./inputs";

describe("toInputValue", () => {
  it("parses booleans from text without treating every non-empty string as true", () => {
    expect(toInputValue("BOOLEAN", "false")).toEqual({ booleanValue: false });
    expect(toInputValue("BOOLEAN", "0")).toEqual({ booleanValue: false });
    expect(toInputValue("BOOLEAN", "True")).toEqual({ booleanValue: true });
    expect(toInputValue("BOOLEAN", true)).toEqual({ booleanValue: true });
    expect(() => toInputValue("BOOLEAN", "maybe")).toThrow("expected true or false");
  });

  it("accepts only whole numbers in the int32 range for INT", () => {
    expect(toInputValue("INT", "42")).toEqual({ intValue: 42 });
    expect(toInputValue("INT", " -7 ")).toEqual({ intValue: -7 });
    expect(() => toInputValue("INT", "abc")).toThrow("whole number");
    expect(() => toInputValue("INT", "3.5")).toThrow("whole number");
    expect(() => toInputValue("INT", "3000000000")).toThrow("whole number");
  });

  it("parses FLOAT values", () => {
    expect(toInputValue("FLOAT", "2.5")).toEqual({ floatValue: 2.5 });
    expect(() => toInputValue("FLOAT", "x")).toThrow("expected a number");
    expect(() => toInputValue("FLOAT", "")).toThrow("expected a number");
  });

  it("normalizes DATE_TIME values to UTC", () => {
    expect(toInputValue("DATE_TIME", "2026-10-17T13:00:00Z")).toEqual({ dateTimeValue: "2026-10-17T13:00:00Z" });
    expect(toInputValue("DATE_TIME", "2026-10-17T13:00:00-04:00")).toEqual({
      dateTimeValue: "2026-10-17T17:00:00Z",
    });
    expect(() => toInputValue("DATE_TIME", "nope")).toThrow("date/time");
  });

  it("passes strings and passwords through unchanged", () => {
    expect(toInputValue("STRING", "chrome")).toEqual({ stringValue: "chrome" });
    expect(toInputValue("PASSWORD", " s3cret ")).toEqual({ stringValue: " s3cret " });
  });
});

describe("validateInput", () => {
  it("treats blank as valid (the input is skipped)", () => {
    expect(validateInput("INT", "")).toBeNull();
    expect(validateInput("INT", undefined)).toBeNull();
  });

  it("returns the reason for an invalid value", () => {
    expect(validateInput("INT", "abc")).toBe("expected a whole number");
  });
});
