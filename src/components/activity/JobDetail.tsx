import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronLeft, ChevronRight, Eye, EyeOff, MoreHorizontal, RotateCcw, Search, Tag as TagIcon, Trash2, X } from "lucide-react";
import { assetIdFromTaskId } from "../../lib/api";
import { formatDateTime } from "../../lib/format";
import { isJobActive, jobStore, type Job } from "../../lib/jobs";
import { suggestTagName } from "../../lib/rules";
import { assetStub, type RunDraft } from "../../lib/targets";
import { countPhases, groupHeadline, groupResults, taskOutcome, taskPhase, type JobTask, type PhaseCounts, type ResultGroup, type TaskPhase } from "../../lib/taskState";
import { AssignTagDialog } from "../AssignTagDialog";
import { PHASE_TONE, PhaseBar, TaskStatus, taskStatus } from "../status";
import { TaskDetail } from "../TaskDetail";
import { Button, Card, Dot, IconButton, Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger, Segmented, SelectionBar, Sheet, Table } from "../ui";

/** Problems first, then work in progress, then passes. */
const ATTENTION_ORDER: TaskPhase[] = ["failed", "nonZero", "running", "queued", "stopped", "succeeded"];

function taskAsset(t: JobTask) {
  return assetStub(t.assetId ?? assetIdFromTaskId(t.taskId) ?? t.taskId, t.deviceName);
}

/** First meaningful output line, for the table's Output column. */
function headline(t: JobTask): string {
  const out = t.execution?.output;
  const text = out?.stdOutput || out?.stdError || t.execution?.errorMessage || "";
  return text.split(/\r?\n/).find((l) => l.trim() && !/^[-=_*\s]+$/.test(l))?.trim() ?? "";
}

const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

/** Big "N of M finished", the bar, and a legend of only the outcomes that occurred. */
function JobSummary({ counts, total, active }: { counts: PhaseCounts; total: number; active: boolean }) {
  const finished = counts.succeeded + counts.nonZero + counts.failed;
  const left = counts.running + counts.queued;
  const items: { phase: TaskPhase; n: number; label: string }[] = [
    { phase: "succeeded", n: counts.succeeded, label: "passed" },
    { phase: "nonZero", n: counts.nonZero, label: "non-zero exit" },
    { phase: "failed", n: counts.failed, label: "didn't run" },
    { phase: "running", n: counts.running, label: "running" },
    { phase: "queued", n: counts.queued, label: "waiting" },
    { phase: "stopped", n: counts.stopped, label: "not watching" },
  ];
  return (
    <div className="space-y-2.5">
      <div className="flex flex-wrap items-baseline gap-x-2.5 gap-y-1">
        <span className="text-2xl font-bold tabular-nums">{finished.toLocaleString()}</span>
        <span className="text-muted">of {total.toLocaleString()} finished</span>
        <span className="flex-1" />
        {active && left > 0 && (
          <span className="inline-flex items-center gap-1.5 text-sm text-accent">
            <Dot tone="accent" pulse />
            Watching · {left.toLocaleString()} left
          </span>
        )}
      </div>
      <PhaseBar counts={counts} total={total} className="h-2.5" />
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm">
        {items
          .filter((i) => i.n > 0)
          .map((i) => (
            <span key={i.phase} className="inline-flex items-center gap-1.5 whitespace-nowrap">
              <Dot tone={PHASE_TONE[i.phase]} pulse={i.phase === "running"} />
              <span className="font-semibold tabular-nums">{i.n.toLocaleString()}</span>
              <span className="text-muted">{i.label}</span>
            </span>
          ))}
      </div>
    </div>
  );
}

/** "Exit 1 · TPM not present": what a group of identical results looks like, in a few words. */
function groupLabel(g: ResultGroup) {
  const status = taskStatus(g.tasks[0]).label;
  return taskOutcome(g.tasks[0]) ? `${status} · ${groupHeadline(g)}` : status;
}

