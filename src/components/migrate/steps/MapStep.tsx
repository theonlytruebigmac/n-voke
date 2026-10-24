import { forgetReads } from "../../../lib/migrate/api";
import { ChevronDown, ChevronRight, KeyRound, PlusSquare, RefreshCw, Search } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { MapDetails, MapRow, MapView, TokenReport, TokenRow, UnitKind, UnitRef } from "../../../lib/migrate/bindings";
import { MATCH_WORDS, TOKEN_WORDS, mapCounts, missingTokens, needsChoice, tokenCell, visibleRows } from "../../../lib/migrate/map";
import { useMap, useMapActions, useMapDetails } from "../../../lib/migrate/queries";
import { Popover, PopoverContent, PopoverTrigger, SearchList } from "../../Combobox";
import { Button, ErrorNote, Menu, MenuContent, MenuItem, MenuTrigger, Segmented, SelectionBar, Table, Tooltip, inputClass } from "../../ui";
import { DestPicker } from "../parts/DestPicker";
import { StructureDialog } from "../parts/StructureDialog";
import { GenerateTokensDialog, TokenResultDialog } from "../parts/TokenDialogs";

const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

/** What a row's status cell says: why it needs a choice, how it was matched, or that it's left out. */
function statusOf(row: MapRow, details: MapDetails | undefined, loading: boolean): { text: string; tone: "warn" | "err" | "muted" | "ok"; hint?: string } {
  const kind = row.kind === "site" ? "site" : "customer";
  if (row.status === "unmatched") return { text: `No ${kind} with this name`, tone: "warn" };
  if (row.status === "ambiguous") return { text: `${row.candidates.length} possible matches`, tone: "warn", hint: "More than one fits equally well. Choose one." };
  if (row.status === "leftOut") return { text: "Left out", tone: "muted" };
  const how = row.how ?? "exact";
  const base =
    how === "exact"
      ? { text: "Matched by name", tone: "muted" as const }
      : how === "chosen"
        ? { text: "Your choice", tone: "muted" as const }
        : { text: how === "similar" ? "Similar name, check it" : "Same name, other customer", tone: "warn" as const };
  // A token only gets a mention when something is wrong with it.
  const cell = tokenCell(row, details, loading);
  if (cell.kind === "state" && cell.state !== "valid") {
    const word = TOKEN_WORDS[cell.state];
    const note = cell.state === "missing" ? "no token yet" : cell.state === "expiring" && cell.expires ? `token expires ${cell.expires}` : `token ${word.label}`;
    return { text: `${base.text} · ${note}`, tone: word.tone === "err" ? "err" : "warn", hint: MATCH_WORDS[how].hint };
  }
  return { ...base, hint: MATCH_WORDS[how].hint };
}

const TONE_TEXT = { warn: "text-warn", err: "text-err", muted: "text-muted", ok: "text-ok" };

type Show = "needs" | "mapped" | "leftOut" | "all";

/** Rows of one status, with the customers of matching sites kept (dimmed) for context. */
function byStatus(rows: MapRow[], show: Show): MapRow[] {
  if (show === "all") return rows;
  const fits = (r: MapRow) => (show === "needs" ? needsChoice(r) : r.status === show);
  const parents = new Set(rows.filter((r) => r.kind === "site" && fits(r)).map((r) => r.parentId));
  return rows.filter((r) => fits(r) || (r.kind === "customer" && parents.has(r.sourceId)));
}

/** A customer and the sites under it that are on show. A site whose customer isn't on show stands alone. */
interface Group {
  head: MapRow;
  sites: MapRow[];
}

function groupRows(rows: MapRow[]): Group[] {
  const groups: Group[] = [];
  const byCustomer = new Map<string, Group>();
  for (const row of rows) {
    if (row.kind === "customer") {
      const g = { head: row, sites: [] };
      byCustomer.set(row.sourceId, g);
      groups.push(g);
    } else {
      const parent = row.parentId ? byCustomer.get(row.parentId) : undefined;
      if (parent) parent.sites.push(row);
      else groups.push({ head: row, sites: [] });
    }
  }
  return groups;
}

