import { useEffect, useState } from "react";
import { useMigrationSettings, useSaveMigrationSettings } from "../../lib/migrate/queries";
import { parseSettings, sameSettings, toSettingsForm, TRACKING, type SettingsForm, type TrackingKey } from "../../lib/migrate/settings";
import { Button, ErrorNote, Field, inputClass } from "../ui";
import { OptionsFields } from "./parts/OptionsFields";

const TRACKING_GROUPS: { title: string; keys: TrackingKey[] }[] = [
  { title: "How often", keys: ["pollSeconds", "autoProbeMinutes"] },
  { title: "When a wait becomes a problem", keys: ["notPickedUpMinutes", "stuckStagedMinutes", "lostMinutes", "ecoverseNotMovedMinutes"] },
  { title: "What counts as checking in", keys: ["healthyCheckinMinutes", "agentQuietMinutes"] },
];

/**
 * The migration settings that belong to this machine: how it watches devices, and what a new project starts with.
 * They are saved together, so they are one form with one Save, shown in Settings.
 */
export function MigrationSettingsPanel() {
  const saved = useMigrationSettings();
  const save = useSaveMigrationSettings();
  const [form, setForm] = useState<SettingsForm | null>(null);
  useEffect(() => {
    if (saved.data) setForm(toSettingsForm(saved.data));
  }, [saved.data]);

  const parsed = form && saved.data ? parseSettings(form, saved.data) : null;
  const errors = parsed && !parsed.ok ? parsed.errors : {};
  const optionErrors = parsed && !parsed.ok ? parsed.optionErrors : {};
  const changed = !!saved.data && (!parsed?.ok || !sameSettings(parsed.settings, saved.data));

  if (saved.isError) return <ErrorNote>{String(saved.error)}</ErrorNote>;
  if (!form) return <p className="text-sm text-muted">Loading…</p>;

  return (
    <div className="space-y-6">
      <section className="space-y-4">
        <h3 className="text-sm font-semibold">Tracking</h3>
        {TRACKING_GROUPS.map((group) => (
          <div key={group.title} className="space-y-3">
            <p className="text-xs font-medium text-muted">{group.title}</p>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-4">
              {group.keys.map((key) => (
                <Field key={key} label={`${TRACKING[key].label} (${TRACKING[key].unit})`} error={errors[key]} hint={TRACKING[key].hint}>
                  <input
                    className={`${inputClass} w-24`}
                    inputMode="numeric"
                    value={form.tracking[key]}
                    onChange={(e) => setForm({ ...form, tracking: { ...form.tracking, [key]: e.target.value } })}
                    aria-invalid={!!errors[key]}
                  />
                </Field>
              ))}
            </div>
          </div>
        ))}
      </section>

      <section className="space-y-4 border-t border-border pt-5">
        <div>
          <h3 className="text-sm font-semibold">New projects start with</h3>
          <p className="text-xs text-muted">Each project keeps its own options after it is made; changing these doesn't touch existing projects.</p>
        </div>
        <OptionsFields form={form.defaults} setForm={(defaults) => setForm({ ...form, defaults })} errors={optionErrors} />
      </section>

      {save.isError && <ErrorNote>{String(save.error)}</ErrorNote>}
      <div className="flex items-center justify-end gap-3 border-t border-border pt-4">
        {save.isSuccess && !changed && <span className="text-xs text-ok">Saved</span>}
        <Button variant="primary" busy={save.isPending} disabled={!parsed?.ok || !changed} onClick={() => parsed?.ok && save.mutate(parsed.settings)}>
          Save migration settings
        </Button>
      </div>
    </div>
  );
}
