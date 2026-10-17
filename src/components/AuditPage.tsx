import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight, Copy, MoreHorizontal, ScrollText, Search } from "lucide-react";
import { getAuditLog, type AuditEntry } from "../lib/api";
import { copyText, formatDateTime } from "../lib/format";
import { EmptyState, ErrorNote, IconButton, Menu, MenuContent, MenuItem, MenuTrigger, PageHeader, Segmented, Status, Table, inputClass } from "./ui";

const ACTION_LABELS: Record<string, string> = {
  "script.run": "Script run",
  "tag.create": "Tag created",
  "tag.update": "Tag updated",
  "tag.delete": "Tag deleted",
  "tag.assign": "Devices tagged",
  "tag.unassign": "Devices untagged",
  "settings.api_key": "API key",
  "settings.endpoint": "Endpoint",
  "settings.history": "History setting",
  "data.clear": "Local data cleared",
  mutation: "API change",
};

type Filter = "all" | "errors";

/** Identifies an entry across refreshes (positions shift as new entries arrive on top). */
const entryKey = (e: AuditEntry) => [e.timestamp, e.action, e.summary, e.outcome, e.error ?? ""].join("\u0000");

/** Read-only view of the local audit log written by the Rust side. */
export function AuditPage() {
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [path, setPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [term, setTerm] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [open, setOpen] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const log = await getAuditLog();
      setEntries(log.entries);
      setPath(log.path);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoaded(true);
    }
  }, []);

  // Refresh while the page is open so new actions show up on their own.
  useEffect(() => {
    void load();
    const id = setInterval(() => void load(), 5000);
    return () => clearInterval(id);
  }, [load]);

  const shown = useMemo(() => {
    const q = term.trim().toLowerCase();
    return entries
      .map((e, i) => ({ e, i, key: entryKey(e) }))
      .filter(({ e }) => filter === "all" || e.outcome === "error")
      .filter(({ e }) => !q || [e.summary, e.action, e.user, e.error ?? ""].some((s) => s.toLowerCase().includes(q)));
  }, [entries, term, filter]);
  const errorCount = entries.filter((e) => e.outcome === "error").length;

  return (
    <>
      <PageHeader
        title="Audit log"
        meta="Changes made from this computer"
        actions={
          <>
            <div className="relative w-56">
              <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
              <input className={`${inputClass} pl-8`} placeholder="Search" value={term} onChange={(e) => setTerm(e.target.value)} />
            </div>
            <Segmented<Filter>
              value={filter}
              onChange={setFilter}
              options={[
                { value: "all", label: "All" },
                { value: "errors", label: errorCount ? `Errors · ${errorCount}` : "Errors" },
              ]}
            />
            <Menu>
              <MenuTrigger asChild>
                <IconButton label="More">
                  <MoreHorizontal />
                </IconButton>
              </MenuTrigger>
              <MenuContent>
                <MenuItem onSelect={() => void copyText(path)} disabled={!path}>
                  <Copy />
                  Copy log file path
                </MenuItem>
              </MenuContent>
            </Menu>
          </>
        }
      />

      <div className="min-h-0 flex-1 overflow-y-auto">
        {error && (
          <div className="p-5">
            <ErrorNote>{error}</ErrorNote>
          </div>
        )}
        {loaded && entries.length === 0 && !error ? (
          <EmptyState icon={<ScrollText />} title="Nothing logged yet">
            Script runs, tag changes and settings changes are recorded here.
          </EmptyState>
        ) : (
          <div className="mx-auto max-w-5xl px-6 py-5">
            <Table
              head={
                <>
                  <th className="w-8" />
                  <th className="w-40">Time</th>
                  <th>Event</th>
                  <th className="w-28">Result</th>
                  <th className="w-28">User</th>
                </>
              }
            >
              {shown.map(({ e, i, key }) => (
                <Fragment key={`${key}-${i}`}>
                  <tr onClick={() => setOpen(open === key ? null : key)} className="cursor-default hover:bg-surface-2/60">
                    <td className="text-faint">
                      <ChevronRight className={`size-3.5 transition-transform ${open === key ? "rotate-90" : ""}`} />
                    </td>
                    <td className="text-muted">
                      <time dateTime={e.timestamp} title={new Date(e.timestamp).toLocaleString()}>
                        {formatDateTime(e.timestamp)}
                      </time>
                    </td>
                    <td className="max-w-0">
                      <div className="truncate" title={ACTION_LABELS[e.action] ?? e.action}>
                        {e.summary}
                      </div>
                      {e.error && <div className="truncate text-xs text-err">{e.error}</div>}
                    </td>
                    <td>{e.outcome === "ok" ? <Status tone="ok" label="OK" /> : <Status tone="err" label="Error" />}</td>
                    <td className="truncate text-muted">{e.user}</td>
                  </tr>
                  {open === key && (
                    <tr>
                      <td />
                      <td colSpan={4} className="pb-3">
                        <pre className="max-h-72 overflow-auto rounded-md bg-surface-2 p-3 font-mono text-xs leading-relaxed">{JSON.stringify(e, null, 2)}</pre>
                      </td>
                    </tr>
                  )}
                </Fragment>
              ))}
            </Table>
            {shown.length === 0 && entries.length > 0 && <p className="py-6 text-center text-sm text-faint">No matching entries</p>}
          </div>
        )}
      </div>
    </>
  );
}
