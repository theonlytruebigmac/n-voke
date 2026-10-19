# 10. Phase 0 findings

Results of the [proof of concept](09-roadmap.md#phase-0-proof-of-concept), recorded as each test runs. The [API reference](03-api-reference.md) statuses are updated to match. The summary below is the short version; the sections after it are the evidence, in the order the tests ran.

## Summary: how a migration actually works

Proven end to end on 2026-10-19 (VM-C, [test 0.10b](#rerun-with-full-cleanup-test-010b-2026-10-19)): launch to healthy in about 18 minutes on a 2 GB VM, both agents on the destination, cleaned up.

```mermaid
sequenceDiagram
    participant N as N-voke
    participant SE as Source Ecoverse tenant
    participant D as Device
    participant DN as Destination N-central
    participant DE as Destination Ecoverse tenant
    N->>DN: Generate registration token if missing (POST …/software/installers)
    N->>SE: assetsScriptRun nvoke-migration.ps1, Action=Launch (all inputs, token as PASSWORD)
    SE->>D: UFA runs launcher (32-bit PowerShell, child of the agent)
    D->>D: Folder (SYSTEM/Admins), job.json, scheduled task (64-bit), outputs Result=staged
    D->>DN: Task run 1: download installer (BITS), check signature
    D->>D: Full cleanup: uninstall all N-able products, delete folders and registry keys
    D->>DN: Install Windows Agent, register in mapped site
    D->>DE: Fresh UFA provisions with the destination (restart if needed): new asset
    D->>D: Task run 2: healthy (agent + UFA), task and job.json removed
    N->>DE: Action=Cleanup on the new asset: status.json collected, folder removed
```

**Rules learned the hard way** (each links to its evidence below):

| Area | Rule |
|---|---|
| Why the handoff | Scripts run through either agent are children of it (`powershell` ← `msp-ufa-script-execution` ← `msp-agent-core`; `powershell` ← `cmd` ← `agent.exe`). The install must run from a scheduled task |
| Why full cleanup | On an in-place **upgrade** the agent installer re-points the Windows Agent but deliberately leaves the UFA (and other components) enrolled with the source, so the device stays in the source Ecoverse tenant. A normal uninstall also leaves folders that hold the UFA's identity; a UFA reinstalled over them fails with `400 The SSL certificate error`. N-voke removes everything itself, once the new installer has downloaded |
| Ecoverse scripts | Inputs are prepended as typed variable assignments (so no `param()` block); every declared input must be sent; outputs are read from variables after the script ends, so never `exit`, never a blank or multi-line output; `exitCode` is always null, `$host.SetShouldExit(n)` gives `FAILED`; runs in 32-bit PowerShell |
| REST scripts | Plain `.ps1` uploaded by hand (no AMPs, no SOAP); a new version means delete and re-upload (new item ID, and the old item's task history is deleted); inputs in one `CommandLine` (visible in N-central's UI even as `password`); any stderr hides stdout; full output and exit code only in an **unauthenticated** zip; `/status` hides failures; 64-bit PowerShell |
| Delivery | Ecoverse pickup is usually seconds but a single task can stall for 30+ minutes: cancel and relaunch. Offline devices need `shouldRunWhenOnline` (refused otherwise); a queued task outlives its timeout. REST direct tasks wait for offline devices (`Pending`) |
| Identity | The Ecoverse asset follows the N-central device record; a reinstall gets a **new** device ID and a **new** asset ID. A new asset can't run scripts for a few minutes and has no inventory (no serial) at first. Match old to new by serial and MAC |
| Signals | Ecoverse notices a disconnect after ~3.5 minutes; N-central's last check-in time drifts and jumps around a move. Trust the destination appearing, not the source going quiet |
| Tokens | A new destination has none; `POST /api/customers/{id}/software/installers` creates one (customer or site) and leaves a valid one unchanged |
| API limits | GraphQL: 130 fields, 15 levels, 5 aliases per operation (task polls at most 4 per request); retry 429, 503 and `INTERNAL_SERVER_ERROR` for reads only |
| Weak endpoints | 2 GB VMs at 90% memory installed in 1–2 minutes; delays come from delivery and provisioning, not the install itself |

The working endpoint scripts (`nvoke-migration.ps1` with the embedded `installAgent.ps1`, build 0.1.0+3db4bba) and the test harness are in [`scripts/migration/`](../../scripts/migration/README.md); they're the starting point for [Phase 1](09-roadmap.md#phase-1-endpoint-scripts). Lab hosts and IDs are kept out of the repository (`lab.config.json` is git-ignored).

## Lab

| | Source | Destination |
|---|---|---|
| N-central | `source.example.com`, 2026.4.0.25 | `destination.example.com`, 2026.4.1.11 |
| REST API version (`/api/server-info`) | 1.3.23 | 1.3.23 |
| Service organization | N-voke Demo Source | N-voke Demo Destination |
| Customers / sites | 3 / 6 | 3 / 6, same names |
| Ecoverse tenant | Own tenant (its own service org ID) | **Separate** tenant (a different service org ID) |
| Windows devices | VM-A, VM-B, VM-C (Windows 11 Pro VMs), one per customer | None yet |

Both Ecoverse tenants have a partner named **a lab partner name**, with different IDs. Identify tenants by ID, never by name.

## Read-only pass (2026-10-19)

A throwaway probe exercised every read the feature needs, on both servers and both tenants. It changed nothing. (One write followed: generating two registration tokens, see [Registration tokens](#registration-tokens).)

### N-central REST

| Finding | Detail | Effect on the plan |
|---|---|---|
| Authentication works as documented | `POST /api/auth/authenticate` with the API-user JWT: access token 3,600 s, refresh token 90,000 s (25 h) | None |
| `GET /api/server-info` needs a bearer token | Returns 401 without one. Body: `{ ncentral: "2026.4.0.25", version: "1.3.23", jvmVersion, startTime }` | Version check runs after authentication. SOAP `versionInfoGet` gives the version without credentials |
| `POST /api/server-info/extra/authenticated` needs a `DmsCredential` body | 400 with an empty body | Not needed; `/api/server-info` is enough |
| Org tree | `GET /api/org-units/{id}/children` returns one level: `orgUnitId` (string), `orgUnitName`, `orgUnitType` (`SO`, `CUSTOMER`, `SITE`), `parentId`, `externalId`, `externalId2`, `descendantCount` | Name matching stays primary. `externalId` is optional and usually blank (the lab's values were added by hand), so it's only a confirming signal when both sides have the same value |
| Registration token, none set | `200 { data: { registrationToken: null, registrationTokenExpiryDate: null } }`, not 404 | Planner treats null as "missing" |
| Registration token, set | `registrationTokenExpiryDate` such as `2026-11-17T23:59:00-05:00[America/New_York]`: ISO 8601 with a Java zone suffix | Parser strips the `[zone]` suffix |
| Registration token on a service org | `403 RegistrationTokenNotAccessibleException` | Only customers and sites are token targets |
| `/customers/{id}/…`, `/sites/{id}/…` and `/org-units/{id}/registration-token` | Return the same data | Use `/org-units/{id}/…` for both |
| **Fresh destination has no tokens** | Every destination customer and site returned null. On the source, only the three sites where an agent had been installed had one | Expected for a new server. N-voke generates them; see [Registration tokens](#registration-tokens) |
| Device list | `GET /api/org-units/{soId}/devices` includes descendants. Fields include `deviceId`, `longName`, `discoveredName`, `deviceClass`, `supportedOs`, `osId` (`winnt`), `customerId`, `siteId`, `orgUnitId`, `soId`, `lastApplianceCheckinTime`, `applianceId` | Enough for planning and the Agent removed check |
| Device assets | `GET /api/devices/{id}/assets`: serial `computersystem.serialnumber`, host name `computersystem.netbiosname`, MAC `networkadapter.list[].macaddress`, SMBIOS UUID `_extra.computersystem.uuid` | Matching rules confirmed on the REST side; the UUID is a further strong key |
| Custom properties | `GET /api/devices/{id}/custom-properties` returns `propertyId`, `propertyName`, `propertyType`, `value` | As planned |

### N-central SOAP (no longer used)

Login with an empty username and the JWT works (`customerList`, `deviceList`), and `versionInfoGet` needs no credentials. `repositoryItemList` faults with `2100 … null` on both servers. SOAP was only needed for AMPs, which are dropped (see [REST path](#rest-path-no-amps)), so this isn't pursued.

### Ecoverse GraphQL

| Finding | Detail | Effect |
|---|---|---|
| `ncentralDevice { deviceId server { id origin } }` | Works; `origin` is `https://source.example.com:443` (with the port) | Compare server origins without the port |
| `systemInfo` | `AssetSystemInfo { hostname serialNumber manufacturer model }`. It has **no** `macAddress`, `uuid` or `domain` | Matching fields confirmed |
| `networkInterfaces { macAddress }` | Works; includes empty strings for some adapters, and was `null` for one device on the first call (inventory not yet reported) | Ignore empty MACs; fall back to REST assets when null |
| Serial numbers agree | Ecoverse `systemInfo.serialNumber` equals REST `computersystem.serialnumber` | Serial plus MAC is usable across both APIs |
| `operatingSystem.name` can be null | Seen on one connected device (VM-A); filled in later, so inventory lag | Use `operatingSystemInfo { name architecture type }` (`type: WINDOWS`, `architecture: 64-bit`), falling back to REST `supportedOs` / `osId` |
| Not on `Asset` | `hostname`, `serialNumber`, `hardware`, `createdAt`, `updatedAt`, `lastSeenAt`, `ipAddress`, `deviceType` | — |
| Introspection | Disabled, but validation errors name the missing field and type, which is enough to probe | — |
| Tenants | Each token sees only its own tenant | See [Two Ecoverse tenants](#two-ecoverse-tenants) |

## Consequences for the design

### Registration tokens

A new destination has no registration tokens, so N-voke must be able to create them. **Confirmed (write test, 2026-10-19):** `POST /api/customers/{id}/software/installers` with body `{ "softwareId": "101" }` (the Windows Agent, from `GET …/software/installers?softwareType=Agent&installerType=Generic`) returned `201 { downloadLink, expirationDate }` and created a registration token for:

- a customer (Demo - Acme Corporation), and
- a site (Demo - Acme HQ): the endpoint accepts a site ID too.

Both tokens are 36 characters and expire `2026-11-17T23:59:00-05:00`, matching the source's tokens, so the lifetime appears to be a server setting. The source's older tokens are 17 characters; the installer accepts either.

The `downloadLink` is a customer-specific installer (`/download/resources/software-installers/<uuid>/<customerId>WindowsAgentSetup_VALID_UNTIL_<date>.exe`) that downloads **without authentication** and expires in 3 days. N-voke doesn't use it: the generic installer plus `REGISTRATION_TOKEN` keeps working for the scheduled task's full retry window, and an unauthenticated link isn't something to hand to endpoints.

**Calling it again doesn't change a valid token** (test 0.9b): for that customer and site the token and its expiry were identical before and after a second call; only a new download link was made. So it's safe to call before every wave, but it can't extend a token that's about to expire. Whether it replaces an **expired** token is still open (the earliest lab tokens expire 2026-11-17).

### Two Ecoverse tenants

Today the destination tenant has **no devices**, which is expected: nothing has been migrated yet, and the only N-central with agents is the source (`source.example.com`). The destination tenant already shows the destination's service org, customers and sites, because `destination.example.com` is linked to it.

This matters **after** a device moves. Its new agent registers with `destination.example.com`, so the device shows up in the destination tenant, not the source tenant. In this lab that means:

- The launcher is published and run in the **source** tenant (where the devices are now).
- Seeing the new device through Ecoverse (Registered, Healthy) and running the cleanup through Ecoverse need the **destination** tenant's key.
- Without that key, the destination side falls back to REST, which can always see the new device.

So credentials hold one Ecoverse key per tenant. When one tenant covers both servers (the common case for a same-server reinstall or a partner with one tenant), one key serves both.

### REST path: no AMPs

Ecoverse scripting doesn't run AMPs, and a REST direct task can run a repository **Script** (`taskType: "Script"`, inputs in one `CommandLine` parameter). The REST API has no upload or listing endpoint for repository items, so:

- AMPs and the SOAP client are dropped.
- The REST path uses the same PowerShell file, uploaded by hand once per server with **Enable API** on; the operator enters its item ID.
- The script reports its own version so N-voke can tell when an upload is stale.

## Ecoverse script runs (tests 0.1, 0.2, 0.4; 2026-10-19)

A throwaway script, **N-voke Phase0 input probe**, was published in the source tenant (organization N-voke Demo Source). It has STRING, PASSWORD, INT and BOOLEAN inputs and five STRING/BOOLEAN outputs set in different ways. It ran on VM-C (exit 0) and VM-B (exit 7) at 15:08 and finished in about 30 seconds.

### Publishing

| Finding | Detail | Effect |
|---|---|---|
| `scriptCreate` works | Payload `scriptCreate(input) { item { id name } errors { … } }`, the same shape as the tag mutations. Input exactly as in the [API reference](03-api-reference.md) | ✅ |
| `scriptUpdate` is a **full replace** | `ScriptUpdateMutationInput` needs `scriptId`, `organizationId`, `name`, `description`, `language`, the three OS flags, `content`, `timeoutSeconds`, `inputs`, `outputs`. It also has an optional preview field `riskLevel` (`@tag preview-2026-10`). The ID doesn't change | Packager always sends the whole definition |
| Validation errors print the input type's schema | Useful because introspection is off | — |

### How the script runs on the device

| Finding | Detail | Effect |
|---|---|---|
| **Inputs are prepended as PowerShell assignments** (Q1) | Ecoverse writes `$TestString="hello world"`, `$TestInt=42`, `$TestBool=$true` above the script body, after `$Error.Clear()` and a UTF-8 `$OutputEncoding` line. Quotes, `$`, backticks, `;`, `&`, `|` and `%` are escaped correctly; types arrive as `String`, `Int32` and `Boolean`. Nothing is in `$args`, `$PSBoundParameters` or the environment | The preamble reads plain variables. Variable names are the input's `variable` field |
| **Outputs are read from variables after the script ends** | Ecoverse appends `echo '######OUTPUT#######'` and one `echo $<variable>` per output to the end of the script and parses the lines after the marker. `$global:`, `$env:` and `Name=value` on stdout don't work | Set outputs as plain script variables |
| **`exit` breaks outputs** | Calling `exit` skips the appended echo lines: the run reports `Something went wrong while parsing the output - expected 5 output variables` and every output is null | Ecoverse scripts must not call `exit`, `return` at script level, or throw an uncaught terminating error |
| **Every declared input must be sent** | A run that supplied 2 of the script's 8 inputs was refused: `ScriptRunValidationError: "Invalid number of input params."` | The launcher always sends the full input set, with `-` for unused values |
| **No `param()` block** | Ecoverse places its input assignments above the script, but `param()` must be the first statement in a PowerShell script | The shared script reads REST arguments from `$args` itself |
| `exitCode` is null | Both runs returned `exitCode: null`. `exit 0` gave `SUCCEEDED`, `exit 7` gave `FAILED` | Use output variables (`Result`, `ErrorCode`) for the outcome, not the exit code |
| **PASSWORD values are on disk in plain text** | The generated script is `C:\Windows\System32\config\systemprofile\AppData\Local\MSPScriptExecution\scripts\<guid>.ps1` (SYSTEM's profile) with the value as a literal. PASSWORD only hides it in the Ecoverse UI and API | Treat the registration token as present on the device anyway (it goes into `job.json` regardless). ❓ Whether old script files are deleted is being checked |
| **32-bit PowerShell 5.1** | `is64BitProcess: false`, working directory `C:\Program Files (x86)\Msp Agent`, `-ExecutionPolicy Bypass`, FullLanguage | File system and registry redirection apply (`System32` → `SysWOW64`, `HKLM\SOFTWARE` → `WOW6432Node`). The launcher must use `Sysnative` or the 64-bit registry view where it matters |
| Runs as SYSTEM | `NT AUTHORITY\SYSTEM`; the API reports `runAs: "Local System"` | As planned |
| **Process tree confirms the original problem** | `powershell.exe` ← `msp-ufa-script-execution.exe` (`components\msp-ufa-script-execution\0.2.1`) ← `msp-agent-core.exe` ← `services.exe` | The script is a child of the UFA agent, so it dies when the agent is removed. The scheduled-task handoff is required |
| stderr captured separately | `Write-Error` text arrives in `stdError`; the run still succeeds | — |

### Delivery delay, multi-line outputs, cancelling

A second batch at 15:15 (after `scriptUpdate`) and a run of a newly created copy at 15:25 sat `IN_PROGRESS` with **no execution** while all three devices reported `CONNECTED`. The Ecoverse audit log shows every create, update and execute was accepted, and no call got a 429 or a limit error.

| Finding | Detail | Effect |
|---|---|---|
| **Pickup can take more than 30 minutes** | The VM-B run was created 19:15:06Z and started **19:46:03Z** (31 minutes), then ran in 4 s. The 15:08 runs had started within about 15 s. The lab VMs are deliberately low-resource | The tracker's **Not picked up** state waits 30 minutes before flagging, and never treats a slow pickup as a failure on its own |
| Unpicked tasks don't expire | Still `IN_PROGRESS` with no execution after 34 minutes, far beyond `timeoutSeconds` (300) | The tracker decides when to give up, then cancels |
| **Multi-line outputs shift silently** | `"line1`nline2"` put `line1` in the second output, `line2` in the third and the JSON in the fourth, with no error | Every output must be one line; the tracker validates output shapes (e.g. `StatusJson` must parse, `TaskCreated` must be `true`/`false`) |
| Generated script files are deleted | Only the running script's `.ps1` was in `MSPScriptExecution\scripts` | The PASSWORD value is on disk only while the script runs |
| 32-bit environment has what the launcher needs | `$env:windir\Sysnative\…\powershell.exe` exists; the `ScheduledTasks` module loads | As planned |
| **`scriptTasksCancel` works** | Input `{ taskIds: [ID!]! }`. Selecting `items { id status }` returns `null` items with `INTERNAL_SERVER_ERROR`, **but the cancel still happens**; tasks then report `CANCELED` (one L) | Select only `errors`. N-voke's task-state code matches `CANCEL` case-insensitively, so both spellings work |
| **Blank outputs break parsing** | An output set to `''` (or never set) echoes a blank line, which Ecoverse drops: the run reports `expected 5 output variables` and every output is null. In the multi-line run above, the extra line and the dropped blank line cancelled out, which is why it "parsed" with shifted values | Every output always has a value; the scripts use `-` for "none" |
| **`$host.SetShouldExit(n)` marks the run failed and keeps the outputs** | With `SetShouldExit(7)` and no `exit`: status `FAILED`, `errorMessage: "error trying to run script: exit status 7"`, `exitCode` still null, and all outputs returned correctly. With no call: `SUCCEEDED` | The script reports failure with `SetShouldExit(ErrorCode)` at the very end. The tracker reads details from outputs and the code from `errorMessage` |
| Pickup was fast again | Runs at 15:49 and 15:50 started within 20 to 40 s on all three VMs | The 31-minute delay was temporary |
| Transient gateway 503 | `upstream connect error or disconnect/reset before headers`, plain text, on a read | Reads retry 503 as well as 429 and `INTERNAL_SERVER_ERROR` |

## Offline devices (test 0.3, 2026-10-19)

VM-A was powered off at about 19:54Z. Two runs were launched after Ecoverse marked it disconnected.

| Finding | Detail | Effect |
|---|---|---|
| Ecoverse notices in about 3.5 minutes | `agentConnection` went `DISCONNECTED` at 19:57:23Z | The planner's view of "online" lags reality by minutes |
| A slow VM keeps checking in while it shuts down | N-central recorded a check-in at 19:56:40Z, about 2.5 minutes after power-off was started | "Agent removed" waits for a quiet period; one late check-in isn't a sign of life |
| **`lastApplianceCheckinTime` drifts while the device is off** | It moved from `19:56:40.969` to `.074` to `.187`, a fraction of a second every 5 minutes, with the VM off | Compare the time's **age** against a threshold; never treat "the value changed" as a check-in |
| **Without `shouldRunWhenOnline`, an offline device is refused** | `assetsScriptRun` returned `items: null` and `ScriptRunValidationError: "Device is offline."`; no task was created | The planner sets `shouldRunWhenOnline: true` for every device Ecoverse shows as disconnected (and can set it for all, to cover the 3.5-minute lag) |
| **With it, the task is accepted and queued** | Task created 20:00:13Z; `task(id)` returned `null` for about 30 s, then `IN_PROGRESS` with no execution | A just-created task can briefly be null (N-voke already tolerates this) |
| A queued task looks exactly like a stalled one | Both are `IN_PROGRESS` with an empty `executions` list | The tracker records which launches were queued for offline devices and doesn't flag them as **Not picked up** |
| **A queued task outlives its `timeoutSeconds`** | Queued for 22.5 minutes, 4.5 times its 300 s timeout, then ran | `timeoutSeconds` limits the run, not the wait. The maximum wait wasn't tested (dropped from Phase 0); the tracker sets its own limit and cancels |
| Devices can come back into Windows Update | On power-on VM-A spent about 10 minutes installing updates before reaching Windows | Expected in real waves; see [Slow and low-resource endpoints](05-endpoint-scripts.md#slow-and-low-resource-endpoints) |
| N-central sees the device before Ecoverse | N-central check-in at 20:19:13Z; Ecoverse `CONNECTED` at 20:21:30Z (its `statusChangedAt` moved at 20:20:06Z while still `DISCONNECTED`) | Don't treat Ecoverse `DISCONNECTED` alone as "offline" during a reboot |
| **The queued task ran once the device was back** | Started 20:22:49Z, about 80 s after Ecoverse reconnected; ran 6.4 s; `SUCCEEDED` with all five outputs correct | ✅ `shouldRunWhenOnline` works as planned |

## REST path (tests 0.5–0.7, 2026-10-19)

`nvoke-rest-probe.ps1` was uploaded by hand to the source server's Script/Software Repository at the N-voke Demo Source service org, with **Enable API** on , then run with `POST /api/scheduled-tasks/direct`, `taskType: "Script"`, `credential: { type: "LocalSystem" }` and one `CommandLine` parameter.

| Finding | Detail | Effect |
|---|---|---|
| **Direct tasks run** | HTTP 201 `{ data: { taskId } }`; the run starts within seconds and takes about 10 s. Runs as Local System from `C:\Program Files (x86)\N-able Technologies\Windows Agent\Temp\Script\Task--<detail task id>\` | ✅ The REST path works with a plain `.ps1`; no AMP needed |
| `ScheduledTaskParameter` needs `name`, `type`, `description` | `type` is one of `string`, `integer`, `boolean`, `text`, `dword`, `password` | The client always sends all three |
| Customer vs site | Passing the device's `customerId` records the task under the customer (`orgUnitId` = customer, `siteId: null`) | Use the device's `customerId` |
| Two task IDs | `/status/details` reports a per-device `taskId` that differs from the direct task's ID; the working folder uses the per-device one | Store both |
| **Pass/fail only, from `/status/details`** | Per device: `status` `Success` or `Failed`, `message` "Successful execution on remote device" or "Failed execution on remote device." `exit 7` and `$host.SetShouldExit(7)` both give `Failed` | One script can use `SetShouldExit` on both paths |
| **`/status` hides failures** | `statusCounts` was `{"Completed": 1}` for failed runs too | Read `/status/details`, never `/status` |
| **No exit code in the API** | The UI's task Status tab shows a **Return Code** (0, 7), but `/status/details` has no such field | The script reports its own `ErrorCode` |
| **Any stderr replaces stdout** | With anything on stderr (`Write-Error` or `[Console]::Error`), `output` holds only the stderr text (line breaks removed), stdout is lost, and there is no output file | The script never writes to stderr: errors are caught and reported in the result line |
| **Without stderr, `output` is a ~500-character preview** | `Write-Output`, `Write-Host` and `[Console]::Out` are all captured, but `output` stops at 497 characters | Don't parse `output` |
| **The full output is in a zip** | `outputFileName` gives `/download/scheduled_tasks/output_files/<site>/<device>/<task>/<name>_<device>_<timestamp>.zip`, containing `Output-Formatted.txt` with **Exit Code**, start and finish times, and the complete console output (the `NVOKE-RESULT` line included) | The tracker downloads the zip and parses `NVOKE-RESULT` and `Exit Code` from it |
| ⚠️ **The output zip downloads without authentication** | A plain GET with no credentials returned the zip (HTTP 200). The path is predictable (org unit, device ID, task ID, task name, timestamp) | Scripts must never print secrets. Worth reporting to N-able |
| **64-bit PowerShell on the REST path** | `is64BitProcess: true`, PowerShell 5.1, `-ExecutionPolicy Bypass -File`, working directory `…\Windows Agent\Temp\Script\Task--<id>` | Unlike Ecoverse (32-bit): the script handles both |
| **Process tree: `powershell.exe` ← `cmd.exe` ← `agent.exe`** | `agent.exe` is the Windows Agent (`…\Windows Agent\bin\agent.exe`) | A REST-launched script also dies when the agent is uninstalled: the scheduled-task handoff is needed on both paths |
| **`cmd.exe` expands `%VAR%` in arguments** | `%PATH%` inside a quoted argument arrived expanded; quotes, `$`, `&`, `\|` and backticks arrived literally | Argument values must not contain `%` or `"`; the client validates them (host names, numeric IDs and tokens are safe) |
| **The token is in the process command line** | `commandLineContainsToken: true` | Any local process can read it while the launcher runs (a few seconds) |
| **Offline devices: the task waits** (test 0.8) | VM-B powered off; `POST /direct` still returned 201. `/status/details` showed **`Pending`** (empty message) while it was off, then `In Progress`, then `Success`. Ecoverse saw it reconnect at 20:51:34Z; the task started at 20:52:11Z (37 s later) and took 21 s on the freshly booted VM | No `shouldRunWhenOnline` equivalent is needed. Unlike Ecoverse, `Pending` tells a queued task from a running one. How long a task can stay `Pending` is still open |
| Deleted or API-disabled item | `POST /direct` returns **HTTP 500** `TaskCreateException: … Failed to verify REST API accessible flag for provided script. taskItemID=…` | Report as "script item missing or Enable API off on <server>", a setup problem, not a device failure |
| **The command line is visible in the UI** | With `type: "string"`, the task's DETAILS tab shows *Command Line Parameters* in full, read-only, including the registration token, exactly as sent (`%PATH%` unexpanded) | Visible with `type: "password"` too: the field showed the same full text. The REST path exposes the registration token in N-central's task details either way |
| Arguments with `type: "password"` still arrive | The script ran normally | — |
| **Deleting an item deletes its task history** | After the first script version was deleted, its tasks vanished from the UI; `GET /api/scheduled-tasks/{id}` and `/status` return 404 `TaskNotFoundException`, and `/status/details` returns `200 { data: [], totalItems: 0 }`, the same as a task that hasn't reported yet | The tracker stores each REST result (status and parsed output zip) as soon as it's available. A known task with a 404 means "history deleted", not "pending". Before a re-upload, N-voke collects outstanding results and only then says it's safe to delete the old item |
| **A repository item can't be updated in place** | To change the file, the item must be deleted and uploaded again, which gives it a **new item ID** | Every script version change on the REST path means: delete, upload, turn on Enable API, enter the new ID in N-voke |

## First migration (test 0.10, 2026-10-19)

VM-C (Windows 11, 2 GB RAM, about 90% memory in use) was moved from `source.example.com` (Demo - Acme Corporation / Demo - Acme HQ) to `destination.example.com` site Demo - Acme HQ, using the endpoint script `nvoke-migration.ps1` 0.1.0+950f50f launched through the **source** Ecoverse tenant.

| Time (UTC) | Event | Source |
|---|---|---|
| 21:13:03 | First launch accepted; **never picked up** (no execution after 10 minutes, even after restarting `Msp Agent` on the device) while a new probe run on the same device ran within 30 s | Ecoverse |
| 21:23:00 | Stuck launch cancelled (`scriptTasksCancel`, selecting only `errors`): `CANCELED`, never ran | Ecoverse |
| 21:23:24 | Relaunched | Ecoverse |
| 21:23:49 | **Staged**: folder, `job.json`, `status.json`, scheduled task registered from 32-bit PowerShell; `mode move` detected from `ServerConfig.xml` | launch outputs |
| 21:25:54 | Scheduled task's first run (64-bit PowerShell) | install log |
| 21:26:06 | Installer downloaded with BITS: 133.3 MB in 11 s, Authenticode valid (N-ABLE TECHNOLOGIES LTD) | install log |
| 21:27:13 | **New device** under Demo - Acme Corporation / Demo - Acme HQ on the destination, no check-in yet | destination REST |
| 21:28:17 | Installer exit 0 (2 min 11 s) | install log |
| 21:31:55 | **First destination check-in** | destination REST |
| 21:32:27 | Source `lastApplianceCheckinTime` jumped to 66 minutes old | source REST |
| 21:40:55 | Second scheduled run: **healthy** (Windows Agent 2026.4.10011, service running, `ServerConfig.xml` → destination). Task unregistered itself; `job.json` (with the token) deleted | `Status` run through the source Ecoverse tenant |

| Finding | Effect |
|---|---|
| The end-to-end design works: scheduled-task handoff, BITS download, signature check, install, registration in the mapped site, no reboot | ✅ |
| **A single launch can be stuck while others to the same device go through.** Restarting `Msp Agent` didn't release it; cancel-and-relaunch did | **Not picked up** → cancel and relaunch, automatically after the threshold |
| The 32-bit launcher can register a task that runs 64-bit PowerShell (`System32` in the task definition) | As planned |
| **The Ecoverse agent (`Msp Agent`) is not moved.** After the N-central move the device stays in the **source** Ecoverse tenant under the same asset ID, `CONNECTED`, still linked to the old N-central device; the destination tenant doesn't show it | Ecoverse "disconnected" can't be used as the Agent removed signal. Status and cleanup can still run through the source tenant. ❓ Whether and when it re-enrolls in the destination tenant |
| **The installer upgraded in place and touched nothing else.** Source agent 2026.4.0.25 → destination 2026.4.1.11 (`Windows Agent` 2026.4.10011). Windows Agent processes restarted 17:27:54–17:30:13 local; **MSP Core Agent 3.43.0** (`msp-agent-core`, the UFA), File Cache Service Agent, Request Handler Agent, Patch Management (`PME.Agent`) and Take Control were not restarted or reinstalled (process start times before the install; `Msp Agent\config` last written at the original install) | The "installer removes the agent, maintenance services and UFA" behaviour applies when it isn't an upgrade (to be checked in 0.12). For an upgrade, the UFA keeps its source enrolment, so **N-voke must handle the UFA itself** (Q13) |
| **Why the UFA stayed: the installer skips the UFA uninstall on an upgrade.** The agent installer only uninstalls and cleans up the UFA when the run is *not* an upgrade; for a Windows Installer major upgrade (newer version, same product) or an N-able-initiated self-upgrade it deliberately skips that step (confirmed from the installer's logic). 0.10 was an upgrade (2026.4.0.25 → 2026.4.1.11) | Whether a migration moves the UFA depends on source and destination versions, which vary by partner. N-voke's full cleanup makes the result the same either way |
| **A leftover UFA identity can't reconnect.** After a full manual uninstall and reinstall of the source agent on VM-C, the old Ecoverse asset went `DISCONNECTED` (21:46:37Z) and the reinstalled `Msp Agent` ran but never reconnected: its log shows `vault … jwtToken … element not found`, then the OAuth token request failing with `400 The SSL certificate error` (nginx), so nothing is sent. `C:\ProgramData\MspAgent\config\ServerConfig.xml` holds DPAPI-encrypted `AgentSessionId`/`AgentAuth` values and `ServerIP` `localhost` | The UFA's identity survives a normal uninstall while the server deprovisions it. The full cleanup deletes `ProgramData\MspAgent`; ❓ whether a client certificate elsewhere (machine store) must go too |
| **A normal uninstall leaves N-able folders behind** | After uninstalling the agent and the UFA on VM-C, these remained and had to be deleted by hand before a clean reinstall: `Program Files (x86)\N-able Technologies`, `ProgramData\{MspAgent, MspPlatform, N-Able Technologies, GetSupportService_N-Central, GetSupportService_Common_N-Central}` | Exactly the full cleanup's folder list. Removing the products isn't enough; the folders carry the old identity |
| **With the folders removed, the reinstalled UFA starts clean but waits to be provisioned** | After deleting the leftover folders and reinstalling the source agent (a new device ID, first check-in about 22:12Z), `Msp Agent` came back with a `deprovision.log` (6:10 PM local) but no `provision.log`. Its log no longer shows `400 The SSL certificate error`; instead `vault … amazonRoot / chain: element not found` and `certificate chain missing` on each MQTT attempt (every ~2 min) | The stale-identity problem is gone. A fresh UFA needs N-central/Ecoverse to provision it (its config points at `localhost`). ❓ How long provisioning takes, and what triggers it. Its `deprovision.log` shows a startup check, *reset the agent if its GUID doesn't match the certificate CN*, skipped here because it isn't provisioned yet |
| **The clean UFA provisions only after a restart** | Timeline (UTC): new device created 22:07:22; UFA installed ~22:09:42 and logging `certificate chain missing`; first Windows Agent check-in 22:12:17; nothing until a **reboot** ~22:20; then `provision.log`: provisioning required → X.509 fetch **200** → config saved (`organization`, `customerId`, `host`, `level`, `parentCustomerId`, `serverGuid`) at 22:20:42; **new asset `asset-2`** visible 22:25:16, `CONNECTED` 22:25:58 | The UFA starts before the new Windows Agent has registered and doesn't provision until it's restarted. The installer restarts `MspAgent` once the Windows Agent is healthy, then waits for a successful provisioning. The provisioned identity carries the provisioning server's GUID and customer, which is what ties it to that server's Ecoverse tenant |
| **The Ecoverse asset ID does not survive a reinstall** (Q5) | Old asset `asset-1` (original device ID) → new asset `asset-2` (new device ID) | Matching uses serial number and MAC address, never the asset ID |
| **The Ecoverse asset follows the N-central device record** | When the original source record was removed at about 21:59:52Z, the asset `asset-1` disappeared from the source tenant at 22:00:54Z (gone, not disconnected). The reinstall registered under a new device ID | Deleting old source records (the optional *Retired* step) also clears the old Ecoverse asset; a migrated device gets a new asset ID, so matching must use serial and MAC (Q5) |
| Source `lastApplianceCheckinTime` is unreliable around the move (reported recent, then jumped to an hour old) | Agent removed relies on the destination appearance plus a quiet source, not one source reading |

## Rerun with full cleanup (test 0.10b, 2026-10-19)

VM-C, freshly reinstalled on the source (new device ID, asset `asset-2`), moved to `destination.example.com` site Demo - Acme HQ with build 0.1.0+05b0889 (`CleanMode=full`, UFA restart step), launched through the source Ecoverse tenant.

| Time (UTC) | Event | Seen in |
|---|---|---|
| 22:29:31 | Launch | Ecoverse (source) |
| 22:30:45 | **Staged** (`mode move, clean full`), picked up within about a minute | launch outputs |
| 22:32:23 | Scheduled task's first run: download, then full cleanup | — |
| 22:35:26 | **Source Ecoverse asset `DISCONNECTED`**: the cleanup removed the UFA | Ecoverse (source) |
| 22:35:28–43 | Folders removed (`BeAnywhere Support Express`, `ProgramData\{N-able Technologies, MspAgent, MspPlatform}`, …); `ProgramData\GetSupportService_N-Central` **partly left (files in use)**; both `N-able Technologies` registry keys removed | `status.json` |
| 22:35:43 → 22:36:46 | New agent installed, exit 0 (about 1 minute) | `status.json` |
| 22:36:29 | **New destination device** under Demo - Acme Corporation / Demo - Acme HQ | destination REST |
| 22:41:23 | First destination check-in | destination REST |
| 22:45:15 | **New asset `asset-3` in the destination tenant** (the UFA provisioned on its own this time, about 4 minutes after the check-in, no restart needed) | Ecoverse (destination) |
| 22:45:57 | Asset `CONNECTED` | Ecoverse (destination) |
| 22:47:31 | Installer's second run: **healthy** (Windows Agent on the destination; UFA present, running, provisioned); task removed itself, `job.json` deleted | `status.json` |
| 22:46–22:49 | Runs against the new asset refused: `ScriptRunAssetNotFoundError: "Asset not found."` | Ecoverse (destination) |
| 22:49:58 | `Status` accepted through the **destination** tenant | Ecoverse (destination) |
| 22:52:09 | **Cleanup**: `Cleaned=true`, task and `C:\ProgramData\N-voke` removed | Ecoverse (destination) |

| Finding | Effect |
|---|---|
| ✅ **With full cleanup, the whole device moves**: Windows Agent and UFA both on the destination, a new asset in the destination tenant, about 18 minutes from launch to healthy on a 2 GB VM | Q13 answered: full cleanup is the default |
| The fresh UFA provisioned by itself here (it didn't on the earlier source reinstall); the restart step is a safety net | Keep the restart, but only when not yet provisioned |
| **A new asset can't run scripts for a few minutes** after it shows `CONNECTED` (`ScriptRunAssetNotFoundError` for ~4 minutes) | The tracker retries this error for newly registered assets instead of falling back to REST |
| **A new asset's inventory is empty at first** (no serial, manufacturer, model or hostname in the Ecoverse UI right after registration) | Matching waits for inventory, or uses the destination N-central record's assets in the meantime |
| `GetSupportService_N-Central` can be locked during cleanup | Retry the leftover folder on the next run, or after the new agent installs |
| The status summary returns only the last 10 history entries, which hides the uninstall results | Return the full history (or the uninstall entries) in `StatusJson` |

## Status of Phase 0 tests

| # | Status |
|---|---|
| 0.9 | Done for missing tokens and the SO case. An **expired** token wasn't available to test |
| 0.9b | ✅ Generation works for a customer and a site; calling it again leaves a valid token unchanged (no renewal). Expired-token behaviour still open |
| 0.11 | Field names confirmed. Whether the asset ID survives a move waits for 0.10 |
| Q6 | Lab runs 2026.4.0 and 2026.4.1; both behave identically for everything tested |
| 0.1 | ✅ `scriptCreate` and `scriptUpdate` (full replace) |
| 0.2 | ✅ Inputs, outputs and run status. Outputs need: no `exit`, single-line values, no blank values. `exitCode` is always null; `SetShouldExit(n)` gives `FAILED` with `exit status n` in `errorMessage` |
| 0.4 | ✅ Inputs are prepended as typed variable assignments |
| 0.3 | ✅ Offline launch is refused without `shouldRunWhenOnline`; with it the task queues, outlives its timeout, and runs about 80 s after the device reconnects. Maximum queue time not yet known |
| 0.5 | ✅ Manual upload; a new version means delete and upload, which gives a new item ID |
| 0.6 | ✅ Direct `Script` task runs as Local System with `CommandLine` arguments in `param()` (64-bit, via `cmd.exe`) |
| 0.7 | ✅ Pass/fail from `/status/details`; full output and **Exit Code** in the unauthenticated output zip; stderr replaces stdout |
| 0.8 | ✅ A direct task to an offline device is accepted, shows `Pending`, and runs about 40 s after the device reconnects |
| 0.10 | ✅ First run: N-central moved, but the UFA stayed in the source tenant (in-place upgrade). **Rerun with full cleanup: complete move** (N-central and Ecoverse), healthy, cleaned up; ~18 min launch to healthy |
| 0.12 | Not started. 0.5 and 0.6 now test a hand-uploaded `.ps1`, not an AMP |
