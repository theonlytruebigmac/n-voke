import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Loader2, MoreHorizontal, Pencil, Play, Plus, Search, Tags, Trash2, X } from "lucide-react";
import { assignStaticTags, deleteTags, unassignStaticTags } from "../lib/api";
import { membersQuery, refreshTagMembership, tagQuery, tagsQuery, uncacheTag } from "../lib/collections";
import { describeRule, filterToRule } from "../lib/rules";
import { matches } from "../lib/sync";
import type { RunDraft } from "../lib/targets";
import type { Asset, Tag } from "../lib/types";
import { DevicePickerField } from "./DevicePickerField";
import { AgentDot, TagKindIcon, tagKindLabel } from "./status";
import { TagEditorDialog } from "./TagEditorDialog";
import { TagWizard } from "./TagWizard";
import { Button, EmptyState, ErrorNote, IconButton, Menu, MenuContent, MenuItem, MenuTrigger, Modal, PageHeader, SectionTitle, Table, inputClass } from "./ui";

export function TagsPage({ ready, onDraft }: { ready: boolean; onDraft: (draft: Omit<RunDraft, "nonce">) => void }) {
  const [term, setTerm] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const { data, isPending, isFetching, error: queryError } = useQuery({ ...tagsQuery, enabled: ready });
  const tags = useMemo(() => (data?.items ?? []).filter((t) => matches(term, t.name)), [data, term]);
  const loading = ready && isPending;
  const error = queryError ? queryError.message : null;

  return (
    <>
      <PageHeader
        title="Tags"
        actions={
          <Button variant="primary" size="sm" onClick={() => setCreating(true)} disabled={!ready}>
            <Plus />
            New tag
          </Button>
        }
      />
      <div className="flex min-h-0 flex-1">
        <div className="flex w-72 shrink-0 flex-col border-r border-border">
          <div className="p-2">
            <div className="relative">
              <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
              <input className={`${inputClass} pl-8`} placeholder="Search tags" value={term} onChange={(e) => setTerm(e.target.value)} spellCheck={false} />
              {isFetching && <Loader2 className="absolute top-1/2 right-2.5 size-3.5 -translate-y-1/2 animate-spin text-faint" />}
            </div>
          </div>
          {error && (
            <div className="px-2">
              <ErrorNote>{error}</ErrorNote>
            </div>
          )}
          <ul className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
            {tags.map((t) => (
              <li key={t.id}>
                <button
                  onClick={() => setSelectedId(t.id)}
                  className={`flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left transition-colors ${t.id === selectedId ? "bg-surface-3" : "hover:bg-surface-2"}`}
                >
                  <TagKindIcon kind={t.__typename} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm">{t.name}</div>
                    <div className="truncate text-xs text-faint">{t.organization?.name}</div>
                  </div>
                  <span className="shrink-0 text-xs text-muted tabular-nums">{t.assetCount ?? 0}</span>
                </button>
              </li>
            ))}
            {!loading && tags.length === 0 && <li className="px-2.5 py-3 text-sm text-faint">{term ? "No matching tags" : "No tags yet"}</li>}
          </ul>
        </div>

        <div className="min-w-0 flex-1 overflow-y-auto">
          {creating ? (
            <TagWizard
              onCancel={() => setCreating(false)}
              onCreated={(t) => {
                setCreating(false);
                setSelectedId(t.id);
              }}
            />
          ) : selectedId ? (
            <TagDetail key={selectedId} tagId={selectedId} onDeleted={() => setSelectedId(null)} onDraft={onDraft} />
          ) : (
            <EmptyState icon={<Tags />} title="Select a tag">
              Static tags hold the devices you choose. Dynamic tags fill themselves from rules.
            </EmptyState>
          )}
        </div>
      </div>

    </>
  );
}

