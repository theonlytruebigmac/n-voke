import { RefreshCw, Stethoscope } from "lucide-react";
import { useMemo, useState } from "react";
import type { GroupCount, NeedKind, ProbeReport, TrackGroup } from "../../../lib/migrate/bindings";
import { useNow } from "../../../lib/format";
import { useDeviceActions, useProject, useTrack, useTracker } from "../../../lib/migrate/queries";
import { readyToFinish } from "../../../lib/migrate/progress";
import { GROUP_LABELS, ago } from "../../../lib/migrate/track";
import { useDebounced } from "../../../lib/useDebounced";
import { describeStage, ecoWords, movedBanner, STAGE_WORDS, stageView, UFA_WORDS } from "../../../lib/migrate/words";
import { Button, ErrorNote, Modal, Status, Table, inputClass, selectClass } from "../../ui";
import { DeviceSheet } from "../parts/DeviceSheet";

const PAGE = 100;
const PATH_WORDS = { ecoverse: "Ecoverse", rest: "N-central" } as const;

function Strip({ counts, active, onPick }: { counts: GroupCount[]; active: TrackGroup | null; onPick: (id: TrackGroup | null) => void }) {
  const total = counts.reduce((n, c) => n + c.count, 0);
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-1.5">
        {counts.map((c) => (
          <button
            key={c.group}
            onClick={() => onPick(active === c.group ? null : c.group)}
            aria-pressed={active === c.group}
            className={`rounded-md border px-2.5 py-1 text-xs transition-colors ${active === c.group ? "border-accent bg-accent-soft" : "border-border hover:bg-surface-2"} ${c.group === "needsYou" && c.count > 0 ? "text-warn" : ""}`}
          >
            {GROUP_LABELS[c.group]} <span className="tabular-nums">{c.count}</span>
          </button>
        ))}
      </div>
      <div className="flex h-1.5 overflow-hidden rounded-full bg-surface-3" aria-hidden>
        {counts.map((c) => (
          <div
            key={c.group}
            style={{ width: total ? `${(c.count / total) * 100}%` : 0 }}
            className={c.group === "needsYou" ? "bg-warn" : c.group === "healthy" || c.group === "done" ? "bg-ok" : c.group === "planned" ? "bg-faint" : "bg-accent"}
          />
        ))}
      </div>
    </div>
  );
}

