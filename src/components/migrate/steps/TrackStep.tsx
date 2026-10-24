import { RefreshCw, Stethoscope } from "lucide-react";
import { useMemo, useState } from "react";
import type { DeviceCommand, GroupCount, NeedKind, ProbeReport, Stage, TrackGroup, TrackRow } from "../../../lib/migrate/bindings";
import { deviceCommand } from "../../../lib/migrate/api";
import { useNow } from "../../../lib/format";
import { useDeviceActions, useProject, useTrack, useTracker } from "../../../lib/migrate/queries";
import { readyToFinish } from "../../../lib/migrate/progress";
import { ACTION_WORDS, GROUP_LABELS, ago } from "../../../lib/migrate/track";
import { useDebounced } from "../../../lib/useDebounced";
import { HeaderActions } from "../HeaderActions";
import { trackerWords, usePublishTracker } from "../TrackerStatus";
import { describeStage, ECO_OVERDUE, ecoWords, movedBanner, STAGE_WORDS, stageView, UFA_WORDS } from "../../../lib/migrate/words";
import { Button, Card, ErrorNote, Modal, SelectionBar, Status, Table, inputClass } from "../../ui";
import { DeviceSheet } from "../parts/DeviceSheet";
import { BulkStatus, useBulk } from "../move/bulk";
import { FilterPicker } from "../move/FilterPicker";
import { NameList } from "../move/NameList";

const PAGE = 100;
const PATH_WORDS = { ecoverse: "Ecoverse", rest: "N-central" } as const;
const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

const GROUP_COLOR = (g: TrackGroup) => (g === "needsYou" ? "bg-warn" : g === "healthy" || g === "done" ? "bg-ok" : g === "planned" ? "bg-faint" : "bg-accent");

/**
 * What can be asked of many devices at once. Each is the per-device command, sent once per ticked device; the core
 * refuses one that doesn't fit a device's stage, and the stages here mirror its rule so the counts are honest.
 */
type BulkKind = "probe" | "retry" | "retryOtherPath" | "markResolved";
const BULK: { kind: BulkKind; command: DeviceCommand; stages: Stage[] }[] = [
  { kind: "probe", command: { kind: "probe" }, stages: ["stuckStaged", "healthy", "registered", "lost", "agentRemoved"] },
  { kind: "retry", command: { kind: "retry", otherPath: false, checked: false }, stages: ["launchFailed", "notPickedUp", "stuckStaged", "unreachable"] },
  { kind: "retryOtherPath", command: { kind: "retry", otherPath: true, checked: false }, stages: ["launchFailed", "stuckStaged", "notPickedUp"] },
  { kind: "markResolved", command: { kind: "resolve" }, stages: ["lost", "wrongPlace", "stuckStaged", "launchFailed", "notPickedUp", "unreachable"] },
];

function Strip({ counts, active, onPick }: { counts: GroupCount[]; active: TrackGroup | null; onPick: (id: TrackGroup | null) => void }) {
  const total = counts.reduce((n, c) => n + c.count, 0);
  return (
    <div className="space-y-2.5">
      <div className="flex flex-wrap gap-1.5" role="group" aria-label="Stages">
        {counts.map((c) => {
          const on = active === c.group;
          const warn = c.group === "needsYou" && c.count > 0;
          return (
            <button
              key={c.group}
              type="button"
              onClick={() => onPick(on ? null : c.group)}
              aria-pressed={on}
              className={`inline-flex h-8 items-center gap-2 rounded-full border px-3 text-xs whitespace-nowrap transition-colors ${
                on ? (warn ? "border-warn/40 bg-warn-soft" : "border-accent/40 bg-accent-soft") : "border-control-border bg-surface hover:bg-surface-2"
              }`}
            >
              <span className={`size-2 rounded-sm ${GROUP_COLOR(c.group)}`} />
              <span className={warn ? "text-warn" : ""}>{GROUP_LABELS[c.group]}</span>
              <span className="text-muted tabular-nums">{c.count.toLocaleString()}</span>
            </button>
          );
        })}
      </div>
      <div className="flex h-1.5 gap-px overflow-hidden rounded-full bg-surface-3" aria-hidden>
        {counts.map((c) => (
          <div key={c.group} style={{ width: total ? `${(c.count / total) * 100}%` : 0 }} className={GROUP_COLOR(c.group)} />
        ))}
      </div>
    </div>
  );
}

