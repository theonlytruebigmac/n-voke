import { useEffect, useMemo, useState } from "react";
import { Check, Loader2 } from "lucide-react";
import { MAX_RESOLVED_DEVICES } from "../lib/api";
import { tagMembersCached } from "../lib/collections";
import { osMismatch, type Target } from "../lib/targets";
import type { Asset, Script } from "../lib/types";
import { AgentDot } from "./status";
import { Button, ErrorNote, Table, Tooltip } from "./ui";

interface Resolved {
  asset: Asset;
  /** Tag names it came from, or "Picked". */
  via: string[];
}

/** What the Run page needs to know about the device list, for its summary. */
export interface DeviceSummary {
  loading: boolean;
  /** Devices the targets resolve to. */
  total: number;
  /** Devices that will run (ticked). */
  included: number;
  /** Ticked devices that are offline or whose OS may not suit the script. */
  flagged: number;
}

/** Lists of this size or smaller start open; longer ones start as a count. */
const OPEN_UP_TO = 20;

const isOffline = (a: Asset) => !!a.agentConnection && a.agentConnection.status !== "CONNECTED";

/**
 * The chosen targets resolved to one de-duplicated device list. Devices that need a look come first, grouped by
 * reason, each group with Keep all / Leave all out; the rest fold into a "ready" count. Untick a row to leave that
 * device out; the parent receives the final list.
 */
export function TargetDevices({
  orgId,
  script,
  targets,
  onChange,
  onSummary,
}: {
  orgId: string;
  script: Script | null;
  targets: Target[];
  onChange: (devices: Asset[]) => void;
  onSummary?: (summary: DeviceSummary) => void;
}) {
  const [resolved, setResolved] = useState<Resolved[]>([]);
  const [truncated, setTruncated] = useState(false);
  const [excluded, setExcluded] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Groups the person opened or closed; the rest follow OPEN_UP_TO. */
  const [shown, setShown] = useState<Record<string, boolean>>({});
  const key = targets.map((t) => (t.kind === "tag" ? `t${t.tag.id}` : `d${t.asset.id}`)).join(",");

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoading(true);
      setError(null);
      try {
        const byId = new Map<string, Resolved>();
        for (const t of targets) if (t.kind === "device") byId.set(t.asset.id, { asset: t.asset, via: ["Picked"] });
        let cut = false;
        for (const t of targets) {
          if (t.kind !== "tag") continue;
          const { assets, totalCount } = await tagMembersCached(orgId, t.tag.id);
          cut ||= totalCount > assets.length;
          for (const a of assets) {
            const existing = byId.get(a.id);
            if (existing) existing.via.push(t.tag.name);
            else byId.set(a.id, { asset: a, via: [t.tag.name] });
          }
        }
        if (!cancelled) {
          const next = [...byId.values()].sort((a, b) => a.asset.name.localeCompare(b.asset.name));
          // Devices that just (re)joined the list start ticked, even if they were unticked before removal.
          setResolved((prev) => {
            const before = new Set(prev.map((r) => r.asset.id));
            const joined = next.filter((r) => !before.has(r.asset.id)).map((r) => r.asset.id);
            if (joined.length) setExcluded((ex) => (joined.some((id) => ex.has(id)) ? new Set([...ex].filter((id) => !joined.includes(id))) : ex));
            return next;
          });
          setTruncated(cut);
        }
      } catch (e) {
        if (!cancelled) setError((e as Error).message);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, orgId]);

  const groups = useMemo(() => {
    const offline: Resolved[] = [];
    const mismatch: Resolved[] = [];
    const ready: Resolved[] = [];
    for (const r of resolved) {
      const off = isOffline(r.asset);
      const os = !!osMismatch(script?.language, r.asset.operatingSystem?.name);
      if (off) offline.push(r);
      if (os) mismatch.push(r);
      if (!off && !os) ready.push(r);
    }
    return { offline, mismatch, ready };
  }, [resolved, script?.language]);

  const included = useMemo(() => resolved.filter((r) => !excluded.has(r.asset.id)), [resolved, excluded]);
  // While the list is out of date (loading, or failed) nothing may be run: the
  // last good list belongs to the previous targets.
  useEffect(() => onChange(loading || error ? [] : included.map((r) => r.asset)), [included, loading, error, onChange]);

  const flaggedIncluded = included.filter((r) => isOffline(r.asset) || !!osMismatch(script?.language, r.asset.operatingSystem?.name)).length;
  useEffect(
    () => onSummary?.({ loading, total: resolved.length, included: included.length, flagged: flaggedIncluded }),
    [onSummary, loading, resolved.length, included.length, flaggedIncluded],
  );

  const setMany = (ids: string[], leaveOut: boolean) =>
    setExcluded((prev) => {
      const next = new Set(prev);
      for (const id of ids) {
        if (leaveOut) next.add(id);
        else next.delete(id);
      }
      return next;
    });
  const toggle = (id: string) => setMany([id], !excluded.has(id));
  const isShown = (group: string, size: number) => shown[group] ?? size <= OPEN_UP_TO;
  const flip = (group: string, size: number) => setShown((s) => ({ ...s, [group]: !isShown(group, size) }));

  if (!targets.length) return null;
  if (error) return <ErrorNote>{error}</ErrorNote>;
  if (loading && !resolved.length)
    return (
      <p className="flex items-center gap-2 text-sm text-muted">
        <Loader2 className="size-3.5 animate-spin" />
        Reading the devices…
      </p>
    );
  if (!resolved.length) return <p className="text-sm text-faint">No devices in these tags.</p>;

  const showSource = targets.some((t) => t.kind === "tag");
  const language = script?.language?.toLowerCase();
  const table = (rows: Resolved[]) => <DeviceTable rows={rows} excluded={excluded} onToggle={toggle} onSetAll={setMany} showSource={showSource} script={script} />;

  const flagGroups = [
    { id: "offline", rows: groups.offline, label: "Offline", detail: "Their agent isn't connected right now." },
    {
      id: "mismatch",
      rows: groups.mismatch,
      label: "Operating system may not suit the script",
      detail: language ? `This is a ${language} script.` : undefined,
    },
  ].filter((g) => g.rows.length > 0);

  return (
    <div className="space-y-3">
      {truncated && <p className="text-xs text-warn">Only the first {MAX_RESOLVED_DEVICES} devices of a tag are included.</p>}

      {flagGroups.length > 0 && (
        <div className="divide-y divide-warn/20 rounded-xl border border-warn/30 bg-warn-soft/40">
          {flagGroups.map((g) => {
            const ids = g.rows.map((r) => r.asset.id);
            const out = ids.filter((id) => excluded.has(id)).length;
            const open = isShown(g.id, g.rows.length);
            return (
              <div key={g.id} className="space-y-3 px-4 py-3">
                <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
                  <span className="w-12 text-base font-bold tabular-nums">{g.rows.length}</span>
                  <div className="min-w-40 flex-1">
                    <div>{g.label}</div>
                    <div className="text-xs text-faint">
                      {g.detail}
                      {out > 0 && <span className="text-muted"> {out === ids.length ? "All left out." : `${out} left out.`}</span>}
                    </div>
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    <Button size="sm" variant="ghost" onClick={() => flip(g.id, g.rows.length)} aria-expanded={open}>
                      {open ? "Hide" : "Show"}
                    </Button>
                    <Button size="sm" onClick={() => setMany(ids, false)} disabled={out === 0}>
                      Keep all
                    </Button>
                    <Button size="sm" onClick={() => setMany(ids, true)} disabled={out === ids.length}>
                      Leave all out
                    </Button>
                  </div>
                </div>
                {open && table(g.rows)}
              </div>
            );
          })}
        </div>
      )}

      {groups.ready.length > 0 && (
        <div className="space-y-3 rounded-xl border border-border px-4 py-3">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <Check className="size-4 text-ok" />
            <span className="flex-1 font-medium">
              {groups.ready.length} ready
              {loading && <Loader2 className="ml-2 inline size-3.5 animate-spin text-muted" />}
            </span>
            <Button size="sm" variant="ghost" onClick={() => flip("ready", groups.ready.length)} aria-expanded={isShown("ready", groups.ready.length)}>
              {isShown("ready", groups.ready.length) ? "Hide" : "Show"}
            </Button>
          </div>
          {isShown("ready", groups.ready.length) && table(groups.ready)}
        </div>
      )}
    </div>
  );
}

