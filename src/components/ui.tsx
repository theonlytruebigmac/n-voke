import { forwardRef, useEffect, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import * as MenuPrimitive from "@radix-ui/react-dropdown-menu";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { Check, Copy, Loader2, X } from "lucide-react";
import { copyText } from "../lib/format";

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

type Variant = "primary" | "tinted" | "secondary" | "ghost" | "danger" | "destructive";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent-fill text-on-accent hover:bg-accent-fill-hover shadow-sm",
  /** Stands out without competing with the one primary action on screen, e.g. the ways into each phase. */
  tinted: "border border-accent/40 bg-accent-soft text-accent hover:border-accent/70 hover:bg-accent/25 hover:text-accent-hover",
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
      <span className="block text-xs font-medium text-muted">{label}</span>
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
    <div className="inline-flex flex-wrap rounded-lg border border-border bg-bg p-0.5" role="tablist">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="tab"
          aria-selected={value === o.value}
          onClick={() => onChange(o.value)}
          className={`inline-flex h-7 items-center gap-1.5 rounded-md px-3 text-xs whitespace-nowrap transition-colors [&_svg]:size-3.5 ${
            value === o.value ? "bg-accent-soft font-semibold text-text" : "font-medium text-muted hover:text-text"
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
          onOpenAutoFocus={focusTheDialogNotClose}
          onEscapeKeyDown={(e) => !dismissible && e.preventDefault()}
          onPointerDownOutside={(e) => !dismissible && e.preventDefault()}
          className={`fixed top-1/2 left-1/2 z-50 flex max-h-[88vh] w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-xl border border-border bg-surface shadow-2xl focus:outline-none ${width} ${bare ? "h-[min(760px,88vh)]" : ""}`}
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

/**
 * A dialog focuses its first control when it opens, and that is the Close button, whose tooltip opens on focus as well
 * as on hover: it showed on every dialog with nobody pointing at it. Keep focus on a field that took it itself (an
 * autofocused input), otherwise on the dialog, so Tab and Escape still work from there.
 */
function focusTheDialogNotClose(e: Event) {
  const dialog = e.currentTarget as HTMLElement | null;
  if (!dialog || dialog.contains(document.activeElement)) return;
  e.preventDefault();
  dialog.focus();
}

/** Right-side panel for details, so the list behind stays in view. */
export function Sheet({ title, subtitle, onClose, children }: { title: ReactNode; subtitle?: ReactNode; onClose: () => void; children: ReactNode }) {
  return (
    <DialogPrimitive.Root open onOpenChange={(open) => !open && onClose()}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-black/30" />
        <DialogPrimitive.Content
          onOpenAutoFocus={focusTheDialogNotClose}
          className="fixed top-0 right-0 bottom-0 z-50 flex w-[min(560px,92vw)] flex-col border-l border-border bg-bg shadow-2xl focus:outline-none"
        >
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

/** Content width shared by page headers and bodies, so their edges line up. */
export const PAGE_WIDTH = "mx-auto w-full max-w-[1280px] px-8";

/**
 * Every page starts with the same header under the top menu: the title (with a short line under it), actions on the
 * right. With `crumbs` the title ends the trail on one line ("Projects / Contoso move / Track"): the way back muted,
 * where you are in bold. It sits on the page, not in a bar.
 */
export function PageHeader({ title, meta, actions, crumbs }: { title: ReactNode; meta?: ReactNode; actions?: ReactNode; crumbs?: ReactNode }) {
  return (
    <header className={`${PAGE_WIDTH} flex shrink-0 flex-wrap items-end gap-x-4 gap-y-3 pt-6 pb-5`}>
      <div className="min-w-0 flex-[1_1_320px]">
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 text-xl">
          {crumbs && (
            <nav aria-label="Breadcrumb" className="contents text-muted">
              {crumbs}
            </nav>
          )}
          <h1 className="min-w-0 truncate font-semibold">{title}</h1>
        </div>
        {meta && <div className="mt-0.5 text-sm text-muted">{meta}</div>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </header>
  );
}

/** A breadcrumb link and the slash after it, for PageHeader's `crumbs`. */
export function Crumb({ children, onClick }: { children: ReactNode; onClick: () => void }) {
  return (
    <>
      <button type="button" onClick={onClick} className="max-w-[24ch] truncate text-muted hover:text-text" title={typeof children === "string" ? children : undefined}>
        {children}
      </button>
      <span className="text-border-strong" aria-hidden="true">
        /
      </span>
    </>
  );
}

/** The scrolling area under a PageHeader, at the shared content width. */
export function PageBody({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className={`${PAGE_WIDTH} pb-8 ${className}`}>{children}</div>
    </div>
  );
}

/** The rounded panel most content sits in. */
export function Card({ children, className = "", tone }: { children: ReactNode; className?: string; tone?: "warn" | "err" | "accent" }) {
  const toneClass =
    tone === "warn" ? "border-warn/30 bg-warn-soft/40" : tone === "err" ? "border-err/30 bg-err-soft/40" : tone === "accent" ? "border-accent/40 bg-accent-soft" : "border-border bg-surface";
  return <div className={`rounded-2xl border ${toneClass} ${className}`}>{children}</div>;
}

/** The bar over a list while rows are ticked: how many, what can be done to them, and a way to clear the ticks. */
export function SelectionBar({ count, label, children, onClear }: { count: number; label?: ReactNode; children: ReactNode; onClear: () => void }) {
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-xl border border-accent/30 bg-accent-soft px-3 py-2">
      <span className="mr-1 flex-1 font-semibold">{label ?? `${count.toLocaleString()} selected`}</span>
      {children}
      <IconButton label="Clear selection" onClick={onClear}>
        <X />
      </IconButton>
    </div>
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

export type StepState = "done" | "current" | "attention" | "upcoming";

export interface TimelineStep {
  key: string;
  state: StepState;
  content: ReactNode;
}

const STEP_MARKER: Record<StepState, string> = {
  done: "bg-ok-soft text-ok",
  current: "bg-accent-fill text-on-accent ring-4 ring-accent-soft",
  attention: "border-2 border-warn text-warn",
  upcoming: "border-2 border-border-strong text-faint",
};

/**
 * Numbered steps down a rail: a check once done, the current one filled, one that needs a look outlined amber.
 * The rail between two steps turns green once the step above it is done.
 */
export function Timeline({ steps, label }: { steps: TimelineStep[]; label: string }) {
  return (
    <ol aria-label={label} className="rounded-2xl border border-border bg-surface px-5 py-1.5">
      {steps.map((s, i) => {
        const prevDone = i > 0 && steps[i - 1].state === "done";
        return (
          <li key={s.key} className="flex gap-4" aria-current={s.state === "current" ? "step" : undefined}>
            <div className="flex w-7 shrink-0 flex-col items-center">
              <span className={`h-4 w-0.5 ${i === 0 ? "" : prevDone ? "bg-ok/50" : "bg-border-strong"}`} />
              <span className={`flex size-7 shrink-0 items-center justify-center rounded-full text-xs font-semibold tabular-nums ${STEP_MARKER[s.state]}`}>
                {s.state === "done" ? <Check className="size-3.5" strokeWidth={3} /> : i + 1}
              </span>
              <span className={`w-0.5 flex-1 ${i === steps.length - 1 ? "" : s.state === "done" ? "bg-ok/50" : "bg-border-strong"}`} />
            </div>
            <div className="min-w-0 flex-1 py-4">{s.content}</div>
          </li>
        );
      })}
    </ol>
  );
}

/** A finished step folded to one line: what was chosen, and a way back to change it. */
export function StepSummary({ label, children, onChange }: { label: string; children: ReactNode; onChange?: () => void }) {
  return (
    <div className="flex min-h-7 flex-wrap items-center gap-x-3 gap-y-1">
      <span className="w-28 shrink-0 text-muted">{label}</span>
      <span className="min-w-0 flex-1 truncate font-semibold">{children}</span>
      {onChange && (
        <button type="button" onClick={onChange} className="text-sm font-medium text-accent hover:text-accent-hover">
          Change
        </button>
      )}
    </div>
  );
}

/** The heading of the step being worked on, with a short hint beside it. */
export function StepTitle({ children, hint }: { children: ReactNode; hint?: ReactNode }) {
  return (
    <div className="flex min-h-7 flex-wrap items-baseline gap-x-3 gap-y-0.5">
      <h2 className="text-base font-semibold">{children}</h2>
      {hint && <span className="text-sm text-faint">{hint}</span>}
    </div>
  );
}

/** Data table shell; rows are plain <tr>/<td> with the shared cell styles. */
export function Table({ head, children }: { head: ReactNode; children: ReactNode }) {
  return (
    <div className="overflow-x-auto rounded-xl border border-border bg-surface">
      <table className="w-full border-collapse text-sm [&_td]:px-3 [&_td]:py-2">
        <thead className="text-left text-xs text-faint">
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
