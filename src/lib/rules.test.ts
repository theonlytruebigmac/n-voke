import { describe, expect, it } from "vitest";
import { describeRule, filterToRule, ruleToFilter, suggestTagName, validateTagName, type Rule } from "./rules";

describe("ruleToFilter", () => {
  it("builds the nested filter shape the API stores", () => {
    const rule: Rule = {
      match: "any",
      conditions: [
        { field: "name", op: "equals", value: "SLS-2188" },
        { field: "os", op: "contains", value: "Windows 10" },
        { field: "tag", op: "equals", value: "PatchGroupTwo" },
      ],
    };
    expect(ruleToFilter(rule)).toEqual({
      or: [
        { name: { equals: "SLS-2188", caseSensitivity: "NON_SENSITIVE" } },
        { operatingSystem: { name: { contains: "Windows 10", caseSensitivity: "NON_SENSITIVE" } } },
        { tags: { name: { equals: "PatchGroupTwo", caseSensitivity: "NON_SENSITIVE" } } },
      ],
    });
  });

  it("skips blank conditions", () => {
    expect(ruleToFilter({ match: "all", conditions: [{ field: "site", op: "contains", value: "  " }] })).toEqual({ and: [] });
  });
});

describe("filterToRule", () => {
  it("parses the filter stored on an existing dynamic tag", () => {
    // Exactly what the API returned for the Windows10 tag.
    const stored = '{"and": [{"operatingSystem": {"name": {"contains": "Windows 10", "caseSensitivity": "NON_SENSITIVE"}}}]}';
    expect(filterToRule(stored)).toEqual({ match: "all", conditions: [{ field: "os", op: "contains", value: "Windows 10" }] });
  });

  it("round-trips a rule", () => {
    const rule: Rule = {
      match: "all",
      conditions: [
        { field: "customer", op: "startsWith", value: "Jason" },
        { field: "site", op: "equals", value: "Parsippany" },
      ],
    };
    expect(filterToRule(ruleToFilter(rule))).toEqual(rule);
  });

  it("returns null for filters the editor can't represent", () => {
    expect(filterToRule({ and: [{ or: [{ name: { equals: "x" } }] }] })).toBeNull();
    expect(filterToRule({ and: [{ cpu: { cores: { gte: 4 } } }] })).toBeNull();
    expect(filterToRule("not json")).toBeNull();
    expect(filterToRule({ and: [], or: [] })).toBeNull();
  });
});

describe("describeRule", () => {
  it("summarizes conditions", () => {
    expect(
      describeRule({
        match: "any",
        conditions: [
          { field: "name", op: "contains", value: "SLS" },
          { field: "site", op: "equals", value: "Lab" },
        ],
      }),
    ).toBe("Device name contains “SLS” or Site is “Lab”");
  });
});

describe("tag names", () => {
  it("validates the API's naming rules", () => {
    expect(validateTagName("Patch_Group-2")).toBeNull();
    expect(validateTagName("has space")).toMatch(/no spaces/);
    expect(validateTagName("x".repeat(65))).toMatch(/64/);
    expect(validateTagName("")).toMatch(/Enter/);
  });

  it("suggests a valid name from free text", () => {
    expect(suggestTagName("  Bitlocker not ready! ")).toBe("Bitlocker_not_ready");
  });
});
