import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { CandidateQuery, CleanupProgress, DeviceChoice, DeviceCommand, MigrationSettings, PlanQuery, TrackQuery, LaunchResult, MapView, Override, PollDigest, ReportFormat, ScriptOrg, ScriptSide } from "./bindings";
import { jobStore } from "../jobs";
import { applyChoice } from "./map";
import {
  addPlanned,
  chooseForDevice,
  createWaveFrom,
  getCandidates,
  getMigrationSettings,
  saveMigrationSettings,
  cancelCleanup,
  checkConnection,
  cleanupHealthy,
  createStructure,
  getStructurePreview,
  getRetire,
  retireSources,
  createWave,
  deviceCommand,
  probeWaiting,
  getDevice,
  getFinish,
  getLaunch,
  getPlan,
  getTrack,
  launchWave,
  pollNow,
  previewWave,
  startTracker,
  stopTracker,
  writeReport,
  clearMapping,
  closeProject,
  createProject,
  deleteProject,
  exportScript,
  generateTokens,
  getMap,
  getMapDetails,
  getPrepare,
  getProject,
  listProjects,
  listServers,
  publishScript,
  removeServer,
  saveServer,
  setMapping,
  setEcoverseLink,
  setOptions,
  setRestScript,
  setServerEcoverse,
  testServer,
} from "./api";

// Migration data isn't in the persisted key list in lib/queryClient.ts, so none of it is written to disk by the
// query cache (projects live in the core's encrypted store).
export const projectsQuery = { queryKey: ["migrate", "projects"], queryFn: listProjects, staleTime: 5_000 } as const;
export const serversQuery = { queryKey: ["migrate", "servers"], queryFn: listServers, staleTime: 30_000 } as const;
export const projectQuery = (id: string) => ({ queryKey: ["migrate", "project", id], queryFn: () => getProject(id), staleTime: 5_000 }) as const;
/** The Connect checklist reads the servers, so it is read each time the step opens and never kept. */
export const connectQuery = (id: string) =>
  ({ queryKey: ["migrate", "connect", id], queryFn: () => checkConnection(id), staleTime: 0, gcTime: 0, retry: false, refetchOnWindowFocus: false }) as const;

export const useProjects = () => useQuery(projectsQuery);
export const useServers = () => useQuery(serversQuery);
export const useProject = (id: string) => useQuery(projectQuery(id));
export const useConnectCheck = (id: string) => useQuery(connectQuery(id));

export function useServerActions() {
  const qc = useQueryClient();
  const refresh = () => qc.invalidateQueries({ queryKey: ["migrate", "servers"] });
  return {
    save: useMutation({ mutationFn: saveServer, onSuccess: refresh }),
    ecoverse: useMutation({ mutationFn: setServerEcoverse, onSuccess: refresh }),
    remove: useMutation({ mutationFn: removeServer, onSuccess: refresh }),
    test: useMutation({ mutationFn: testServer }),
  };
}

export function useProjectActions() {
  const qc = useQueryClient();
  const refresh = () => qc.invalidateQueries({ queryKey: ["migrate", "projects"] });
  return {
    create: useMutation({ mutationFn: createProject, onSuccess: refresh }),
    // Deleting a project also drops what this window holds of it: its cached screens and its entries in Activity.
    remove: useMutation({
      mutationFn: deleteProject,
      onSuccess: (_done, id: string) => {
        jobStore.removeForProject(id);
        qc.removeQueries({ predicate: (q) => q.queryKey[0] === "migrate" && q.queryKey.includes(id) });
        return refresh();
      },
    }),
  };
}

// The Map rows come from both servers and the details from every customer's devices, so neither is refetched on
// focus; the operator asks again with Re-run matching.
const mapQuery = (id: string) => ({ queryKey: ["migrate", "map", id], queryFn: () => getMap(id), staleTime: Infinity, gcTime: 300_000, retry: false, refetchOnWindowFocus: false }) as const;
const mapDetailsQuery = (id: string) =>
  ({ queryKey: ["migrate", "map-details", id], queryFn: () => getMapDetails(id), staleTime: Infinity, gcTime: 300_000, retry: false, refetchOnWindowFocus: false }) as const;

export const useMap = (id: string) => useQuery(mapQuery(id));
/** Waits for the rows, so the two server reads don't compete. */
export const useMapDetails = (id: string, enabled: boolean) => useQuery({ ...mapDetailsQuery(id), enabled });

