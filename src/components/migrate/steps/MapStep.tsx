import { ChevronRight, KeyRound, PlusSquare, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";
import type { MapRow, MapView, TokenReport, TokenRow, UnitRef } from "../../../lib/migrate/bindings";
import { MATCH_WORDS, TOKEN_WORDS, mapCounts, missingTokens, needsChoice, tokenCell, visibleRows } from "../../../lib/migrate/map";
import { useMap, useMapActions, useMapDetails } from "../../../lib/migrate/queries";
import { Button, Dot, ErrorNote, Menu, MenuContent, MenuItem, MenuTrigger, Segmented, Table, Tooltip, inputClass } from "../../ui";
import { DestPicker } from "../parts/DestPicker";
import { StructureDialog } from "../parts/StructureDialog";
import { GenerateTokensDialog, TokenResultDialog } from "../parts/TokenDialogs";

function TokenCellView({ row, details, loading }: { row: MapRow; details: ReturnType<typeof useMapDetails>["data"]; loading: boolean }) {
  const cell = tokenCell(row, details, loading);
  if (cell.kind === "loading") return <span className="text-faint">…</span>;
  if (cell.kind === "none" || cell.kind === "unread") return <span className="text-faint">–</span>;
  const word = TOKEN_WORDS[cell.state];
  return (
    <span className="inline-flex items-center gap-1.5 whitespace-nowrap">
      <Dot tone={word.tone} />
      {word.label}
      {cell.expires && cell.state !== "missing" && <span className="text-xs text-faint">{cell.expires}</span>}
    </span>
  );
}

function MatchCell({ row }: { row: MapRow }) {
  if (!row.how) return <span className="text-faint">–</span>;
  const word = MATCH_WORDS[row.how];
  const flag = row.how === "similar" || row.how === "exactElsewhere";
  return (
    <Tooltip content={word.hint}>
      <span className={flag ? "text-warn" : "text-muted"}>{word.label}</span>
    </Tooltip>
  );
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Where every source customer and site goes, and whether its destination can take devices (step 2 in docs/migration/11-gui-design.md). */
export function MapStep({ projectId, onContinue }: { projectId: string; onContinue: () => void }) {
  const map = useMap(projectId);
  const details = useMapDetails(projectId, !!map.data);
  const actions = useMapActions(projectId);
  const [text, setText] = useState("");
  const [onlyNeeds, setOnlyNeeds] = useState<"all" | "needs">("all");
  const [confirming, setConfirming] = useState(false);
  const [creating, setCreating] = useState(false);
  const [report, setReport] = useState<TokenReport | null>(null);
  const [rowError, setRowError] = useState<string | null>(null);

  const view: MapView | undefined = map.data;
  const rows = view?.rows ?? [];
  const counts = useMemo(() => mapCounts(rows), [rows]);
  const shown = useMemo(() => visibleRows(rows, text, onlyNeeds === "needs"), [rows, text, onlyNeeds]);
  const missing: TokenRow[] = missingTokens(details.data);
  const refetchAll = () => {
    map.refetch().then(() => details.refetch());
  };

  const fail = (e: unknown) => setRowError(String(e));
  const choose = (row: MapRow, unit: UnitRef) => {
    setRowError(null);
    actions.choose.mutate({ sourceId: row.sourceId, decision: { kind: "mapTo", destId: unit.id } }, { onError: fail });
  };

  return (
    <div className="space-y-4 px-6 py-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-semibold">Map</h2>
        <div className="flex items-center gap-2">
          <Button size="sm" disabled={!view || rows.every((r) => r.status !== "unmatched")} onClick={() => setCreating(true)}>
            <PlusSquare />
            Create missing…
          </Button>
          <Menu>
            <MenuTrigger asChild>
              <Button size="sm" disabled={!details.data}>
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
          <Button size="sm" onClick={refetchAll} busy={map.isFetching}>
            <RefreshCw />
            Re-run matching
          </Button>
        </div>
      </div>

      {map.isError ? (
        <ErrorNote>{String(map.error)}</ErrorNote>
      ) : !view ? (
        <p className="text-sm text-muted">Reading customers and sites from both servers…</p>
      ) : (
        <>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="text-sm text-muted">
              <span className="text-ok">{counts.mapped} matched</span>
              {" · "}
              <span className={counts.needsChoice ? "text-warn" : ""}>{counts.needsChoice} need a choice</span>
              {" · "}
              {counts.leftOut} left out
            </p>
            <div className="flex items-center gap-2">
              <Segmented
                value={onlyNeeds}
                onChange={setOnlyNeeds}
                options={[
                  { value: "all", label: "All" },
                  { value: "needs", label: "Need a choice" },
                ]}
              />
              <input className={`${inputClass} w-44`} placeholder="Filter" value={text} onChange={(e) => setText(e.target.value)} aria-label="Filter rows" />
            </div>
          </div>

          {rowError && <ErrorNote>{rowError}</ErrorNote>}
          {details.isError && <ErrorNote>{`Device counts and tokens couldn't be read: ${String(details.error)}`}</ErrorNote>}
          {details.data?.warnings.map((w) => (
            <p key={w} className="text-xs text-warn">
              {w}
            </p>
          ))}

          {shown.length === 0 ? (
            <p className="text-sm text-muted">{rows.length === 0 ? "The source server has no customers." : "No rows match."}</p>
          ) : (
            <Table
              head={
                <>
                  <th className="w-[32%]">Source</th>
                  <th className="w-8" />
                  <th className="w-[36%]">Destination</th>
                  <th>Match</th>
                  <th>Token</th>
                </>
              }
            >
              {shown.map((row) => (
                <tr key={row.sourceId} className={needsChoice(row) ? "bg-warn-soft/40" : undefined}>
                  <td>
                    <div className={row.kind === "site" ? "pl-6" : undefined}>
                      <div className={`break-words ${row.kind === "customer" ? "font-medium" : ""}`}>{row.sourceName}</div>
                      <div className="text-xs text-faint">
                        {row.kind}
                        {details.data && ` · ${plural(details.data.devices[row.sourceId] ?? 0, "device")}`}
                      </div>
                    </div>
                  </td>
                  <td className="text-faint">
                    <ChevronRight className="size-3.5" />
                  </td>
                  <td>
                    <DestPicker
                      row={row}
                      units={view.destUnits}
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
                  <td>
                    <MatchCell row={row} />
                  </td>
                  <td>
                    <TokenCellView row={row} details={details.data} loading={details.isFetching} />
                  </td>
                </tr>
              ))}
            </Table>
          )}
          <div className="flex items-center justify-between gap-3">
            <p className="text-xs text-faint">{counts.needsChoice > 0 ? "Devices under a customer or site that needs a choice can't be planned until it has one." : ""}</p>
            <Button variant="primary" size="sm" onClick={onContinue}>
              Continue
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
