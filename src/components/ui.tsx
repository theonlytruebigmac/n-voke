import { forwardRef, useEffect, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import * as MenuPrimitive from "@radix-ui/react-dropdown-menu";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { Check, Copy, Loader2, X } from "lucide-react";
import { copyText } from "../lib/format";

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

type Variant = "primary" | "secondary" | "ghost" | "danger" | "destructive";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-on-accent hover:bg-accent-hover shadow-sm",
  secondary: "border border-border bg-surface text-text hover:bg-surface-2 shadow-sm",
  ghost: "text-muted hover:bg-surface-2 hover:text-text",
  danger: "text-err hover:bg-err-soft",
  destructive: "bg-err text-white hover:opacity-90 shadow-sm",
};

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: "sm" | "md"; busy?: boolean };

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "secondary", size = "md", busy = false, className = "", children, disabled, type = "button", ...rest },
  ref,
) {
  const sizes = size === "sm" ? "h-7 px-2.5 text-xs gap-1.5" : "h-8 px-3 text-sm gap-1.5";
  return (
    <button
      ref={ref}
      type={type}
      {...rest}
      disabled={disabled || busy}
      className={`inline-flex shrink-0 items-center justify-center rounded-md font-medium whitespace-nowrap transition-colors disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-3.5 [&_svg]:shrink-0 ${sizes} ${VARIANTS[variant]} ${className}`}
    >
      {busy && <Loader2 className="animate-spin" />}
      {children}
    </button>
  );
});

/** Square icon-only button; the label is used for screen readers and the tooltip. */
export const IconButton = forwardRef<HTMLButtonElement, ButtonHTMLAttributes<HTMLButtonElement> & { label: string }>(function IconButton(
  { label, className = "", children, type = "button", ...rest },
  ref,
) {
  return (
    <Tooltip content={label}>
      <button
        ref={ref}
        type={type}
        aria-label={label}
        {...rest}
        className={`inline-flex size-7 shrink-0 items-center justify-center rounded-md text-muted transition-colors hover:bg-surface-2 hover:text-text disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-4 ${className}`}
      >
        {children}
      </button>
    </Tooltip>
  );
});

// ---------------------------------------------------------------------------
// Form controls
// ---------------------------------------------------------------------------

export const inputClass =
  "h-8 w-full rounded-md border border-border bg-surface px-2.5 text-sm text-text shadow-sm placeholder:text-faint transition-colors hover:border-border-strong focus:border-accent focus:outline-none focus:ring-3 focus:ring-accent-soft disabled:opacity-60";

/** Native selects styled to match inputs (themed instead of the OS default). */
export const selectClass =
  "select-chevron h-8 appearance-none rounded-md border border-border bg-surface pr-7 pl-2.5 text-sm text-text shadow-sm hover:border-border-strong focus:border-accent focus:outline-none focus:ring-3 focus:ring-accent-soft [&>option]:bg-surface [&>option]:text-text";

/** Stacked label + control, for dialogs. */
export function Field({ label, hint, error, children }: { label: string; hint?: ReactNode; error?: string | null; children: ReactNode }) {
  return (
    <label className="block space-y-1.5">
      <span className="text-xs font-medium text-muted">{label}</span>
      {children}
      {error ? <span className="block text-xs text-err">{error}</span> : hint ? <span className="block text-xs text-faint">{hint}</span> : null}
    </label>
  );
}

/** Label on the left, control on the right: the main form layout. */
export function FormRow({ label, children, align = "center" }: { label: ReactNode; children: ReactNode; align?: "center" | "start" }) {
  return (
    <div className={`grid grid-cols-[112px_minmax(0,1fr)] gap-4 py-2.5 ${align === "start" ? "items-start" : "items-center"}`}>
      <div className={`text-sm text-muted ${align === "start" ? "pt-1.5" : ""}`}>{label}</div>
      <div className="min-w-0">{children}</div>
    </div>
  );
}

