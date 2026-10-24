import { useEffect, useRef, useState, type ReactNode } from "react";
import { KeyRound, Trash2 } from "lucide-react";
import { clearApiKey, saveSettings, type Settings } from "../../lib/api";
import { clearFinishedJobs } from "../../lib/jobs";
import { queryClient } from "../../lib/queryClient";
import { clearLocalData } from "../../lib/secureStore";
import { MigrationSettingsPanel } from "../migrate/MigrationSettingsPanel";
import { ServersPanel } from "../migrate/ServersSheet";
import { Button, Card, Dot, ErrorNote, Field, PageBody, PageHeader, inputClass, selectClass } from "../ui";

export type SettingsSection = "nable" | "data" | "servers" | "migration";

const SECTIONS: { group: string; items: { id: SettingsSection; label: string }[] }[] = [
  {
    group: "General",
    items: [
      { id: "nable", label: "Ecoverse" },
      { id: "data", label: "Local data" },
    ],
  },
  {
    group: "Migrate",
    items: [
      { id: "servers", label: "Servers" },
      { id: "migration", label: "Tracking & defaults" },
    ],
  },
];

const HISTORY_OPTIONS = [
  { days: 0, label: "Don't save" },
  { days: 7, label: "7 days" },
  { days: 30, label: "30 days" },
  { days: 90, label: "90 days" },
];

/**
 * Every setting in one place, opened from the gear wherever you are: what the app connects to and keeps, then
 * Migrate's servers and how it tracks. Each section saves on its own. `section` (with a changing `nonce`) scrolls to
 * one, for links like "Manage servers".
 */
export function SettingsPage({ settings, onSaved, section, nonce }: { settings: Settings; onSaved: (s: Settings) => void; section?: SettingsSection; nonce?: number }) {
  const refs = useRef<Partial<Record<SettingsSection, HTMLElement | null>>>({});
  const [active, setActive] = useState<SettingsSection>(section ?? "nable");
  const go = (id: SettingsSection) => {
    setActive(id);
    refs.current[id]?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  useEffect(() => {
    if (section) go(section);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [section, nonce]);

  const sectionProps = (id: SettingsSection) => ({ id: `settings-${id}`, ref: (el: HTMLElement | null) => void (refs.current[id] = el) });

  return (
    <>
      <PageHeader title="Settings" meta="Kept on this computer. Keys and tokens are in your system keychain." />
      <PageBody>
        <div className="flex flex-wrap items-start gap-6">
          <nav aria-label="Settings sections" className="sticky top-0 flex flex-[1_1_200px] flex-col gap-4">
            {SECTIONS.map((g) => (
              <div key={g.group} className="space-y-1">
                <div className="px-3 text-[11px] font-semibold tracking-wider text-faint uppercase">{g.group}</div>
                {g.items.map((it) => (
                  <button
                    key={it.id}
                    type="button"
                    onClick={() => go(it.id)}
                    aria-current={active === it.id ? "true" : undefined}
                    className={`flex h-8 w-full items-center rounded-lg px-3 text-left text-sm ${active === it.id ? "bg-accent-soft font-semibold text-text" : "text-muted hover:bg-surface-2 hover:text-text"}`}
                  >
                    {it.label}
                  </button>
                ))}
              </div>
            ))}
          </nav>

          <div className="min-w-0 flex-[999_1_560px] space-y-6">
            <SettingsCard {...sectionProps("nable")} title="Ecoverse" text="The Ecoverse API token used to run scripts on demand: Run, Activity and Tags.">
              <NableSettings settings={settings} onSaved={onSaved} />
            </SettingsCard>
            <SettingsCard {...sectionProps("data")} title="Local data" text="Run history, cached lists and the audit log, kept on this computer.">
              <DataSettings settings={settings} onSaved={onSaved} />
            </SettingsCard>
            <SettingsCard {...sectionProps("servers")} title="Servers" text="The N-central servers migrations move devices between. JWTs and Ecoverse tokens stay in the keychain.">
              <ServersPanel />
            </SettingsCard>
            <SettingsCard {...sectionProps("migration")} title="Tracking & defaults" text="How migrations watch devices, and the options a new project starts with.">
              <MigrationSettingsPanel />
            </SettingsCard>
          </div>
        </div>
      </PageBody>
    </>
  );
}

function SettingsCard({ id, ref, title, text, children }: { id: string; ref: (el: HTMLElement | null) => void; title: string; text: string; children: ReactNode }) {
  return (
    <section id={id} ref={ref} aria-labelledby={`${id}-title`} className="scroll-mt-4">
      <Card className="space-y-5 p-5">
        <div>
          <h2 id={`${id}-title`} className="text-base font-semibold">
            {title}
          </h2>
          <p className="text-sm text-muted">{text}</p>
        </div>
        {children}
      </Card>
    </section>
  );
}

function NableSettings({ settings, onSaved }: { settings: Settings; onSaved: (s: Settings) => void }) {
  const [apiUrl, setApiUrl] = useState(settings.apiUrl);
  const [apiKey, setApiKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const changed = apiKey.trim() !== "" || apiUrl !== settings.apiUrl;

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const next = await saveSettings(apiUrl, apiKey, settings.historyDays);
      // A different key or endpoint may mean a different account: drop cached data.
      queryClient.clear();
      setApiKey("");
      setSaved(true);
      onSaved(next);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await clearApiKey();
      queryClient.clear();
      // Without a key the app asks to connect again.
      onSaved({ ...settings, hasApiKey: false });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (changed) void save();
      }}
    >
      <p className="-mt-2 text-xs text-faint">N-central servers set to use “the app's Ecoverse key” use this token for migrations too.</p>
      <Field label="Ecoverse API token" hint="Saved in your system keychain. Paste a new token to replace it.">
        <input
          type="password"
          className={`${inputClass} h-9 max-w-md`}
          value={apiKey}
          onChange={(e) => {
            setApiKey(e.target.value);
            setSaved(false);
          }}
          placeholder="••••••••••••"
          autoComplete="off"
          spellCheck={false}
        />
      </Field>
      <Field label="Ecoverse API endpoint">
        <input
          className={`${inputClass} max-w-md`}
          value={apiUrl}
          onChange={(e) => {
            setApiUrl(e.target.value);
            setSaved(false);
          }}
          spellCheck={false}
        />
      </Field>
      {error && <ErrorNote>{error}</ErrorNote>}
      <div className="flex flex-wrap items-center gap-2 border-t border-border pt-4">
        <Button variant="danger" onClick={() => void remove()} disabled={busy}>
          <KeyRound />
          Remove token
        </Button>
        <span className="flex-1" />
        {saved && !changed && <span className="text-xs text-ok">Saved</span>}
        <Button type="submit" variant="primary" busy={busy} disabled={!changed}>
          Save
        </Button>
      </div>
    </form>
  );
}

