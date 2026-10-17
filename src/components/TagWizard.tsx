import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Check, ChevronLeft, Loader2, Search } from "lucide-react";
import { assignStaticTags, createDynamicTag, createStaticTag, previewAssets } from "../lib/api";
import { cacheTag, orgsQuery, refreshTagMembership } from "../lib/collections";
import { humanize } from "../lib/format";
import { activeConditions, describeRule, emptyRule, ruleToFilter, validateTagName, type Rule } from "../lib/rules";
import { matches } from "../lib/sync";
import type { Asset, Organization, Tag, TagKind } from "../lib/types";
import { DevicePickerField } from "./DevicePickerField";
import { RuleBuilder } from "./RuleBuilder";
import { AgentDot, TagKindIcon } from "./status";
import { Button, ErrorNote, FormRow, Segmented, Table, inputClass } from "./ui";

/*
 * New-tag wizard, following N-central's flow:
 *   Static:  Details → Access → Devices (optional) → Review
 *   Dynamic: Details → Access → Filters → Review
 */

type Step = "details" | "access" | "members" | "review";
const STEPS: Step[] = ["details", "access", "members", "review"];

type AccessCategory = "organization" | "customer" | "site";
const CATEGORY_TYPES: Record<AccessCategory, string[]> = {
  organization: ["ServiceOrganization", "Partner"],
  customer: ["Customer"],
  site: ["Site"],
};

const orgKind = (o: Organization) => (o.__typename ? humanize(o.__typename.replace(/([a-z])([A-Z])/g, "$1_$2")) : "");

export interface TagWizardProps {
  initialName?: string;
  initialKind?: TagKind;
  initialOrg?: Organization | null;
  initialDevices?: Asset[];
  onCancel: () => void;
  onCreated: (tag: Tag) => void;
}

