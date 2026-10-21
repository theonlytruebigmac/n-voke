import { Plus } from "lucide-react";
import { useState } from "react";
import type { WaveRow } from "../../../lib/migrate/bindings";
import { useLaunch, useLaunchActions, useCandidates } from "../../../lib/migrate/queries";
import { useNow } from "../../../lib/format";
import { ago } from "../../../lib/migrate/track";
import { useDebounced } from "../../../lib/useDebounced";
import { Button, ErrorNote, Field, Modal, Segmented, Status, inputClass } from "../../ui";
import { LaunchDialog } from "../parts/LaunchDialog";
import type { StepId } from "../parts/StepRail";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const PILOT = 5;

const CANDIDATES = 50;

function NewWaveDialog({ projectId, unassigned, defaultSize, onClose }: { projectId: string; unassigned: number; defaultSize: number; onClose: () => void }) {
  const { createWave, createWaveFrom } = useLaunchActions(projectId);
  const [how, setHow] = useState<"count" | "choose">("count");
  const [label, setLabel] = useState("");
  const [size, setSize] = useState(String(Math.min(defaultSize, unassigned)));
  const [text, setText] = useState("");
  const [offset, setOffset] = useState(0);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const search = useDebounced(text.trim());
  const candidates = useCandidates(projectId, { text: search, offset, limit: CANDIDATES }, how === "choose");
  const count = Number(size);
  const valid = Number.isInteger(count) && count >= 1 && count <= unassigned;
  const page = candidates.data;
  const toggle = (id: string) =>
    setPicked((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const pickPage = () =>
    setPicked((current) => {
      const next = new Set(current);
      for (const r of page?.rows ?? []) next.add(r.deviceId);
      return next;
    });
  const pending = createWave.isPending || createWaveFrom.isPending;
  const failure = createWave.error ?? createWaveFrom.error;
  const choosing = how === "choose";

  return (
    <Modal
      title="New wave"
      size={choosing ? "lg" : "md"}
      description={
        choosing
          ? `${plural(unassigned, "planned device")} aren't in a wave yet. Tick the ones this wave should take.`
          : `${plural(unassigned, "planned device")} aren't in a wave yet. The wave takes the next ones in name order.`
      }
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          {choosing ? (
            <Button variant="primary" busy={pending} disabled={picked.size === 0} onClick={() => createWaveFrom.mutate({ label, deviceIds: [...picked] }, { onSuccess: onClose })}>
              {picked.size > 0 ? `Create wave with ${plural(picked.size, "device")}` : "Create wave"}
            </Button>
          ) : (
            <Button variant="primary" busy={pending} disabled={!valid} onClick={() => createWave.mutate({ label, max: count }, { onSuccess: onClose })}>
              Create wave
            </Button>
          )}
        </>
      }
    >
      <div className="space-y-3">
        <Segmented
          value={how}
          onChange={setHow}
          options={[
            { value: "count", label: "By number" },
            { value: "choose", label: "Choose devices" },
          ]}
        />
        <Field label="Name (optional)" hint="Wave 2, pilot, Friday night…">
          <input className={inputClass} value={label} onChange={(e) => setLabel(e.target.value)} placeholder="Pilot" autoFocus />
        </Field>
        {!choosing ? (
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
        ) : (
          <div className="space-y-2">
            <input
              className={inputClass}
              placeholder="Search by device or customer"
              value={text}
              onChange={(e) => (setText(e.target.value), setOffset(0))}
              aria-label="Search the planned devices"
            />
            {candidates.isError ? (
              <ErrorNote>{String(candidates.error)}</ErrorNote>
            ) : !page ? (
              <p className="text-sm text-muted">Loading…</p>
            ) : page.rows.length === 0 ? (
              <p className="text-sm text-muted">{page.waiting === 0 ? "Every planned device is already in a wave." : "No planned device matches."}</p>
            ) : (
              <>
                <div className="max-h-72 overflow-y-auto rounded-lg border border-border">
                  <table className="w-full border-collapse text-sm [&_td]:px-3 [&_td]:py-1.5">
                    <tbody className="[&>tr]:border-t [&>tr]:border-border first:[&>tr]:border-t-0">
                      {page.rows.map((r) => (
                        <tr key={r.deviceId} className="cursor-pointer hover:bg-surface-2/60" onClick={() => toggle(r.deviceId)}>
                          <td className="w-8">
                            <input type="checkbox" checked={picked.has(r.deviceId)} onChange={() => toggle(r.deviceId)} onClick={(e) => e.stopPropagation()} aria-label={`Take ${r.name}`} />
                          </td>
                          <td className="font-medium">{r.name}</td>
                          <td className="text-muted">{r.customer}</td>
                          <td className="text-faint">{r.path === "ecoverse" ? "Ecoverse" : r.path === "rest" ? "N-central" : "–"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted">
                  <span>
                    {plural(page.matching, "device")} match
                    {picked.size > 0 && ` · ${picked.size} ticked`}
                  </span>
                  <span className="flex items-center gap-2">
                    <Button size="sm" onClick={pickPage}>
                      Tick this page
                    </Button>
                    {picked.size > 0 && (
                      <Button size="sm" onClick={() => setPicked(new Set())}>
                        Clear
                      </Button>
                    )}
                    <Button size="sm" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - CANDIDATES))}>
                      Previous
                    </Button>
                    <Button size="sm" disabled={offset + CANDIDATES >= page.matching} onClick={() => setOffset(offset + CANDIDATES)}>
                      Next
                    </Button>
                  </span>
                </div>
              </>
            )}
          </div>
        )}
        {failure != null && <ErrorNote>{String(failure)}</ErrorNote>}
      </div>
    </Modal>
  );
}

function WaveCard({ wave, onLaunch, onTrack }: { wave: WaveRow; onLaunch: () => void; onTrack: () => void }) {
  const launched = wave.launchedAt !== null;
  const now = useNow(60_000);
  const parts = [
    wave.working > 0 && { text: `${wave.working} working`, tone: "text-ok" },
    wave.moving > 0 && { text: `${wave.moving} moving`, tone: "text-muted" },
    wave.needsYou > 0 && { text: `${wave.needsYou} need you`, tone: "text-warn" },
    wave.waiting > 0 && launched && { text: `${wave.waiting} still waiting`, tone: "text-muted" },
  ].filter(Boolean) as { text: string; tone: string }[];
  return (
    <li className="flex items-center justify-between gap-3 rounded-lg border border-border px-4 py-3">
      <div className="min-w-0">
        <div className="text-sm font-medium">{wave.label}</div>
        <div className="text-xs text-muted">
          {plural(wave.devices, "device")}
          {launched && wave.launchedAt && ` · launched ${ago(wave.launchedAt, now)} ago`}
        </div>
        {launched && parts.length > 0 && (
          <div className="mt-0.5 flex flex-wrap gap-x-3 text-xs">
            {parts.map((p) => (
              <span key={p.text} className={p.tone}>
                {p.text}
              </span>
            ))}
          </div>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-3">
        <Status tone={launched ? (wave.waiting > 0 ? "accent" : "ok") : "muted"} label={launched ? (wave.waiting > 0 ? "Partly launched" : "Launched") : "Waiting"} />
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
