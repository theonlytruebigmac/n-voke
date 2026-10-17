import { Tag as TagIcon, Zap } from "lucide-react";
import { humanize } from "../lib/format";
import { outcomeLabel, taskOutcome, taskPhase, type JobTask, type TaskPhase } from "../lib/taskState";
import type { TagKind } from "../lib/types";
import { Dot, Status, Tooltip, type Tone } from "./ui";

export const PHASE_TONE: Record<TaskPhase, Tone> = {
  queued: "accent",
  running: "accent",
  succeeded: "ok",
  nonZero: "warn",
  failed: "err",
  stopped: "muted",
};

export const PHASE_WORD: Record<TaskPhase, string> = {
  queued: "Waiting",
  running: "Running",
  succeeded: "Passed",
  nonZero: "Non-zero exit",
  failed: "Didn't run",
  stopped: "Not watching",
};

/** Short status for one device's task: "Passed", "Exit 1001", "Waiting"... */
export function taskStatus(t: JobTask): { tone: Tone; label: string; pulse: boolean; title?: string } {
  const phase = taskPhase(t);
  const outcome = taskOutcome(t);
  const label = (phase === "nonZero" || phase === "failed") && outcome ? outcomeLabel(outcome) : PHASE_WORD[phase];
  const title =
    phase === "nonZero"
      ? "The script ran. N-able counts any non-zero exit code as failed."
      : phase === "queued"
        ? "Waiting for the device to pick up the task"
        : undefined;
  return { tone: PHASE_TONE[phase], label: label === "Succeeded" ? "Passed" : label, pulse: phase === "running", title };
}

export function TaskStatus({ task }: { task: JobTask }) {
  const s = taskStatus(task);
  return <Status tone={s.tone} label={s.label} pulse={s.pulse} title={s.title} />;
}

export function AgentDot({ status }: { status: string | undefined | null }) {
  const tone: Tone = status === "CONNECTED" ? "ok" : status ? "err" : "muted";
  return (
    <Tooltip content={status ? `Agent ${humanize(status).toLowerCase()}` : "Agent status unknown"}>
      <span className="inline-flex">
        <Dot tone={tone} />
      </span>
    </Tooltip>
  );
}

export function TagKindIcon({ kind, className = "size-3.5" }: { kind: TagKind; className?: string }) {
  return kind === "DynamicTag" ? (
    <Zap className={`${className} shrink-0 text-warn`} aria-label="Dynamic tag" />
  ) : (
    <TagIcon className={`${className} shrink-0 text-muted`} aria-label="Static tag" />
  );
}

export function tagKindLabel(kind: TagKind) {
  return kind === "DynamicTag" ? "Dynamic" : "Static";
}

/** Thin stacked bar of task phases, for job progress at a glance. */
export function PhaseBar({ counts, total, className = "" }: { counts: Record<TaskPhase, number>; total: number; className?: string }) {
  const order: TaskPhase[] = ["succeeded", "nonZero", "failed", "running", "queued", "stopped"];
  const color: Record<TaskPhase, string> = {
    succeeded: "bg-ok",
    nonZero: "bg-warn",
    failed: "bg-err",
    running: "bg-accent",
    queued: "bg-accent/35",
    stopped: "bg-faint/50",
  };
  return (
    <div className={`flex h-1 gap-px overflow-hidden rounded-full bg-surface-3 ${className}`}>
      {order.map((p) => counts[p] > 0 && <span key={p} className={color[p]} style={{ width: `${(counts[p] / total) * 100}%` }} />)}
    </div>
  );
}
