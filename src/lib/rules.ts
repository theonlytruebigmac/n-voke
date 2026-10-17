/**
 * Dynamic tag rules as the editor sees them: "match all/any" of a flat list
 * of conditions. Converts to and from the API filter JSON, which has the same
 * shape for `AssetDynamicTagWhereInput` (saving) and `AssetWhereInput`
 * (previewing with assetSearch).
 */

export type RuleField = "name" | "os" | "customer" | "site" | "tag";
export type RuleOp = "contains" | "equals" | "startsWith";

export interface RuleCondition {
  field: RuleField;
  op: RuleOp;
  value: string;
}

export interface Rule {
  match: "all" | "any";
  conditions: RuleCondition[];
}

export const RULE_FIELDS: { field: RuleField; label: string; ops: RuleOp[] }[] = [
  { field: "name", label: "Device name", ops: ["contains", "equals", "startsWith"] },
  { field: "os", label: "Operating system", ops: ["contains", "equals", "startsWith"] },
  { field: "customer", label: "Customer", ops: ["contains", "equals", "startsWith"] },
  { field: "site", label: "Site", ops: ["contains", "equals", "startsWith"] },
  { field: "tag", label: "Has tag", ops: ["equals"] },
];

export const OP_LABELS: Record<RuleOp, string> = { contains: "contains", equals: "is", startsWith: "starts with" };

const PATHS: Record<RuleField, string[]> = {
  name: ["name"],
  os: ["operatingSystem", "name"],
  customer: ["customer", "name"],
  site: ["site", "name"],
  tag: ["tags", "name"],
};

export function emptyRule(): Rule {
  return { match: "all", conditions: [{ field: "name", op: "contains", value: "" }] };
}

/** Conditions with a value; blank rows are ignored. */
export function activeConditions(rule: Rule): RuleCondition[] {
  return rule.conditions.filter((c) => c.value.trim() !== "");
}

export function ruleToFilter(rule: Rule): object {
  const clauses = activeConditions(rule).map((c) => {
    const leaf = { [c.op]: c.value.trim(), caseSensitivity: "NON_SENSITIVE" };
    return PATHS[c.field].reduceRight<object>((inner, key) => ({ [key]: inner }), leaf);
  });
  return { [rule.match === "all" ? "and" : "or"]: clauses };
}

/**
 * Parses a stored filter back into an editable rule. Returns null when the
 * filter uses anything the editor can't represent (nesting, other fields).
 */
export function filterToRule(filter: unknown): Rule | null {
  const f = typeof filter === "string" ? safeParse(filter) : filter;
  if (!f || typeof f !== "object") return null;
  const keys = Object.keys(f);
  if (keys.length !== 1 || (keys[0] !== "and" && keys[0] !== "or")) return null;
  const clauses = (f as Record<string, unknown>)[keys[0]];
  if (!Array.isArray(clauses)) return null;

  const conditions: RuleCondition[] = [];
  for (const clause of clauses) {
    const condition = clauseToCondition(clause);
    if (!condition) return null;
    conditions.push(condition);
  }
  return { match: keys[0] === "and" ? "all" : "any", conditions };
}

function clauseToCondition(clause: unknown): RuleCondition | null {
  for (const [field, path] of Object.entries(PATHS) as [RuleField, string[]][]) {
    let node: unknown = clause;
    for (const key of path) {
      if (!node || typeof node !== "object" || Object.keys(node).length !== 1 || !(key in node)) {
        node = undefined;
        break;
      }
      node = (node as Record<string, unknown>)[key];
    }
    if (!node || typeof node !== "object") continue;
    const leaf = node as Record<string, unknown>;
    const op = (["contains", "equals", "startsWith"] as RuleOp[]).find((o) => typeof leaf[o] === "string");
    const extra = Object.keys(leaf).filter((k) => k !== op && k !== "caseSensitivity");
    if (op && extra.length === 0) return { field, op, value: leaf[op] as string };
  }
  return null;
}

function safeParse(s: string): unknown {
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
}

/** One-line summary, e.g. `Device name contains "SLS" and Site is "Lab"`. */
export function describeRule(rule: Rule): string {
  const parts = activeConditions(rule).map((c) => {
    const label = RULE_FIELDS.find((f) => f.field === c.field)?.label ?? c.field;
    return `${label} ${OP_LABELS[c.op]} “${c.value.trim()}”`;
  });
  return parts.length ? parts.join(rule.match === "all" ? " and " : " or ") : "No conditions";
}

/** Tag names: letters, numbers, hyphens and underscores, up to 64 characters. */
export function validateTagName(name: string): string | null {
  if (!name) return "Enter a name.";
  if (name.length > 64) return "Use 64 characters or fewer.";
  if (!/^[A-Za-z0-9_-]+$/.test(name)) return "Use only letters, numbers, hyphens and underscores (no spaces).";
  return null;
}

/** Turns free text into a valid tag name suggestion: "Bitlocker not ready" -> "Bitlocker_not_ready". */
export function suggestTagName(text: string): string {
  return text
    .trim()
    .replace(/\s+/g, "_")
    .replace(/[^A-Za-z0-9_-]/g, "")
    .slice(0, 64);
}
