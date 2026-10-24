import { Download } from "lucide-react";
import { useState } from "react";
import type { CleanupDevice, CleanupReport } from "../../../lib/migrate/bindings";
import { cleanupScope, waveGroups } from "../../../lib/migrate/cleanup";
import { stopCleanup, useCleanupProgress, useFinish, useFinishActions } from "../../../lib/migrate/queries";
import { Button, Card, CopyButton, Dot, ErrorNote, Modal, inputClass } from "../../ui";
import { NameList } from "../move/NameList";
import { RetireCard } from "../parts/RetireCard";

const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

/** Devices drawn in the cleanup list before a search is needed. */
const SHOWN = 50;

function Section({ title, aside, children }: { title: string; aside?: React.ReactNode; children: React.ReactNode }) {
  return (
    <Card className="space-y-3 p-4">
      <div className="flex items-center justify-between gap-3">
        <h3 className="font-semibold">{title}</h3>
        {aside}
      </div>
      {children}
    </Card>
  );
}

function Stat({ value, label, tone = "" }: { value: number; label: string; tone?: string }) {
  return (
    <Card className="px-4 py-3">
      <div className={`text-2xl font-bold tabular-nums ${tone}`}>{value.toLocaleString()}</div>
      <div className="text-sm text-muted">{label}</div>
    </Card>
  );
}

/** The ready devices with a tick each. A project can have thousands, so only the first 50 that match the search are drawn. */
function PickList({ devices, picked, onToggle }: { devices: CleanupDevice[]; picked: ReadonlySet<string>; onToggle: (id: string) => void }) {
  const [term, setTerm] = useState("");
  const needle = term.trim().toLowerCase();
  const matching = needle ? devices.filter((d) => d.name.toLowerCase().includes(needle)) : devices;
  return (
    <div className="space-y-2">
      {devices.length > SHOWN && <input className={inputClass} placeholder={`Search ${devices.length.toLocaleString()} devices`} value={term} onChange={(e) => setTerm(e.target.value)} aria-label="Search the ready devices" />}
      <ul className="max-h-56 space-y-1.5 overflow-y-auto text-sm">
        {matching.slice(0, SHOWN).map((d) => (
          <li key={d.deviceId} className="flex items-center gap-2 whitespace-nowrap">
            <input type="checkbox" className="accent-accent" checked={picked.has(d.deviceId)} onChange={() => onToggle(d.deviceId)} aria-label={`Clean up ${d.name}`} />
            <span className="min-w-0 truncate" title={d.name}>
              {d.name}
            </span>
            {d.waveLabel && <span className="shrink-0 text-xs text-faint">{d.waveLabel}</span>}
          </li>
        ))}
      </ul>
      {matching.length > SHOWN && <p className="text-xs text-faint">and {(matching.length - SHOWN).toLocaleString()} more</p>}
    </div>
  );
}

