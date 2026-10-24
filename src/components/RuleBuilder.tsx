import { Plus, X } from "lucide-react";
import { OP_LABELS, RULE_FIELDS, type Rule, type RuleCondition, type RuleField } from "../lib/rules";
import { Button, IconButton, inputClass, selectClass } from "./ui";

export function RuleBuilder({ rule, onChange }: { rule: Rule; onChange: (rule: Rule) => void }) {
  const setCondition = (i: number, patch: Partial<RuleCondition>) =>
    onChange({ ...rule, conditions: rule.conditions.map((c, j) => (j === i ? { ...c, ...patch } : c)) });

  return (
    <div className="space-y-2.5">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        Devices that match
        <select className={`${selectClass} w-auto`} value={rule.match} aria-label="Match" onChange={(e) => onChange({ ...rule, match: e.target.value as Rule["match"] })}>
          <option value="all">all</option>
          <option value="any">any</option>
        </select>
        of these
      </div>

      {rule.conditions.map((c, i) => {
        const fieldDef = RULE_FIELDS.find((f) => f.field === c.field)!;
        return (
          <div key={i} className="flex flex-wrap items-center gap-2">
            <select
              className={`${selectClass} min-w-36 flex-[1_1_140px]`}
              value={c.field}
              aria-label="Field"
              onChange={(e) => {
                const field = e.target.value as RuleField;
                const ops = RULE_FIELDS.find((f) => f.field === field)!.ops;
                setCondition(i, { field, op: ops.includes(c.op) ? c.op : ops[0] });
              }}
            >
              {RULE_FIELDS.map((f) => (
                <option key={f.field} value={f.field}>
                  {f.label}
                </option>
              ))}
            </select>
            <select
              className={`${selectClass} min-w-28 flex-[0_1_120px]`}
              value={c.op}
              aria-label="Operator"
              onChange={(e) => setCondition(i, { op: e.target.value as RuleCondition["op"] })}
            >
              {fieldDef.ops.map((op) => (
                <option key={op} value={op}>
                  {OP_LABELS[op]}
                </option>
              ))}
            </select>
            <input
              className={`${inputClass} min-w-40 flex-[2_1_160px]`}
              value={c.value}
              aria-label="Value"
              placeholder={c.field === "tag" ? "Tag name" : c.field === "os" ? "Windows 11" : ""}
              onChange={(e) => setCondition(i, { value: e.target.value })}
              spellCheck={false}
            />
            <IconButton
              label="Remove condition"
              onClick={() => onChange({ ...rule, conditions: rule.conditions.filter((_, j) => j !== i) })}
              disabled={rule.conditions.length === 1}
            >
              <X />
            </IconButton>
          </div>
        );
      })}

      <Button
        variant="ghost"
        size="sm"
        className="border border-dashed border-border-strong text-accent hover:text-accent-hover"
        onClick={() => onChange({ ...rule, conditions: [...rule.conditions, { field: "name", op: "contains", value: "" }] })}
      >
        <Plus />
        Add condition
      </Button>
    </div>
  );
}
