import { ArrowLeft } from "lucide-react";
import { useEffect, useState } from "react";
import { closeProject } from "../../lib/migrate/api";
import { usePrepare, useProject, useServers } from "../../lib/migrate/queries";
import { stepStatus } from "../../lib/migrate/steps";
import { Button, ErrorNote, PageHeader } from "../ui";
import { StepRail, type StepId } from "./parts/StepRail";
import { ConnectStep } from "./steps/ConnectStep";
import { MapStep } from "./steps/MapStep";
import { FinishStep } from "./steps/FinishStep";
import { LaunchStep } from "./steps/LaunchStep";
import { PlanStep } from "./steps/PlanStep";
import { PrepareStep } from "./steps/PrepareStep";
import { TrackStep } from "./steps/TrackStep";

const AVAILABLE: ReadonlySet<StepId> = new Set<StepId>(["connect", "map", "prepare", "plan", "launch", "track", "finish"]);

/** One project: its header, the step rail, and the current step (docs/migration/11-gui-design.md). */
export function Workspace({ projectId, onBack }: { projectId: string; onBack: () => void }) {
  const project = useProject(projectId);
  const servers = useServers();
  const [chosen, setChosen] = useState<StepId | null>(null);

  // Changing a project takes its lease; let go of it when the project closes so another window can work on it.
  useEffect(() => () => void closeProject(projectId).catch(() => undefined), [projectId]);

  const name = (id: string) => servers.data?.find((s) => s.id === id)?.label ?? "a removed server";
  const p = project.data;
  const meta = p ? (p.sourceServerId === p.destServerId ? `${name(p.sourceServerId)} (reinstall)` : `${name(p.sourceServerId)} → ${name(p.destServerId)}`) : undefined;
  // A project that has launched opens at Track, the others at Connect, until the person picks a step.
  const launched = p ? Object.entries(p.stageCounts).some(([stage, count]) => count > 0 && stage !== "planned" && stage !== "excluded") : false;
  const step: StepId = chosen ?? (launched ? "track" : "connect");
  const setStep = setChosen;
  const prepare = usePrepare(projectId);
  const { marks, notes } = stepStatus(p, prepare.data);

  return (
    <>
      <PageHeader
        title={p?.name ?? "Project"}
        meta={meta}
        actions={
          <Button size="sm" variant="ghost" onClick={onBack}>
            <ArrowLeft />
            Projects
          </Button>
        }
      />
      {project.isError ? (
        <div className="p-5">
          <ErrorNote>{String(project.error)}</ErrorNote>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1">
          <StepRail current={step} marks={marks} notes={notes} available={AVAILABLE} onSelect={setStep} />
          <div className="min-w-0 flex-1 overflow-y-auto">
            {step === "connect" && <ConnectStep projectId={projectId} onContinue={() => setStep("map")} />}
            {step === "map" && <MapStep projectId={projectId} onContinue={() => setStep("prepare")} />}
            {step === "prepare" && <PrepareStep projectId={projectId} onBack={() => setStep("map")} onContinue={() => setStep("plan")} />}
            {step === "plan" && <PlanStep projectId={projectId} onGo={setStep} />}
            {step === "launch" && <LaunchStep projectId={projectId} onGo={setStep} />}
            {step === "track" && <TrackStep projectId={projectId} onContinue={() => setStep("finish")} />}
            {step === "finish" && <FinishStep projectId={projectId} onOptions={() => setStep("prepare")} />}
          </div>
        </div>
      )}
    </>
  );
}