function NeedsYou({ kinds, total, now, onOpen, onShowAll }: { kinds: NeedKind[]; total: number; now: number; onOpen: (index: number) => void; onShowAll: () => void }) {
  if (kinds.length === 0) return null;
  return (
    <Card tone="warn" className="px-4 py-3">
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <h3 className="font-semibold text-warn">{plural(total, "device")} need you · by reason</h3>
        <button type="button" className="text-xs font-medium text-accent hover:text-accent-hover" onClick={onShowAll}>
          Show all in the table
        </button>
      </div>
      <ul className="mt-1 divide-y divide-warn/20">
        {kinds.map((k) => {
          const first = k.examples[0];
          const label = k.needsMatch ? "Several fit" : STAGE_WORDS[k.stage].short;
          return (
            <li key={`${k.stage}-${k.needsMatch}`} className="flex flex-wrap items-start gap-x-4 gap-y-2 py-2.5">
              <span className="w-14 text-base font-bold tabular-nums">{k.count.toLocaleString()}</span>
              <div className="min-w-0 flex-[1_1_260px] space-y-2">
                <div>
                  <span className="font-medium">{label}</span>
                  <p className="text-xs text-muted">
                    {k.needsMatch
                      ? "More than one device on the new server fits. Choose which one each is."
                      : describeStage(k.stage, { since: ago(first.since, now), destination: first.destination, reason: first.lastError })}
                  </p>
                </div>
                <div className="flex flex-wrap items-center gap-1.5">
                  {k.examples.map((r) => (
                    <Button key={r.index} size="sm" onClick={() => onOpen(r.index)} title={r.name}>
                      <span className="max-w-40 truncate">{r.name}</span>
                    </Button>
                  ))}
                  {k.count > k.examples.length && <span className="text-xs text-faint">and {(k.count - k.examples.length).toLocaleString()} more</span>}
                </div>
              </div>
            </li>
          );
        })}
      </ul>
    </Card>
  );
}