function NeedsYou({ kinds, total, now, onOpen, onShowAll }: { kinds: NeedKind[]; total: number; now: number; onOpen: (index: number) => void; onShowAll: () => void }) {
  if (kinds.length === 0) return null;
  return (
    <section className="space-y-2">
      <div className="flex items-baseline justify-between gap-3">
        <h3 className="text-sm font-semibold">Needs you ({total})</h3>
        <button className="text-xs text-muted underline hover:text-text" onClick={onShowAll}>
          Show all in the table
        </button>
      </div>
      <ul className="divide-y divide-border rounded-lg border border-border">
        {kinds.map((k) => {
          const first = k.examples[0];
          const label = k.needsMatch ? "Several fit" : STAGE_WORDS[k.stage].short;
          return (
            <li key={`${k.stage}-${k.needsMatch}`} className="space-y-2 px-3 py-2.5">
              <div className="min-w-0 text-sm">
                <span className="font-medium text-warn">{label}</span> <span className="text-muted tabular-nums">· {k.count === 1 ? "1 device" : `${k.count} devices`}</span>
                <p className="text-xs text-muted">
                  {k.needsMatch
                    ? "More than one device on the new server fits. Choose which one each is."
                    : describeStage(k.stage, { since: ago(first.since, now), destination: first.destination, reason: first.lastError })}
                </p>
              </div>
              <div className="flex flex-wrap items-center gap-1.5">
                {k.examples.map((r) => (
                  <Button key={r.index} size="sm" onClick={() => onOpen(r.index)}>
                    {r.name}
                  </Button>
                ))}
                {k.count > k.examples.length && <span className="text-xs text-faint">and {k.count - k.examples.length} more</span>}
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** The home of a running project: the stages, what needs the person, and every device (step 6 in docs/migration/11-gui-design.md). */
export function TrackStep({ projectId, onContinue, onPrepare }: { projectId: string; onContinue: () => void; onPrepare: () => void }) {
  const project = useProject(projectId);
  const tracker = useTracker(projectId, true);
  const { poll, probeWaiting } = useDeviceActions(projectId);
  const [probed, setProbed] = useState<ProbeReport | null>(null);
  const now = useNow();
  const [group, setGroup] = useState<TrackGroup | null>(null);
  const [wave, setWave] = useState<number | null>(null);
  const [customer, setCustomer] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [offset, setOffset] = useState(0);
  const [open, setOpen] = useState<number | null>(null);
  const search = useDebounced(text.trim());
  const query = useMemo(() => ({ group, wave, customer, text: search, offset, limit: PAGE }), [group, wave, customer, search, offset]);
  const track = useTrack(projectId, query);
  const narrow = (change: () => void) => {
    change();
    setOffset(0);
  };

  const view = track.data;
  const rows = view?.rows ?? [];
  const waitingCount = view?.counts.find((c) => c.group === "waiting")?.count ?? 0;

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
          {waitingCount > 0 && (
            <Button size="sm" busy={probeWaiting.isPending} disabled={readOnly || poll.isPending} onClick={() => probeWaiting.mutate(undefined, { onSuccess: setProbed })}>
              <Stethoscope />
              Probe waiting ({waitingCount})
            </Button>
          )}
          <Button size="sm" busy={poll.isPending} disabled={readOnly || probeWaiting.isPending} onClick={() => poll.mutate()}>
            <RefreshCw />
            Poll now
          </Button>
        </div>
      </div>

      {probeWaiting.isError && <ErrorNote>{String(probeWaiting.error)}</ErrorNote>}
      {probed && (
        <Modal title="Probe finished" onClose={() => setProbed(null)} footer={<Button onClick={() => setProbed(null)}>Done</Button>}>
          <div className="space-y-3 text-sm">
            <p>
              {probed.probed} {probed.probed === 1 ? "device answered" : "devices answered"}.
            </p>
            {probed.states.length > 0 && (
              <ul className="space-y-1">
                {probed.states.map((s) => (
                  <li key={s.state} className="flex items-center justify-between gap-3">
                    <Status tone={UFA_WORDS[s.state].tone} label={UFA_WORDS[s.state].short} />
                    <span className="tabular-nums text-muted">{s.count}</span>
                  </li>
                ))}
              </ul>
            )}
            {probed.noRecord > 0 && <p className="text-muted">{probed.noRecord} answered with no record of the Ecoverse agent.</p>}
            {probed.failed.length > 0 && (
              <>
                <p className="text-err">{probed.failed.length} couldn't be reached</p>
                <ul className="max-h-40 space-y-1 overflow-y-auto pl-4 text-muted">
                  {probed.failed.map((f) => (
                    <li key={f.name} className="list-disc break-words">
                      {f.name}: {f.why}
                    </li>
                  ))}
                </ul>
              </>
            )}
            {probed.notReached > 0 && <p className="text-warn">{probed.notReached} more are still waiting and weren't probed this time (the longest-waiting go first). Probe again for the rest.</p>}
            <p className="text-xs text-faint">Open a device to see what its Ecoverse agent reported and what the installer did.</p>
          </div>
        </Modal>
      )}

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

      {view && <Strip counts={view.counts} active={group} onPick={(g) => narrow(() => setGroup(g))} />}
      {project.data && readyToFinish(project.data) && (
        <div className="flex items-center justify-between gap-3 rounded-lg border border-ok/40 bg-ok-soft px-4 py-3">
          <p className="text-sm">
            <span className="font-medium">{movedBanner(project.data.ecoverseChecked).lead}</span> {movedBanner(project.data.ecoverseChecked).rest}
          </p>
          <div className="flex shrink-0 items-center gap-2">
            {!project.data.ecoverseChecked && (
              <Button size="sm" onClick={onPrepare}>
                Check Ecoverse in Prepare
              </Button>
            )}
            <Button size="sm" variant="primary" onClick={onContinue}>
              Continue to Finish
            </Button>
          </div>
        </div>
      )}
      {view && <NeedsYou kinds={view.needsYou} total={view.needsYouTotal} now={now} onOpen={setOpen} onShowAll={() => narrow(() => setGroup("needsYou"))} />}

      {track.isError ? (
        <ErrorNote>{String(track.error)}</ErrorNote>
      ) : !view ? (
        <p className="text-sm text-muted">Loading…</p>
      ) : view.counts.every((c) => c.count === 0) ? (
        <p className="text-sm text-muted">No devices are in this project yet. Add them in Plan.</p>
      ) : (
        <section className="space-y-2">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <h3 className="text-sm font-semibold">Devices</h3>
            <div className="flex items-center gap-2">
              <select className={`${selectClass} w-36`} value={wave ?? ""} onChange={(e) => narrow(() => setWave(e.target.value ? Number(e.target.value) : null))} aria-label="Wave">
                <option value="">All waves</option>
                {view.waves.map((w) => (
                  <option key={w.number} value={w.number}>
                    {w.label}
                  </option>
                ))}
              </select>
              <select className={`${selectClass} w-40`} value={customer ?? ""} onChange={(e) => narrow(() => setCustomer(e.target.value || null))} aria-label="Customer">
                <option value="">All customers</option>
                {view.customers.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
              <input className={`${inputClass} w-40`} placeholder="Search" value={text} onChange={(e) => narrow(() => setText(e.target.value))} aria-label="Search devices" />
            </div>
          </div>
          {rows.length === 0 ? (
            <p className="text-sm text-muted">No devices match{group ? ` ${GROUP_LABELS[group]}` : ""}.</p>
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
                {rows.map((r) => (
                  <tr key={r.index} className="cursor-pointer hover:bg-surface-2/60" onClick={() => setOpen(r.index)}>
                    <td className="font-medium whitespace-nowrap">{r.name}</td>
                    <td className="text-muted">{r.customer}</td>
                    <td className="text-muted tabular-nums">{r.wave ?? "–"}</td>
                    <td>
                      <Status tone={stageView(r.stage, r.waitingForEcoverse).tone} pulse={stageView(r.stage, r.waitingForEcoverse).pulse} label={stageView(r.stage, r.waitingForEcoverse).short} />
                      {r.waitingForEcoverse && <div className={`text-xs ${ecoWords(r.ecoverse).tone === "warn" ? "text-warn" : "text-faint"}`}>{ecoWords(r.ecoverse).short}</div>}
                      {r.stage === "healthy" && r.ecoverse === "reporting" && <div className="text-xs text-faint">{ecoWords(r.ecoverse).short}</div>}
                    </td>
                    <td className="text-faint">{ago(r.since, now)}</td>
                    <td className="text-muted whitespace-nowrap">{r.path ? PATH_WORDS[r.path] : "–"}</td>
                  </tr>
                ))}
              </Table>
              <div className="flex items-center justify-between gap-3 text-xs text-muted">
                <span>
                  {view.offset + 1}–{view.offset + rows.length} of {view.matching}
                </span>
                {view.matching > PAGE && (
                  <span className="flex items-center gap-2">
                    <Button size="sm" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - PAGE))}>
                      Previous
                    </Button>
                    <Button size="sm" disabled={offset + PAGE >= view.matching} onClick={() => setOffset(offset + PAGE)}>
                      Next
                    </Button>
                  </span>
                )}
              </div>
            </>
          )}
        </section>
      )}

      {open !== null && <DeviceSheet projectId={projectId} index={open} onClose={() => setOpen(null)} />}
    </div>
  );
}
