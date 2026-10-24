import { useEffect, useMemo, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { previewAssets } from "../../lib/api";
import { activeConditions, ruleToFilter, type Rule } from "../../lib/rules";
import type { Asset } from "../../lib/types";
import { AgentDot } from "../status";

export interface RulePreview {
  /** Null until the first answer, and while the rule has no conditions. */
  result: { assets: Asset[]; totalCount: number } | null;
  loading: boolean;
  error: string | null;
  hasConditions: boolean;
}

/** Devices a rule matches right now (the first 50), refreshed shortly after the rule stops changing. */
export function useRulePreview(orgId: string | null, rule: Rule): RulePreview {
  const filter = useMemo(() => ruleToFilter(rule), [rule]);
  const key = JSON.stringify(filter);
  const [result, setResult] = useState<RulePreview["result"]>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const hasConditions = activeConditions(rule).length > 0;

  useEffect(() => {
    if (!hasConditions || !orgId) {
      requestId.current++;
      setResult(null);
      setLoading(false);
      return;
    }
    const id = ++requestId.current;
    setLoading(true);
    const timer = setTimeout(async () => {
      try {
        const next = await previewAssets(orgId, JSON.parse(key), 50);
        if (id === requestId.current) {
          setResult(next);
          setError(null);
        }
      } catch (e) {
        if (id === requestId.current) setError((e as Error).message);
      } finally {
        if (id === requestId.current) setLoading(false);
      }
    }, 400);
    return () => clearTimeout(timer);
  }, [orgId, key, hasConditions]);

  return { result, loading, error, hasConditions };
}

/** How many chips to show before "+N more". */
const CHIPS = 12;

/** A big count over device chips: "1,842 devices match now", the first few names, "+1,830 more". */
export function MatchPreview({ preview, emptyHint = "Add a condition to see which devices match" }: { preview: RulePreview; emptyHint?: string }) {
  const { result, loading, error, hasConditions } = preview;
  if (error) return <p className="text-sm text-err">{error}</p>;
  if (!hasConditions) return <p className="text-sm text-faint">{emptyHint}</p>;
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-baseline gap-2">
        <span className="text-3xl font-bold tabular-nums">{result ? result.totalCount.toLocaleString() : "–"}</span>
        <span className="text-muted">{result?.totalCount === 1 ? "device matches now" : "devices match now"}</span>
        {loading && <Loader2 className="size-3.5 animate-spin self-center text-muted" />}
      </div>
      {result && result.assets.length > 0 ? <DeviceChips assets={result.assets} total={result.totalCount} /> : result && <p className="text-sm text-faint">No devices match yet</p>}
    </div>
  );
}

/** The first few devices as chips, with how many more there are. */
export function DeviceChips({ assets, total }: { assets: Asset[]; total: number }) {
  const shown = assets.slice(0, CHIPS);
  return (
    <ul className="flex flex-wrap gap-1.5">
      {shown.map((a) => (
        <li key={a.id} title={a.name} className="inline-flex h-6 max-w-full items-center gap-1.5 rounded-md border border-border bg-surface-2 px-2 text-xs">
          <AgentDot status={a.agentConnection?.status} />
          <span className="truncate">{a.name}</span>
        </li>
      ))}
      {total > shown.length && <li className="inline-flex h-6 items-center px-1 text-xs text-faint">+ {(total - shown.length).toLocaleString()} more</li>}
    </ul>
  );
}
