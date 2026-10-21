# 11. Desktop app design: the Migrate page

Status: **draft for review** (2026-10-20). Nothing in this document is built. It is the design the desktop app is built from, and the terminal app follows it afterwards ([What the terminal app takes from it](#what-the-terminal-app-takes-from-it)).

It turns the workflow in [User experience](07-user-experience.md), the stages in [Tracking](06-tracking.md) and the data the core already has (`crates/core/src/migration/`) into screens. Where the core has no answer yet, the text says **Core gap**.

## Decisions already made

| Decision | Choice |
|---|---|
| How to build it | Design first (this document), then build in slices |
| Live updates | The Rust side runs the tracker while a project is open and **pushes events**; the page redraws from them |
| TypeScript types | **Generated from the Rust types** (one source of truth); no migration types written by hand |
| Where the logic lives | All migration logic in `nvoke-core` ([Architecture](02-architecture.md#a-deliberate-change-migration-logic-is-rust-only)); React renders state and sends actions |

## Principles

These are the existing app's habits, kept on purpose so Migrate feels like the rest of N-voke:

1. **Review before anything changes.** Every write (publish, generate tokens, launch, cleanup, retire) shows what it will do first, then asks.
2. **Dry-run is a mode, not a screen.** The same buttons, a banner, and the request list instead of the effect.
3. **Say what happened, why, and what to do.** No stage names or error codes on their own; each state has a sentence ([Words](#words)).
4. **Group by outcome.** Counts by stage first, then the devices that need a person, then everything else.
5. **Waiting is a normal state.** A new Ecoverse agent takes minutes to tens of minutes to catch up ([Known delays](06-tracking.md#known-delays)). The page says what is pending and roughly how long, and never calls it stuck before it is.
6. **Dense and calm.** The existing 13 px scale, sidebar navigation, tokens (`bg-surface`, `text-muted`, `ok`/`warn`/`err`), light and dark, `PageHeader`, `Table`, `Status`, `Sheet`, `Modal`, `Segmented`, `EmptyState`.

## Where it lives

A new **Migrate** item in the sidebar, under Run, Activity and Tags:

```
 N-voke
 ─────────────
 ▶ Run
 ≡ Activity            2
 ⇄ Migrate            14     <- count of devices that need a person
 # Tags
 ▤ Audit log
 ─────────────
 ⚙ Settings
```

Inside, two levels: the **project list**, and a **project workspace** (full page, with a back link to the list). The workspace opens on the step the project is on (Track once anything has launched).

```mermaid
flowchart LR
    L[Project list] -->|New project| N[New project]
    L -->|Open| W[Project workspace]
    N --> W
    W --> S1[Connect] --> S2[Map] --> S3[Prepare] --> S4[Plan] --> S5[Launch] --> S6[Track] --> S7[Finish]
    S6 -->|retry / relaunch| S5
    S6 -->|next wave| S4
    L -.-> V[Servers sheet]
    W -.-> V
```

## Screens

### A. Project list

```
┌ Migrate ───────────────────────────────────────────────────────── [ + New project ] ┐
│                                                                                      │
│  Name                    From → To                 Progress              Needs you   │
│  ─────────────────────────────────────────────────────────────────────────────────  │
│  Contoso move            old.example.com →         ████████░░░░  118/160      3 ▸   │
│                          new.example.com           wave 3 running · 4 min ago        │
│  Fabrikam reinstall      lab.example.com →         ██████████████  24/24      ✓     │
│                          lab.example.com           closed · 2 days ago               │
│  Acme pilot              old.example.com →         ░░░░░░░░░░░░    0/5        –     │
│                          new.example.com           planned · not launched            │
│                                                                                      │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

- One row per project: name, servers, a progress bar of devices in a final or healthy stage, the **needs you** count (attention stages), and last activity. Opening a row opens the workspace.
- A project open in another N-voke window shows *Open in another window* and opens read-only ([Lease](#lease-and-two-windows)).
- Empty state: *No migrations yet. A project moves devices from one N-central server to another, or reinstalls them on the same one.* with **New project**.
- Row menu: Open, Duplicate settings, Export report, Delete (confirmation: *removes the local history; the audit log keeps its entries*).

### B. New project

A `Modal`: a name, two servers, how devices are reached, and how the destination relates to Ecoverse:

```
┌ New project ──────────────────────────────────────────────────────┐
│ Name            [ Contoso move                               ]    │
│ Move devices from   [ old.example.com        ▾ ]  Manage servers  │
│ to                  [ new.example.com        ▾ ]  (same server = reinstall) │
│                                                                    │
│ How should devices be reached to start the move?                   │
│  (•) Ecoverse first, N-central as the fallback                     │
│  ( ) N-central only                                                │
│                                                                    │
│ Is the destination linked to Ecoverse?                             │
│  (•) Not yet        Devices leave Ecoverse until it's linked.      │
│                     Progress after the move is tracked over REST.  │
│  ( ) Same tenant as the source                                     │
│  ( ) Another tenant   (needs a token saved for the destination)    │
│                                                                    │
│  Ecoverse organizations to search on the source (optional)         │
│  [ choose organizations ▾ ]                                        │
│                                                  [Cancel] [Create] │
└────────────────────────────────────────────────────────────────────┘
```

- Both server pickers list the saved servers and end with **Add server…** (opens [the servers sheet](#c-servers-sheet)).
- Choosing the same server for both switches the wording to *reinstall on the same server* and shows the review/clean options later accordingly.
- The "linked to Ecoverse" choice shows the **prerequisites note** when anything other than *Not yet* is picked: cloud eventing must be on and SSO enabled for the destination to deploy Ecoverse agents.
- **Ecoverse organizations are optional.** Empty means Ecoverse is searched across every organization the key can see (checked in the lab: `assetSearch` works without a filter), and devices are matched to the source server by its address. Picking organizations only narrows the search. A source server with its own token has no picker, since the search would use the wrong key. Publishing the endpoint script needs one organization; that is asked for in Prepare.
- **Delivery** is saved on the project (`ProjectOptions.delivery`). *N-central only* never reads Ecoverse and hides the organization picker. When the source server is set to not use Ecoverse, the dialog says its devices go through N-central.
- *Another tenant* is disabled until the destination server has an Ecoverse token of its own (see [the servers sheet](#c-servers-sheet)); the destination's Ecoverse is then read with that token.
- *Create* saves the project and opens it at **Connect**.

### C. Servers sheet

A right-hand `Sheet` (like the existing settings/detail sheets), reachable from the picker and from Settings:

```
┌ N-central servers ──────────────────────────────── ✕ ┐
│  Old server  JWT saved · Ecoverse: app key  [Test] [Edit] │
│    ✓ signed in · N-central 2026.4.0 · API 1.3.23          │
│  New server  JWT saved · Ecoverse: own token [Test] [Edit]│
│  Plain       JWT saved · REST only           [Test] [Edit]│
│                                         [ + Add ]   │
└─────────────────────────────────────────────────────┘
   Add / edit:  Name [        ]  Host [               ]
                User API JWT [••••••••]  (leave blank to keep)
                Ecoverse key for this server's devices
                  (•) The app's Ecoverse key   ( ) A token for this server [••••]   ( ) Don't use Ecoverse
                [Add and test]   ✓ signed in · version
```

- **Test** signs in, reads the version and confirms the endpoints the migration needs. Results feed [Connect](#1-connect).
- The host is normalized and validated by the core (`normalize_host`: HTTPS only, no port or path). The JWT goes to the keychain and never comes back to the web view (only `hasJwt`).
- **The Ecoverse key is chosen per server.** *The app's key* (default) is the key in Settings; use it when the server is in the same Ecoverse tenant. *A token for this server* is its own Ecoverse token, for a server in another tenant; it goes to the keychain (`ecoverse:<server id>`), never comes back to the web view, and is deleted when the choice changes or the server is removed. *Don't use Ecoverse* makes every device of that server a REST device.
- **The JWT is always needed**, whichever Ecoverse key is used: org units, registration tokens, device lists and the REST fallback exist only in N-central. The sheet says so.
- The core scopes a token to one call (`with_ecoverse`), so a server's token is never used for another server's requests. A server set to *Don't use Ecoverse* refuses an Ecoverse call and says how to change it.
- Removing a server a project uses is refused, with the project names.

### 1. Connect

The first step of a project: a checklist of everything the project depends on, each green, amber or red with a one-line fix.

```
┌ Connect ───────────────────────────────────────────────────────────────────────────┐
│  ● Source old.example.com        signed in · 2026.4.0 · endpoints respond          │
│  ● Destination new.example.com   signed in · 2026.4.1 · endpoints respond          │
│  ● Ecoverse                      sees 412 devices on the source server             │
│  ▲ Destination token access      The destination JWT's user can't read             │
│                                  registration tokens for 2 of 6 customers.         │
│                                  Give that user access, or use another JWT.        │
│  ▲ Version                       2026.3 is older than the tested releases (2026.4).│
│                                                                                    │
│                                                   [ Re-check ]   [ Continue ▸ ]    │
└────────────────────────────────────────────────────────────────────────────────────┘
```

- Red blocks **Continue**; amber doesn't.
- Data: server summaries, versions and `survey.warnings`; the Ecoverse line is `ecoverse_linked`.

### 2. Map

Source units on the left, destination on the right, one row per source customer or site, so the page is a table with a destination picker, not two trees to drag between (drag is a later addition).

```
┌ Map ───────────────────────────────────────────── [ Tokens ▾ ]  [ Re-run matching ] ┐
│ ▾ 8 mapped   ▲ 2 need a choice   ○ 1 unmatched   – 1 left out            Filter [   ]│
│                                                                                       │
│ Source                               →   Destination                  Match   Token  │
│ ───────────────────────────────────────────────────────────────────────────────────  │
│ ▾ Acme Corporation (customer)        →   Acme Corporation             exact    ● ok   │
│     Acme HQ (site)                   →   Acme HQ                      exact    ● ok   │
│     Acme Branch Office (site)        →   Acme Branch Office           exact    ▲ missing
│ ▾ GlobalRetail Co (customer)         →   GlobalRetail Co              exact    ▲ missing
│     Warehouse (site)                 →   ▲ Choose… (2 fit)             ambiguous       │
│     Downtown (site)                  →   Downtown                     similar  ▲ missing
│ ▾ TechStart Inc (customer)           →   ○ No match   [ Choose ▾ ]                    │
│ ▾ Old Dept (customer)                →   – Left out                    manual          │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

- **Match** shows how it was matched (exact under the parent, exact anywhere, normalized) or *manual*. Normalized matches are shown as *similar*, so the person checks them.
- The destination cell is a combobox over destination units of the same kind (customer or site), with *Leave out* at the end. It writes through `migration_set_mapping` (`MapTo` or `Exclude`) and re-evaluates the row.
- **Token** column: present, expiring on a date, expired, or missing. **Tokens ▾ → Generate missing** previews the units, asks, and runs `migration_generate_tokens` (dry-run holds it back); the result sheet lists generated, already fine, expired (*N-central has to renew these*) and failed with reasons.
- **Built (2026-10-21):** *Create missing…* in the Map header. It previews what would be created (customers, then sites under their customer, and sites it cannot create because their customer has no destination), asks for the destination service organization when there is more than one, says that N-central cannot delete what it creates, and can be run as a dry run. It creates customers first, maps each source unit to what was created, then creates the registration tokens. Only the name is copied. A name that already exists is reported (N-central answers `400 OrganizationUnitAlreadyExistException`, not `409`).
- Ambiguous and unmatched rows block **Plan** for the devices under them; a banner on the next step says so.

### 3. Prepare

What goes to the devices, and the options. Two cards and an options form.

```
┌ Prepare ───────────────────────────────────────────────────────────────────────────┐
│  Endpoint script   0.1.0+fa1d1a0                                                    │
│  ┌ Ecoverse ──────────────────────────────────────────────────────────────────────┐ │
│  │ N-voke migration 0.1.0+fa1d1a0 (a1b2c3)   in "Contoso (source org)"             │ │
│  │ ● Published                                              [ Publish again ]      │ │
│  └─────────────────────────────────────────────────────────────────────────────────┘ │
│  ┌ N-central repository (REST path) ─── 6 devices need it ────────────────────────┐ │
│  │ 1. [ Export script ]  saves nvoke-migration-0.1.0+fa1d1a0.ps1                   │ │
│  │ 2. Upload it to old.example.com's repository, turn on Enable API                │ │
│  │ 3. Item ID  [ 1271448998 ]  version [ 0.1.0+fa1d1a0 ]      ● matches            │ │
│  └─────────────────────────────────────────────────────────────────────────────────┘ │
│                                                                                      │
│  Options                                                                             │
│   Clean mode            (•) Full cleanup   ( ) Install over the top                  │
│   Review period         [ 7 ] days   keep each device's migration record, then it   │
│                         removes itself                                               │
│   Wave size             [ 50 ]       Task repeat [ 15 ] min   Task expiry [ 14 ] days│
│   After the review period  [ ] also delete the old source records (asks again)      │
│                                                                                      │
│                                                    [ Back ]   [ Continue ▸ ]        │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

- **Publish** is one previewed, audited action (`migration_publish`): create or update, or *already up to date* when the hash matches.
- The REST card appears only when some planned device needs it. A script version that differs from the build shows *outdated: delete the repository item, upload the new file, enter the new item ID* (N-central can't replace a file).
- Options write to `ProjectOptions`. Descriptions say what each does in a sentence; *Install over the top* carries the warning that a newer destination agent leaves the device in the source Ecoverse tenant.

### 4. Plan

The review table, like the Run page's: what will happen to each device before it does.

```
┌ Plan ───────────────────────────────────────────── [ Read servers again ] [ Add to project ]
│ 160 devices · ✓ 148 ready · ▲ 9 blocked · 3 left out            Customer [ all ▾ ] [ search ]
│
│  ☑  Device        Customer → destination         Path       Online   Notes
│  ─────────────────────────────────────────────────────────────────────────────────────────
│  ☑  PC-0142       Acme HQ → Acme HQ              Ecoverse   now      ✓
│  ☑  PC-0143       Acme HQ → Acme HQ              Ecoverse   2 h ago  queued until online
│  ☑  SRV-DB01      GlobalRetail → GlobalRetail    REST       now      script not uploaded ▾
│  ☐  MAC-0007      Acme HQ                        –          now      Not a Windows device
│  ☐  PC-0210       TechStart Main                 –          –        Customer isn't mapped
│
│ Blocked reasons:  Unmapped (4) · Token missing (3) · Not Windows (2)   [ Fix in Map ▸ ]
└─────────────────────────────────────────────────────────────────────────────────────────────
```

- Rows come from the plan (`PlannedDevice`): device, customer to destination, delivery path and the reason for it, online state, blockers (stop a launch) and warnings (don't).
- A blocked row can't be ticked; its note is the blocker's message with a button to the screen that fixes it (Map for unmapped or ambiguous, Prepare for a missing script, Map → Tokens for tokens).
- **Path override** per device (Ecoverse or REST) in the row menu, marked *overridden*. **Leave out** per device or per selection.
- *Add to project* is `migration_plan_into`: it adds the ready devices, leaves the others, and reports *added / already in / blocked*. Planning again after a fix adds only what is new.
- **Built (2026-10-21):** the plan comes back one page at a time (`PlanQuery`: show, customer, text, offset, limit; default 100 a page), filtered in the core. The core keeps what it read from both servers for 15 minutes, so paging, filtering and a per-device choice do not read the servers again; *Read servers again* does.

### 5. Launch

Waves are made here, then launched. A wave is a named group of devices.

```
┌ Launch ───────────────────────────────────────────────────────────────────────────┐
│  Waves                                                    [ + New wave ▾ ]          │
│  ┌ Wave 1 · pilot ───────────────────────── 5 devices ── launched 14:02 ── Track ▸ ┐│
│  ┌ Wave 2 ───────────────────────────────── 50 devices ── waiting ─ [ Launch… ] ───┐│
│  ┌ Wave 3 ───────────────────────────────── 50 devices ── waiting ─────────────────┘│
│  48 planned devices are not in a wave yet.          [ Fill next wave from them ]    │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

**New wave** offers *Pilot (2 to 5 devices)*, *Next N devices* (default the project's wave size, grouped by customer), or *Choose devices*.

Launching asks for confirmation, in the existing confirmation pattern:

```
┌ Launch wave 2 ────────────────────────────────────────────────────┐
│  This will remove the old agent from 50 devices and install the   │
│  new one from new.example.com. Devices go offline for a while.    │
│                                                                    │
│   By path:       Ecoverse 44 · REST 6                              │
│   By destination: Acme HQ 20 · GlobalRetail Warehouse 30           │
│   Offline now:   3 (they run when they reconnect)                  │
│   Registration tokens: all present, soonest expiry 2026-11-17      │
│   Clean mode: full cleanup · review period 7 days                  │
│                                                                    │
│   [ ] Dry-run: show the requests, change nothing                   │
│                                        [ Cancel ]  [ Launch 50 ]   │
└────────────────────────────────────────────────────────────────────┘
```

- The counts come from `plan_wave`. A wave with a device whose launch outcome is unknown (*launch_unsure*) refuses to launch again until the person confirms what happened, with that explanation.
- Dry-run shows the exact requests per path with tokens redacted (a `Sheet` with the list) and moves nothing.
- On launch the page switches to Track for that wave. Ecoverse launches also appear in Activity as ordinary runs.

### 6. Track

The home of a running project. Top to bottom: a stage strip, the attention list, the device table.

```
┌ Contoso move ·  old.example.com → new.example.com ─────────────  ● Tracking   [ Poll now ] ┐
│ Planned 48 │ Launched 3 │ Staged 6 │ Moving 12 │ On new server 14 │ Healthy 62 │ Done 15      │
│ ██████░░░░░░░░░░░░░░░░░░░░████████████████████████████████████████████████                   │
│                                                                                             │
│ Needs you (3)                                                                               │
│  ▲ SRV-DB01     Not picked up   Launched 34 min ago and still not running. It may be        │
│                                 offline or its agent is busy.   [ Cancel and relaunch ]     │
│  ▲ PC-0210      Lost            The old agent is gone and nothing has appeared on the new   │
│                                 server after 4 h.   [ Probe ]  [ Mark resolved ]            │
│  ▲ PC-0388      Ecoverse login  The Ecoverse agent can't log in to the cloud. The installer │
│                 rejected        repairs it once; if it persists, check the device's network.│
│                                                                                             │
│ Devices                         Stage [ all ▾ ]  Wave [ all ▾ ]  Customer [ all ▾ ]  [ search ]
│  Device     Customer       Wave  Stage                          Since     Path             │
│  ───────────────────────────────────────────────────────────────────────────────────────── │
│  PC-0142    Acme HQ        1     ● Healthy                       14:31     Ecoverse         │
│  PC-0143    Acme HQ        1     ◐ Moving: installing            14:22     Ecoverse         │
│  PC-0150    GlobalRetail   2     ◐ Staged, waiting to start      14:40     REST             │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Stage strip**: the 16 stages grouped for reading (Planned, Launched, Staged, Moving, On the new server, Healthy, Done, Needs you). Clicking a segment filters the table.
- **Needs you**: one card per attention device with the sentence for its state ([Words](#words)) and the actions that apply ([Actions](#actions)). *Known delays* are not listed here until they pass their thresholds.
- **Device table**: one row per device; a click opens the **device sheet**.
- The header shows the tracker state (*Tracking · last read 12 s ago*, *Paused*, *Read-only: open in another window*) and **Poll now**.

**Device sheet** (`Sheet`, wide):

```
┌ PC-0143 ─ Acme HQ → Acme HQ ────────────────────────────────── ✕ ┐
│  ◐ Moving: installing         Ecoverse · wave 1 · launched 14:02   │
│  Old device  1361454171  (old.example.com)                         │
│  New device  889002710   (new.example.com)   asset 065a…           │
│                                                                    │
│  Timeline                                                          │
│   14:02  Launched          task MDFh… accepted                     │
│   14:02  Staged            scheduled task registered               │
│   14:08  Agent removed     old agent stopped checking in           │
│   14:12  Registered        found on new.example.com                │
│   14:19  Healthy           checking in; Ecoverse agent syncing     │
│                                                                    │
│  Last probe 14:19  (status.json)                                   │
│    cleanup: Windows Agent 0 · MSP Core Agent 1605 · folders removed│
│    Ecoverse agent: provisioned · data sync: waiting (12 min)       │
│                                                                    │
│  [ Probe ] [ Run cleanup now ] [ Retry ] [ Tag… ] [ Run a script… ]│
└────────────────────────────────────────────────────────────────────┘
```

- The timeline is the device's event list; each event is one sentence. Collected `status.json` is summarized, with the raw JSON behind *Show details* (token-free by design).
- Actions are the ones valid for the device's stage; the others are hidden, not disabled.

### 7. Finish

```
┌ Finish ───────────────────────────────────────────────────────────────────────────┐
│  118 of 160 devices are done. 3 need you. Nothing is cleaned up straight away.     │
│                                                                                    │
│  Review period    Devices keep their migration record for 7 days, then remove it   │
│                   themselves.    [ Run cleanup now ▾ ]  for healthy devices        │
│  Old records      After the review period you can delete the old source records   │
│                   (the devices themselves are untouched).   [ Review and delete… ] │
│  Reports          [ Export CSV ]  [ Export HTML ]   Filter: [ all ▾ ]              │
│                                                                                    │
│  The project closes when every device is retired, left out or marked resolved.     │
└────────────────────────────────────────────────────────────────────────────────────┘
```

- *Run cleanup now* and *Delete old records* each preview the list and ask (the second behind its own confirmation, only after the review period, with the note that it also removes their stale Ecoverse assets). **Core gap:** the delete call isn't built yet, so in the first slices this row says *Not available yet* and the old records are tidied by hand.
- Reports are written by the core (`migration_write_report`) to a folder the person picks; the page shows the path.

### Settings additions

In the existing Settings sheet, a **Migration** section: stall thresholds (not picked up 30 min, stuck staged 90 min, lost 4 h, Ecoverse not moved 75 min, check-in recency), poll interval, default options for new projects, and **Servers** (the same sheet as C). Defaults come from the design; changes are per machine.

## Words

One sentence per stage, shown in the table (short form) and the device sheet (long form). Tone: ok = green, warn = amber, err = red, neutral = grey, pulse = a moving dot.

| Stage | Short | Tone | Long (what is happening, what to do) |
|---|---|---|---|
| Planned | Planned | neutral | Ready to launch. |
| Unreachable | Can't reach | err | No agent can reach it now. It's picked up if it comes back; export the list for on-site follow-up. |
| Launched | Launched | neutral, pulse | The launch was accepted. Waiting for the device to start it. |
| Not picked up | Not picked up | warn | Launched {n} min ago and still not running. It may be offline or busy. Cancel and relaunch, or use the other path. |
| Launch failed | Launch failed | err | The launch was refused: {reason}. Retry, or retry on the other path. |
| Staged | Staged | neutral, pulse | The script ran and scheduled the install. It starts within a couple of minutes. |
| Stuck staged | Stuck | warn | Staged {n} min ago but the old agent is still checking in. Probe it to see why. |
| Agent removed | Moving | neutral, pulse | The old agent is gone; the new one is installing. This takes about 10 to 15 minutes. |
| Lost | Lost | err | The old agent is gone and nothing appeared on the new server after {n}. Probe it; plan on-site follow-up. |
| Registered | On the new server | neutral, pulse | Found on {destination}. Waiting for its first check-in. |
| Wrong place | Wrong place | warn | Registered under {actual}, not {expected}. Fix the mapping or leave it. |
| Healthy | Healthy | ok | Checking in to {destination}. |
| Cleaned up | Cleaned up | ok | Its migration record is removed. |
| Retired | Done | ok | Finished, and the old record is retired. |
| Excluded | Left out | neutral | Taken out of the project. |
| Resolved | Resolved | neutral | Fixed outside N-voke. |

**Sub-states that add a second line under Healthy** (from [Known delays](06-tracking.md#known-delays), not new stages):

| Situation | Line |
|---|---|
| Destination linked, asset not yet connected | *Ecoverse agent still setting up (installed {n} min ago). This commonly takes 20 to 60 minutes and doesn't block the move.* |
| Asset connected, no data yet | *Ecoverse is waiting for the agent's data. Scripts run once it arrives.* |
| Healthy, and the destination's Ecoverse has **no asset data** yet | The table reads **Checking in** (not Healthy), with *Waiting for Ecoverse data* under it, and the sheet says: *Registered with Ecoverse and shown there, but no asset data has arrived yet. Until it does the device isn't fully working: its name, serial number and addresses are unknown and scripts can't run on it.* **Healthy** is shown only once Ecoverse is receiving the device's data (a host name, serial number or address). The stage in the core is still Healthy; the page tells the two apart with `waitingForEcoverse`. Waiting devices are counted under *Waiting for Ecoverse* on the strip, don't count toward progress, and are left out of the bulk cleanup |
| Login rejected / components denied (from `status.json`) | *The Ecoverse agent can't {log in / download its components}. The installer repairs it once after 30 minutes; if it persists, check the device's network.* |
| Destination not linked | *Not in Ecoverse until the destination is linked. This is expected.* |

## Actions

| Action | Stage(s) | What it does | Confirms? |
|---|---|---|---|
| Poll now | project | One tracker pass immediately | no |
| Probe | Stuck staged, Healthy, Registered | Runs `Status` through whichever agent reaches it and shows `status.json`, the task state and the log tail | no |
| Cancel and relaunch | Not picked up | Cancels the Ecoverse task and returns the device to Planned (the core's `Retry`; the task cancel itself is **Core gap** to check) | yes |
| Retry | Launch failed, Not picked up, Stuck staged, unsure launches | Back to Planned, then launch again | yes |
| Retry on other path | Launch failed, Stuck staged | Flips Ecoverse and REST | yes |
| Choose match | Ambiguous match | Pick which destination device is this one | yes |
| Mark resolved | Lost, Wrong place, Unreachable | Closes it as fixed outside N-voke | yes, with a note |
| Run cleanup now | Healthy | Runs `Cleanup` through the agent that reaches it | yes |
| Tag… / Run a script… | any | The existing Tags and Run flows, prefilled with the devices | as today |

## Data and commands

The web view never builds a migration object itself. It calls commands, gets typed results (generated), and subscribes to events.

### Commands (Tauri, over the core's `Core` methods)

| Group | Commands | Core |
|---|---|---|
| Servers | `servers_list`, `server_save`, `server_ecoverse`, `server_test`, `server_remove` | `ncentral_servers`, `save_ncentral_server`, `set_server_ecoverse`, `remove_ncentral_server`, the sign-in check |
| Projects | `projects_list`, `project_create`, `project_open` (takes the lease), `project_close` (releases it), `project_delete`, `project_options_save` | `migration_list`, `migration_create_full`, `migration_load` and `migration_lease`, `migration_release_lease`, `migration_delete` |
| Connect | `survey_run` | `migration_survey` |
| Map | `project_map`, `project_map_details`, `project_map_set`, `project_map_clear`, `project_tokens_generate` | `migration_map_view`, `migration_map_details`, `migration_map_set`, `migration_map_clear`, `migration_generate_tokens` |
| Prepare | `project_prepare`, `project_script_orgs`, `project_publish`, `project_export_script`, `project_rest_script`, `project_options` | `migration_prepare_view`, `migration_search_script_orgs`, `migration_prepare_publish`, `migration_prepare_export`, `migration_prepare_rest_script`, `migration_prepare_options` |
| Plan | `project_plan`, `project_plan_add` | `migration_plan_view`, `migration_plan_add` |
| Launch | `project_launch`, `project_wave_create`, `project_wave_preview`, `project_wave_launch` (with `dry_run`) | `migration_launch_view`, `migration_wave_create`, `migration_launch_preview`, `migration_launch_saved` |
| Track | `tracker_start`, `tracker_stop`, `project_poll`, `project_track`, `project_device`, `project_device_command` | `migration_poll_digest`, `migration_track_view`, `migration_device_detail`, `migration_device_command` |
| Finish | `project_finish`, `project_report`, `project_cleanup`, `project_retire_view`, `project_retire` | `migration_finish_view`, `migration_finish_report`, `migration_cleanup_healthy`, `migration_retire_view`, `migration_retire_sources` (built 2026-10-21: `DELETE /api/devices/{id}?removeAgents=false` for cleaned-up devices past their review period, only while the project's retire option is on) |

Every command that writes takes `dry_run`, takes the project's lease first, and returns the refreshed summary. Errors are the core's one-sentence messages; the page shows them in place with `ErrorNote`.

### Types

Generated from the Rust structs and enums (`Project`, `ProjectSummary`, `ProjectDevice`, `Stage`, `Event`, `ProjectOptions`, `MappingEntry`, `Confidence`, `Override`, `PlannedDevice`, `Blocker`, `Warning`, `PlanIntoReport`, `TokenReport`, `PollReport`, `Thresholds`, `ServerView`). The build fails if the generated file is out of date, so the web view can't drift.

### Live updates

```mermaid
sequenceDiagram
    participant UI as Migrate page
    participant T as Tauri commands
    participant C as Core tracker task
    UI->>T: project_open(id)
    T->>C: take the lease, start the tracker
    loop every poll interval
        C->>C: poll, move stages, save
        C-->>UI: event migration://<id>/summary
        C-->>UI: event migration://<id>/moved (device, from, to)
    end
    UI->>T: devices_page(filter, page)
    UI->>T: project_close(id)
    T->>C: stop the tracker, release the lease
```

- **Events carry summaries, not the whole project.** `summary` is stage counts, per-wave and per-customer progress, the attention list and the tracker state; `moved` is the stage changes in that pass. The page keeps these in the TanStack Query cache (`setQueryData`) and refetches the device table pages that are visible.
- **The tracker runs only while a project is open** in a window and stops on close, like job polling today; on the next open it asks the APIs for the current state and catches up, instead of replaying missed events.
- **Core gap:** the core has `migration_poll_saved` (one pass) but no runner and no event channel; both are new, behind a small trait so the terminal app can use the same runner with a different sink.
- **Scale, measured (2026-10-21):** `Project` still holds every device and each save rewrites it. Measured with the release build: 10,000 devices with 30 events each is a 41 MB file that saves in about 130 ms, loads in about 150 ms and builds a Track page in about 5 ms (1,000 devices: 4 MB, 23 ms, 16 ms). That is comfortably interactive, so the SQLite move ([Q7](09-roadmap.md#open-questions)) is **deferred** until a project is measured to be slow. The Plan and Track views are paged in the core, so the page never receives or renders thousands of devices at once. Re-run the measurement with `cargo test scale_of_the_project_store --release -- --ignored --nocapture` in `crates/core`.

### Lease and two windows

Opening a project takes its lease; a second window (or the terminal app) gets *Open in another window* and a **read-only** view with the same screens and every action disabled, with a banner and a *Take over* button after the lease expires (120 s without a heartbeat). The page sends the heartbeat while it is open.

### Errors and states every screen has

| State | Shown as |
|---|---|
| Loading | Skeleton rows in the existing table style |
| Nothing yet | `EmptyState` with the one next action |
| Server unreachable or JWT refused | Red line on Connect and an `ErrorNote` where a call failed, with *Test* |
| Write refused (project changed in another window) | The core's message and **Reload project** |
| Dry-run on | A banner on every screen: *Dry-run: nothing will change* |

## Frontend structure

```
src/components/migrate/
  MigratePage.tsx          list ↔ workspace switch, back link
  ProjectList.tsx          screen A
  NewProjectDialog.tsx     screen B
  ServersSheet.tsx         screen C (also used from Settings)
  Workspace.tsx            header, step rail, step content, read-only banner
  steps/
    ConnectStep.tsx  MapStep.tsx  PrepareStep.tsx  PlanStep.tsx  LaunchStep.tsx  TrackStep.tsx  FinishStep.tsx
  parts/
    StepRail.tsx           the seven steps with done / current / blocked marks
    StageStrip.tsx         counts per stage group, filter on click
    StageChip.tsx          dot + word from the Words table
    AttentionCard.tsx      sentence + actions for one device
    DeviceTable.tsx        paged, filtered, selectable
    DeviceSheet.tsx        timeline, last probe, actions
    MappingTable.tsx       row with destination combobox and token cell
    ConfirmLaunch.tsx      the launch confirmation
    DryRunBanner.tsx
src/lib/migrate/
  api.ts                   typed wrappers over the commands (no logic)
  events.ts                subscribes to the events, writes the Query cache
  words.ts                 stage → short word, tone, long sentence (the only place wording lives)
  bindings.ts              GENERATED from the Rust types; never edited
```

New pieces beyond today's `ui.tsx`: `StepRail`, `StageStrip` (a segmented progress bar like `PhaseBar`), and a paged `DeviceTable` (the current `Table` is one page). Everything else is composed from what exists. `words.ts` is the only TypeScript that decides wording, and it is data, not rules; the rules (which stage, which action) come from the core.

## Build order

Slices that each end in something you can click, in this order after this design is approved:

| Slice | Contents | Done when |
|---|---|---|
| 0. Plumbing | The type generator, the command layer for servers and projects, `api.ts`, `bindings.ts` in CI, the Migrate sidebar entry with an empty page | A command round-trips a typed `ProjectSummary` and CI fails on a stale `bindings.ts` |
| 1. Servers and projects | Screens A, B, C; Connect | Add the lab servers, test them, create a project, see the checklist |
| 2. Map and tokens | Screen 2 including tokens | The lab's customers map, ambiguous ones can be chosen, missing tokens generated |
| 3. Prepare and plan | Screens 3 and 4 | Publish the script, enter the REST item, plan the lab devices |
| 4. Launch and track | Screens 5 and 6, the tracker runner and events | Launch a wave on the lab and watch it reach healthy, with the delays shown as words |
| 5. Finish and polish | Screen 7, reports, Settings section, dry-run banner, read-only mode, empty and error states | The Phase 3 exit test (a forced stuck and a forced lost device) runs from the app |
| 6. After the app | Create missing structure (Q18), drag mapping, the move of the existing features to the Rust core | per [Phase 5](09-roadmap.md#phase-5-desktop-app) |

Each slice also moves one existing feature's logic into the core when it touches it, as the [Rust-core plan](09-roadmap.md#phase-5-desktop-app) says; nothing in a slice changes how Run, Activity or Tags look.

## What the terminal app takes from it

Nothing: the terminal app's Migrate page and commands were removed (2026-10-21), so there is no second interface to keep in step. The wording tables stay in TypeScript (`words.ts`) with the screens that use them.

## Design decisions (2026-10-20)

The questions raised in the draft were settled by taking the recommendations:

| Question | Decision |
|---|---|
| Step rail or tabs | **Step rail** (vertical, with done, current and blocked marks) |
| Does Track open first for a project with launches | **Yes**; Connect to Prepare stay one click away |
| How much of the stage list does the strip show | **Eight groups**; the sixteen real stages are in the table filter |
| Where does *Create missing structure* sit | **In Map** (built 2026-10-21, after the Q18 lab test) |
| What the list's progress bar counts | **Devices that are healthy or beyond**, out of the devices being moved (left-out devices are not counted) |
| Thresholds | **Global**, per machine, in the **Settings** button on the Migrate page (built 2026-10-21; it is a sheet there rather than a section of the app Settings dialog). They also hold the poll interval and the options a new project starts with |

## Build status

| Slice | State |
|---|---|
| 0. Plumbing | **Built** (2026-10-20): the generator (`ts-rs` behind a `ts` feature of `nvoke-core`, `npm run bindings`, and `npm run bindings:check` in CI), the `servers_list` and `projects_list` commands, `api.ts`, `queries.ts`, `words.ts` (every stage has words, enforced by the generated `Stage` type), `progress.ts`, the Migrate sidebar entry with the "needs you" count, and the project list (screen A) with its loading, empty and error states. Checked in a browser with the native side mocked (list, empty, error) |
| 1. Servers and projects | **Built** (2026-10-20): the servers sheet (add, edit, test, remove; a server a project uses can't be removed), the new-project dialog (name, two servers, the Ecoverse link choice, optional Ecoverse organizations), delete from the list's row menu, the project workspace with the step rail, and **Connect** with its checklist. Core: `connect.rs` (`migration_connect_check`, with the checklist rules as a tested pure function), `migration_summary`, `migration_project_using_server`. Commands: `server_save`, `server_test`, `server_remove`, `project_summary`, `project_create`, `project_delete`, `project_connect_check`. Checked in a browser with a stateful mock of the native side (invalid host, bad JWT, an old release, no Ecoverse organizations, a same-server reinstall, removing a server in use, delete). **Ecoverse credentials and delivery (added, uncommitted):** each server has an Ecoverse key choice (app key, its own token, or none) and each project a delivery (Ecoverse first, or N-central only); every Ecoverse call in the migration code runs with the right server's key; the survey and Connect respect both choices; *Another tenant* is enabled when the destination has its own token. Command `server_ecoverse`; `project_create` takes `delivery`. **Not covered yet:** a real Tauri window and real servers; post-move Status and Cleanup for an *Another tenant* destination still go over N-central (the project holds one Ecoverse script id); the *token access* line on Connect (it needs the org units, so it moves to Map); the **Continue** button (there is no Map step to go to yet) |
| 2. Map and tokens | **Built** (2026-10-20): the Map step with a row per source customer and site (a site under its customer), the destination picker (same kind only, units that fit equally well first, a site's customer shown to tell two sites of one name apart, *Leave out*, *Use the automatic match*), how each row matched (*similar* and *exact, other parent* are flagged), device counts, the token column, **Tokens → Create missing** (preview, then a result list), a filter and a *Need a choice* view, and **Continue** from Connect. Choices show at once and are put back if the core refuses. Core: `mapview.rs` (`migration_map_view`, `migration_map_details`, `migration_map_set`, `migration_map_clear`, tested). Commands: `project_map`, `project_map_details`, `project_map_set`, `project_map_clear`, `project_tokens_generate`, `project_close` (lets go of the project's lease). The rows come first and the device counts and tokens a moment later, because they read every customer's devices. **Not covered yet:** a real window and servers; counts and tokens are read again after each choice, which is slow on a large server (the core should keep the device lists for a few minutes); *Create missing structure*; a destination chosen from the picker isn't checked against the destination server by the core |
| 3. Prepare | **Built** (2026-10-20): the Prepare step with an Ecoverse card (choose the organization to publish in, searched with the source server's key; publish, or already up to date; the organization is remembered on the project), a REST card (save the script to the Downloads folder, the repository item ID and version, *outdated* when the saved version isn't this build's), and the options form (clean mode, review period, wave size, retry interval and duration, retire source records), with the limits checked in the page and again in the core. Core: `prepare.rs` (`migration_prepare_view`, `migration_search_script_orgs`, `migration_prepare_publish`, `migration_prepare_export`, `migration_prepare_rest_script`, `migration_prepare_options`; tested). Commands: `project_prepare`, `project_script_orgs`, `project_publish`, `project_export_script`, `project_rest_script`, `project_options`. **The destination's script (added after the first live run):** a move to another server also gets **Ecoverse on the destination** (only when the destination is in another Ecoverse tenant: the script is published there with the destination's key, and Probe and Cleanup use it for a device that has moved) and **N-central repository on the destination** (the repository item that reaches a moved device when Ecoverse can't). Before this, Probe and Cleanup on a moved device said *the script hasn't been uploaded to the destination* with nowhere to fix it. **The destination's Ecoverse script is published automatically** the first time Probe or Cleanup needs it, in the tenant's single service organization (type `ServiceOrganization`; when there isn't exactly one, the app asks in Prepare, where the field is preselected when there is). The N-central repository item for the destination is **optional**: it is only used when Ecoverse can't reach a moved device, and the error says why Ecoverse wasn't used (script not published, device not shown yet, not connected, or unreadable) before it mentions an upload. A reinstall has one server and needs neither. **Differences from the design:** the REST card is always shown, because the plan doesn't exist yet at this step (it says it is the fallback, or the only path for *N-central only*); the file is saved to the Downloads folder, since the app has no save dialog yet; there is no Continue button until Plan is built. **Not covered yet:** a real window and servers; the delivery can't be changed after the project is created |
| 4. Plan | **Built** (2026-10-20): the Plan step with the counts, what blocks devices (each reason worded by the page, with *Fix in Map* or *Fix in Prepare*), filters (All, Ready, Blocked, customer, search), a table of 200 rows at a time, **Add N devices to the project** (then *Continue to Launch*), and *Read servers again*. Core: `planview.rs` (`migration_plan_view`, `migration_plan_add`; tested). Commands: `project_plan`, `project_plan_add`. **Not built:** a per-device path override and *leave out*, because the plan has nowhere to keep them (leaving a whole customer out is done in Map); paging and filtering in the core for very large servers (the plan comes back as one list) |
| 5. Launch | **Built** (2026-10-20): waves with their device counts and how many still wait, **New wave** (a name and a size, with *Pilot* and *Wave size* shortcuts; the next devices in name order), and the launch confirmation (by path, by destination, clean mode and review period, devices that can't be launched as planned, and what stops the launch: a script that isn't current in Ecoverse or the repository) with **Dry run**, which shows the held-back requests with passwords hidden and changes nothing. A real launch ends at Track. Core: `launchview.rs` (`migration_launch_view`, `migration_wave_create`, `migration_launch_preview`, `migration_launch_saved`; tested). Commands: `project_launch`, `project_wave_create`, `project_wave_preview`, `project_wave_launch`. **Not built:** *Choose devices* for a wave; the offline count and the token expiry in the confirmation (tokens are read at launch) |
| 6. Track | **Built** (2026-10-20): the stage strip (eight groups, a click filters the table), **Needs you** cards with the sentence for each stage, the device table (wave, customer and search filters, 200 rows at a time), the device sheet (where it is, both device IDs, the timeline newest first, the actions that fit its stage with a confirmation, *Choose match*, and the *I looked, and it did not run* check for a launch whose answer was lost), **Poll now**, and the tracker: the native side polls when the step opens and every minute, saves what moved and sends `migration-polled`, which refreshes the page; it stops when the step or project closes. A second window gets *Read-only*. Core: `trackview.rs` (`migration_track_view`, `migration_device_detail`, `migration_device_command`, `migration_poll_digest`; tested). Commands: `project_track`, `project_device`, `project_device_command`, `project_poll`, `tracker_start`, `tracker_stop`. **After everything is healthy:** Track shows *Everything has moved and is working* with **Continue to Finish** once no device is planned, moving, needing you or waiting for Ecoverse's data. Finish explains what happens next: each device removes its own migration record after the review period (counted from when it became healthy), the tracker notes it while the project is open, and the project **closes** when every device has cleaned up (or, if the project retires the old source records, when they are retired). **Checks on the step rail:** a step gets a green check only when all of its work is done. Connect, Map and Plan are checked once devices are in the project; Prepare when the script is in place; **Launch only when every device has been launched** (with several waves it is outlined, with a tooltip like *4 of 6 devices launched; 2 still waiting*, and a retried device takes the check back); Track when everything that moved is working; Finish when the project is closed. The rail can't tell that Plan still has blocked devices left, since that needs the servers read. **Bulk cleanup** shows each device's progress as it runs, can be stopped after the device it is on, and never changes either N-central server or Ecoverse (it removes the device's scheduled tasks and the N-voke folder). Every command that changes a project, and each tracker pass, takes a per-project lock in the native layer, so a probe or cleanup that takes minutes can't have its save refused because the tracker saved in between. **Not built:** the *known delays* second line under Healthy (the Ecoverse agent still setting up); the probe's `status.json` shown as its own block (it is on the timeline); the tracker doesn't keep running when you leave the Track step |
| 7. Finish | **Built** (2026-10-20): where the project stands, the report (a web page or CSV, saved to Downloads), **Run cleanup now** for every healthy device (previewed, then done one after another and saved after each, with the ones that failed listed), and *Old source records* marked *Not available yet*. Core: `finish.rs` (`migration_finish_view`, `migration_finish_report`, `migration_cleanup_healthy`; tested). Commands: `project_finish`, `project_report`, `project_cleanup`. **Not built:** deleting the old source records (the core gap above) |
| 8. Functional gaps | **Built** (2026-10-21): a per-device choice of path and *Leave out* / *Include again* (Plan row menu, kept on the project so planning keeps it); **Choose devices** for a wave (a search, ticks that stay across searches); paged Plan and Track views with the strip counts and the first *Needs you* devices from the core; **Settings** for the poll interval, the thresholds and the new-project defaults; **Activity** entries for Ecoverse launches (only when the source is read with the app's own Ecoverse key, because Activity watches tasks with that key); **Old source records** in Finish; **Create missing…** in Map; **delivery** (Ecoverse first or N-central only) can be changed in Prepare until a wave launches, which re-paths the planned devices that have no path of the operator's own choosing, and is locked after that. **Cleanup scope** (built 2026-10-21): *Run cleanup now* lists the ready devices with a tick each and a button per wave, so cleanup can run on one wave or on chosen devices. **Known delays** (built 2026-10-21, partly): a launch on a device whose Ecoverse asset is connected but has sent no data waits 60 minutes before it counts as *Not picked up*, and says why; the Ecoverse waiting texts say it can take up to about an hour and a half because the agent retries hourly. The cleanup now logs a `cleanup-verify` line saying whether any old agent folder is left. **Reads and caching** (built 2026-10-21): the screens do not ask the servers for what they already know. (1) The core remembers customers and sites, a customer's devices, registration tokens and a device's identity for a few minutes (`migration/readcache.rs`): choosing a row on Map no longer re-reads every customer's devices, and a plan or preview asked again reads nothing. *Re-run matching*, *Read servers again* and creating anything drop what was remembered for the project's two servers; the tracker never uses it, it reads fresh. (2) Each device keeps the names of its customer and destination unit from when it was planned, so Track, waves and the launch preview need no read; a project from before that looks them up once. A server that is down is not asked again for 30 seconds. (3) The tracker reads nothing for a healthy device with no Ecoverse to wait for, and looks at one Ecoverse is receiving data from every 15 minutes, not every pass. Matching a moved device reads the identity of each new device under its customer once, not once per source device (which was thousands of calls for a large customer). (4) The parsed project is kept in memory while its file is unchanged (10,000 devices: about 50 microseconds instead of a 150 ms decrypt and parse on every screen), and saves update it. (5) Views the tracker keeps current do not refetch when the window gains focus.

**UI review** (built 2026-10-21): a status bar on every step before Track (devices moved and working, waiting for Ecoverse, a *Review* button for those who need you); *Needs you* grouped by reason with a count, the sentence once and a few devices to open, instead of one card each; wave cards with launch time and working / moving / need-you counts; the Plan counts as stat blocks; the device timeline reads as stages and kinds instead of raw event names; tables scroll sideways instead of clipping at the 900 px minimum width; a narrower step rail below 1100 px; the Ecoverse choices in New project folded behind a one-line summary; a *Ready to plan?* checklist at the top of Prepare (each line scrolls to its card; the destination's scripts are marked optional because they are made when first needed); and a page error boundary so a drawing error shows *Try again* instead of emptying the window. **Not built:** showing the next hourly retry time (the minute is not readable from outside the device); tag and custom-property mirroring (it could only reach source-side assets, which disappear after the move); a warning for devices installed in the last hour (neither the N-central device list nor Ecoverse gives an install time); SQLite storage (measured, deferred: see Scale above) |