export function JobDetail({ job, onDraft }: { job: Job; onDraft: (draft: Omit<RunDraft, "nonce">) => void }) {
  const counts = countPhases(job.tasks);
  const [openTaskId, setOpenTaskId] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [tagging, setTagging] = useState<JobTask[] | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [view, setView] = useState<"devices" | "outcomes">("outcomes");
  const [filterKey, setFilterKey] = useState<string | null>(null);
  const [term, setTerm] = useState("");
  const active = isJobActive(job);
  const org = job.orgId ? { id: job.orgId, name: job.orgName ?? "Organization", __typename: "" } : null;
  const single = job.tasks.length === 1 ? job.tasks[0] : null;
  const notPassed = job.tasks.filter((t) => ["nonZero", "failed"].includes(taskPhase(t)));
  const groups = useMemo(() => groupResults(job.tasks), [job.tasks]);
  const groupOf = useMemo(() => new Map(groups.flatMap((g) => g.tasks.map((t) => [t.taskId, g.key] as const))), [groups]);
  const filterGroup = groups.find((g) => g.key === filterKey) ?? null;

  useEffect(() => {
    if (!notice) return;
    const id = setTimeout(() => setNotice(null), 5000);
    return () => clearTimeout(id);
  }, [notice]);

  const rows = useMemo(() => {
    const t = term.trim().toLowerCase();
    return [...job.tasks]
      .filter((task) => !filterKey || groupOf.get(task.taskId) === filterKey)
      .filter((task) => !t || task.deviceName.toLowerCase().includes(t))
      .sort(
        (a, b) =>
          ATTENTION_ORDER.indexOf(taskPhase(a)) - ATTENTION_ORDER.indexOf(taskPhase(b)) ||
          headline(a).localeCompare(headline(b)) ||
          a.deviceName.localeCompare(b.deviceName),
      );
  }, [job.tasks, filterKey, groupOf, term]);

  const runAgain = (tasks: JobTask[]) =>
    onDraft({ org, script: job.script ?? null, targets: tasks.map((t) => ({ kind: "device" as const, asset: taskAsset(t) })) });
  const checkedTasks = job.tasks.filter((t) => checked.has(t.taskId));
  const openIndex = rows.findIndex((t) => t.taskId === openTaskId);
  // A device opened from a list that has since been filtered still shows; it just has no neighbours to step to.
  const openTask = job.tasks.find((t) => t.taskId === openTaskId) ?? null;
  const toggle = (id: string) =>
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const showGroup = (key: string) => {
    setFilterKey(key);
    setTerm("");
    setView("devices");
  };

  return (
    <Card className="space-y-5 p-5">
      <header className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-[1_1_300px]">
          <h2 className="truncate text-lg font-semibold" title={job.scriptName}>
            {job.scriptName}
          </h2>
          <p className="truncate text-sm text-muted">
            {job.targetLabel}
            {job.orgName && ` · ${job.orgName}`} · {formatDateTime(job.createdAt)}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {org && (
            <Menu>
              <MenuTrigger asChild>
                <Button>
                  <RotateCcw />
                  Run again
                  <ChevronDown />
                </Button>
              </MenuTrigger>
              <MenuContent>
                <MenuItem onSelect={() => runAgain(job.tasks)}>{single ? "Same device" : `All ${plural(job.tasks.length, "device")}`}</MenuItem>
                {!single && (
                  <MenuItem onSelect={() => runAgain(notPassed)} disabled={!notPassed.length}>
                    {notPassed.length.toLocaleString()} that didn't pass
                  </MenuItem>
                )}
              </MenuContent>
            </Menu>
          )}
          <Menu>
            <MenuTrigger asChild>
              <IconButton label="More" className="border border-border-strong">
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
          <JobSummary counts={counts} total={job.tasks.length} active={active} />

          <div className="flex flex-wrap items-center gap-3">
            <Segmented
              value={view}
              onChange={setView}
              options={[
                { value: "outcomes", label: `Outcomes · ${groups.length}` },
                { value: "devices", label: `Devices · ${job.tasks.length.toLocaleString()}` },
              ]}
            />
            {view === "outcomes" ? (
              <span className="text-xs text-faint">Devices with the same result are grouped</span>
            ) : (
              <>
                {filterGroup && (
                  <span className="inline-flex h-7 max-w-full items-center gap-1 rounded-full border border-border-strong bg-surface-2 pr-1 pl-3 text-xs whitespace-nowrap">
                    <span className="truncate" title={groupLabel(filterGroup)}>
                      {groupLabel(filterGroup)}
                    </span>
                    <button type="button" onClick={() => setFilterKey(null)} aria-label="Show every outcome" className="rounded-full p-0.5 text-faint hover:text-text">
                      <X className="size-3.5" />
                    </button>
                  </span>
                )}
                <label className="flex h-8 max-w-72 min-w-40 flex-1 items-center gap-2 rounded-lg border border-border-strong bg-bg px-2.5 text-faint focus-within:border-accent">
                  <Search className="size-3.5 shrink-0" />
                  <input
                    value={term}
                    onChange={(e) => setTerm(e.target.value)}
                    placeholder={`Search ${plural(filterGroup ? filterGroup.tasks.length : job.tasks.length, "device")}`}
                    aria-label="Search devices"
                    className="min-w-0 flex-1 bg-transparent text-sm text-text outline-none placeholder:text-faint"
                  />
                </label>
              </>
            )}
          </div>

          {view === "devices" && checked.size > 0 && (
            <SelectionBar count={checked.size} onClear={() => setChecked(new Set())}>
              {org && (
                <Button size="sm" onClick={() => setTagging(checkedTasks)}>
                  <TagIcon />
                  Tag…
                </Button>
              )}
              {org && (
                <Button size="sm" onClick={() => runAgain(checkedTasks)}>
                  <RotateCcw />
                  Run a script…
                </Button>
              )}
            </SelectionBar>
          )}

          {view === "outcomes" ? (
            <Table
              head={
                <>
                  <th className="w-20 text-right">Devices</th>
                  <th className="w-36">Result</th>
                  <th>Output</th>
                  <th className="w-0">
                    <span className="sr-only">Actions</span>
                  </th>
                </>
              }
            >
              {groups.map((g) => {
                const out = taskOutcome(g.tasks[0]) ? groupHeadline(g) : "";
                return (
                  <tr key={g.key} onClick={() => showGroup(g.key)} className="cursor-default hover:bg-surface-2/60">
                    <td className="text-right font-semibold tabular-nums">{g.tasks.length.toLocaleString()}</td>
                    <td className="whitespace-nowrap">
                      <TaskStatus task={g.tasks[0]} />
                    </td>
                    <td className="max-w-0 truncate font-mono text-xs text-muted" title={out}>
                      {out}
                    </td>
                    <td className="whitespace-nowrap" onClick={(e) => e.stopPropagation()}>
                      <div className="flex justify-end gap-1.5">
                        {org && (
                          <Button size="sm" onClick={() => setTagging(g.tasks)}>
                            Tag these
                          </Button>
                        )}
                        {org && (
                          <Button size="sm" onClick={() => runAgain(g.tasks)}>
                            Run a script on these
                          </Button>
                        )}
                        <Button size="sm" onClick={() => showGroup(g.key)}>
                          Show devices
                        </Button>
                      </div>
                    </td>
                  </tr>
                );
              })}
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
                <tr
                  key={t.taskId}
                  onClick={() => setOpenTaskId(t.taskId)}
                  className={`cursor-default hover:bg-surface-2/60 ${t.taskId === openTaskId ? "bg-surface-3" : ""}`}
                >
                  <td onClick={(e) => e.stopPropagation()}>
                    <input
                      type="checkbox"
                      className="size-3.5 accent-accent"
                      checked={checked.has(t.taskId)}
                      onChange={() => toggle(t.taskId)}
                      aria-label={`Select ${t.deviceName}`}
                    />
                  </td>
                  <td className="max-w-0 truncate font-medium" title={t.deviceName}>
                    {t.deviceName}
                  </td>
                  <td className="whitespace-nowrap">
                    <TaskStatus task={t} />
                  </td>
                  <td className="text-right whitespace-nowrap text-muted tabular-nums">
                    {t.execution?.durationMilliseconds != null ? `${(t.execution.durationMilliseconds / 1000).toFixed(1)}s` : ""}
                  </td>
                  <td className="max-w-0 truncate font-mono text-xs text-muted" title={headline(t)}>
                    {headline(t)}
                  </td>
                </tr>
              ))}
              {!rows.length && (
                <tr>
                  <td colSpan={5} className="py-6 text-center text-faint">
                    No matching devices
                  </td>
                </tr>
              )}
            </Table>
          )}
        </>
      )}

      {openTask && (
        <Sheet title={openTask.deviceName} subtitle={job.scriptName} onClose={() => setOpenTaskId(null)}>
          {openIndex >= 0 && rows.length > 1 && (
            <div className="-mt-1 mb-4 flex items-center justify-end gap-1.5">
              <span className="mr-1 text-xs text-faint tabular-nums">
                {(openIndex + 1).toLocaleString()} of {rows.length.toLocaleString()}
              </span>
              <IconButton label="Previous device" disabled={openIndex <= 0} onClick={() => setOpenTaskId(rows[openIndex - 1].taskId)} className="border border-border-strong">
                <ChevronLeft />
              </IconButton>
              <IconButton
                label="Next device"
                disabled={openIndex >= rows.length - 1}
                onClick={() => setOpenTaskId(rows[openIndex + 1].taskId)}
                className="border border-border-strong"
              >
                <ChevronRight />
              </IconButton>
            </div>
          )}
          <TaskDetail
            key={openTask.taskId}
            task={openTask}
            timeoutSeconds={job.timeoutSeconds}
            createdAt={job.createdAt}
            onResume={() => jobStore.resume(job.id, openTask.taskId)}
          />
        </Sheet>
      )}
      {tagging && org && (
        <AssignTagDialog
          org={org}
          devices={tagging.map(taskAsset)}
          suggestedName={suggestTagName(`${job.scriptName}_selected`)}
          onClose={() => setTagging(null)}
          onDone={(tag) => {
            setNotice(`Tagged ${plural(tagging.length, "device")} with ${tag.name}`);
            setTagging(null);
            setChecked(new Set());
          }}
        />
      )}
    </Card>
  );
}
