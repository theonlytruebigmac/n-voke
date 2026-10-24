import { useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { useState } from "react";
import { Button, ErrorNote } from "../../ui";
import { NameList } from "./NameList";

export interface BulkFailure {
  name: string;
  why: string;
}

export interface BulkState {
  /** What is being done, e.g. "Leave out". */
  label: string;
  done: number;
  total: number;
  failed: BulkFailure[];
  finished: boolean;
}

/**
 * Does one thing to many devices by calling the per-device command for each in turn (there is no batch command),
 * then refreshes the project once, as the per-device mutations would have after each.
 */
export function useBulk(projectId: string) {
  const qc = useQueryClient();
  const [state, setState] = useState<BulkState | null>(null);

  const refresh = () =>
    Promise.all([
      qc.invalidateQueries({ queryKey: ["migrate", "projects"] }),
      qc.invalidateQueries({ queryKey: ["migrate", "project", projectId] }),
      qc.invalidateQueries({ queryKey: ["migrate", "device", projectId] }),
      ...["launch", "track", "finish", "plan"].map((what) => qc.invalidateQueries({ queryKey: ["migrate", what, projectId] })),
    ]);

  async function run<T>(label: string, items: T[], nameOf: (item: T) => string, act: (item: T) => Promise<unknown>) {
    const failed: BulkFailure[] = [];
    setState({ label, done: 0, total: items.length, failed, finished: false });
    for (let i = 0; i < items.length; i++) {
      try {
        await act(items[i]);
      } catch (e) {
        failed.push({ name: nameOf(items[i]), why: String(e) });
      }
      setState({ label, done: i + 1, total: items.length, failed: [...failed], finished: false });
    }
    await refresh();
    setState({ label, done: items.length, total: items.length, failed, finished: true });
    return failed;
  }

  return { state, run, busy: !!state && !state.finished, clear: () => setState(null) };
}

/** Progress while a bulk action runs, then how it went. */
export function BulkStatus({ state, onClear }: { state: BulkState | null; onClear: () => void }) {
  if (!state) return null;
  if (!state.finished)
    return (
      <p className="flex items-center gap-2 text-sm text-muted">
        <Loader2 className="size-3.5 animate-spin" />
        {state.label}: {state.done.toLocaleString()} of {state.total.toLocaleString()}…
      </p>
    );
  const ok = state.total - state.failed.length;
  return (
    <div className="space-y-2">
      {state.failed.length === 0 ? (
        <div className="flex items-center justify-between gap-3 rounded-xl border border-ok/30 bg-ok-soft px-3 py-2 text-sm">
          <span>
            {state.label}: done for {ok.toLocaleString()} {ok === 1 ? "device" : "devices"}.
          </span>
          <Button size="sm" variant="ghost" onClick={onClear}>
            Dismiss
          </Button>
        </div>
      ) : (
        <ErrorNote>
          <div className="space-y-2">
            <div className="flex items-center justify-between gap-3">
              <span>
                {state.label}: done for {ok.toLocaleString()}, {state.failed.length.toLocaleString()} didn't take.
              </span>
              <Button size="sm" variant="ghost" onClick={onClear}>
                Dismiss
              </Button>
            </div>
            <NameList items={state.failed.map((f) => ({ key: f.name, text: `${f.name}: ${f.why}` }))} />
          </div>
        </ErrorNote>
      )}
    </div>
  );
}
