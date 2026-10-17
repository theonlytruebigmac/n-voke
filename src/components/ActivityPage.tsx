import { useEffect, useMemo, useState } from "react";
import { Eye, EyeOff, ListChecks, MoreHorizontal, Plus, RotateCcw, Tag as TagIcon, Trash2, X } from "lucide-react";
import { assetIdFromTaskId } from "../lib/api";
import { formatDateTime, formatRelative, useNow } from "../lib/format";
import { isJobActive, jobStore, type Job } from "../lib/jobs";
import { suggestTagName } from "../lib/rules";
import { assetStub, type RunDraft } from "../lib/targets";
import { countPhases, groupHeadline, groupResults, taskOutcome, taskPhase, type JobTask, type PhaseCounts, type TaskPhase } from "../lib/taskState";
import { AssignTagDialog } from "./AssignTagDialog";
import { PHASE_TONE, PhaseBar, TaskStatus } from "./status";
import { TaskDetail } from "./TaskDetail";
import { Button, Dot, EmptyState, IconButton, Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger, PageHeader, Segmented, Sheet, Status, Table, type Tone } from "./ui";
import { WatchTaskDialog } from "./WatchTaskDialog";

/** Problems first, then work in progress, then passes. */
const ATTENTION_ORDER: TaskPhase[] = ["failed", "nonZero", "running", "queued", "stopped", "succeeded"];

function taskAsset(t: JobTask) {
  return assetStub(t.assetId ?? assetIdFromTaskId(t.taskId) ?? t.taskId, t.deviceName);
}

/** One word for a whole job. */
function jobState(job: Job, counts: PhaseCounts): { tone: Tone; label: string; pulse?: boolean } {
  if (isJobActive(job)) return { tone: "accent", label: "In progress", pulse: true };
  if (counts.failed) return { tone: "err", label: `${counts.failed} didn't run` };
  if (counts.nonZero) return { tone: "warn", label: `${counts.nonZero} non-zero` };
  if (counts.stopped) return { tone: "muted", label: "Not watching" };
  return { tone: "ok", label: "Passed" };
}

/** First meaningful output line, for the table's Output column. */
function headline(t: JobTask): string {
  const out = t.execution?.output;
  const text = out?.stdOutput || out?.stdError || t.execution?.errorMessage || "";
  return text.split(/\r?\n/).find((l) => l.trim() && !/^[-=_*\s]+$/.test(l))?.trim() ?? "";
}

