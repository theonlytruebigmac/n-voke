import { useState } from "react";
import type { ReactNode } from "react";
import { MoreHorizontal, Plus, Trash2 } from "lucide-react";
import type { EcoCredential, ServerView } from "../../lib/migrate/bindings";
import { useServerActions, useServers } from "../../lib/migrate/queries";
import { Button, Dot, ErrorNote, Field, IconButton, Menu, MenuContent, MenuItem, MenuTrigger, Modal, Sheet, inputClass, type Tone } from "../ui";

/** A short state word on a server card: a dot and the word, on a tinted pill. */
function Chip({ tone, children }: { tone: Tone; children: ReactNode }) {
  return (
    <span className="inline-flex h-6 items-center gap-1.5 rounded-full border border-border bg-bg px-2.5 text-xs whitespace-nowrap text-muted">
      <Dot tone={tone} />
      {children}
    </span>
  );
}

const ECO_CHOICES: { value: EcoCredential; title: string; text: string }[] = [
  { value: "appKey", title: "The app's Ecoverse key", text: "The Ecoverse token in Settings › Ecoverse. Use it when this server is in the same Ecoverse tenant." },
  { value: "own", title: "A token for this server", text: "Its own Ecoverse token, for a server in another tenant. Kept in the keychain." },
  { value: "none", title: "Don't use Ecoverse", text: "Reach its devices through N-central only." },
];

const ecoSummary = (s: ServerView) => (s.ecoverse === "appKey" ? "Ecoverse: app key" : s.ecoverse === "own" ? (s.hasEcoverseToken ? "Ecoverse: own token" : "Ecoverse: token missing") : "REST only");

type TestState = { kind: "testing" } | { kind: "ok"; text: string } | { kind: "err"; text: string };

interface Form {
  /** Null while adding. */
  id: string | null;
  label: string;
  host: string;
  jwt: string;
  eco: EcoCredential;
  ecoToken: string;
  /** An Ecoverse token is already saved for this server. */
  hasEcoToken: boolean;
}

const emptyForm: Form = { id: null, label: "", host: "", jwt: "", eco: "appKey", ecoToken: "", hasEcoToken: false };

/** The servers in a side panel, for opening from the middle of something else (the New project dialog). */
export function ServersSheet({ onClose }: { onClose: () => void }) {
  return (
    <Sheet title="N-central servers" subtitle="JWTs and Ecoverse tokens are kept in the system keychain" onClose={onClose}>
      <ServersPanel />
    </Sheet>
  );
}

/**
 * Add, edit, test and remove the N-central servers (screen C in docs/migration/11-gui-design.md), on its own: in
 * Settings, or inside ServersSheet.
 */
