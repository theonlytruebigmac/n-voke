import { useMemo, useState, type ReactNode } from "react";
import { Eye, Info, Search, WrapText } from "lucide-react";
import { formatDateTime, formatDuration, useNow } from "../lib/format";
import { maxWaitSeconds, taskOutcome, taskPhase, type JobTask, type TaskPhase } from "../lib/taskState";
import type { InputValue } from "../lib/types";
import { TaskStatus } from "./status";
import { Button, CopyButton, IconButton, Tooltip } from "./ui";

const PARSE_ERROR = /parsing the powershell output/i;

/** The status box takes the colour of how the run went. */
const PHASE_BOX: Record<TaskPhase, string> = {
  queued: "border-accent/30 bg-accent-soft",
  running: "border-accent/30 bg-accent-soft",
  succeeded: "border-ok/25 bg-ok-soft",
  nonZero: "border-warn/30 bg-warn-soft",
  failed: "border-err/30 bg-err-soft",
  stopped: "border-border bg-surface-2",
};

function outputValue(v: InputValue | null): string {
  if (!v) return "";
  const first = [v.stringValue, v.booleanValue, v.intValue, v.floatValue, v.dateTimeValue].find((x) => x !== null && x !== undefined);
  return first === undefined ? "" : String(first);
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-sm text-faint">{label}</dt>
      <dd className="min-w-0 truncate text-sm">{children}</dd>
    </>
  );
}

/** Splits `text` around case-insensitive matches of `term`, marking each match. */
function highlight(text: string, term: string): { parts: ReactNode[]; count: number } {
  if (!term) return { parts: [text], count: 0 };
  const lower = text.toLowerCase();
  const needle = term.toLowerCase();
  const parts: ReactNode[] = [];
  let count = 0;
  let from = 0;
  for (let at = lower.indexOf(needle); at !== -1; at = lower.indexOf(needle, at + needle.length)) {
    if (at > from) parts.push(text.slice(from, at));
    parts.push(
      <mark key={at} className="rounded-sm bg-warn/40 text-text">
        {text.slice(at, at + needle.length)}
      </mark>,
    );
    from = at + needle.length;
    count++;
  }
  parts.push(text.slice(from));
  return { parts, count };
}

/** A block of script output with Find, Wrap and Copy. */
export function OutputBlock({ title, text, tone }: { title: string; text: string; tone?: "err" }) {
  const [term, setTerm] = useState("");
  const [wrap, setWrap] = useState(true);
  const { parts, count } = useMemo(() => highlight(text, term.trim()), [text, term]);
  return (
    <section className={`overflow-hidden rounded-xl border ${tone === "err" ? "border-err/30" : "border-border-strong"}`}>
      <div className={`flex flex-wrap items-center gap-2 border-b px-3 py-2 ${tone === "err" ? "border-err/30 bg-err-soft" : "border-border-strong bg-surface-2"}`}>
        <h3 className={`flex-1 text-sm font-semibold ${tone === "err" ? "text-err" : ""}`}>{title}</h3>
        <label className="flex h-7 w-40 items-center gap-1.5 rounded-md border border-border-strong bg-bg px-2 text-faint focus-within:border-accent">
          <Search className="size-3 shrink-0" />
          <input
            value={term}
            onChange={(e) => setTerm(e.target.value)}
            placeholder="Find"
            aria-label={`Find in ${title.toLowerCase()}`}
            className="min-w-0 flex-1 bg-transparent text-xs text-text outline-none placeholder:text-faint"
          />
        </label>
        {term.trim() && <span className="text-xs text-faint tabular-nums">{count === 1 ? "1 match" : `${count} matches`}</span>}
        <IconButton label={wrap ? "Don't wrap lines" : "Wrap lines"} onClick={() => setWrap((w) => !w)} aria-pressed={wrap} className={wrap ? "text-text" : ""}>
          <WrapText />
        </IconButton>
        <CopyButton text={text} label={`Copy ${title.toLowerCase()}`} />
      </div>
      <pre
        className={`max-h-[50vh] overflow-auto p-3 font-mono text-xs leading-relaxed ${wrap ? "wrap-break-word whitespace-pre-wrap" : "whitespace-pre"} ${
          tone === "err" ? "text-err" : "text-muted"
        }`}
      >
        {parts}
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
      <div className="flex flex-wrap items-center justify-between gap-3">
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
        The script ran. N-able counts any non-zero exit code as failed.
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
      <div className={`space-y-1 rounded-xl border px-4 py-3 ${PHASE_BOX[taskPhase(task)]}`}>
        <div className="text-base font-semibold">
          <TaskStatus task={task} />
        </div>
        <StatusNote task={task} timeoutSeconds={timeoutSeconds} createdAt={createdAt} onResume={onResume} />
        {task.lastError && (
          <p className="text-xs wrap-break-word text-warn">
            {task.pollFailures > 0 ? `Status check failed (${task.pollFailures}/5): ` : ""}
            {task.lastError}
          </p>
        )}
      </div>

      <dl className="grid grid-cols-[96px_minmax(0,1fr)] items-center gap-x-3 gap-y-2">
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
        <section className="space-y-2">
          <h3 className="text-sm font-semibold">Output variables</h3>
          <dl className="grid grid-cols-[96px_minmax(0,1fr)] gap-x-3 gap-y-2">
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
