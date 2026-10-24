import { Check, RefreshCw, X } from "lucide-react";
import type { ConnectItem } from "../../../lib/migrate/bindings";
import { useConnectCheck } from "../../../lib/migrate/queries";
import { Button, Card, ErrorNote } from "../../ui";

const MARK = {
  ok: { className: "bg-ok-soft text-ok", icon: <Check className="size-3.5" strokeWidth={3} /> },
  warn: { className: "bg-warn-soft text-warn", icon: <span className="text-xs font-bold">!</span> },
  err: { className: "bg-err-soft text-err", icon: <X className="size-3.5" strokeWidth={3} /> },
} as const;

function Item({ item }: { item: ConnectItem }) {
  const mark = MARK[item.level];
  return (
    <li className="flex gap-3.5 px-5 py-3.5">
      <span className={`mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-full ${mark.className}`}>{mark.icon}</span>
      <div className="min-w-0">
        <div className="font-semibold">{item.title}</div>
        <div className="text-sm wrap-break-word text-muted">{item.detail}</div>
      </div>
    </li>
  );
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Is everything the project depends on reachable and signed in? (step 1 in docs/migration/11-gui-design.md) */
export function ConnectStep({ projectId, onContinue }: { projectId: string; onContinue: () => void }) {
  const check = useConnectCheck(projectId);
  const report = check.data;
  const count = (level: ConnectItem["level"]) => report?.items.filter((i) => i.level === level).length ?? 0;
  const errors = count("err");
  const warnings = count("warn");

  return (
    <div className="space-y-4">
      {check.isError ? (
        <ErrorNote>{String(check.error)}</ErrorNote>
      ) : !report ? (
        <p className="text-sm text-muted">Checking the servers…</p>
      ) : (
        <>
          <Card tone={errors ? "err" : warnings ? "warn" : undefined} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-5 py-3.5">
            <span className={`flex-[1_1_260px] font-semibold ${errors ? "text-err" : report.canContinue ? "text-ok" : "text-warn"}`}>
              {errors ? `${plural(errors, "thing")} to fix before you go on` : report.canContinue ? "Everything this project needs is reachable" : "Not ready yet"}
            </span>
            <span className="text-muted">
              {count("ok")} OK{warnings > 0 && ` · ${plural(warnings, "warning")}`}
              {errors > 0 && ` · ${errors} to fix`}
            </span>
            <Button onClick={() => check.refetch()} busy={check.isFetching}>
              <RefreshCw />
              Check again
            </Button>
          </Card>

          <Card>
            <ul className="divide-y divide-border">
              {report.items.map((i) => (
                <Item key={i.id} item={i} />
              ))}
            </ul>
          </Card>

          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="text-sm text-muted">
              {report.canContinue ? "Mapping customers and sites is the next step." : "Fix the red items before going on, then check again."}
            </p>
            <Button variant="primary" onClick={onContinue} disabled={!report.canContinue}>
              Continue to Map
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
