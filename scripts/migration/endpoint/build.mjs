// Builds dist/nvoke-migration.ps1: embeds installAgent.ps1 (base64) and its SHA-256, and stamps a content-hash version.
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const installer = readFileSync(join(here, "installAgent.ps1"));
const template = readFileSync(join(here, "nvoke-migration.template.ps1"), "utf8");
for (const [name, text] of [["installAgent.ps1", installer.toString("utf8")], ["template", template]]) {
  if (/[^\x09\x0a\x0d\x20-\x7e]/.test(text)) throw new Error(`${name} must be ASCII (Windows PowerShell 5.1 reads BOM-less files as ANSI)`);
}
const installerSha = createHash("sha256").update(installer).digest("hex").toUpperCase();
const version = "0.1.0+" + createHash("sha256").update(template).update(installer).digest("hex").slice(0, 7);
const out = template
  .replaceAll("__SCRIPT_VERSION__", version)
  .replace("__INSTALLER_B64__", installer.toString("base64"))
  .replace("__INSTALLER_SHA256__", installerSha);
mkdirSync(join(here, "dist"), { recursive: true });
writeFileSync(join(here, "dist", "nvoke-migration.ps1"), out.replace(/\r?\n/g, "\r\n"));
writeFileSync(join(here, "dist", "version.json"), JSON.stringify({ version, installerSha256: installerSha, bytes: Buffer.byteLength(out) }, null, 2));
console.log(`built dist/nvoke-migration.ps1 ${version} (${Buffer.byteLength(out)} bytes, installer sha256 ${installerSha.slice(0, 12)}…)`);
