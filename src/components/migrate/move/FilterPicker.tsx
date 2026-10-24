import { ChevronDown } from "lucide-react";
import { useCallback, useState } from "react";
import { Popover, PopoverContent, PopoverTrigger, SearchList } from "../../Combobox";

interface Option<K> {
  key: K | null;
  label: string;
}

/**
 * A filter that can have hundreds of choices (customers, waves): a pill showing the current choice, opening a
 * searchable list. The first entry clears the filter.
 */
export function FilterPicker<K extends string | number>({
  label,
  anyLabel,
  options,
  value,
  onChange,
}: {
  /** "Customer", "Wave". */
  label: string;
  /** The entry that clears the filter, e.g. "All customers". */
  anyLabel: string;
  options: { key: K; label: string }[];
  value: K | null;
  onChange: (value: K | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const current = value === null ? null : options.find((o) => o.key === value);
  const search = useCallback(
    async (term: string): Promise<Option<K>[]> => {
      const t = term.toLowerCase();
      const hits = options.filter((o) => o.label.toLowerCase().includes(t));
      return t ? hits : [{ key: null, label: anyLabel }, ...hits];
    },
    [options, anyLabel],
  );
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={`flex h-8 max-w-64 items-center gap-1.5 rounded-lg border px-3 text-xs whitespace-nowrap transition-colors ${
            current ? "border-accent/40 bg-accent-soft text-text" : "border-border bg-surface text-muted hover:text-text"
          }`}
        >
          <span className="truncate">
            {label}: <span className={current ? "font-semibold" : ""}>{current ? current.label : "any"}</span>
          </span>
          <ChevronDown className="size-3.5 shrink-0" />
        </button>
      </PopoverTrigger>
      <PopoverContent width={280}>
        <SearchList<Option<K>>
          search={search}
          debounceMs={0}
          scope={options}
          placeholder={`Search ${options.length.toLocaleString()} ${label.toLowerCase()}s`}
          getKey={(o) => String(o.key ?? "\u0000any")}
          isSelected={(o) => o.key === value}
          onPick={(o) => {
            onChange(o.key);
            setOpen(false);
          }}
          renderItem={(o) => <span className={`block truncate ${o.key === null ? "text-muted" : ""}`} title={o.label}>{o.label}</span>}
        />
      </PopoverContent>
    </Popover>
  );
}