export function useMapActions(id: string) {
  const qc = useQueryClient();
  const key = ["migrate", "map", id];
  const refreshDetails = () => qc.invalidateQueries({ queryKey: ["migrate", "map-details", id] });
  return {
    /** Changes the row at once and puts it back if the core refuses. */
    choose: useMutation({
      mutationFn: ({ sourceId, decision }: { sourceId: string; decision: Override }) => setMapping(id, sourceId, decision),
      onMutate: async ({ sourceId, decision }) => {
        await qc.cancelQueries({ queryKey: key });
        const before = qc.getQueryData<MapView>(key);
        if (before) qc.setQueryData<MapView>(key, applyChoice(before, sourceId, decision));
        return { before };
      },
      onError: (_e, _v, context) => context?.before && qc.setQueryData(key, context.before),
      onSuccess: refreshDetails,
    }),
    /** Back to the automatic match, which only the core can work out. */
    clear: useMutation({
      mutationFn: (sourceId: string) => clearMapping(id, sourceId),
      onSuccess: () => Promise.all([qc.invalidateQueries({ queryKey: key }), refreshDetails()]),
    }),
    generate: useMutation({ mutationFn: () => generateTokens(id), onSuccess: refreshDetails }),
    release: () => closeProject(id).catch(() => undefined),
  };
}

const prepareQuery = (id: string) => ({ queryKey: ["migrate", "prepare", id], queryFn: () => getPrepare(id), staleTime: 0, retry: false }) as const;
export const usePrepare = (id: string) => useQuery(prepareQuery(id));

export function usePrepareActions(id: string) {
  const qc = useQueryClient();
  const refresh = () => Promise.all([qc.invalidateQueries({ queryKey: ["migrate", "prepare", id] }), qc.invalidateQueries({ queryKey: ["migrate", "project", id] })]);
  return {
    publish: useMutation({ mutationFn: ({ side, org }: { side: ScriptSide; org: ScriptOrg }) => publishScript(id, side, org), onSuccess: refresh }),
    exportScript: useMutation({ mutationFn: exportScript }),
    restScript: useMutation({ mutationFn: ({ side, itemId, version }: { side: ScriptSide; itemId: string; version: string }) => setRestScript(id, side, itemId, version), onSuccess: refresh }),
    // Whether Ecoverse is checked changes what every later screen says about a device, so all of them are read again.
    ecoverseLink: useMutation({
      mutationFn: (choice: Parameters<typeof setEcoverseLink>[1]) => setEcoverseLink(id, choice),
      onSuccess: () => Promise.all([refresh(), ...["plan", "launch", "track", "finish", "device"].map((what) => qc.invalidateQueries({ queryKey: ["migrate", what, id] })), qc.invalidateQueries({ queryKey: ["migrate", "projects"] })]),
    }),
    // A delivery change re-paths the planned devices, so what Plan, Launch and Track show changes too.
    options: useMutation({
      mutationFn: (options: Parameters<typeof setOptions>[1]) => setOptions(id, options),
      onSuccess: () => Promise.all([refresh(), ...["plan", "launch", "track", "wave-preview"].map((what) => qc.invalidateQueries({ queryKey: ["migrate", what, id] }))]),
    }),
  };
}

// ---- Plan -------------------------------------------------------------------------------------------------------

// The plan reads both servers and Ecoverse, so it is read when the step opens and when asked, never in the background.
// Reading the servers is the slow part, so the core keeps what it read for a while; a page, filter or choice is quick.
// Asking to read again goes through `useReread`, which sets the result under the same key.
export const usePlan = (id: string, query: PlanQuery) =>
  useQuery({ queryKey: ["migrate", "plan", id, query], queryFn: () => getPlan(id, false, query), staleTime: Infinity, gcTime: 300_000, retry: false, refetchOnWindowFocus: false, placeholderData: keepPreviousData });

export function useRereadPlan(id: string, query: PlanQuery) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => getPlan(id, true, query),
    onSuccess: (view) => {
      qc.invalidateQueries({ queryKey: ["migrate", "plan", id], refetchType: "none" });
      qc.setQueryData(["migrate", "plan", id, query], view);
    },
  });
}

