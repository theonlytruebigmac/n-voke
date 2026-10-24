import { useState, type ReactNode } from "react";
import type { AuditEntry } from "../../lib/api";
import { Button, CopyButton, Status } from "../ui";

export const ACTION_LABELS: Record<string, string> = {
  "script.run": "Script run",
  "tag.create": "Tag created",
  "tag.update": "Tag updated",
  "tag.delete": "Tag deleted",
  "tag.assign": "Devices tagged",
  "tag.unassign": "Devices untagged",
  "settings.api_key": "API key",
  "settings.endpoint": "Endpoint",
  "settings.history": "History setting",
  "settings.migration": "Migration settings",
  "settings.ncentral_server": "N-central server",
  "data.clear": "Local data cleared",
  mutation: "API change",
  "migration.launch": "Migration launch (Ecoverse)",
  "migration.launch.rest": "N-central task",
  "migration.cleanup": "Cleanup",
  "migration.device.choice": "Device choice",
  "migration.mapping.set": "Mapping changed",
  "migration.match": "Device matched",
  "migration.options": "Migration options",
  "migration.project.delete": "Project deleted",
  "migration.report": "Report saved",
  "migration.resolve": "Marked resolved",
  "migration.retire": "Old records retired",
  "migration.retry": "Retry",
  "migration.script.export": "Script saved",
  "migration.structure.customer": "Customer created",
  "migration.structure.site": "Site created",
};

/** "migration.device.choice" → "Migration device choice", for actions without a label of their own. */
export const actionLabel = (action: string) => ACTION_LABELS[action] ?? sentence(action.split(/[._]/).join(" "));

const ACRONYMS = new Set(["id", "ids", "url", "api", "jwt", "os", "sso", "json", "uuid", "mac", "smbios"]);

/** "customerId" → "Customer ID", "run_as" → "Run as". */
function fieldLabel(key: string): string {
  const words = key
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .split(/[\s._-]+/)
    .filter(Boolean)
    .map((w) => (ACRONYMS.has(w.toLowerCase()) ? w.toUpperCase() : w.toLowerCase()));
  return sentence(words.join(" "));
}

const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

type Json = string | number | boolean | null | undefined | Json[] | { [key: string]: Json };
const isObject = (v: Json): v is { [key: string]: Json } => typeof v === "object" && v !== null && !Array.isArray(v);
const isEmpty = (v: Json) => v === null || v === undefined || v === "" || (Array.isArray(v) && v.length === 0) || (isObject(v) && Object.keys(v).length === 0);

/** A value on one line: words for yes/no, IDs and tokens in monospace so they read as identifiers. */
function Value({ value }: { value: Json }) {
  if (typeof value === "boolean") return <>{value ? "Yes" : "No"}</>;
  if (typeof value === "number") return <span className="font-mono text-xs">{String(value)}</span>;
  const text = String(value);
  const identifier = !/\s/.test(text) && text.length > 8;
  return <span className={identifier ? "font-mono text-xs" : undefined}>{text}</span>;
}

/** One level of an entry's details as labelled rows; nested objects become their own small sections. */
function Fields({ value }: { value: { [key: string]: Json } }) {
  const entries = Object.entries(value).filter(([, v]) => !isEmpty(v));
  const flat = entries.filter(([, v]) => !isObject(v) && !(Array.isArray(v) && v.some((x) => typeof x === "object" && x !== null)));
  const nested = entries.filter((e) => !flat.includes(e));
  return (
    <div className="space-y-3">
      {flat.length > 0 && (
        <dl className="grid grid-cols-[minmax(110px,160px)_minmax(0,1fr)] gap-x-4 gap-y-1.5">
          {flat.map(([k, v]) => (
            <Row key={k} label={fieldLabel(k)}>
              {Array.isArray(v) ? v.map((x) => String(x)).join(", ") : <Value value={v} />}
            </Row>
          ))}
        </dl>
      )}
      {nested.map(([k, v]) => (
        <section key={k} className="space-y-2">
          <h4 className="text-xs font-semibold tracking-wide text-faint uppercase">{fieldLabel(k)}</h4>
          <div className="border-l-2 border-border pl-3">
            {Array.isArray(v) ? <List items={v} /> : <Fields value={v as { [key: string]: Json }} />}
          </div>
        </section>
      ))}
    </div>
  );
}