function TagDetail({ tagId, onDeleted, onDraft }: { tagId: string; onDeleted: () => void; onDraft: (draft: Omit<RunDraft, "nonce">) => void }) {
  const tagResult = useQuery(tagQuery(tagId));
  const tag = tagResult.data ?? null;
  const orgId = tag?.organization?.id ?? "";
  const membersResult = useQuery({ ...membersQuery(orgId, tagId), enabled: !!orgId });
  const members: Asset[] | null = membersResult.data?.assets ?? null;
  /** A failed action, shown above the device list until the next action. */
  const [actionError, setActionError] = useState<string | null>(null);
  const error = tagResult.error?.message ?? membersResult.error?.message ?? null;
  const [editing, setEditing] = useState(false);
  const [adding, setAdding] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  if (error)
    return (
      <div className="p-6">
        <ErrorNote>{error}</ErrorNote>
      </div>
    );
  if (!tag && tagResult.isPending)
    return (
      <div className="flex items-center gap-2 p-6 text-sm text-muted">
        <Loader2 className="size-4 animate-spin" /> Loading
      </div>
    );
  if (!tag) return <EmptyState icon={<Tags />} title="Tag not found">It may have been deleted.</EmptyState>;

  const isStatic = tag.__typename === "StaticTag";
  const rule = tag.filter ? filterToRule(tag.filter) : null;
  const org = tag.organization!;
  const count = members?.length ?? tag.assetCount ?? 0;

  const removeChecked = async () => {
    setBusy(true);
    setActionError(null);
    try {
      await unassignStaticTags([tag.id], [...checked], tag.name);
      setChecked(new Set());
      await refreshTagMembership(tag.id);
    } catch (e) {
      setActionError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    setActionError(null);
    try {
      await deleteTags([tag.id], `tag ${tag.name}`);
      uncacheTag(tag.id);
      onDeleted();
    } catch (e) {
      setActionError((e as Error).message);
      setConfirmDelete(false);
      setBusy(false);
    }
  };

  const toggle = (id: string) =>
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div className="mx-auto max-w-4xl space-y-5 px-6 py-5">
      <header className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="flex items-center gap-2 text-lg font-semibold">
            <TagKindIcon kind={tag.__typename} className="size-4" />
            <span className="truncate">{tag.name}</span>
          </h2>
          <p className="truncate text-sm text-muted">
            {tagKindLabel(tag.__typename)} · {org.name} · {count} device{count === 1 ? "" : "s"}
            {tag.description && ` · ${tag.description}`}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <Button size="sm" variant="primary" onClick={() => onDraft({ org: { id: org.id, name: org.name, __typename: "" }, targets: [{ kind: "tag", tag }] })}>
            <Play />
            Run script
          </Button>
          <Button size="sm" onClick={() => setEditing(true)}>
            <Pencil />
            Edit
          </Button>
          <Menu>
            <MenuTrigger asChild>
              <IconButton label="More">
                <MoreHorizontal />
              </IconButton>
            </MenuTrigger>
            <MenuContent>
              <MenuItem danger onSelect={() => setConfirmDelete(true)}>
                <Trash2 />
                Delete tag
              </MenuItem>
            </MenuContent>
          </Menu>
        </div>
      </header>

      {actionError && <ErrorNote>{actionError}</ErrorNote>}

      {!isStatic && (
        <div className="grid grid-cols-[112px_minmax(0,1fr)] gap-4 border-y border-border py-2.5 text-sm">
          <span className="text-muted">Rule</span>
          <span>{rule ? describeRule(rule) : <code className="font-mono text-xs">{tag.filter}</code>}</span>
        </div>
      )}

      <section className="space-y-2">
        <SectionTitle
          actions={
            isStatic &&
            (checked.size > 0 ? (
              <>
                <span className="mr-1 text-sm text-muted">{checked.size} selected</span>
                <Button size="sm" variant="danger" onClick={removeChecked} busy={busy}>
                  Remove from tag
                </Button>
                <IconButton label="Clear selection" onClick={() => setChecked(new Set())}>
                  <X />
                </IconButton>
              </>
            ) : (
              <Button size="sm" onClick={() => setAdding(true)}>
                <Plus />
                Add devices
              </Button>
            ))
          }
        >
          Devices
        </SectionTitle>
        {members === null ? (
          <p className="flex items-center gap-2 py-6 text-sm text-muted">
            <Loader2 className="size-4 animate-spin" /> Loading devices
          </p>
        ) : members.length === 0 ? (
          <p className="py-8 text-center text-sm text-faint">{isStatic ? "No devices yet" : "No devices match right now"}</p>
        ) : (
          <Table
            head={
              <>
                {isStatic && (
                  <th className="w-9">
                    <input
                      type="checkbox"
                      className="size-3.5 accent-accent"
                      checked={members.every((m) => checked.has(m.id))}
                      onChange={(e) => setChecked(e.target.checked ? new Set(members.map((m) => m.id)) : new Set())}
                      aria-label="Select all"
                    />
                  </th>
                )}
                <th>Device</th>
                <th>Location</th>
                <th>Operating system</th>
              </>
            }
          >
            {members.map((a) => (
              <tr key={a.id} className="hover:bg-surface-2/60" onClick={() => isStatic && toggle(a.id)}>
                {isStatic && (
                  <td onClick={(e) => e.stopPropagation()}>
                    <input type="checkbox" className="size-3.5 accent-accent" checked={checked.has(a.id)} onChange={() => toggle(a.id)} aria-label={`Select ${a.name}`} />
                  </td>
                )}
                <td>
                  <span className="flex items-center gap-2">
                    <AgentDot status={a.agentConnection?.status} />
                    <span className="truncate font-medium">{a.name}</span>
                  </span>
                </td>
                <td className="max-w-0 truncate text-muted">{[a.customer?.name, a.site?.name].filter(Boolean).join(" · ")}</td>
                <td className="max-w-0 truncate text-muted">{a.operatingSystem?.name}</td>
              </tr>
            ))}
          </Table>
        )}
        {!isStatic && <p className="text-xs text-faint">Membership updates automatically and can lag a minute after a change.</p>}
      </section>

      {editing && (
        <TagEditorDialog
          org={org}
          tag={tag}
          onClose={() => setEditing(false)}
          onSaved={() => undefined}
        />
      )}
      {adding && (
        <AddDevicesDialog
          tag={tag}
          existing={new Set(members?.map((m) => m.id))}
          onClose={() => setAdding(false)}
          onAdded={() => void refreshTagMembership(tag.id)}
        />
      )}
      {confirmDelete && (
        <Modal
          title={`Delete ${tag.name}?`}
          description={count ? `It's removed from ${count} device${count === 1 ? "" : "s"}. The devices aren't affected.` : "No devices have this tag."}
          onClose={() => setConfirmDelete(false)}
          size="sm"
          footer={
            <>
              <Button onClick={() => setConfirmDelete(false)} disabled={busy}>
                Cancel
              </Button>
              <Button variant="destructive" onClick={remove} busy={busy}>
                Delete
              </Button>
            </>
          }
        />
      )}
    </div>
  );
}

function AddDevicesDialog({ tag, existing, onClose, onAdded }: { tag: Tag; existing: Set<string>; onClose: () => void; onAdded: () => void }) {
  const [devices, setDevices] = useState<Asset[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const add = async () => {
    setBusy(true);
    setError(null);
    try {
      await assignStaticTags([tag.id], devices.map((d) => d.id), tag.name);
      onAdded();
      onClose();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={`Add devices to ${tag.name}`}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={add} busy={busy} disabled={!devices.length}>
            Add {devices.length || ""}
          </Button>
        </>
      }
    >
      <div className="space-y-3">
        <DevicePickerField orgId={tag.organization!.id} selected={devices} onChange={setDevices} exclude={existing} autoOpen />
        {error && <ErrorNote>{error}</ErrorNote>}
      </div>
    </Modal>
  );
}