/** Everything the steps after Plan show comes from the saved project, so adding devices refreshes all of it. */
function useRefreshProject(id: string) {
  const qc = useQueryClient();
  return () =>
    Promise.all([
      // The project list shows each project's progress and who needs you, so it changes with the project.
      qc.invalidateQueries({ queryKey: ["migrate", "projects"] }),
      ...["project", "launch", "track", "finish", "plan"].map((what) => qc.invalidateQueries({ queryKey: what === "project" ? ["migrate", "project", id] : ["migrate", what, id] })),
    ]);
}

export function usePlanActions(id: string) {
  const refresh = useRefreshProject(id);
  return {
    add: useMutation({ mutationFn: () => addPlanned(id), onSuccess: refresh }),
    choose: useMutation({ mutationFn: ({ deviceId, choice }: { deviceId: string; choice: DeviceChoice }) => chooseForDevice(id, deviceId, choice), onSuccess: refresh }),
  };
}

// ---- Launch -----------------------------------------------------------------------------------------------------

export const useLaunch = (id: string) => useQuery({ queryKey: ["migrate", "launch", id], queryFn: () => getLaunch(id), staleTime: 0, retry: false, refetchOnWindowFocus: false });
export const useCandidates = (id: string, query: CandidateQuery, enabled: boolean) =>
  useQuery({ queryKey: ["migrate", "candidates", id, query], queryFn: () => getCandidates(id, query), enabled, staleTime: 0, gcTime: 0, retry: false, placeholderData: keepPreviousData });
export const useWavePreview = (id: string, wave: number | null) =>
  useQuery({ queryKey: ["migrate", "wave-preview", id, wave], queryFn: () => previewWave(id, wave as number), enabled: wave !== null, staleTime: 0, gcTime: 0, retry: false });

export function useLaunchActions(id: string) {
  const refresh = useRefreshProject(id);
  return {
    createWave: useMutation({ mutationFn: ({ label, max }: { label: string; max: number }) => createWave(id, label, max), onSuccess: refresh }),
    createWaveFrom: useMutation({ mutationFn: ({ label, deviceIds }: { label: string; deviceIds: string[] }) => createWaveFrom(id, label, deviceIds), onSuccess: refresh }),
    launch: useMutation<LaunchResult, unknown, { wave: number; dryRun: boolean }>({ mutationFn: ({ wave, dryRun }) => launchWave(id, wave, dryRun), onSuccess: refresh }),
  };
}

// ---- Track ------------------------------------------------------------------------------------------------------

export const useTrack = (id: string, query: TrackQuery) =>
  useQuery({ queryKey: ["migrate", "track", id, query], queryFn: () => getTrack(id, query), staleTime: 0, retry: false, refetchOnWindowFocus: false, placeholderData: keepPreviousData });
export const useDevice = (id: string, index: number | null) =>
  useQuery({ queryKey: ["migrate", "device", id, index], queryFn: () => getDevice(id, index as number), enabled: index !== null, staleTime: 0, retry: false });

export interface TrackerState {
  // The last pass, and when this window heard of it.
  last: PollDigest | null;
  error: string | null;
  // The tracker has been asked to run and hasn't reported yet.
  starting: boolean;
}

/**
 * Runs the project's tracker while the Track step is open and refreshes what it shows after each pass. The runner is
 * in the native side (one pass now and then every minute); this only starts it, stops it, and listens.
 */
export function useTracker(id: string, enabled: boolean): TrackerState {
  const qc = useQueryClient();
  const [state, setState] = useState<TrackerState>({ last: null, error: null, starting: true });

  useEffect(() => {
    if (!enabled) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    setState({ last: null, error: null, starting: true });
    listen<{ id: string; digest: PollDigest | null; error: string | null }>("migration-polled", (event) => {
      if (event.payload.id !== id) return;
      setState({ last: event.payload.digest, error: event.payload.error, starting: false });
      for (const key of ["project", "track", "finish", "launch", "device"]) qc.invalidateQueries({ queryKey: ["migrate", key, id] });
      qc.invalidateQueries({ queryKey: ["migrate", "projects"] });
    })
      .then((stop) => {
        if (!cancelled) unlisten = stop;
        else Promise.resolve().then(stop).catch(() => undefined);
      })
      .catch(() => undefined);
    startTracker(id).catch((e) => setState({ last: null, error: String(e), starting: false }));
    return () => {
      cancelled = true;
      // Stopping a listener that is already gone isn't an error, and it fails asynchronously.
      Promise.resolve()
        .then(() => unlisten?.())
        .catch(() => undefined);
      stopTracker(id).catch(() => undefined);
    };
  }, [id, enabled, qc]);

  return state;
}

