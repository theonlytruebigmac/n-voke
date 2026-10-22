# Agent migration: endpoint scripts and lab harness

Phase 0 working material for the [agent migration plan](../../docs/migration/README.md). The design and everything learned so far are in the [Phase 0 findings](../../docs/migration/10-phase0-findings.md).

## `endpoint/`

| File | What it is |
|---|---|
| `nvoke-migration.template.ps1` | The one script that runs on devices, through Ecoverse or N-central REST. `Action` = `Launch`, `Status` or `Cleanup`. No `param()` block (Ecoverse prepends its inputs); never calls `exit`; never writes to stderr |
| `installAgent.ps1` | Run by the scheduled task the launcher registers: download and verify the installer, full cleanup of the old agent (`CleanMode=full`), install, then wait until the Windows Agent and the UFA are healthy on the destination |
| `build.mjs` | Embeds `installAgent.ps1` (base64 + SHA-256) into the template and stamps a content-hash version: `node build.mjs` writes `dist/nvoke-migration.ps1` (git-ignored). Both files must stay ASCII |

Checks: `pwsh -File endpoint/run-tests.ps1` runs PSScriptAnalyzer and the Pester tests (`endpoint/Endpoint.Tests.ps1`, which build the script first). Needs Node, Pester 5+ and PSScriptAnalyzer (`Install-Module Pester, PSScriptAnalyzer -Scope CurrentUser`). CI runs the same on Windows PowerShell 5.1 and pwsh.

The built `dist/nvoke-migration.ps1` is what gets published to Ecoverse, or uploaded by hand to an N-central Script/Software Repository (with **Enable API** on) for the REST path.

## `lab/`

Node scripts (Node 20+, macOS) that drove the Phase 0 tests against two lab N-central servers and their Ecoverse tenants.

Setup:

1. `cp lab.config.example.json lab.config.json` and fill in both servers (host, service org name and numeric ID, Ecoverse service org ID). `lab.config.json` is git-ignored.
2. Store the four secrets in the macOS Keychain (prompted, so they never reach shell history):
   ```sh
   security add-generic-password -U -s nvoke-lab -a ecoverse:src -w
   security add-generic-password -U -s nvoke-lab -a ecoverse:dst -w
   security add-generic-password -U -s nvoke-lab -a ncentral:src -w
   security add-generic-password -U -s nvoke-lab -a ncentral:dst -w
   ```
3. `node ../endpoint/build.mjs` before using `mig.mjs`.

| Script | Use |
|---|---|
| `probe.mjs [src\|dst\|all]` | Read-only pass over REST, SOAP and Ecoverse; writes redacted responses to `out-*/` |
| `eco-test.mjs create\|update\|run\|poll` | Ecoverse script publishing and input/output tests (`probes/input-probe*.ps1`) |
| `rest-test.mjs run\|poll` | REST direct-task tests against a hand-uploaded `probes/nvoke-rest-probe.ps1` |
| `mig.mjs publish\|launch\|action\|result\|watch` | Publish the built endpoint script to both tenants, launch a migration, run `Status`/`Cleanup`, and watch every stage |
| `watch-name.mjs <device>` | Log a device's presence and state in both N-centrals and both Ecoverse tenants, by name |
| `run-ps1.mjs <src\|dst> <assetId> probes/<file>.ps1` | Run a read-only diagnostic (`nc-inventory.ps1`, `ufa-diag.ps1`) and print its report |
| `probes/collect-logs.ps1` | Run **on the device**, elevated (`powershell -NoProfile -ExecutionPolicy Bypass -File collect-logs.ps1 -ExtraHost <source N-central>,<destination N-central>`): collects every Ecoverse agent log (core, component, provision and each component's own), the MSI logs in `C:\ProgramData\MspAgent\Tools`, the Windows Agent's logs from the last 3 days (`-Days`), the N-voke installer's `status.json` and logs, and a `system.txt` (services, processes, the `MSP Core Agent` Windows Installer registration, proxy, time, scheduled tasks, the last 7 days of MsiInstaller and service events, and a DNS + TCP + TLS test of the agent's hosts, 3 tries each) into one zip in `C:\Temp`. Read-only. Tokens, JWTs, passwords and long base64 values are masked, and in the Windows Agent and MSI logs any line mentioning a token, secret, password, key or the provisioning GUID is withheld whole; certificates, vault, database and config files are never taken. `manifest.txt` in the zip lists what it holds. Look through the zip before sending it |
| `probes/ufa-log-summary.ps1` | Run **on the device** (`powershell -NoProfile -ExecutionPolicy Bypass -File ufa-log-summary.ps1`, or `-LogPath` for a copied log): summarises the Ecoverse agent's core log: provisioning, login-token results (accepted, 400 SSL error, timeouts, connection resets), the last login attempts, the hourly update checks, components and service restarts, the device-identity errors in `component.log` (`-ComponentLogPath`; `-AsOfUtc <time>` for a copied log, since only the last 30 minutes count), and a `STATE:`. Read-only, prints no secrets. Its `STATE:` comes from a verbatim copy of the installer's `Get-UfaSyncState` (between `BEGIN shared` and `END shared` markers; a Pester test fails if the copies differ), so it shows the state the installer's watcher acts on, with what the installer does about it. The probe adds the counts, the last login attempts and the timeline. Use it to see what the agent's log says; the device sheet (after a Probe) shows what the installer actually did. [Troubleshooting](../../docs/migration/12-troubleshooting.md) says when to use it |

Everything the harness writes (state, logs, task dumps) is git-ignored. Test scripts it publishes are named `N-voke Phase0 …` or `N-voke lab: …` and are safe to delete.

## Driving a migration

A migration is run from the **Migrate** page of the desktop app (Connect, Map, Prepare, Plan, Launch, Track, Finish). The terminal app has no migration features; it used to have a command line and a Migrate page for this, and both were removed (2026-10-21). Every change the desktop app makes can be tried first as a dry run: reads work, and each change is held back and listed with its exact (redacted) request.

The endpoint script is built into the app from the same two source files `build.mjs` uses (a test checks the bytes match). `NVOKE_MIGRATION_SCRIPT` points the app at a built file instead, to try a change without rebuilding it. Any change to the scripts changes the version. A launch carries the version N-voke expects, and an uploaded copy that is a different version refuses to run and says to upload again, so a stale file fails clearly instead of running old logic. Prepare has a button that saves the script for the REST path, where it is uploaded to N-central by hand (with *Enable API* on).
