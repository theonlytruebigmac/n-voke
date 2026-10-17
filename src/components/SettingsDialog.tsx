import { useState } from "react";
import { KeyRound, Trash2 } from "lucide-react";
import { clearApiKey, saveSettings, type Settings } from "../lib/api";
import { clearFinishedJobs } from "../lib/jobs";
import { queryClient } from "../lib/queryClient";
import { clearLocalData } from "../lib/secureStore";
import { Button, Dot, ErrorNote, Field, Modal, inputClass, selectClass } from "./ui";

const HISTORY_OPTIONS = [
  { days: 0, label: "Don't save" },
  { days: 7, label: "7 days" },
  { days: 30, label: "30 days" },
  { days: 90, label: "90 days" },
];

export function SettingsDialog({
  settings,
  onClose,
  onSaved,
}: {
  settings: Settings;
  onClose: () => void;
  onSaved: (s: Settings) => void;
}) {
  const [apiUrl, setApiUrl] = useState(settings.apiUrl);
  const [apiKey, setApiKey] = useState("");
  const [historyDays, setHistoryDays] = useState(settings.historyDays);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [cleared, setCleared] = useState(false);
  const firstRun = !settings.hasApiKey;

  const save = async () => {
    if (firstRun && !apiKey.trim()) {
      setError("Enter your API key.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const saved = await saveSettings(apiUrl, apiKey, historyDays);
      // A different key or endpoint may mean a different account: drop cached data.
      if (apiKey.trim() || saved.apiUrl !== settings.apiUrl) queryClient.clear();
      onSaved(saved);
      onClose();
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
      onSaved({ ...settings, hasApiKey: false });
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
    <Modal
      title={firstRun ? "Connect to N-able" : "Settings"}
      description={firstRun ? "Paste an API-Access token from the N-able dashboard." : undefined}
      onClose={onClose}
      dismissible={!firstRun}
      footer={
        <>
          {settings.hasApiKey && (
            <Button variant="danger" onClick={remove} disabled={busy} className="mr-auto">
              <KeyRound />
              Remove key
            </Button>
          )}
          {!firstRun && (
            <Button onClick={onClose} disabled={busy}>
              Cancel
            </Button>
          )}
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
        <section className="space-y-4">
          <Field label="API key" hint={settings.hasApiKey ? "Saved in your system keychain. Enter a new key to replace it." : "Stored in your system keychain."}>
            <input
              type="password"
              className={inputClass}
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder={settings.hasApiKey ? "••••••••••••" : "Paste your API key"}
              autoFocus
              autoComplete="off"
              spellCheck={false}
            />
          </Field>
          <Field label="API endpoint">
            <input className={inputClass} value={apiUrl} onChange={(e) => setApiUrl(e.target.value)} spellCheck={false} />
          </Field>
        </section>

        <section className="space-y-3 border-t border-border pt-4">
          <h3 className="text-sm font-semibold">Data on this computer</h3>
          {settings.encryptionAvailable ? (
            <p className="flex items-start gap-2 text-sm text-muted">
              <Dot tone="ok" className="mt-1.5" />
              Run history, cached lists and the audit log are encrypted with a key kept in your system keychain.
            </p>
          ) : (
            <p className="flex items-start gap-2 text-sm text-muted">
              <Dot tone="warn" className="mt-1.5" />
              <span>
                Encryption is unavailable ({settings.encryptionError ?? "no system keychain"}). History and cached lists are kept in memory only, and the
                audit log is stored unencrypted.
              </span>
            </p>
          )}
          <div className="flex items-center justify-between gap-4">
            <label htmlFor="history-days" className="text-sm">
              Keep run history
            </label>
            <select id="history-days" className={`${selectClass} w-36`} value={historyDays} onChange={(e) => setHistoryDays(Number(e.target.value))}>
              {HISTORY_OPTIONS.map((o) => (
                <option key={o.days} value={o.days}>
                  {o.label}
                </option>
              ))}
            </select>
          </div>
          <div className="flex items-center justify-between gap-4">
            <span className="text-sm text-muted">{cleared ? "History and cached data cleared." : "Runs in progress keep going."}</span>
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
        </section>

        {error && <ErrorNote>{error}</ErrorNote>}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}
