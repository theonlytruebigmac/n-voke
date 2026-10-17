import { useEffect, useMemo, useState } from "react";
import { Loader2 } from "lucide-react";
import { MAX_RESOLVED_DEVICES } from "../lib/api";
import { tagMembersCached } from "../lib/collections";
import { osMismatch, type Target } from "../lib/targets";
import type { Asset, Script } from "../lib/types";
import { AgentDot } from "./status";
import { ErrorNote, SectionTitle, Table, Tooltip } from "./ui";

interface Resolved {
  asset: Asset;
  /** Tag names it came from, or "Picked". */
  via: string[];
}

/**
 * The chosen targets resolved to one de-duplicated device table. Untick a row
 * to leave that device out; the parent receives the final list.
 */
export function TargetDevices({
  orgId,
  script,
  targets,
  onChange,
}: {
  orgId: string;
  script: Script | null;
  targets: Target[];
  onChange: (devices: Asset[]) => void;
}) {
  const [resolved, setResolved] = useState<Resolved[]>([]);
  const [truncated, setTruncated] = useState(false);
  const [excluded, setExcluded] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
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

  const included = useMemo(() => resolved.filter((r) => !excluded.has(r.asset.id)), [resolved, excluded]);
  // While the list is out of date (loading, or failed) nothing may be run: the
  // last good list belongs to the previous targets.
  useEffect(() => onChange(loading || error ? [] : included.map((r) => r.asset)), [included, loading, error, onChange]);

  // The "From" column only says something when tags are involved.
  const showSource = targets.some((t) => t.kind === "tag");
  const offline = included.filter((r) => r.asset.agentConnection && r.asset.agentConnection.status !== "CONNECTED").length;
  // `excluded` can still hold devices whose target was removed; count only the listed ones.
  const excludedCount = resolved.length - included.length;
  const allChecked = resolved.length > 0 && excludedCount === 0;

  const toggle = (id: string) =>
    setExcluded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  if (!targets.length) return null;
  if (error) return <ErrorNote>{error}</ErrorNote>;

  return (
    <section className="space-y-2">
      <SectionTitle
        actions={
          <span className="text-xs text-muted">
            {loading ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <>
                {offline > 0 && <span className="text-warn">{offline} offline · </span>}
                {included.length} of {resolved.length} selected
              </>
            )}
          </span>
        }
      >
        Devices
      </SectionTitle>

      {truncated && <p className="text-xs text-warn">Only the first {MAX_RESOLVED_DEVICES} devices of a tag are included.</p>}
      {!loading && resolved.length === 0 ? (
        <p className="py-6 text-center text-sm text-faint">No devices in these tags.</p>
      ) : (
        <Table
          head={
            <>
              <th className="w-9">
                <input
                  type="checkbox"
                  className="size-3.5 accent-accent"
                  checked={allChecked}
                  ref={(el) => {
                    if (el) el.indeterminate = excludedCount > 0 && excludedCount < resolved.length;
                  }}
                  onChange={() => setExcluded(allChecked ? new Set(resolved.map((r) => r.asset.id)) : new Set())}
                  aria-label="Select all"
                />
              </th>
              <th>Device</th>
              <th>Location</th>
              {showSource && <th>From</th>}
            </>
          }
        >
          {resolved.map((r) => {
            const isExcluded = excluded.has(r.asset.id);
            const mismatch = osMismatch(script?.language, r.asset.operatingSystem?.name);
            return (
              <tr key={r.asset.id} onClick={() => toggle(r.asset.id)} className={`cursor-default hover:bg-surface-2/60 ${isExcluded ? "text-faint" : ""}`}>
                <td>
                  <input
                    type="checkbox"
                    className="size-3.5 accent-accent"
                    checked={!isExcluded}
                    onChange={() => toggle(r.asset.id)}
                    onClick={(e) => e.stopPropagation()}
                    aria-label={`Include ${r.asset.name}`}
                  />
                </td>
                <td>
                  <div className="flex items-center gap-2">
                    <AgentDot status={r.asset.agentConnection?.status} />
                    <span className="truncate font-medium">{r.asset.name}</span>
                    {mismatch && (
                      <Tooltip content={`${r.asset.operatingSystem?.name}: may not support ${script?.language?.toLowerCase()} scripts`}>
                        <span className="text-xs text-warn">OS</span>
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
      )}
    </section>
  );
}
