import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Plus, X } from "lucide-react";
import { Popover, PopoverAnchor, PopoverContent, PopoverTrigger } from "./Combobox";

export interface Token {
  key: string;
  icon: ReactNode;
  label: string;
  /** Small trailing detail, e.g. a tag's device count. */
  detail?: ReactNode;
  onRemove: () => void;
}

/**
 * A one-line field of removable tokens with a picker dropdown. The dropdown is
 * anchored to the whole field and the field never wraps, so the dropdown stays
 * put while items are added. Tokens that don't fit fade out; the count shows
 * how many there are.
 */
export function TokenField({
  tokens,
  placeholder,
  open,
  onOpenChange,
  width = 440,
  children,
}: {
  tokens: Token[];
  placeholder: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  width?: number;
  children: ReactNode;
}) {
  const fieldRef = useRef<HTMLDivElement>(null);
  const tokensRef = useRef<HTMLDivElement>(null);
  const [overflowing, setOverflowing] = useState(false);

  // Fade the right edge only when tokens are actually cut off.
  useLayoutEffect(() => {
    const el = tokensRef.current;
    if (!el) return;
    const check = () => setOverflowing(el.scrollWidth > el.clientWidth + 1);
    check();
    const observer = new ResizeObserver(check);
    observer.observe(el);
    return () => observer.disconnect();
  }, [tokens.length]);

  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverAnchor asChild>
        <div
          ref={fieldRef}
          onClick={() => onOpenChange(true)}
          className={`flex h-8 w-full cursor-text items-center gap-1 rounded-md border bg-surface pr-1 pl-1 shadow-sm transition-colors ${
            open ? "border-accent ring-3 ring-accent-soft" : "border-border hover:border-border-strong"
          }`}
        >
          <div
            ref={tokensRef}
            className="flex min-w-0 flex-1 items-center gap-1 overflow-hidden"
            style={overflowing ? { maskImage: "linear-gradient(to right, black 80%, transparent)", WebkitMaskImage: "linear-gradient(to right, black 80%, transparent)" } : undefined}
          >
            {tokens.length === 0 && <span className="px-1.5 text-sm text-faint">{placeholder}</span>}
            {tokens.map((t) => (
              <span key={t.key} className="inline-flex h-6 shrink-0 items-center gap-1.5 rounded bg-surface-3 pr-1 pl-2 text-xs">
                {t.icon}
                <span className="max-w-44 truncate">{t.label}</span>
                {t.detail}
                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation();
                    t.onRemove();
                  }}
                  className="rounded p-0.5 text-faint hover:bg-surface-2 hover:text-text"
                  aria-label={`Remove ${t.label}`}
                >
                  <X className="size-3" />
                </button>
              </span>
            ))}
          </div>
          {tokens.length > 0 && <span className="shrink-0 px-1 text-xs text-muted tabular-nums">{tokens.length} selected</span>}
          <PopoverTrigger
            onClick={(e) => e.stopPropagation()}
            className="inline-flex size-6 shrink-0 items-center justify-center rounded text-faint hover:bg-surface-2 hover:text-text"
            aria-label="Add"
          >
            <Plus className="size-3.5" />
          </PopoverTrigger>
        </div>
      </PopoverAnchor>
      <PopoverContent
        width={width}
        // Clicks inside the field (removing a token, re-focusing) shouldn't close the dropdown.
        onInteractOutside={(e) => {
          if (fieldRef.current?.contains(e.target as Node)) e.preventDefault();
        }}
      >
        {children}
      </PopoverContent>
    </Popover>
  );
}
