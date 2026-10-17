import { useEffect, useMemo, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { previewAssets, updateDynamicTag, updateStaticTag } from "../lib/api";
import { cacheTag } from "../lib/collections";
import { activeConditions, emptyRule, filterToRule, ruleToFilter, validateTagName, type Rule } from "../lib/rules";
import type { Asset, Tag } from "../lib/types";
import { RuleBuilder } from "./RuleBuilder";
import { AgentDot } from "./status";
import { Button, ErrorNote, Field, Modal, inputClass } from "./ui";

/** Edit an existing tag's name, description and (for dynamic tags) rule. New tags use TagWizard. */
export function TagEditorDialog({ org, tag, onClose, onSaved }: { org: { id: string; name: string }; tag: Tag; onClose: () => void; onSaved: (tag: Tag) => void }) {
  const isDynamic = tag.__typename === "DynamicTag";
  const [name, setName] = useState(tag.name);
  const [description, setDescription] = useState(tag.description ?? "");
  const storedRule = useMemo(() => (tag.filter ? filterToRule(tag.filter) : null), [tag]);
  const ruleEditable = !isDynamic || !!storedRule;
  const [rule, setRule] = useState<Rule>(storedRule ?? emptyRule());
  const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const nameError = validateTagName(name.trim());
  const ruleError = isDynamic && ruleEditable && activeConditions(rule).length === 0 ? "Add at least one condition" : null;

  const save = async () => {
    setTouched(true);
    if (nameError || ruleError) return;
    setBusy(true);
    setError(null);
    try {
      const saved = isDynamic
        ? await updateDynamicTag(tag.id, name.trim(), description, ruleEditable ? ruleToFilter(rule) : JSON.parse(tag.filter!))
        : await updateStaticTag(tag.id, name.trim(), description);
      cacheTag(saved);
      onSaved(saved);
      onClose();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={`Edit ${tag.name}`}
      description={<>In {org.name}</>}
      onClose={onClose}
      size={isDynamic ? "lg" : "md"}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy} onClick={() => void save()}>
            Save
          </Button>
        </>
      }
    >
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <div className="grid grid-cols-2 gap-3">
          <Field label="Name" error={touched ? nameError : null}>
            <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} autoFocus spellCheck={false} maxLength={64} />
          </Field>
          <Field label="Description">
            <input className={inputClass} value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Optional" />
          </Field>
        </div>

        {isDynamic &&
          (ruleEditable ? (
            <div className="space-y-3">
              <RuleBuilder rule={rule} onChange={setRule} />
              {touched && ruleError && <p className="text-xs text-err">{ruleError}</p>}
              <RulePreview orgId={org.id} rule={rule} />
            </div>
          ) : (
            <Field label="Rule (kept as is — uses conditions the editor doesn't support)">
              <pre className="overflow-auto rounded-md bg-surface-2 p-2 font-mono text-xs">{tag.filter}</pre>
            </Field>
          ))}

        {error && <ErrorNote>{error}</ErrorNote>}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}

/** Devices the rule matches right now, via the same filter on assetSearch. */
function RulePreview({ orgId, rule }: { orgId: string; rule: Rule }) {
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
        const result = await previewAssets(orgId, JSON.parse(key));
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
    <div className="rounded-lg bg-surface-2 px-3 py-2.5">
      <div className="flex items-center gap-2 text-xs text-muted">
        Matches now
        {loading ? <Loader2 className="size-3.5 animate-spin" /> : state && <span className="font-medium text-text tabular-nums">{state.totalCount}</span>}
      </div>
      {error ? (
        <p className="mt-1 text-xs text-err">{error}</p>
      ) : state && state.assets.length > 0 ? (
        <ul className="mt-2 flex flex-wrap gap-x-3 gap-y-1">
          {state.assets.map((a) => (
            <li key={a.id} className="inline-flex items-center gap-1.5 text-sm">
              <AgentDot status={a.agentConnection?.status} />
              {a.name}
            </li>
          ))}
          {state.totalCount > state.assets.length && <li className="text-sm text-faint">+{state.totalCount - state.assets.length} more</li>}
        </ul>
      ) : (
        hasConditions && state && <p className="mt-1 text-xs text-faint">No devices match yet</p>
      )}
    </div>
  );
}
