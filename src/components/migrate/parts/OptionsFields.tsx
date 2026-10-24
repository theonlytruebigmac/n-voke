import type { OptionsForm } from "../../../lib/migrate/options";
import { Field, inputClass } from "../../ui";

type Errors = Partial<Record<"reviewDays" | "waveSize" | "taskRepeatMinutes" | "taskExpiryDays", string>>;

/** The migration options, shared by a project's Prepare step and the defaults new projects start with. */
export function OptionsFields({ form, setForm, errors, deliveryLocked = false }: { form: OptionsForm; setForm: (form: OptionsForm) => void; errors: Errors; deliveryLocked?: boolean }) {
  const number = (key: keyof Errors) => (
    <input className={`${inputClass} w-24`} inputMode="numeric" value={form[key]} onChange={(e) => setForm({ ...form, [key]: e.target.value })} aria-invalid={!!errors[key]} />
  );
  return (
    <>
      <fieldset className="space-y-2" disabled={deliveryLocked}>
        <legend className="text-xs font-medium text-muted">How devices are reached</legend>
        {(
          [
            ["ecoverseFirst", "Ecoverse first", "Through Ecoverse where a device has an Ecoverse asset, otherwise through N-central."],
            ["restOnly", "N-central only", "Every device through N-central, without using Ecoverse."],
          ] as const
        ).map(([value, title, text]) => (
          <label key={value} className="flex cursor-pointer items-start gap-2.5 rounded-xl border border-border px-3.5 py-2.5 has-[:checked]:border-accent has-[:checked]:bg-accent-soft has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-60">
            <input type="radio" name="delivery" className="mt-1" checked={form.delivery === value} onChange={() => setForm({ ...form, delivery: value })} />
            <span className="text-sm">
              <span className="font-medium">{title}</span>
              <span className="block text-xs text-muted">{text}</span>
            </span>
          </label>
        ))}
        {deliveryLocked && <p className="text-xs text-muted">A wave has launched, so this can't change. Choose a path for one device in Plan instead.</p>}
      </fieldset>

      <fieldset className="space-y-2">
        <legend className="text-xs font-medium text-muted">What happens to the old agent</legend>
        {(
          [
            ["full", "Full cleanup", "Removes every N-central component, then installs. The default."],
            ["none", "Install over the top", "Installs without removing first. A newer destination agent can leave the device in the source's Ecoverse tenant."],
          ] as const
        ).map(([value, title, text]) => (
          <label key={value} className="flex cursor-pointer items-start gap-2.5 rounded-xl border border-border px-3.5 py-2.5 has-[:checked]:border-accent has-[:checked]:bg-accent-soft">
            <input type="radio" name="clean" className="mt-1" checked={form.cleanMode === value} onChange={() => setForm({ ...form, cleanMode: value })} />
            <span className="text-sm">
              <span className="font-medium">{title}</span>
              <span className="block text-xs text-muted">{text}</span>
            </span>
          </label>
        ))}
      </fieldset>

      <div className="grid grid-cols-2 gap-4">
        <Field label="Review period (days)" error={errors.reviewDays} hint="How long each device keeps its migration record before it removes itself.">
          {number("reviewDays")}
        </Field>
        <Field label="Wave size (devices)" error={errors.waveSize} hint="How many devices go in one wave.">
          {number("waveSize")}
        </Field>
        <Field label="Retry every (minutes)" error={errors.taskRepeatMinutes} hint="How often a device tries again until the migration works.">
          {number("taskRepeatMinutes")}
        </Field>
        <Field label="Keep retrying for (days)" error={errors.taskExpiryDays} hint="After this the device stops trying.">
          {number("taskExpiryDays")}
        </Field>
      </div>

      <label className="flex cursor-pointer items-start gap-2.5 text-sm">
        <input type="checkbox" className="mt-1" checked={form.retireSourceRecords} onChange={(e) => setForm({ ...form, retireSourceRecords: e.target.checked })} />
        <span>
          After the review period, retire the old source records
          <span className="block text-xs text-muted">The finish step asks again before anything is deleted.</span>
        </span>
      </label>
    </>
  );
}
