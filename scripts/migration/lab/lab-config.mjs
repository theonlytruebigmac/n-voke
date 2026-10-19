// Lab settings for the migration test harness. Real values live in lab.config.json (git-ignored);
// see lab.config.example.json. Secrets never go in the config: they're read from the macOS Keychain.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const LAB_DIR = dirname(fileURLToPath(import.meta.url));
const configPath = join(LAB_DIR, "lab.config.json");
if (!existsSync(configPath)) {
  throw new Error(`Missing ${configPath}. Copy lab.config.example.json to lab.config.json and fill in your lab servers.`);
}
const config = JSON.parse(readFileSync(configPath, "utf8"));

export const KEYCHAIN_SERVICE = config.keychainService ?? "nvoke-lab";

/**
 * Per side ("src", "dst"): host, serviceOrgName, serviceOrgId (N-central), ecoverseOrgId,
 * apiUser (only used by the SOAP probe), and the Keychain accounts for its secrets.
 */
export const SIDES = Object.fromEntries(
  ["src", "dst"].map((side) => [side, { ...config[side], eco: `ecoverse:${side}`, nc: `ncentral:${side}` }]),
);

/** Reads a secret from the Keychain (accounts: ecoverse:src, ecoverse:dst, ncentral:src, ncentral:dst). */
export function secret(account, { optional = false } = {}) {
  try {
    return execFileSync("security", ["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", account, "-w"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch {
    if (optional) return null;
    throw new Error(`No Keychain item "${account}" in service "${KEYCHAIN_SERVICE}". Add it with: security add-generic-password -U -s ${KEYCHAIN_SERVICE} -a ${account} -w`);
  }
}
