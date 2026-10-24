import { invoke } from "@tauri-apps/api/core";
import type { LinkChoice, StructurePreview, StructureReport, RetireReport, RetireView, CandidatePage, CandidateQuery, DeviceChoice, MigrationSettings, PlanQuery, ProbeReport, TrackQuery, CleanupReport, ConnectReport, DeviceCommand, DeviceDetail, FinishView, LaunchPreview, LaunchResult, LaunchView, PlanAdded, PlanView, PollDigest, ReportFormat, TrackView, Delivery, DestEcoverse, EcoCredential, MapDetails, MapView, Override, PrepareView, ProjectOptions, ScriptSide, ProjectSummary, ScriptOrg, ServerInfo, ServerView, TokenReport } from "./bindings";

// Thin typed wrappers over the Tauri commands. No logic lives here: the rules are in nvoke-core, and the types
// come from the Rust types (`npm run bindings`), so this file only names the commands.

export interface ServerInput {
  /** Present when editing a saved server. */
  id?: string | null;
  label: string;
  host: string;
  /** Blank keeps the saved one when editing. Goes to the keychain; never comes back. */
  jwt?: string | null;
}

export interface EcoverseInput {
  id: string;
  mode: EcoCredential;
  /** Only with `own`. Blank keeps the saved one. Goes to the keychain; never comes back. */
  token?: string | null;
}

export interface NewProject {
  name: string;
  sourceServerId: string;
  destServerId: string;
  destEcoverse: DestEcoverse;
  /** Ecoverse organizations to search for the source server's devices; empty plans everything on the REST path. */
  sourceEcoverseOrgs: string[];
  delivery: Delivery;
}

export const listServers = () => invoke<ServerView[]>("servers_list");
export const saveServer = (s: ServerInput) =>
  invoke<ServerView>("server_save", { id: s.id ?? null, label: s.label, host: s.host, jwt: s.jwt?.trim() || null, ecoverseTenant: null });
export const setServerEcoverse = (e: EcoverseInput) => invoke<ServerView>("server_ecoverse", { id: e.id, mode: e.mode, token: e.token?.trim() || null });
export const testServer = (id: string) => invoke<ServerInfo>("server_test", { id });
export const removeServer = (id: string) => invoke<void>("server_remove", { id });

export const listProjects = () => invoke<ProjectSummary[]>("projects_list");
export const getProject = (id: string) => invoke<ProjectSummary>("project_summary", { id });
export const createProject = (p: NewProject) => invoke<ProjectSummary>("project_create", { ...p });
export const deleteProject = (id: string) => invoke<void>("project_delete", { id });
export const checkConnection = (id: string) => invoke<ConnectReport>("project_connect_check", { id });
export const closeProject = (id: string) => invoke<void>("project_close", { id });

export const getMap = (id: string) => invoke<MapView>("project_map", { id });
export const getMapDetails = (id: string) => invoke<MapDetails>("project_map_details", { id });
export const setMapping = (id: string, sourceId: string, decision: Override) => invoke<void>("project_map_set", { id, sourceId, decision });
export const clearMapping = (id: string, sourceId: string) => invoke<void>("project_map_clear", { id, sourceId });
export const generateTokens = (id: string) => invoke<TokenReport>("project_tokens_generate", { id });

export const getPrepare = (id: string) => invoke<PrepareView>("project_prepare", { id });
export const searchScriptOrgs = (id: string, side: ScriptSide, term: string) => invoke<ScriptOrg[]>("project_script_orgs", { id, side, term });
export const defaultScriptOrg = (id: string, side: ScriptSide) => invoke<ScriptOrg | null>("project_default_org", { id, side });
export const publishScript = (id: string, side: ScriptSide, org: ScriptOrg) => invoke<string>("project_publish", { id, side, org });
export const exportScript = () => invoke<string>("project_export_script");
export const setRestScript = (id: string, side: ScriptSide, itemId: string, version: string) => invoke<void>("project_rest_script", { id, side, itemId, version });
export const setEcoverseLink = (id: string, choice: LinkChoice) => invoke<void>("project_ecoverse_link", { id, choice });
export const setOptions = (id: string, options: ProjectOptions) => invoke<void>("project_options", { id, options });

export const forgetReads = (id: string) => invoke<void>("project_forget_reads", { id });
export const getPlan = (id: string, refresh: boolean, query: PlanQuery) => invoke<PlanView>("project_plan", { id, refresh, query });
export const chooseForDevice = (id: string, deviceId: string, choice: DeviceChoice) => invoke<string>("project_device_choice", { id, deviceId, choice });
export const addPlanned = (id: string) => invoke<PlanAdded>("project_plan_add", { id });

export const getLaunch = (id: string) => invoke<LaunchView>("project_launch", { id });
export const createWave = (id: string, label: string, max: number) => invoke<string>("project_wave_create", { id, label, max });
export const getCandidates = (id: string, query: CandidateQuery) => invoke<CandidatePage>("project_wave_candidates", { id, query });
export const createWaveFrom = (id: string, label: string, deviceIds: string[]) => invoke<string>("project_wave_create_from", { id, label, deviceIds });
export const previewWave = (id: string, wave: number) => invoke<LaunchPreview>("project_wave_preview", { id, wave });
export const launchWave = (id: string, wave: number, dryRun: boolean) => invoke<LaunchResult>("project_wave_launch", { id, wave, dryRun });

export const getTrack = (id: string, query: TrackQuery) => invoke<TrackView>("project_track", { id, query });
export const getDevice = (id: string, index: number) => invoke<DeviceDetail>("project_device", { id, index });
export const deviceCommand = (id: string, index: number, command: DeviceCommand) => invoke<string>("project_device_command", { id, index, command });
export const probeWaiting = (id: string) => invoke<ProbeReport>("project_probe_waiting", { id });
export const pollNow = (id: string) => invoke<PollDigest>("project_poll", { id });
export const startTracker = (id: string) => invoke<void>("tracker_start", { id });
export const stopTracker = (id: string) => invoke<void>("tracker_stop", { id });

export const getFinish = (id: string) => invoke<FinishView>("project_finish", { id });
export const writeReport = (id: string, format: ReportFormat) => invoke<string>("project_report", { id, format });
export const openReport = (path: string) => invoke<void>("project_report_open", { path });
export const revealReport = (path: string) => invoke<void>("project_report_reveal", { path });
export const setProjectClosed = (id: string, closed: boolean) => invoke<void>("project_set_closed", { id, closed });
export const cleanupHealthy = (id: string, only: string[] | null) => invoke<CleanupReport>("project_cleanup", { id, only });
export const getStructurePreview = (id: string) => invoke<StructurePreview>("project_structure_preview", { id, only: null });
export const createStructure = (id: string, serviceOrgId: string, dryRun: boolean) => invoke<StructureReport>("project_structure_create", { id, serviceOrgId, only: null, dryRun });
export const getRetire = (id: string) => invoke<RetireView>("project_retire_view", { id });
export const retireSources = (id: string, dryRun: boolean, beforeReviewEnds: boolean) => invoke<RetireReport>("project_retire", { id, dryRun, beforeReviewEnds });
export const cancelCleanup = (id: string) => invoke<void>("project_cleanup_cancel", { id });

export const getMigrationSettings = () => invoke<MigrationSettings>("migration_settings_get");
export const saveMigrationSettings = (settings: MigrationSettings) => invoke<MigrationSettings>("migration_settings_save", { settings });