export function useDeviceActions(id: string) {
  const refresh = useRefreshProject(id);
  const qc = useQueryClient();
  return {
    command: useMutation({
      mutationFn: ({ index, command }: { index: number; command: DeviceCommand }) => deviceCommand(id, index, command),
      onSettled: () => Promise.all([refresh(), qc.invalidateQueries({ queryKey: ["migrate", "device", id] })]),
    }),
    poll: useMutation({
      mutationFn: () => pollNow(id),
      onSettled: () => Promise.all([refresh(), qc.invalidateQueries({ queryKey: ["migrate", "device", id] })]),
    }),
    probeWaiting: useMutation({
      mutationFn: () => probeWaiting(id),
      onSettled: () => Promise.all([refresh(), qc.invalidateQueries({ queryKey: ["migrate", "device", id] })]),
    }),
  };
}

// ---- Finish -----------------------------------------------------------------------------------------------------

export const useFinish = (id: string) => useQuery({ queryKey: ["migrate", "finish", id], queryFn: () => getFinish(id), staleTime: 0, retry: false, refetchOnWindowFocus: false });

export function useFinishActions(id: string) {
  const refresh = useRefreshProject(id);
  return {
    report: useMutation({ mutationFn: (format: ReportFormat) => writeReport(id, format) }),
    cleanup: useMutation({ mutationFn: (only: string[] | null) => cleanupHealthy(id, only), onSettled: refresh }),
  };
}

export const useStructurePreview = (id: string, enabled: boolean) =>
  useQuery({ queryKey: ["migrate", "structure", id], queryFn: () => getStructurePreview(id), enabled, staleTime: 0, gcTime: 0, retry: false });

/** Creating customers and sites changes what the Map and Plan show and what tokens are missing. */
export function useCreateStructure(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ serviceOrgId, dryRun }: { serviceOrgId: string; dryRun: boolean }) => createStructure(id, serviceOrgId, dryRun),
    onSettled: () => Promise.all(["map", "map-details", "plan", "structure"].map((what) => qc.invalidateQueries({ queryKey: ["migrate", what, id] }))),
  });
}

export const useRetire = (id: string) => useQuery({ queryKey: ["migrate", "retire", id], queryFn: () => getRetire(id), staleTime: 0, retry: false, refetchOnWindowFocus: false });

export function useRetireActions(id: string) {
  const qc = useQueryClient();
  const refresh = useRefreshProject(id);
  return useMutation({
    mutationFn: ({ dryRun, beforeReviewEnds }: { dryRun: boolean; beforeReviewEnds: boolean }) => retireSources(id, dryRun, beforeReviewEnds),
    onSettled: () => Promise.all([refresh(), qc.invalidateQueries({ queryKey: ["migrate", "retire", id] })]),
  });
}

/** The latest step of each device in a bulk cleanup, by device name, while it runs. */
export function useCleanupProgress(id: string, active: boolean): Record<string, CleanupProgress> {
  const [steps, setSteps] = useState<Record<string, CleanupProgress>>({});
  useEffect(() => {
    if (!active) return;
    setSteps({});
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    listen<{ id: string; progress: CleanupProgress }>("migration-cleanup", (event) => {
      if (event.payload.id === id) setSteps((cur) => ({ ...cur, [event.payload.progress.name]: event.payload.progress }));
    })
      .then((stop) => {
        if (!cancelled) unlisten = stop;
        else Promise.resolve().then(stop).catch(() => undefined);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      Promise.resolve()
        .then(() => unlisten?.())
        .catch(() => undefined);
    };
  }, [id, active]);
  return steps;
}

export const stopCleanup = (id: string) => cancelCleanup(id).catch(() => undefined);

// ---- Migration settings -----------------------------------------------------------------------------------------

export const useMigrationSettings = (enabled = true) => useQuery({ queryKey: ["migrate", "settings"], queryFn: getMigrationSettings, enabled, staleTime: Infinity, retry: false });
export function useSaveMigrationSettings() {
  const qc = useQueryClient();
  return useMutation({ mutationFn: (settings: MigrationSettings) => saveMigrationSettings(settings), onSuccess: (saved) => qc.setQueryData(["migrate", "settings"], saved) });
}
