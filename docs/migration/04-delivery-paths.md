# 4. Delivery paths

A device can be launched in one of two ways. Both run the same endpoint script and hand off to the same scheduled task; they differ only in how the launcher reaches the device and how its result comes back.

| | **Ecoverse path** (preferred) | **REST path** (fallback) |
|---|---|---|
| Runs on the device through | The UFA (Unified Agent) | The N-central Windows Agent |
| Launcher published as | An N-able script, through `scriptCreate` (plain PowerShell), automatically | A PowerShell script in the N-central **Script/Software Repository**, **uploaded by hand once per server** with **Enable API** on. N-voke exports the file and asks for its item ID |
| Launched with | `assetsScriptRun`: many devices per call, typed inputs per call | `POST /api/scheduled-tasks/direct` with `taskType: "Script"`: one device per call, inputs as one `CommandLine` parameter |
| Offline devices | `shouldRunWhenOnline` queues the launch (without it the launch is refused) | The direct task is accepted, shows `Pending`, and runs when the device reconnects (confirmed) |
| Result | Typed output variables and `SUCCEEDED`/`FAILED` (`exitCode` is always null) | `Success`/`Failed` from `/status/details`; the exit code and the `NVOKE-RESULT {json}` line come from the output zip |
| Manual steps | None | Upload the script and turn on **Enable API**, once per server and again when the script version changes |
| Needs | Ecoverse token for the tenant the device is in now, that server linked to Ecoverse, device's UFA connected | That server's REST JWT |
| Runs as | 32-bit PowerShell 5.1, child of `msp-ufa-script-execution` | 64-bit PowerShell 5.1, child of `cmd.exe` ← `agent.exe` |

Both paths need the **destination** REST JWT, because registration tokens and numeric customer IDs come only from N-central REST.

## Ecoverse on the destination

