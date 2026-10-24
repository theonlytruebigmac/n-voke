import { useCallback, useEffect, useState } from "react";
import { KeyRound, Play } from "lucide-react";
import { runScript } from "../lib/api";
import { searchOrganizationsLocal, searchScriptsLocal } from "../lib/collections";
import { formatDuration, humanize, languageName } from "../lib/format";
import { toInputValue, validateInput } from "../lib/inputs";
import { jobStore, newTask } from "../lib/jobs";
import { targetLabel, type RunDraft, type Target } from "../lib/targets";
import type { Asset, Organization, Script } from "../lib/types";
import { Combobox } from "./Combobox";
import { InputsForm, type InputValues } from "./InputsForm";
import { TargetDevices, type DeviceSummary } from "./TargetDevices";
import { TargetField } from "./TargetField";
import { Button, EmptyState, Field, PageBody, PageHeader, StepSummary, StepTitle, Timeline, inputClass, type StepState, type TimelineStep } from "./ui";

const orgKind = (o: Organization) => (o.__typename ? humanize(o.__typename.replace(/([a-z])([A-Z])/g, "$1_$2")) : "");

const NO_DEVICES: DeviceSummary = { loading: false, total: 0, included: 0, flagged: 0 };

export function RunPage({
  ready,
  draft,
  onStarted,
  onOpenSettings,
}: {
  ready: boolean;
  draft: RunDraft | null;
  onStarted: (jobId: string) => void;
  onOpenSettings: () => void;
}) {
  const [org, setOrg] = useState<Organization | null>(null);
  const [script, setScript] = useState<Script | null>(null);
  const [targets, setTargets] = useState<Target[]>([]);
  const [devices, setDevices] = useState<Asset[]>([]);
  const [summary, setSummary] = useState<DeviceSummary>(NO_DEVICES);
  const [values, setValues] = useState<InputValues>({});
  const [showErrors, setShowErrors] = useState(false);
  /** A finished step reopened with Change. */
  const [editing, setEditing] = useState<"org" | "script" | null>(null);
  const [editOptions, setEditOptions] = useState(false);
  const [taskName, setTaskName] = useState("");
  const [timeout, setTimeoutValue] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Apply a pre-filled draft (from Activity or Tags).
  useEffect(() => {
    if (!draft) return;
    setOrg(draft.org);
    setScript(draft.script ?? null);
    setTargets(draft.targets ?? []);
    setValues({});
    setShowErrors(false);
    setEditing(null);
    setConfirming(false);
    setError(null);
  }, [draft]);

  // Any change to what would run cancels a pending confirmation.
  useEffect(() => setConfirming(false), [org, script, devices, values]);

  const inputs = script?.inputs?.filter(Boolean) ?? [];
  const timeoutError = timeout && !(/^\d+$/.test(timeout) && Number(timeout) <= 3600) ? "0–3600" : null;
  const effectiveTimeout = timeout ? Number(timeout) : (script?.timeoutSeconds ?? 0);
  const hasInputErrors = inputs.some((d) => validateInput(d.type, values[d.variable]) !== null);

  const searchOrgs = useCallback((t: string) => searchOrganizationsLocal(t), []);
  const searchScriptsInOrg = useCallback((t: string) => (org ? searchScriptsLocal(org.id, t) : Promise.resolve([])), [org]);
  const onDevices = useCallback((d: Asset[]) => setDevices(d), []);

  const startOver = () => {
    setOrg(null);
    setScript(null);
    setTargets([]);
    setSummary(NO_DEVICES);
    setValues({});
    setShowErrors(false);
    setEditing(null);
    setEditOptions(false);
    setTaskName("");
    setTimeoutValue("");
    setError(null);
  };

  const start = async () => {
    if (!org || !script || !devices.length) return;
    setShowErrors(true);
    if (hasInputErrors || timeoutError) return;
    if (!confirming) {
      setConfirming(true);
      return;
    }
    setRunning(true);
    setError(null);
    try {
      const name = (taskName.trim() || `N-voke - ${new Date().toLocaleString()}`).slice(0, 128);
      const runInputs = inputs
        .filter((d) => values[d.variable] !== undefined && values[d.variable] !== "")
        .map((d) => ({ name: d.name, variable: d.variable, type: d.type, value: toInputValue(d.type, values[d.variable]) }));
      const { tasks, warnings } = await runScript({
        scriptId: script.id,
        scriptName: script.name,
        targetLabel: targetLabel(targets),
        assetIds: devices.map((d) => d.id),
        name,
        timeoutSeconds: effectiveTimeout,
        inputs: runInputs,
      });
      const names = new Map(devices.map((d) => [d.id, d.name]));
      const jobId = crypto.randomUUID();
      jobStore.add({
        id: jobId,
        name,
        scriptId: script.id,
        script,
        scriptName: script.name,
        orgId: org.id,
        orgName: org.name,
        targetLabel: targetLabel(targets),
        createdAt: new Date().toISOString(),
        timeoutSeconds: effectiveTimeout,
        warnings,
        tasks: tasks.map((t) => newTask(t.taskId, t.assetId, (t.assetId && names.get(t.assetId)) || "Unknown device")),
      });
      setConfirming(false);
      onStarted(jobId);
    } catch (e) {
      setError((e as Error).message);
      setConfirming(false);
    } finally {
      setRunning(false);
    }
  };

  if (!ready) {
    return (
      <EmptyState
        icon={<KeyRound />}
        title="Connect to Ecoverse"
        action={
          <Button variant="primary" onClick={onOpenSettings}>
            Add Ecoverse token
          </Button>
        }
      >
        Scripts run through Ecoverse. Add its API token to start.
      </EmptyState>
    );
  }

  const deviceWord = devices.length === 1 ? devices[0].name : `${devices.length.toLocaleString()} devices`;

  const orgState: StepState = org && editing !== "org" ? "done" : "current";
  const scriptState: StepState = !org ? "upcoming" : script && editing !== "script" ? "done" : "current";
  const targetsState: StepState = !org ? "upcoming" : targets.length ? "done" : script ? "current" : "upcoming";
  const inputsState: StepState = !targets.length ? "upcoming" : showErrors && hasInputErrors ? "attention" : "done";
  const checkState: StepState = !targets.length ? "upcoming" : summary.loading ? "current" : summary.flagged > 0 ? "attention" : "done";

  const steps: TimelineStep[] = [
    {
      key: "org",
      state: orgState,
      content:
        orgState === "done" && org ? (
          <StepSummary label="Organization" onChange={() => setEditing("org")}>
            {org.name} <span className="ml-1 text-xs font-normal text-faint">{orgKind(org)}</span>
          </StepSummary>
        ) : (
          <div className="space-y-3">
            <StepTitle>Which organization?</StepTitle>
            <Combobox<Organization>
              value={org}
              onChange={(o) => {
                setEditing(null);
                if (o.id === org?.id) return;
                setOrg(o);
                setScript(null);
                setTargets([]);
              }}
              defaultOpen={editing === "org"}
              search={searchOrgs}
              debounceMs={0}
              getKey={(o) => o.id}
              placeholder="Choose an organization"
              searchPlaceholder="Search organizations"
              renderValue={(o) => o.name}
              renderItem={(o) => (
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate">{o.name}</span>
                  <span className="shrink-0 text-xs text-faint">{orgKind(o)}</span>
                </div>
              )}
            />
          </div>
        ),
    },
    {
      key: "script",
      state: scriptState,
      content:
        scriptState === "done" && script ? (
          <StepSummary label="Script" onChange={() => setEditing("script")}>
            {script.name}
            <span className="ml-2 text-sm font-normal text-faint">
              {languageName(script.language)}
              {inputs.length > 0 && ` · ${inputs.length} input${inputs.length === 1 ? "" : "s"}`}
            </span>
          </StepSummary>
        ) : !org ? (
          <StepTitle hint="Choose an organization first">
            <span className="text-muted">Script</span>
          </StepTitle>
        ) : (
          <div className="space-y-3">
            <StepTitle>Which script?</StepTitle>
            <Combobox<Script>
              key={org.id}
              value={script}
              onChange={(s) => {
                setEditing(null);
                setScript(s);
                setValues({});
                setShowErrors(false);
              }}
              defaultOpen={(!script && !draft?.script) || editing === "script"}
              search={searchScriptsInOrg}
              debounceMs={0}
              scope={org.id}
              getKey={(s) => s.id}
              placeholder="Choose a script"
              searchPlaceholder="Search scripts by name or description"
              renderValue={(s) => (
                <span className="flex items-center gap-2">
                  <span className="truncate">{s.name}</span>
                  <span className="text-xs text-faint">{languageName(s.language)}</span>
                </span>
              )}
              renderItem={(s) => (
                <div className="min-w-0">
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate">{s.name}</span>
                    <span className="shrink-0 text-xs text-faint">{languageName(s.language)}</span>
                  </div>
                  {s.description && s.description !== s.name && <div className="truncate text-xs text-muted">{s.description}</div>}
                </div>
              )}
            />
          </div>
        ),
    },
    {
      key: "targets",
      state: targetsState,
      content: org ? (
        <div className="space-y-3">
          <StepTitle hint="Devices, tags, or both">Who should it run on?</StepTitle>
          <TargetField key={org.id} org={org} targets={targets} onChange={setTargets} autoOpen={!!script && targets.length === 0} />
        </div>
      ) : (
        <StepTitle>
          <span className="text-muted">Who it runs on</span>
        </StepTitle>
      ),
    },
    ...(inputs.length > 0
      ? [
          {
            key: "inputs",
            state: inputsState,
            content: (
              <div className="space-y-3">
                <StepTitle hint="Leave blank to use the script's default">Script inputs</StepTitle>
                <InputsForm inputs={inputs} values={values} onChange={setValues} showErrors={showErrors} />
              </div>
            ),
          },
        ]
      : []),
    {
      key: "check",
      state: checkState,
      content: (
        <div className="space-y-3">
          <StepTitle
            hint={
              !targets.length
                ? undefined
                : summary.loading
                  ? "Reading the devices…"
                  : summary.flagged > 0
                    ? `${summary.flagged.toLocaleString()} need a look`
                    : `${summary.total.toLocaleString()} devices`
            }
          >
            {targets.length ? "Check the devices" : <span className="text-muted">Check the devices</span>}
          </StepTitle>
          {org && <TargetDevices orgId={org.id} script={script} targets={targets} onChange={onDevices} onSummary={setSummary} />}
        </div>
      ),
    },
  ];

  const leftOut = summary.total - summary.included;

  return (
    <>
      <PageHeader
        title="Run a script"
        actions={
          (org || targets.length > 0) && (
            <Button variant="secondary" onClick={startOver} disabled={running}>
              Start over
            </Button>
          )
        }
      />
      <PageBody>
        <div className="flex flex-wrap items-start gap-6">
          <div className="min-w-0 flex-[3_1_480px]">
            <Timeline label="Steps" steps={steps} />
          </div>

          <aside aria-label="Summary" className="sticky top-0 flex flex-[2_1_280px] flex-col gap-4 rounded-2xl border border-border bg-surface p-5">
            {summary.total > 0 ? (
              <>
                <div className="flex flex-wrap items-baseline gap-2">
                  <span className="text-3xl font-bold tabular-nums">{summary.included.toLocaleString()}</span>
                  <span className="text-muted">{summary.included === 1 ? "device" : "devices"}{org ? ` in ${org.name}` : ""}</span>
                </div>
                <ul className="space-y-2">
                  <SummaryLine tone="bg-ok" label="Ready" count={summary.included - summary.flagged} />
                  {summary.flagged > 0 && <SummaryLine tone="bg-warn" label="Need a look" hint="optional" count={summary.flagged} />}
                  {leftOut > 0 && <SummaryLine tone="bg-border-strong" label="Left out" count={leftOut} />}
                </ul>
              </>
            ) : (
              <p className="text-muted">{summary.loading ? "Reading the devices…" : "Choose a script and who to run it on."}</p>
            )}

            {script && (
              <>
                <div className="h-px bg-border" />
                {editOptions ? (
                  <div className="space-y-3">
                    <Field label="Task name">
                      <input className={inputClass} value={taskName} maxLength={128} onChange={(e) => setTaskName(e.target.value)} placeholder="Named automatically" />
                    </Field>
                    <Field label="Timeout, seconds" hint="Up to 3600. 0 means the maximum." error={timeoutError ? "Use a whole number from 0 to 3600." : null}>
                      <input
                        className={`${inputClass} ${timeoutError ? "border-err" : ""}`}
                        inputMode="numeric"
                        placeholder={`${script.timeoutSeconds ?? 3600}`}
                        value={timeout}
                        onChange={(e) => setTimeoutValue(e.target.value)}
                      />
                    </Field>
                  </div>
                ) : (
                  <div className="flex items-center justify-between gap-3 text-xs text-faint">
                    <span>
                      Timeout {formatDuration(effectiveTimeout || 3600)} · {taskName || "named automatically"}
                    </span>
                    <button type="button" onClick={() => setEditOptions(true)} className="font-medium text-accent hover:text-accent-hover">
                      Edit
                    </button>
                  </div>
                )}
              </>
            )}

            {error && (
              <p className="text-sm text-err" role="alert">
                {error}
              </p>
            )}

            {confirming && script ? (
              <div className="space-y-3 rounded-xl border border-accent/40 bg-accent-soft p-3">
                <p>
                  Run <span className="font-semibold">{script.name}</span> on <span className="font-semibold">{deviceWord}</span>?
                </p>
                <div className="flex gap-2">
                  <Button variant="secondary" className="flex-1" onClick={() => setConfirming(false)} disabled={running}>
                    Cancel
                  </Button>
                  <Button variant="primary" className="flex-2" onClick={start} busy={running}>
                    <Play />
                    Confirm and run
                  </Button>
                </div>
              </div>
            ) : (
              <>
                <Button variant="primary" className="h-10 w-full text-sm" onClick={start} disabled={!script || !devices.length}>
                  <Play />
                  {script && devices.length ? `Run on ${deviceWord}` : "Run"}
                </Button>
                <p className="text-center text-xs text-faint">{!script ? "Pick a script to run first." : !devices.length ? "Pick the devices to run it on first." : "You'll confirm before it starts."}</p>
              </>
            )}
          </aside>
        </div>
      </PageBody>
    </>
  );
}

function SummaryLine({ tone, label, hint, count }: { tone: string; label: string; hint?: string; count: number }) {
  return (
    <li className="flex items-center gap-2.5">
      <span className={`size-2.5 rounded-sm ${tone}`} />
      <span className="flex-1">
        {label}
        {hint && <span className="text-faint"> · {hint}</span>}
      </span>
      <span className="font-semibold tabular-nums">{count.toLocaleString()}</span>
    </li>
  );
}
