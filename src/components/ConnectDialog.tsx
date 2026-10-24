import { useState, type FormEvent } from "react";
import { saveSettings, type Settings } from "../lib/api";
import { queryClient } from "../lib/queryClient";
import { Button, ErrorNote, Field, Modal, inputClass } from "./ui";

/**
 * First launch, or after the key was removed: one job, connecting. It can't be dismissed until a key is saved;
 * everything else is in Settings.
 */
export function ConnectDialog({ settings, onSaved }: { settings: Settings; onSaved: (s: Settings) => void }) {
  const [apiUrl, setApiUrl] = useState(settings.apiUrl);
  const [apiKey, setApiKey] = useState("");
  const [showEndpoint, setShowEndpoint] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const connect = async (e: FormEvent) => {
    e.preventDefault();
    if (!apiKey.trim()) {
      setError("Enter your Ecoverse API token.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const saved = await saveSettings(apiUrl, apiKey, settings.historyDays);
      // A new key may mean a different account: drop cached data.
      queryClient.clear();
      onSaved(saved);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="Connect to Ecoverse" description="Paste an Ecoverse API-Access token from the N-able dashboard. It's what runs scripts on your devices." onClose={() => undefined} dismissible={false} size="sm">
      <form className="space-y-4" onSubmit={connect}>
        <Field label="Ecoverse API token" hint="Stored in your system keychain.">
          <input
            type="password"
            className={`${inputClass} h-9`}
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder="Paste your token"
            autoFocus
            autoComplete="off"
            spellCheck={false}
          />
        </Field>
        {showEndpoint || apiUrl !== settings.apiUrl ? (
          <Field label="Ecoverse API endpoint">
            <input className={inputClass} value={apiUrl} onChange={(e) => setApiUrl(e.target.value)} spellCheck={false} />
          </Field>
        ) : (
          <button type="button" onClick={() => setShowEndpoint(true)} className="text-xs font-medium text-accent hover:text-accent-hover">
            Use a different API endpoint
          </button>
        )}
        {error && <ErrorNote>{error}</ErrorNote>}
        <Button type="submit" variant="primary" busy={busy} className="h-10 w-full text-sm">
          Connect
        </Button>
      </form>
    </Modal>
  );
}
