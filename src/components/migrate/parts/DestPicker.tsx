import { Command } from "cmdk";
import { useState } from "react";
import { Ban, RotateCcw } from "lucide-react";
import type { MapRow, UnitRef } from "../../../lib/migrate/bindings";
import { ComboboxTrigger, Popover, PopoverContent, SearchList } from "../../Combobox";

const label = (u: UnitRef) => (u.parentName ? `${u.name} · ${u.parentName}` : u.name);

/** Where a source customer or site goes: a search over the destination units of the same kind, plus Leave out. */
export function DestPicker({
  row,
  units,
  onChoose,
  onLeaveOut,
  onAutomatic,
}: {
  row: MapRow;
  /** Every destination customer and site. */
  units: UnitRef[];
  onChoose: (unit: UnitRef) => void;
  onLeaveOut: () => void;
  /** Go back to the automatic match; offered only when the operator chose this one. */
  onAutomatic: () => void;
}) {
  const [open, setOpen] = useState(false);
  const sameKind = units.filter((u) => u.kind === row.kind);
  const close = (then: () => void) => () => {
    setOpen(false);
    then();
  };

  const trigger =
    row.status === "leftOut" ? (
      <span className="text-muted">Left out</span>
    ) : row.dest ? (
      <span>{label(row.dest)}</span>
    ) : row.status === "ambiguous" ? (
      <span className="text-warn">Choose… ({row.candidates.length} fit)</span>
    ) : (
      <span className="text-err">No match</span>
    );

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <ComboboxTrigger placeholder="Choose">{trigger}</ComboboxTrigger>
      <PopoverContent width={340}>
        <SearchList<UnitRef>
          debounceMs={0}
          search={async (term) => {
            const t = term.toLowerCase();
            const fits = sameKind.filter((u) => label(u).toLowerCase().includes(t));
            // Units that fit equally well come first.
            const best = new Set(row.candidates.map((c) => c.id));
            return [...fits.filter((u) => best.has(u.id)), ...fits.filter((u) => !best.has(u.id))];
          }}
          getKey={(u) => u.id}
          isSelected={(u) => u.id === row.dest?.id}
          onPick={(u) => close(() => onChoose(u))()}
          placeholder={`Search ${row.kind === "site" ? "sites" : "customers"}`}
          renderItem={(u) => (
            <div className="flex items-baseline justify-between gap-3">
              <span className="truncate">{u.name}</span>
              {u.parentName && <span className="shrink-0 text-xs text-faint">{u.parentName}</span>}
            </div>
          )}
          footer={() => (
            <>
              <Command.Item
                value="__leave-out"
                onSelect={close(onLeaveOut)}
                className="flex min-h-8 cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-sm text-muted data-[selected=true]:bg-surface-3"
              >
                <Ban className="size-4" />
                Leave out of the migration
              </Command.Item>
              {row.how === "chosen" && (
                <Command.Item
                  value="__automatic"
                  onSelect={close(onAutomatic)}
                  className="flex min-h-8 cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-sm text-muted data-[selected=true]:bg-surface-3"
                >
                  <RotateCcw className="size-4" />
                  Use the automatic match
                </Command.Item>
              )}
            </>
          )}
        />
      </PopoverContent>
    </Popover>
  );
}
