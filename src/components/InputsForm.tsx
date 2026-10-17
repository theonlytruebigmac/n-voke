import { validateInput } from "../lib/inputs";
import type { ScriptInputDecl } from "../lib/types";
import { Tooltip, inputClass } from "./ui";

export type InputValues = Record<string, string | boolean>;

/** Script input variables as a compact two-column grid. */
export function InputsForm({
  inputs,
  values,
  onChange,
  showErrors,
}: {
  inputs: ScriptInputDecl[];
  values: InputValues;
  onChange: (values: InputValues) => void;
  showErrors: boolean;
}) {
  const set = (variable: string, value: string | boolean) => onChange({ ...values, [variable]: value });

  return (
    <div className="grid grid-cols-2 gap-x-3 gap-y-2.5">
      {inputs.map((decl) => {
        const value = values[decl.variable];
        const error = showErrors ? validateInput(decl.type, value) : null;

        if (decl.type === "BOOLEAN") {
          return (
            <label key={decl.variable} className="flex h-8 items-center gap-2 self-end text-sm">
              <input type="checkbox" className="size-3.5 accent-accent" checked={value === true} onChange={(e) => set(decl.variable, e.target.checked)} />
              {decl.name}
            </label>
          );
        }

        const type =
          decl.type === "PASSWORD" ? "password" : decl.type === "DATE_TIME" ? "datetime-local" : decl.type === "INT" || decl.type === "FLOAT" ? "number" : "text";
        return (
          <label key={decl.variable} className="block space-y-1">
            <Tooltip content={`Variable ${decl.variable} · ${decl.type.toLowerCase()}`} side="top">
              <span className="text-xs text-muted">{decl.name}</span>
            </Tooltip>
            <input
              type={type}
              step={decl.type === "INT" ? 1 : decl.type === "FLOAT" ? "any" : undefined}
              className={`${inputClass} ${error ? "border-err focus:border-err" : ""}`}
              value={typeof value === "string" ? value : ""}
              onChange={(e) => set(decl.variable, e.target.value)}
              placeholder="Optional"
              spellCheck={false}
              autoComplete="off"
              aria-invalid={!!error}
            />
            {error && <span className="block text-xs text-err">{error}</span>}
          </label>
        );
      })}
    </div>
  );
}