/**
 * A list of objects. Name/value pairs (a script's inputs, a task's parameters) read as "name: value" rows; anything
 * else gets one small block per item.
 */
function List({ items }: { items: Json[] }) {
  const pairs = items.every((x) => isObject(x) && "name" in x && "value" in x);
  if (pairs)
    return (
      <dl className="grid grid-cols-[minmax(110px,160px)_minmax(0,1fr)] gap-x-4 gap-y-1.5">
        {items.map((x, i) => {
          const o = x as { [key: string]: Json };
          return (
            <Row key={i} label={String(o.name)}>
              <Value value={o.value} />
            </Row>
          );
        })}
      </dl>
    );
  return (
    <div className="space-y-3">
      {items.map((x, i) => (
        <div key={i}>{isObject(x) ? <Fields value={x} /> : <Value value={x} />}</div>
      ))}
    </div>
  );
}

function Row({ label, children }: { label: ReactNode; children: ReactNode }) {
  return (
    <>
      <dt className="text-faint">{label}</dt>
      <dd className="min-w-0 wrap-break-word">{children}</dd>
    </>
  );
}

const SHOWN_ABOVE = new Set(["timestamp", "user", "action", "summary", "outcome", "error", "details"]);

/** An audit entry opened up: what happened as fields, the error in full, the details as labelled rows, raw JSON on request. */
export function AuditDetail({ entry }: { entry: AuditEntry }) {
  const [raw, setRaw] = useState(false);
  const json = JSON.stringify(entry, null, 2);
  // Whatever else the entry carries goes with its details, so nothing is hidden.
  const extra = Object.fromEntries(Object.entries(entry).filter(([k]) => !SHOWN_ABOVE.has(k))) as { [key: string]: Json };
  const details = { ...extra, ...((entry.details ?? {}) as { [key: string]: Json }) };
  const hasDetails = Object.values(details).some((v) => !isEmpty(v));

  return (
    <div className="space-y-4 rounded-xl border border-border bg-bg p-4 text-sm">
      <dl className="grid grid-cols-[minmax(110px,160px)_minmax(0,1fr)] gap-x-4 gap-y-1.5">
        <Row label="Time">{new Date(entry.timestamp).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "medium" })}</Row>
        <Row label="Event">{actionLabel(entry.action)}</Row>
        <Row label="Summary">{entry.summary}</Row>
        <Row label="Result">{entry.outcome === "ok" ? <Status tone="ok" label="OK" /> : <Status tone="err" label="Error" />}</Row>
        <Row label="User">{entry.user}</Row>
      </dl>

      {entry.error && (
        <div className="rounded-lg border border-err/30 bg-err-soft/40 px-3 py-2">
          <div className="mb-0.5 text-xs font-semibold text-err">Error</div>
          <p className="font-mono text-xs leading-relaxed wrap-break-word text-text" data-selectable>
            {entry.error}
          </p>
        </div>
      )}

      {hasDetails && (
        <section className="space-y-2">
          <h3 className="text-xs font-semibold tracking-wide text-faint uppercase">Details</h3>
          <Fields value={details} />
        </section>
      )}

      <div className="flex items-center gap-2 border-t border-border pt-3">
        <Button size="sm" variant="ghost" onClick={() => setRaw((r) => !r)} aria-expanded={raw}>
          {raw ? "Hide raw" : "Show raw"}
        </Button>
        <CopyButton text={json} label="Copy raw JSON" />
      </div>
      {raw && <pre className="max-h-72 overflow-auto rounded-lg border border-border bg-sidebar p-3 font-mono text-xs leading-relaxed text-muted">{json}</pre>}
    </div>
  );
}
