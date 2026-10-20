# 5. Endpoint scripts

Four roles run on devices. The working Phase 0 versions are in [`scripts/migration/endpoint/`](../../scripts/migration/README.md). The launcher, status probe and cleanup are one script, `nvoke-migration.ps1`, selected by its `Action` input (`Launch`, `Status`, `Cleanup`), so the REST path needs only one manual upload per server. The installer is embedded in it. Everything is embedded in N-voke, versioned by content hash and packaged as described in [Delivery paths](04-delivery-paths.md#one-script-two-packages). None of it contains a credential.

| Role | Runs through | When | Job |
|---|---|---|---|
| **Launcher** (`Action=Launch`) | Old agent (UFA or Windows Agent) | Launch | Stage files, write the job file, register the scheduled task, report back |
| **Installer** (`installAgent.ps1`) | The scheduled task, as SYSTEM (64-bit PowerShell) | Every 15 minutes until done | Check health; if not healthy on the destination, download, clean up the old agent, install; record progress |
| **Status probe** (`Action=Status`) | Whichever agent the device has now (the old one before the cleanup, the new one after) | On demand | Return the status file, task state and log tail |
| **Cleanup** (`Action=Cleanup`) | New agent (UFA or Windows Agent) on the destination | Optional, any time after healthy | Return the status file, remove the tasks and the folder before the review period ends |

A successful device doesn't need `Cleanup`: after the review period a one-time task on the device removes the folder (below). `Cleanup` is for collecting the record early, or for tidying up sooner; on the REST path it runs on the **destination** server, so the file has to be uploaded there too.

## Files on the device

```
C:\ProgramData\N-voke\Migration\           ACL: SYSTEM and Administrators only (no inheritance)
  job.json          what to install (written by the launcher)
  installAgent.ps1  the installer script
  status.json       progress record (written by the installer)
  logs\install-<timestamp>.log  (last 10 kept)
  logs\msi-<timestamp>.log      Windows Installer verbose log of the install
  logs\uninstall-<product>.log  verbose logs of the cleanup's MSI uninstalls
  WindowsAgentSetup.exe          (kept and reused across attempts if its signature is valid; deleted once healthy)
```

This replaces `C:\NableMigrationTemp`, which the PowerShell project opened to **Everyone: Full Control**. That let any local user edit a script that runs as SYSTEM, and read the API key embedded in it. The new folder is created with an explicit ACL and inheritance disabled.

## Inputs

The same names are used on both paths. The packager declares them as Ecoverse script inputs (every one must be sent on every run); on the REST path they are `-Name value` arguments in the direct task's single `CommandLine` parameter (`-Action Launch -DestServer … -RegistrationToken …`), read from `$args`.

| Input | Type | Example | Notes |
|---|---|---|---|
| `Action` | STRING | `Launch` | `Launch`, `Status` or `Cleanup` |
| `DestServer` | STRING | `new.example.com` | Validated as a host name, with no scheme, path or port |
| `DestCustomerId` | STRING | `1234` | Numeric N-central customer or site ID on the destination (a site ID registers the device directly under the site) |
| `RegistrationToken` | PASSWORD | | Must match `^[A-Za-z0-9-]{8,128}$`. Redacted in N-voke's audit log and dry-run view, and hidden in the Ecoverse UI, but written in plain text into Ecoverse's temporary copy of the script while it runs |
| `MigrationId` | STRING | project and wave ID | Written into `status.json` so results can be tied back |
| `LauncherVersion` | STRING | `1.0.0+3f9a2c1` | Lets the installer and tracker detect mixed versions |
| `TaskRepeatMinutes` | INT | `15` | Optional |
| `TaskExpiryDays` | INT | `14` | Optional; the task deletes itself after this |
| `CleanMode` | STRING | `full` | `full` (default): remove every N-central component before installing ([Full cleanup](#full-cleanup-cleanmodefull-the-default)); `none`: install over the top |
| `ExpectUfa` | STRING | `false` | `true` only when the destination is linked to Ecoverse (per project). With `false` the installer doesn't restart or wait for the UFA |
| `ReviewDays` | INT | `7` | How long `status.json` and the logs stay on the device after it's healthy, for review, before a one-time task removes them. `0` removes them immediately |

Outputs (all STRING, always one non-blank line): `Result` (`staged`, `ok`, `cleaned`, `failed`), `ErrorCode`, `Message`, `ScriptVersion`, `TaskCreated`, `StagedAt`, `StatusJson` (compact JSON, last 30 history entries), `TaskState`, `Cleaned`. The REST path prints the same fields as one `NVOKE-RESULT {json}` line.

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
4. Read the agent's current server from its `ServerConfig.xml`: if it's already `DestServer`, the job is a same-server **`reinstall`** (health then counts only after an install the installer ran itself); otherwise a **`move`**. Write `job.json`: mode, source and destination server, customer ID, clean mode, registration token, migration ID, launcher version, repeat interval, expiry. The registration token goes **in this file, not on the scheduled task's command line**, so it doesn't appear in the task definition or process list.
5. Register or replace the scheduled task `N-voke Agent Migration`: runs `System32\WindowsPowerShell\v1.0\powershell.exe` (64-bit, even when the launcher itself is 32-bit) as SYSTEM at highest privileges; first run in 2 minutes, repeats at `TaskRepeatMinutes`, also runs 2 minutes after every boot, ends after `TaskExpiryDays`, 2-hour limit per run, one instance at a time, starts late if a run was missed.
6. Report: outputs `Result=staged`, `TaskCreated=true`, `ScriptVersion`, `StagedAt`, `TaskState` and the initial `StatusJson`. On the REST path the same values are printed as a single `NVOKE-RESULT {json}` line that the tracker parses.

The launcher never runs the installer itself. It does as little as possible (no WMI/CIM queries, no inventory) so it finishes quickly even on a starved machine; the Ecoverse run is still given a generous `timeoutSeconds` (15 minutes), because a slow device can take minutes just to start PowerShell.

## Slow and low-resource endpoints

The lab VMs are deliberately under-provisioned, and real fleets include old, overloaded machines on poor links. The scripts assume the device is slow:

- **Download once, resume, reuse.** `WindowsAgentSetup.exe` is about 140 MB. It's downloaded with BITS (resumable, bandwidth-friendly), falling back to `Invoke-WebRequest` with `$ProgressPreference = 'SilentlyContinue'` (the progress bar alone makes 5.1 downloads many times slower). A completed download is kept across attempts and reused if its size and Authenticode signature check out.
- **Never start a second install over a running one.** Before installing, the installer checks for a running `WindowsAgentSetup.exe` or agent `msiexec` from an earlier attempt and, if one is found, records `install-in-progress` and leaves it alone.
- **Give the new agent time.** After an install, the agent service can take many minutes to start and register on a slow machine. The installer records `installed-awaiting-health` and only reinstalls when the device has been unhealthy for **60 minutes** after a successful install (configurable), so a slow start never turns into a reinstall loop.
- **Windows Update and other installers come first.** If Windows Installer is busy (its `_MSIExecute` mutex is held, or the installer returns 1618, "another installation is already in progress"), the installer records `waiting-on-windows` and leaves the attempt to the next scheduled run. This doesn't count as a failed install, and the tracker shows it as *waiting on Windows Update* rather than counting toward **Lost**. A pending restart (Windows Update or Component Based Servicing flags) is only recorded (`rebootPending`), not waited on: servers can carry one for weeks. A device can also come back from a restart into a long Windows Update (about 10 minutes in the lab) before anything runs.
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

1. Read `job.json` and `status.json`; increment `attempts`; record the agent's state (installed, version, service running, server, UFA running).
2. **Healthy check.** If the Windows Agent is installed, its service is running and `ServerConfig.xml` points at `DestServer`, check the UFA, **only when `ExpectUfa=true`**: a UFA installed alongside a new agent starts before that agent registers and doesn't provision until it's restarted ([findings](10-phase0-findings.md)). If `MspAgent` exists but hasn't provisioned since our install (no `Status: 200` X.509 fetch in its `provision.log`), restart it once, wait up to 5 minutes for provisioning, and otherwise set `stage = awaiting-ufa` and check again next run (giving up with a warning after 60 minutes). Then set `stage = healthy` and `healthyAt`. Delete `job.json` (removing the token from disk) and the downloaded installer, register the one-time **N-voke Migration Cleanup** task for `healthyAt + ReviewDays` (it deletes `C:\ProgramData\N-voke\Migration` and itself; it runs late if the device is off that day), record `reviewUntil`, unregister the migration task, and exit 0. During the review period `status.json` and the logs can be read on the device or with `Status`. With `ReviewDays=0` the folder is removed at once.
3. **Wait if appropriate.** Within 60 minutes of our own successful install (`installed-awaiting-health`), just record and exit. If an earlier `WindowsAgentSetup.exe` is still running (`install-in-progress`) or Windows Installer is busy (`waiting-on-windows`), record and exit.
4. **Download** `WindowsAgentSetup.exe` from `DestServer` (BITS, then `Invoke-WebRequest`), check its Authenticode signature, and record method, size and time (`download-failed`, exit 1001, if it can't).
5. **Full cleanup** (unless `CleanMode=none`; once per job): see below. Nothing before this step changes the device's agents.
6. **Install:** set `stage = installing` and run
   `WindowsAgentSetup.exe /s /v" /qn SERVERADDRESS=… CUSTOMERID=… CUSTOMERSPECIFIC=1 REGISTRATION_TOKEN=… SERVERPROTOCOL=HTTPS /L*V <msi log>"`
7. Exit code 0 or 3010: `stage = installed-awaiting-health`, `installedAt`. 1618: `waiting-on-windows`. Anything else: `stage = install-failed` with `lastError`, exit 1003.

### UFA watchdog (planned)

Observed in the lab: a UFA whose core log shows communication errors (timeouts) doesn't sync its data to Ecoverse, so the asset sits `CONNECTED` with no inventory, and **restarting the `MspAgent` service fixes it** ([findings](10-phase0-findings.md#known-delays-2026-10-20)). The installer builds this in, because the device is the one place that can read the log:

- **When:** only when `ExpectUfa=true`, the `MspAgent` service exists, and it has provisioned. At a set time after our install (planned default 20 minutes, an input) and again once later, so a late fault is caught too.
- **What it reads:** the tail of `Program Files (x86)\Msp Agent\logs\msp-agent-core.log`, only lines written since our install, for timeout and communication errors. Lines that are normal on every non-cloud machine must be excluded: the cloud-instance metadata lookup (`169.254.169.254`) always times out on a physical or VM host and says nothing about connectivity.
- **What it does:** if errors are found, `Restart-Service MspAgent`, record `ufa-restarted` in `status.json` with the matched error (no secrets), and check again after a set time. At most two restarts per job, so a real outage can't cause a restart loop.
- **How it runs:** the migration task is gone once the device is healthy, so the check is a separate one-time scheduled task (like the review task) that removes itself, and only touches the service if the log shows errors.
- **Reported:** `status.json` records `ufaWatchdog` (checked at, found, restarted) so the tracker and the report show what happened.

Device-side stages in `status.json`: `staged`, `downloading`, `download-failed`, `cleaning`, `installing`, `installed-awaiting-health`, `install-failed`, `install-in-progress`, `waiting-on-windows`, `awaiting-ufa`, `healthy`, `error`.

Downloading before installing matters: if the destination can't be reached, the old agent is never removed.

### Full cleanup (`CleanMode=full`, the default)

An in-place upgrade (destination agent newer than the source's) only re-points the Windows Agent: the UFA (MSP Core Agent) and the other components keep their source configuration, so the device stays in the **source** Ecoverse tenant ([Phase 0 findings](10-phase0-findings.md#first-migration-test-010-2026-10-19)). So after the download and signature check succeed, and before the install, the installer removes every N-central component once per job:

| Step | What | How |
|---|---|---|
| 1 | Windows Agent, MSP Core Agent (UFA) | `msiexec /x {product code} /qn REBOOT=ReallySuppress`, product code read from the uninstall entry (never hard-coded) |
| 2 | Patch Management Service Controller, Request Handler Agent, File Cache Service Agent | Their Inno Setup uninstallers with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, then wait until the uninstall entry is gone (these uninstallers exit before they finish) |
| 3 | Leftover services | Stop and delete any service whose executable lives in an N-able folder |
| 4 | Leftover processes | Stop any process running from an N-able folder |
| 5 | Folders | `Program Files (x86)\{N-able Technologies, Msp Agent, MspPlatform}`; `ProgramData\{N-able Technologies, MspAgent, MspPlatform}`; SYSTEM's `MSPScriptExecution` |
| 6 | Registry | `HKLM\SOFTWARE\N-able Technologies` (64-bit and `WOW6432Node`) |

Products are matched by exact display name and paths are a fixed list; nothing is removed by wildcard. In practice the Windows Agent's own uninstall already removes the companions (the others then return 1605, "not installed"). Take Control isn't touched: removing it was never a requirement, so its task, services and folders are left for the new agent to reuse or replace. Each uninstall has a 20-minute limit. Every action and its result goes into `status.json`. The new agent then installs into a clean machine, and the destination deploys its own UFA and components. `CleanMode=none` skips all of this (plain install over the top).

## `status.json`

Shape as written by the Phase 0 scripts (a real run, shortened):

```json
{
  "schema": 1,
  "migrationId": "mig_7f3c/wave-2",
  "launcherVersion": "0.1.0+3db4bba",
  "mode": "move",
  "sourceServer": "old.example.com",
  "destServer": "new.example.com",
  "destCustomerId": "1234",
  "stage": "healthy",
  "stagedAt": "2026-10-19T23:29:33Z",
  "attempts": 2,
  "agent": { "installed": true, "version": "2026.4.1025", "serviceRunning": true, "server": "new.example.com", "ufaRunning": true },
  "ufa": { "present": true, "running": true, "provisioned": true },
  "rebootPending": false,
  "cleanedAt": "2026-10-19T23:34:33Z",
  "installedAt": "2026-10-19T23:35:24Z",
  "lastInstallExitCode": 0,
  "healthyAt": "2026-10-19T23:46:40Z",
  "updatedAt": "2026-10-19T23:46:40Z",
  "history": [
    { "at": "2026-10-19T23:29:33Z", "event": "staged", "detail": "mode move, clean full, from old.example.com" },
    { "at": "2026-10-19T23:31:47Z", "event": "downloaded", "detail": "BITS, 133.1 MB in 9 s, signed by CN=N-ABLE TECHNOLOGIES LTD, …" },
    { "at": "2026-10-19T23:33:36Z", "event": "cleanup-uninstall", "detail": "Windows Agent -> 0" },
    { "at": "2026-10-19T23:33:37Z", "event": "cleanup-uninstall", "detail": "MSP Core Agent -> 1605" },
    { "at": "2026-10-19T23:34:33Z", "event": "install-started", "detail": "reboot pending: False" },
    { "at": "2026-10-19T23:35:24Z", "event": "install-finished", "detail": "exit 0" },
    { "at": "2026-10-19T23:46:40Z", "event": "healthy", "detail": "agent 2026.4.1025 on new.example.com; UFA present True, provisioned True" }
  ]
}
```

Other fields as they occur: `lastError`, `warning` (for example the UFA not provisioning), `ufaRestartedAt`. Written atomically (temporary file, then rename).

It never contains the registration token. `history` is capped at the last 50 entries; `StatusJson` returns the last 30.

## Status probe

Read-only. It returns `status.json` (`StatusJson`), the scheduled task's state, last run, last result and next run (`TaskState`, or `absent`), and the last 15 lines of the newest install log (`Message`). The REST path prints the same as one `NVOKE-RESULT {json}` line. Before the cleanup it runs through the old agent (diagnosing **Stuck staged**); after the move, through the new agent on the destination. Between the two (cleanup to new UFA provisioned, typically 10–15 minutes) nothing can reach the device.

## Cleanup

It runs on the destination, through whichever agent is connected there:

1. Return `status.json` and the log tail (same output shape as the status probe).
2. Unregister the migration task and the review-period cleanup task if they still exist.
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
- A lab VM for full install runs against a test destination server, both paths. The Phase 0 harness in [`scripts/migration/lab/`](../../scripts/migration/README.md) drives this.
- Before publishing: both files must be ASCII (Windows PowerShell 5.1 reads BOM-less files as ANSI), parse cleanly on their own and in the "Ecoverse form" (input assignments prepended, output `echo` lines appended), and a failed validation must produce one `NVOKE-RESULT` line and nothing on stderr. `build.mjs` and the harness check this.
- **MSI log and the token.** The verbose MSI log (`/L*V`) normally records the properties it was started with, which would include `REGISTRATION_TOKEN`. Rather than depend on that being checked per Windows Installer version, the installer scrubs every `*.log` in its folder (`Hide-RegistrationToken`): right after each install attempt, and again at the start of every run in case an earlier run was killed first. Each occurrence becomes `[token removed]`; ANSI and UTF-16 logs are both handled and all other bytes are left alone. While the install runs the log sits in the restricted folder. ❓ Still to confirm on a lab VM: whether a log does contain the token before scrubbing (it makes no difference to the result), and whether it can be avoided at the source with `MsiHiddenProperties`.
- **Checks that run in CI** (`scripts/migration/endpoint/run-tests.ps1`, on Windows PowerShell 5.1 and pwsh): PSScriptAnalyzer clean; the built script is ASCII, parses, has no `param()` block and no `exit`, and carries its version and the installer's SHA-256; every launch validation failure gives one `NVOKE-RESULT` line, error 1004 and nothing on stderr, in both the REST form (`-Name value`) and the Ecoverse form (variables above the script); the token never appears in output; `Get-OneLine`, `Get-StatusSummary`, the restricted ACL and the log scrub are unit tested.