/** The home of a running project: the stages, what needs the person, and every device (step 6 in docs/migration/11-gui-design.md). */
export function TrackStep({ projectId, onContinue, onPrepare }: { projectId: string; onContinue: () => void; onPrepare: () => void }) {
  const project = useProject(projectId);
  // A closed project isn't tracked: everything in it is finished, or it was closed by hand and refuses changes.
  const tracker = useTracker(projectId, project.data ? !project.data.closed : false);
  const { poll, probeWaiting } = useDeviceActions(projectId);
  const bulk = useBulk(projectId);
  const [probed, setProbed] = useState<ProbeReport | null>(null);
  const now = useNow();
  const [group, setGroup] = useState<TrackGroup | null>(null);
  const [wave, setWave] = useState<number | null>(null);
  const [customer, setCustomer] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [offset, setOffset] = useState(0);
  const [open, setOpen] = useState<number | null>(null);
  const [ticked, setTicked] = useState<Set<number>>(new Set());
  const [confirming, setConfirming] = useState<(typeof BULK)[number] | null>(null);
  const search = useDebounced(text.trim());
  const query = useMemo(() => ({ group, wave, customer, text: search, offset, limit: PAGE }), [group, wave, customer, search, offset]);
  const track = useTrack(projectId, query);
  const narrow = (change: () => void) => {
    change();
    setOffset(0);
    setTicked(new Set());
  };

  const view = track.data;
  const rows = view?.rows ?? [];
  const waitingCount = view?.counts.find((c) => c.group === "waiting")?.count ?? 0;
  const waves = useMemo(() => (view?.waves ?? []).map((w) => ({ key: w.number, label: w.label })), [view?.waves]);
  const customers = useMemo(() => (view?.customers ?? []).map((c) => ({ key: c, label: c })), [view?.customers]);
  const tickedRows = rows.filter((r) => ticked.has(r.index));
  const allTicked = rows.length > 0 && tickedRows.length === rows.length;
  const fits = (b: (typeof BULK)[number]) => tickedRows.filter((r) => b.stages.includes(r.stage));

  // The tracker's state shows in the top menu; only being read-only changes what this page lets you do.
  usePublishTracker(project.data?.name, tracker);
  const { readOnly } = trackerWords(tracker, now);

  const toggle = (index: number) =>
    setTicked((cur) => {
      const next = new Set(cur);
      if (!next.delete(index)) next.add(index);
      return next;
    });
  const runBulk = (b: (typeof BULK)[number]) => {
    const targets = fits(b);
    setConfirming(null);
    void bulk
      .run(
        ACTION_WORDS[b.kind].label,
        targets,
        (r: TrackRow) => r.name,
        (r) => deviceCommand(projectId, r.index, b.command),
      )
      .then(() => setTicked(new Set()));
  };

  return (
    <div className="space-y-4">
      <HeaderActions>
        {waitingCount > 0 && (
          <Button busy={probeWaiting.isPending} disabled={readOnly || poll.isPending} onClick={() => probeWaiting.mutate(undefined, { onSuccess: setProbed })}>
            <Stethoscope />
            Probe waiting · {waitingCount.toLocaleString()}
          </Button>
        )}
        <Button busy={poll.isPending} disabled={readOnly || probeWaiting.isPending} onClick={() => poll.mutate()}>
          <RefreshCw />
          Poll now
        </Button>
      </HeaderActions>
      {readOnly && <p className="text-sm text-warn">Read-only: another window is working on this project.</p>}

      {probeWaiting.isError && <ErrorNote>{String(probeWaiting.error)}</ErrorNote>}
      {probed && (
        <Modal title="Probe finished" onClose={() => setProbed(null)} footer={<Button onClick={() => setProbed(null)}>Done</Button>}>
          <div className="space-y-3 text-sm">
            <p>
              {probed.probed.toLocaleString()} {probed.probed === 1 ? "device answered" : "devices answered"}.
            </p>
            {probed.states.length > 0 && (
              <ul className="space-y-1">
                {probed.states.map((s) => (
                  <li key={s.state} className="flex items-center justify-between gap-3">
                    <Status tone={UFA_WORDS[s.state].tone} label={UFA_WORDS[s.state].short} />
                    <span className="text-muted tabular-nums">{s.count.toLocaleString()}</span>
                  </li>
                ))}
              </ul>
            )}
            {probed.noRecord > 0 && <p className="text-muted">{probed.noRecord.toLocaleString()} answered with no record of the Ecoverse agent.</p>}
            {probed.failed.length > 0 && (
              <>
                <p className="text-err">{probed.failed.length.toLocaleString()} couldn't be reached</p>
                <NameList className="text-muted" items={probed.failed.map((f) => ({ key: f.name, text: `${f.name}: ${f.why}` }))} />
              </>
            )}
            {probed.notReached > 0 && (
              <p className="text-warn">{probed.notReached.toLocaleString()} more are still waiting and weren't probed this time (the longest-waiting go first). Probe again for the rest.</p>
            )}
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

      {view && view.ecoverseOverdue > 0 && (
        <Card tone="warn" className="px-4 py-3 text-sm">
          <span className="font-medium text-warn">
            {view.ecoverseOverdue.toLocaleString()} {view.ecoverseOverdue === 1 ? "device has" : "devices have"} waited more than 2 hours for Ecoverse's data.
          </span>{" "}
          <span className="text-muted">Healthy ones got theirs in about an hour and a half. Open one, or use Probe waiting, to see what its Ecoverse agent reports.</span>
        </Card>
      )}
      {view && <Strip counts={view.counts} active={group} onPick={(g) => narrow(() => setGroup(g))} />}
      {project.data && readyToFinish(project.data) && (
        <div className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border border-ok/30 bg-ok-soft px-4 py-3">
          <p className="min-w-60 flex-1 text-sm">
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
        <section className="space-y-3" aria-label="Devices">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="mr-2 font-semibold">Devices</h3>
            <FilterPicker label="Wave" anyLabel="All waves" options={waves} value={wave} onChange={(w) => narrow(() => setWave(w))} />
            <FilterPicker label="Customer" anyLabel="All customers" options={customers} value={customer} onChange={(c) => narrow(() => setCustomer(c))} />
            <input className={`${inputClass} max-w-64 flex-[1_1_180px]`} placeholder="Search devices" value={text} onChange={(e) => narrow(() => setText(e.target.value))} aria-label="Search devices" />
          </div>

          {tickedRows.length > 0 && (
            <SelectionBar count={tickedRows.length} onClear={() => setTicked(new Set())}>
              {BULK.map((b) => {
                const n = fits(b).length;
                const label = ACTION_WORDS[b.kind].label;
                return (
                  <Button
                    key={b.kind}
                    size="sm"
                    disabled={n === 0 || bulk.busy || readOnly}
                    title={n < tickedRows.length ? `Fits ${n} of the ticked devices` : undefined}
                    onClick={() => (ACTION_WORDS[b.kind].confirm ? setConfirming(b) : runBulk(b))}
                  >
                    {ACTION_WORDS[b.kind].confirm ? `${label}…` : label}
                    {n > 0 && n < tickedRows.length && <span className="text-faint tabular-nums">{n}</span>}
                  </Button>
                );
              })}
            </SelectionBar>
          )}
          <BulkStatus state={bulk.state} onClear={bulk.clear} />

          {rows.length === 0 ? (
            <p className="text-sm text-muted">No devices match{group ? ` ${GROUP_LABELS[group]}` : ""}.</p>
          ) : (
            <>
              <Table
                head={
                  <>
                    <th className="w-9">
                      <input
                        type="checkbox"
                        className="size-3.5 accent-accent"
                        checked={allTicked}
                        ref={(el) => {
                          if (el) el.indeterminate = tickedRows.length > 0 && !allTicked;
                        }}
                        onChange={() => setTicked(allTicked ? new Set() : new Set(rows.map((r) => r.index)))}
                        aria-label="Tick every device on this page"
                      />
                    </th>
                    <th>Device</th>
                    <th>Customer</th>
                    <th>Wave</th>
                    <th>Stage</th>
                    <th>Since</th>
                    <th>Path</th>
                  </>
                }
              >
                {rows.map((r) => {
                  const sv = stageView(r.stage, r.waitingForEcoverse);
                  return (
                    <tr key={r.index} className={`cursor-pointer hover:bg-surface-2/60 ${ticked.has(r.index) ? "bg-accent-soft/60" : ""}`} onClick={() => setOpen(r.index)}>
                      <td onClick={(e) => e.stopPropagation()}>
                        <input type="checkbox" className="size-3.5 accent-accent" checked={ticked.has(r.index)} onChange={() => toggle(r.index)} aria-label={`Tick ${r.name}`} />
                      </td>
                      <td className="max-w-56 truncate font-medium whitespace-nowrap" title={r.name}>
                        {r.name}
                      </td>
                      <td className="max-w-0 truncate text-muted" title={r.customer}>
                        {r.customer}
                      </td>
                      <td className="text-muted tabular-nums">{r.wave ?? "–"}</td>
                      <td className="whitespace-nowrap">
                        <Status tone={sv.tone} pulse={sv.pulse} label={sv.short} />
                        {r.waitingForEcoverse && r.ecoverseOverdue && <div className="text-xs text-warn">{ECO_OVERDUE.short}</div>}
                        {r.waitingForEcoverse && !r.ecoverseOverdue && <div className={`text-xs ${ecoWords(r.ecoverse).tone === "warn" ? "text-warn" : "text-faint"}`}>{ecoWords(r.ecoverse).short}</div>}
                        {r.stage === "healthy" && r.ecoverse === "reporting" && <div className="text-xs text-faint">{ecoWords(r.ecoverse).short}</div>}
                      </td>
                      <td className="whitespace-nowrap text-faint">{ago(r.since, now)}</td>
                      <td className="whitespace-nowrap text-muted">{r.path ? PATH_WORDS[r.path] : "–"}</td>
                    </tr>
                  );
                })}
              </Table>
              <div className="flex items-center justify-between gap-3 text-xs text-muted">
                <span className="tabular-nums">
                  {(view.offset + 1).toLocaleString()}–{(view.offset + rows.length).toLocaleString()} of {view.matching.toLocaleString()}
                </span>
                {view.matching > PAGE && (
                  <span className="flex items-center gap-2">
                    <Button size="sm" disabled={offset === 0} onClick={() => (setOffset(Math.max(0, offset - PAGE)), setTicked(new Set()))}>
                      Previous
                    </Button>
                    <Button size="sm" disabled={offset + PAGE >= view.matching} onClick={() => (setOffset(offset + PAGE), setTicked(new Set()))}>
                      Next
                    </Button>
                  </span>
                )}
              </div>
            </>
          )}
        </section>
      )}

      {confirming && (
        <Modal
          title={`${ACTION_WORDS[confirming.kind].label}: ${plural(fits(confirming).length, "device")}?`}
          description={ACTION_WORDS[confirming.kind].confirm ?? undefined}
          onClose={() => setConfirming(null)}
          footer={
            <>
              <Button onClick={() => setConfirming(null)}>Cancel</Button>
              <Button variant="primary" onClick={() => runBulk(confirming)}>
                {ACTION_WORDS[confirming.kind].label}
              </Button>
            </>
          }
        >
          <div className="space-y-2 text-sm">
            {fits(confirming).length < tickedRows.length && (
              <p className="text-muted">
                {plural(tickedRows.length - fits(confirming).length, "ticked device")} {tickedRows.length - fits(confirming).length === 1 ? "isn't" : "aren't"} at a stage this fits, and {tickedRows.length - fits(confirming).length === 1 ? "is" : "are"} left
                as {tickedRows.length - fits(confirming).length === 1 ? "it is" : "they are"}.
              </p>
            )}
            <NameList items={fits(confirming).map((r) => ({ key: String(r.index), text: r.name }))} />
          </div>
        </Modal>
      )}

      {open !== null && <DeviceSheet projectId={projectId} index={open} onClose={() => setOpen(null)} />}
    </div>
  );
}