export function TagWizard({ initialName = "", initialKind = "StaticTag", initialOrg = null, initialDevices = [], onCancel, onCreated }: TagWizardProps) {
  const [step, setStep] = useState<Step>("details");
  const [kind, setKind] = useState<TagKind>(initialKind);
  const [name, setName] = useState(initialName);
  const [description, setDescription] = useState("");
  const [category, setCategory] = useState("");
  const [org, setOrg] = useState<Organization | null>(initialOrg);
  const [devices, setDevices] = useState<Asset[]>(initialDevices);
  const [rule, setRule] = useState<Rule>(emptyRule());
  const [touched, setTouched] = useState<Set<Step>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** A static tag that was created but couldn't get its devices; a retry only adds them. */
  const createdRef = useRef<Tag | null>(null);

  const nameError = validateTagName(name.trim());
  const stepError: Record<Step, string | null> = {
    details: nameError,
    access: org ? null : "Choose where the tag can be used",
    members: kind === "DynamicTag" && activeConditions(rule).length === 0 ? "Add at least one condition" : null,
    review: null,
  };
  const index = STEPS.indexOf(step);
  const membersLabel = kind === "StaticTag" ? "Devices" : "Filters";
  const labels: Record<Step, string> = { details: "Details", access: "Access", members: membersLabel, review: "Review" };

  const next = () => {
    setTouched((t) => new Set(t).add(step));
    if (stepError[step]) return;
    if (step === "review") void create();
    else setStep(STEPS[index + 1]);
  };

  const create = async () => {
    if (!org) return;
    setBusy(true);
    setError(null);
    try {
      const trimmed = name.trim();
      let tag: Tag;
      if (kind === "StaticTag") {
        if (!createdRef.current) {
          createdRef.current = await createStaticTag(org.id, trimmed, description.trim(), category.trim());
          cacheTag(createdRef.current);
        }
        tag = createdRef.current;
        if (devices.length) {
          try {
            await assignStaticTags([tag.id], devices.map((d) => d.id), tag.name);
          } catch (e) {
            throw new Error(`Created ${tag.name}, but couldn't add the devices: ${(e as Error).message}`);
          }
        }
        tag = { ...tag, assetCount: devices.length };
      } else {
        tag = await createDynamicTag(org.id, trimmed, description.trim(), ruleToFilter(rule), category.trim());
      }
      cacheTag(tag);
      if (kind === "StaticTag" && devices.length) void refreshTagMembership(tag.id);
      onCreated(tag);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <Stepper steps={STEPS.map((s) => labels[s])} current={index} onSelect={(i) => i < index && setStep(STEPS[i])} />

      <div className="min-h-0 flex-1 overflow-y-auto">
        <form
          className="mx-auto max-w-3xl px-6 py-5"
          onSubmit={(e) => {
            e.preventDefault();
            next();
          }}
        >
          {step === "details" && (
            <div className="divide-y divide-border">
              <FormRow label="Type">
                <Segmented<TagKind>
                  value={kind}
                  onChange={setKind}
                  options={[
                    { value: "StaticTag", label: <><TagKindIcon kind="StaticTag" /> Static</> },
                    { value: "DynamicTag", label: <><TagKindIcon kind="DynamicTag" /> Dynamic</> },
                  ]}
                />
              </FormRow>
              <FormRow label="Name" align="start">
                <input className={`${inputClass} ${touched.has("details") && nameError ? "border-err" : ""}`} value={name} onChange={(e) => setName(e.target.value)} autoFocus spellCheck={false} maxLength={64} />
                <p className={`mt-1 text-xs ${touched.has("details") && nameError ? "text-err" : "text-faint"}`}>
                  {touched.has("details") && nameError ? nameError : "Letters, numbers, - and _ · unique within the organization"}
                </p>
              </FormRow>
              <FormRow label="Description" align="start">
                <textarea className={`${inputClass} h-20 resize-none py-1.5`} value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Optional" />
              </FormRow>
              <FormRow label="Category">
                <input className={`${inputClass} max-w-64`} value={category} onChange={(e) => setCategory(e.target.value)} placeholder="Optional" />
              </FormRow>
            </div>
          )}

          {step === "access" && <AccessStep org={org} onChange={setOrg} error={touched.has("access") ? stepError.access : null} />}

          {step === "members" &&
            org &&
            (kind === "StaticTag" ? (
              <div className="space-y-2">
                <p className="text-sm text-muted">Add devices now, or later from the tag's page.</p>
                <DevicePickerField orgId={org.id} selected={devices} onChange={setDevices} />
                {devices.length > 0 && <DeviceTable assets={devices} />}
              </div>
            ) : (
              <div className="space-y-4">
                <RuleBuilder rule={rule} onChange={setRule} />
                {touched.has("members") && stepError.members && <p className="text-xs text-err">{stepError.members}</p>}
                <FilterPreview orgId={org.id} rule={rule} />
              </div>
            ))}

          {step === "review" && org && (
            <div className="space-y-6">
              <ReviewSection title="Details" onEdit={() => setStep("details")}>
                <ReviewRow label="Name">{name.trim()}</ReviewRow>
                <ReviewRow label="Type">
                  <span className="inline-flex items-center gap-1.5">
                    <TagKindIcon kind={kind} /> {kind === "StaticTag" ? "Static" : "Dynamic"}
                  </span>
                </ReviewRow>
                {description.trim() && <ReviewRow label="Description">{description.trim()}</ReviewRow>}
                {category.trim() && <ReviewRow label="Category">{category.trim()}</ReviewRow>}
              </ReviewSection>
              <ReviewSection title="Access" onEdit={() => setStep("access")}>
                <ReviewRow label="Usable in">
                  {org.name} <span className="text-faint">· {orgKind(org)}</span>
                </ReviewRow>
              </ReviewSection>
              <ReviewSection title={membersLabel} onEdit={() => setStep("members")}>
                {kind === "StaticTag" ? (
                  <ReviewRow label="Devices">{devices.length ? devices.map((d) => d.name).join(", ") : <span className="text-faint">None yet</span>}</ReviewRow>
                ) : (
                  <ReviewRow label="Rule">{describeRule(rule)}</ReviewRow>
                )}
              </ReviewSection>
              {error && <ErrorNote>{error}</ErrorNote>}
            </div>
          )}
          <button type="submit" hidden />
        </form>
      </div>

      <footer className="flex h-14 shrink-0 items-center justify-between gap-3 border-t border-border px-5">
        <Button variant="ghost" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
        <div className="flex gap-2">
          {index > 0 && (
            <Button onClick={() => setStep(STEPS[index - 1])} disabled={busy}>
              <ChevronLeft />
              Back
            </Button>
          )}
          <Button variant="primary" onClick={next} busy={busy}>
            {step === "review" ? "Create tag" : "Next"}
          </Button>
        </div>
      </footer>
    </div>
  );
}

/** Compact numbered steps; finished steps can be clicked to go back. */
function Stepper({ steps, current, onSelect }: { steps: string[]; current: number; onSelect: (i: number) => void }) {
  return (
    <ol className="flex h-12 shrink-0 items-center justify-center gap-2 border-b border-border px-5">
      {steps.map((label, i) => {
        const done = i < current;
        const active = i === current;
        return (
          <li key={label} className="flex items-center gap-2">
            {i > 0 && <span className={`h-px w-8 ${done || active ? "bg-accent" : "bg-border"}`} />}
            <button
              type="button"
              onClick={() => onSelect(i)}
              disabled={!done}
              className={`flex items-center gap-1.5 text-sm ${active ? "font-medium text-text" : done ? "text-muted hover:text-text" : "text-faint"}`}
            >
              <span
                className={`flex size-5 items-center justify-center rounded-full text-[11px] font-semibold ${
                  done ? "bg-accent text-on-accent" : active ? "border border-accent text-accent" : "border border-border text-faint"
                }`}
              >
                {done ? <Check className="size-3" /> : i + 1}
              </span>
              {label}
            </button>
          </li>
        );
      })}
    </ol>
  );
}

/** Where the tag can be used: one organization, customer or site (and everything under it). */
function AccessStep({ org, onChange, error }: { org: Organization | null; onChange: (o: Organization) => void; error: string | null }) {
  const { data, isPending } = useQuery(orgsQuery);
  const [term, setTerm] = useState("");
  const [category, setCategory] = useState<AccessCategory>(org?.__typename && CATEGORY_TYPES.customer.includes(org.__typename) ? "customer" : org?.__typename === "Site" ? "site" : "organization");
  const listRef = useRef<HTMLUListElement>(null);

  const results = useMemo(
    () => (data?.items ?? []).filter((o) => CATEGORY_TYPES[category].includes(o.__typename) && matches(term, o.name)).slice(0, 100),
    [data, category, term],
  );

  return (
    <div className="space-y-3">
      <p className="text-sm text-muted">The tag can be used on devices in the selected organization and everything under it.</p>
      <div className="flex items-center gap-3">
        <div className="relative flex-1">
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
          <input className={`${inputClass} pl-8`} placeholder="Search" value={term} onChange={(e) => setTerm(e.target.value)} autoFocus spellCheck={false} />
        </div>
        <Segmented<AccessCategory>
          value={category}
          onChange={setCategory}
          options={[
            { value: "organization", label: "Organization" },
            { value: "customer", label: "Customer" },
            { value: "site", label: "Site" },
          ]}
        />
      </div>
      {org && (
        <p className="text-sm">
          Selected: <span className="font-medium">{org.name}</span> <span className="text-faint">· {orgKind(org)}</span>
        </p>
      )}
      {error && <p className="text-xs text-err">{error}</p>}
      <ul ref={listRef} className="max-h-[46vh] overflow-y-auto rounded-lg border border-border" role="listbox">
        {isPending && (
          <li className="flex items-center gap-2 px-3 py-3 text-sm text-muted">
            <Loader2 className="size-4 animate-spin" /> Loading organizations
          </li>
        )}
        {!isPending && results.length === 0 && <li className="px-3 py-3 text-sm text-faint">No matches</li>}
        {results.map((o) => {
          const selected = o.id === org?.id;
          return (
            <li key={o.id} role="option" aria-selected={selected} className="border-b border-border last:border-b-0">
              <button
                type="button"
                onClick={() => onChange(o)}
                className={`flex w-full items-center gap-2.5 px-3 py-2 text-left text-sm ${selected ? "bg-accent-soft" : "hover:bg-surface-2"}`}
              >
                <span className={`flex size-4 items-center justify-center rounded-full border ${selected ? "border-accent bg-accent text-on-accent" : "border-border-strong"}`}>
                  {selected && <Check className="size-3" />}
                </span>
                <span className="truncate">{o.name}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function DeviceTable({ assets }: { assets: Asset[] }) {
  return (
    <Table
      head={
        <>
          <th>Device</th>
          <th>Location</th>
          <th>Operating system</th>
        </>
      }
    >
      {assets.map((a) => (
        <tr key={a.id}>
          <td>
            <span className="flex items-center gap-2">
              <AgentDot status={a.agentConnection?.status} />
              <span className="truncate font-medium">{a.name}</span>
            </span>
          </td>
          <td className="max-w-0 truncate text-muted">{[a.customer?.name, a.site?.name].filter(Boolean).join(" · ")}</td>
          <td className="max-w-0 truncate text-muted">{a.operatingSystem?.name}</td>
        </tr>
      ))}
    </Table>
  );
}

/** Devices the rule matches right now, as a table, refreshed as the rule changes. */
function FilterPreview({ orgId, rule }: { orgId: string; rule: Rule }) {
  const filter = useMemo(() => ruleToFilter(rule), [rule]);
  const key = JSON.stringify(filter);
  const [state, setState] = useState<{ assets: Asset[]; totalCount: number } | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const hasConditions = activeConditions(rule).length > 0;

  useEffect(() => {
    if (!hasConditions) {
      setState(null);
      return;
    }
    const id = ++requestId.current;
    setLoading(true);
    const timer = setTimeout(async () => {
      try {
        const result = await previewAssets(orgId, JSON.parse(key), 50);
        if (id === requestId.current) {
          setState(result);
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

  return (
    <section className="space-y-2">
      <div className="flex h-7 items-center justify-between">
        <h3 className="text-sm font-semibold">Matching devices</h3>
        <span className="text-xs text-muted">
          {loading ? <Loader2 className="size-3.5 animate-spin" /> : state ? `${state.totalCount} match${state.totalCount === 1 ? "" : "es"}` : ""}
        </span>
      </div>
      {error ? (
        <ErrorNote>{error}</ErrorNote>
      ) : !hasConditions ? (
        <p className="py-6 text-center text-sm text-faint">Add a condition to see which devices match</p>
      ) : state && state.assets.length > 0 ? (
        <>
          <DeviceTable assets={state.assets} />
          {state.totalCount > state.assets.length && <p className="text-xs text-faint">Showing the first {state.assets.length}</p>}
        </>
      ) : (
        state && <p className="py-6 text-center text-sm text-faint">No devices match yet</p>
      )}
    </section>
  );
}

function ReviewSection({ title, onEdit, children }: { title: string; onEdit: () => void; children: ReactNode }) {
  return (
    <section>
      <div className="mb-1 flex h-7 items-center justify-between">
        <h3 className="text-sm font-semibold">{title}</h3>
        <button type="button" onClick={onEdit} className="text-sm text-accent hover:underline">
          Edit
        </button>
      </div>
      <dl className="divide-y divide-border border-y border-border">{children}</dl>
    </section>
  );
}

function ReviewRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[112px_minmax(0,1fr)] gap-4 py-2 text-sm">
      <dt className="text-muted">{label}</dt>
      <dd className="min-w-0 break-words">{children}</dd>
    </div>
  );
}
