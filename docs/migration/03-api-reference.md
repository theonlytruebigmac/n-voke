# 3. API reference

What the feature needs from each API, and how sure we are of it.

**Status key:**
- ✅ **Verified:** used in N-voke today, called live, or present in the server's own OpenAPI spec (N-central 2026.4).
- 🔶 **From the schema:** in the published schema or API spec, but not yet called.
- ❓ **To verify:** inferred; must be confirmed in the proof of concept ([Roadmap](09-roadmap.md#phase-0-proof-of-concept)).
- ✅ **(lab)** / ❌: called against the lab servers; see [Phase 0 findings](10-phase0-findings.md).

## Ecoverse GraphQL (`https://api.n-able.com/graphql`)

Authentication: `Authorization: Bearer <API-Access token>`. Introspection is disabled, so client code is written against the published schema. Requests need a normal `User-Agent` header; the gateway refuses some default library agents with HTTP 403.

### Reads

| Operation | Use | Status |
|---|---|---|
| `me { id }` | Check the token works | ✅ |
| `organizationSearch` | Org tree (service orgs, customers, sites) | ✅ |
| `assetSearch(inOrganizations, where, first, after)` | Devices with `name`, `customer`, `site`, `agentConnection { status statusChangedAt }`, `operatingSystem { name }` | ✅ |
| `asset.operatingSystemInfo { name architecture type }` | Windows filter (`type: WINDOWS`) and `architecture` (`64-bit`); preferred over `operatingSystem { name }`, which can be null while inventory is pending | ✅ (lab) |
| `assetSearch(first: 1000, orderBy: { field: NAME, direction: ASC })` | Page devices in large chunks; `first: 1000` is accepted (N-voke's existing queries use 100) | ✅ (lab) |
| `asset.ncentralDevice { deviceId server { id origin } }` | Link an Ecoverse device to its N-central device ID and server URL | ✅ |
| `asset.systemInfo { hostname serialNumber manufacturer model }`, `asset.networkInterfaces { macAddress }` | Match the new device to the old one. `systemInfo` has no `macAddress`; `networkInterfaces` can be null until inventory reports, and includes empty MACs | ✅ (lab) |
| `scriptSearch(inOrganizations)`, `script(id)` | Find an already published launcher | ✅ |
| `task(id)` (batched with aliases, at most 4 per request; see [Limits and errors](#limits-and-errors)) | Task `status`, `createdAt`, and `executions.nodes` with `status`, `exitCode`, `errorMessage`, `startedAt`, `updatedAt`, `durationMilliseconds`, and on `ScriptTaskExecution`: `output { runAs stdOutput stdError outputs { name variable type value { … } } }` | ✅ |

### Writes

| Operation | Use | Status |
|---|---|---|
| `scriptCreate(input: ScriptCreateMutationInput!)` | Publish the launcher. Input: `organizationId`, `name` (≤64), `description` (≤1024), `language` (`POWERSHELL` …), `isWindowsCompatible`, `isLinuxCompatible`, `isDarwinCompatible`, `content` (plain text), `timeoutSeconds` (0–3600, 0 = 3600), `inputs` (`name`, `variable` ≤48 chars each, `type` STRING/PASSWORD/BOOLEAN/INT/FLOAT/DATE_TIME, default `value`), `outputs` (`name`, `variable`, `type`) | ✅ (lab). Payload `item { id }`. Inputs are prepended to the script as typed PowerShell assignments; outputs are read from variables after the script ends, so the script must not `exit`, and every output must be one non-blank line. `exitCode` is always null; `$host.SetShouldExit(n)` yields `FAILED` with `exit status n` in `errorMessage`. See [Phase 0 findings](10-phase0-findings.md#ecoverse-script-runs-tests-01-02-04-2026-10-19) |
| `scriptUpdate` | Refresh the launcher. **Full replace**: every field of the create input plus `scriptId` is required | ✅ (lab) |
| `scriptsDelete` | Remove the launcher | 🔶 |
| `assetsScriptRun(input: { assetIds, configuration: { scriptId, name ≤128, timeoutSeconds, credential, inputs, shouldRunWhenOnline } })` | Launch on devices with per-run inputs. Returns `items { id }` (one task per device) and typed `errors` | ✅ (lab) |
| `shouldRunWhenOnline: true` | Queue the launch for offline devices. **Required** for a disconnected device: without it the run is refused (`ScriptRunValidationError: "Device is offline."`). A queued task is `IN_PROGRESS` with no execution, outlives `timeoutSeconds`, and runs shortly after the device reconnects | ✅ (lab) |
| `scriptTasksCancel(input: { taskIds })` | Cancel queued launches. Select only `errors`: selecting `items { … }` returns null items with `INTERNAL_SERVER_ERROR`, although the cancel succeeds. Tasks then report `CANCELED` | ✅ (lab) |
| `staticTagCreate`, `assetStaticTagsAssign`, `assetStaticTagsUnassign` | Mirror stages and waves as tags | ✅ |

### Known quirks (already handled in N-voke)

- `assetsScriptRun` must select only `items { id }`; on a just-created task, `name` resolves to null and errors the whole response.
- Script-run task IDs are base64 `"<task uuid>:<asset uuid>"`; the device comes from decoding the ID.
- A task can end (cancelled, or expired before the device picked it up) while its execution still reports pending.
- `text: { contains }` is whole-word; `name: { contains }` is substring.
- The gateway returns plain-text 401/403/429, not GraphQL errors.

### Limits and errors

From N-able's documentation ([limits](https://developer.n-able.com/gql/docs/why-do-i-see-max_height_limit-max_depth_limit-and-max_aliases_limit-errors), [error handling](https://developer.n-able.com/gql/docs/error-handling)), confirmed in the lab on 2026-10-19:

| Limit | Value | Error (HTTP 400, in `errors[].extensions.code`) |
|---|---|---|
| Fields per operation (height), counting nested selections and every alias | 130 | `MAX_HEIGHT_LIMIT` |
| Nesting depth | 15 | `MAX_DEPTH_LIMIT` |
| Aliases per operation | 5 | `MAX_ALIASES_LIMIT` |

- A full task selection is about 30 fields, so **at most 4 tasks per aliased poll**: 4 passed, 5 failed with `MAX_HEIGHT_LIMIT`, and 6 minimal tasks failed with `MAX_ALIASES_LIMIT`. N-voke's poll batch was 20, which broke polling for any run on 5 or more devices; it is now 4.
- Rate limiting is **cost-based**, but responses carry no cost or budget headers or extensions, so the client can only react: back off on 429 and keep each operation small.
- **Expected errors** come back typed in the mutation payload's `errors` (`__typename`, `message`). **Unexpected errors** come back as top-level `errors` with `extensions.code: INTERNAL_SERVER_ERROR` and `data: null`; N-able's guidance is to wait and retry. The gateway can also return a plain-text **503** (`upstream connect error …`). The migration client retries 429, 503 and `INTERNAL_SERVER_ERROR` for **reads only**. It never retries `assetsScriptRun` or another create automatically: a failed-looking launch may still have created tasks, so the tracker checks for them before relaunching.

### Permissions the token's user needs

Viewing, managing and running scripts (`monitoring_scripts_view`, `monitoring_scripts_manage`, `monitoring_scripts_execute`), plus tag management for stage tags. N-voke reports a missing permission from the typed errors (`ScriptRunNotPermittedError`) and offers the REST path for those devices.

## N-central REST (`https://<server>/api/...`)

Authentication: `POST /api/auth/authenticate` with `Authorization: Bearer <user API JWT>` returns an access token (3,600 s) and a refresh token (90,000 s), confirmed in the lab. `POST /api/auth/refresh` renews it. A server restart invalidates sessions, so the client must re-authenticate on 401.

| Endpoint | Use | Status |
|---|---|---|
| `GET /api/server-info` | Version check (target: 2026.4 or later). **Needs a bearer token** (401 without). Returns `ncentral` (e.g. `2026.4.0.25`) and the API `version`. `POST /api/server-info/extra/authenticated` needs a credential body and isn't used | ✅ (lab) |
| `GET /api/service-orgs`, `/api/customers`, `/api/sites`, `/api/org-units`, `/api/org-units/{id}/children` | Org trees and numeric IDs. `children` is one level; units carry `orgUnitType` (`SO`/`CUSTOMER`/`SITE`), `parentId` and `externalId` (optional; often blank) | ✅ (lab) |
| `GET /api/customers/{id}/registration-token`, `/api/sites/{id}/registration-token`, `/api/org-units/{id}/registration-token` | Destination registration token. All three return the same data. No token: `200` with `registrationToken: null`. Service org: `403 RegistrationTokenNotAccessibleException`. Expiry looks like `2026-11-17T23:59:00-05:00[America/New_York]`. Generate one with the installer endpoint below | ✅ (lab) |
| `GET /api/devices`, `/api/devices/{id}`, `/api/org-units/{id}/devices`, `/api/device-filters` | Device lists; `lastApplianceCheckinTime`, `deviceClass`, `supportedOs`, `osId`, `customerId`, `siteId`. `/org-units/{id}/devices` includes descendants | ✅ (lab) |
| `GET /api/devices/{id}/assets` | Matching: serial `computersystem.serialnumber`, host name `computersystem.netbiosname`, MAC `networkadapter.list[].macaddress`, SMBIOS UUID `_extra.computersystem.uuid` | ✅ (lab) |
| `GET /api/customers/{id}/software/installers?softwareType=Agent&installerType=Generic` | Lists installers; the Windows Agent is `softwareId` 101 (`operatingSystem: winnt`). Accepts a site ID as `{id}` | ✅ (lab) |
| `POST /api/customers/{id}/software/installers` body `{ "softwareId": "101" }` | **Generates the registration token** for that customer or site if it has none (returns `201 { downloadLink, expirationDate }`). Works for customers and sites. The link is a customer-specific installer that downloads without authentication and expires in 3 days; N-voke doesn't use it | ✅ (lab, write) |
| `GET`/`PUT /api/devices/{id}/custom-properties/{propertyId}` | Mirror stages for REST-path devices; one property per call | ✅ in spec |
| `GET`/`PUT /api/org-units/{id}/custom-properties/{propertyId}` | Optional: write the mapping back, like the PowerShell prep script | ✅ in spec |
| `POST /api/scheduled-tasks/direct` | Run a repository item on one device. Body: `name`, `itemId`, `taskType` (`AutomationPolicy`, `Script`, `MacScript`), `customerId`, `deviceId`, `credential` (`LocalSystem`), `parameters[]`. For `taskType: "Script"` the inputs go in one parameter, `{ "name": "CommandLine", "value": "-Action Launch …" }` | ✅ in spec |
| `GET /api/scheduled-tasks/{id}`, `/status`, `/status/details` | Task result per device: `status`, `output`, `message`; **no separate exit code** | ✅ in spec |
| `GET /api/devices/{id}/scheduled-tasks` | Tasks for a device | ✅ in spec |
| `DELETE /api/devices/{id}?removeAgents=false` | Remove the old device record from the source after migration | ✅ in spec |
| `POST /api/device`, `GET /api/devices/{id}/activation-key` | Pre-create the destination device and get an activation key (optional variant) | ✅ in spec |
| Upload or list repository items | — | **Not in the 2026.4 REST API.** The REST path's script is uploaded by hand and the operator enters its item ID |

**Limits.** Concurrency is limited per endpoint and excess calls get HTTP 429. The default is 50; heavier endpoints are lower (for example `GET /api/devices` 5, `POST /api/scheduled-tasks/direct` 30, `POST /api/device` and activation keys 1). Page size defaults to 50, maximum 1000. The client keeps a semaphore per endpoint and backs off on 429.

**Item setting.** A repository item can only be run through `POST /api/scheduled-tasks/direct` when **Enable API** is on in its repository settings. The operator turns it on during the manual upload.

## N-central SOAP: not used

The plan originally used SOAP to upload and find an AMP. AMPs are dropped ([Delivery paths](04-delivery-paths.md)), registration tokens can be generated through REST, and the version is available from REST after authentication, so N-voke needs no SOAP client. What the lab showed, for reference: authentication with an empty username and the JWT works (`customerList`, `deviceList`, unauthenticated `versionInfoGet`), elements must be namespace-qualified, and `repositoryItemList` faults with `2100 … null` on both servers. Whether `repositoryItemPublish` accepts a `.ps1` (which would automate the REST path's upload) is untested.

## Windows Agent installer

| Property | Meaning |
|---|---|
| `SERVERADDRESS`, `SERVERPORT`, `SERVERPROTOCOL` | Destination server |
| `CUSTOMERID`, `CUSTOMERSPECIFIC=1` | Destination customer or site |
| `REGISTRATION_TOKEN` | Registration token for that customer or site |
| `AGENTACTIVATIONKEY` | Alternative to the above when the device was pre-created |

Generic installer URL: `https://<server>/download/current/winnt/N-central/WindowsAgentSetup.exe`. Silent install: `/s /v" /qn …"`.