The launch only needs Ecoverse on the **source**. What happens after the move depends on how the destination relates to Ecoverse, which the operator states per project (a fresh destination has no devices, so it can't be detected):

| Setup | Typical when | After the move |
|---|---|---|
| **Destination not linked to Ecoverse** (default) | A new N-central server, linked to the partner's tenant only after the source is decommissioned, or later by N-able | The full cleanup removes the device's UFA, and an unlinked server can't provision a new one, so **the device leaves Ecoverse until the destination is linked**. Tracking after the move is REST-only (destination appearance and check-ins); the installer doesn't wait for a UFA (`ExpectUfa=false`); `Status`/`Cleanup` aren't needed for a successful device (see the review period in [Endpoint scripts](05-endpoint-scripts.md)) |
| Destination linked to the **same** tenant | N-able links the new server before the migration | The device reappears in the same tenant as a **new asset** linked to the destination server; one Ecoverse key covers both sides |
| Destination linked to **another** tenant | Separate partner tenants (the Phase 0 lab) | As above, in the other tenant; needs that tenant's key too |

Even a linked destination only deploys UFAs when the server meets Ecoverse's prerequisites (✅ per N-able: **cloud eventing is turned on and SSO is enabled**; the UFA needs what the N-central agent needs). The UFA is installed by the agent as a component, like Take Control, not by the main installer: **the N-central agent triggers it when those conditions are met**. A destination without them gets no UFA, however the device arrived.

**No AMPs.** Ecoverse scripting doesn't run AMPs: its repository holds plain scripts with declared **input** and **output** variables (confirmed in the Ecoverse UI, *Scripting → Add script*). The REST path doesn't need one either, because a direct task can run a repository **Script**. So N-voke never builds, publishes or wraps an AMP, and needs no SOAP. The REST API has no endpoint for uploading repository items, which is why the REST path's script is uploaded by hand.

## Choosing a path

The choice is made **per device**, not per customer: in practice some devices in a customer are connected through Ecoverse and some aren't.

```mermaid
flowchart TD
    A[Device selected] --> B{Ecoverse token configured<br/>and valid?}
    B -- No --> R[REST path]
    B -- Yes --> C{Source server linked to Ecoverse?<br/>any asset whose ncentralDevice.server.origin<br/>matches the source host}
    C -- No --> R
    C -- Yes --> D{Ecoverse asset found with the same<br/>N-central device ID on the source server?}
    D -- No --> R
    D -- Yes --> E{agentConnection.status = CONNECTED?}
    E -- Yes --> G[Ecoverse path]
    E -- No --> F{Source N-central says the device<br/>checked in recently?}
    F -- Yes --> R
    F -- No --> H[Ecoverse path,<br/>queued with shouldRunWhenOnline]
```

Rules:

1. **Detection is cheap.** One paged `assetSearch` over the source organizations gives every linked device's N-central ID, server and connection status. One `GET /api/devices` per source org unit gives N-central's own check-in times. The planner joins the two on `(server origin, N-central device ID)`.
2. **The operator can override** the path per device, per customer or per wave (for example "REST only" for a customer where the UFA isn't trusted yet).
3. **Fallback at launch time.** If `assetsScriptRun` returns `ScriptRunNotPermittedError`, `ScriptRunAssetNotFoundError` or `ScriptRunScriptNotFoundError` for a device, that device moves to the REST path and is launched there in the same wave, and the switch is recorded in its timeline. Exception: for an asset that only just registered (for example the destination asset when running `Cleanup`), `ScriptRunAssetNotFoundError` is normal for a few minutes and is retried instead.
   **Always queue on Ecoverse.** Every Ecoverse launch sets `shouldRunWhenOnline: true`: Ecoverse refuses an offline device without it (`"Device is offline."`), and its view of "connected" lags reality by about 3.5 minutes.
   **Stalled tasks are relaunched.** A task with no execution 30 minutes after launch, on a device that is connected, is cancelled and relaunched (once on the same path, then on the other); see **Not picked up** in [Tracking](06-tracking.md).
4. **No double launch.** A device is launched on one path at a time. Switching paths after a launch requires the first launch to have ended (failed, cancelled or expired). The scheduled task's fixed name also guards against duplicates on the device itself.
   **A launch that may have gone through is never sent again by itself.** Before anything is sent, the devices are saved as "launch sent, outcome not recorded", so a crash, a quit or a refused save can't leave a launched device looking untouched. If the answer is lost (the connection dropped, a server error, an answer with no task ID) the device stays marked, shown as **Launch failed** with a message to check Activity (Ecoverse) or the task list (N-central), and Retry is refused until the operator confirms it didn't run (TUI: `R`/`O` asks first; CLI: `retry … --checked`). A plain refusal (access denied, validation, rate limiting) is not treated as unsure. If the save fails, the launch stops with nothing more sent. Launches also refuse a script that isn't this version: the uploaded REST copy and the published Ecoverse copy are each checked against the script built into N-voke.
5. **Same-server reinstall** works on either path; source and destination are the same server (confirmed, test 0.12).
6. **The REST path is opt-in per server.** Until the operator has uploaded the script and entered its item ID for the source server, devices that would need the REST path are shown as **Blocked: upload the script** instead of being launched. A project that never uses the REST path never asks for an upload.
7. **Launch from where the device is now.** The launch goes through the tenant (or server) the device currently belongs to; `Status` before the move goes there too. After the move, `Status` and `Cleanup` go through the **destination**: its Ecoverse tenant when it's linked, otherwise its REST path (which needs the script uploaded to the destination's repository), or they aren't run at all and the device cleans up after the review period. A move back runs the same way in reverse (confirmed).

## One script, two packages

The endpoint logic is written once (see [Endpoint scripts](05-endpoint-scripts.md)) as a single `nvoke-migration.ps1` with an `Action` input (`Launch`, `Status`, `Cleanup`) and `installAgent.ps1` embedded. One file per server keeps the REST path's manual upload to a minimum. The packager produces it two ways:

```mermaid
flowchart LR
    S["nvoke-migration.ps1<br/>(single source, embedded in N-voke)"] --> P{Packager}
    I["installAgent.ps1"] --> P
    P --> E["Ecoverse script<br/>scriptCreate / scriptUpdate, automatic<br/>inputs = Action, DestServer, DestCustomerId, RegistrationToken (PASSWORD), …<br/>outputs = TaskCreated, LauncherVersion, StatusJson, …"]
    P --> M["Repository script file<br/>exported for manual upload<br/>to each server with Enable API on<br/>-Name value arguments = same names"]
```

- Both read the same input names. A small preamble maps the platform's input mechanism (Ecoverse input variables assigned above the script, or `-Name value` arguments from the REST `CommandLine`, read from `$args` because there can be no `param()` block) onto common variables, so the rest of the script doesn't know which path it came through.
- Both carry a **content hash** in their name or description (for example `N-voke migration 1.0.0 (3f9a2c1)`). On Ecoverse the packager republishes only when the hash changes.
- On the REST path N-voke can't read back the uploaded file. The script reports its own `LauncherVersion` in the `NVOKE-RESULT` line; when it doesn't match, the tracker stops launching on that server and asks the operator to upload the current file.
- N-central can't replace a repository item's file: the operator **deletes the item and uploads it again**, which creates a **new item ID**. N-voke stores the item ID and script version per server, asks for the new ID after each upload, and reports launches against a deleted item as a setup problem rather than a device failure. **Deleting an item also deletes the history of every task that used it** (UI and API), so before telling the operator to delete the old item, N-voke makes sure it has collected and stored every outstanding result from it.
- Nothing secret is baked into either package. Everything environment-specific is an input supplied at launch.

## Launching a wave

| Step | Ecoverse | REST |
|---|---|---|
| Group devices | By destination customer or site (inputs are per call) | By device (one call each) |
| Inputs | **All** of `Action`, `DestServer`, `DestCustomerId`, `RegistrationToken` (PASSWORD), `MigrationId`, `LauncherVersion`, `TaskRepeatMinutes`, `TaskExpiryDays`, `CleanMode`, `ExpectUfa`, `ReviewDays` (Ecoverse refuses a run that omits any; unused ones are `-`) | The same names, as `-Name value` pairs in the `CommandLine` parameter. The registration token is therefore visible in the task's details in N-central (see [Security](08-security.md)) |
| Throttle | A few calls per second; 429 and 503 are retried; a launch is never retried automatically (it may have created tasks) | A semaphore at 30 concurrent direct-task calls, 429 retried |
| Record | The task ID per device (decoded from the returned IDs) | The task ID per device |
| Visible in Activity | Yes, as a normal N-voke job | Shown in the migration page only |

## When neither path works

Devices with no connected UFA **and** no recent N-central check-in can't be launched remotely. The planner lists them as **Unreachable**, with their last-seen times, and the report exports them so they can be handled out of band (GPO, Intune, on site). If they come back online they are picked up again automatically.