export function ActivityPage({
  jobs,
  selectedId,
  onSelect,
  onDraft,
  ready,
}: {
  jobs: Job[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onDraft: (draft: Omit<RunDraft, "nonce">) => void;
  ready: boolean;
}) {
  const now = useNow(30_000);
  const [watching, setWatching] = useState(false);
  const selected = jobs.find((j) => j.id === selectedId) ?? jobs[0] ?? null;

  return (
    <>
      <PageHeader
        title="Activity"
        actions={
          <Button size="sm" onClick={() => setWatching(true)} disabled={!ready}>
            <Plus />
            Watch task ID
          </Button>
        }
      />
      {!jobs.length ? (
        <EmptyState icon={<ListChecks />} title="No runs yet">
          Runs you start show up here and update until every device finishes.
        </EmptyState>
      ) : (
        <div className="flex min-h-0 flex-1">
          <ul className="w-72 shrink-0 overflow-y-auto border-r border-border p-2">
            {jobs.map((job) => {
              const counts = countPhases(job.tasks);
              const state = jobState(job, counts);
              return (
                <li key={job.id}>
                  <button
                    onClick={() => onSelect(job.id)}
                    className={`block w-full rounded-md px-2.5 py-2 text-left transition-colors ${job.id === selected?.id ? "bg-surface-3" : "hover:bg-surface-2"}`}
                  >
                    <div className="flex items-baseline justify-between gap-2">
                      <span className="truncate text-sm font-medium">{job.scriptName}</span>
                      <span className="shrink-0 text-xs text-faint">{formatRelative(job.createdAt, now)}</span>
                    </div>
                    <div className="mt-0.5 flex items-center justify-between gap-2 text-xs text-muted">
                      <span className="truncate">{job.targetLabel}</span>
                      <span className="flex shrink-0 items-center gap-1.5">
                        <Status tone={state.tone} label={<span className="text-xs">{state.label}</span>} pulse={state.pulse} />
                      </span>
                    </div>
                    <PhaseBar counts={counts} total={job.tasks.length} className="mt-2" />
                  </button>
                </li>
              );
            })}
          </ul>
          <div className="min-w-0 flex-1 overflow-y-auto">{selected && <JobDetail key={selected.id} job={selected} onDraft={onDraft} />}</div>
        </div>
      )}
      {watching && <WatchTaskDialog onClose={() => setWatching(false)} onAdded={onSelect} />}
    </>
  );
}

/** Bar plus a legend of only the outcomes that occurred, e.g. "● 1 non-zero exit  ● 1 waiting". */
function JobSummary({ counts, total }: { counts: PhaseCounts; total: number }) {
  const items: { phase: TaskPhase; n: number; label: string }[] = [
    { phase: "succeeded", n: counts.succeeded, label: "passed" },
    { phase: "nonZero", n: counts.nonZero, label: "non-zero exit" },
    { phase: "failed", n: counts.failed, label: "didn't run" },
    { phase: "running", n: counts.running, label: "running" },
    { phase: "queued", n: counts.queued, label: "waiting" },
    { phase: "stopped", n: counts.stopped, label: "not watching" },
  ];
  return (
    <div className="min-w-0 flex-1 space-y-2">
      <PhaseBar counts={counts} total={total} className="h-1.5" />
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm">
        {items
          .filter((i) => i.n > 0)
          .map((i) => (
            <span key={i.phase} className="inline-flex items-center gap-1.5">
              <Dot tone={PHASE_TONE[i.phase]} pulse={i.phase === "running"} />
              <span className="font-medium tabular-nums">{i.n}</span>
              <span className="text-muted">{i.label}</span>
            </span>
          ))}
      </div>
    </div>
  );
}

function JobDetail({ job, onDraft }: { job: Job; onDraft: (draft: Omit<RunDraft, "nonce">) => void }) {
  const counts = countPhases(job.tasks);
  const [openTaskId, setOpenTaskId] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [tagging, setTagging] = useState<JobTask[] | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [view, setView] = useState<"devices" | "outcomes">("devices");
  const [filterKey, setFilterKey] = useState<string | null>(null);
  const active = isJobActive(job);
  const org = job.orgId ? { id: job.orgId, name: job.orgName ?? "Organization", __typename: "" } : null;
  const single = job.tasks.length === 1 ? job.tasks[0] : null;
  const notPassed = job.tasks.filter((t) => ["nonZero", "failed"].includes(taskPhase(t)));
  const groups = useMemo(() => groupResults(job.tasks), [job.tasks]);
  const groupOf = useMemo(() => new Map(groups.flatMap((g) => g.tasks.map((t) => [t.taskId, g.key] as const))), [groups]);

  useEffect(() => {
    if (!notice) return;
    const id = setTimeout(() => setNotice(null), 5000);
    return () => clearTimeout(id);
  }, [notice]);

  const rows = useMemo(
    () =>
      [...job.tasks]
        .filter((t) => !filterKey || groupOf.get(t.taskId) === filterKey)
        .sort(
          (a, b) =>
            ATTENTION_ORDER.indexOf(taskPhase(a)) - ATTENTION_ORDER.indexOf(taskPhase(b)) ||
            headline(a).localeCompare(headline(b)) ||
            a.deviceName.localeCompare(b.deviceName),
        ),
    [job.tasks, filterKey, groupOf],
  );

  const runAgain = (tasks: JobTask[]) =>
    onDraft({ org, script: job.script ?? null, targets: tasks.map((t) => ({ kind: "device" as const, asset: taskAsset(t) })) });
  const checkedTasks = job.tasks.filter((t) => checked.has(t.taskId));
  const openTask = job.tasks.find((t) => t.taskId === openTaskId) ?? null;
  const toggle = (id: string) =>
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div className="mx-auto max-w-4xl space-y-5 px-6 py-5">
      <header className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="truncate text-lg font-semibold">{job.scriptName}</h2>
          <p className="truncate text-sm text-muted">
            {job.targetLabel}
            {job.orgName && ` · ${job.orgName}`} · {formatDateTime(job.createdAt)}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {org && (
            <Menu>
              <MenuTrigger asChild>
                <Button size="sm">
                  <RotateCcw />
                  Run again
                </Button>
              </MenuTrigger>
              <MenuContent>
                <MenuItem onSelect={() => runAgain(job.tasks)}>{single ? "Same device" : `All ${job.tasks.length} devices`}</MenuItem>
                {!single && (
                  <MenuItem onSelect={() => runAgain(notPassed)} disabled={!notPassed.length}>
                    {notPassed.length} that didn't pass
                  </MenuItem>
                )}
              </MenuContent>
            </Menu>
          )}
          <Menu>
            <MenuTrigger asChild>
              <IconButton label="More">
                <MoreHorizontal />
              </IconButton>
            </MenuTrigger>
            <MenuContent>
              {active ? (
                <MenuItem onSelect={() => jobStore.stop(job.id)}>
                  <EyeOff />
                  Stop watching
                </MenuItem>
              ) : (
                <MenuItem onSelect={() => jobStore.resume(job.id)} disabled={!counts.stopped}>
                  <Eye />
                  Resume watching
                </MenuItem>
              )}
              <MenuSeparator />
              <MenuItem danger onSelect={() => jobStore.remove(job.id)}>
                <Trash2 />
                Remove from list
              </MenuItem>
            </MenuContent>
          </Menu>
        </div>
      </header>

      {notice && (
        <p className="text-sm text-ok" role="status">
          {notice}
        </p>
      )}
      {job.warnings?.map((w) => (
        <p key={w} className="text-sm text-warn">
          {w}
        </p>
      ))}

      {single ? (
        <TaskDetail task={single} timeoutSeconds={job.timeoutSeconds} createdAt={job.createdAt} onResume={() => jobStore.resume(job.id, single.taskId)} />
      ) : (
        <>
          <div className="flex items-end justify-between gap-6">
            <JobSummary counts={counts} total={job.tasks.length} />
            <Segmented
              value={view}
              onChange={setView}
              options={[
                { value: "devices", label: "Devices" },
                { value: "outcomes", label: `Outcomes · ${groups.length}` },
              ]}
            />
          </div>

          <div className={`flex items-center justify-between gap-3 ${checked.size || filterKey ? "h-8" : "hidden"}`}>
            {checked.size > 0 ? (
              <div className="flex items-center gap-1.5">
                <span className="mr-1 text-sm">{checked.size} selected</span>
                {org && (
                  <Button size="sm" onClick={() => setTagging(checkedTasks)}>
                    <TagIcon />
                    Tag
                  </Button>
                )}
                {org && (
                  <Button size="sm" onClick={() => runAgain(checkedTasks)}>
                    <RotateCcw />
                    Run script
                  </Button>
                )}
                <IconButton label="Clear selection" onClick={() => setChecked(new Set())}>
                  <X />
                </IconButton>
              </div>
            ) : filterKey ? (
              <button onClick={() => setFilterKey(null)} className="inline-flex items-center gap-1 text-sm text-muted hover:text-text">
                Showing one outcome <X className="size-3.5" />
              </button>
            ) : null}
          </div>

          {view === "outcomes" ? (
            <Table
              head={
                <>
                  <th className="w-16 text-right">Devices</th>
                  <th className="w-40">Result</th>
                  <th>Output</th>
                </>
              }
            >
              {groups.map((g) => (
                <tr
                  key={g.key}
                  onClick={() => {
                    setFilterKey(g.key);
                    setView("devices");
                  }}
                  className="cursor-default hover:bg-surface-2/60"
                >
                  <td className="text-right font-medium tabular-nums">{g.tasks.length}</td>
                  <td>
                    <TaskStatus task={g.tasks[0]} />
                  </td>
                  <td className="max-w-0 truncate text-muted">{taskOutcome(g.tasks[0]) ? groupHeadline(g) : ""}</td>
                </tr>
              ))}
            </Table>
          ) : (
            <Table
              head={
                <>
                  <th className="w-9">
                    <input
                      type="checkbox"
                      className="size-3.5 accent-accent"
                      checked={rows.length > 0 && rows.every((t) => checked.has(t.taskId))}
                      onChange={(e) => setChecked(e.target.checked ? new Set(rows.map((t) => t.taskId)) : new Set())}
                      aria-label="Select all"
                    />
                  </th>
                  <th className="w-48">Device</th>
                  <th className="w-36">Result</th>
                  <th className="w-20 text-right">Time</th>
                  <th>Output</th>
                </>
              }
            >
              {rows.map((t) => (
                <tr key={t.taskId} onClick={() => setOpenTaskId(t.taskId)} className="cursor-default hover:bg-surface-2/60">
                  <td onClick={(e) => e.stopPropagation()}>
                    <input
                      type="checkbox"
                      className="size-3.5 accent-accent"
                      checked={checked.has(t.taskId)}
                      onChange={() => toggle(t.taskId)}
                      aria-label={`Select ${t.deviceName}`}
                    />
                  </td>
                  <td className="max-w-0 truncate font-medium">{t.deviceName}</td>
                  <td>
                    <TaskStatus task={t} />
                  </td>
                  <td className="text-right text-muted">
                    {t.execution?.durationMilliseconds != null ? `${(t.execution.durationMilliseconds / 1000).toFixed(1)}s` : ""}
                  </td>
                  <td className="max-w-0 truncate font-mono text-xs text-muted">{headline(t)}</td>
                </tr>
              ))}
            </Table>
          )}
        </>
      )}

      {openTask && (
        <Sheet title={openTask.deviceName} subtitle={job.scriptName} onClose={() => setOpenTaskId(null)}>
          <TaskDetail task={openTask} timeoutSeconds={job.timeoutSeconds} createdAt={job.createdAt} onResume={() => jobStore.resume(job.id, openTask.taskId)} />
        </Sheet>
      )}
      {tagging && org && (
        <AssignTagDialog
          org={org}
          devices={tagging.map(taskAsset)}
          suggestedName={suggestTagName(`${job.scriptName}_selected`)}
          onClose={() => setTagging(null)}
          onDone={(tag) => {
            setNotice(`Tagged ${tagging.length} device${tagging.length === 1 ? "" : "s"} with ${tag.name}`);
            setTagging(null);
            setChecked(new Set());
          }}
        />
      )}
    </div>
  );
}
