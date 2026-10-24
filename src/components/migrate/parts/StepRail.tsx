import { Check } from "lucide-react";

export { STEPS, type StepId, type StepMark } from "../../../lib/migrate/steps";
import { STEPS, type StepId, type StepMark } from "../../../lib/migrate/steps";

/** The four steps that set a project up, shown as one group on the overview and as chips over each of them. */
export const SETUP_STEPS = STEPS.filter((s) => s.id === "connect" || s.id === "map" || s.id === "prepare" || s.id === "plan");

export const isSetupStep = (id: StepId) => SETUP_STEPS.some((s) => s.id === id);

/**
 * The set-up steps as chips: a check once done, the open one highlighted. On the overview they sit under "Set up";
 * over a set-up step they let you move between its siblings (docs/migration/11-gui-design.md, "Where it lives").
 */
export function SetupChips({
  current,
  marks,
  notes = {},
  onSelect,
}: {
  current?: StepId;
  marks: Partial<Record<StepId, StepMark>>;
  /** Why a step is marked as it is, shown when hovering it. */
  notes?: Partial<Record<StepId, string>>;
  onSelect: (step: StepId) => void;
}) {
  return (
    <ol aria-label="Set up steps" className="flex flex-wrap gap-1.5">
      {SETUP_STEPS.map((s) => {
        const isCurrent = s.id === current;
        const done = marks[s.id] === "done";
        return (
          <li key={s.id}>
            <button
              type="button"
              aria-current={isCurrent ? "step" : undefined}
              title={notes[s.id]}
              onClick={() => onSelect(s.id)}
              className={`inline-flex h-7 items-center gap-1.5 rounded-full border px-3 text-xs whitespace-nowrap transition-colors ${
                isCurrent ? "border-accent/50 bg-accent-soft font-semibold text-text" : done ? "border-control-border bg-surface-2 text-muted hover:text-text" : "border-control-border text-faint hover:text-text"
              }`}
            >
              {done && !isCurrent && <Check className="size-3 text-ok" strokeWidth={3} />}
              {s.label}
            </button>
          </li>
        );
      })}
    </ol>
  );
}
