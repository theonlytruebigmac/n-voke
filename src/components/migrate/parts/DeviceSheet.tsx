import { useState } from "react";
import type { DeviceAction, DeviceCommand, DeviceDetail } from "../../../lib/migrate/bindings";
import { ACTION_WORDS, ago } from "../../../lib/migrate/track";
import { useDevice, useDeviceActions } from "../../../lib/migrate/queries";
import { describeStage, ecoWords, stageView } from "../../../lib/migrate/words";
import { Button, ErrorNote, Modal, Sheet, Status } from "../../ui";
import { useNow } from "../../../lib/format";

const PATH_WORDS = { ecoverse: "Ecoverse", rest: "N-central" } as const;

function Timeline({ events }: { events: DeviceDetail["events"] }) {
  return (
    <ol className="space-y-2">
      {[...events].reverse().map((e, i) => (
        <li key={`${e.at}-${i}`} className="grid grid-cols-[7.5rem_1fr] gap-3 text-xs">
          <span className="text-faint tabular-nums">{new Date(e.at).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}</span>
          <span className="min-w-0">
            <span className="font-medium">{e.kind}</span>
            {e.detail && <pre className="mt-0.5 max-h-32 overflow-auto font-sans break-words whitespace-pre-wrap text-muted">{e.detail}</pre>}
          </span>
        </li>
      ))}
    </ol>
  );
}

