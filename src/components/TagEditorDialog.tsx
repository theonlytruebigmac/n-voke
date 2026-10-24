import { useMemo, useState } from "react";
import { updateDynamicTag, updateStaticTag } from "../lib/api";
import { cacheTag } from "../lib/collections";
import { activeConditions, emptyRule, filterToRule, ruleToFilter, validateTagName, type Rule } from "../lib/rules";
import type { Tag } from "../lib/types";
import { RuleBuilder } from "./RuleBuilder";
import { MatchPreview, useRulePreview } from "./tags/MatchPreview";
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
  const preview = useRulePreview(isDynamic && ruleEditable ? org.id : null, rule);

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
        className="space-y-5"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Name" error={touched ? nameError : null}>
            <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} autoFocus spellCheck={false} maxLength={64} />
          </Field>
          <Field label="Description">
            <input className={inputClass} value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Optional" />
          </Field>
        </div>

        {isDynamic &&
          (ruleEditable ? (
            <div className="space-y-4">
              <RuleBuilder rule={rule} onChange={setRule} />
              {touched && ruleError && <p className="text-xs text-err">{ruleError}</p>}
              <div className="rounded-xl border border-border bg-bg p-4">
                <MatchPreview preview={preview} />
              </div>
            </div>
          ) : (
            <Field label="Rule (kept as is — uses conditions the editor doesn't support)">
              <pre className="overflow-auto rounded-lg border border-border bg-sidebar p-3 font-mono text-xs">{tag.filter}</pre>
            </Field>
          ))}

        {error && <ErrorNote>{error}</ErrorNote>}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}
