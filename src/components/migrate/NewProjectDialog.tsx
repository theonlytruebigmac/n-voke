import { useState } from "react";
import { X } from "lucide-react";
import { searchOrganizations } from "../../lib/api";
import type { Organization } from "../../lib/types";
import type { Delivery, ProjectSummary } from "../../lib/migrate/bindings";
import { useProjectActions, useServers } from "../../lib/migrate/queries";
import { Combobox } from "../Combobox";
import { Button, ErrorNote, Field, Modal, selectClass, inputClass } from "../ui";

type Linked = "notLinked" | "sameTenant" | "otherTenant";

const LINKED: { value: Linked; title: string; text: string }[] = [
  { value: "notLinked", title: "Not yet", text: "Devices leave Ecoverse until it's linked. Progress after the move is tracked over REST." },
  { value: "sameTenant", title: "Same tenant as the source", text: "Devices show up in the same Ecoverse tenant again after the move." },
  { value: "otherTenant", title: "Another tenant", text: "Devices show up in the destination's own Ecoverse tenant. Needs an Ecoverse token saved for the destination server." },
];

const DELIVERY: { value: Delivery; title: string; text: string }[] = [
  { value: "ecoverseFirst", title: "Ecoverse first, N-central as the fallback", text: "Devices Ecoverse can reach start through it; the rest, and any that fail, go through N-central." },
  { value: "restOnly", title: "N-central only", text: "Every device starts through N-central. Nothing is read from Ecoverse." },
];