export function Segmented<T extends string>({
  value,
  onChange,
  options,
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: ReactNode }[];
}) {
  return (
    <div className="inline-flex rounded-md bg-surface-2 p-0.5" role="tablist">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="tab"
          aria-selected={value === o.value}
          onClick={() => onChange(o.value)}
          className={`inline-flex h-6 items-center gap-1.5 rounded px-2.5 text-xs font-medium transition-colors [&_svg]:size-3.5 ${
            value === o.value ? "bg-surface text-text shadow-sm" : "text-muted hover:text-text"
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

export type Tone = "ok" | "err" | "warn" | "accent" | "muted";

const DOT: Record<Tone, string> = {
  ok: "bg-ok",
  err: "bg-err",
  warn: "bg-warn",
  accent: "bg-accent",
  muted: "bg-faint",
};

export function Dot({ tone, pulse = false, className = "" }: { tone: Tone; pulse?: boolean; className?: string }) {
  return (
    <span className={`relative inline-flex size-2 shrink-0 ${className}`}>
      {pulse && <span className={`absolute inset-0 animate-ping rounded-full opacity-60 ${DOT[tone]}`} />}
      <span className={`relative inline-block size-2 rounded-full ${DOT[tone]}`} />
    </span>
  );
}

/** Dot + short word. Color carries the meaning; text stays neutral. */
export function Status({ tone, label, pulse, title }: { tone: Tone; label: ReactNode; pulse?: boolean; title?: string }) {
  return (
    <span className="inline-flex items-center gap-1.5 text-sm whitespace-nowrap" title={title}>
      <Dot tone={tone} pulse={pulse} />
      {label}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

export const TooltipProvider = TooltipPrimitive.Provider;

export function Tooltip({ content, children, side = "top" }: { content: ReactNode; children: ReactNode; side?: "top" | "bottom" | "left" | "right" }) {
  if (!content) return <>{children}</>;
  return (
    <TooltipPrimitive.Root>
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipPrimitive.Portal>
        <TooltipPrimitive.Content
          side={side}
          sideOffset={6}
          className="z-50 max-w-xs animate-fade rounded-md bg-text px-2 py-1 text-xs text-bg shadow-lg"
        >
          {content}
        </TooltipPrimitive.Content>
      </TooltipPrimitive.Portal>
    </TooltipPrimitive.Root>
  );
}

export const Menu = MenuPrimitive.Root;
export const MenuTrigger = MenuPrimitive.Trigger;

export function MenuContent({ children, align = "end" }: { children: ReactNode; align?: "start" | "end" }) {
  return (
    <MenuPrimitive.Portal>
      <MenuPrimitive.Content
        align={align}
        sideOffset={4}
        className="z-50 min-w-44 animate-pop rounded-lg border border-border bg-surface p-1 shadow-xl"
      >
        {children}
      </MenuPrimitive.Content>
    </MenuPrimitive.Portal>
  );
}

export function MenuItem({ children, onSelect, danger, disabled }: { children: ReactNode; onSelect: () => void; danger?: boolean; disabled?: boolean }) {
  return (
    <MenuPrimitive.Item
      disabled={disabled}
      onSelect={onSelect}
      className={`flex h-8 cursor-default items-center gap-2 rounded-md px-2 text-sm outline-none data-[disabled]:opacity-40 data-[highlighted]:bg-surface-2 [&_svg]:size-4 [&_svg]:text-faint ${
        danger ? "text-err [&_svg]:text-err" : ""
      }`}
    >
      {children}
    </MenuPrimitive.Item>
  );
}

export const MenuSeparator = () => <MenuPrimitive.Separator className="my-1 h-px bg-border" />;

/*
 * Dialogs and the side panel deliberately don't animate: WebKitGTK (Linux) can
 * skip the last repaint of an opacity animation and leave them half transparent.
 */
export function Modal({
  title,
  description,
  onClose,
  children,
  footer,
  dismissible = true,
  size = "md",
  bare = false,
}: {
  title: string;
  description?: ReactNode;
  onClose: () => void;
  children?: ReactNode;
  footer?: ReactNode;
  dismissible?: boolean;
  size?: "sm" | "md" | "lg" | "xl";
  /** Children fill a fixed-height body edge to edge (e.g. a wizard with its own footer). */
  bare?: boolean;
}) {
  const width = size === "xl" ? "max-w-4xl" : size === "lg" ? "max-w-2xl" : size === "sm" ? "max-w-sm" : "max-w-md";
  return (
    <DialogPrimitive.Root open onOpenChange={(open) => !open && dismissible && onClose()}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <DialogPrimitive.Content
          onEscapeKeyDown={(e) => !dismissible && e.preventDefault()}
          onPointerDownOutside={(e) => !dismissible && e.preventDefault()}
          className={`fixed top-1/2 left-1/2 z-50 flex max-h-[88vh] w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-xl border border-border bg-surface shadow-2xl ${width} ${bare ? "h-[min(760px,88vh)]" : ""}`}
        >
          <div className="flex shrink-0 items-start justify-between gap-4 px-5 pt-4 pb-3">
            <div className="min-w-0">
              <DialogPrimitive.Title className="text-base font-semibold">{title}</DialogPrimitive.Title>
              <DialogPrimitive.Description className={description ? "mt-0.5 text-sm text-muted" : "sr-only"}>
                {description ?? title}
              </DialogPrimitive.Description>
            </div>
            {dismissible && (
              <DialogPrimitive.Close asChild>
                <IconButton label="Close">
                  <X />
                </IconButton>
              </DialogPrimitive.Close>
            )}
          </div>
          {bare ? (
            <div className="min-h-0 flex-1 border-t border-border">{children}</div>
          ) : children ? (
            <div className="min-h-0 overflow-y-auto px-5 pb-5">{children}</div>
          ) : (
            <div className="pb-2" />
          )}
          {footer && <div className="flex shrink-0 justify-end gap-2 border-t border-border px-5 py-3">{footer}</div>}
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

/** Right-side panel for details, so the list behind stays in view. */
export function Sheet({ title, subtitle, onClose, children }: { title: ReactNode; subtitle?: ReactNode; onClose: () => void; children: ReactNode }) {
  return (
    <DialogPrimitive.Root open onOpenChange={(open) => !open && onClose()}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-black/30" />
        <DialogPrimitive.Content className="fixed top-0 right-0 bottom-0 z-50 flex w-[min(560px,92vw)] flex-col border-l border-border bg-bg shadow-2xl">
          <div className="flex h-[52px] shrink-0 items-center justify-between gap-3 border-b border-border px-5">
            <div className="min-w-0">
              <DialogPrimitive.Title className="truncate text-base font-semibold">{title}</DialogPrimitive.Title>
              <DialogPrimitive.Description className={subtitle ? "truncate text-xs text-muted" : "sr-only"}>{subtitle ?? "Details"}</DialogPrimitive.Description>
            </div>
            <DialogPrimitive.Close asChild>
              <IconButton label="Close">
                <X />
              </IconButton>
            </DialogPrimitive.Close>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto p-5">{children}</div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

// ---------------------------------------------------------------------------
// Layout & content
// ---------------------------------------------------------------------------

/** Every page starts with the same 52px header: title left, actions right. */
export function PageHeader({ title, meta, actions }: { title: ReactNode; meta?: ReactNode; actions?: ReactNode }) {
  return (
    <header className="flex h-[52px] shrink-0 items-center justify-between gap-4 border-b border-border px-5">
      <div className="flex min-w-0 items-baseline gap-3">
        <h1 className="truncate text-base font-semibold">{title}</h1>
        {meta && <div className="truncate text-sm text-muted">{meta}</div>}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </header>
  );
}

export function SectionTitle({ children, actions }: { children: ReactNode; actions?: ReactNode }) {
  return (
    <div className="flex h-8 items-center justify-between gap-3">
      <h2 className="text-sm font-semibold">{children}</h2>
      {actions && <div className="flex items-center gap-1.5">{actions}</div>}
    </div>
  );
}

/** Data table shell; rows are plain <tr>/<td> with the shared cell styles. */
export function Table({ head, children }: { head: ReactNode; children: ReactNode }) {
  return (
    <div className="overflow-hidden rounded-lg border border-border">
      <table className="w-full border-collapse text-sm [&_td]:px-3 [&_td]:py-2">
        <thead className="bg-surface-2/60 text-left text-xs text-muted">
          <tr className="[&>th]:h-8 [&>th]:px-3 [&>th]:font-medium">{head}</tr>
        </thead>
        <tbody className="[&>tr]:border-t [&>tr]:border-border">{children}</tbody>
      </table>
    </div>
  );
}

export function EmptyState({ icon, title, children, action }: { icon: ReactNode; title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-1.5 p-10 text-center">
      <div className="mb-1 text-faint [&_svg]:size-6">{icon}</div>
      <p className="text-sm font-medium">{title}</p>
      {children && <div className="max-w-xs text-sm text-muted">{children}</div>}
      {action && <div className="mt-3">{action}</div>}
    </div>
  );
}

export function ErrorNote({ children }: { children: ReactNode }) {
  return <div className="rounded-md bg-err-soft px-3 py-2 text-sm break-words text-err">{children}</div>;
}

export function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  return (
    <IconButton
      label={copied ? "Copied" : label}
      onClick={async () => {
        await copyText(text);
        setCopied(true);
        clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setCopied(false), 1500);
      }}
    >
      {copied ? <Check className="text-ok" /> : <Copy />}
    </IconButton>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="rounded border border-border bg-surface-2 px-1 font-sans text-[10.5px] text-muted">{children}</kbd>;
}
