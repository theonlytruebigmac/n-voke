import { useCallback, useState } from "react";
import { Monitor, Plus } from "lucide-react";
import { searchAssetsCached, searchTagsLocal } from "../lib/collections";
import { targetKey, type Target } from "../lib/targets";
import type { Asset, Organization, Tag } from "../lib/types";
import { SearchList } from "./Combobox";
import { TokenField } from "./TokenField";
import { AgentDot, TagKindIcon, tagKindLabel } from "./status";
import { TagWizard } from "./TagWizard";
import { Modal } from "./ui";
import { Segmented } from "./ui";

type Mode = "devices" | "tags";

/**
 * Devices and tags as removable tokens in one field. The dropdown stays open
 * while picking several; a tag search with no match offers to create it.
 */
export function TargetField({
  org,
  targets,
  onChange,
  autoOpen,
}: {
  org: Organization;
  targets: Target[];
  onChange: (targets: Target[]) => void;
  autoOpen?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [mode, setMode] = useState<Mode>("devices");
  const [creating, setCreating] = useState<string | null>(null);
  const selected = new Set(targets.map(targetKey));

  // Open once when this becomes the next step to fill in.
  const [autoOpened, setAutoOpened] = useState(false);
  if (autoOpen && !autoOpened && !open) {
    setAutoOpened(true);
    setOpen(true);
  }

  const searchDevices = useCallback((t: string) => searchAssetsCached(org.id, t), [org.id]);
  const searchTagList = useCallback((t: string) => searchTagsLocal(t, org.id), [org.id]);

  const toggle = (t: Target) => {
    const key = targetKey(t);
    onChange(selected.has(key) ? targets.filter((x) => targetKey(x) !== key) : [...targets, t]);
  };

  return (
    <>
      <TokenField
        open={open}
        onOpenChange={setOpen}
        placeholder="Add devices or tags"
        tokens={targets.map((t) => ({
          key: targetKey(t),
          icon: t.kind === "tag" ? <TagKindIcon kind={t.tag.__typename} className="size-3" /> : <Monitor className="size-3 text-muted" />,
          label: t.kind === "tag" ? t.tag.name : t.asset.name,
          detail: t.kind === "tag" && t.tag.assetCount != null ? <span className="text-faint tabular-nums">{t.tag.assetCount}</span> : undefined,
          onRemove: () => onChange(targets.filter((x) => targetKey(x) !== targetKey(t))),
        }))}
      >
          {mode === "devices" ? (
            <SearchList<Asset>
              key="devices"
              header={<ModeSwitch mode={mode} onChange={setMode} />}
              search={searchDevices}
              scope={org.id}
              placeholder="Search devices"
              getKey={(a) => a.id}
              isSelected={(a) => selected.has(`d:${a.id}`)}
              onPick={(a) => toggle({ kind: "device", asset: a })}
              renderItem={(a) => (
                <div className="flex items-center gap-2">
                  <AgentDot status={a.agentConnection?.status} />
                  <span className="truncate">{a.name}</span>
                  <span className="ml-auto truncate text-xs text-faint">{a.customer?.name}</span>
                </div>
              )}
            />
          ) : (
            <SearchList<Tag>
              key="tags"
              header={<ModeSwitch mode={mode} onChange={setMode} />}
              search={searchTagList}
              debounceMs={0}
              scope={org.id}
              placeholder="Search tags"
              getKey={(t) => t.id}
              isSelected={(t) => selected.has(`t:${t.id}`)}
              onPick={(t) => toggle({ kind: "tag", tag: t })}
              exactMatch={(t, term) => t.name.toLowerCase() === term.toLowerCase()}
              renderItem={(t) => (
                <div className="flex items-center gap-2">
                  <TagKindIcon kind={t.__typename} />
                  <span className="truncate">{t.name}</span>
                  <span className="ml-auto shrink-0 text-xs text-faint tabular-nums">
                    {tagKindLabel(t.__typename)} · {t.assetCount ?? 0}
                  </span>
                </div>
              )}
              footer={(term, hasExact) =>
                !hasExact && (
                  <button
                    type="button"
                    onClick={() => {
                      setOpen(false);
                      setCreating(term);
                    }}
                    className="mt-1 flex h-8 w-full items-center gap-2 rounded-md border-t border-border px-2 text-sm text-accent hover:bg-surface-2"
                  >
                    <Plus className="size-4" />
                    {term ? <>Create tag “{term}”</> : "New tag"}
                  </button>
                )
              }
            />
          )}
      </TokenField>

      {creating !== null && (
        <Modal title="New tag" onClose={() => setCreating(null)} size="xl" bare>
          <TagWizard
            initialName={creating.replace(/\s+/g, "_")}
            initialOrg={org}
            onCancel={() => setCreating(null)}
            onCreated={(tag) => {
              onChange([...targets, { kind: "tag", tag }]);
              setCreating(null);
            }}
          />
        </Modal>
      )}
    </>
  );
}

function ModeSwitch({ mode, onChange }: { mode: Mode; onChange: (m: Mode) => void }) {
  return (
    <div className="border-b border-border px-2 py-1.5">
      <Segmented<Mode>
        value={mode}
        onChange={onChange}
        options={[
          { value: "devices", label: "Devices" },
          { value: "tags", label: "Tags" },
        ]}
      />
    </div>
  );
}
