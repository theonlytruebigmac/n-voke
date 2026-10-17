import { useCallback, useState } from "react";
import { searchAssetsCached } from "../lib/collections";
import type { Asset } from "../lib/types";
import { SearchList } from "./Combobox";
import { TokenField } from "./TokenField";
import { AgentDot } from "./status";

/** Several devices as removable tokens, picked from a searchable dropdown. */
export function DevicePickerField({
  orgId,
  selected,
  onChange,
  exclude,
  autoOpen = false,
}: {
  orgId: string;
  selected: Asset[];
  onChange: (assets: Asset[]) => void;
  /** Device IDs that can't be picked (e.g. already in the tag). */
  exclude?: Set<string>;
  autoOpen?: boolean;
}) {
  const [open, setOpen] = useState(autoOpen);
  const ids = new Set(selected.map((a) => a.id));
  const search = useCallback(
    async (t: string) => (await searchAssetsCached(orgId, t)).filter((a) => !exclude?.has(a.id)),
    [orgId, exclude],
  );
  const toggle = (a: Asset) => onChange(ids.has(a.id) ? selected.filter((s) => s.id !== a.id) : [...selected, a]);

  return (
    <TokenField
      open={open}
      onOpenChange={setOpen}
      width={400}
      placeholder="Add devices"
      tokens={selected.map((a) => ({ key: a.id, icon: <AgentDot status={a.agentConnection?.status} />, label: a.name, onRemove: () => toggle(a) }))}
    >
      <SearchList<Asset>
        search={search}
        scope={orgId}
        placeholder="Search devices"
        getKey={(a) => a.id}
        isSelected={(a) => ids.has(a.id)}
        onPick={toggle}
        renderItem={(a) => (
          <div className="flex items-center gap-2">
            <AgentDot status={a.agentConnection?.status} />
            <span className="truncate">{a.name}</span>
            <span className="ml-auto truncate text-xs text-faint">{a.customer?.name}</span>
          </div>
        )}
      />
    </TokenField>
  );
}