/** Customers drawn at a time; hundreds of customers with their sites would make one table too heavy. */
const PAGE = 100;

/** Where every source customer and site goes, and whether its destination can take devices (step 2 in docs/migration/11-gui-design.md). */
export function MapStep({ projectId, onContinue }: { projectId: string; onContinue: () => void }) {
  const map = useMap(projectId);
  const details = useMapDetails(projectId, !!map.data);
  const actions = useMapActions(projectId);
  const [text, setText] = useState("");
  const [chosenShow, setShow] = useState<Show | null>(null);
  const [limit, setLimit] = useState(PAGE);
  /** Customers the person opened or closed; the rest follow `openByDefault`. */
  const [opened, setOpened] = useState<Record<string, boolean>>({});
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkBusy, setBulkBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [creating, setCreating] = useState(false);
  const [report, setReport] = useState<TokenReport | null>(null);
  const [rowError, setRowError] = useState<string | null>(null);

  const view: MapView | undefined = map.data;
  const rows = useMemo(() => view?.rows ?? [], [view]);
  const counts = useMemo(() => mapCounts(rows), [rows]);
  // Open on what needs a choice while anything does; the person can switch.
  const show: Show = chosenShow ?? (counts.needsChoice > 0 ? "needs" : "all");
  const shown = useMemo(() => visibleRows(byStatus(rows, show), text, false), [rows, show, text]);
  const groups = useMemo(() => groupRows(shown), [shown]);
  const missing: TokenRow[] = missingTokens(details.data);

  // A new filter starts at the top of the list with nothing ticked and customers folded as the filter suggests.
  useEffect(() => {
    setLimit(PAGE);
    setSelected(new Set());
    setOpened({});
  }, [show, text]);

  const fits = (r: MapRow) => (show === "needs" ? needsChoice(r) : show === "all" || r.status === show);
  const devicesOf = (id: string) => details.data?.devices[id] ?? 0;
  const sitesOf = useMemo(() => {
    const m = new Map<string, number>();
    for (const r of rows) if (r.kind === "site" && r.parentId) m.set(r.parentId, (m.get(r.parentId) ?? 0) + 1);
    return m;
  }, [rows]);
  /** A customer's devices: its own plus every site's under it on the source. */
  const totalDevices = useMemo(() => {
    const m = new Map<string, number>();
    for (const r of rows) {
      const own = details.data?.devices[r.sourceId] ?? 0;
      const key = r.kind === "site" && r.parentId ? r.parentId : r.sourceId;
      m.set(key, (m.get(key) ?? 0) + own);
    }
    return m;
  }, [rows, details.data]);
  const waitingDevices = useMemo(() => rows.filter(needsChoice).reduce((n, r) => n + (details.data?.devices[r.sourceId] ?? 0), 0), [rows, details.data]);

  // While filtering, open the customers whose sites are what the filter is about; on All and Matched start folded.
  const openByDefault = (g: Group) => g.sites.length > 0 && (text.trim() !== "" || ((show === "needs" || show === "leftOut") && g.sites.some(fits)));
  const isOpen = (g: Group) => opened[g.head.sourceId] ?? openByDefault(g);

  // Re-running matching is the way to ask the servers again, so what the core remembered of them is dropped first.
  const refetchAll = () => {
    forgetReads(projectId)
      .catch(() => undefined)
      .then(() => map.refetch())
      .then(() => details.refetch());
  };

  const fail = (e: unknown) => setRowError(String(e));
  const choose = (row: MapRow, unit: UnitRef) => {
    setRowError(null);
    actions.choose.mutate({ sourceId: row.sourceId, decision: { kind: "mapTo", destId: unit.id } }, { onError: fail });
  };

  const selectedRows = rows.filter((r) => selected.has(r.sourceId));
  const chosenSelected = selectedRows.filter((r) => r.how === "chosen");
  const selectedKinds = new Set(selectedRows.map((r) => r.kind));
  const oneKind: UnitKind | null = selectedKinds.size === 1 ? selectedRows[0].kind : null;
  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const pageGroups = groups.slice(0, limit);
  const selectable = pageGroups.flatMap((g) => [g.head, ...(isOpen(g) ? g.sites : [])]).filter(fits);
  const allTicked = selectable.length > 0 && selectable.every((r) => selected.has(r.sourceId));

  /** One request per row, in turn, as the single-row choices do; stops at the first refusal. */
  const bulk = async (list: MapRow[], one: (row: MapRow) => Promise<unknown>) => {
    setRowError(null);
    setBulkBusy(true);
    try {
      for (const row of list) await one(row);
      setSelected(new Set());
    } catch (e) {
      fail(e);
    } finally {
      setBulkBusy(false);
    }
  };

  const tabs: { value: Show; label: string }[] = [
    { value: "needs", label: `Need a choice · ${counts.needsChoice.toLocaleString()}` },
    { value: "mapped", label: `Matched · ${counts.mapped.toLocaleString()}` },
    { value: "leftOut", label: `Left out · ${counts.leftOut.toLocaleString()}` },
    { value: "all", label: `All · ${rows.length.toLocaleString()}` },
  ];

  const customers = rows.filter((r) => r.kind === "customer").length;
  const sites = rows.length - customers;
  const allDevices = details.data ? Object.values(details.data.devices).reduce((a, b) => a + b, 0) : null;

  const renderRow = (row: MapRow, group: Group | null) => {
    const context = !fits(row);
    const status = statusOf(row, details.data, details.isFetching);
    // Sites under a customer are drawn without a group; a site whose customer isn't on show heads its own.
    const isSite = row.kind === "site" && group === null;
    const open = group ? isOpen(group) : false;
    const siteCount = row.kind === "customer" ? (sitesOf.get(row.sourceId) ?? 0) : 0;
    const closedNeeds = group && !open ? group.sites.filter(needsChoice).length : 0;
    return (
      <tr
        key={row.sourceId}
        className={context ? "text-faint" : needsChoice(row) ? "bg-warn-soft/30" : selected.has(row.sourceId) ? "bg-accent-soft/50" : isSite ? "bg-bg/40" : undefined}
      >
        <td>
          {!context && (
            <input type="checkbox" className="size-3.5 accent-accent" checked={selected.has(row.sourceId)} onChange={() => toggle(row.sourceId)} aria-label={`Select ${row.sourceName}`} />
          )}
        </td>
        <td className="max-w-0">
          <div className={`flex items-center gap-1.5 ${isSite ? "pl-7" : ""}`}>
            {group && group.sites.length > 0 ? (
              <button
                type="button"
                onClick={() => setOpened((o) => ({ ...o, [row.sourceId]: !open }))}
                aria-expanded={open}
                aria-label={`${open ? "Hide" : "Show"} the sites of ${row.sourceName}`}
                className="flex size-5 shrink-0 items-center justify-center rounded text-faint hover:bg-surface-2 hover:text-text"
              >
                {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
              </button>
            ) : (
              !isSite && <span className="size-5 shrink-0" />
            )}
            <div className="min-w-0">
              <div className={`truncate ${row.kind === "customer" ? "font-semibold" : ""}`} title={row.sourceName}>
                {row.sourceName}
              </div>
              <div className="truncate text-xs text-faint">
                {row.kind === "customer" ? (siteCount ? plural(siteCount, "site") : "customer") : "site"}
                {closedNeeds > 0 && <span className="text-warn"> · {plural(closedNeeds, "site")} need a choice</span>}
              </div>
            </div>
          </div>
        </td>
        <td className="text-right text-muted tabular-nums">
          {details.data ? (row.kind === "customer" ? (totalDevices.get(row.sourceId) ?? 0) : devicesOf(row.sourceId)).toLocaleString() : "…"}
        </td>
        <td className="max-w-0">
          <DestPicker
            row={row}
            units={view?.destUnits ?? []}
            onChoose={(unit) => choose(row, unit)}
            onLeaveOut={() => {
              setRowError(null);
              actions.choose.mutate({ sourceId: row.sourceId, decision: { kind: "exclude" } }, { onError: fail });
            }}
            onAutomatic={() => {
              setRowError(null);
              actions.clear.mutate(row.sourceId, { onError: fail });
            }}
          />
        </td>
        <td className="max-w-0">
          <Tooltip content={status.hint}>
            <span className={`block truncate text-xs ${TONE_TEXT[status.tone]}`}>{status.text}</span>
          </Tooltip>
        </td>
      </tr>
    );
  };

  return (
    <div className="space-y-4">
      {view && (
        <p className="text-sm text-muted">
          <span className="font-semibold text-text">{customers.toLocaleString()}</span> customers · <span className="font-semibold text-text">{sites.toLocaleString()}</span> sites
          {allDevices !== null && (
            <>
              {" "}
              · <span className="font-semibold text-text">{allDevices.toLocaleString()}</span> devices
            </>
          )}{" "}
          on {view.sourceLabel}
        </p>
      )}

      <div className="flex flex-wrap items-center gap-2.5">
        {view && <Segmented value={show} onChange={setShow} options={tabs} />}
        <label className="relative w-72 max-w-full">
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
          <input className={`${inputClass} pl-8`} placeholder="Search customers or sites" value={text} onChange={(e) => setText(e.target.value)} aria-label="Search customers or sites" />
        </label>
        <span className="flex-1" />
        <Button disabled={!view || rows.every((r) => r.status !== "unmatched")} onClick={() => setCreating(true)}>
          <PlusSquare />
          Create missing…
        </Button>
        <Menu>
          <MenuTrigger asChild>
            <Button disabled={!details.data}>
              <KeyRound />
              Tokens
            </Button>
          </MenuTrigger>
          <MenuContent>
            <MenuItem
              disabled={missing.length === 0}
              onSelect={() => {
                actions.generate.reset();
                setConfirming(true);
              }}
            >
              {missing.length ? `Create ${missing.length} missing…` : "No tokens missing"}
            </MenuItem>
          </MenuContent>
        </Menu>
        <Button onClick={refetchAll} busy={map.isFetching}>
          <RefreshCw />
          Re-run matching
        </Button>
      </div>

      {map.isError ? (
        <ErrorNote>{String(map.error)}</ErrorNote>
      ) : !view ? (
        <p className="text-sm text-muted">Reading customers and sites from both servers…</p>
      ) : (
        <>
          {rowError && <ErrorNote>{rowError}</ErrorNote>}
          {details.isError && <ErrorNote>{`Device counts and tokens couldn't be read: ${String(details.error)}`}</ErrorNote>}
          {details.data?.warnings.map((w) => (
            <p key={w} className="text-xs text-warn">
              {w}
            </p>
          ))}

          {selected.size > 0 && (
            <SelectionBar count={selected.size} onClear={() => setSelected(new Set())}>
              {oneKind && (
                <MapToButton
                  kind={oneKind}
                  units={view.destUnits}
                  busy={bulkBusy}
                  onPick={(unit) => bulk(selectedRows, (row) => actions.choose.mutateAsync({ sourceId: row.sourceId, decision: { kind: "mapTo", destId: unit.id } }))}
                />
              )}
              <Button size="sm" busy={bulkBusy} onClick={() => bulk(selectedRows, (row) => actions.choose.mutateAsync({ sourceId: row.sourceId, decision: { kind: "exclude" } }))}>
                Leave out
              </Button>
              <Button size="sm" disabled={bulkBusy || chosenSelected.length === 0} onClick={() => bulk(chosenSelected, (row) => actions.clear.mutateAsync(row.sourceId))}>
                Use the automatic match{chosenSelected.length > 0 && chosenSelected.length < selected.size ? ` (${chosenSelected.length})` : ""}
              </Button>
            </SelectionBar>
          )}

          {shown.length === 0 ? (
            <p className="py-6 text-center text-sm text-muted">
              {rows.length === 0 ? "The source server has no customers." : text ? "No rows match." : show === "needs" ? "Nothing needs a choice." : "No rows here."}
            </p>
          ) : (
            <Table
              head={
                <>
                  <th className="w-9">
                    <input
                      type="checkbox"
                      className="size-3.5 accent-accent"
                      checked={allTicked}
                      onChange={() => setSelected(allTicked ? new Set() : new Set(selectable.map((r) => r.sourceId)))}
                      aria-label="Select all shown"
                    />
                  </th>
                  <th className="w-[32%]">On {view.sourceLabel}</th>
                  <th className="w-20 text-right">Devices</th>
                  <th className="w-[34%]">Goes to on {view.destLabel}</th>
                  <th>Status</th>
                </>
              }
            >
              {pageGroups.flatMap((g) => [renderRow(g.head, g), ...(isOpen(g) ? g.sites.map((s) => renderRow(s, null)) : [])])}
            </Table>
          )}
          {groups.length > limit && (
            <div className="flex items-center justify-center gap-3 text-sm text-muted">
              <span>
                {limit.toLocaleString()} of {groups.length.toLocaleString()} customers shown
              </span>
              <Button size="sm" onClick={() => setLimit((l) => l + PAGE)}>
                Show {Math.min(PAGE, groups.length - limit)} more
              </Button>
            </div>
          )}

          <div className="sticky bottom-0 -mx-1 flex flex-wrap items-center justify-between gap-3 border-t border-border bg-bg px-1 py-3">
            <p className="text-sm text-muted">
              {counts.needsChoice > 0 ? (
                <>
                  <span className="font-semibold text-warn">{plural(counts.needsChoice, "customer or site")} left</span>
                  {details.data && waitingDevices > 0 && ` · ${plural(waitingDevices, "device")} can't be planned until they're chosen`}
                </>
              ) : (
                "Every customer and site has a destination or is left out."
              )}
            </p>
            <Button variant="primary" onClick={onContinue}>
              Continue to Prepare
            </Button>
          </div>
        </>
      )}

      {confirming && (
        <GenerateTokensDialog
          missing={missing}
          busy={actions.generate.isPending}
          error={actions.generate.isError ? String(actions.generate.error) : null}
          onClose={() => setConfirming(false)}
          onConfirm={() =>
            actions.generate.mutate(undefined, {
              onSuccess: (r) => {
                setConfirming(false);
                setReport(r);
              },
            })
          }
        />
      )}
      {report && <TokenResultDialog report={report} onClose={() => setReport(null)} />}
      {creating && <StructureDialog projectId={projectId} onClose={() => setCreating(false)} />}
    </div>
  );
}

/** "Map to a customer…" for ticked rows of one kind: a search over the destination units of that kind. */
function MapToButton({ kind, units, busy, onPick }: { kind: UnitKind; units: UnitRef[]; busy: boolean; onPick: (unit: UnitRef) => void }) {
  const [open, setOpen] = useState(false);
  const word = kind === "site" ? "site" : "customer";
  const sameKind = units.filter((u) => u.kind === kind);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button size="sm" busy={busy}>
          Map to a {word}…
        </Button>
      </PopoverTrigger>
      <PopoverContent width={340}>
        <SearchList<UnitRef>
          debounceMs={0}
          search={async (term) => {
            const t = term.toLowerCase();
            return sameKind.filter((u) => `${u.name} ${u.parentName ?? ""}`.toLowerCase().includes(t));
          }}
          getKey={(u) => u.id}
          onPick={(u) => {
            setOpen(false);
            onPick(u);
          }}
          placeholder={`Search ${word}s`}
          renderItem={(u) => (
            <div className="flex items-baseline justify-between gap-3">
              <span className="truncate">{u.name}</span>
              {u.parentName && <span className="shrink-0 text-xs text-faint">{u.parentName}</span>}
            </div>
          )}
        />
      </PopoverContent>
    </Popover>
  );
}
