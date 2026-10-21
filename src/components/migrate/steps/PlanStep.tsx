import { RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";
import type { PlanAdded, PlanRow } from "../../../lib/migrate/bindings";
import { BLOCKER_WORDS, filterPlan, planCustomers, rowNote, type PlanShow } from "../../../lib/migrate/plan";
import { usePlan, usePlanActions } from "../../../lib/migrate/queries";
import { Button, ErrorNote, Segmented, Status, Table, inputClass, selectClass } from "../../ui";
import type { StepId } from "../parts/StepRail";

const PAGE = 200;
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const PATH_WORDS = { ecoverse: "Ecoverse", rest: "N-central" } as const;

/** The devices as the plan sees them, and adding the ready ones to the project (step 4 in docs/migration/11-gui-design.md). */
export function PlanStep({ projectId, onGo }: { projectId: string; onGo: (step: StepId) => void }) {
  const plan = usePlan(projectId);
  const { add } = usePlanActions(projectId);
  const [show, setShow] = useState<PlanShow>("all");
  const [customer, setCustomer] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [shown, setShown] = useState(PAGE);
  const [added, setAdded] = useState<PlanAdded | null>(null);

  const view = plan.data;
  const rows = useMemo(() => (view ? filterPlan(view.rows, { show, customer, text }) : []), [view, show, customer, text]);
  const customers = useMemo(() => (view ? planCustomers(view.rows) : []), [view]);

  const blockerFixes = view?.blockedBy ?? [];
  const reread = () => {
    setAdded(null);
    plan.refetch();
  };

  return (
    <div className="space-y-4 px-6 py-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-semibold">Plan</h2>
        <div className="flex items-center gap-2">
          <Button size="sm" onClick={reread} busy={plan.isFetching}>
            <RefreshCw />
            Read servers again
          </Button>
          <Button
            variant="primary"
            size="sm"
            busy={add.isPending}
            disabled={!view || view.readyNew === 0}
            onClick={() => add.mutate(undefined, { onSuccess: (r) => setAdded(r) })}
          >
            {view && view.readyNew > 0 ? `Add ${plural(view.readyNew, "device")} to the project` : "Add to the project"}
          </Button>
        </div>
      </div>

      {plan.isError ? (
        <ErrorNote>{String(plan.error)}</ErrorNote>
      ) : !view ? (
        <p className="text-sm text-muted">Reading both servers{" "}and Ecoverse. This takes a moment on a large server…</p>
      ) : (
        <>
          <p className="text-sm text-muted">
            {plural(view.total, "device")} · <span className="text-ok">{view.readyNew} ready</span> ({view.readyEcoverse} through Ecoverse, {view.readyRest} through N-central) ·{" "}
            <span className={view.blocked ? "text-warn" : ""}>{view.blocked} blocked</span> · {view.leftOut} left out · {view.inProject} already in the project
          </p>

          {view.warnings.map((w) => (
            <p key={w} className="text-xs text-warn">
              {w}
            </p>
          ))}

          {add.isError && <ErrorNote>{String(add.error)}</ErrorNote>}
          {added && (
            <div className="flex items-center justify-between gap-3 rounded-lg border border-ok/40 bg-ok-soft px-3 py-2 text-sm">
              <span>
                Added {plural(added.added, "device")}
                {added.alreadyIn > 0 && `, ${added.alreadyIn} were already in`}
                {added.blocked > 0 && `. ${added.blocked} stay out until what blocks them is fixed`}.
              </span>
              <Button size="sm" variant="primary" onClick={() => onGo("launch")}>
                Continue to Launch
              </Button>
            </div>
          )}

          {blockerFixes.length > 0 && (
            <ul className="space-y-1 rounded-lg border border-border px-3 py-2">
              {blockerFixes.map(({ blocker, devices }) => {
                const words = BLOCKER_WORDS[blocker];
                return (
                  <li key={blocker} className="flex items-center justify-between gap-3 text-sm">
                    <span>
                      <span className="font-medium">{plural(devices, "device")}</span> · {words.short}
                      <span className="block text-xs text-muted">{words.text}</span>
                    </span>
                    {words.fix && (
                      <Button size="sm" onClick={() => onGo(words.fix as StepId)}>
                        Fix in {words.fix === "map" ? "Map" : "Prepare"}
                      </Button>
                    )}
                  </li>
                );
              })}
            </ul>
          )}

          <div className="flex flex-wrap items-center justify-between gap-3">
            <Segmented
              value={show}
              onChange={(v) => {
                setShow(v);
                setShown(PAGE);
              }}
              options={[
                { value: "all", label: "All" },
                { value: "ready", label: "Ready" },
                { value: "blocked", label: "Blocked" },
              ]}
            />
            <div className="flex items-center gap-2">
              <select className={`${selectClass} w-44`} value={customer ?? ""} onChange={(e) => setCustomer(e.target.value || null)} aria-label="Customer">
                <option value="">All customers</option>
                {customers.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
              <input className={`${inputClass} w-44`} placeholder="Search" value={text} onChange={(e) => setText(e.target.value)} aria-label="Search devices" />
            </div>
          </div>

          {rows.length === 0 ? (
            <p className="text-sm text-muted">{view.total === 0 ? "The source server has no devices." : "No devices match."}</p>
          ) : (
            <>
              <Table
                head={
                  <>
                    <th>Device</th>
                    <th>Customer → destination</th>
                    <th>Path</th>
                    <th>Notes</th>
                  </>
                }
              >
                {rows.slice(0, shown).map((r) => (
                  <PlanRowView key={r.deviceId} row={r} />
                ))}
              </Table>
              {rows.length > shown && (
                <Button size="sm" onClick={() => setShown(shown + PAGE)}>
                  Show more ({rows.length - shown} left)
                </Button>
              )}
            </>
          )}
        </>
      )}
    </div>
  );
}

function PlanRowView({ row }: { row: PlanRow }) {
  const note = rowNote(row);
  return (
    <tr>
      <td className="font-medium">
        {row.name}
        {row.stage && <span className="ml-2 text-xs font-normal text-faint">in the project</span>}
      </td>
      <td className="text-muted">
        {row.customer}
        {row.site ? ` · ${row.site}` : ""} → {row.destination ?? "–"}
      </td>
      <td>{row.path ? PATH_WORDS[row.path] : <span className="text-faint">–</span>}</td>
      <td className="max-w-xs">
        <Status tone={note.tone === "ok" ? "ok" : note.tone === "err" ? "err" : note.tone === "warn" ? "warn" : "muted"} label={<span className="whitespace-normal">{note.text}</span>} />
      </td>
    </tr>
  );
}