export function ServersPanel() {
  const servers = useServers();
  const actions = useServerActions();
  const [form, setForm] = useState<Form | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [tests, setTests] = useState<Record<string, TestState>>({});
  const [removing, setRemoving] = useState<ServerView | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);

  const test = (server: { id: string; label: string }) => {
    setTests((t) => ({ ...t, [server.id]: { kind: "testing" } }));
    actions.test.mutate(server.id, {
      onSuccess: (info) => setTests((t) => ({ ...t, [server.id]: { kind: "ok", text: `signed in · N-central ${info.ncentral} · API ${info.version}` } })),
      onError: (e) => setTests((t) => ({ ...t, [server.id]: { kind: "err", text: String(e) } })),
    });
  };

  const save = () => {
    if (!form) return;
    setFormError(null);
    actions.save.mutate(
      { id: form.id, label: form.label, host: form.host, jwt: form.jwt },
      {
        onSuccess: (saved) => {
          // The server exists now, so a failed key step is retried as an edit rather than a second add.
          actions.ecoverse.mutate(
            { id: saved.id, mode: form.eco, token: form.ecoToken },
            {
              onSuccess: () => {
                setForm(null);
                test(saved);
              },
              onError: (e) => {
                setForm({ ...form, id: saved.id, jwt: "" });
                setFormError(String(e));
              },
            },
          );
        },
        onError: (e) => setFormError(String(e)),
      },
    );
  };

  const list = servers.data ?? [];

  return (
    <>
      <div className="space-y-4">
        {servers.isError && <ErrorNote>{String(servers.error)}</ErrorNote>}

        {list.length === 0 && !form && !servers.isPending && (
          <p className="text-sm text-muted">No servers yet. Add the source and the destination server to start a migration.</p>
        )}

        {list.length > 0 && (
          <ul className="space-y-2">
            {list.map((s) => {
              const t = tests[s.id];
              const ecoMissing = s.ecoverse === "own" && !s.hasEcoverseToken;
              return (
                <li key={s.id} className="space-y-2.5 rounded-xl border border-border bg-surface px-4 py-3">
                  <div className="flex items-start gap-3">
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-semibold" title={s.label}>
                        {s.label}
                      </div>
                      {s.label !== s.host && (
                        <div className="truncate text-xs text-faint" title={s.host}>
                          {s.host}
                        </div>
                      )}
                    </div>
                    <div className="flex shrink-0 items-center gap-1.5">
                      {s.hasJwt && (
                        <Button size="sm" onClick={() => test(s)} busy={t?.kind === "testing"}>
                          Test
                        </Button>
                      )}
                      <Button
                        size="sm"
                        onClick={() => {
                          setFormError(null);
                          setForm({ id: s.id, label: s.label, host: s.host, jwt: "", eco: s.ecoverse, ecoToken: "", hasEcoToken: s.hasEcoverseToken });
                        }}
                      >
                        Edit
                      </Button>
                      <Menu>
                        <MenuTrigger asChild>
                          <IconButton label={`More for ${s.label}`}>
                            <MoreHorizontal />
                          </IconButton>
                        </MenuTrigger>
                        <MenuContent>
                          <MenuItem
                            danger
                            onSelect={() => {
                              setRemoveError(null);
                              setRemoving(s);
                            }}
                          >
                            <Trash2 />
                            Remove server…
                          </MenuItem>
                        </MenuContent>
                      </Menu>
                    </div>
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    <Chip tone={s.hasJwt ? "ok" : "err"}>{s.hasJwt ? "JWT saved" : "No JWT"}</Chip>
                    <Chip tone={ecoMissing ? "err" : s.ecoverse === "none" ? "muted" : "ok"}>{ecoSummary(s)}</Chip>
                  </div>
                  {t && t.kind !== "testing" && (
                    <p className={`text-xs wrap-break-word ${t.kind === "ok" ? "text-ok" : "text-err"}`}>{t.kind === "ok" ? `✓ ${t.text}` : t.text}</p>
                  )}
                </li>
              );
            })}
          </ul>
        )}

        {form ? (
          <div className="space-y-3 rounded-xl border border-accent/40 bg-surface p-4">
            <h3 className="text-sm font-semibold">{form.id ? "Edit server" : "Add a server"}</h3>
            <Field label="Name" hint="Shown in lists. Defaults to the host name. Use a different name per service organization when they share a host.">
              <input className={inputClass} value={form.label} onChange={(e) => setForm({ ...form, label: e.target.value })} placeholder="Old server" />
            </Field>
            <Field label="Host" hint="The server's address, for example old.example.com. HTTPS only, no port or path.">
              <input className={inputClass} value={form.host} onChange={(e) => setForm({ ...form, host: e.target.value })} placeholder="old.example.com" autoCapitalize="off" spellCheck={false} />
            </Field>
            <Field label="User API JWT" hint={form.id ? "Leave blank to keep the saved one." : "Create one in N-central under the user's API access. It is stored in the keychain."}>
              <input
                className={inputClass}
                type="password"
                value={form.jwt}
                onChange={(e) => setForm({ ...form, jwt: e.target.value })}
                placeholder={form.id ? "••••••••" : "Paste the JWT"}
                autoComplete="off"
                spellCheck={false}
              />
            </Field>
            <fieldset className="space-y-2">
              <legend className="text-xs font-medium text-muted">Ecoverse key for this server's devices</legend>
              {ECO_CHOICES.map((o) => (
                <label key={o.value} className="flex cursor-pointer items-start gap-2.5 rounded-xl border border-border px-3.5 py-2.5 has-[:checked]:border-accent has-[:checked]:bg-accent-soft">
                  <input type="radio" name="eco" className="mt-1" checked={form.eco === o.value} onChange={() => setForm({ ...form, eco: o.value })} />
                  <span className="text-sm">
                    <span className="font-medium">{o.title}</span>
                    <span className="block text-xs text-muted">{o.text}</span>
                  </span>
                </label>
              ))}
              {form.eco === "own" && (
                <input
                  className={inputClass}
                  type="password"
                  value={form.ecoToken}
                  onChange={(e) => setForm({ ...form, ecoToken: e.target.value })}
                  placeholder={form.hasEcoToken ? "•••••••• (leave blank to keep it)" : "Paste the Ecoverse token"}
                  aria-label="Ecoverse token"
                  autoComplete="off"
                  spellCheck={false}
                />
              )}
              <p className="text-xs text-faint">The JWT is always needed: org units, registration tokens and the REST fallback only exist in N-central.</p>
            </fieldset>
            {formError && <ErrorNote>{formError}</ErrorNote>}
            <div className="flex justify-end gap-2">
              <Button
                onClick={() => {
                  setForm(null);
                  setFormError(null);
                }}
              >
                Cancel
              </Button>
              <Button variant="primary" onClick={save} busy={actions.save.isPending || actions.ecoverse.isPending} disabled={!form.host.trim() || (!form.id && !form.jwt.trim()) || (form.eco === "own" && !form.ecoToken.trim() && !form.hasEcoToken)}>
                {form.id ? "Save" : "Add and test"}
              </Button>
            </div>
          </div>
        ) : (
          <Button
            onClick={() => {
              setFormError(null);
              setForm(emptyForm);
            }}
          >
            <Plus />
            Add server
          </Button>
        )}
      </div>

      {removing && (
        <Modal
          title={`Remove ${removing.label}?`}
          description="Its saved JWT and Ecoverse token are deleted from the keychain. A server a project uses can't be removed."
          onClose={() => setRemoving(null)}
          footer={
            <>
              <Button onClick={() => setRemoving(null)}>Cancel</Button>
              <Button
                variant="destructive"
                busy={actions.remove.isPending}
                onClick={() =>
                  actions.remove.mutate(removing.id, {
                    onSuccess: () => setRemoving(null),
                    onError: (e) => setRemoveError(String(e)),
                  })
                }
              >
                Remove
              </Button>
            </>
          }
        >
          {removeError && <ErrorNote>{removeError}</ErrorNote>}
        </Modal>
      )}
    </>
  );
}
