import { Check } from "lucide-react";
import { Tooltip } from "../../ui";

export { STEPS, type StepId, type StepMark } from "../../../lib/migrate/steps";
import { STEPS, type StepId, type StepMark } from "../../../lib/migrate/steps";

/** The seven steps down the left of a project, with where it stands (docs/migration/11-gui-design.md, "Where it lives"). */
export function StepRail({
  current,
  marks,
  notes = {},
  available,
  onSelect,
}: {
  current: StepId;
  /** A green check for a step that is done, an outline for one that is started. */
  marks: Partial<Record<StepId, StepMark>>;
  /** Why a step is marked as it is, shown when hovering it. */
  notes?: Partial<Record<StepId, string>>;
  /** Steps that can be opened now; the others are shown but unavailable. */
  available: ReadonlySet<StepId>;
  onSelect: (step: StepId) => void;
}) {
  return (
    <ol className="w-44 shrink-0 space-y-0.5 border-r border-border px-2 py-3" aria-label="Steps">
      {STEPS.map((s, i) => {
        const mark = marks[s.id];
        const isCurrent = s.id === current;
        const open = available.has(s.id);
        const button = (
          <button
            disabled={!open}
            aria-current={isCurrent ? "step" : undefined}
            title={notes[s.id]}
            onClick={() => onSelect(s.id)}
            className={`flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-sm transition-colors disabled:cursor-default ${
              isCurrent ? "bg-surface-3 font-medium text-text" : open ? "text-muted hover:bg-surface-2 hover:text-text" : "text-faint"
            }`}
          >
            <span
              className={`flex size-5 shrink-0 items-center justify-center rounded-full border text-[11px] tabular-nums ${
                mark === "done" ? "border-ok bg-ok-soft text-ok" : mark === "partial" ? "border-accent bg-accent-soft text-accent" : isCurrent ? "border-accent text-accent" : "border-border text-faint"
              }`}
            >
              {mark === "done" ? <Check className="size-3" /> : i + 1}
            </span>
            <span className="flex-1 text-left">{s.label}</span>
          </button>
        );
        return <li key={s.id}>{open ? button : <Tooltip content="Not available yet" side="right"><span className="block">{button}</span></Tooltip>}</li>;
      })}
    </ol>
  );
}
