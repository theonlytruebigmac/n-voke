import { Check } from "lucide-react";
import { Tooltip } from "../../ui";

export type StepId = "connect" | "map" | "prepare" | "plan" | "launch" | "track" | "finish";

export const STEPS: { id: StepId; label: string }[] = [
  { id: "connect", label: "Connect" },
  { id: "map", label: "Map" },
  { id: "prepare", label: "Prepare" },
  { id: "plan", label: "Plan" },
  { id: "launch", label: "Launch" },
  { id: "track", label: "Track" },
  { id: "finish", label: "Finish" },
];

export type StepMark = "done" | "current" | "blocked" | "todo";

/** The seven steps down the left of a project, with where it stands (docs/migration/11-gui-design.md, "Where it lives"). */
export function StepRail({
  current,
  marks,
  available,
  onSelect,
}: {
  current: StepId;
  marks: Partial<Record<StepId, StepMark>>;
  /** Steps that can be opened now; the others are shown but unavailable. */
  available: ReadonlySet<StepId>;
  onSelect: (step: StepId) => void;
}) {
  return (
    <ol className="w-44 shrink-0 space-y-0.5 border-r border-border px-2 py-3" aria-label="Steps">
      {STEPS.map((s, i) => {
        const mark = marks[s.id] ?? "todo";
        const isCurrent = s.id === current;
        const open = available.has(s.id);
        const button = (
          <button
            disabled={!open}
            aria-current={isCurrent ? "step" : undefined}
            onClick={() => onSelect(s.id)}
            className={`flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-sm transition-colors disabled:cursor-default ${
              isCurrent ? "bg-surface-3 font-medium text-text" : open ? "text-muted hover:bg-surface-2 hover:text-text" : "text-faint"
            }`}
          >
            <span
              className={`flex size-5 shrink-0 items-center justify-center rounded-full border text-[11px] tabular-nums ${
                mark === "done" ? "border-ok bg-ok-soft text-ok" : mark === "blocked" ? "border-warn bg-warn-soft text-warn" : isCurrent ? "border-accent text-accent" : "border-border text-faint"
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
