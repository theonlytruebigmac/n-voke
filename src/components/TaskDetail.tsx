import type { ReactNode } from "react";
import { Eye, Info } from "lucide-react";
import { formatDateTime, formatDuration, useNow } from "../lib/format";
import { maxWaitSeconds, taskOutcome, taskPhase, type JobTask } from "../lib/taskState";
import type { InputValue } from "../lib/types";
import { TaskStatus } from "./status";
import { Button, CopyButton, Tooltip } from "./ui";

const PARSE_ERROR = /parsing the powershell output/i;

function outputValue(v: InputValue | null): string {
  if (!v) return "";
  const first = [v.stringValue, v.booleanValue, v.intValue, v.floatValue, v.dateTimeValue].find((x) => x !== null && x !== undefined);
  return first === undefined ? "" : String(first);
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[96px_minmax(0,1fr)] gap-3 py-1.5">
      <dt className="text-sm text-muted">{label}</dt>
      <dd className="min-w-0 truncate text-sm">{children}</dd>
    </div>
  );
}

export function OutputBlock({ title, text, tone }: { title: string; text: string; tone?: "err" }) {
  return (
    <section>
      <div className="mb-1.5 flex h-7 items-center justify-between">
        <h3 className={`text-sm font-semibold ${tone === "err" ? "text-err" : ""}`}>{title}</h3>
        <CopyButton text={text} label={`Copy ${title.toLowerCase()}`} />
      </div>
      <pre
        className={`max-h-[50vh] overflow-auto rounded-lg p-3 font-mono text-xs leading-relaxed break-words whitespace-pre-wrap ${
          tone === "err" ? "bg-err-soft text-err" : "bg-surface-2"
        }`}
      >
        {text}
      </pre>
    </section>
  );
}

/** One line under the status explaining what it means, only when it needs explaining. */
function StatusNote({ task, timeoutSeconds, createdAt, onResume }: { task: JobTask; timeoutSeconds: number; createdAt: string; onResume: () => void }) {
  const now = useNow(1000);
  const phase = taskPhase(task);
  const outcome = taskOutcome(task);
  const elapsed = formatDuration((now - new Date(createdAt).getTime()) / 1000);
  const message = task.execution?.errorMessage?.trim();

  if (phase === "queued") return <p className="text-sm text-muted">Waiting for the device to pick it up · {elapsed}</p>;
  if (phase === "running") return <p className="text-sm text-muted">Running · {elapsed}</p>;
  if (phase === "stopped")
    return (
      <div className="flex items-center justify-between gap-3">
        <p className="text-sm text-muted">
          {task.stopReason === "timeout"
            ? `No result after ${formatDuration(maxWaitSeconds(timeoutSeconds))}`
            : task.stopReason === "errors"
              ? "Stopped after repeated errors"
              : "Not watching"}
        </p>
        <Button size="sm" onClick={onResume}>
          <Eye />
          Resume
        </Button>
      </div>
    );
  if (phase === "nonZero")
    return (
      <p className="flex items-center gap-1.5 text-sm text-muted">
        The script ran. N-able counts a non-zero exit code as failed.
        {message && (
          <Tooltip content={PARSE_ERROR.test(message) ? `N-able reports “${message}” when a PowerShell script exits non-zero.` : `N-able: ${message}`}>
            <Info className="size-3.5 shrink-0 text-faint" />
          </Tooltip>
        )}
      </p>
    );
  if (phase === "failed" && outcome?.kind === "cancelled") return <p className="text-sm text-muted">The task was cancelled in N-able.</p>;
  if (phase === "failed" && outcome?.kind === "timedOut") return <p className="text-sm text-muted">Stopped after {formatDuration(timeoutSeconds || 3600)}.</p>;
  if (phase === "failed" && message) return <p className="text-sm text-err">{message}</p>;
  return null;
}

/** Everything about one device's run. */
export function TaskDetail({ task, timeoutSeconds, createdAt, onResume }: { task: JobTask; timeoutSeconds: number; createdAt: string; onResume: () => void }) {
  const outcome = taskOutcome(task);
  const exec = task.execution;
  const output = exec?.output;
  const outputs = output?.outputs?.filter(Boolean) ?? [];
  const exitCode = outcome?.exitCode ?? (outcome?.kind === "nonZeroExit" ? "Non-zero" : null);

  return (
    <div className="space-y-5">
      <div className="space-y-1.5">
        <div className="text-base">
          <TaskStatus task={task} />
        </div>
        <StatusNote task={task} timeoutSeconds={timeoutSeconds} createdAt={createdAt} onResume={onResume} />
        {task.lastError && (
          <p className="text-xs break-words text-warn">
            {task.pollFailures > 0 ? `Status check failed (${task.pollFailures}/5): ` : ""}
            {task.lastError}
          </p>
        )}
      </div>

      <dl className="divide-y divide-border border-y border-border">
        {exitCode !== null && <Row label="Exit code">{exitCode}</Row>}
        {exec?.durationMilliseconds != null && <Row label="Duration">{(exec.durationMilliseconds / 1000).toFixed(1)}s</Row>}
        {exec?.startedAt && <Row label="Started">{formatDateTime(exec.startedAt)}</Row>}
        {output?.runAs && <Row label="Run as">{output.runAs}</Row>}
        <Row label="Task ID">
          <span className="flex items-center gap-1">
            <code className="truncate font-mono text-xs text-muted">{task.taskId}</code>
            <CopyButton text={task.taskId} label="Copy task ID" />
          </span>
        </Row>
      </dl>

      {outputs.length > 0 && (
        <section>
          <h3 className="mb-1.5 text-sm font-semibold">Output variables</h3>
          <dl className="divide-y divide-border border-y border-border">
            {outputs.map((o) => (
              <Row key={o.variable} label={o.name || o.variable}>
                <span className="font-mono text-xs">{outputValue(o.value)}</span>
              </Row>
            ))}
          </dl>
        </section>
      )}

      {output?.stdOutput && <OutputBlock title="Output" text={output.stdOutput} />}
      {output?.stdError && <OutputBlock title="Errors" text={output.stdError} tone="err" />}
      {outcome?.ran && !outcome.hasOutput && <p className="text-sm text-faint">No output.</p>}
    </div>
  );
}
