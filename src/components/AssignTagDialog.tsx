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
    <Modal title={`Tag ${devices.length} device${devices.length === 1 ? "" : "s"}`} description="Pick a static tag or create one" onClose={onClose}>
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
            <div className="flex items-center gap-2">
              <TagKindIcon kind={t.__typename} />
              <span className="truncate">{t.name}</span>
              <span className="ml-auto shrink-0 text-xs text-faint tabular-nums">
                {busyId === t.id ? <Loader2 className="size-3.5 animate-spin" /> : `${t.assetCount ?? 0}`}
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