function DeviceTable({
  rows,
  excluded,
  onToggle,
  onSetAll,
  showSource,
  script,
}: {
  rows: Resolved[];
  excluded: Set<string>;
  onToggle: (id: string) => void;
  onSetAll: (ids: string[], leaveOut: boolean) => void;
  showSource: boolean;
  script: Script | null;
}) {
  const ids = rows.map((r) => r.asset.id);
  const out = ids.filter((id) => excluded.has(id)).length;
  return (
    <Table
      head={
        <>
          <th className="w-9">
            <input
              type="checkbox"
              className="size-3.5 accent-accent"
              checked={out === 0}
              ref={(el) => {
                if (el) el.indeterminate = out > 0 && out < ids.length;
              }}
              onChange={() => onSetAll(ids, out === 0)}
              aria-label="Select all"
            />
          </th>
          <th>Device</th>
          <th>Location</th>
          {showSource && <th>From</th>}
        </>
      }
    >
      {rows.map((r) => {
        const isExcluded = excluded.has(r.asset.id);
        const mismatch = osMismatch(script?.language, r.asset.operatingSystem?.name);
        return (
          <tr key={r.asset.id} onClick={() => onToggle(r.asset.id)} className={`cursor-default hover:bg-surface-2/60 ${isExcluded ? "text-faint" : ""}`}>
            <td>
              <input
                type="checkbox"
                className="size-3.5 accent-accent"
                checked={!isExcluded}
                onChange={() => onToggle(r.asset.id)}
                onClick={(e) => e.stopPropagation()}
                aria-label={`Include ${r.asset.name}`}
              />
            </td>
            <td>
              <div className="flex items-center gap-2 whitespace-nowrap">
                <AgentDot status={r.asset.agentConnection?.status} />
                <span className="truncate font-medium">{r.asset.name}</span>
                {mismatch && (
                  <Tooltip content={`${r.asset.operatingSystem?.name}: may not support ${script?.language?.toLowerCase()} scripts`}>
                    <span className="text-xs text-warn">{r.asset.operatingSystem?.name ?? "OS"}</span>
                  </Tooltip>
                )}
              </div>
            </td>
            <td className="max-w-0 truncate text-muted">{[r.asset.customer?.name, r.asset.site?.name].filter(Boolean).join(" · ")}</td>
            {showSource && <td className="max-w-0 truncate text-muted">{r.via.map((v) => (v === "Picked" ? "Added directly" : v)).join(", ")}</td>}
          </tr>
        );
      })}
    </Table>
  );
}
