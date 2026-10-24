import { useCallback, useEffect, useState, type ReactNode } from "react";
import { Settings as SettingsIcon } from "lucide-react";
import { PersistQueryClientProvider } from "@tanstack/react-query-persist-client";
import logo from "./assets/logo.png";
import { getSettings, type Settings } from "./lib/api";
import { orgsQuery, tagsQuery } from "./lib/collections";
import { useProjects } from "./lib/migrate/queries";
import { persistOptions, queryClient } from "./lib/queryClient";
import { initJobs, isJobActive, setHistoryDays, useJobs } from "./lib/jobs";
import type { RunDraft } from "./lib/targets";
import { ActivityPage } from "./components/ActivityPage";
import { AuditPage } from "./components/AuditPage";
import { PageBoundary } from "./components/PageBoundary";
import { MigratePage } from "./components/migrate/MigratePage";
import { TrackerIndicator } from "./components/migrate/TrackerStatus";
import { RunPage } from "./components/RunPage";
import { ConnectDialog } from "./components/ConnectDialog";
import { SettingsPage, type SettingsSection } from "./components/settings/SettingsPage";
import { TagsPage } from "./components/TagsPage";
import { ErrorNote, IconButton, TooltipProvider } from "./components/ui";

type Page = "run" | "activity" | "migrate" | "tags" | "audit" | "settings";

function NavItem({ label, active, children, onClick }: { label: string; active: boolean; children?: ReactNode; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={`flex h-8 items-center gap-2 rounded-full px-3.5 text-sm whitespace-nowrap transition-colors ${
        active ? "bg-accent-soft font-semibold text-text" : "text-muted hover:bg-surface-2 hover:text-text"
      }`}
    >
      {label}
      {children}
    </button>
  );
}

/** A count on a nav item that asks for a person: amber. */
function NeedsYouBadge({ count }: { count: number }) {
  return <span className="min-w-4.5 rounded-full bg-warn-soft px-1.5 text-center text-[11px] leading-4.5 font-semibold text-warn tabular-nums">{count}</span>;
}

/** The Migrate entry shows how many devices across all projects need a person. */
function MigrateNavItem({ active, onClick }: { active: boolean; onClick: () => void }) {
  const { data } = useProjects();
  const needYou = (data ?? []).reduce((n, p) => n + p.attention, 0);
  return (
    <NavItem label="Migrate" active={active} onClick={onClick}>
      {needYou > 0 && <NeedsYouBadge count={needYou} />}
    </NavItem>
  );
}

export default function App() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  /** Which Settings section a link asked for; the nonce lets the same link scroll there again. */
  const [settingsAt, setSettingsAt] = useState<{ section?: SettingsSection; nonce: number }>({ nonce: 0 });
  const [page, setPage] = useState<Page>("run");
  const [draft, setDraft] = useState<RunDraft | null>(null);
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  /** The on-disk cache has been loaded (or failed to load). */
  const [restored, setRestored] = useState(false);
  const jobs = useJobs();
  const activeJobs = jobs.filter(isJobActive).length;

  useEffect(() => {
    getSettings()
      .then((s) => {
        setSettings(s);
        void initJobs(s.historyDays);
      })
      .catch((e) => setSettingsError(String(e)));
  }, []);

  const ready = !!settings?.hasApiKey;

  // Warm the reference lists in the background so the first picker opens instantly.
  // Waits for the disk cache: fresh copies are used as-is, older ones only need a delta.
  useEffect(() => {
    if (!ready || !restored) return;
    void queryClient.prefetchQuery(orgsQuery);
    void queryClient.prefetchQuery(tagsQuery);
  }, [ready, restored]);

  const openDraft = useCallback((d: Omit<RunDraft, "nonce">) => {
    setDraft({ ...d, nonce: Date.now() });
    setPage("run");
  }, []);

  const openSettings = useCallback((section?: SettingsSection) => {
    setSettingsAt({ section, nonce: Date.now() });
    setPage("settings");
  }, []);

  const onSettingsSaved = useCallback((s: Settings) => {
    setSettings(s);
    setHistoryDays(s.historyDays);
  }, []);

  const showJob = useCallback((jobId: string) => {
    setSelectedJobId(jobId);
    setPage("activity");
  }, []);

  return (
    <PersistQueryClientProvider client={queryClient} persistOptions={persistOptions} onSuccess={() => setRestored(true)} onError={() => setRestored(true)}>
      <TooltipProvider delayDuration={300}>
        <div className="flex h-full flex-col">
          <header className="flex min-h-14 shrink-0 flex-wrap items-center gap-x-6 gap-y-2 border-b border-border bg-sidebar px-5 py-2.5">
            <div className="flex items-center gap-2.5">
              <img src={logo} alt="" className="size-6 rounded-md" />
              <span className="text-[15px] font-semibold">N-voke</span>
            </div>
            <nav aria-label="Main" className="flex flex-1 flex-wrap items-center gap-1">
              <NavItem label="Run" active={page === "run"} onClick={() => setPage("run")} />
              <NavItem label="Activity" active={page === "activity"} onClick={() => setPage("activity")}>
                {activeJobs > 0 && <span className="text-[11px] font-semibold text-accent tabular-nums">{activeJobs} running</span>}
              </NavItem>
              <MigrateNavItem active={page === "migrate"} onClick={() => setPage("migrate")} />
              <NavItem label="Tags" active={page === "tags"} onClick={() => setPage("tags")} />
              <NavItem label="Audit log" active={page === "audit"} onClick={() => setPage("audit")} />
            </nav>
            <div className="flex items-center gap-2">
              <TrackerIndicator />
              <IconButton
                label="Settings"
                onClick={() => settings && openSettings()}
                aria-current={page === "settings" ? "page" : undefined}
                className={`border ${page === "settings" ? "border-accent/50 bg-accent-soft text-text" : "border-border-strong"}`}
              >
                <SettingsIcon />
              </IconButton>
            </div>
          </header>

          <main className="flex min-h-0 min-w-0 flex-1 flex-col">
            {settingsError && (
              <div className="p-3">
                <ErrorNote>{settingsError}</ErrorNote>
              </div>
            )}
            {/* Run stays mounted so a half-built run survives a trip to another page. */}
            <div className={page === "run" ? "flex min-h-0 flex-1 flex-col" : "hidden"}>
              <RunPage ready={ready} draft={draft} onStarted={showJob} onOpenSettings={() => openSettings("nable")} />
            </div>
            {page === "activity" && <ActivityPage jobs={jobs} selectedId={selectedJobId} onSelect={setSelectedJobId} onDraft={openDraft} ready={ready} />}
            {page === "migrate" && (
              <PageBoundary name="Migrate">
                <MigratePage ecoverseReady={ready} onOpenSettings={openSettings} />
              </PageBoundary>
            )}
            {page === "tags" && <TagsPage ready={ready} onDraft={openDraft} />}
            {page === "audit" && <AuditPage />}
            {page === "settings" && settings && <SettingsPage settings={settings} onSaved={onSettingsSaved} section={settingsAt.section} nonce={settingsAt.nonce} />}
          </main>

          {/* Without a key there's nothing else to do: connecting comes first, and can't be dismissed. */}
          {settings && !settings.hasApiKey && <ConnectDialog settings={settings} onSaved={onSettingsSaved} />}
        </div>
      </TooltipProvider>
    </PersistQueryClientProvider>
  );
}
