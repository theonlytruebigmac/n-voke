import { useState } from "react";
import type { LaunchResult } from "../../../lib/migrate/bindings";
import { useLaunchActions, useWavePreview } from "../../../lib/migrate/queries";
import { Button, ErrorNote, Modal } from "../../ui";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

function Result({ result }: { result: LaunchResult }) {
  const r = result.report;
  const lines = [
    r.launchedEcoverse > 0 && `${plural(r.launchedEcoverse, "device")} launched through Ecoverse`,
    r.launchedRest > 0 && `${plural(r.launchedRest, "device")} launched through N-central${r.fellBack > 0 ? ` (${r.fellBack} of them because Ecoverse couldn't reach them)` : ""}`,
    r.heldBack > 0 && `${plural(r.heldBack, "request")} held back: nothing was sent`,
  ].filter(Boolean) as string[];
  return (
    <div className="space-y-3 text-sm">
      {result.wasDryRun && <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">Dry run: nothing was sent and no device changed.</p>}
      {lines.map((l) => (
        <p key={l} className="text-ok">
          {l}
        </p>
      ))}
      {r.stopped && <p className="text-err">The launch stopped part-way: {r.stopped} The devices it didn't reach are still waiting in the wave.</p>}
      {r.failed.length > 0 && (
        <div>
          <p className="text-err">{plural(r.failed.length, "device")} didn't launch</p>
          <ul className="mt-1 max-h-40 space-y-1 overflow-y-auto pl-4 text-muted">
            {r.failed.map(([name, why]) => (
              <li key={name} className="list-disc break-words">
                {name}: {why}
              </li>
            ))}
          </ul>
        </div>
      )}
      {result.dryRun.length > 0 && (
        <div>
          <p className="font-medium">What would have been sent</p>
          <ul className="mt-1 max-h-48 space-y-2 overflow-y-auto">
            {result.dryRun.map((e, i) => (
              <li key={i} className="rounded-md border border-border p-2">
                <div className="text-xs">{e.summary}</div>
                <pre className="mt-1 max-h-24 overflow-auto text-[11px] whitespace-pre-wrap text-faint">{JSON.stringify(e.variables, null, 2)}</pre>
              </li>
            ))}
          </ul>
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

  return (
    <Modal
      title={p ? `Launch ${p.label}` : "Launch"}
      description={
        p
          ? p.reinstall
            ? `This reinstalls the agent on ${plural(total, "device")}. They go offline for a while.`
            : `This removes the old agent from ${plural(total, "device")} and installs the new one from ${p.destinationHost}. They go offline for a while.`
          : undefined
      }
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" busy={launch.isPending} disabled={blocked} onClick={() => launch.mutate({ wave, dryRun }, { onSuccess: setResult })}>
            {dryRun ? "Show the requests" : p ? `Launch ${total}` : "Launch"}
          </Button>
        </>
      }
    >
      {preview.isError ? (
        <ErrorNote>{String(preview.error)}</ErrorNote>
      ) : !p ? (
        <p className="text-sm text-muted">Checking the wave…</p>
      ) : (
        <div className="space-y-3 text-sm">
          {p.stoppers.map((s) => (
            <ErrorNote key={s}>{s}</ErrorNote>
          ))}
          <dl className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-1.5">
            <dt className="text-muted">By path</dt>
            <dd>
              Ecoverse {p.ecoverse} · N-central {p.rest}
            </dd>
            <dt className="text-muted">By destination</dt>
            <dd>{p.byDestination.map((d) => `${d.name} ${d.devices}`).join(" · ") || "–"}</dd>
            <dt className="text-muted">Cleanup</dt>
            <dd>{p.cleanFull ? "Full cleanup" : "Install over the top"} · review period {p.reviewDays} days</dd>
          </dl>
          {p.problems.length > 0 && (
            <div>
              <p className="text-warn">{plural(p.problems.length, "device")} can't be launched as planned</p>
              <ul className="mt-1 max-h-32 space-y-1 overflow-y-auto pl-4 text-xs text-muted">
                {p.problems.map((d) => (
                  <li key={d.name} className="list-disc break-words">
                    {d.name}: {d.why}
                  </li>
                ))}
              </ul>
            </div>
          )}
          <label className="flex cursor-pointer items-start gap-2.5">
            <input type="checkbox" className="mt-1" checked={dryRun} onChange={(e) => setDryRun(e.target.checked)} />
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