/** Where the project stands, the report, and tidying up (step 7 in docs/migration/11-gui-design.md). */
export function FinishStep({ projectId, onOptions }: { projectId: string; onOptions: () => void }) {
  const finish = useFinish(projectId);
  const { report, cleanup } = useFinishActions(projectId);
  const [savedAt, setSavedAt] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [cleaned, setCleaned] = useState<CleanupReport | null>(null);
  const v = finish.data;
  const steps = useCleanupProgress(projectId, cleanup.isPending);
  const [stopping, setStopping] = useState(false);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const ready = v?.cleanupDevices ?? [];
  const chosen = ready.filter((d) => picked.has(d.deviceId));
  const waves = waveGroups(ready);
  const allPicked = ready.length > 0 && chosen.length === ready.length;
  const toggle = (id: string) =>
    setPicked((cur) => {
      const next = new Set(cur);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const startCleanup = () => (cleanup.reset(), setPicked(new Set(ready.map((d) => d.deviceId))), setConfirming(true));
  const healthy = v ? v.healthy - v.waitingForEcoverse.length : 0;
  const stepCounts = Object.values(steps).reduce((n, s) => ({ ...n, [s.state]: (n[s.state] ?? 0) + 1 }), {} as Record<string, number>);

  return (
    <div className="space-y-4">
      {finish.isError ? (
        <ErrorNote>{String(finish.error)}</ErrorNote>
      ) : !v ? (
        <p className="text-sm text-muted">Loading…</p>
      ) : (
        <>
          <Card tone={v.closed ? undefined : "accent"} className="flex flex-wrap items-center gap-x-5 gap-y-3 px-5 py-4">
            <div className="min-w-0 flex-[1_1_360px] space-y-1">
              <div className="flex items-center gap-2">
                <span className="text-xs font-semibold tracking-wider text-accent uppercase">What happens next</span>
                <span className={`rounded-full px-2 text-[11px] leading-5 font-semibold ${v.closed ? "bg-ok-soft text-ok" : "bg-surface-3 text-muted"}`}>{v.closed ? "Closed" : "Open"}</span>
              </div>
              <p className="text-sm text-muted">
                {v.closed
                  ? "Every device is finished with, so this project is closed. Nothing more is needed."
                  : healthy > 0
                    ? `Each healthy device removes its own migration record ${v.reviewDays} days after it became healthy. This app notes it the next time the project is open (it checks every minute while the Track step is showing), and the project closes once every device has cleaned up. To finish sooner, run the cleanup below.`
                    : v.moving + v.waiting + v.needsYou > 0
                      ? "Some devices aren't done yet. Go back to Track to see what each is waiting for."
                      : "Nothing is waiting on you."}
              </p>
            </div>
            {!v.closed && ready.length > 0 && (
              <Button variant="primary" onClick={startCleanup}>
                Clean up {plural(ready.length, "device")} now…
              </Button>
            )}
          </Card>

          <div className="grid grid-cols-[repeat(auto-fit,minmax(150px,1fr))] gap-3">
            <Stat value={v.total} label="devices moved" />
            <Stat value={healthy} label="healthy" tone="text-ok" />
            {v.waitingForEcoverse.length > 0 && <Stat value={v.waitingForEcoverse.length} label="waiting for Ecoverse" tone="text-warn" />}
            <Stat value={v.moving} label="still moving" tone="text-accent" />
            <Stat value={v.needsYou} label="need you" tone={v.needsYou ? "text-warn" : ""} />
            <Stat value={v.waiting} label="not launched yet" tone="text-muted" />
            <Stat value={v.cleanedUp + v.retired} label="cleaned up or done" tone="text-muted" />
          </div>

          <div className="grid grid-cols-[repeat(auto-fit,minmax(380px,1fr))] items-start gap-4">
            <Section title="Clean up" aside={<span className="text-xs text-faint">{plural(ready.length, "device")} ready</span>}>
              <p className="text-sm text-muted">
                Each healthy device removes what the migration left on it (its scheduled tasks and the N-voke folder, with the status file and logs) by itself after its review period of {v.reviewDays} days. To do it sooner, run the cleanup now. It never changes either N-central server or
                Ecoverse.
              </p>
              {v.waitingForEcoverse.length > 0 && (
                <p className="text-sm text-warn">
                  {plural(v.waitingForEcoverse.length, "device")} {v.waitingForEcoverse.length === 1 ? "is" : "are"} registered with Ecoverse but not sending asset data yet, so {v.waitingForEcoverse.length === 1 ? "it isn't" : "they aren't"} fully working and
                  {v.waitingForEcoverse.length === 1 ? " is" : " are"} left out of this cleanup: {v.waitingForEcoverse.slice(0, 6).join(", ")}
                  {v.waitingForEcoverse.length > 6 ? ` and ${(v.waitingForEcoverse.length - 6).toLocaleString()} more` : ""}.
                </p>
              )}
              <Button size="sm" disabled={ready.length === 0} onClick={startCleanup}>
                Run cleanup now…
              </Button>
            </Section>

            <RetireCard projectId={projectId} onOptions={onOptions} />
          </div>

          <Section title="Report">
            <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
              <p className="min-w-60 flex-1 text-sm text-muted">Every device with its stage, path, wave and history. It is saved to your Downloads folder.</p>
              {(["html", "csv"] as const).map((format) => (
                <Button key={format} size="sm" busy={report.isPending && report.variables === format} onClick={() => report.mutate(format, { onSuccess: setSavedAt })}>
                  <Download />
                  Save as {format === "html" ? "a web page" : "a spreadsheet (CSV)"}
                </Button>
              ))}
            </div>
            {savedAt && (
              <div className="flex items-center gap-2 text-xs text-ok">
                <span className="break-all">Saved to {savedAt}</span>
                <CopyButton text={savedAt} label="Copy path" />
              </div>
            )}
            {report.isError && <ErrorNote>{String(report.error)}</ErrorNote>}
          </Section>
        </>
      )}

      {confirming && v && (
        <Modal
          title={cleanup.isPending ? "Cleaning up…" : `Run cleanup on ${plural(chosen.length, "device")}?`}
          description={
            cleanup.isPending
              ? "Each device is asked in turn and its answer is waited for, up to two minutes. You can stop after the device it is on."
              : "Each device removes what the migration left on it: its scheduled tasks and the N-voke folder. Nothing on either N-central server changes. This can't be undone. Devices are done one after another, so it can take a while."
          }
          dismissible={!cleanup.isPending}
          onClose={() => setConfirming(false)}
          footer={
            cleanup.isPending ? (
              <Button
                disabled={stopping}
                onClick={() => {
                  setStopping(true);
                  void stopCleanup(projectId);
                }}
              >
                {stopping ? "Stopping after this device…" : "Stop after this device"}
              </Button>
            ) : (
              <>
                <Button onClick={() => setConfirming(false)}>Cancel</Button>
                <Button
                  variant="primary"
                  disabled={chosen.length === 0}
                  onClick={() => {
                    setStopping(false);
                    cleanup.mutate(cleanupScope(ready, picked), { onSuccess: (r) => (setConfirming(false), setCleaned(r)) });
                  }}
                >
                  Run cleanup
                </Button>
              </>
            )
          }
        >
          {cleanup.isPending ? (
            <div className="space-y-2">
              <p className="text-sm text-muted tabular-nums">
                {(stepCounts.cleaned ?? 0).toLocaleString()} cleaned up · {(stepCounts.failed ?? 0).toLocaleString()} didn't · {(chosen.length - (stepCounts.cleaned ?? 0) - (stepCounts.failed ?? 0)).toLocaleString()} to go
              </p>
              <NameList
                items={chosen.map((d) => {
                  const step = steps[d.name];
                  const tone = !step ? "muted" : step.state === "cleaned" ? "ok" : step.state === "failed" ? "err" : "accent";
                  return {
                    key: d.deviceId,
                    search: d.name,
                    text: (
                      <span className="inline-flex flex-col">
                        <span className="flex items-center gap-2">
                          <Dot tone={tone} pulse={step?.state === "running"} />
                          <span>{d.name}</span>
                          {step && <span className="text-xs text-muted">{step.state === "running" ? "cleaning…" : step.state === "cleaned" ? "cleaned up" : "didn't clean up"}</span>}
                        </span>
                        {step?.why && <span className="pl-4 text-xs break-words text-muted">{step.why}</span>}
                      </span>
                    ),
                  };
                })}
              />
            </div>
          ) : (
            <div className="space-y-3">
              <div className="flex flex-wrap items-center gap-1.5 text-xs">
                <span className="mr-1 text-muted">Choose by</span>
                <Button size="sm" variant={allPicked ? "primary" : "secondary"} onClick={() => setPicked(new Set(ready.map((d) => d.deviceId)))}>
                  All ready · {ready.length.toLocaleString()}
                </Button>
                {waves.length > 1 &&
                  waves.map((g) => (
                    <Button key={g.wave ?? "none"} size="sm" onClick={() => setPicked(new Set(g.ids))}>
                      {g.label} · {g.ids.length.toLocaleString()}
                    </Button>
                  ))}
                <Button size="sm" variant="ghost" onClick={() => setPicked(new Set())}>
                  None
                </Button>
              </div>
              <PickList devices={ready} picked={picked} onToggle={toggle} />
            </div>
          )}
          {cleanup.isError && (
            <div className="mt-3">
              <ErrorNote>{String(cleanup.error)}</ErrorNote>
            </div>
          )}
        </Modal>
      )}
      {cleaned && (
        <Modal title="Cleanup finished" onClose={() => setCleaned(null)} footer={<Button onClick={() => setCleaned(null)}>Done</Button>}>
          <div className="space-y-2 text-sm">
            {cleaned.stopped && <p className="text-warn">Stopped as asked: {plural(cleaned.notReached, "device")} not reached, and still as they were.</p>}
            <p className="text-ok">{plural(cleaned.cleaned, "device")} cleaned up</p>
            {cleaned.failed.length > 0 && (
              <>
                <p className="text-err">{plural(cleaned.failed.length, "device")} didn't clean up</p>
                <NameList className="text-muted" items={cleaned.failed.map((f) => ({ key: f.name, text: `${f.name}: ${f.why}` }))} />
              </>
            )}
          </div>
        </Modal>
      )}
    </div>
  );
}
