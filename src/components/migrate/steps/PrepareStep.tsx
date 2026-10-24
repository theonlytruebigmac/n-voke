import { Download } from "lucide-react";
import { useEffect, useState } from "react";
import { defaultScriptOrg, searchScriptOrgs as searchOrgs } from "../../../lib/migrate/api";
import type { EcoverseCard as EcoverseCardData, LinkChoice, PrepareView, RestCard as RestCardData, ScriptOrg, ScriptSide, ScriptState } from "../../../lib/migrate/bindings";
import { parseOptions, sameOptions, toForm, type OptionsForm } from "../../../lib/migrate/options";
import { usePrepare, usePrepareActions } from "../../../lib/migrate/queries";
import { Combobox } from "../../Combobox";
import { prepareChecklist } from "../../../lib/migrate/checklist";
import { OptionsFields } from "../parts/OptionsFields";
import { Button, CopyButton, Dot, ErrorNote, Field, Status, inputClass } from "../../ui";

const STATE: Record<ScriptState, { tone: "ok" | "warn" | "muted"; label: string }> = {
  current: { tone: "ok", label: "Up to date" },
  outdated: { tone: "warn", label: "Outdated" },
  notThere: { tone: "muted", label: "Not there yet" },
};

function Card({ id, title, aside, attention, children }: { id?: string; title: string; aside?: React.ReactNode; attention?: boolean; children: React.ReactNode }) {
  return (
    <section id={id} className={`scroll-mt-4 space-y-3 rounded-2xl border bg-surface px-5 py-4 ${attention ? "border-warn/40" : "border-border"}`}>
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
        <h3 className="font-semibold">{title}</h3>
        {aside}
      </div>
      {children}
    </section>
  );
}

