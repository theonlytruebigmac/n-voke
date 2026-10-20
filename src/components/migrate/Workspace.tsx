import { ArrowLeft } from "lucide-react";
import { useState } from "react";
import { useProject, useServers } from "../../lib/migrate/queries";
import { Button, ErrorNote, PageHeader } from "../ui";
import { StepRail, type StepId, type StepMark } from "./parts/StepRail";
import { ConnectStep } from "./steps/ConnectStep";

// Only Connect exists so far; the rail shows the other steps so the order is visible, and opens them as they are built.
const AVAILABLE: ReadonlySet<StepId> = new Set<StepId>(["connect"]);

/** One project: its header, the step rail, and the current step (docs/migration/11-gui-design.md). */
export function Workspace({ projectId, onBack }: { projectId: string; onBack: () => void }) {
  const project = useProject(projectId);
  const servers = useServers();
  const [step, setStep] = useState<StepId>("connect");

  const name = (id: string) => servers.data?.find((s) => s.id === id)?.label ?? "a removed server";
  const p = project.data;
  const meta = p ? (p.sourceServerId === p.destServerId ? `${name(p.sourceServerId)} (reinstall)` : `${name(p.sourceServerId)} → ${name(p.destServerId)}`) : undefined;
  const marks: Partial<Record<StepId, StepMark>> = { connect: "current" };

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
          <StepRail current={step} marks={marks} available={AVAILABLE} onSelect={setStep} />
          <div className="min-w-0 flex-1 overflow-y-auto">{step === "connect" && <ConnectStep projectId={projectId} />}</div>
        </div>
      )}
    </>
  );
}
