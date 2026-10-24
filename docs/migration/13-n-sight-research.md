# 13. N-sight research

**Status: research only, nothing built or tested.** Could N-voke also move Windows devices from **N-sight RMM** to N-central? This page keeps track of what we know, what it would change, and what still has to be proven. Everything here comes from N-able's published documentation (researched 2026-10-24) unless it says otherwise.

**Status key** (as in the [API reference](03-api-reference.md)):
- ✅ **(lab)**: called against a lab account.
- 🔶 **Documented:** in N-able's documentation, but not yet tried.
- ❓ **To verify:** inferred; must be confirmed in a lab.

## Summary

It looks possible, and most of the current design carries over. N-sight's own API (the *Data Extraction API*) is mostly read-only and can't run the migration by itself. But N-sight now has an Ecoverse side, and that side runs on **the same on-device agent N-voke already uses as the UFA**:

- N-sight's **Modern Agent** is `msp-agent-core.exe`, the `MspAgent` service, installed in `C:\Program Files (x86)\Msp Agent`. These are the same names that `installAgent.ps1` already handles as the UFA (*MSP Core Agent*). It is installed automatically with N-sight Windows agent 10.14.2 or later. 🔶
- N-sight's new **Scripting view** sits under **Ecoverse > Views > Scripting**. It has a script repository, input and output parameters, script tasks, and **Execute when online**, which match `scriptCreate` and `assetsScriptRun` with `shouldRunWhenOnline`. 🔶
- N-able's GraphQL and MCP documentation says `api.n-able.com` covers N-sight RMM data, including running scripts on devices. 🔶

If that holds, the [Ecoverse path](04-delivery-paths.md) works on N-sight devices unchanged: `assetsScriptRun`, then the SYSTEM scheduled task, then `WindowsAgentSetup.exe` against the destination N-central. What's new is the source side (inventory, matching and removing the N-sight agent). Unlike N-central, N-sight has no REST fallback.

## N-sight Data Extraction API

