import { useEffect, useRef, useState, type ReactNode } from "react";
import * as PopoverPrimitive from "@radix-ui/react-popover";
import { Command } from "cmdk";
import { Check, ChevronsUpDown, Loader2, Search } from "lucide-react";

const DEBOUNCE_MS = 200;

/** Runs `search` as the term changes, ignoring responses that arrive out of order. */
export function useAsyncSearch<T>(search: (term: string) => Promise<T[]>, term: string, active: boolean, scope: unknown = null, debounceMs = DEBOUNCE_MS) {
  const [items, setItems] = useState<T[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const searchRef = useRef(search);
  searchRef.current = search;

  useEffect(() => {
    if (!active) return;
    const id = ++requestId.current;
    setLoading(true);
    const timer = setTimeout(
      async () => {
        try {
          const result = await searchRef.current(term.trim());
          if (id !== requestId.current) return;
          setItems(result);
          setError(null);
        } catch (e) {
          if (id === requestId.current) setError((e as Error).message);
        } finally {
          if (id === requestId.current) setLoading(false);
        }
      },
      term ? debounceMs : 0,
    );
    return () => clearTimeout(timer);
  }, [term, active, scope, debounceMs]);

  return { items, loading, error };
}

interface SearchListProps<T> {
  search: (term: string) => Promise<T[]>;
  getKey: (item: T) => string;
  renderItem: (item: T) => ReactNode;
  onPick: (item: T) => void;
  isSelected?: (item: T) => boolean;
  placeholder: string;
  /** Changing this re-runs the search (e.g. a different organization or tab). */
  scope?: unknown;
  /** Extra row(s) under the results, e.g. "Create tag". Receives the current term. */
  footer?: (term: string, hasExactMatch: boolean) => ReactNode;
  exactMatch?: (item: T, term: string) => boolean;
  header?: ReactNode;
  /** 0 for searches over local (cached) data; the default suits server searches. */
  debounceMs?: number;
}

/** Search box + keyboard-navigable results, used inside popovers. */
export function SearchList<T>({ search, getKey, renderItem, onPick, isSelected, placeholder, scope, footer, exactMatch, header, debounceMs }: SearchListProps<T>) {
  const [term, setTerm] = useState("");
  const { items, loading, error } = useAsyncSearch(search, term, true, scope, debounceMs);
  const hasExact = !!exactMatch && items.some((i) => exactMatch(i, term.trim()));
  // Results arrive asynchronously, so keep the highlight on the first one
  // whenever they change (cmdk only does this for its own filtering).
  const [active, setActive] = useState("");
  const firstKey = items.length ? getKey(items[0]) : "";
  useEffect(() => setActive(firstKey), [firstKey]);

  return (
    <Command
      shouldFilter={false}
      loop
      value={active}
      onValueChange={setActive}
      className="flex max-h-[min(420px,var(--radix-popover-content-available-height))] flex-col"
    >
      {header}
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border px-3">
        {loading ? <Loader2 className="size-4 shrink-0 animate-spin text-faint" /> : <Search className="size-4 shrink-0 text-faint" />}
        <Command.Input
          value={term}
          onValueChange={setTerm}
          placeholder={placeholder}
          className="h-full w-full bg-transparent text-sm outline-none placeholder:text-faint focus-visible:outline-none"
        />
      </div>
      <Command.List className="min-h-0 overflow-y-auto p-1">
        {error && <div className="px-2 py-3 text-sm text-err">{error}</div>}
        {!error && !loading && items.length === 0 && <div className="px-2 py-3 text-sm text-faint">No results</div>}
        {items.map((item) => {
          const selected = isSelected?.(item) ?? false;
          return (
            <Command.Item
              key={getKey(item)}
              value={getKey(item)}
              onSelect={() => onPick(item)}
              className="flex min-h-8 cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-sm data-[selected=true]:bg-surface-3"
            >
              {isSelected && (
                <span className={`flex size-4 shrink-0 items-center justify-center ${selected ? "text-accent" : "text-transparent"}`}>
                  <Check className="size-4" />
                </span>
              )}
              <div className="min-w-0 flex-1">{renderItem(item)}</div>
            </Command.Item>
          );
        })}
        {footer && !error && footer(term.trim(), hasExact)}
      </Command.List>
    </Command>
  );
}

export const Popover = PopoverPrimitive.Root;
export const PopoverTrigger = PopoverPrimitive.Trigger;

export const PopoverAnchor = PopoverPrimitive.Anchor;

export function PopoverContent({
  children,
  width = "trigger",
  onCloseAutoFocus,
  onInteractOutside,
}: {
  children: ReactNode;
  width?: "trigger" | number;
  onCloseAutoFocus?: (e: Event) => void;
  onInteractOutside?: (e: Event) => void;
}) {
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Content
        onCloseAutoFocus={onCloseAutoFocus}
        onInteractOutside={onInteractOutside}
        onOpenAutoFocus={(e) => {
          // Focus the search box rather than the first focusable (e.g. a tab).
          const input = (e.currentTarget as HTMLElement | null)?.querySelector<HTMLInputElement>("[cmdk-input], input");
          if (input) {
            e.preventDefault();
            input.focus();
          }
        }}
        align="start"
        sideOffset={4}
        collisionPadding={12}
        style={{ width: width === "trigger" ? "var(--radix-popover-trigger-width)" : width }}
        className="z-50 animate-pop overflow-hidden rounded-lg border border-border bg-surface shadow-xl"
      >
        {children}
      </PopoverPrimitive.Content>
    </PopoverPrimitive.Portal>
  );
}

