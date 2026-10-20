# 8. Security

Migration adds two kinds of credential to N-voke, two new APIs it writes to, and code that runs as SYSTEM on customer devices. This document covers how each is handled.

## Credentials

| Credential | Used for | Stored |
|---|---|---|
| Ecoverse API-Access token | Every Ecoverse request (unchanged) | OS keychain, account `api-key` (unchanged) |
| Second Ecoverse token, when the destination server is linked to another tenant | Seeing and cleaning up moved devices in the destination tenant | OS keychain, account `ecoverse:<tenant id>` |
| N-central user API JWT, one per server | REST authentication | OS keychain, account `ncentral:<host>` |
| REST access and refresh tokens | Short-lived REST sessions | Memory only |
| Destination registration tokens | Passed to devices at launch | Memory, plus the encrypted project store (needed to relaunch). Never in logs, the audit log or reports |

- The web UI never sees any of these. As today, it calls Rust commands; it doesn't make network requests (CSP `connect-src` stays limited to the app's IPC).
- Every request goes over HTTPS with OS trust-store validation. Non-HTTPS server addresses are rejected, as the Ecoverse endpoint already is.
- Replacing or removing a server's JWT clears its cached REST session and the server's cached data.
- **Least privilege:** the recommended N-central API users are scoped to the service orgs being migrated. N-voke shows which permissions failed during Connect, but it doesn't need, ask for or store administrator passwords.

## What reaches endpoints

| Item | On the device? | Notes |
|---|---|---|
| N-central API JWT | **Never** | The PowerShell project's main weakness, removed |
| Ecoverse token | **Never** | |
| Registration token | Yes, in `job.json` | Required by the installer. It can only register agents to one customer or site, and it expires. Deleted once the agent is healthy. On the **REST path** it's also in the direct task's `CommandLine`, which N-central shows in full in the task's details to anyone who can view scheduled tasks, **even when the parameter is sent as type `password`** (confirmed in the lab), and briefly in the device's process list. Rotate tokens after the project; if this exposure isn't acceptable, use Ecoverse only, or pre-created devices with per-device activation keys ([Q8](09-roadmap.md#open-questions)) |
| Destination server and customer ID | Yes | Not secret |

Where the registration token is briefly visible on the device or elsewhere, by design of the platforms:

- **Ecoverse:** PASSWORD inputs are hidden in the Ecoverse UI and API, but Ecoverse writes them as plain-text assignments into its temporary copy of the script (`…\systemprofile\AppData\Local\MSPScriptExecution\scripts\<guid>.ps1`, SYSTEM's profile) for the duration of the run; the file is deleted afterwards.
- **Installer:** `REGISTRATION_TOKEN` is on `WindowsAgentSetup.exe`'s command line for the minute the install runs (readable by local administrators). ❓ The verbose MSI log N-voke writes (`/L*V`) may also record it; the log sits in the restricted folder and is deleted at cleanup, but this must be checked and fixed (drop the log or scrub it) before release.
- **REST path:** the command line in N-central's task details (above), and the device's process list while the launcher runs.

**REST task output is public to anyone with the URL.** N-central stores each direct task's full output and exit code in a zip under `/download/scheduled_tasks/output_files/…` that downloads **without authentication**, at a predictable path (org unit, device ID, task ID, task name, timestamp). The endpoint script therefore never prints a secret, and nothing it prints is sensitive. (To be reported to N-able.)

Device-side protections ([Endpoint scripts](05-endpoint-scripts.md)):

- The working folder is `C:\ProgramData\N-voke\Migration`, readable and writable only by SYSTEM and Administrators, with inheritance disabled. The PowerShell project's Everyone: Full Control folder let any local user plant code that ran as SYSTEM; that's fixed.
- The installer script's hash is checked against the value baked into the launcher before each install.
- The registration token is read from `job.json`, never placed on the scheduled task's command line.
- The scheduled task has an expiry date and removes itself on success, so nothing keeps running as SYSTEM indefinitely.
- The installer is downloaded only over HTTPS from the destination server named in the job.

## Audit log

Every change N-voke makes is recorded in the encrypted, append-only audit log, now including REST writes:

| Action | Recorded |
|---|---|
| `script.create`, `script.update`, `script.delete` | Script name, organization, version hash |
| `ncentral.registrationToken.generate` | Server, customer or site; never the token |
| `migration.restScript.set` | Server, item ID entered by the operator |
| `migration.launch` | Project, wave, path, device count, destination customers; **registration tokens redacted** |
| `migration.cleanup` | Devices, outcome |
| `migration.retire` | Source device IDs deleted or tagged |
| `tag.assign`, `tag.unassign` | As today |
| `ncentral.customProperty.set` | Device or org unit, property, new value |
| Settings changes to servers or credentials | Server and action; never the secret |

PASSWORD-type script inputs are already redacted by N-voke; the registration token input is declared PASSWORD, so it's covered in the audit log, the dry-run view and task details shown in the app.

## Destructive actions

| Action | Effect | Guard |
|---|---|---|
| Launch a wave | Removes the old agent **and, with full cleanup (the default), every N-central component, folder and registry key** (see [Endpoint scripts](05-endpoint-scripts.md#full-cleanup-cleanmodefull-the-default)) from every device in the wave | Wave confirmation listing counts and paths. A pilot wave is suggested first. Dry-run available. On the device, nothing is removed until the destination's installer has downloaded and passed its signature check |
| Delete source device records | Removes the old records (`removeAgents=false`), and with them their stale Ecoverse assets | Off by default; separate confirmation; only for devices in **Cleaned up** |
| Delete the Ecoverse script | Removes it from Ecoverse (the REST path's repository copy is removed by hand) | Only when no device in any open project still needs it |
| Delete a project | Removes local history | Confirmation; the audit log keeps its entries |

N-voke's API calls never uninstall agents (`removeAgents` is always false) and never delete destination devices or org units. Removing the old agent from a device is done by the endpoint script on that device, as the migration itself, after the confirmation above.

## Data at rest

- Project stores use the existing encryption (XChaCha20-Poly1305, data key in the keychain) and atomic, user-only file writes.
- Collected `status.json` files and outputs are capped like run output (64 KB per device per event).
- Reports exported to disk contain device names, IDs and stages, **never** tokens. They're written readable only by the current user.

## Threats considered

| Threat | Mitigation |
|---|---|
| Local user on an endpoint reads or replaces the installer to get SYSTEM | Restricted folder ACL; hash check before running |
| API key leaked from an endpoint | No API key is ever sent to endpoints |
| Registration token leaked from an endpoint | Lets someone register a device into one customer until the token expires. The token is deleted after success; rotate tokens in N-central after the project |
| Wrong customer ID sends devices to the wrong customer | Mapping review; Wrong place detection; inputs validated against the mapping at launch |
| Operator machine compromised | Credentials in the OS keychain, data encrypted at rest, and the same exposure as N-voke today; recommend scoped API users |
| Malicious or mistaken edits to the launcher in Ecoverse | N-voke checks the published script's content hash before each launch and republishes or stops if it differs |
| Rate-limit lockout during large waves | Per-endpoint semaphores, backoff, and customer-level polling |
| A launch sent to the wrong device or server wipes its agent | Wave confirmation; inputs validated against the mapping; `DestServer` must be a plain host name; nothing is removed until the destination installer has downloaded and verified; the scheduled task keeps retrying the install until it's healthy |
| Secrets exposed through REST task output | The endpoint script never prints secrets (output zips download without authentication) |
