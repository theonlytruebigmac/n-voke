import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Loader2, MoreHorizontal, Pencil, Play, Plus, Search, Tags, Trash2 } from "lucide-react";
import { assignStaticTags, deleteTags, unassignStaticTags } from "../lib/api";
import { membersQuery, refreshTagMembership, tagQuery, tagsQuery, uncacheTag } from "../lib/collections";
import { describeRule, filterToRule } from "../lib/rules";
import { matches } from "../lib/sync";
import type { RunDraft } from "../lib/targets";
import type { Asset, Tag, TagKind } from "../lib/types";
import { DevicePickerField } from "./DevicePickerField";
import { AgentDot, TagKindIcon, tagKindLabel } from "./status";
import { TagEditorDialog } from "./TagEditorDialog";
import { TagWizard } from "./TagWizard";
import {
  Button,
  Card,
  EmptyState,
  ErrorNote,
  IconButton,
  Menu,
  MenuContent,
  MenuItem,
  MenuTrigger,
  Modal,
  PageBody,
  PageHeader,
  Segmented,
  SelectionBar,
  Table,
  inputClass,
} from "./ui";

type KindFilter = "all" | TagKind;

export function TagsPage({ ready, onDraft }: { ready: boolean; onDraft: (draft: Omit<RunDraft, "nonce">) => void }) {
  const [term, setTerm] = useState("");
  const [kind, setKind] = useState<KindFilter>("all");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const { data, isPending, isFetching, error: queryError } = useQuery({ ...tagsQuery, enabled: ready });
  const all = data?.items ?? [];
  const tags = useMemo(() => all.filter((t) => (kind === "all" || t.__typename === kind) && matches(term, t.name)), [all, kind, term]);
  const loading = ready && isPending;
  const error = queryError ? queryError.message : null;
  const count = (k: TagKind) => all.filter((t) => t.__typename === k).length;

  return (
    <>
      <PageHeader
        title="Tags"
        actions={
          <Button variant="primary" onClick={() => setCreating(true)} disabled={!ready}>
            <Plus />
            New tag
          </Button>
        }
      />
      <PageBody>
        <div className="flex flex-wrap items-start gap-5">
          <section aria-label="All tags" className="flex max-h-[calc(100vh-11rem)] min-w-0 flex-[1_1_280px] flex-col gap-2.5 md:sticky md:top-0 md:max-w-80">
            <div className="relative">
              <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
              <input
                className={`${inputClass} h-9 pl-8`}
                placeholder={all.length ? `Search ${all.length.toLocaleString()} tags` : "Search tags"}
                aria-label="Search tags"
                value={term}
                onChange={(e) => setTerm(e.target.value)}
                spellCheck={false}
              />
              {isFetching && <Loader2 className="absolute top-1/2 right-2.5 size-3.5 -translate-y-1/2 animate-spin text-faint" />}
            </div>
            <div>
              <Segmented<KindFilter>
                value={kind}
                onChange={setKind}
                options={[
                  { value: "all", label: "All" },
                  { value: "StaticTag", label: <><TagKindIcon kind="StaticTag" /> Static · {count("StaticTag")}</> },
                  { value: "DynamicTag", label: <><TagKindIcon kind="DynamicTag" /> Dynamic · {count("DynamicTag")}</> },
                ]}
              />
            </div>
            {error && <ErrorNote>{error}</ErrorNote>}
            <ul className="min-h-0 flex-1 space-y-0.5 overflow-y-auto">
              {tags.map((t) => (
                <li key={t.id}>
                  <button
                    onClick={() => {
                      setCreating(false);
                      setSelectedId(t.id);
                    }}
                    aria-current={t.id === selectedId && !creating ? "true" : undefined}
                    title={t.name}
                    className={`flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left transition-colors ${t.id === selectedId && !creating ? "bg-surface-3" : "hover:bg-surface-2"}`}
                  >
                    <TagKindIcon kind={t.__typename} />
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-medium">{t.name}</div>
                      <div className="truncate text-xs text-faint">{t.organization?.name}</div>
                    </div>
                    <span className="shrink-0 text-xs whitespace-nowrap text-muted tabular-nums">{(t.assetCount ?? 0).toLocaleString()}</span>
                  </button>
                </li>
              ))}
              {!loading && tags.length === 0 && <li className="px-2.5 py-3 text-sm text-faint">{term || kind !== "all" ? "No matching tags" : "No tags yet"}</li>}
            </ul>
          </section>

          <div className="min-w-0 flex-[999_1_560px]">
            {creating ? (
              <Card className="overflow-hidden">
                <div className="flex items-center gap-2 border-b border-border px-5 py-3">
                  <h2 className="flex-1 text-base font-semibold">New tag</h2>
                </div>
                <TagWizard
                  onCancel={() => setCreating(false)}
                  onCreated={(t) => {
                    setCreating(false);
                    setSelectedId(t.id);
                  }}
                />
              </Card>
            ) : selectedId ? (
              <TagDetail key={selectedId} tagId={selectedId} onDeleted={() => setSelectedId(null)} onDraft={onDraft} />
            ) : (
              <Card>
                <EmptyState icon={<Tags />} title="Select a tag">
                  Static tags hold the devices you choose. Dynamic tags fill themselves from rules.
                </EmptyState>
              </Card>
            )}
          </div>
        </div>
      </PageBody>
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
  const [term, setTerm] = useState("");
  const [busy, setBusy] = useState(false);

  const shown = useMemo(
    () => (members ?? []).filter((a) => !term || [a.name, a.customer?.name, a.site?.name, a.operatingSystem?.name].some((s) => s && matches(term, s))),
    [members, term],
  );

  if (error)
    return (
      <Card className="p-5">
        <ErrorNote>{error}</ErrorNote>
      </Card>
    );
  if (!tag && tagResult.isPending)
    return (
      <Card className="flex items-center gap-2 p-5 text-sm text-muted">
        <Loader2 className="size-4 animate-spin" /> Loading
      </Card>
    );
  if (!tag)
    return (
      <Card>
        <EmptyState icon={<Tags />} title="Tag not found">
          It may have been deleted.
        </EmptyState>
      </Card>
    );

  const isStatic = tag.__typename === "StaticTag";
  const rule = tag.filter ? filterToRule(tag.filter) : null;
  const org = tag.organization!;
  const count = members?.length ?? tag.assetCount ?? 0;
  const allShownChecked = shown.length > 0 && shown.every((m) => checked.has(m.id));

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
    <Card className="space-y-5 p-5">
      <header className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-[1_1_280px]">
          <h2 className="flex items-center gap-2 text-lg font-semibold" title={tag.name}>
            <TagKindIcon kind={tag.__typename} className="size-4" />
            <span className="truncate">{tag.name}</span>
          </h2>
          <p className="text-sm text-muted">
            {tagKindLabel(tag.__typename)} · {org.name} · {count.toLocaleString()} device{count === 1 ? "" : "s"}
            {tag.description && ` · ${tag.description}`}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <Button variant="primary" onClick={() => onDraft({ org: { id: org.id, name: org.name, __typename: "" }, targets: [{ kind: "tag", tag }] })}>
            <Play />
            Run a script
          </Button>
          <Button onClick={() => setEditing(true)}>
            <Pencil />
            Edit
          </Button>
          <Menu>
            <MenuTrigger asChild>
              <IconButton label="More" className="size-8 border border-border-strong">
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
        <div className="flex flex-wrap gap-x-4 gap-y-1 rounded-xl border border-border bg-bg px-4 py-3 text-sm">
          <span className="w-24 shrink-0 text-muted">Rule</span>
          <span className="min-w-0 flex-1">{rule ? describeRule(rule) : <code className="font-mono text-xs break-all">{tag.filter}</code>}</span>
        </div>
      )}

      <section className="space-y-3">
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="flex-1 text-sm font-semibold">Devices</h3>
          {members && members.length > 0 && (
            <div className="relative w-56 max-w-full">
              <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
              <input className={`${inputClass} pl-8`} placeholder="Search devices" aria-label="Search devices in this tag" value={term} onChange={(e) => setTerm(e.target.value)} spellCheck={false} />
            </div>
          )}
          {isStatic && (
            <Button onClick={() => setAdding(true)}>
              <Plus />
              Add devices
            </Button>
          )}
        </div>

        {isStatic && checked.size > 0 && (
          <SelectionBar count={checked.size} onClear={() => setChecked(new Set())}>
            <Button size="sm" variant="danger" onClick={removeChecked} busy={busy}>
              Remove from tag
            </Button>
          </SelectionBar>
        )}

        {members === null ? (
          <p className="flex items-center gap-2 py-6 text-sm text-muted">
            <Loader2 className="size-4 animate-spin" /> Loading devices
          </p>
        ) : members.length === 0 ? (
          <p className="py-8 text-center text-sm text-faint">{isStatic ? "No devices yet" : "No devices match right now"}</p>
        ) : shown.length === 0 ? (
          <p className="py-8 text-center text-sm text-faint">No devices match “{term}”</p>
        ) : (
          <Table
            head={
              <>
                {isStatic && (
                  <th className="w-9">
                    <input
                      type="checkbox"
                      className="size-3.5 accent-accent"
                      checked={allShownChecked}
                      onChange={() =>
                        setChecked((prev) => {
                          const next = new Set(prev);
                          for (const m of shown) {
                            if (allShownChecked) next.delete(m.id);
                            else next.add(m.id);
                          }
                          return next;
                        })
                      }
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
            {shown.map((a) => (
              <tr key={a.id} className={`hover:bg-surface-2/60 ${checked.has(a.id) ? "bg-accent-soft" : ""}`} onClick={() => isStatic && toggle(a.id)}>
                {isStatic && (
                  <td onClick={(e) => e.stopPropagation()}>
                    <input type="checkbox" className="size-3.5 accent-accent" checked={checked.has(a.id)} onChange={() => toggle(a.id)} aria-label={`Select ${a.name}`} />
                  </td>
                )}
                <td>
                  <span className="flex items-center gap-2 whitespace-nowrap">
                    <AgentDot status={a.agentConnection?.status} />
                    <span className="truncate font-medium" title={a.name}>
                      {a.name}
                    </span>
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

      {editing && <TagEditorDialog org={org} tag={tag} onClose={() => setEditing(false)} onSaved={() => undefined} />}
      {adding && (
        <AddDevicesDialog tag={tag} existing={new Set(members?.map((m) => m.id))} onClose={() => setAdding(false)} onAdded={() => void refreshTagMembership(tag.id)} />
      )}
      {confirmDelete && (
        <Modal
          title={`Delete ${tag.name}?`}
          description={count ? `It's removed from ${count.toLocaleString()} device${count === 1 ? "" : "s"}. The devices aren't affected.` : "No devices have this tag."}
          onClose={() => setConfirmDelete(false)}
          size="sm"
          footer={
            <>
              <Button onClick={() => setConfirmDelete(false)} disabled={busy}>
                Cancel
              </Button>
              <Button variant="destructive" onClick={remove} busy={busy}>
                Delete tag
              </Button>
            </>
          }
        />
      )}
    </Card>
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