Documentation: [Getting started](https://developer.n-able.com/n-sight/docs/getting-started-with-the-n-sight-api), [service index](https://developer.n-able.com/n-sight/llms.txt).

- **Request:** `GET https://{server}/api/?apikey={key}&service={name}&{params}`. The response is XML. It is not OData. Adding `describe=true` to any call returns its description and fields.
- **Server:** depends on the account's region (for example `www.am.remote.management` for the Americas, `www.systemmonitor.us` for the United States, `www.systemmonitor.co.uk` for the United Kingdom). A custom dashboard domain can be resolved with `nslookup` to find it.
- **Key:** one per account (*Settings > General Settings > API*). Generating a new key breaks everything using the old one. A key made with the Agent Key assigned to a client group only sees that group's clients.
- **Rate limit:** about **60 calls a minute**. N-able may contact accounts whose usage affects performance.

| Service | Use for migration | Status |
|---|---|---|
| `list_clients` | Source customers (`clientid`, name) | 🔶 |
| `list_sites&clientid=` | Source sites under each client (`siteid`, name) | 🔶 |
| `list_devices_at_client&clientid=&devicetype=server\|workstation` | Every device of one type at a client, ordered by site. **One call per client per device type**, so this is the cheapest way to take inventory | 🔶 |
| `list_servers&siteid=`, `list_workstations&siteid=` | Per-site device lists. Fields include `name`, `mac1`–`mac3`, `device_serial`, `online`, `agent_version`, `os`, `assetid` and the last check-in times (`utc_time_247`, `dsc_utc_time`). This is enough to match old and new devices | 🔶 |
| `list_device_asset_details` | Hardware details for one device | 🔶 |
| `add_client`, `add_site`, `get_site_installation_package` | Not needed. The destination is N-central | 🔶 |
| `task_run_now&checkid=` | Starts an **existing** Automated Task now. It has no per-run inputs, and the task must first be set up in the dashboard | 🔶 |
| Run an arbitrary script with inputs | **None** | 🔶 |
| Delete a device | **None**. Devices can only be deleted in the dashboard (and deleting one there also uninstalls its agent) | 🔶 |

At 60 calls a minute, a full inventory of 500 clients takes about 1,500 calls (`list_sites` plus two `list_devices_at_client` per client), which is about 25 minutes. That's fine for a cached survey, but too slow for live tracking. Live state should come from Ecoverse.

## N-sight in Ecoverse

| What | Detail | Status |
|---|---|---|
| Modern Agent | Windows service `MspAgent` (*MSP Agent*), `msp-agent-core.exe`, `C:\Program Files (x86)\Msp Agent`. It runs alongside the N-sight agent, which checks that it is present and working. It activates in the background and updates itself | 🔶 |
| Minimum versions | Windows agent 10.14.2, macOS 3.10.0, Linux 2.2.0 | 🔶 |
| Running scripts | From the Assets view, on one device or many. You choose **Credentials** (the account to run as), an **Execution timeout**, and input and output parameters. **Execute when online** queues tasks for offline devices as *Pending* | 🔶 |
| Permissions | Admin and SuperUser can do everything. Other roles need *Ecoverse > Scripting view* permissions (*Manage*, *View*, *Run Script*), limited to their scope | 🔶 |
| GraphQL `assetSearch` returns N-sight assets | The documentation says N-sight data is in the API, but doesn't show the fields | ❓ |
| What identifies an N-sight asset | N-central assets have `ncentralDevice { deviceId server { origin } }`. Does an N-sight asset have its own field, or only `systemInfo` plus the organisation? | ❓ |
| `assetsScriptRun` accepts N-sight assets | Is a `credential` required? N-sight's run dialog asks for one | ❓ |
| Tenant | Is N-sight in the same Ecoverse tenant as the partner's N-central, or a separate one that needs its own key? | ❓ |

GraphQL introspection is disabled (see the [API reference](03-api-reference.md)). Check the fields in the published schema in Apollo Explorer, or with the N-able MCP `introspect` and `search` tools.

## Removing the N-sight agent

| What | Detail | Status |
|---|---|---|
| Product | *Advanced Monitoring Agent*, in Add/Remove Programs | 🔶 |
| Silent uninstall | `winagent.exe /removequiet`, run from the agent folder (there are separate 32-bit, 64-bit and Group Policy install paths) | 🔶 |
| Uninstall protection | When it's turned on, a manual uninstall needs the uninstall code, and **the silent uninstall fails** | 🔶 |
| What it removes | *"All features and checks deployed by the Agent are removed."* The documentation doesn't list them. Does it remove Managed AV (Bitdefender), Take Control, Web Protection, and the Modern Agent with its data folder? | ❓ |
| Dashboard record | After a local uninstall the device stays in the dashboard (and N-able advises deleting it only once the agent is fully removed). Deleting a device in the dashboard uninstalls the agent, which can take up to an hour | 🔶 |

## What changes compared with N-central to N-central

| Step | Change |
|---|---|
| Connect | A new source kind, **N-sight**: region server and API key (in the keychain), plus the Ecoverse token for the tenant the N-sight devices are in. The destination stays N-central REST |
| Map | N-sight client → N-central customer, N-sight site → N-central site. Name matching, manual correction, and creating missing customers and sites stay as they are |
| Prepare | Unchanged on the destination. The script is published to the N-sight devices' Ecoverse tenant. There is no repository upload to N-sight, because there is no REST path |
| Plan | The planner joins Ecoverse assets on `(server origin, N-central device ID)`. An N-sight source needs another join, either an N-sight field on the asset or a match with N-sight inventory by host name, serial and MAC. **There is no REST path**: a device with no connected Modern Agent is **Unreachable** and is handled outside N-voke |
| Endpoint | `installAgent.ps1` needs an N-sight branch. The N-central installer won't remove an N-sight agent (they are different products), so the script must remove it with `winagent.exe /removequiet` and then remove the MSP Core Agent as it does today (`msp-agent-core.exe --uninstall`, then the leftover folders). The source is detected by the *Advanced Monitoring Agent* product or service instead of `ServerConfig.xml` |
| Track | The source side ("left the source") comes from Ecoverse disconnection, or from `online=0` in N-sight. The destination side is unchanged |
| Finish | Devices stay in the N-sight dashboard (offline, and possibly alerting and counted) until they are deleted by hand. The report lists them so they can be deleted in bulk |

### Why the REST fallback doesn't carry over

`task_run_now` only starts a task that already exists in the dashboard, with fixed parameters. To use it, the operator would have to create an Automated Task per site with that site's destination customer ID and registration token in its command line. That doesn't scale to thousands of sites, and it breaks the rule that each device receives only its own token ([Security](08-security.md)). A single shared task holding the whole mapping table would put every token on every device. So for N-sight, a device without the Modern Agent is out of scope.

### Make before break?

N-central to N-central has to remove the old agent first, because the installer does it anyway. N-sight and N-central agents are different products, so the endpoint script could **install the N-central agent first and then remove N-sight**, and the device would never be left without an agent. The catch is that both products use the same `Msp Agent` folder and identity, and the UFA joins the tenant of the local agent that provisions it ([API reference](03-api-reference.md)). This needs a lab test (NS4).

## Open questions

| # | Status | Question | How to answer |
|---|---|---|---|
| NS1 | ❓ Open | Does `assetSearch` return N-sight assets, and which fields identify them (an N-sight device ID, client and site, `systemInfo`)? | Published schema or MCP `introspect`, then one query with an N-sight-linked token |
| NS2 | ❓ Open | Does `assetsScriptRun` run on an N-sight asset with the current inputs? Is `credential` required, and what does `LocalSystem` map to? Is the script 32-bit or 64-bit? | Publish the current script to the N-sight tenant and run `Status` on one lab device |
| NS3 | ❓ Open | Are N-sight and N-central in one Ecoverse tenant for a partner with both, or two? | Check a real account; the credential store already holds more than one Ecoverse key |
| NS4 | ❓ Open | Install first or remove first? What happens when the N-central Windows Agent is installed while N-sight's Modern Agent is still enrolled? Does it take the UFA over, leave it on N-sight's tenant, or conflict? | Lab: install both and watch `component.log` and the asset in each tenant |
| NS5 | ❓ Open | What does `winagent.exe /removequiet` leave behind (Managed AV, Take Control, Web Protection, the Modern Agent and `ProgramData\MspAgent`)? Do the leftovers bring back the stale-identity `400 The SSL certificate error` seen in Phase 0? | Lab: uninstall, then list products, services and folders |
| NS6 | ❓ Open | Uninstall protection: where does the code come from, how is it passed to `winagent.exe`, and is it per account or per device? If needed, it becomes a `PASSWORD` input | N-sight documentation or support, then lab |
| NS7 | ❓ Open | Does an Ecoverse task queued on an N-sight asset behave like one on N-central (stalls, expiry, `ScriptRunAssetNotFoundError` timing)? | Lab, alongside [Q15](09-roadmap.md#open-questions) |
| NS8 | ❓ Open | Take Control: does N-sight's Take Control conflict with the one the N-central agent installs? | Lab |
| NS9 | ❓ Open | Is there an API or bulk way to delete migrated devices from N-sight, or is the dashboard the only way? | Support; the published API has none |

## Suggested first lab test

One Windows device in N-sight with Modern Agent 10.14.2 or later, and a lab N-central destination:

1. Answer NS1 and NS3 with one `assetSearch`.
2. Publish the current `nvoke-migration.ps1` to the N-sight tenant and run `Action=Status` (NS2).
3. Remove the N-sight agent by hand with `/removequiet` and record what's left (NS5).
4. Run `Launch` with an N-sight cleanup added by hand to `installAgent.ps1`, and follow it to healthy on the destination.

If that works, the code changes are mostly a new source adapter (N-sight inventory and asset matching), a source kind on the project, and an N-sight branch in the cleanup. Tracking, Map, Prepare and the destination side stay as they are.

## Sources

- N-sight API: [Getting started](https://developer.n-able.com/n-sight/docs/getting-started-with-the-n-sight-api), [Determine the server](https://developer.n-able.com/n-sight/docs/determine-server-for-api-query), [Generate an API key](https://developer.n-able.com/n-sight/docs/generate-an-api-key), [Run Task Now](https://developer.n-able.com/n-sight/docs/run-task-now), [List devices at client](https://developer.n-able.com/n-sight/docs/list-devices-at-client), [List workstations](https://developer.n-able.com/n-sight/docs/listing-workstations)
- N-sight Ecoverse: [Modern Agent](https://documentation.n-able.com/remote-management/userguide/Content/Modern-agent/modern-agent.htm), [Scripting view](https://documentation.n-able.com/remote-management/userguide/Content/Views/scriptrepo.htm), [Scripting requirements](https://documentation.n-able.com/remote-management/userguide/Content/Views/scriptrepo-sysreqs.htm), [Run scripts from the Assets view](https://documentation.n-able.com/remote-management/userguide/Content/Views/Assets_runscript.htm), [Assets view requirements](https://documentation.n-able.com/remote-management/userguide/Content/Views/Assets_sysreqs.htm)
- N-sight agent: [Uninstall the Windows Agent](https://documentation.n-able.com/remote-management/userguide/Content/uninstall_the_agent.htm), [Delete clients, sites or devices](https://documentation.n-able.com/remote-management/userguide/Content/deleteaserversiteorclient.htm)
- N-able GraphQL: [N-able MCP](https://developer.n-able.com/gql/docs/mcp), [MCP capabilities](https://developer.n-able.com/gql/docs/mcp-capabilities)
