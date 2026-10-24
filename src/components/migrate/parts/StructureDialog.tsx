import { useEffect, useState } from "react";
import type { StructureProgress, StructureReport } from "../../../lib/migrate/bindings";
import { useCreateStructure, useStructurePreview, useStructureProgress } from "../../../lib/migrate/queries";
import { Button, Dot, ErrorNote, Modal, selectClass } from "../../ui";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

const stepTone = (s?: StructureProgress) => (!s ? "muted" : s.state === "failed" ? "err" : s.state === "alreadyThere" ? "warn" : s.state === "running" ? "accent" : "ok");
const stepLabel = (s?: StructureProgress) =>
  !s ? "waiting" : s.state === "running" ? "creating…" : s.state === "created" ? "created" : s.state === "alreadyThere" ? "already exists" : s.state === "wouldCreate" ? "would be created" : "failed";

function Progress({ items, steps }: { items: { key: string; name: string; under?: string }[]; steps: Record<string, StructureProgress> }) {
  const finished = items.filter((i) => steps[i.key] && steps[i.key].state !== "running").length;
  return (
    <div className="space-y-2 text-sm">
      <p className="text-muted">
        {finished} of {items.length} done
      </p>
      <ul className="max-h-72 space-y-1.5 overflow-y-auto">
        {items.map((i) => {
          const s = steps[i.key];
          return (
            <li key={i.key}>
              <div className="flex items-center gap-2">
                <Dot tone={stepTone(s)} pulse={s?.state === "running"} />
                <span>{i.name}</span>
                {i.under && <span className="text-xs text-faint">under {i.under}</span>}
                <span className="text-xs text-muted">{stepLabel(s)}</span>
              </div>
              {s?.why && <p className="pl-4 text-xs break-words text-err">{s.why}</p>}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function Result({ report }: { report: StructureReport }) {
  return (
    <div className="space-y-2 text-sm">
      {report.wouldCreate.length > 0 && (
        <>
          <p className="rounded-xl bg-warn-soft px-3 py-2 text-warn">Dry run: nothing was created.</p>
          <ul className="max-h-48 list-disc space-y-0.5 overflow-y-auto pl-5 text-muted">
            {report.wouldCreate.map((w) => (
              <li key={w}>{w}</li>
            ))}
          </ul>
        </>
      )}
      {report.customersCreated.length > 0 && <p className="text-ok">{plural(report.customersCreated.length, "customer")} created</p>}
      {report.sitesCreated.length > 0 && <p className="text-ok">{plural(report.sitesCreated.length, "site")} created</p>}
      {report.tokensGenerated > 0 && <p className="text-ok">{plural(report.tokensGenerated, "registration token")} created for them</p>}
      {report.notVisible.length > 0 && (
        <p className="text-warn">
          {report.notVisible.join(", ")} {report.notVisible.length === 1 ? "was" : "were"} created, but the user whose JWT is saved for this server can't see {report.notVisible.length === 1 ? "it" : "them"}, so {report.notVisible.length === 1 ? "it" : "they"} can't be mapped or
          given a token here. N-central only shows a new customer or site to a user whose access group includes it: add {report.notVisible.length === 1 ? "it" : "them"} to that user's access group, then use Re-run matching.
        </p>
      )}
      {report.alreadyThere.length > 0 && (
        <p className="text-warn">
          {report.alreadyThere.join(", ")} already exist{report.alreadyThere.length === 1 ? "s" : ""} on the destination under that name, so nothing was created. Choose {report.alreadyThere.length === 1 ? "it" : "them"} in the table.
        </p>
      )}
      {report.failed.length > 0 && (
        <>
          <p className="text-err">{report.failed.length} couldn't be done</p>
          <ul className="max-h-40 space-y-1 overflow-y-auto pl-4 text-muted">
            {report.failed.map(([name, why]) => (
              <li key={name} className="list-disc break-words">
                {name}: {why}
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

/** Creates the destination's missing customers and sites, after showing exactly what will be made. */
export function StructureDialog({ projectId, onClose }: { projectId: string; onClose: () => void }) {
  const preview = useStructurePreview(projectId, true);
  const create = useCreateStructure(projectId);
  const steps = useStructureProgress(projectId, create.isPending);
  const [serviceOrg, setServiceOrg] = useState("");
  const [dryRun, setDryRun] = useState(false);
  const [report, setReport] = useState<StructureReport | null>(null);
  const p = preview.data;
  useEffect(() => {
    if (p && !serviceOrg && p.serviceOrgs.length === 1) setServiceOrg(p.serviceOrgs[0].id);
  }, [p, serviceOrg]);

  if (report) {
    return (
      <Modal title={report.wouldCreate.length > 0 ? "Dry run" : "Created"} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>Done</Button>}>
        <Result report={report} />
      </Modal>
    );
  }

  if (create.isPending && p) {
    const items = [
      ...p.customers.map((c) => ({ key: `customer:${c.name}`, name: c.name })),
      ...p.sites.map((s) => ({ key: `site:${s.name}`, name: s.name, under: s.under })),
      { key: "tokens:Registration tokens", name: "Registration tokens" },
    ];
    return (
      <Modal title={dryRun ? "Checking what would be created" : "Creating on the destination"} size="lg" onClose={() => undefined} footer={<span className="text-xs text-faint">This can't be stopped part way. It takes a moment.</span>}>
        <Progress items={dryRun ? items.slice(0, -1) : items} steps={steps} />
      </Modal>
    );
  }
  const total = p ? p.customers.length + p.sites.length : 0;
  return (
    <Modal
      title="Create what is missing on the destination"
      size="lg"
      description={p ? `On ${p.destHost}. N-central has no way to delete a customer or site, so anything created here stays until someone removes it in N-central.` : undefined}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" busy={create.isPending} disabled={!p || total === 0 || !serviceOrg} onClick={() => create.mutate({ serviceOrgId: serviceOrg, dryRun }, { onSuccess: setReport })}>
            {dryRun ? "Show what would be created" : total > 0 ? `Create ${total}` : "Create"}
          </Button>
        </>
      }
    >
      {preview.isError ? (
        <ErrorNote>{String(preview.error)}</ErrorNote>
      ) : !p ? (
        <p className="text-sm text-muted">Reading both servers…</p>
      ) : (
        <div className="space-y-3 text-sm">
          {total === 0 && <p className="text-muted">Nothing is missing: every customer and site has a destination or has been left out.</p>}
          {p.customers.length > 0 && (
            <div>
              <p className="font-medium">{plural(p.customers.length, "customer")}</p>
              <ul className="mt-1 max-h-32 list-disc space-y-0.5 overflow-y-auto pl-5 text-muted">
                {p.customers.map((c) => (
                  <li key={c.sourceId}>{c.name}</li>
                ))}
              </ul>
            </div>
          )}
          {p.sites.length > 0 && (
            <div>
              <p className="font-medium">{plural(p.sites.length, "site")}</p>
              <ul className="mt-1 max-h-32 list-disc space-y-0.5 overflow-y-auto pl-5 text-muted">
                {p.sites.map((s) => (
                  <li key={s.sourceId}>
                    {s.name} <span className="text-faint">under {s.under}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {p.skipped.length > 0 && (
            <div>
              <p className="text-warn">{plural(p.skipped.length, "site")} can't be created</p>
              <ul className="mt-1 max-h-24 list-disc space-y-0.5 overflow-y-auto pl-5 text-xs text-muted">
                {p.skipped.map((s) => (
                  <li key={s}>{s}</li>
                ))}
              </ul>
            </div>
          )}
          {total > 0 && (
            <>
              <label className="block space-y-1">
                <span className="text-xs font-medium text-muted">Customers go under this service organization</span>
                <select className={selectClass} value={serviceOrg} onChange={(e) => setServiceOrg(e.target.value)}>
                  {p.serviceOrgs.length !== 1 && <option value="">Choose one</option>}
                  {p.serviceOrgs.map((so) => (
                    <option key={so.id} value={so.id}>
                      {so.name}
                    </option>
                  ))}
                </select>
              </label>
              <p className="text-xs text-muted">Only the name is copied. Registration tokens are created for them afterwards.</p>
              <label className="flex cursor-pointer items-start gap-2.5">
                <input type="checkbox" className="mt-1" checked={dryRun} onChange={(e) => setDryRun(e.target.checked)} />
                <span>Dry run: list what would be created, change nothing</span>
              </label>
            </>
          )}
          {create.isError && <ErrorNote>{String(create.error)}</ErrorNote>}
        </div>
      )}
    </Modal>
  );
}