function DataSettings({ settings, onSaved }: { settings: Settings; onSaved: (s: Settings) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [cleared, setCleared] = useState(false);
  const [savedHistory, setSavedHistory] = useState(false);

  // One choice, so it saves as soon as it changes.
  const setHistory = async (days: number) => {
    setBusy(true);
    setError(null);
    try {
      onSaved(await saveSettings(settings.apiUrl, undefined, days));
      setSavedHistory(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const clearData = async () => {
    setBusy(true);
    setError(null);
    try {
      await clearLocalData();
      queryClient.clear();
      clearFinishedJobs();
      setConfirmClear(false);
      setCleared(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      {settings.encryptionAvailable ? (
        <p className="flex items-start gap-2 text-sm text-muted">
          <Dot tone="ok" className="mt-1.5" />
          Everything here is encrypted with a key kept in your system keychain.
        </p>
      ) : (
        <p className="flex items-start gap-2 text-sm text-muted">
          <Dot tone="warn" className="mt-1.5" />
          <span>
            Encryption is unavailable ({settings.encryptionError ?? "no system keychain"}). History and cached lists are kept in memory only, and the audit log is
            stored unencrypted.
          </span>
        </p>
      )}
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <label htmlFor="history-days" className="w-40 text-sm">
          Keep run history
        </label>
        <select id="history-days" className={`${selectClass} w-36`} value={settings.historyDays} disabled={busy} onChange={(e) => void setHistory(Number(e.target.value))}>
          {HISTORY_OPTIONS.map((o) => (
            <option key={o.days} value={o.days}>
              {o.label}
            </option>
          ))}
        </select>
        {savedHistory && <span className="text-xs text-ok">Saved</span>}
      </div>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-t border-border pt-4">
        <span className="flex-1 text-sm text-muted">{cleared ? "History and cached data cleared." : "Clears finished runs and cached lists. Runs in progress keep going."}</span>
        {confirmClear ? (
          <div className="flex gap-2">
            <Button size="sm" variant="ghost" onClick={() => setConfirmClear(false)} disabled={busy}>
              Cancel
            </Button>
            <Button size="sm" variant="destructive" onClick={() => void clearData()} busy={busy}>
              Clear
            </Button>
          </div>
        ) : (
          <Button size="sm" variant="danger" onClick={() => setConfirmClear(true)} disabled={busy}>
            <Trash2 />
            Clear history and cache
          </Button>
        )}
      </div>
      {error && <ErrorNote>{error}</ErrorNote>}
    </div>
  );
}
