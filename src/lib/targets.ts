import type { Asset, Organization, Script, Tag } from "./types";

export type Target = { kind: "device"; asset: Asset } | { kind: "tag"; tag: Tag };

export const targetKey = (t: Target) => (t.kind === "device" ? `d:${t.asset.id}` : `t:${t.tag.id}`);

/** "PatchGroupTwo + 2 devices", "SLS-2188", "3 tags + 1 device". */
export function targetLabel(targets: Target[]): string {
  const tags = targets.filter((t): t is Extract<Target, { kind: "tag" }> => t.kind === "tag");
  const devices = targets.filter((t): t is Extract<Target, { kind: "device" }> => t.kind === "device");
  const parts: string[] = [];
  if (tags.length === 1) parts.push(tags[0].tag.name);
  else if (tags.length > 1) parts.push(`${tags.length} tags`);
  if (devices.length === 1 && !tags.length) parts.push(devices[0].asset.name);
  else if (devices.length) parts.push(`${devices.length} device${devices.length === 1 ? "" : "s"}`);
  return parts.join(" + ") || "No targets";
}

/** Pre-filled Run page state, e.g. from "Run again on failed" or a tag's "Run script". */
export interface RunDraft {
  /** Changes on every request so the Run page knows to apply it. */
  nonce: number;
  org: Organization | null;
  script?: Script | null;
  targets?: Target[];
}

/** Minimal Asset for a device known only by ID and name (e.g. from a past job). */
export function assetStub(id: string, name: string): Asset {
  return { id, name, customer: null, site: null, agentConnection: null };
}

/** Scripts in these languages only run on Windows. */
const WINDOWS_LANGUAGES = new Set(["POWERSHELL", "BATCH", "CMD", "VBSCRIPT"]);
const UNIX_LANGUAGES = new Set(["BASH", "SHELL", "SH", "ZSH"]);

/** Explains why a script probably can't run on a device's OS, or null. */
export function osMismatch(language: string | null | undefined, osName: string | null | undefined): string | null {
  if (!language || !osName) return null;
  const isWindows = /windows/i.test(osName);
  if (WINDOWS_LANGUAGES.has(language) && !isWindows) return "not Windows";
  if (UNIX_LANGUAGES.has(language) && isWindows) return "Windows";
  return null;
}
