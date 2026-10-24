import { useEffect, useMemo, useState } from "react";
import { ArrowRightLeft, MoreHorizontal, Plus, Search } from "lucide-react";
import type { ProjectSummary, ServerView } from "../../lib/migrate/bindings";
import { projectProgress } from "../../lib/migrate/progress";
import { useProjectActions, useProjects, useServers } from "../../lib/migrate/queries";
import { formatRelative, useNow } from "../../lib/format";
import { Button, Card, Dot, EmptyState, ErrorNote, IconButton, Menu, MenuContent, MenuItem, MenuTrigger, Modal, PageBody, PageHeader, Segmented, Table, inputClass } from "../ui";
import { NewProjectDialog } from "./NewProjectDialog";
import type { SettingsSection } from "../settings/SettingsPage";
import { ServersSheet } from "./ServersSheet";
import { Workspace } from "./Workspace";

const serverName = (servers: ServerView[] | undefined, id: string) => servers?.find((s) => s.id === id)?.label ?? "a removed server";

function ProgressBar({ fraction }: { fraction: number }) {
  return (
    <div className="h-1.5 min-w-24 flex-1 overflow-hidden rounded-full bg-surface-3" role="progressbar" aria-valuenow={Math.round(fraction * 100)} aria-valuemin={0} aria-valuemax={100}>
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
  const route = project.sourceServerId === project.destServerId ? `${source} (reinstall)` : `${source} → ${dest}`;
  return (
    <tr className="cursor-pointer hover:bg-surface-2/60" onClick={onOpen}>
      <td className="max-w-0 truncate font-semibold" title={project.name}>
        {project.name}
      </td>
      <td className="max-w-0 truncate text-muted" title={route}>
        {route}
      </td>
      <td>
        <div className="flex items-center gap-2.5">
          <ProgressBar fraction={fraction} />
          <span className="text-xs whitespace-nowrap text-muted tabular-nums">
            {done.toLocaleString()} / {total.toLocaleString()}
          </span>
        </div>
      </td>
      <td className="whitespace-nowrap">
        {project.closed ? <span className="text-ok">Closed</span> : project.attention > 0 ? <span className="font-semibold text-warn">{project.attention.toLocaleString()} need you</span> : <span className="text-faint">–</span>}
      </td>
      <td className="whitespace-nowrap text-faint">{formatRelative(project.updatedAt, now)}</td>
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

/** The saved servers at a glance, from what the app already knows about them (no server is asked). */
function ServersCard({ servers, onManage }: { servers: ServerView[]; onManage: () => void }) {
  return (
    <Card className="space-y-3 px-5 py-4">
      <div className="flex items-center gap-3">
        <h2 className="flex-1 font-semibold">Servers</h2>
        <button type="button" onClick={onManage} className="text-sm font-medium text-accent hover:text-accent-hover">
          Manage
        </button>
      </div>
      <div className="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-2.5">
        {servers.map((s) => {
          const eco = s.ecoverse === "none" ? "REST only" : s.ecoverse === "own" ? (s.hasEcoverseToken ? "Ecoverse: own token" : "Ecoverse: token missing") : "Ecoverse: app key";
          const ok = s.hasJwt && !(s.ecoverse === "own" && !s.hasEcoverseToken);
          return (
            <div key={s.id} className="space-y-0.5 rounded-xl border border-border bg-surface-2 px-3.5 py-2.5">
              <div className="flex items-center gap-2">
                <Dot tone={ok ? "ok" : "err"} />
                <span className="truncate font-semibold" title={s.label}>
                  {s.label}
                </span>
              </div>
              {s.host !== s.label && (
                <div className="truncate text-xs text-faint" title={s.host}>
                  {s.host}
                </div>
              )}
              <div className={`truncate text-xs ${ok ? "text-muted" : "text-err"}`}>{s.hasJwt ? `JWT saved · ${eco}` : "No JWT saved"}</div>
            </div>
          );
        })}
      </div>
    </Card>
  );
}

/** The Migrate page: the project list (screen A in docs/migration/11-gui-design.md), and a project once one is opened. */
export function MigratePage({ ecoverseReady, onOpenSettings }: { ecoverseReady: boolean; onOpenSettings: (section: SettingsSection) => void }) {
  const projects = useProjects();
  const servers = useServers();
  const { remove } = useProjectActions();
  const now = useNow(30_000);
  const [openId, setOpenId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [showServers, setShowServers] = useState(false);
  const [deleting, setDeleting] = useState<ProjectSummary | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [show, setShow] = useState<"open" | "closed">("open");
  const list = useMemo(() => projects.data ?? [], [projects.data]);
  const openCount = list.filter((p) => !p.closed).length;
  const shown = useMemo(() => {
    const term = text.trim().toLowerCase();
    return list.filter((p) => (show === "closed") === p.closed && (!term || p.name.toLowerCase().includes(term)));
  }, [list, text, show]);

  // The list stays mounted while a project is open, so read it again when coming back to it.
  const refetchProjects = projects.refetch;
  useEffect(() => {
    if (openId === null) void refetchProjects();
  }, [openId, refetchProjects]);

  const startCreating = () => {
    setDeleteError(null);
    setCreating(true);
  };

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
          description="This permanently destroys the project's saved data on this computer: its encryption key is deleted and its file is overwritten and removed, so it can't be recovered, even from a backup. Its entries in Activity go too. Not changed: the servers and devices, the audit log (which records that the project was deleted), and any report or script you saved to your Downloads folder."
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
        meta="Move devices to another N-central server, or reinstall them on the same one."
        actions={
          // Servers and migration settings live in Settings (the gear), with everything else.
          <Button variant="primary" onClick={startCreating}>
            <Plus />
            New project
          </Button>
        }
      />
      <PageBody className="space-y-5">
        {projects.isError ? (
          <ErrorNote>{String(projects.error)}</ErrorNote>
        ) : projects.isPending ? (
          <p className="text-sm text-muted">Loading projects…</p>
        ) : list.length === 0 ? (
          <EmptyState
            icon={<ArrowRightLeft />}
            title="No migrations yet"
            action={
              <Button variant="primary" onClick={startCreating}>
                <Plus />
                New project
              </Button>
            }
          >
            A project moves devices from one N-central server to another, or reinstalls them on the same one.
          </EmptyState>
        ) : (
          <>
            <div className="flex flex-wrap items-center gap-2.5">
              <label className="relative w-72 max-w-full">
                <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
                <input className={`${inputClass} pl-8`} placeholder="Search projects" value={text} onChange={(e) => setText(e.target.value)} aria-label="Search projects" />
              </label>
              <Segmented
                value={show}
                onChange={setShow}
                options={[
                  { value: "open", label: `Open · ${openCount}` },
                  { value: "closed", label: `Closed · ${list.length - openCount}` },
                ]}
              />
            </div>
            {shown.length === 0 ? (
              <p className="py-6 text-center text-sm text-faint">{text ? "No projects match." : show === "open" ? "No open projects." : "No closed projects."}</p>
            ) : (
              <Table
                head={
                  <>
                    <th className="w-[26%]">Project</th>
                    <th className="w-[22%]">From → to</th>
                    <th className="w-[24%]">Moved and working</th>
                    <th>Needs you</th>
                    <th>Last change</th>
                    <th />
                  </>
                }
              >
                {shown.map((p) => (
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
            )}
          </>
        )}
        {servers.data && servers.data.length > 0 && <ServersCard servers={servers.data} onManage={() => onOpenSettings("servers")} />}
      </PageBody>
      {overlays}
    </>
  );
}
