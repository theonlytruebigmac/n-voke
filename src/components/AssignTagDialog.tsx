import { useCallback, useState } from "react";
import { Loader2, Plus } from "lucide-react";
import { assignStaticTags } from "../lib/api";
import { refreshTagMembership, searchTagsLocal } from "../lib/collections";
import type { Asset, Tag } from "../lib/types";
import { SearchList } from "./Combobox";
import { TagKindIcon } from "./status";
import { TagWizard } from "./TagWizard";
import { ErrorNote, Modal } from "./ui";

/** Add devices to an existing static tag, or create a new one for them. */
export function AssignTagDialog({
  org,
  devices,
  suggestedName,
  onClose,
  onDone,
}: {
  org: { id: string; name: string };
  devices: Asset[];
  suggestedName: string;
  onClose: () => void;
  onDone: (tag: Tag) => void;
}) {
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState<string | null>(null);
  // Only static tags can have devices added by hand.
  const search = useCallback(async (t: string) => (await searchTagsLocal(t, org.id)).filter((tag) => tag.__typename === "StaticTag"), [org.id]);

  const assign = async (tag: Tag) => {
    setBusyId(tag.id);
    setError(null);
    try {
      await assignStaticTags([tag.id], devices.map((d) => d.id), tag.name);
      void refreshTagMembership(tag.id);
      onDone(tag);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusyId(null);
    }
  };

  if (creating !== null) {
    return (
      <Modal title="New tag" onClose={onClose} size="xl" bare>
        <TagWizard
          initialName={creating || suggestedName}
          initialOrg={{ ...org, __typename: "" }}
          initialDevices={devices}
          onCancel={onClose}
          onCreated={onDone}
        />
      </Modal>
    );
  }

  return (
    <Modal
      title={`Tag ${devices.length.toLocaleString()} device${devices.length === 1 ? "" : "s"}`}
      description="Add them to a static tag, or make a new one."
      onClose={onClose}
    >
      <div className="-mx-5 -mb-5 border-t border-border">
        <SearchList<Tag>
          search={search}
          debounceMs={0}
          scope={org.id}
          placeholder="Search static tags"
          getKey={(t) => t.id}
          onPick={(t) => void assign(t)}
          exactMatch={(t, term) => t.name.toLowerCase() === term.toLowerCase()}
          renderItem={(t) => (
            <div className="grid grid-cols-[14px_minmax(0,1fr)_auto] items-center gap-2.5 whitespace-nowrap" title={t.name}>
              <TagKindIcon kind={t.__typename} />
              <span className="truncate">{t.name}</span>
              <span className="text-right text-xs text-faint tabular-nums">
                {busyId === t.id ? <Loader2 className="ml-auto size-3.5 animate-spin" /> : `${(t.assetCount ?? 0).toLocaleString()} device${t.assetCount === 1 ? "" : "s"}`}
              </span>
            </div>
          )}
          footer={(term, hasExact) =>
            !hasExact && (
              <button
                type="button"
                onClick={() => setCreating(term.replace(/\s+/g, "_"))}
                className="mt-1 flex h-8 w-full items-center gap-2 rounded-md border-t border-border px-2 text-sm text-accent hover:bg-surface-2"
              >
                <Plus className="size-4" />
                {term ? <>Create tag “{term}”</> : "New tag"}
              </button>
            )
          }
        />
        {error && (
          <div className="p-3">
            <ErrorNote>{error}</ErrorNote>
          </div>
        )}
      </div>
    </Modal>
  );
}
