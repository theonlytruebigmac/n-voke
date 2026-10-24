import { useMemo, useRef, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Check, ChevronLeft, Loader2, Search } from "lucide-react";
import { assignStaticTags, createDynamicTag, createStaticTag } from "../lib/api";
import { cacheTag, orgsQuery, refreshTagMembership } from "../lib/collections";
import { humanize } from "../lib/format";
import { activeConditions, describeRule, emptyRule, ruleToFilter, validateTagName, type Rule } from "../lib/rules";
import { matches } from "../lib/sync";
import type { Asset, Organization, Tag, TagKind } from "../lib/types";
import { DevicePickerField } from "./DevicePickerField";
import { RuleBuilder } from "./RuleBuilder";
import { AgentDot, TagKindIcon, tagKindLabel } from "./status";
import { DeviceChips, MatchPreview, useRulePreview, type RulePreview } from "./tags/MatchPreview";
import { Button, ErrorNote, Field, Segmented, StepSummary, StepTitle, Table, Timeline, inputClass, type StepState, type TimelineStep } from "./ui";

/*
 * New-tag wizard, following N-central's flow:
 *   Static:  Details → Access → Devices (optional) → Review
 *   Dynamic: Details → Access → Filters → Review
 * Shown as a timeline: finished steps fold to one line with Change, the current one is open, and a panel beside it
 * previews the tag (devices it will hold or match) and carries Back / Next.
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
  const preview = useRulePreview(kind === "DynamicTag" ? (org?.id ?? null) : null, rule);

  const nameError = validateTagName(name.trim());
  const stepError: Record<Step, string | null> = {
    details: nameError,
    access: org ? null : "Choose where the tag can be used",
    members: kind === "DynamicTag" && activeConditions(rule).length === 0 ? "Add at least one condition" : null,
    review: null,
  };
  const index = STEPS.indexOf(step);
  const isStatic = kind === "StaticTag";

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

  const stateOf = (s: Step, i: number): StepState =>
    i < index ? "done" : i > index ? "upcoming" : touched.has(s) && stepError[s] ? "attention" : "current";
  /** Going back to a finished step; the steps after it keep what was entered. */
  const change = (s: Step) => {
    setError(null);
    setStep(s);
  };

  const content: Record<Step, () => ReactNode> = {
    details: () =>
      index > 0 ? (
        <StepSummary label="Details" onChange={() => change("details")}>
          <span className="inline-flex items-center gap-1.5">
            <TagKindIcon kind={kind} />
            {name.trim()}
          </span>
          <span className="ml-2 text-sm font-normal text-faint">
            {tagKindLabel(kind)}
            {description.trim() && ` · ${description.trim()}`}
            {category.trim() && ` · ${category.trim()}`}
          </span>
        </StepSummary>
      ) : (
        <div className="space-y-4">
          <StepTitle>Name the tag</StepTitle>
          <Segmented<TagKind>
            value={kind}
            onChange={setKind}
            options={[
              { value: "StaticTag", label: <><TagKindIcon kind="StaticTag" /> Static</> },
              { value: "DynamicTag", label: <><TagKindIcon kind="DynamicTag" /> Dynamic</> },
            ]}
          />
          <p className="text-xs text-faint">{isStatic ? "Static tags hold the devices you choose." : "Dynamic tags fill themselves from rules."}</p>
          <Field label="Name" hint="Letters, numbers, - and _ · unique within the organization" error={touched.has("details") ? nameError : null}>
            <input className={`${inputClass} ${touched.has("details") && nameError ? "border-err" : ""}`} value={name} onChange={(e) => setName(e.target.value)} autoFocus spellCheck={false} maxLength={64} />
          </Field>
          <Field label="Description">
            <textarea className={`${inputClass} h-20 resize-none py-1.5`} value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Optional" />
          </Field>
          <Field label="Category">
            <input className={`${inputClass} max-w-64`} value={category} onChange={(e) => setCategory(e.target.value)} placeholder="Optional" />
          </Field>
        </div>
      ),
    access: () =>
      index > 1 && org ? (
        <StepSummary label="Usable in" onChange={() => change("access")}>
          {org.name}
          <span className="ml-2 text-sm font-normal text-faint">{orgKind(org)}, and everything under it</span>
        </StepSummary>
      ) : index < 1 ? (
        <StepTitle>
          <span className="text-muted">Where it can be used</span>
        </StepTitle>
      ) : (
        <div className="space-y-3">
          <StepTitle>Where can it be used?</StepTitle>
          <AccessStep org={org} onChange={setOrg} error={touched.has("access") ? stepError.access : null} />
        </div>
      ),
    members: () =>
      index > 2 ? (
        <StepSummary label={isStatic ? "Devices" : "Rule"} onChange={() => change("members")}>
          {isStatic ? (devices.length ? `${devices.length} device${devices.length === 1 ? "" : "s"}` : <span className="font-normal text-faint">None yet</span>) : describeRule(rule)}
        </StepSummary>
      ) : index < 2 || !org ? (
        <StepTitle>
          <span className="text-muted">{isStatic ? "Devices" : "Devices that match"}</span>
        </StepTitle>
      ) : isStatic ? (
        <div className="space-y-3">
          <StepTitle hint="Optional">Add devices</StepTitle>
          <p className="text-sm text-muted">Add devices now, or later from the tag's page.</p>
          <DevicePickerField orgId={org.id} selected={devices} onChange={setDevices} />
          {devices.length > 0 && <DeviceTable assets={devices} />}
        </div>
      ) : (
        <div className="space-y-3">
          <RuleBuilder rule={rule} onChange={setRule} />
          {touched.has("members") && stepError.members && <p className="text-xs text-err">{stepError.members}</p>}
          <MatchTable preview={preview} />
        </div>
      ),
    review: () =>
      index < 3 ? (
        <StepTitle>
          <span className="text-muted">Review and create</span>
        </StepTitle>
      ) : (
        <div className="space-y-3">
          <StepTitle>Review and create</StepTitle>
          <p className="text-sm text-muted">Check the steps above, then create the tag.</p>
          {error && <ErrorNote>{error}</ErrorNote>}
        </div>
      ),
  };

  const steps: TimelineStep[] = STEPS.map((s, i) => ({ key: s, state: stateOf(s, i), content: content[s]() }));

  return (
    <div className="h-full min-h-0 overflow-y-auto">
      <form
        className="flex flex-wrap items-start gap-5 p-5"
        onSubmit={(e) => {
          e.preventDefault();
          next();
        }}
      >
        <div className="min-w-0 flex-[3_1_420px]">
          <Timeline label="New tag steps" steps={steps} />
        </div>

        <aside aria-label="Preview" className="flex min-w-0 flex-[2_1_260px] flex-col gap-4 rounded-2xl border border-border bg-surface p-5">
          {isStatic ? (
            <div className="space-y-3">
              <div className="flex flex-wrap items-baseline gap-2">
                <span className="text-3xl font-bold tabular-nums">{devices.length.toLocaleString()}</span>
                <span className="text-muted">{devices.length === 1 ? "device to add" : "devices to add"}</span>
              </div>
              {devices.length > 0 ? <DeviceChips assets={devices} total={devices.length} /> : <p className="text-sm text-faint">A static tag can start empty.</p>}
            </div>
          ) : (
            <>
              <MatchPreview preview={preview} emptyHint={org ? "Add a condition to see which devices match" : "Choose where the tag can be used, then add conditions."} />
              <p className="text-xs text-faint">Membership updates by itself and can lag a minute after a device changes.</p>
            </>
          )}
          <div className="h-px bg-border" />
          {error && step === "review" && <ErrorNote>{error}</ErrorNote>}
          <div className="flex gap-2">
            {index > 0 && (
              <Button className="h-10 flex-1" onClick={() => change(STEPS[index - 1])} disabled={busy}>
                <ChevronLeft />
                Back
              </Button>
            )}
            <Button type="submit" variant="primary" className="h-10 flex-2 text-sm" busy={busy}>
              {step === "review" ? "Create tag" : step === "members" ? "Next: review" : "Next"}
            </Button>
          </div>
          <Button onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
        </aside>
      </form>
    </div>
  );
}

