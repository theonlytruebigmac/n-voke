import { useEffect, useState } from "react";
import { ArrowRightLeft, MoreHorizontal, Plus, Server } from "lucide-react";
import type { ProjectSummary, ServerView } from "../../lib/migrate/bindings";
import { projectProgress } from "../../lib/migrate/progress";
import { useProjectActions, useProjects, useServers } from "../../lib/migrate/queries";
import { formatRelative, useNow } from "../../lib/format";
import { Button, EmptyState, ErrorNote, IconButton, Menu, MenuContent, MenuItem, MenuTrigger, Modal, PageHeader, Status, Table } from "../ui";
import { NewProjectDialog } from "./NewProjectDialog";
import { ServersSheet } from "./ServersSheet";
import { Workspace } from "./Workspace";

const serverName = (servers: ServerView[] | undefined, id: string) => servers?.find((s) => s.id === id)?.label ?? "a removed server";

function ProgressBar({ fraction }: { fraction: number }) {
  return (
    <div className="h-1.5 w-28 overflow-hidden rounded-full bg-surface-3" role="progressbar" aria-valuenow={Math.round(fraction * 100)} aria-valuemin={0} aria-valuemax={100}>
      <div className="h-full rounded-full bg-ok transition-[width]" style={{ width: `${Math.round(fraction * 100)}%` }} />
    </div>
  );
}

function ProjectRow({
  project,
  servers,
  now,
  onOpen,
  onDelete,
}: {
  project: ProjectSummary;
  servers: ServerView[] | undefined;
  now: number;
  onOpen: () => void;
  onDelete: () => void;
}) {
  const { total, done, fraction } = projectProgress(project);
  const source = serverName(servers, project.sourceServerId);
  const dest = serverName(servers, project.destServerId);
  return (
    <tr className="cursor-pointer hover:bg-surface-2/60" onClick={onOpen}>
      <td className="font-medium">{project.name}</td>
      <td className="text-muted">{project.sourceServerId === project.destServerId ? `${source} (reinstall)` : `${source} → ${dest}`}</td>
      <td>
        <div className="flex items-center gap-2.5">
          <ProgressBar fraction={fraction} />
          <span className="text-xs text-muted tabular-nums">
            {done}/{total}
          </span>
        </div>
      </td>
      <td>{project.closed ? <Status tone="ok" label="Closed" /> : project.attention > 0 ? <Status tone="warn" label={`${project.attention} need you`} /> : <span className="text-faint">–</span>}</td>
      <td className="text-faint">{formatRelative(project.updatedAt, now)}</td>
      <td className="w-10 text-right" onClick={(e) => e.stopPropagation()}>
        <Menu>
          <MenuTrigger asChild>
            <IconButton label={`More for ${project.name}`}>
              <MoreHorizontal />
            </IconButton>
          </MenuTrigger>
          <MenuContent>
            <MenuItem onSelect={onOpen}>Open</MenuItem>
            <MenuItem onSelect={onDelete} danger>
              Delete…
            </MenuItem>
          </MenuContent>
        </Menu>
      </td>
    </tr>
  );
}

/** The Migrate page: the project list (screen A in docs/migration/11-gui-design.md), and a project once one is opened. */
export function MigratePage({ ecoverseReady }: { ecoverseReady: boolean }) {
  const projects = useProjects();
  const servers = useServers();
  const { remove } = useProjectActions();
  const now = useNow(30_000);
  const [openId, setOpenId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [showServers, setShowServers] = useState(false);
  const [deleting, setDeleting] = useState<ProjectSummary | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const list = projects.data ?? [];

  // The list stays mounted while a project is open, so read it again when coming back to it.
  const refetchProjects = projects.refetch;
  useEffect(() => {
    if (openId === null) void refetchProjects();
  }, [openId, refetchProjects]);

  const overlays = (
    <>
      {showServers && <ServersSheet onClose={() => setShowServers(false)} />}
      {creating && (
        <NewProjectDialog
          ecoverseReady={ecoverseReady}
          onClose={() => setCreating(false)}
          onCreated={(p) => {
            setCreating(false);
            setOpenId(p.id);
          }}
          onManageServers={() => setShowServers(true)}
        />
      )}
      {deleting && (
        <Modal
          title={`Delete ${deleting.name}?`}
          description="This removes the project's local history. The audit log keeps its entries. Nothing on the servers or devices changes."
          onClose={() => setDeleting(null)}
          footer={
            <>
              <Button onClick={() => setDeleting(null)}>Cancel</Button>
              <Button
                variant="destructive"
                busy={remove.isPending}
                onClick={() =>
                  remove.mutate(deleting.id, {
                    onSuccess: () => setDeleting(null),
                    onError: (e) => setDeleteError(String(e)),
                  })
                }
              >
                Delete
              </Button>
            </>
          }
        >
          {deleteError && <ErrorNote>{deleteError}</ErrorNote>}
        </Modal>
      )}
    </>
  );

  if (openId) {
    return (
      <>
        <Workspace projectId={openId} onBack={() => setOpenId(null)} />
        {overlays}
      </>
    );
  }

  return (
    <>
      <PageHeader
        title="Migrate"
        meta={list.length > 0 ? `${list.length} ${list.length === 1 ? "project" : "projects"}` : undefined}
        actions={
          <>
            <Button size="sm" onClick={() => setShowServers(true)}>
              <Server />
              Manage servers
            </Button>
            <Button
              size="sm"
              variant="primary"
              onClick={() => {
                setDeleteError(null);
                setCreating(true);
              }}
            >
              <Plus />
              New project
            </Button>
          </>
        }
      />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {projects.isError ? (
          <div className="mx-auto max-w-4xl px-6 py-5">
            <ErrorNote>{String(projects.error)}</ErrorNote>
          </div>
        ) : projects.isPending ? (
          <p className="px-6 py-5 text-sm text-muted">Loading projects…</p>
        ) : list.length === 0 ? (
          <EmptyState
            icon={<ArrowRightLeft />}
            title="No migrations yet"
            action={
              <Button variant="primary" onClick={() => setCreating(true)}>
                <Plus />
                New project
              </Button>
            }
          >
            A project moves devices from one N-central server to another, or reinstalls them on the same one.
          </EmptyState>
        ) : (
          <div className="mx-auto max-w-4xl px-6 py-5">
            <Table
              head={
                <>
                  <th>Name</th>
                  <th>From → To</th>
                  <th>Progress</th>
                  <th>Needs you</th>
                  <th>Last change</th>
                  <th />
                </>
              }
            >
              {list.map((p) => (
                <ProjectRow
                  key={p.id}
                  project={p}
                  servers={servers.data}
                  now={now}
                  onOpen={() => setOpenId(p.id)}
                  onDelete={() => {
                    setDeleteError(null);
                    setDeleting(p);
                  }}
                />
              ))}
            </Table>
          </div>
        )}
      </div>
      {overlays}
    </>
  );
}
