import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight, Copy, MoreHorizontal, ScrollText, Search } from "lucide-react";
import { getAuditLog, type AuditEntry } from "../lib/api";
import { copyText, formatDateTime } from "../lib/format";
import { AuditDetail, actionLabel } from "./audit/AuditDetail";
import { Card, EmptyState, ErrorNote, IconButton, Menu, MenuContent, MenuItem, MenuTrigger, PageBody, PageHeader, Segmented, Status, Table, inputClass } from "./ui";

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
            <div className="relative w-64 max-w-full">
              <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
              <input className={`${inputClass} h-9 pl-8`} placeholder="Search" aria-label="Search the log" value={term} onChange={(e) => setTerm(e.target.value)} />
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
                <IconButton label="More" className="size-9 border border-border-strong">
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

      <PageBody className="space-y-4">
        {error && <ErrorNote>{error}</ErrorNote>}
        {loaded && entries.length === 0 && !error ? (
          <Card>
            <EmptyState icon={<ScrollText />} title="Nothing logged yet">
              Script runs, tag changes and settings changes are recorded here.
            </EmptyState>
          </Card>
        ) : (
          <div>
            <Table
              head={
                <>
                  <th className="w-8" />
                  <th className="w-40">Time</th>
                  <th>Event</th>
                  <th className="w-28">Result</th>
                  <th className="w-40">User</th>
                </>
              }
            >
              {shown.map(({ e, i, key }) => (
                <Fragment key={`${key}-${i}`}>
                  <tr onClick={() => setOpen(open === key ? null : key)} aria-expanded={open === key} className={`cursor-default hover:bg-surface-2/60 ${open === key ? "bg-surface-2/60" : ""}`}>
                    <td className="text-faint">
                      <ChevronRight className={`size-3.5 transition-transform ${open === key ? "rotate-90" : ""}`} />
                    </td>
                    <td className="text-muted">
                      <time dateTime={e.timestamp} title={new Date(e.timestamp).toLocaleString()}>
                        {formatDateTime(e.timestamp)}
                      </time>
                    </td>
                    <td className="max-w-0">
                      <div className="truncate" title={e.summary}>
                        <span className="mr-2 text-xs text-faint">{actionLabel(e.action)}</span>
                        {e.summary}
                      </div>
                      {e.error && <div className="truncate text-xs text-err">{e.error}</div>}
                    </td>
                    <td>{e.outcome === "ok" ? <Status tone="ok" label="OK" /> : <Status tone="err" label="Error" />}</td>
                    <td className="max-w-0 truncate text-muted" title={e.user}>{e.user}</td>
                  </tr>
                  {open === key && (
                    <tr>
                      <td />
                      <td colSpan={4} className="pb-3">
                        <AuditDetail entry={e} />
                      </td>
                    </tr>
                  )}
                </Fragment>
              ))}
            </Table>
            {shown.length === 0 && entries.length > 0 && <p className="py-6 text-center text-sm text-faint">No matching entries</p>}
          </div>
        )}
      </PageBody>
    </>
  );
}
