import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { CleanupProgress, DeviceCommand, LaunchResult, MapView, Override, PollDigest, ReportFormat, ScriptOrg, ScriptSide } from "./bindings";
import { applyChoice } from "./map";
import {
  addPlanned,
  cancelCleanup,
  checkConnection,
  cleanupHealthy,
  createWave,
  deviceCommand,
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
    remove: useMutation({ mutationFn: deleteProject, onSuccess: refresh }),
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
    options: useMutation({ mutationFn: (options: Parameters<typeof setOptions>[1]) => setOptions(id, options), onSuccess: refresh }),
  };
}

// ---- Plan -------------------------------------------------------------------------------------------------------

// The plan reads both servers and Ecoverse, so it is read when the step opens and when asked, never in the background.
const planQuery = (id: string) => ({ queryKey: ["migrate", "plan", id], queryFn: () => getPlan(id), staleTime: Infinity, gcTime: 300_000, retry: false, refetchOnWindowFocus: false }) as const;
export const usePlan = (id: string) => useQuery(planQuery(id));

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
  return { add: useMutation({ mutationFn: () => addPlanned(id), onSuccess: refresh }) };
}

// ---- Launch -----------------------------------------------------------------------------------------------------

export const useLaunch = (id: string) => useQuery({ queryKey: ["migrate", "launch", id], queryFn: () => getLaunch(id), staleTime: 0, retry: false });
export const useWavePreview = (id: string, wave: number | null) =>
  useQuery({ queryKey: ["migrate", "wave-preview", id, wave], queryFn: () => previewWave(id, wave as number), enabled: wave !== null, staleTime: 0, gcTime: 0, retry: false });

export function useLaunchActions(id: string) {
  const refresh = useRefreshProject(id);
  return {
    createWave: useMutation({ mutationFn: ({ label, max }: { label: string; max: number }) => createWave(id, label, max), onSuccess: refresh }),
    launch: useMutation<LaunchResult, unknown, { wave: number; dryRun: boolean }>({ mutationFn: ({ wave, dryRun }) => launchWave(id, wave, dryRun), onSuccess: refresh }),
  };
}

// ---- Track ------------------------------------------------------------------------------------------------------

export const useTrack = (id: string) => useQuery({ queryKey: ["migrate", "track", id], queryFn: () => getTrack(id), staleTime: 0, retry: false });
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
  };
}

// ---- Finish -----------------------------------------------------------------------------------------------------

export const useFinish = (id: string) => useQuery({ queryKey: ["migrate", "finish", id], queryFn: () => getFinish(id), staleTime: 0, retry: false });

export function useFinishActions(id: string) {
  const refresh = useRefreshProject(id);
  return {
    report: useMutation({ mutationFn: (format: ReportFormat) => writeReport(id, format) }),
    cleanup: useMutation({ mutationFn: () => cleanupHealthy(id), onSettled: refresh }),
  };
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
