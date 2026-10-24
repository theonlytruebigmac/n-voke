import { MoreHorizontal, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";
import type { DeviceChoice, PlanAdded, PlanRow, PlanShow } from "../../../lib/migrate/bindings";
import { chooseForDevice } from "../../../lib/migrate/api";
import { BLOCKER_WORDS, rowNote } from "../../../lib/migrate/plan";
import { usePlan, usePlanActions, useRereadPlan } from "../../../lib/migrate/queries";
import { useDebounced } from "../../../lib/useDebounced";
import { Button, Card, ErrorNote, IconButton, Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger, Segmented, SelectionBar, Status, Table, inputClass } from "../../ui";
import { BulkStatus, useBulk } from "../move/bulk";
import { FilterPicker } from "../move/FilterPicker";
import type { StepId } from "../parts/StepRail";

const PAGE = 100;
const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
const PATH_WORDS = { ecoverse: "Ecoverse", rest: "N-central" } as const;

/** A device that has launched is past choosing. */
const choosable = (r: PlanRow) => !r.stage || r.stage === "planned";

/** What can be asked of many devices at once, and which of the ticked rows each applies to. */
const BULK: { label: string; choice: DeviceChoice; applies: (r: PlanRow) => boolean }[] = [
  { label: "Use Ecoverse", choice: { kind: "usePath", path: "ecoverse" }, applies: (r) => r.path !== "ecoverse" },
  { label: "Use N-central", choice: { kind: "usePath", path: "rest" }, applies: (r) => r.path !== "rest" },
  { label: "Suggested path", choice: { kind: "suggestedPath" }, applies: (r) => r.pathChosen },
  { label: "Leave out", choice: { kind: "leaveOut" }, applies: (r) => !r.leftOutByChoice },
  { label: "Include again", choice: { kind: "include" }, applies: (r) => r.leftOutByChoice },
];

function Stat({ value, label, tone = "" }: { value: number; label: string; tone?: string }) {
  return (
    <Card className="px-4 py-3">
      <div className={`text-2xl font-bold tabular-nums ${tone}`}>{value.toLocaleString()}</div>
      <div className="text-sm text-muted">{label}</div>
    </Card>
  );
}

/** The devices as the plan sees them, and adding the ready ones to the project (step 4 in docs/migration/11-gui-design.md). */
export function PlanStep({ projectId, onGo }: { projectId: string; onGo: (step: StepId) => void }) {
  const { add, choose } = usePlanActions(projectId);
  const bulk = useBulk(projectId);
  const [show, setShow] = useState<PlanShow>("all");
  const [customer, setCustomer] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [offset, setOffset] = useState(0);
  const [added, setAdded] = useState<PlanAdded | null>(null);
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const search = useDebounced(text.trim());
  const query = useMemo(() => ({ show, customer, text: search, offset, limit: PAGE }), [show, customer, search, offset]);
  const plan = usePlan(projectId, query);
  const reread = useRereadPlan(projectId, query);

  const view = plan.data;
  const rows = view?.rows ?? [];
  const customers = useMemo(() => (view?.customers ?? []).map((c) => ({ key: c, label: c })), [view?.customers]);
  const choosableRows = rows.filter(choosable);
  const tickedRows = choosableRows.filter((r) => ticked.has(r.deviceId));
  const allTicked = choosableRows.length > 0 && tickedRows.length === choosableRows.length;

  const blockerFixes = view?.blockedBy ?? [];
  const readAgain = () => {
    setAdded(null);
    reread.mutate();
  };
  const narrow = (change: () => void) => {
    change();
    setOffset(0);
    setTicked(new Set());
  };
  const toggle = (id: string) =>
    setTicked((cur) => {
      const next = new Set(cur);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const runBulk = (b: (typeof BULK)[number]) => {
    const targets = tickedRows.filter(b.applies);
    void bulk
      .run(
        b.label,
        targets,
        (r) => r.name,
        (r) => chooseForDevice(projectId, r.deviceId, b.choice),
      )
      .then(() => setTicked(new Set()));
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-end gap-3">
        <Button onClick={readAgain} busy={reread.isPending}>
          <RefreshCw />
          Read servers again
        </Button>
        {view && view.readyNew === 0 ? (
          // A greyed-out "Add to the project" read as if something were wrong; say what's true instead.
          <span className="text-sm text-muted">Every ready device is in the project</span>
        ) : (
          <Button variant="primary" busy={add.isPending} disabled={!view} onClick={() => add.mutate(undefined, { onSuccess: (r) => setAdded(r) })}>
            {view ? `Add ${plural(view.readyNew, "device")} to the project` : "Add to the project"}
          </Button>
        )}
      </div>

      {reread.isError && <ErrorNote>{String(reread.error)}</ErrorNote>}
      {plan.isError ? (
        <ErrorNote>{String(plan.error)}</ErrorNote>
      ) : !view ? (
        <p className="text-sm text-muted">Reading both servers and Ecoverse. This takes a moment on a large server…</p>
      ) : (
        <>
          <div className="flex flex-wrap items-stretch gap-4">
            <div className="grid min-w-0 flex-[1_1_380px] grid-cols-[repeat(auto-fit,minmax(150px,1fr))] gap-3">
              <Stat value={view.total} label="devices on the source" />
              <Card className="px-4 py-3">
                <div className="text-2xl font-bold text-ok tabular-nums">{view.readyNew.toLocaleString()}</div>
                <div className="text-sm text-muted">ready</div>
                {view.readyNew > 0 && (
                  <div className="text-xs text-faint">
                    {view.readyEcoverse.toLocaleString()} through Ecoverse, {view.readyRest.toLocaleString()} through N-central
                  </div>
                )}
              </Card>
              <Stat value={view.blocked} label="blocked" tone={view.blocked ? "text-warn" : ""} />
              <Stat value={view.leftOut} label="left out" tone="text-muted" />
              <Stat value={view.inProject} label="in the project" />
            </div>

            {blockerFixes.length > 0 && (
              <Card tone="warn" className="flex-[1_1_380px] px-4 py-3">
                <h3 className="mb-1 font-semibold text-warn">{plural(view.blocked, "device")} blocked · by reason</h3>
                <ul className="divide-y divide-warn/20">
                  {blockerFixes.map(({ blocker, devices }) => {
                    const words = BLOCKER_WORDS[blocker];
                    return (
                      <li key={blocker} className="flex flex-wrap items-center gap-x-3 gap-y-1.5 py-2">
                        <span className="w-14 font-bold tabular-nums">{devices.toLocaleString()}</span>
                        <span className="min-w-40 flex-1">
                          {words.short}
                          <span className="block text-xs text-faint">{words.text}</span>
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
              </Card>
            )}
          </div>

          {view.warnings.map((w) => (
            <p key={w} className="text-xs text-warn">
              {w}
            </p>
          ))}

          {add.isError && <ErrorNote>{String(add.error)}</ErrorNote>}
          {choose.isError && <ErrorNote>{String(choose.error)}</ErrorNote>}
          {added && (
            <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-ok/30 bg-ok-soft px-4 py-2.5 text-sm">
              <span>
                Added {plural(added.added, "device")}
                {added.alreadyIn > 0 && `, ${added.alreadyIn.toLocaleString()} were already in`}
                {added.blocked > 0 && `. ${added.blocked.toLocaleString()} stay out until what blocks them is fixed`}.
              </span>
              <Button size="sm" variant="primary" onClick={() => onGo("launch")}>
                Continue to Launch
              </Button>
            </div>
          )}

          <div className="flex flex-wrap items-center gap-2">
            <Segmented
              value={show}
              onChange={(v) => narrow(() => setShow(v))}
              options={[
                { value: "all", label: "All" },
                { value: "ready", label: "Ready" },
                { value: "blocked", label: "Blocked" },
              ]}
            />
            <FilterPicker label="Customer" anyLabel="All customers" options={customers} value={customer} onChange={(c) => narrow(() => setCustomer(c))} />
            <input className={`${inputClass} max-w-64 flex-[1_1_180px]`} placeholder="Search devices" value={text} onChange={(e) => narrow(() => setText(e.target.value))} aria-label="Search devices" />
          </div>

          {tickedRows.length > 0 && (
            <SelectionBar count={tickedRows.length} onClear={() => setTicked(new Set())}>
              {BULK.map((b) => {
                const n = tickedRows.filter(b.applies).length;
                return (
                  <Button key={b.label} size="sm" disabled={n === 0 || bulk.busy} onClick={() => runBulk(b)} title={n < tickedRows.length ? `Applies to ${n} of the ticked devices` : undefined}>
                    {b.label}
                    {n > 0 && n < tickedRows.length && <span className="text-faint tabular-nums">{n}</span>}
                  </Button>
                );
              })}
            </SelectionBar>
          )}
          <BulkStatus state={bulk.state} onClear={bulk.clear} />

          {rows.length === 0 ? (
            <p className="text-sm text-muted">{view.total === 0 ? "The source server has no devices." : "No devices match."}</p>
          ) : (
            <>
              <Table
                head={
                  <>
                    <th className="w-9">
                      <input
                        type="checkbox"
                        className="size-3.5 accent-accent"
                        checked={allTicked}
                        disabled={choosableRows.length === 0}
                        ref={(el) => {
                          if (el) el.indeterminate = tickedRows.length > 0 && !allTicked;
                        }}
                        onChange={() => setTicked(allTicked ? new Set() : new Set(choosableRows.map((r) => r.deviceId)))}
                        aria-label="Tick every device on this page"
                      />
                    </th>
                    <th>Device</th>
                    <th>Customer → destination</th>
                    <th>Path</th>
                    <th>Notes</th>
                    <th className="w-10" />
                  </>
                }
              >
                {rows.map((r) => (
                  <PlanRowView
                    key={r.deviceId}
                    row={r}
                    ticked={ticked.has(r.deviceId)}
                    onTick={() => toggle(r.deviceId)}
                    busy={choose.isPending || bulk.busy}
                    onChoose={(choice) => choose.mutate({ deviceId: r.deviceId, choice })}
                  />
                ))}
              </Table>
              <div className="flex items-center justify-between gap-3 text-xs text-muted">
                <span className="tabular-nums">
                  {(view.offset + 1).toLocaleString()}–{(view.offset + rows.length).toLocaleString()} of {view.matching.toLocaleString()}
                </span>
                {view.matching > PAGE && (
                  <span className="flex items-center gap-2">
                    <Button size="sm" disabled={offset === 0} onClick={() => (setOffset(Math.max(0, offset - PAGE)), setTicked(new Set()))}>
                      Previous
                    </Button>
                    <Button size="sm" disabled={offset + PAGE >= view.matching} onClick={() => (setOffset(offset + PAGE), setTicked(new Set()))}>
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
  if (!choosable(row)) return null;
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

function PlanRowView({ row, ticked, onTick, busy, onChoose }: { row: PlanRow; ticked: boolean; onTick: () => void; busy: boolean; onChoose: (choice: DeviceChoice) => void }) {
  const note = rowNote(row);
  const route = `${row.customer}${row.site ? ` · ${row.site}` : ""} → ${row.destination ?? "–"}`;
  return (
    <tr className={ticked ? "bg-accent-soft/60" : undefined}>
      <td>{choosable(row) && <input type="checkbox" className="size-3.5 accent-accent" checked={ticked} onChange={onTick} aria-label={`Tick ${row.name}`} />}</td>
      <td className="max-w-56 font-medium whitespace-nowrap">
        <span className="flex items-center gap-2">
          <span className="truncate" title={row.name}>
            {row.name}
          </span>
          {row.stage && <span className="shrink-0 text-xs font-normal text-faint">in the project</span>}
        </span>
      </td>
      <td className="max-w-0 truncate text-muted" title={route}>
        {route}
      </td>
      <td className="whitespace-nowrap">
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
