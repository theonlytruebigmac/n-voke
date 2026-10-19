# 5. Endpoint scripts

Four roles run on devices. The launcher, status probe and cleanup are one script, `nvoke-migration.ps1`, selected by its `Action` input (`Launch`, `Status`, `Cleanup`), so the REST path needs only one manual upload per server. The installer is embedded in it. Everything is embedded in N-voke, versioned by content hash and packaged as described in [Delivery paths](04-delivery-paths.md#one-script-two-packages). None of it contains a credential.

| Role | Runs through | When | Job |
|---|---|---|---|
| **Launcher** (`Action=Launch`) | Old agent (UFA or Windows Agent) | Launch | Stage files, write the job file, register the scheduled task, report back, exit |
| **Installer** (`installAgent.ps1`) | The scheduled task, as SYSTEM | Every 15 minutes until done | Check health; if not healthy on the destination, download and install; record progress |
| **Status probe** (`Action=Status`) | Old agent, while it still exists | On demand, for stuck devices | Return the status file and task state |
| **Cleanup** (`Action=Cleanup`) | New agent (UFA or Windows Agent) on the destination | After the device is healthy | Return the status file, remove the task and the folder |

On the REST path the cleanup runs on the **destination** server, so the same file is uploaded there too.

## Files on the device

```
C:\ProgramData\N-voke\Migration\           ACL: SYSTEM and Administrators only (no inheritance)
  job.json          what to install (written by the launcher)
  installAgent.ps1  the installer script
  status.json       progress record (written by the installer)
  logs\install-<timestamp>.log  (last 10 kept)
  WindowsAgentSetup.exe          (removed after each attempt)
```

This replaces `C:\NableMigrationTemp`, which the PowerShell project opened to **Everyone: Full Control**. That let any local user edit a script that runs as SYSTEM, and read the API key embedded in it. The new folder is created with an explicit ACL and inheritance disabled.

## Inputs

The same names are used on both paths. The packager declares them as Ecoverse script inputs; on the REST path they are `param()` arguments passed in the direct task's single `CommandLine` parameter (`-Action Launch -DestServer … -RegistrationToken …`).

| Input | Type | Example | Notes |
|---|---|---|---|
| `Action` | STRING | `Launch` | `Launch`, `Status` or `Cleanup` |
| `DestServer` | STRING | `new.example.com` | Validated as a host name, with no scheme, path or port |
| `DestCustomerId` | STRING | `1234` | Numeric N-central customer or site ID on the destination |
| `RegistrationToken` | PASSWORD | | Redacted in N-voke's audit log and dry-run view |
| `MigrationId` | STRING | project and wave ID | Written into `status.json` so results can be tied back |
| `LauncherVersion` | STRING | `1.0.0+3f9a2c1` | Lets the installer and tracker detect mixed versions |
| `TaskRepeatMinutes` | INT | `15` | Optional |
| `TaskExpiryDays` | INT | `14` | Optional; the task deletes itself after this |

A short preamble at the top of the script maps the platform's mechanism onto these variables. On Ecoverse, inputs arrive as PowerShell variables that Ecoverse assigns above the script body (confirmed, test 0.4). On the REST path they arrive as ordinary script arguments. The rest of the script doesn't depend on either.

Rules that come from how Ecoverse runs scripts ([Phase 0 findings](10-phase0-findings.md#ecoverse-script-runs-tests-01-02-04-2026-10-19)):

- **No `param()` block.** Ecoverse's input assignments come first, and `param()` must be the first statement, so the script reads REST `-Name value` arguments from `$args` itself. Every run sends **all** declared inputs (Ecoverse refuses a run with "Invalid number of input params." otherwise), using `-` for values an action doesn't need.
- **Never call `exit`** (or `return` at script level, or let a terminating error escape). Ecoverse reads outputs from lines it appends after the script body, so leaving early loses every output. The script wraps its work in `try`/`catch` and always falls through to the end. The outcome is reported in output variables (`Result`, `ErrorCode`). On failure the last statement is `$host.SetShouldExit($ErrorCode)`: Ecoverse then marks the run `FAILED` (with `exit status <code>` in `errorMessage`) and still returns the outputs. `exitCode` itself is always null. On the REST path the same values go in the `NVOKE-RESULT` line.
- **Outputs are single-line, never blank.** A value with a line break silently shifts every later output into the wrong variable. A blank value (empty string or unset variable) is dropped, which fails the whole output set. Every output is initialised to `-` at the top of the script. Values such as `StatusJson` are written with `ConvertTo-Json -Compress`, and every output is passed through a helper that strips line breaks.
- **The script runs in 32-bit PowerShell on Ecoverse and 64-bit on the REST path.** Anything that touches `System32`, `Program Files` or `HKLM\SOFTWARE` checks `[Environment]::Is64BitProcess` and uses `$env:windir\Sysnative` or the 64-bit registry view when needed.
- **Never write to stderr.** On the REST path any stderr output replaces stdout entirely, so the `NVOKE-RESULT` line is lost. Errors are caught and reported in the result; `$ErrorActionPreference` and `-ErrorAction` keep cmdlets from writing error records.
- **Never print a secret.** REST task output is stored in a zip that downloads without authentication. The registration token is never echoed, logged to stdout, or included in `NVOKE-RESULT`.
- **No `%` or `"` in argument values.** On the REST path the command line passes through `cmd.exe`, which expands `%VAR%`.

## Launcher

1. Check PowerShell 5.1 or newer and the inputs. On failure, set `Result=failed` and `ErrorCode`, and skip to the end (no `exit`; `SetShouldExit` as the last statement).
2. Create the folder with the restricted ACL.
3. Write `installAgent.ps1` (embedded in the launcher) and check its SHA-256 against the value baked in at packaging time. Replace it if it differs.
4. Write `job.json`: destination server, customer ID, registration token, migration ID, launcher version, repeat interval, expiry. The registration token goes **in this file, not on the scheduled task's command line**, so it doesn't appear in the task definition or process list.
5. Register or replace the scheduled task `N-voke Agent Migration`: runs as SYSTEM at highest privileges, first run in 2 minutes, repeats at `TaskRepeatMinutes`, ends after `TaskExpiryDays`, a 2-hour limit per run, and only one instance at a time.
6. Report: outputs `Result=staged`, `TaskCreated=true`, `LauncherVersion`, `StagedAt`. On the REST path the same values are printed as a single `NVOKE-RESULT {json}` line that the tracker parses.

The launcher never runs the installer itself. It does as little as possible (no WMI/CIM queries, no inventory) so it finishes quickly even on a starved machine; the Ecoverse run is still given a generous `timeoutSeconds` (15 minutes), because a slow device can take minutes just to start PowerShell.

## Slow and low-resource endpoints

The lab VMs are deliberately under-provisioned, and real fleets include old, overloaded machines on poor links. The scripts assume the device is slow:

- **Download once, resume, reuse.** `WindowsAgentSetup.exe` is about 140 MB. It's downloaded with BITS (resumable, bandwidth-friendly), falling back to `Invoke-WebRequest` with `$ProgressPreference = 'SilentlyContinue'` (the progress bar alone makes 5.1 downloads many times slower). A completed download is kept across attempts and reused if its size and Authenticode signature check out.
- **Never start a second install over a running one.** Before installing, the installer checks for a running `WindowsAgentSetup.exe` or agent `msiexec` from an earlier attempt and, if one is found, records `install-in-progress` and leaves it alone.
- **Give the new agent time.** After an install, the agent service can take many minutes to start and register on a slow machine. The installer records `installed-awaiting-health` and only reinstalls when the device has been unhealthy for **60 minutes** after a successful install (configurable), so a slow start never turns into a reinstall loop.
- **Windows Update and other installers come first.** If Windows Installer is busy (exit code 1618, "another installation is already in progress") or a restart is pending (Windows Update or Component Based Servicing reboot flags), the installer records `waiting-on-windows` with the reason and leaves the attempt to the next scheduled run. This doesn't count as a failed install, and the tracker shows it as *waiting on Windows Update* rather than counting toward **Lost**.
- **A generous per-run limit.** The scheduled task's per-run limit is **2 hours**, with one instance at a time, so a slow install isn't killed halfway through.
- **Waves limit concurrent downloads.** Each device pulls the installer from the destination server; the planner caps how many devices in a wave can be in the download/install window at once.

## Installer (`installAgent.ps1`)

Changes from the PowerShell project's version:

| Today | New |
|---|---|
| Takes the destination API JWT and calls the SOAP API to find a registration token | Takes the registration token from `job.json`; makes **no API calls** |
| Returns 0 even when the installer fails | Exits with the installer's code and records it |
| Logs before logging is set up are lost | Logging starts first |
| Runs forever until a separate cleanup AMP removes it | Removes its own task once healthy; the task also has an expiry |
| Health check: product installed, service running, `ServerConfig.xml` server matches | Same checks, plus "has checked in since this install" when it can tell |

Each run:

1. Read `job.json` and `status.json`; increment `attempts`.
2. **Healthy check.** If the Windows Agent is installed, its service is running and `ServerConfig.xml` points at `DestServer`, check the UFA: a UFA installed alongside a new agent starts before that agent registers and doesn't provision until it's restarted ([findings](10-phase0-findings.md)). If `MspAgent` exists but hasn't provisioned since our install (no `Status: 200` X.509 fetch in its `provision.log`), restart it once, wait up to 5 minutes for provisioning, and otherwise set `stage = awaiting-ufa` and check again next run (giving up with a warning after 60 minutes). Then set `stage = healthy` and `healthyAt`. Unregister the scheduled task, delete `job.json` (removing the token from disk), and exit 0. Keep `status.json` and the logs for the cleanup script to collect.
3. **Install.** Download `WindowsAgentSetup.exe` from `DestServer` and record the HTTP outcome. Then set `stage = installing` and run:
   `WindowsAgentSetup.exe /s /v" /qn SERVERADDRESS=… CUSTOMERID=… CUSTOMERSPECIFIC=1 REGISTRATION_TOKEN=… SERVERPROTOCOL=HTTPS"`
4. Record the installer's exit code. Set `stage = installed-awaiting-health` or `stage = install-failed` with `lastError`, and exit with that code.

Downloading before installing matters: if the destination can't be reached, the old agent is never removed.

### Full cleanup (`CleanMode=full`, the default)

An in-place upgrade (destination agent newer than the source's) only re-points the Windows Agent: the UFA (MSP Core Agent) and the other components keep their source configuration, so the device stays in the **source** Ecoverse tenant ([Phase 0 findings](10-phase0-findings.md#first-migration-test-010-2026-10-19)). So after the download and signature check succeed, and before the install, the installer removes every N-central component once per job:

| Step | What | How |
|---|---|---|
| 1 | Windows Agent, MSP Core Agent (UFA) | `msiexec /x {product code} /qn REBOOT=ReallySuppress`, product code read from the uninstall entry (never hard-coded) |
| 2 | Patch Management Service Controller, Request Handler Agent, File Cache Service Agent | Their Inno Setup uninstallers with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, then wait until the uninstall entry is gone (these uninstallers exit before they finish) |
| 3 | Take Control (no uninstall entry) | Remove the *Take Control Agent N-Central Helper* task; stop and delete any service whose executable lives in an N-able folder |
| 4 | Leftover processes | Stop any process running from an N-able folder |
| 5 | Folders | `Program Files (x86)\{N-able Technologies, Msp Agent, MspPlatform, BeAnywhere Support Express}`; `ProgramData\{N-able Technologies, MspAgent, MspPlatform, GetSupportService_N-Central, GetSupportService_Common_N-Central}`; SYSTEM's `MSPScriptExecution` |
| 6 | Registry | `HKLM\SOFTWARE\N-able Technologies` (64-bit and `WOW6432Node`) |

Products are matched by exact display name and paths are a fixed list; nothing is removed by wildcard. In practice the Windows Agent's own uninstall already removes the companions (the others then return 1605, "not installed"). Take Control's credential provider DLL stays locked by the logon process and is deliberately left until the next reboot: scheduling it for deletion could remove same-named files of the newly installed Take Control. Each uninstall has a 20-minute limit. Every action and its result goes into `status.json`. The new agent then installs into a clean machine, and the destination deploys its own UFA and components. `CleanMode=none` skips all of this (plain install over the top).

## `status.json`

```json
{
  "schema": 1,
  "migrationId": "mig_7f3c/wave-2",
  "launcherVersion": "1.0.0+3f9a2c1",
  "destServer": "new.example.com",
  "destCustomerId": "1234",
  "stage": "installed-awaiting-health",
  "attempts": 2,
  "stagedAt": "2026-10-20T14:02:11Z",
  "firstAttemptAt": "2026-10-20T14:04:00Z",
  "lastAttemptAt": "2026-10-20T14:19:00Z",
  "installStartedAt": "2026-10-20T14:19:05Z",
  "installerExitCode": 0,
  "healthyAt": null,
  "lastError": null,
  "history": [
    { "at": "2026-10-20T14:04:00Z", "event": "download-failed", "detail": "The remote name could not be resolved" },
    { "at": "2026-10-20T14:19:05Z", "event": "install-started" },
    { "at": "2026-10-20T14:21:40Z", "event": "install-finished", "detail": "exit 0" }
  ]
}
```

It never contains the registration token. `history` is capped at the last 50 entries.

## Status probe

Read-only. It returns `status.json`, the scheduled task's state, last run time and last result, and the tail of the newest log. Ecoverse returns these as output variables (`StatusJson`, `TaskState`, `TaskLastResult`); the REST path prints them as one `NVOKE-RESULT {json}` line. It's used for devices stuck at **Staged** while the old agent is still online.

## Cleanup

It runs on the destination, through whichever agent is connected there:

1. Return `status.json` and the log tail (same output shape as the status probe).
2. Unregister the scheduled task if it still exists.
3. Delete `C:\ProgramData\N-voke\Migration`.
4. Return `Cleaned=true`, or what couldn't be removed.

## Error codes

Reported in the `ErrorCode` output (Ecoverse) or the `NVOKE-RESULT` line (REST). The installer, which runs from the scheduled task and not through Ecoverse, also uses them as its process exit code.

| Code | Meaning |
|---|---|
| 0 | Success |
| 1001 | Destination not reachable / download failed |
| 1003 | Installer failed (the installer's own code is in `status.json`) |
| 1004 | Validation failed (PowerShell version, inputs) |
| 1005 | Permission problem (folder, ACL, scheduled task) |
| 1007 | Configuration missing (`job.json` absent or invalid) |

## Testing the scripts

- Pester tests for the pure parts: input validation, `status.json` read and write, health-check decisions, ACL creation (on a temp folder).
- PSScriptAnalyzer in CI.
- A lab VM for full install runs against a test destination server, both paths.
