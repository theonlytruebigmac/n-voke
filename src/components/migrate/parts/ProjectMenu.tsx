import { useState } from "react";
import { ExternalLink, Loader2, MoreHorizontal, RotateCcw, Trash2, XCircle } from "lucide-react";
import type { ProjectSummary } from "../../../lib/migrate/bindings";
import { closeProject, openReport } from "../../../lib/migrate/api";
import { useFinishActions, useSetClosed } from "../../../lib/migrate/queries";
import { Button, ErrorNote, IconButton, Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger, Modal } from "../../ui";
import { CloseProjectDialog } from "./CloseProjectDialog";

/**
 * A project's own menu, on the project list and in the project's header: the report, closing or reopening, and on the
 * list opening and deleting. From the list the actions take the project's lease like an open project does, so they let
 * go of it afterwards for other windows; the project's own page (`holdsLease`) keeps it.
 */
export function ProjectMenu({ project, onOpen, onDelete, holdsLease = false, bordered = false }: { project: ProjectSummary; onOpen?: () => void; onDelete?: () => void; holdsLease?: boolean; bordered?: boolean }) {
  const { report } = useFinishActions(project.id);
  const setClosed = useSetClosed(project.id);
  const [closing, setClosing] = useState(false);
  const [reporting, setReporting] = useState(false);
  const [problem, setProblem] = useState<{ title: string; text: string } | null>(null);
  const release = () => {
    if (!holdsLease) void closeProject(project.id).catch(() => undefined);
  };

  /** Builds the report now (reading an open project's devices first), saves it and opens it in the browser. */
  const saveReport = async () => {
    setProblem(null);
    setReporting(true);
    try {
      await openReport(await report.mutateAsync("html"));
    } catch (e) {
      setProblem({ title: `Report for ${project.name}`, text: String(e) });
    } finally {
      setReporting(false);
      release();
    }
  };
  const reopen = () => {
    setProblem(null);
    setClosed.mutate(false, { onError: (e) => setProblem({ title: `Couldn't reopen ${project.name}`, text: String(e) }), onSettled: release });
  };

  return (
    <>
      <Menu>
        <MenuTrigger asChild>
          <IconButton label={`More for ${project.name}`} className={bordered ? "size-8 border border-control-border bg-control text-text hover:bg-control-hover" : ""}>
            <MoreHorizontal />
          </IconButton>
        </MenuTrigger>
        <MenuContent>
          {onOpen && <MenuItem onSelect={onOpen}>Open</MenuItem>}
          <MenuItem onSelect={() => void saveReport()}>
            <ExternalLink />
            Save and open the report
          </MenuItem>
          {project.closed ? (
            <MenuItem onSelect={reopen}>
              <RotateCcw />
              Reopen
            </MenuItem>
          ) : (
            project.devices > 0 && (
              <MenuItem onSelect={() => setClosing(true)}>
                <XCircle />
                Close project…
              </MenuItem>
            )
          )}
          {onDelete && (
            <>
              <MenuSeparator />
              <MenuItem onSelect={onDelete} danger>
                <Trash2 />
                Delete…
              </MenuItem>
            </>
          )}
        </MenuContent>
      </Menu>

      {closing && <CloseProjectDialog projectId={project.id} name={project.name} onClose={() => (release(), setClosing(false))} />}
      {reporting && (
        <Modal title={`Report for ${project.name}`} dismissible={false} onClose={() => undefined}>
          <p className="flex items-center gap-2 text-sm text-muted">
            <Loader2 className="size-4 shrink-0 animate-spin" />
            {project.closed ? "Building the report…" : "Reading where each device is now, then building the report. On a large project this takes a moment."}
          </p>
        </Modal>
      )}
      {problem && (
        <Modal title={problem.title} onClose={() => setProblem(null)} footer={<Button onClick={() => setProblem(null)}>Close</Button>}>
          <ErrorNote>{problem.text}</ErrorNote>
        </Modal>
      )}
    </>
  );
}
