import { invoke } from "@tauri-apps/api/core";

/*
 * Encrypted local storage, provided by the Rust side (see vault.rs). The web
 * view itself keeps nothing on disk. If encryption is unavailable (no system
 * keychain), reads return null and writes are skipped, so data simply stays
 * in memory for the session instead of being written in plain text.
 */

let available: boolean | null = null;
/** Set when old plain-text data was found in localStorage (see takeLegacy). */
let legacyFound = false;

export async function storeGet(name: string): Promise<string | null> {
  try {
    const value = await invoke<string | null>("store_get", { name });
    available = true;
    return value;
  } catch {
    available = false;
    return null;
  }
}

export async function storeSet(name: string, value: string): Promise<void> {
  if (available === false) return;
  try {
    await invoke("store_set", { name, value });
  } catch (e) {
    console.warn(`Couldn't save ${name}:`, e);
  }
}

export async function storeRemove(name: string): Promise<void> {
  try {
    await invoke("store_remove", { name });
  } catch {
    // Nothing to remove, or storage unavailable: either way it's gone.
  }
}

export const clearLocalData = () => invoke<void>("clear_local_data");

/**
 * Earlier versions kept data in the web view's localStorage in plain text.
 * Returns the old value (for migrating into the encrypted store) and deletes it.
 */
export function takeLegacy(key: string): string | null {
  try {
    const value = localStorage.getItem(key);
    if (value !== null) legacyFound = true;
    localStorage.removeItem(key);
    return value;
  } catch {
    return null;
  }
}

/**
 * After old data has been moved into the encrypted store, wipe the web view's
 * storage entirely so no plain-text copy survives (deleted database rows can
 * otherwise linger on disk). Does nothing if there was no old data.
 */
export async function purgeLegacyStorage(): Promise<void> {
  if (!legacyFound) return;
  try {
    await invoke("purge_webview_storage");
    legacyFound = false;
  } catch (e) {
    console.warn("Couldn't clear old web view storage:", e);
  }
}