/** Where the tag can be used: one organization, customer or site (and everything under it). */
function AccessStep({ org, onChange, error }: { org: Organization | null; onChange: (o: Organization) => void; error: string | null }) {
  const { data, isPending } = useQuery(orgsQuery);
  const [term, setTerm] = useState("");
  const [category, setCategory] = useState<AccessCategory>(org?.__typename && CATEGORY_TYPES.customer.includes(org.__typename) ? "customer" : org?.__typename === "Site" ? "site" : "organization");

  const results = useMemo(
    () => (data?.items ?? []).filter((o) => CATEGORY_TYPES[category].includes(o.__typename) && matches(term, o.name)).slice(0, 100),
    [data, category, term],
  );

  return (
    <div className="space-y-3">
      <p className="text-sm text-muted">The tag can be used on devices in the selected organization and everything under it.</p>
      <div className="flex flex-wrap items-center gap-3">
        <div className="relative min-w-48 flex-1">
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
      <ul className="max-h-[40vh] overflow-y-auto rounded-xl border border-border bg-bg" role="listbox">
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
                title={o.name}
                className={`flex w-full items-center gap-2.5 px-3 py-2 text-left text-sm ${selected ? "bg-accent-soft" : "hover:bg-surface-2"}`}
              >
                <span className={`flex size-4 shrink-0 items-center justify-center rounded-full border ${selected ? "border-accent-fill bg-accent-fill text-on-accent" : "border-border-strong"}`}>
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
            <span className="flex items-center gap-2 whitespace-nowrap">
              <AgentDot status={a.agentConnection?.status} />
              <span className="truncate font-medium" title={a.name}>
                {a.name}
              </span>
            </span>
          </td>
          <td className="max-w-0 truncate text-muted">{[a.customer?.name, a.site?.name].filter(Boolean).join(" · ")}</td>
          <td className="max-w-0 truncate text-muted">{a.operatingSystem?.name}</td>
        </tr>
      ))}
    </Table>
  );
}

/** The devices a rule matches, as a table on request (the first 50). */
function MatchTable({ preview }: { preview: RulePreview }) {
  const [open, setOpen] = useState(false);
  const { result } = preview;
  if (!result || result.assets.length === 0) return null;
  return (
    <div className="space-y-2">
      <Button size="sm" variant="ghost" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        {open ? "Hide matching devices" : "Show matching devices"}
      </Button>
      {open && (
        <>
          <DeviceTable assets={result.assets} />
          {result.totalCount > result.assets.length && <p className="text-xs text-faint">Showing the first {result.assets.length}</p>}
        </>
      )}
    </div>
  );
}
