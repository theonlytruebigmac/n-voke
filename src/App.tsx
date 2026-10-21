import { useCallback, useEffect, useState, type ReactNode } from "react";
import { ArrowRightLeft, ListChecks, Play, ScrollText, Settings as SettingsIcon, Tags } from "lucide-react";
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
import { RunPage } from "./components/RunPage";
import { SettingsDialog } from "./components/SettingsDialog";
import { TagsPage } from "./components/TagsPage";
import { ErrorNote, TooltipProvider } from "./components/ui";

type Page = "run" | "activity" | "migrate" | "tags" | "audit";

function NavItem({ icon, label, active, count, onClick }: { icon: ReactNode; label: string; active: boolean; count?: number; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={`flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-sm transition-colors [&_svg]:size-4 ${
        active ? "bg-surface-3 font-medium text-text" : "text-muted hover:bg-surface-2 hover:text-text"
      }`}
    >
      {icon}
      <span className="flex-1 text-left">{label}</span>
      {!!count && <span className="text-xs text-accent tabular-nums">{count}</span>}
    </button>
  );
}

/** The Migrate entry shows how many devices across all projects need a person. */
function MigrateNavItem({ active, onClick }: { active: boolean; onClick: () => void }) {
  const { data } = useProjects();
  const needYou = (data ?? []).reduce((n, p) => n + p.attention, 0);
  return <NavItem icon={<ArrowRightLeft />} label="Migrate" active={active} count={needYou} onClick={onClick} />;
}

export default function App() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
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
        if (!s.hasApiKey) setShowSettings(true);
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

  const showJob = useCallback((jobId: string) => {
    setSelectedJobId(jobId);
    setPage("activity");
  }, []);

  return (
    <PersistQueryClientProvider client={queryClient} persistOptions={persistOptions} onSuccess={() => setRestored(true)} onError={() => setRestored(true)}>
      <TooltipProvider delayDuration={300}>
        <div className="flex h-full">
          <nav className="flex w-48 shrink-0 flex-col border-r border-border bg-sidebar px-2 pb-2">
            <div className="flex h-[52px] items-center gap-2 px-2.5">
              <img src={logo} alt="" className="size-5 rounded-[5px]" />
              <span className="text-sm font-semibold">N-voke</span>
            </div>
            <div className="space-y-0.5">
              <NavItem icon={<Play />} label="Run" active={page === "run"} onClick={() => setPage("run")} />
              <NavItem icon={<ListChecks />} label="Activity" active={page === "activity"} count={activeJobs} onClick={() => setPage("activity")} />
              <MigrateNavItem active={page === "migrate"} onClick={() => setPage("migrate")} />
              <NavItem icon={<Tags />} label="Tags" active={page === "tags"} onClick={() => setPage("tags")} />
              <NavItem icon={<ScrollText />} label="Audit log" active={page === "audit"} onClick={() => setPage("audit")} />
            </div>
            <div className="mt-auto">
              <NavItem icon={<SettingsIcon />} label="Settings" active={false} onClick={() => settings && setShowSettings(true)} />
            </div>
          </nav>

          <main className="flex min-w-0 flex-1 flex-col">
            {settingsError && (
              <div className="p-3">
                <ErrorNote>{settingsError}</ErrorNote>
              </div>
            )}
            {/* Run stays mounted so a half-built run survives a trip to another page. */}
            <div className={page === "run" ? "flex min-h-0 flex-1 flex-col" : "hidden"}>
              <RunPage ready={ready} draft={draft} onStarted={showJob} onOpenSettings={() => setShowSettings(true)} />
            </div>
            {page === "activity" && <ActivityPage jobs={jobs} selectedId={selectedJobId} onSelect={setSelectedJobId} onDraft={openDraft} ready={ready} />}
            {page === "migrate" && (
              <PageBoundary name="Migrate">
                <MigratePage ecoverseReady={ready} />
              </PageBoundary>
            )}
            {page === "tags" && <TagsPage ready={ready} onDraft={openDraft} />}
            {page === "audit" && <AuditPage />}
          </main>

          {showSettings && settings && (
          <SettingsDialog
            settings={settings}
            onClose={() => setShowSettings(false)}
            onSaved={(s) => {
              setSettings(s);
              setHistoryDays(s.historyDays);
            }}
          />
        )}
        </div>
      </TooltipProvider>
    </PersistQueryClientProvider>
  );
}
