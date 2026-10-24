import { useMemo, useState } from "react";
import { ListChecks, Plus, Search } from "lucide-react";
import { formatRelative, useNow } from "../lib/format";
import { isJobActive, type Job } from "../lib/jobs";
import type { RunDraft } from "../lib/targets";
import { countPhases, type PhaseCounts } from "../lib/taskState";
import { JobDetail } from "./activity/JobDetail";
import { PhaseBar } from "./status";
import { Button, Dot, EmptyState, PageBody, PageHeader, Segmented, type Tone } from "./ui";
import { WatchTaskDialog } from "./WatchTaskDialog";

type Filter = "all" | "running" | "problems";

/** One word for a whole job. */
function jobState(job: Job, counts: PhaseCounts): { tone: Tone; label: string; pulse?: boolean } {
  if (isJobActive(job)) return { tone: "accent", label: "In progress", pulse: true };
  if (counts.failed) return { tone: "err", label: `${counts.failed.toLocaleString()} didn't run` };
  if (counts.nonZero) return { tone: "warn", label: `${counts.nonZero.toLocaleString()} non-zero` };
  if (counts.stopped) return { tone: "muted", label: "Not watching" };
  return { tone: "ok", label: "Passed" };
}

const TONE_TEXT: Record<Tone, string> = { ok: "text-ok", err: "text-err", warn: "text-warn", accent: "text-accent", muted: "text-faint" };

const hasProblems = (counts: PhaseCounts) => counts.failed > 0 || counts.nonZero > 0;

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
  const [term, setTerm] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const selected = jobs.find((j) => j.id === selectedId) ?? jobs[0] ?? null;

  const withCounts = useMemo(() => jobs.map((job) => ({ job, counts: countPhases(job.tasks) })), [jobs]);
  const running = withCounts.filter((j) => isJobActive(j.job)).length;
  const problems = withCounts.filter((j) => hasProblems(j.counts)).length;

  const shown = useMemo(() => {
    const t = term.trim().toLowerCase();
    return withCounts.filter(({ job, counts }) => {
      if (filter === "running" && !isJobActive(job)) return false;
      if (filter === "problems" && !hasProblems(counts)) return false;
      if (!t) return true;
      return (
        job.scriptName.toLowerCase().includes(t) ||
        job.targetLabel.toLowerCase().includes(t) ||
        (job.orgName ?? "").toLowerCase().includes(t) ||
        job.tasks.some((task) => task.deviceName.toLowerCase().includes(t))
      );
    });
  }, [withCounts, term, filter]);

  return (
    <>
      <PageHeader
        title="Activity"
        actions={
          <Button onClick={() => setWatching(true)} disabled={!ready}>
            <Plus />
            Watch a task ID
          </Button>
        }
      />
      {!jobs.length ? (
        <EmptyState icon={<ListChecks />} title="No runs yet">
          Runs you start show up here and update until every device finishes.
        </EmptyState>
      ) : (
        <PageBody>
          <div className="flex flex-wrap items-start gap-5">
            <section aria-label="Runs" className="sticky top-0 flex max-h-[calc(100vh-11rem)] min-w-0 flex-[1_1_280px] flex-col gap-2.5">
              <label className="flex h-9 shrink-0 items-center gap-2 rounded-lg border border-border-strong bg-surface px-2.5 text-faint focus-within:border-accent">
                <Search className="size-3.5 shrink-0" />
                <input
                  value={term}
                  onChange={(e) => setTerm(e.target.value)}
                  placeholder="Search runs, scripts, devices"
                  aria-label="Search runs"
                  className="min-w-0 flex-1 bg-transparent text-sm text-text outline-none placeholder:text-faint"
                />
              </label>
              <div className="shrink-0">
                <Segmented<Filter>
                  value={filter}
                  onChange={setFilter}
                  options={[
                    { value: "all", label: "All" },
                    { value: "running", label: `Running · ${running}` },
                    { value: "problems", label: `Problems · ${problems}` },
                  ]}
                />
              </div>
              <ul className="-mx-1 min-h-0 space-y-1 overflow-y-auto px-1">
                {shown.map(({ job, counts }) => {
                  const state = jobState(job, counts);
                  const current = job.id === selected?.id;
                  return (
                    <li key={job.id}>
                      <button
                        onClick={() => onSelect(job.id)}
                        aria-current={current ? "true" : undefined}
                        className={`block w-full rounded-xl border px-3 py-2.5 text-left transition-colors ${current ? "border-border-strong bg-surface-3" : "border-transparent hover:bg-surface-2"}`}
                      >
                        <div className="flex items-baseline gap-2 whitespace-nowrap">
                          <span className="min-w-0 flex-1 truncate font-medium" title={job.scriptName}>
                            {job.scriptName}
                          </span>
                          <span className="shrink-0 text-xs text-faint">{formatRelative(job.createdAt, now)}</span>
                        </div>
                        <div className="mt-0.5 flex items-center gap-2 text-xs whitespace-nowrap">
                          <span className="min-w-0 flex-1 truncate text-muted" title={job.targetLabel}>
                            {job.targetLabel}
                          </span>
                          <span className={`flex shrink-0 items-center gap-1.5 ${TONE_TEXT[state.tone]}`}>
                            <Dot tone={state.tone} pulse={state.pulse} />
                            {state.label}
                          </span>
                        </div>
                        <PhaseBar counts={counts} total={job.tasks.length} className="mt-2" />
                      </button>
                    </li>
                  );
                })}
                {!shown.length && <li className="px-3 py-6 text-center text-sm text-faint">No matching runs</li>}
              </ul>
            </section>

            <section aria-label="Run detail" className="min-w-0 flex-[999_1_560px]">
              {selected && <JobDetail key={selected.id} job={selected} onDraft={onDraft} />}
            </section>
          </div>
        </PageBody>
      )}
      {watching && <WatchTaskDialog onClose={() => setWatching(false)} onAdded={onSelect} />}
    </>
  );
}
