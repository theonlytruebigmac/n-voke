import { useState } from "react";
import type { RetireReport } from "../../../lib/migrate/bindings";
import { useRetire, useRetireActions } from "../../../lib/migrate/queries";
import { Button, ErrorNote, Modal, Status } from "../../ui";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Deleting the moved devices' old records from the source server, after the review period and behind a confirmation. */
export function RetireCard({ projectId, onOptions }: { projectId: string; onOptions: () => void }) {
  const retire = useRetire(projectId);
  const run = useRetireActions(projectId);
  const [confirming, setConfirming] = useState<null | "ready" | "early">(null);
  const [dryRun, setDryRun] = useState(false);
  const [acknowledged, setAcknowledged] = useState(false);
  const [result, setResult] = useState<RetireReport | null>(null);
  const v = retire.data;
  const early = confirming === "early";
  const count = v ? v.ready.length + (early ? v.waiting.length : 0) : 0;
  const open = (mode: "ready" | "early") => (run.reset(), setDryRun(false), setAcknowledged(false), setConfirming(mode));

  const aside = !v ? null : v.turnedOn ? <span className="text-xs text-faint">{plural(v.ready.length, "record")} ready</span> : <Status tone="muted" label="Off" />;

  return (
    <section className="space-y-3 rounded-lg border border-border p-4">
      <div className="flex items-center justify-between gap-3">
        <h3 className="text-sm font-semibold">Old source records</h3>
        {aside}
      </div>
      {retire.isError ? (
        <ErrorNote>{String(retire.error)}</ErrorNote>
      ) : !v ? (
        <p className="text-sm text-muted">Loading…</p>
      ) : !v.turnedOn ? (
        <>
          <p className="text-sm text-muted">
            The moved devices' old records stay on {v.sourceHost} until you remove them. This project can retire them for you once a device has been cleaned up and its review period of {v.reviewDays} days is over. Only the record goes: nothing is sent to the device, and the stale Ecoverse asset for it
            disappears by itself.
          </p>
          <Button size="sm" onClick={onOptions}>
            Turn it on in Prepare
          </Button>
        </>
      ) : (
        <>
          <p className="text-sm text-muted">
            Deletes the old record of each cleaned-up device from {v.sourceHost}, once its review period of {v.reviewDays} days is over. Only the record goes: nothing is sent to the device. Devices that haven't been cleaned up yet are left alone.
          </p>
          <p className="text-sm text-muted">
            {plural(v.ready.length, "device")} ready
            {v.inReview > 0 && ` · ${v.inReview} still in the review period${v.nextReviewEnds ? ` (the first ends ${new Date(v.nextReviewEnds).toLocaleDateString()})` : ""}`}
            {v.notCleanedUp > 0 && ` · ${v.notCleanedUp} healthy but not cleaned up`}
          </p>
          <div className="flex flex-wrap gap-2">
            <Button size="sm" disabled={v.ready.length === 0} onClick={() => open("ready")}>
              Retire old records…
            </Button>
            {v.waiting.length > 0 && (
              <Button size="sm" variant="ghost" onClick={() => open("early")}>
                Retire before the review period ends…
              </Button>
            )}
          </div>
          {v.waiting.length > 0 && <p className="text-xs text-faint">The review period is there so you can check each moved device first. You can skip it, with a warning, for example to test retiring.</p>}
        </>
      )}

      {confirming && v && (
        <Modal
          title={run.isPending ? "Retiring…" : early ? `Delete ${plural(count, "old record")} from ${v.sourceHost} before the review period ends?` : `Delete ${plural(count, "old record")} from ${v.sourceHost}?`}
          description="The device records are deleted from the source server. Nothing is sent to the devices, and this can't be undone."
          dismissible={!run.isPending}
          onClose={() => setConfirming(null)}
          footer={
            <>
              <Button disabled={run.isPending} onClick={() => setConfirming(null)}>
                Cancel
              </Button>
              <Button
                variant="destructive"
                busy={run.isPending}
                disabled={early && !dryRun && !acknowledged}
                onClick={() =>
                  run.mutate(
                    { dryRun, beforeReviewEnds: early },
                    {
                      onSuccess: (r) => {
                        setConfirming(null);
                        setResult(r);
                      },
                    },
                  )
                }
              >
                {dryRun ? "Show what would go" : "Delete the records"}
              </Button>
            </>
          }
        >
          <div className="space-y-3 text-sm">
            {early && (
              <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">
                This skips the review period. The {v.reviewDays}-day review period is the time you have to check that each moved device works before its old record goes.{" "}
                {plural(v.waiting.length, "device")} {v.waiting.length === 1 ? "is" : "are"} still inside it{v.nextReviewEnds ? ` (the first ends ${new Date(v.nextReviewEnds).toLocaleDateString()})` : ""}. Once the records are deleted they can't be brought back.
              </p>
            )}
            <ul className="max-h-48 list-disc space-y-0.5 overflow-y-auto pl-5">
              {v.ready.map((n) => (
                <li key={n}>{n}</li>
              ))}
              {early &&
                v.waiting.map((w) => (
                  <li key={w.name} className="text-warn">
                    {w.name}
                    {w.reviewEnds ? ` (review ends ${new Date(w.reviewEnds).toLocaleDateString()})` : ""}
                  </li>
                ))}
            </ul>
            {early && (
              <label className="flex cursor-pointer items-start gap-2.5">
                <input type="checkbox" className="mt-1" checked={acknowledged} onChange={(e) => setAcknowledged(e.target.checked)} />
                <span>I understand this skips the review period for {plural(v.waiting.length, "device")}.</span>
              </label>
            )}
            <label className="flex cursor-pointer items-start gap-2.5">
              <input type="checkbox" className="mt-1" checked={dryRun} onChange={(e) => setDryRun(e.target.checked)} />
              <span>
                Dry run: list what would be deleted, change nothing
              </span>
            </label>
            {run.isError && <ErrorNote>{String(run.error)}</ErrorNote>}
          </div>
        </Modal>
      )}

      {result && (
        <Modal title={result.wouldRemove.length > 0 ? "Dry run" : "Retiring finished"} onClose={() => setResult(null)} footer={<Button onClick={() => setResult(null)}>Done</Button>}>
          <div className="space-y-2 text-sm">
            {result.wouldRemove.length > 0 && (
              <>
                <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">Dry run: nothing was deleted.</p>
                <p>These records would have been deleted:</p>
                <ul className="max-h-40 list-disc overflow-y-auto pl-5 text-muted">
                  {result.wouldRemove.map((n) => (
                    <li key={n}>{n}</li>
                  ))}
                </ul>
              </>
            )}
            {result.retired > 0 && <p className="text-ok">{plural(result.retired, "record")} deleted</p>}
            {result.alreadyGone > 0 && <p className="text-muted">{plural(result.alreadyGone, "record")} had already gone from the server</p>}
            {result.failed.length > 0 && (
              <>
                <p className="text-err">{plural(result.failed.length, "record")} couldn't be deleted</p>
                <ul className="max-h-40 space-y-1 overflow-y-auto pl-4 text-muted">
                  {result.failed.map((f) => (
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
    </section>
  );
}