/** One device: where it is, its timeline, and what can be done for it (the device sheet in docs/migration/11-gui-design.md). */
export function DeviceSheet({ projectId, index, onClose }: { projectId: string; index: number; onClose: () => void }) {
  const detail = useDevice(projectId, index);
  const { command } = useDeviceActions(projectId);
  const now = useNow();
  const [confirming, setConfirming] = useState<{ action: DeviceAction; command: DeviceCommand } | null>(null);
  const [checked, setChecked] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const d = detail.data;

  const run = (cmd: DeviceCommand) => {
    setMessage(null);
    command.mutate({ index, command: cmd }, { onSuccess: setMessage, onSettled: () => setConfirming(null) });
  };

  const ask = (action: DeviceAction, d: DeviceDetail) => {
    const cmd: DeviceCommand | null =
      action === "probe"
        ? { kind: "probe" }
        : action === "cleanup"
          ? { kind: "cleanup" }
          : action === "retry"
            ? { kind: "retry", otherPath: false, checked: false }
            : action === "retryOtherPath"
              ? { kind: "retry", otherPath: true, checked: false }
              : action === "markResolved"
                ? { kind: "resolve" }
                : null;
    if (!cmd) return;
    if (ACTION_WORDS[action].confirm === null && !d.launchUnsure) return run(cmd);
    setChecked(false);
    setConfirming({ action, command: cmd });
  };

  return (
    <Sheet title={d?.name ?? "Device"} subtitle={d ? `${d.customer} → ${d.destination ?? "no destination"}` : undefined} onClose={onClose}>
      {detail.isError ? (
        <ErrorNote>{String(detail.error)}</ErrorNote>
      ) : !d ? (
        <p className="text-sm text-muted">Loading…</p>
      ) : (
        <div className="space-y-5">
          <div className="space-y-1">
            <Status tone={stageView(d.stage, d.waitingForEcoverse).tone} pulse={stageView(d.stage, d.waitingForEcoverse).pulse} label={stageView(d.stage, d.waitingForEcoverse).short} />
            <p className="text-sm text-muted">{describeStage(d.stage, { since: ago(d.since, now), destination: d.destination, reason: d.lastError, waitingForEcoverse: d.waitingForEcoverse })}</p>
            <p className="text-xs text-faint">
              {d.path ? PATH_WORDS[d.path] : "No path"}
              {d.wave !== null && ` · wave ${d.wave}`}
              {d.attempts > 1 && ` · ${d.attempts} attempts`}
            </p>
            {d.launchUnsure && <p className="text-xs text-warn">A launch for this device was sent and its answer lost, so it may be running. Look in Activity (Ecoverse) or the task list (N-central) before retrying.</p>}
          </div>

          {(d.ecoverse !== null || d.waitingForEcoverse) && (
            <div className={`space-y-1 rounded-lg border p-3 ${d.waitingForEcoverse ? "border-warn/40" : "border-border"}`}>
              <Status tone={ecoWords(d.ecoverse).tone} label={ecoWords(d.ecoverse).short} />
              <p className="text-xs text-muted">{ecoWords(d.ecoverse).long}</p>
            </div>
          )}

          <dl className="grid grid-cols-[7rem_1fr] gap-x-3 gap-y-1 text-xs">
            <dt className="text-muted">Old device</dt>
            <dd className="tabular-nums">{d.sourceDeviceId}</dd>
            <dt className="text-muted">New device</dt>
            <dd className="tabular-nums">{d.destDeviceId ?? "not found yet"}</dd>
            {d.assetId && (
              <>
                <dt className="text-muted">Ecoverse asset</dt>
                <dd className="break-all">{d.assetId}</dd>
              </>
            )}
          </dl>

          {d.candidates.length > 0 && (
            <div className="space-y-2 rounded-lg border border-warn/40 p-3">
              <p className="text-sm">More than one device on the new server fits this one. Which is it?</p>
              {d.candidates.map((c) => (
                <div key={c.deviceId} className="flex items-center justify-between gap-3 text-sm">
                  <span>
                    {c.name} <span className="text-xs text-faint">{c.deviceId}</span>
                  </span>
                  <Button size="sm" busy={command.isPending} onClick={() => run({ kind: "chooseMatch", destDeviceId: c.deviceId })}>
                    This one
                  </Button>
                </div>
              ))}
            </div>
          )}

          {d.actions.filter((a) => a !== "chooseMatch").length > 0 && (
            <div className="flex flex-wrap gap-2">
              {d.actions
                .filter((a) => a !== "chooseMatch")
                .map((a) => (
                  <Button key={a} size="sm" variant={a === "markResolved" ? "ghost" : undefined} busy={command.isPending && confirming === null} onClick={() => ask(a, d)}>
                    {ACTION_WORDS[a].label}
                  </Button>
                ))}
            </div>
          )}
          {message && <p className="text-sm break-words text-ok">{message}</p>}
          {command.isError && !confirming && <ErrorNote>{String(command.error)}</ErrorNote>}

          <div>
            <h3 className="mb-2 text-sm font-semibold">Timeline</h3>
            <Timeline events={d.events} />
          </div>
        </div>
      )}

      {confirming && d && (
        <Modal
          title={ACTION_WORDS[confirming.action].label}
          description={ACTION_WORDS[confirming.action].confirm ?? undefined}
          onClose={() => setConfirming(null)}
          footer={
            <>
              <Button onClick={() => setConfirming(null)}>Cancel</Button>
              <Button
                variant="primary"
                busy={command.isPending}
                disabled={d.launchUnsure && confirming.command.kind === "retry" && !checked}
                onClick={() => run(confirming.command.kind === "retry" ? { ...confirming.command, checked } : confirming.command)}
              >
                {ACTION_WORDS[confirming.action].label}
              </Button>
            </>
          }
        >
          {d.waitingForEcoverse && confirming.command.kind === "cleanup" && (
            <p className="mb-3 rounded-md bg-warn-soft px-3 py-2 text-sm text-warn">
              Ecoverse isn't receiving this device's data yet. Cleanup removes the migration's record on the device, which is what you'd look at if it never does.
            </p>
          )}
          {d.launchUnsure && confirming.command.kind === "retry" && (
            <label className="flex cursor-pointer items-start gap-2.5 text-sm">
              <input type="checkbox" className="mt-1" checked={checked} onChange={(e) => setChecked(e.target.checked)} />
              <span>I looked, and the earlier launch did not run.</span>
            </label>
          )}
          {command.isError && <ErrorNote>{String(command.error)}</ErrorNote>}
        </Modal>
      )}
    </Sheet>
  );
}
