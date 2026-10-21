import { RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";
import type { TrackRow } from "../../../lib/migrate/bindings";
import { useNow } from "../../../lib/format";
import { useDeviceActions, useProject, useTrack, useTracker } from "../../../lib/migrate/queries";
import { readyToFinish } from "../../../lib/migrate/progress";
import { GROUPS, ago, filterTrack, groupCounts, needsYou, type GroupId } from "../../../lib/migrate/track";
import { describeStage, ecoWords, STAGE_WORDS, stageView } from "../../../lib/migrate/words";
import { Button, ErrorNote, Status, Table, inputClass, selectClass } from "../../ui";
import { DeviceSheet } from "../parts/DeviceSheet";

const PAGE = 200;
const PATH_WORDS = { ecoverse: "Ecoverse", rest: "N-central" } as const;

function Strip({ counts, active, onPick }: { counts: ReturnType<typeof groupCounts>; active: GroupId | null; onPick: (id: GroupId | null) => void }) {
  const total = counts.reduce((n, c) => n + c.count, 0);
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-1.5">
        {counts.map((c) => (
          <button
            key={c.id}
            onClick={() => onPick(active === c.id ? null : c.id)}
            aria-pressed={active === c.id}
            className={`rounded-md border px-2.5 py-1 text-xs transition-colors ${active === c.id ? "border-accent bg-accent-soft" : "border-border hover:bg-surface-2"} ${c.id === "needsYou" && c.count > 0 ? "text-warn" : ""}`}
          >
            {c.label} <span className="tabular-nums">{c.count}</span>
          </button>
        ))}
      </div>
      <div className="flex h-1.5 overflow-hidden rounded-full bg-surface-3" aria-hidden>
        {counts.map((c) => (
          <div
            key={c.id}
            style={{ width: total ? `${(c.count / total) * 100}%` : 0 }}
            className={c.id === "needsYou" ? "bg-warn" : c.id === "healthy" || c.id === "done" ? "bg-ok" : c.id === "planned" ? "bg-faint" : "bg-accent"}
          />
        ))}
      </div>
    </div>
  );
}

function NeedsYou({ rows, now, onOpen }: { rows: TrackRow[]; now: number; onOpen: (index: number) => void }) {
  if (rows.length === 0) return null;
  return (
    <section className="space-y-2">
      <h3 className="text-sm font-semibold">Needs you ({rows.length})</h3>
      <ul className="divide-y divide-border rounded-lg border border-border">
        {rows.slice(0, 8).map((r) => (
          <li key={r.index} className="flex items-start justify-between gap-3 px-3 py-2.5">
            <div className="min-w-0 text-sm">
              <span className="font-medium">{r.name}</span> <span className="text-warn">{r.needsMatch ? "Several fit" : STAGE_WORDS[r.stage].short}</span>
              <p className="text-xs text-muted">
                {r.needsMatch ? "More than one device on the new server fits. Choose which one it is." : describeStage(r.stage, { since: ago(r.since, now), destination: r.destination, reason: r.lastError })}
              </p>
            </div>
            <Button size="sm" onClick={() => onOpen(r.index)}>
              Open
            </Button>
          </li>
        ))}
      </ul>
      {rows.length > 8 && <p className="text-xs text-faint">{rows.length - 8} more: choose Needs you above to see them all.</p>}
    </section>
  );
}

