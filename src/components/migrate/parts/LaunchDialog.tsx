import { useState } from "react";
import type { LaunchResult } from "../../../lib/migrate/bindings";
import { jobsFromLaunch } from "../../../lib/migrate/activity";
import { jobStore } from "../../../lib/jobs";
import { useLaunchActions, useWavePreview } from "../../../lib/migrate/queries";
import { Button, ErrorNote, Modal } from "../../ui";
import { NameList } from "../move/NameList";

const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

/** Destinations listed by name before the rest are counted. */
const TOP_DESTINATIONS = 3;
/** Dry-run requests drawn before the rest are counted. */
const REQUESTS_SHOWN = 50;

function Result({ result }: { result: LaunchResult }) {
  const r = result.report;
  const lines = [
    r.launchedEcoverse > 0 && `${plural(r.launchedEcoverse, "device")} launched through Ecoverse`,
    r.launchedRest > 0 && `${plural(r.launchedRest, "device")} launched through N-central${r.fellBack > 0 ? ` (${r.fellBack.toLocaleString()} of them because Ecoverse couldn't reach them)` : ""}`,
    r.heldBack > 0 && `${plural(r.heldBack, "request")} held back: nothing was sent`,
  ].filter(Boolean) as string[];
  return (
    <div className="space-y-3 text-sm">
      {result.wasDryRun && <p className="rounded-xl border border-warn/30 bg-warn-soft px-3 py-2 text-warn">Dry run: nothing was sent and no device changed.</p>}
      {lines.map((l) => (
        <p key={l} className="text-ok">
          {l}
        </p>
      ))}
      {r.stopped && <p className="text-err">The launch stopped part-way: {r.stopped} The devices it didn't reach are still waiting in the wave.</p>}
      {r.failed.length > 0 && (
        <div className="space-y-1">
          <p className="text-err">{plural(r.failed.length, "device")} didn't launch</p>
          <NameList className="text-muted" items={r.failed.map(([name, why]) => ({ key: name, text: `${name}: ${why}` }))} />
        </div>
      )}
      {result.dryRun.length > 0 && (
        <div>
          <p className="font-medium">What would have been sent</p>
          <ul className="mt-1 max-h-48 space-y-2 overflow-y-auto">
            {result.dryRun.slice(0, REQUESTS_SHOWN).map((e, i) => (
              <li key={i} className="rounded-lg border border-border bg-bg p-2">
                <div className="text-xs">{e.summary}</div>
                <pre className="mt-1 max-h-24 overflow-auto text-[11px] whitespace-pre-wrap text-faint">{JSON.stringify(e.variables, null, 2)}</pre>
              </li>
            ))}
          </ul>
          {result.dryRun.length > REQUESTS_SHOWN && <p className="mt-1 text-xs text-faint">and {(result.dryRun.length - REQUESTS_SHOWN).toLocaleString()} more requests like these</p>}
        </div>
      )}
    </div>
  );
}

/** Says what launching a wave will do, and launches it (or shows the requests without sending them). */
export function LaunchDialog({ projectId, wave, onClose, onLaunched }: { projectId: string; wave: number; onClose: () => void; onLaunched: () => void }) {
  const preview = useWavePreview(projectId, wave);
  const { launch } = useLaunchActions(projectId);
  const [dryRun, setDryRun] = useState(false);
  const [result, setResult] = useState<LaunchResult | null>(null);
  const [allDestinations, setAllDestinations] = useState(false);
  const [showProblems, setShowProblems] = useState(false);
  const p = preview.data;
  const total = p ? p.ecoverse + p.rest : 0;
  const blocked = !p || p.stoppers.length > 0 || total === 0;

  if (result) {
    return (
      <Modal
        title={result.wasDryRun ? "Dry run" : "Launched"}
        onClose={() => (result.wasDryRun ? setResult(null) : onLaunched())}
        footer={
          result.wasDryRun ? (
            <>
              <Button onClick={onClose}>Close</Button>
              <Button variant="primary" onClick={() => setResult(null)}>
                Back
              </Button>
            </>
          ) : (
            <Button variant="primary" onClick={onLaunched}>
              Track this wave
            </Button>
          )
        }
      >
        <Result result={result} />
      </Modal>
    );
  }

  const destinations = p ? [...p.byDestination].sort((a, b) => b.devices - a.devices) : [];
  const shownDestinations = allDestinations ? destinations : destinations.slice(0, TOP_DESTINATIONS);

  return (
    <Modal
      title={p ? `Launch ${p.label}` : "Launch"}
      size="lg"
      description={
        p ? (
          p.reinstall ? (
            <>
              This reinstalls the agent on <span className="font-semibold text-text">{plural(total, "device")}</span>. They go offline for a while.
            </>
          ) : (
            <>
              This removes the old agent from <span className="font-semibold text-text">{plural(total, "device")}</span> and installs the new one from {p.destinationHost}. They go offline for a while.
            </>
          )
        ) : undefined
      }
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            busy={launch.isPending}
            disabled={blocked}
            onClick={() =>
              launch.mutate(
                { wave, dryRun },
                {
                  onSuccess: (r) => {
                    if (!r.wasDryRun) jobsFromLaunch(r.report).forEach((job) => jobStore.add(job));
                    setResult(r);
                  },
                },
              )
            }
          >
            {dryRun ? "Show the requests" : p ? `Launch ${plural(total, "device")}` : "Launch"}
          </Button>
        </>
      }
    >
      {preview.isError ? (
        <ErrorNote>{String(preview.error)}</ErrorNote>
      ) : !p ? (
        <p className="text-sm text-muted">Checking the wave…</p>
      ) : (
        <div className="space-y-4 text-sm">
          {p.stoppers.map((s) => (
            <div key={s} className="flex gap-2.5 rounded-xl border border-err/30 bg-err-soft px-3 py-2.5">
              <span className="font-bold text-err">×</span>
              <span className="flex-1 break-words">{s}</span>
            </div>
          ))}

          <dl className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-2.5">
            <dt className="text-muted">By path</dt>
            <dd className="tabular-nums">
              Ecoverse {p.ecoverse.toLocaleString()} · N-central {p.rest.toLocaleString()}
            </dd>
            <dt className="text-muted">Goes to</dt>
            <dd className="min-w-0">
              {destinations.length === 0 ? (
                "–"
              ) : (
                <ul className="space-y-0.5">
                  {shownDestinations.map((d) => (
                    <li key={d.name} className="flex gap-3 whitespace-nowrap">
                      <span className="min-w-0 truncate" title={d.name}>
                        {d.name}
                      </span>
                      <span className="shrink-0 text-muted tabular-nums">{d.devices.toLocaleString()}</span>
                    </li>
                  ))}
                  {destinations.length > TOP_DESTINATIONS && (
                    <li>
                      <button type="button" onClick={() => setAllDestinations(!allDestinations)} className="text-xs font-medium text-accent hover:text-accent-hover">
                        {allDestinations ? "Show the top 3" : `+ ${(destinations.length - TOP_DESTINATIONS).toLocaleString()} more`}
                      </button>
                    </li>
                  )}
                </ul>
              )}
            </dd>
            <dt className="text-muted">Old agent</dt>
            <dd>
              {p.cleanFull ? "Full cleanup" : "Install over the top"} · review period {p.reviewDays} days
            </dd>
          </dl>

          {p.problems.length > 0 && (
            <div className="space-y-2 rounded-xl border border-warn/30 bg-warn-soft/40 px-3 py-2.5">
              <div className="flex flex-wrap items-center gap-3">
                <span className="font-bold text-warn">!</span>
                <span className="flex-1">{plural(p.problems.length, "device")} can't be launched as planned</span>
                <button type="button" onClick={() => setShowProblems(!showProblems)} className="text-xs font-medium text-accent hover:text-accent-hover" aria-expanded={showProblems}>
                  {showProblems ? "Hide" : "Show"}
                </button>
              </div>
              {showProblems && <NameList className="text-xs text-muted" items={p.problems.map((d) => ({ key: d.name, text: `${d.name}: ${d.why}` }))} />}
            </div>
          )}

          <label className="flex cursor-pointer items-start gap-2.5">
            <input type="checkbox" className="mt-1 accent-accent" checked={dryRun} onChange={(e) => setDryRun(e.target.checked)} />
            <span>
              Dry run: show the requests, change nothing
              <span className="block text-xs text-muted">Passwords in the requests are hidden.</span>
            </span>
          </label>
          {launch.isError && <ErrorNote>{String(launch.error)}</ErrorNote>}
        </div>
      )}
    </Modal>
  );
}
