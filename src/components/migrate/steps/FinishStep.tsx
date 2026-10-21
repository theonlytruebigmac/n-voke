import { Download } from "lucide-react";
import { useState } from "react";
import type { CleanupReport } from "../../../lib/migrate/bindings";
import { stopCleanup, useCleanupProgress, useFinish, useFinishActions } from "../../../lib/migrate/queries";
import { Button, CopyButton, Dot, ErrorNote, Modal, Status } from "../../ui";
import { RetireCard } from "../parts/RetireCard";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

function Card({ title, aside, children }: { title: string; aside?: React.ReactNode; children: React.ReactNode }) {
  return (
    <section className="space-y-3 rounded-lg border border-border p-4">
      <div className="flex items-center justify-between gap-3">
        <h3 className="text-sm font-semibold">{title}</h3>
        {aside}
      </div>
      {children}
    </section>
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

  return (
    <div className="mx-auto max-w-2xl space-y-4 px-6 py-5">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-sm font-semibold">Finish</h2>
        {v && <Status tone={v.closed ? "ok" : "accent"} label={v.closed ? "Closed" : "Open"} />}
      </div>

      {finish.isError ? (
        <ErrorNote>{String(finish.error)}</ErrorNote>
      ) : !v ? (
        <p className="text-sm text-muted">Loading…</p>
      ) : (
        <>
          <Card title="What happens next">
            <p className="text-sm text-muted">
              {v.closed
                ? "Every device is finished with, so this project is closed. Nothing more is needed."
                : v.healthy - v.waitingForEcoverse.length > 0
                  ? `Each healthy device removes its own migration record ${v.reviewDays} days after it became healthy. This app notes it the next time the project is open (it checks every minute while the Track step is showing), and the project closes once every device has cleaned up. To finish sooner, run the cleanup below.`
                  : v.moving + v.waiting + v.needsYou > 0
                    ? "Some devices aren't done yet. Go back to Track to see what each is waiting for."
                    : "Nothing is waiting on you."}
            </p>
          </Card>

          <Card title="Where it stands">
            <dl className="grid grid-cols-[10rem_1fr] gap-x-3 gap-y-1 text-sm">
              <dt className="text-muted">Devices moved</dt>
              <dd>{v.total}</dd>
              <dt className="text-muted">Healthy</dt>
              <dd>{v.healthy - v.waitingForEcoverse.length}</dd>
              {v.waitingForEcoverse.length > 0 && (
                <>
                  <dt className="text-muted">Waiting for Ecoverse</dt>
                  <dd className="text-warn">{v.waitingForEcoverse.length}</dd>
                </>
              )}
              <dt className="text-muted">Cleaned up or done</dt>
              <dd>{v.cleanedUp + v.retired}</dd>
              <dt className="text-muted">Still moving</dt>
              <dd>{v.moving}</dd>
              <dt className="text-muted">Not launched yet</dt>
              <dd>{v.waiting}</dd>
              <dt className="text-muted">Need you</dt>
              <dd className={v.needsYou ? "text-warn" : ""}>{v.needsYou}</dd>
            </dl>
          </Card>

          <Card title="Report">
            <p className="text-sm text-muted">Every device with its stage, path, wave and history. It is saved to your Downloads folder.</p>
            <div className="flex flex-wrap items-center gap-2">
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
          </Card>

          <Card title="Clean up" aside={<span className="text-xs text-faint">{plural(v.cleanupCandidates.length, "device")} ready</span>}>
            <p className="text-sm text-muted">
              Each healthy device removes what the migration left on it (its scheduled tasks and the N-voke folder, with the status file and logs) by itself after its review period of {v.reviewDays} days. To do it sooner, run the cleanup now. It never changes either N-central server or Ecoverse.
            </p>
            {v.waitingForEcoverse.length > 0 && (
              <p className="text-sm text-warn">
                {plural(v.waitingForEcoverse.length, "device")} {v.waitingForEcoverse.length === 1 ? "is" : "are"} registered with Ecoverse but not sending asset data yet, so {v.waitingForEcoverse.length === 1 ? "it isn't" : "they aren't"} fully working and
                {v.waitingForEcoverse.length === 1 ? " is" : " are"} left out of this cleanup: {v.waitingForEcoverse.slice(0, 6).join(", ")}
                {v.waitingForEcoverse.length > 6 ? ` and ${v.waitingForEcoverse.length - 6} more` : ""}.
              </p>
            )}
            <Button size="sm" disabled={v.cleanupCandidates.length === 0} onClick={() => (cleanup.reset(), setConfirming(true))}>
              Run cleanup now…
            </Button>
          </Card>

          <RetireCard projectId={projectId} onOptions={onOptions} />
        </>
      )}

      {confirming && v && (
        <Modal
          title={cleanup.isPending ? "Cleaning up…" : `Run cleanup on ${plural(v.cleanupCandidates.length, "device")}?`}
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
                  onClick={() => {
                    setStopping(false);
                    cleanup.mutate(undefined, { onSuccess: (r) => (setConfirming(false), setCleaned(r)) });
                  }}
                >
                  Run cleanup
                </Button>
              </>
            )
          }
        >
          <ul className="max-h-56 space-y-1.5 overflow-y-auto text-sm">
            {v.cleanupCandidates.map((n) => {
              const step = steps[n];
              const tone = !cleanup.isPending || !step ? "muted" : step.state === "cleaned" ? "ok" : step.state === "failed" ? "err" : "accent";
              return (
                <li key={n}>
                  <div className="flex items-center gap-2">
                    <Dot tone={tone} pulse={step?.state === "running"} />
                    <span>{n}</span>
                    {cleanup.isPending && step && <span className="text-xs text-muted">{step.state === "running" ? "cleaning…" : step.state === "cleaned" ? "cleaned up" : "didn't clean up"}</span>}
                  </div>
                  {step?.why && <p className="pl-4 text-xs break-words text-muted">{step.why}</p>}
                </li>
              );
            })}
          </ul>
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
                <ul className="max-h-40 space-y-1 overflow-y-auto pl-4 text-muted">
                  {cleaned.failed.map((f) => (
                    <li key={f.name} className="list-disc break-words">
                      {f.name}: {f.why}
                    </li>
                  ))}
                </ul>
              </>
            )}
          </div>
        </Modal>
      )}
    </div>
  );
}
