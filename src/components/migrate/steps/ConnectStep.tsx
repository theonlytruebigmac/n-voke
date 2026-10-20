import { RefreshCw } from "lucide-react";
import type { ConnectItem } from "../../../lib/migrate/bindings";
import { useConnectCheck } from "../../../lib/migrate/queries";
import { Button, Dot, ErrorNote } from "../../ui";

const TONE = { ok: "ok", warn: "warn", err: "err" } as const;

function Item({ item }: { item: ConnectItem }) {
  return (
    <li className="flex gap-3 py-3">
      <Dot tone={TONE[item.level]} className="mt-1.5" />
      <div className="min-w-0">
        <div className="text-sm font-medium">{item.title}</div>
        <div className="text-sm break-words text-muted">{item.detail}</div>
      </div>
    </li>
  );
}

/** Is everything the project depends on reachable and signed in? (step 1 in docs/migration/11-gui-design.md) */
export function ConnectStep({ projectId }: { projectId: string }) {
  const check = useConnectCheck(projectId);
  const report = check.data;

  return (
    <div className="mx-auto max-w-2xl space-y-4 px-6 py-5">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-sm font-semibold">Connect</h2>
        <Button size="sm" onClick={() => check.refetch()} busy={check.isFetching}>
          <RefreshCw />
          Check again
        </Button>
      </div>

      {check.isError ? (
        <ErrorNote>{String(check.error)}</ErrorNote>
      ) : !report ? (
        <p className="text-sm text-muted">Checking the servers…</p>
      ) : (
        <>
          <ul className="divide-y divide-border rounded-lg border border-border px-4">
            {report.items.map((i) => (
              <Item key={i.id} item={i} />
            ))}
          </ul>
          <p className={`text-sm ${report.canContinue ? "text-ok" : "text-err"}`}>
            {report.canContinue ? "Everything this project needs is reachable. Mapping customers and sites is the next step." : "Fix the red items before going on, then check again."}
          </p>
        </>
      )}
    </div>
  );
}