/** Name, two servers, how the destination relates to Ecoverse, and the Ecoverse organizations to search (screen B). */
export function NewProjectDialog({
  ecoverseReady,
  onClose,
  onCreated,
  onManageServers,
}: {
  /** An Ecoverse API key is set, so organizations can be searched. */
  ecoverseReady: boolean;
  onClose: () => void;
  onCreated: (project: ProjectSummary) => void;
  onManageServers: () => void;
}) {
  const servers = useServers();
  const { create } = useProjectActions();
  const list = servers.data ?? [];

  const [name, setName] = useState("");
  const [sourceId, setSourceId] = useState("");
  const [destId, setDestId] = useState("");
  const [linked, setLinked] = useState<Linked>("notLinked");
  const [delivery, setDelivery] = useState<Delivery>("ecoverseFirst");
  const [orgs, setOrgs] = useState<Organization[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [showEco, setShowEco] = useState(false);

  const same = sourceId !== "" && sourceId === destId;
  const dest = list.find((s) => s.id === destId);
  const destHasOwnToken = dest?.ecoverse === "own" && dest.hasEcoverseToken;
  const otherTenantBlocked = !destHasOwnToken;
  const valid = name.trim() !== "" && sourceId !== "" && destId !== "" && !(linked === "otherTenant" && otherTenantBlocked);
  const source = list.find((s) => s.id === sourceId);
  const sourceNoEco = source?.ecoverse === "none";

  const submit = () => {
    setError(null);
    create.mutate(
      {
        name: name.trim(),
        sourceServerId: sourceId,
        destServerId: destId,
        destEcoverse: linked === "otherTenant" ? { kind: "otherTenant", tenant_id: dest?.ecoverseTenant ?? dest?.host ?? "" } : { kind: linked },
        sourceEcoverseOrgs: delivery === "restOnly" || sourceNoEco || source?.ecoverse === "own" ? [] : orgs.map((o) => o.id),
        delivery,
      },
      { onSuccess: onCreated, onError: (e) => setError(String(e)) },
    );
  };

  return (
    <Modal
      title="New project"
      description="A project moves devices from one N-central server to another, or reinstalls them on the same one."
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={submit} busy={create.isPending} disabled={!valid}>
            Create
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label="Name">
          <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} placeholder="Contoso move" autoFocus />
        </Field>

        {list.length === 0 && !servers.isPending ? (
          <div className="rounded-md border border-border px-3 py-3 text-sm text-muted">
            Add the servers first.{" "}
            <button className="text-accent underline-offset-2 hover:underline" onClick={onManageServers}>
              Manage servers
            </button>
          </div>
        ) : (
          <>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Move devices from">
                <select className={`${selectClass} w-full`} value={sourceId} onChange={(e) => setSourceId(e.target.value)}>
                  <option value="">Choose a server</option>
                  {list.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.label}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="to">
                <select className={`${selectClass} w-full`} value={destId} onChange={(e) => setDestId(e.target.value)}>
                  <option value="">Choose a server</option>
                  {list.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.label}
                    </option>
                  ))}
                </select>
              </Field>
            </div>
            <p className="-mt-2 text-xs text-faint">
              {same ? "Same server: the agents are reinstalled in place." : "Choose the same server twice to reinstall the agents in place."}{" "}
              <button className="text-accent underline-offset-2 hover:underline" onClick={onManageServers}>
                Manage servers
              </button>
            </p>
          </>
        )}

        <div className="rounded-lg border border-border">
          <button type="button" className="flex w-full items-center justify-between gap-3 px-3 py-2 text-left" aria-expanded={showEco} onClick={() => setShowEco((v) => !v)}>
            <span className="text-sm">
              <span className="font-medium">Ecoverse</span>
              <span className="block text-xs text-muted">
                {DELIVERY.find((o) => o.value === delivery)?.title} · destination linked to Ecoverse: {LINKED.find((o) => o.value === linked)?.title.toLowerCase()}
              </span>
              {delivery === "ecoverseFirst" && sourceNoEco && <span className="block text-xs text-warn">{source?.label} is set to not use Ecoverse, so its devices will go through N-central.</span>}
            </span>
            <span className="text-xs text-accent">{showEco ? "Hide" : "Change"}</span>
          </button>
          {showEco && <div className="space-y-4 border-t border-border px-3 py-3">
        <fieldset className="space-y-2">
          <legend className="text-xs font-medium text-muted">How should devices be reached to start the move?</legend>
          {DELIVERY.map((o) => (
            <label key={o.value} className="flex cursor-pointer items-start gap-2.5 rounded-md border border-border px-3 py-2 has-[:checked]:border-accent has-[:checked]:bg-accent-soft">
              <input type="radio" name="delivery" className="mt-1" checked={delivery === o.value} onChange={() => setDelivery(o.value)} />
              <span className="text-sm">
                <span className="font-medium">{o.title}</span>
                <span className="block text-xs text-muted">{o.text}</span>
              </span>
            </label>
          ))}
          {delivery === "ecoverseFirst" && sourceNoEco && <p className="text-xs text-warn">{source?.label} is set to not use Ecoverse, so its devices will go through N-central. Change that under Manage servers.</p>}
        </fieldset>

        <fieldset className="space-y-2">
          <legend className="text-xs font-medium text-muted">Is the destination linked to Ecoverse?</legend>
          {LINKED.map((o) => {
            const blocked = o.value === "otherTenant" && otherTenantBlocked;
            return (
              <label
                key={o.value}
                className={`flex items-start gap-2.5 rounded-md border border-border px-3 py-2 has-[:checked]:border-accent has-[:checked]:bg-accent-soft ${blocked ? "cursor-not-allowed opacity-60" : "cursor-pointer"}`}
              >
                <input type="radio" name="linked" className="mt-1" disabled={blocked} checked={linked === o.value} onChange={() => setLinked(o.value)} />
                <span className="text-sm">
                  <span className="font-medium">{o.title}</span>
                  <span className="block text-xs text-muted">{o.text}</span>
                </span>
              </label>
            );
          })}
          <p className="text-xs text-faint">A destination deploys Ecoverse agents once cloud eventing is on and SSO is enabled there.</p>
        </fieldset>

        {delivery === "ecoverseFirst" && !sourceNoEco && source?.ecoverse === "own" && (
          <p className="text-xs text-faint">Ecoverse is read with {source.label}'s own token, across every organization that token can see.</p>
        )}

        {ecoverseReady && delivery === "ecoverseFirst" && !sourceNoEco && source?.ecoverse !== "own" && (
          <Field
            label="Narrow the Ecoverse search to certain organizations (optional)"
            hint="Ecoverse organizations are the customers and sites in Ecoverse. Leave this empty to search all of them: devices are matched to this N-central server by its address, so nothing else is picked up."
          >
            <div className="space-y-2">
              <Combobox<Organization>
                value={null}
                onChange={(o) => setOrgs((cur) => (cur.some((c) => c.id === o.id) ? cur : [...cur, o]))}
                search={searchOrganizations}
                getKey={(o) => o.id}
                placeholder="Add an organization"
                searchPlaceholder="Search organizations"
                renderValue={(o) => o.name}
                renderItem={(o) => <span className="truncate">{o.name}</span>}
              />
              {orgs.length > 0 && (
                <div className="flex flex-wrap gap-1.5">
                  {orgs.map((o) => (
                    <span key={o.id} className="inline-flex items-center gap-1 rounded-md bg-surface-3 py-0.5 pr-1 pl-2 text-xs">
                      {o.name}
                      <button aria-label={`Remove ${o.name}`} className="rounded p-0.5 text-faint hover:bg-surface-2 hover:text-text" onClick={() => setOrgs((cur) => cur.filter((c) => c.id !== o.id))}>
                        <X className="size-3" />
                      </button>
                    </span>
                  ))}
                </div>
              )}
            </div>
          </Field>
        )}
          </div>}
        </div>

        {error && <ErrorNote>{error}</ErrorNote>}
      </div>
    </Modal>
  );
}
