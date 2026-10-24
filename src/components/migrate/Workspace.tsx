import { useEffect, useState } from "react";
import { closeProject } from "../../lib/migrate/api";
import { usePrepare, useProject, useServers } from "../../lib/migrate/queries";
import { STEPS, stepStatus } from "../../lib/migrate/steps";
import { Crumb, ErrorNote, PageBody, PageHeader } from "../ui";
import { HeaderActionsSlot } from "./HeaderActions";
import { Overview } from "./Overview";
import { SetupChips, isSetupStep, type StepId } from "./parts/StepRail";
import { ConnectStep } from "./steps/ConnectStep";
import { MapStep } from "./steps/MapStep";
import { FinishStep } from "./steps/FinishStep";
import { LaunchStep } from "./steps/LaunchStep";
import { PlanStep } from "./steps/PlanStep";
import { PrepareStep } from "./steps/PrepareStep";
import { TrackStep } from "./steps/TrackStep";

type View = "overview" | StepId;

/**
 * One project (docs/migration/11-gui-design.md). It opens on the overview: the phases, the next thing to do, what
 * needs the person and where the devices are. Each step is a page of its own under the same header, reached from the
 * overview, from the set-up chips, and back with the breadcrumb. Steps render only their body; the header and the
 * scrolling area at the page width are here.
 */
export function Workspace({ projectId, onBack }: { projectId: string; onBack: () => void }) {
  const project = useProject(projectId);
  const servers = useServers();
  const [view, setView] = useState<View>("overview");
  const [slot, setSlot] = useState<HTMLElement | null>(null);

  // Changing a project takes its lease; let go of it when the project closes so another window can work on it.
  useEffect(() => () => void closeProject(projectId).catch(() => undefined), [projectId]);

  const name = (id: string) => servers.data?.find((s) => s.id === id)?.label ?? "a removed server";
  const p = project.data;
  const dest = p ? name(p.destServerId) : "the destination";
  const route = p ? (p.sourceServerId === p.destServerId ? `${name(p.sourceServerId)} (reinstall)` : `${name(p.sourceServerId)} → ${dest}`) : undefined;
  const prepare = usePrepare(projectId);
  const status = stepStatus(p, prepare.data);
  const setStep = (s: StepId) => setView(s);

  const crumbs =
    view === "overview" ? (
      <Crumb onClick={onBack}>Projects</Crumb>
    ) : (
      <>
        <Crumb onClick={onBack}>Projects</Crumb>
        <Crumb onClick={() => setView("overview")}>{p?.name ?? "Project"}</Crumb>
        {isSetupStep(view) && (
          <>
            <span>Set up</span>
            <span className="text-border-strong" aria-hidden="true">
              /
            </span>
          </>
        )}
      </>
    );

  const devices = p ? p.devices - (p.stageCounts.excluded ?? 0) : 0;
  const meta =
    view === "overview"
      ? [route, p && devices > 0 ? `${devices.toLocaleString()} device${devices === 1 ? "" : "s"}` : null].filter(Boolean).join(" · ")
      : STEP_META[view](dest);

  return (
    <>
      <PageHeader
        crumbs={crumbs}
        title={view === "overview" ? (p?.name ?? "Project") : STEPS.find((s) => s.id === view)?.label}
        meta={meta}
        actions={
          view !== "overview" && isSetupStep(view) ? (
            <SetupChips current={view} marks={status.marks} notes={status.notes} onSelect={setStep} />
          ) : (
            // Launch, Track and Finish put their own buttons here (HeaderActions).
            <div ref={setSlot} className="flex flex-wrap items-center gap-2 empty:hidden" />
          )
        }
      />
      <HeaderActionsSlot.Provider value={slot}>
        <PageBody>
          {project.isError ? (
            <ErrorNote>{String(project.error)}</ErrorNote>
          ) : !p ? (
            <p className="text-sm text-muted">Loading the project…</p>
          ) : (
            <>
              {view === "overview" && <Overview projectId={projectId} project={p} status={status} onGo={setStep} />}
              {view === "connect" && <ConnectStep projectId={projectId} onContinue={() => setStep("map")} />}
              {view === "map" && <MapStep projectId={projectId} onContinue={() => setStep("prepare")} />}
              {view === "prepare" && <PrepareStep projectId={projectId} onBack={() => setStep("map")} onContinue={() => setStep("plan")} />}
              {view === "plan" && <PlanStep projectId={projectId} onGo={setStep} />}
              {view === "launch" && <LaunchStep projectId={projectId} onGo={setStep} />}
              {view === "track" && <TrackStep projectId={projectId} onContinue={() => setStep("finish")} onPrepare={() => setStep("prepare")} />}
              {view === "finish" && <FinishStep projectId={projectId} onOptions={() => setStep("prepare")} />}
            </>
          )}
        </PageBody>
      </HeaderActionsSlot.Provider>
    </>
  );
}

/** The line under each step's title: what the step is for. */
const STEP_META: Record<StepId, (dest: string) => string> = {
  connect: () => "Checks that both servers and Ecoverse can do what this project needs.",
  map: (dest) => `Choose where each customer and site goes on ${dest}. Matching names are paired for you.`,
  prepare: () => "Put the endpoint script where devices can get it, and choose how the move behaves.",
  plan: () => "Choose which devices this project moves, and how each is reached.",
  launch: () => "Send the devices in waves. You confirm each wave before old agents are removed.",
  track: (dest) => `Follow every device until it is working on ${dest}.`,
  finish: () => "Clean up, retire the old records and save the report.",
};
