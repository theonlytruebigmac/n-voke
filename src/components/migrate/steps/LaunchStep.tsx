import { Plus } from "lucide-react";
import { useState } from "react";
import type { WaveRow } from "../../../lib/migrate/bindings";
import { useLaunch, useLaunchActions } from "../../../lib/migrate/queries";
import { Button, ErrorNote, Field, Modal, Status, inputClass } from "../../ui";
import { LaunchDialog } from "../parts/LaunchDialog";
import type { StepId } from "../parts/StepRail";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const PILOT = 5;

function NewWaveDialog({ projectId, unassigned, defaultSize, onClose }: { projectId: string; unassigned: number; defaultSize: number; onClose: () => void }) {
  const { createWave } = useLaunchActions(projectId);
  const [label, setLabel] = useState("");
  const [size, setSize] = useState(String(Math.min(defaultSize, unassigned)));
  const count = Number(size);
  const valid = Number.isInteger(count) && count >= 1 && count <= unassigned;
  return (
    <Modal
      title="New wave"
      description={`${plural(unassigned, "planned device")} aren't in a wave yet. The wave takes the next ones in name order.`}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" busy={createWave.isPending} disabled={!valid} onClick={() => createWave.mutate({ label, max: count }, { onSuccess: onClose })}>
            Create wave
          </Button>
        </>
      }
    >
      <div className="space-y-3">
        <Field label="Name (optional)" hint="Wave 2, pilot, Friday night…">
          <input className={inputClass} value={label} onChange={(e) => setLabel(e.target.value)} placeholder="Pilot" autoFocus />
        </Field>
        <Field label="Devices" error={valid ? null : `Between 1 and ${unassigned}.`}>
          <div className="flex items-center gap-2">
            <input className={`${inputClass} w-28`} inputMode="numeric" value={size} onChange={(e) => setSize(e.target.value)} aria-label="Devices in the wave" />
            <Button size="sm" onClick={() => (setSize(String(Math.min(PILOT, unassigned))), setLabel(label || "Pilot"))}>
              Pilot ({Math.min(PILOT, unassigned)})
            </Button>
            <Button size="sm" onClick={() => setSize(String(Math.min(defaultSize, unassigned)))}>
              Wave size ({Math.min(defaultSize, unassigned)})
            </Button>
          </div>
        </Field>
        {createWave.isError && <ErrorNote>{String(createWave.error)}</ErrorNote>}
      </div>
    </Modal>
  );
}

function WaveCard({ wave, onLaunch, onTrack }: { wave: WaveRow; onLaunch: () => void; onTrack: () => void }) {
  const launched = wave.launchedAt !== null;
  return (
    <li className="flex items-center justify-between gap-3 rounded-lg border border-border px-4 py-3">
      <div className="min-w-0">
        <div className="text-sm font-medium">{wave.label}</div>
        <div className="text-xs text-muted">
          {plural(wave.devices, "device")}
          {wave.waiting > 0 && wave.waiting < wave.devices && ` · ${wave.waiting} still waiting`}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-3">
        <Status tone={launched ? "ok" : "muted"} label={launched ? "Launched" : "Waiting"} />
        {wave.waiting > 0 && (
          <Button size="sm" variant="primary" onClick={onLaunch}>
            Launch…
          </Button>
        )}
        {launched && (
          <Button size="sm" onClick={onTrack}>
            Track
          </Button>
        )}
      </div>
    </li>
  );
}

/** Waves are made here, then launched (step 5 in docs/migration/11-gui-design.md). */
export function LaunchStep({ projectId, onGo }: { projectId: string; onGo: (step: StepId) => void }) {
  const launch = useLaunch(projectId);
  const [making, setMaking] = useState(false);
  const [launching, setLaunching] = useState<number | null>(null);
  const view = launch.data;

  return (
    <div className="mx-auto max-w-2xl space-y-4 px-6 py-5">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-sm font-semibold">Launch</h2>
        <Button size="sm" disabled={!view || view.unassigned === 0} onClick={() => setMaking(true)}>
          <Plus />
          New wave
        </Button>
      </div>

      {launch.isError ? (
        <ErrorNote>{String(launch.error)}</ErrorNote>
      ) : !view ? (
        <p className="text-sm text-muted">Loading…</p>
      ) : (
        <>
          {view.waves.length === 0 ? (
            <div className="space-y-2 rounded-lg border border-border px-4 py-5 text-sm">
              {view.unassigned === 0 ? (
                <>
                  <p className="text-muted">No devices are planned yet.</p>
                  <Button size="sm" onClick={() => onGo("plan")}>
                    Go to Plan
                  </Button>
                </>
              ) : (
                <p className="text-muted">Make a first wave from the planned devices. A pilot of a few devices is a good start.</p>
              )}
            </div>
          ) : (
            <ul className="space-y-2">
              {view.waves.map((w) => (
                <WaveCard key={w.number} wave={w} onLaunch={() => setLaunching(w.number)} onTrack={() => onGo("track")} />
              ))}
            </ul>
          )}
          {view.unassigned > 0 && view.waves.length > 0 && <p className="text-sm text-muted">{plural(view.unassigned, "planned device")} aren't in a wave yet.</p>}
        </>
      )}

      {making && view && <NewWaveDialog projectId={projectId} unassigned={view.unassigned} defaultSize={view.waveSize} onClose={() => setMaking(false)} />}
      {launching !== null && (
        <LaunchDialog
          projectId={projectId}
          wave={launching}
          onClose={() => setLaunching(null)}
          onLaunched={() => {
            setLaunching(null);
            onGo("track");
          }}
        />
      )}
    </div>
  );
}
