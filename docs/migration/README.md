# N-central agent migration: plan

**Status: Phase 0 (proof of concept) done; Phase 2 (core clients and model) built, awaiting a live run; Phase 1 (script hardening) coded and tested, awaiting a lab run.** The approach is proven end to end in a lab (whole device moved, both directions, same-server reinstall); see the [Phase 0 findings](10-phase0-findings.md), whose summary is the quickest way in. The core now has the N-central REST client, the mapping, planner and project store, and `nvoke migrate-plan` prints a plan without changing anything; the terminal app has a first **Migrate** page (project list, live dashboard, launch, probe, report); creating, mapping and planning a project are still command-line only, and the desktop app has nothing yet. The working endpoint scripts and test harness are in [`scripts/migration/`](../../scripts/migration/README.md). These documents describe how N-voke grows from "run a script and watch it" into a tool that moves Windows devices from one N-central server to another (or reinstalls the agent on the same server), and tracks every device until it is checked in and healthy on the destination.

## The problem in one paragraph

Redeploying an N-central agent means running `WindowsAgentSetup.exe` against the destination server. When the installer sees an existing agent that isn't a version upgrade, it removes it first: the agent, its maintenance services, and the newer Unified Agent (UFA) component. When it *is* an upgrade, it re-points the agent in place and leaves the UFA enrolled with the old server's Ecoverse tenant, so N-voke cleans up every component itself before installing. Anything launched through either agent (an AMP, or an N-able script run) is killed partway through. The proven workaround is to hand the install to a Windows scheduled task running as SYSTEM, which survives the uninstall, survives reboots and retries until the new agent is up. Today that takes manual Base64 encoding, AMP editing, credentials baked into scripts and spreadsheets to track progress. This plan removes all of that.

## What N-voke will do

1. **Connect** to N-able (Ecoverse GraphQL, which N-voke already uses) and to the source and destination N-central servers (REST).
2. **Map** source customers and sites to destination customers and sites, with automatic name matching and manual correction.
3. **Prepare** the endpoint script once: N-voke publishes it as an N-able script (Ecoverse) automatically. For the REST fallback, the operator uploads the same PowerShell file to each N-central server's repository once.
4. **Plan waves:** choose devices, see which delivery path each one will use, and see the warnings before anything runs.
5. **Launch** each wave. Each device gets only the destination address, customer ID and registration token, never an API key. On the device, a scheduled task removes every N-central component and installs the destination's agent, so the whole device (N-central agent and Ecoverse agent) moves.
6. **Track** every device through fixed stages, from launch to healthy on the destination to cleaned up, with stall detection, the device's own log collected afterwards, and a report.

## Documents

| # | Document | What it covers |
|---|---|---|
| 1 | [Problem and approach](01-problem-and-approach.md) | Why the scheduled task is still needed, alternatives considered, what we learned |
| 2 | [Architecture](02-architecture.md) | How this fits into `nvoke-core`, the desktop app and the terminal app |
| 3 | [API reference](03-api-reference.md) | Every Ecoverse and REST operation the feature needs, and what has been verified |
| 4 | [Delivery paths](04-delivery-paths.md) | Ecoverse first, REST fallback, and how each device's path is chosen |
| 5 | [Endpoint scripts](05-endpoint-scripts.md) | The launcher, installer, status file and cleanup scripts that run on devices |
| 6 | [Tracking and progress](06-tracking.md) | Device stages, failure states, detection, polling and reporting |
| 7 | [User experience](07-user-experience.md) | The migration workflow in the desktop and terminal apps |
| 8 | [Security](08-security.md) | Credentials, what reaches endpoints, audit, destructive actions |
| 9 | [Roadmap and open questions](09-roadmap.md) | Phases, acceptance criteria, the proof-of-concept test plan and open risks |
| 10 | [Phase 0 findings](10-phase0-findings.md) | What the lab tests showed, and how it changes the plan |

## Goals

- One app, two interfaces (desktop and terminal), the same way N-voke works today.
- No hand-editing of scripts, no Base64 copy and paste, no AMP editing.
- No API credential ever reaches an endpoint.
- Every device's progress is visible, and anything stuck is flagged with a likely cause.
- Works with or without Ecoverse: Ecoverse when a device is reachable through it, N-central REST when it isn't.
- Safe by default: dry-run, previews, confirmation before each wave, an audit entry for every change.

## Non-goals (for now)

- Migrating configuration (customers, sites, rules, filters, users, custom property definitions). The separate RMM-to-N-central migrator project covers estate migration; this feature only moves agents.
- macOS and Linux agents. The endpoint scripts are Windows-only to start with; other operating systems are listed as manual.
- Deploying to devices that neither N-able agent can reach. These are reported for out-of-band handling (GPO, Intune, on site).

## Lineage

The endpoint side builds on the PowerShell project [nc-agent-reinstall-schtask](https://github.com/theonlytruebigmac/nc-agent-reinstall-schtask) (scheduled-task handoff, `installAgent.ps1`, cleanup AMP, `Build-MigrationAmp.ps1`). Its scripts are reused and hardened as described in [Endpoint scripts](05-endpoint-scripts.md).

## Glossary

| Term | Meaning |
|---|---|
| **Source / destination** | The N-central server a device leaves and the one it moves to. They can be the same server for a plain reinstall. |
| **Ecoverse** | N-able's cloud platform and its GraphQL API at `api.n-able.com/graphql`, which N-voke already talks to. |
| **UFA** | The Unified Agent component that runs Ecoverse script tasks on a device. The N-central agent installs and removes it. |
| **AMP** | An N-central Automation Manager policy. The PowerShell project used AMPs; N-voke doesn't (both paths run plain PowerShell). |
| **Launcher** | The short script that runs through an agent, stages files and registers the scheduled task, then exits. |
| **Wave** | A batch of devices launched together. |
