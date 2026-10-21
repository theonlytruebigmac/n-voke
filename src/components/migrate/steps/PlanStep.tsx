import { MoreHorizontal, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";
import type { DeviceChoice, PlanAdded, PlanRow, PlanShow } from "../../../lib/migrate/bindings";
import { BLOCKER_WORDS, rowNote } from "../../../lib/migrate/plan";
import { usePlan, usePlanActions, useRereadPlan } from "../../../lib/migrate/queries";
import { useDebounced } from "../../../lib/useDebounced";
import { Button, ErrorNote, IconButton, Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger, Segmented, Status, Table, inputClass, selectClass } from "../../ui";
import type { StepId } from "../parts/StepRail";

const PAGE = 100;
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const PATH_WORDS = { ecoverse: "Ecoverse", rest: "N-central" } as const;

/** The devices as the plan sees them, and adding the ready ones to the project (step 4 in docs/migration/11-gui-design.md). */
export function PlanStep({ projectId, onGo }: { projectId: string; onGo: (step: StepId) => void }) {
  const { add, choose } = usePlanActions(projectId);
  const [show, setShow] = useState<PlanShow>("all");
  const [customer, setCustomer] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [offset, setOffset] = useState(0);
  const [added, setAdded] = useState<PlanAdded | null>(null);
  const search = useDebounced(text.trim());
  const query = useMemo(() => ({ show, customer, text: search, offset, limit: PAGE }), [show, customer, search, offset]);
  const plan = usePlan(projectId, query);
  const reread = useRereadPlan(projectId, query);

  const view = plan.data;
  const rows = view?.rows ?? [];
  const customers = view?.customers ?? [];

  const blockerFixes = view?.blockedBy ?? [];
  const readAgain = () => {
    setAdded(null);
    reread.mutate();
  };
  const narrow = (change: () => void) => {
    change();
    setOffset(0);
  };

  return (
    <div className="space-y-4 px-6 py-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-semibold">Plan</h2>
        <div className="flex items-center gap-2">
          <Button size="sm" onClick={readAgain} busy={reread.isPending}>
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

      {reread.isError && <ErrorNote>{String(reread.error)}</ErrorNote>}
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
          {choose.isError && <ErrorNote>{String(choose.error)}</ErrorNote>}
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
              onChange={(v) => narrow(() => setShow(v))}
              options={[
                { value: "all", label: "All" },
                { value: "ready", label: "Ready" },
                { value: "blocked", label: "Blocked" },
              ]}
            />
            <div className="flex items-center gap-2">
              <select className={`${selectClass} w-44`} value={customer ?? ""} onChange={(e) => narrow(() => setCustomer(e.target.value || null))} aria-label="Customer">
                <option value="">All customers</option>
                {customers.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
              <input className={`${inputClass} w-44`} placeholder="Search" value={text} onChange={(e) => narrow(() => setText(e.target.value))} aria-label="Search devices" />
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
                    <th className="w-10" />
                  </>
                }
              >
                {rows.map((r) => (
                  <PlanRowView key={r.deviceId} row={r} busy={choose.isPending} onChoose={(choice) => choose.mutate({ deviceId: r.deviceId, choice })} />
                ))}
              </Table>
              <div className="flex items-center justify-between gap-3 text-xs text-muted">
                <span>
                  {view.offset + 1}–{view.offset + rows.length} of {view.matching}
                </span>
                {view.matching > PAGE && (
                  <span className="flex items-center gap-2">
                    <Button size="sm" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - PAGE))}>
                      Previous
                    </Button>
                    <Button size="sm" disabled={offset + PAGE >= view.matching} onClick={() => setOffset(offset + PAGE)}>
                      Next
                    </Button>
                  </span>
                )}
              </div>
            </>
          )}
        </>
      )}
    </div>
  );
}

/** What the person can say about one device. A device that has launched is past choosing. */
function ChoiceMenu({ row, busy, onChoose }: { row: PlanRow; busy: boolean; onChoose: (choice: DeviceChoice) => void }) {
  if (row.stage && row.stage !== "planned") return null;
  const items: { label: string; choice: DeviceChoice; danger?: boolean }[] = [];
  if (row.path !== "ecoverse") items.push({ label: "Use Ecoverse", choice: { kind: "usePath", path: "ecoverse" } });
  if (row.path !== "rest") items.push({ label: "Use N-central", choice: { kind: "usePath", path: "rest" } });
  if (row.pathChosen) items.push({ label: "Use the suggested path", choice: { kind: "suggestedPath" } });
  const leave: { label: string; choice: DeviceChoice } = row.leftOutByChoice ? { label: "Include again", choice: { kind: "include" } } : { label: "Leave out", choice: { kind: "leaveOut" } };
  return (
    <Menu>
      <MenuTrigger asChild>
        <IconButton label={`Choices for ${row.name}`} disabled={busy}>
          <MoreHorizontal />
        </IconButton>
      </MenuTrigger>
      <MenuContent>
        {items.map((i) => (
          <MenuItem key={i.label} onSelect={() => onChoose(i.choice)}>
            {i.label}
          </MenuItem>
        ))}
        {items.length > 0 && <MenuSeparator />}
        <MenuItem onSelect={() => onChoose(leave.choice)}>{leave.label}</MenuItem>
      </MenuContent>
    </Menu>
  );
}

function PlanRowView({ row, busy, onChoose }: { row: PlanRow; busy: boolean; onChoose: (choice: DeviceChoice) => void }) {
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
      <td>
        {row.path ? PATH_WORDS[row.path] : <span className="text-faint">–</span>}
        {row.pathChosen && <span className="ml-1.5 text-xs text-faint">your choice</span>}
      </td>
      <td className="max-w-xs">
        <Status tone={note.tone === "ok" ? "ok" : note.tone === "err" ? "err" : note.tone === "warn" ? "warn" : "muted"} label={<span className="whitespace-normal">{note.text}</span>} />
      </td>
      <td className="text-right">
        <ChoiceMenu row={row} busy={busy} onChoose={onChoose} />
      </td>
    </tr>
  );
}