/** The home of a running project: the stages, what needs the person, and every device (step 6 in docs/migration/11-gui-design.md). */
export function TrackStep({ projectId, onContinue }: { projectId: string; onContinue: () => void }) {
  const project = useProject(projectId);
  const track = useTrack(projectId);
  const tracker = useTracker(projectId, true);
  const { poll } = useDeviceActions(projectId);
  const now = useNow();
  const [group, setGroup] = useState<GroupId | null>(null);
  const [wave, setWave] = useState<number | null>(null);
  const [customer, setCustomer] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [shown, setShown] = useState(PAGE);
  const [open, setOpen] = useState<number | null>(null);

  const view = track.data;
  const counts = useMemo(() => groupCounts(project.data?.stageCounts ?? {}, project.data?.ecoverseWaiting ?? 0), [project.data]);
  const rows = useMemo(() => (view ? filterTrack(view.rows, { group, wave, customer, text }) : []), [view, group, wave, customer, text]);
  const attention = useMemo(() => (view ? view.rows.filter(needsYou) : []), [view]);

  const readOnly = tracker.error?.includes("Another N-voke window") ?? false;
  const status = readOnly ? (
    <Status tone="warn" label="Read-only: another window is working on this project" />
  ) : tracker.error ? (
    <Status tone="err" label={`Couldn't read: ${tracker.error}`} />
  ) : tracker.starting ? (
    <Status tone="accent" pulse label="Reading the devices…" />
  ) : (
    <Status tone="ok" label={tracker.last ? `Tracking · last read ${ago(tracker.last.at, now)} ago` : "Tracking"} />
  );

  return (
    <div className="space-y-4 px-6 py-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-semibold">Track</h2>
        <div className="flex items-center gap-3">
          {status}
          <Button size="sm" busy={poll.isPending} disabled={readOnly} onClick={() => poll.mutate()}>
            <RefreshCw />
            Poll now
          </Button>
        </div>
      </div>

      {poll.isError && <ErrorNote>{String(poll.error)}</ErrorNote>}
      {tracker.last && tracker.last.errors.length > 0 && (
        <div className="space-y-0.5">
          {tracker.last.errors.map((e) => (
            <p key={e} className="text-xs text-warn">
              {e}
            </p>
          ))}
        </div>
      )}

      <Strip counts={counts} active={group} onPick={(g) => (setGroup(g), setShown(PAGE))} />
      {project.data && readyToFinish(project.data) && (
        <div className="flex items-center justify-between gap-3 rounded-lg border border-ok/40 bg-ok-soft px-4 py-3">
          <p className="text-sm">
            <span className="font-medium">Everything has moved and is working.</span> Each device is healthy and Ecoverse is receiving its data. What's left is in Finish: a report, and tidying up the migration's records.
          </p>
          <Button size="sm" variant="primary" onClick={onContinue}>
            Continue to Finish
          </Button>
        </div>
      )}
      <NeedsYou rows={attention} now={now} onOpen={setOpen} />

      {track.isError ? (
        <ErrorNote>{String(track.error)}</ErrorNote>
      ) : !view ? (
        <p className="text-sm text-muted">Loading…</p>
      ) : view.rows.length === 0 ? (
        <p className="text-sm text-muted">No devices are in this project yet. Add them in Plan.</p>
      ) : (
        <section className="space-y-2">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <h3 className="text-sm font-semibold">Devices</h3>
            <div className="flex items-center gap-2">
              <select className={`${selectClass} w-36`} value={wave ?? ""} onChange={(e) => setWave(e.target.value ? Number(e.target.value) : null)} aria-label="Wave">
                <option value="">All waves</option>
                {view.waves.map((w) => (
                  <option key={w.number} value={w.number}>
                    {w.label}
                  </option>
                ))}
              </select>
              <select className={`${selectClass} w-40`} value={customer ?? ""} onChange={(e) => setCustomer(e.target.value || null)} aria-label="Customer">
                <option value="">All customers</option>
                {view.customers.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
              <input className={`${inputClass} w-40`} placeholder="Search" value={text} onChange={(e) => setText(e.target.value)} aria-label="Search devices" />
            </div>
          </div>
          {rows.length === 0 ? (
            <p className="text-sm text-muted">No devices match{group ? ` ${GROUPS.find((g) => g.id === group)?.label}` : ""}.</p>
          ) : (
            <>
              <Table
                head={
                  <>
                    <th>Device</th>
                    <th>Customer</th>
                    <th>Wave</th>
                    <th>Stage</th>
                    <th>Since</th>
                    <th>Path</th>
                  </>
                }
              >
                {rows.slice(0, shown).map((r) => (
                  <tr key={r.index} className="cursor-pointer hover:bg-surface-2/60" onClick={() => setOpen(r.index)}>
                    <td className="font-medium">{r.name}</td>
                    <td className="text-muted">{r.customer}</td>
                    <td className="text-muted tabular-nums">{r.wave ?? "–"}</td>
                    <td>
                      <Status tone={stageView(r.stage, r.waitingForEcoverse).tone} pulse={stageView(r.stage, r.waitingForEcoverse).pulse} label={stageView(r.stage, r.waitingForEcoverse).short} />
                      {r.waitingForEcoverse && <div className={`text-xs ${ecoWords(r.ecoverse).tone === "warn" ? "text-warn" : "text-faint"}`}>{ecoWords(r.ecoverse).short}</div>}
                      {r.stage === "healthy" && r.ecoverse === "reporting" && <div className="text-xs text-faint">{ecoWords(r.ecoverse).short}</div>}
                    </td>
                    <td className="text-faint">{ago(r.since, now)}</td>
                    <td className="text-muted">{r.path ? PATH_WORDS[r.path] : "–"}</td>
                  </tr>
                ))}
              </Table>
              {rows.length > shown && (
                <Button size="sm" onClick={() => setShown(shown + PAGE)}>
                  Show more ({rows.length - shown} left)
                </Button>
              )}
            </>
          )}
        </section>
      )}

      {open !== null && <DeviceSheet projectId={projectId} index={open} onClose={() => setOpen(null)} />}
    </div>
  );
}