/** Looks like an input; shows the chosen value or a placeholder. */
export function ComboboxTrigger({ children, placeholder, disabled }: { children?: ReactNode; placeholder: string; disabled?: boolean }) {
  return (
    <PopoverPrimitive.Trigger
      disabled={disabled}
      className="flex h-8 w-full items-center gap-2 rounded-md border border-border bg-surface px-2.5 text-left text-sm shadow-sm transition-colors hover:border-border-strong focus:border-accent focus:ring-3 focus:ring-accent-soft focus:outline-none disabled:opacity-50 data-[state=open]:border-accent"
    >
      <span className="min-w-0 flex-1 truncate">{children ?? <span className="text-faint">{placeholder}</span>}</span>
      <ChevronsUpDown className="size-3.5 shrink-0 text-faint" />
    </PopoverPrimitive.Trigger>
  );
}

/** Single-value searchable picker. */
export function Combobox<T>({
  value,
  onChange,
  renderValue,
  placeholder,
  searchPlaceholder,
  disabled,
  defaultOpen,
  ...list
}: Omit<SearchListProps<T>, "onPick" | "placeholder"> & {
  value: T | null;
  onChange: (item: T) => void;
  renderValue: (item: T) => ReactNode;
  /** Shown in the field when nothing is chosen. */
  placeholder: string;
  /** Shown in the dropdown's search box. */
  searchPlaceholder: string;
  disabled?: boolean;
  /** Open the dropdown as soon as this becomes true (e.g. when the step becomes active). */
  defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(!!defaultOpen);
  const picked = useRef(false);
  useEffect(() => {
    if (defaultOpen) setOpen(true);
  }, [defaultOpen]);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <ComboboxTrigger placeholder={placeholder} disabled={disabled}>
        {value ? renderValue(value) : undefined}
      </ComboboxTrigger>
      <PopoverContent
        onCloseAutoFocus={(e) => {
          // After a pick, don't pull focus back to this field: the next step's
          // dropdown may be opening, and stealing focus would close it.
          if (picked.current) {
            e.preventDefault();
            picked.current = false;
          }
        }}
      >
        <SearchList
          {...list}
          placeholder={searchPlaceholder}
          isSelected={value ? (i) => list.getKey(i) === list.getKey(value) : undefined}
          onPick={(item) => {
            picked.current = true;
            onChange(item);
            setOpen(false);
          }}
        />
      </PopoverContent>
    </Popover>
  );
}
