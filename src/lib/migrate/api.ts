import { invoke } from "@tauri-apps/api/core";
import type { ConnectReport, Delivery, DestEcoverse, EcoCredential, ProjectSummary, ServerInfo, ServerView } from "./bindings";

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
