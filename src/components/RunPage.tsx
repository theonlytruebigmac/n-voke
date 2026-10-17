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
import { TargetDevices } from "./TargetDevices";
import { TargetField } from "./TargetField";
import { Button, EmptyState, FormRow, PageHeader, inputClass } from "./ui";

const orgKind = (o: Organization) => (o.__typename ? humanize(o.__typename.replace(/([a-z])([A-Z])/g, "$1_$2")) : "");

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
  const [values, setValues] = useState<InputValues>({});
  const [showErrors, setShowErrors] = useState(false);
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
      const name = (taskName.trim() || `Script Runner - ${new Date().toLocaleString()}`).slice(0, 128);
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
      <>
        <PageHeader title="Run a script" />
        <EmptyState
          icon={<KeyRound />}
          title="Connect to N-able"
          action={
            <Button variant="primary" onClick={onOpenSettings}>
              Add API key
            </Button>
          }
        >
          Add your API key to start running scripts.
        </EmptyState>
      </>
    );
  }

  const deviceWord = devices.length === 1 ? devices[0].name : `${devices.length} devices`;

  return (
    <>
      <PageHeader title="Run a script" />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl space-y-6 px-6 py-5">
          <div className="divide-y divide-border">
            <FormRow label="Organization">
              <Combobox<Organization>
                value={org}
                onChange={(o) => {
                  if (o.id === org?.id) return;
                  setOrg(o);
                  setScript(null);
                  setTargets([]);
                }}
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
            </FormRow>

            <FormRow label="Script">
              <Combobox<Script>
                key={org?.id ?? "none"}
                value={script}
                onChange={(s) => {
                  setScript(s);
                  setValues({});
                  setShowErrors(false);
                }}
                disabled={!org}
                defaultOpen={!!org && !script && !draft?.script}
                search={searchScriptsInOrg}
                debounceMs={0}
                scope={org?.id}
                getKey={(s) => s.id}
                placeholder={org ? "Choose a script" : "Choose an organization first"}
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
            </FormRow>

            <FormRow label="Run on">
              {org ? (
                <TargetField key={org.id} org={org} targets={targets} onChange={setTargets} autoOpen={!!script && targets.length === 0} />
              ) : (
                <div className="flex h-8 items-center text-sm text-faint">Choose an organization first</div>
              )}
            </FormRow>

            {inputs.length > 0 && (
              <FormRow label="Inputs" align="start">
                <InputsForm inputs={inputs} values={values} onChange={setValues} showErrors={showErrors} />
              </FormRow>
            )}

            {script && (
              <FormRow label="Options" align={editOptions ? "start" : "center"}>
                {editOptions ? (
                  <div className="grid grid-cols-[1fr_140px] gap-3">
                    <input className={inputClass} value={taskName} maxLength={128} onChange={(e) => setTaskName(e.target.value)} placeholder="Task name (optional)" />
                    <input
                      className={`${inputClass} ${timeoutError ? "border-err" : ""}`}
                      inputMode="numeric"
                      placeholder={`Timeout ${script.timeoutSeconds ?? 3600}s`}
                      value={timeout}
                      onChange={(e) => setTimeoutValue(e.target.value)}
                      aria-label="Timeout in seconds"
                      title="Timeout in seconds (0–3600, 0 = maximum)"
                    />
                  </div>
                ) : (
                  <button type="button" onClick={() => setEditOptions(true)} className="flex h-8 items-center gap-2 text-sm text-muted hover:text-text">
                    Timeout {formatDuration(effectiveTimeout || 3600)}
                    <span className="text-faint">·</span>
                    {taskName || "auto-named"}
                    <span className="text-accent">Edit</span>
                  </button>
                )}
              </FormRow>
            )}
          </div>

          {org && <TargetDevices orgId={org.id} script={script} targets={targets} onChange={onDevices} />}
        </div>
      </div>

      <footer className="flex h-14 shrink-0 items-center justify-between gap-4 border-t border-border px-5">
        <div className="min-w-0 truncate text-sm text-muted">
          {error ? (
            <span className="text-err" role="alert">
              {error}
            </span>
          ) : script && devices.length ? (
            <>
              <span className="text-text">{script.name}</span> on {deviceWord}
            </>
          ) : (
            "Choose a script and who to run it on"
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {confirming && (
            <Button variant="ghost" onClick={() => setConfirming(false)} disabled={running}>
              Cancel
            </Button>
          )}
          <Button variant="primary" onClick={start} busy={running} disabled={!script || !devices.length}>
            <Play />
            {confirming ? `Confirm: run on ${deviceWord}` : "Run"}
          </Button>
        </div>
      </footer>
    </>
  );
}