function EcoverseCard({ projectId, view, side, card }: { projectId: string; view: PrepareView; side: ScriptSide; card: EcoverseCardData }) {
  const { publish } = usePrepareActions(projectId);
  const [org, setOrg] = useState<ScriptOrg | null>(card.org);
  const [message, setMessage] = useState<string | null>(null);
  const destination = side === "destination";
  const title = destination ? "Ecoverse on the destination" : "Ecoverse";

  // Nobody has chosen yet: offer the tenant's service organization, which is almost always the right one.
  useEffect(() => {
    if (card.org || card.notUsed) return;
    let live = true;
    defaultScriptOrg(projectId, side)
      .then((found) => live && found && setOrg((current) => current ?? found))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [projectId, side, card.org, card.notUsed]);

  if (card.notUsed) {
    return (
      <Card id={`prepare-${side}-ecoverse`} title={title}>
        <p className="text-sm text-muted">{card.notUsed}</p>
      </Card>
    );
  }
  return (
    <Card id={`prepare-${side}-ecoverse`} title={title} attention={!destination && card.state !== "current"} aside={<Status tone={STATE[card.state].tone} label={STATE[card.state].label} />}>
      <p className="text-sm text-muted">
        {destination
          ? "The destination is in its own Ecoverse tenant, so a device that has moved is reached with a copy of the script published there. Probe and Cleanup use it. You don't have to do anything: it is published in the tenant's service organization the first time one of them needs it. Publish it now if you'd rather see it done."
          : "The script is published in an Ecoverse organization, then run on each device from there. Choose the organization that holds the devices: the service organization works for all of its customers."}
      </p>
      <Field label="Publish in organization">
        <Combobox<ScriptOrg>
          value={org}
          onChange={(o) => {
            setMessage(null);
            setOrg(o);
          }}
          search={(term) => searchOrgs(projectId, side, term)}
          getKey={(o) => o.id}
          placeholder="Choose an organization"
          searchPlaceholder="Search organizations"
          renderValue={(o) => o.name}
          renderItem={(o) => <span className="truncate">{o.name}</span>}
        />
      </Field>
      <div className="flex items-center justify-between gap-3">
        <p className="min-w-0 text-xs text-faint">{view.scriptName}</p>
        <Button
          variant="primary"
          size="sm"
          busy={publish.isPending}
          disabled={!org}
          onClick={() => {
            setMessage(null);
            if (org) publish.mutate({ side, org }, { onSuccess: setMessage });
          }}
        >
          {card.state === "notThere" ? "Publish" : card.state === "outdated" ? "Publish the new version" : "Publish again"}
        </Button>
      </div>
      {message && <p className="text-sm text-ok">{message}</p>}
      {publish.isError && <ErrorNote>{String(publish.error)}</ErrorNote>}
    </Card>
  );
}

function RestCard({ projectId, view, side, card }: { projectId: string; view: PrepareView; side: ScriptSide; card: RestCardData }) {
  const { exportScript, restScript } = usePrepareActions(projectId);
  const [itemId, setItemId] = useState(card.itemId ?? "");
  const [version, setVersion] = useState(card.uploadedVersion ?? view.scriptVersion);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const destination = side === "destination";
  const intro = destination
    ? "Optional. Only used to probe or clean up a device that has moved when Ecoverse can't reach it (for example it isn't connected there yet). If Ecoverse works for the destination you can skip this."
    : view.delivery === "restOnly"
      ? "Every device is reached this way."
      : "The fallback, for devices Ecoverse can't reach and ones that fail through it. Optional, but without it those devices can't be launched.";

  return (
    <Card id={`prepare-${side}-rest`} title={destination ? "N-central repository on the destination" : "N-central repository (REST path)"} aside={<Status tone={STATE[card.state].tone} label={STATE[card.state].label} />}>
      <p className="text-sm text-muted">{intro}</p>
      <ol className="space-y-3 text-sm">
        <li className="space-y-1.5">
          <div className="flex items-center gap-2">
            <span className="text-muted">1.</span>
            <Button size="sm" busy={exportScript.isPending} onClick={() => exportScript.mutate(undefined, { onSuccess: setSavedPath })}>
              <Download />
              Save the script
            </Button>
            <span className="text-xs text-faint">{view.fileName}</span>
          </div>
          {savedPath && (
            <div className="flex items-center gap-2 pl-5 text-xs text-ok">
              <span className="break-all">Saved to {savedPath}</span>
              <CopyButton text={savedPath} label="Copy path" />
            </div>
          )}
          {exportScript.isError && <ErrorNote>{String(exportScript.error)}</ErrorNote>}
        </li>
        <li>
          <span className="text-muted">2.</span> Upload it to {card.serverLabel}'s script repository ({card.serverHost}) and turn on <em>Enable API</em> for it.
        </li>
        <li className="space-y-1.5">
          <span className="text-muted">3.</span> Enter the item ID N-central shows for it.
          <div className="flex flex-wrap items-end gap-3 pl-5">
            <Field label="Item ID">
              <input className={`${inputClass} w-44`} value={itemId} onChange={(e) => setItemId(e.target.value)} inputMode="numeric" placeholder="1271448998" spellCheck={false} />
            </Field>
            <Field label="Script version">
              <input className={`${inputClass} w-44`} value={version} onChange={(e) => setVersion(e.target.value)} spellCheck={false} />
            </Field>
            <Button size="sm" busy={restScript.isPending} disabled={!itemId.trim()} onClick={() => restScript.mutate({ side, itemId, version })}>
              Save
            </Button>
          </div>
          {card.state === "outdated" && (
            <p className="pl-5 text-xs text-warn">The saved item is version {card.uploadedVersion}, and this build has {view.scriptVersion}. N-central can't replace a file: delete the repository item, upload the new file, and enter the new item ID.</p>
          )}
          {restScript.isError && <ErrorNote>{String(restScript.error)}</ErrorNote>}
        </li>
      </ol>
    </Card>
  );
}

const LINK_CHOICES: { value: LinkChoice; title: string; text: string }[] = [
  { value: "notLinked", title: "Not linked", text: "Ecoverse is not looked at after a device moves. Devices are tracked through N-central only, and nothing is claimed about Ecoverse." },
  { value: "sameTenant", title: "Same tenant as the source", text: "Devices show up in the same Ecoverse tenant again after the move. Ecoverse is checked, and a device isn't fully working until it is sending asset data there." },
  { value: "otherTenant", title: "Another tenant", text: "Devices show up in the destination's own Ecoverse tenant. Needs an Ecoverse token saved for the destination server." },
];

/** Whether the destination is linked to Ecoverse decides whether a moved device is checked there, so it is asked here and can be changed. */
function EcoverseLinkCard({ projectId, view }: { projectId: string; view: PrepareView }) {
  const { ecoverseLink } = usePrepareActions(projectId);
  return (
    <Card id="prepare-ecoverse-link" title="Is the destination linked to Ecoverse?" aside={<Status tone={view.ecoverseLink === "notLinked" ? "muted" : "ok"} label={view.ecoverseLink === "notLinked" ? "Not checked" : "Checked"} />}>
      <p className="text-sm text-muted">
        This decides what <span className="font-medium">healthy</span> means. When the destination is linked, a device only counts as fully working once Ecoverse is receiving its asset data (name, serial number, addresses). When it isn't, Ecoverse is never read and the app says so.
      </p>
      <fieldset className="space-y-2" disabled={ecoverseLink.isPending}>
        {LINK_CHOICES.map((o) => {
          const blocked = o.value === "otherTenant" && !view.otherTenantAllowed;
          return (
            <label
              key={o.value}
              className={`flex items-start gap-2.5 rounded-md border border-border px-3 py-2 has-[:checked]:border-accent has-[:checked]:bg-accent-soft ${blocked ? "cursor-not-allowed opacity-60" : "cursor-pointer"}`}
            >
              <input type="radio" name="ecoverse-link" className="mt-1" disabled={blocked} checked={view.ecoverseLink === o.value} onChange={() => ecoverseLink.mutate(o.value)} />
              <span className="text-sm">
                <span className="font-medium">{o.title}</span>
                <span className="block text-xs text-muted">{o.text}</span>
              </span>
            </label>
          );
        })}
      </fieldset>
      {ecoverseLink.isError && <ErrorNote>{String(ecoverseLink.error)}</ErrorNote>}
    </Card>
  );
}

function OptionsCard({ projectId, view }: { projectId: string; view: PrepareView }) {
  const { options } = usePrepareActions(projectId);
  const [form, setForm] = useState<OptionsForm>(toForm(view.options));
  useEffect(() => setForm(toForm(view.options)), [view.options]);
  const parsed = parseOptions(form);
  const errors = parsed.ok ? {} : parsed.errors;
  const changed = !parsed.ok || !sameOptions(parsed.options, view.options);

  return (
    <Card title="Options">
      <div className="space-y-4">
        <OptionsFields form={form} setForm={setForm} errors={errors} deliveryLocked={view.deliveryLocked} />

        {options.isError && <ErrorNote>{String(options.error)}</ErrorNote>}
        <div className="flex items-center justify-end gap-3">
          {options.isSuccess && !changed && <span className="text-xs text-ok">Saved</span>}
          <Button variant="primary" size="sm" busy={options.isPending} disabled={!parsed.ok || !changed} onClick={() => parsed.ok && options.mutate(parsed.options)}>
            Save options
          </Button>
        </div>
      </div>
    </Card>
  );
}

/** What Prepare still asks for, at a glance, each line taking you to its card. */
function Checklist({ view }: { view: PrepareView }) {
  const { items, ready } = prepareChecklist(view);
  const tone = { ready: "ok", needed: "warn", optional: "muted" } as const;
  return (
    <section className="rounded-2xl border border-border bg-surface">
      <div className="flex items-center justify-between gap-3 border-b border-border px-5 py-3.5">
        <h3 className="text-base font-semibold">Ready to plan?</h3>
        <span className={`font-semibold ${ready ? "text-ok" : "text-warn"}`}>{ready ? "Yes" : "Not yet"}</span>
      </div>
      <ul className="divide-y divide-border">
        {items.map((i) => (
          <li key={i.anchor} className="flex items-start gap-2.5 px-5 py-2.5 text-sm">
            <span className="mt-1.5">
              <Dot tone={tone[i.status]} />
            </span>
            <button className="min-w-0 flex-1 text-left" onClick={() => document.getElementById(i.anchor)?.scrollIntoView({ behavior: "smooth", block: "start" })}>
              <span className="block">{i.label}</span>
              <span className="block text-xs text-muted">
                {i.status === "optional" ? "Optional · " : ""}
                {i.text}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** What goes to the devices, and how the project behaves (step 3 in docs/migration/11-gui-design.md). */
export function PrepareStep({ projectId, onBack, onContinue }: { projectId: string; onBack: () => void; onContinue: () => void }) {
  const prepare = usePrepare(projectId);
  const view = prepare.data;
  if (prepare.isError) return <ErrorNote>{String(prepare.error)}</ErrorNote>;
  if (!view) return <p className="text-sm text-muted">Loading…</p>;
  const { ready } = prepareChecklist(view);
  return (
    <div className="flex flex-wrap items-start gap-6">
      <div className="min-w-0 flex-[3_1_480px] space-y-4">
        <EcoverseCard projectId={projectId} view={view} side="source" card={view.ecoverse} />
        <RestCard projectId={projectId} view={view} side="source" card={view.rest} />
        {view.destination?.ecoverse && <EcoverseCard projectId={projectId} view={view} side="destination" card={view.destination.ecoverse} />}
        {view.destination && <RestCard projectId={projectId} view={view} side="destination" card={view.destination.rest} />}
        <EcoverseLinkCard projectId={projectId} view={view} />
        <OptionsCard projectId={projectId} view={view} />
      </div>
      <aside className="sticky top-0 flex min-w-0 flex-[2_1_280px] flex-col gap-3">
        <Checklist view={view} />
        <p className="px-1 text-xs text-faint">Endpoint script {view.scriptVersion}</p>
        <div className="flex gap-2">
          <Button onClick={onBack}>Back to Map</Button>
          <Button variant={ready ? "primary" : "secondary"} className="flex-1" onClick={onContinue}>
            {ready ? "Continue to Plan" : "Continue to Plan anyway"}
          </Button>
        </div>
      </aside>
    </div>
  );
}
