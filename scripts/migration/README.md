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

Everything the harness writes (state, logs, task dumps) is git-ignored. Test scripts it publishes are named `N-voke Phase0 …` or `N-voke lab: …` and are safe to delete.

## Driving a migration from the terminal (Phase 3)

`nvoke` can run a whole project from the command line, which is how the lab run is done. The app's **Migrate** page (key 4) shows the same projects live and can poll, launch the next wave, probe a device and export a report; creating a project, planning, making waves and publishing the script can be done there too (keys `n`, `T`, `P`, `w`, `U`), and so can adding, editing, testing and removing servers (`S`, or `a` in the new-project form); the N-central script item and the CSV report stay here. Every command that changes something honours `--dry-run`: reads work, and each change is held back and listed with its exact (redacted) request.

```sh
export NVOKE_NCENTRAL_JWT=...                                   # or paste when asked; stored in the keychain
nvoke migrate-server add --host old.example.com --label Old
nvoke migrate-server add --host new.example.com --label New
nvoke migrate-server list                                       # shows the ids used below

nvoke migrate-project create --name "Lab move" --source <old id> --dest <new id> \
      --source-ecoverse-org <source Ecoverse org id>           # omit it to use the REST path only
nvoke migrate-project generate-tokens <project> --customer "Demo"   # a new destination has no registration tokens
nvoke migrate-project plan <project> --customer "Demo"          # adds ready devices, says why others aren't
nvoke migrate-project map <project> --source <id> --dest <id>    # only if the automatic mapping is wrong; ids are in [brackets] in plan
nvoke migrate-project publish <project>                          # the endpoint script is built into nvoke
nvoke migrate-project export-script                              # REST path only: upload the file by hand (Enable API on) ...
nvoke migrate-project rest-script <project> --item <id> --version <version>   # ... then tell nvoke its item number
nvoke migrate-project wave <project> --label Pilot --max 2
nvoke migrate-project launch <project> --wave 1 --dry-run
nvoke migrate-project launch <project> --wave 1
nvoke migrate-project poll <project> --watch                    # saves after every pass; Ctrl+C is safe
nvoke migrate-project probe <project> --device <name>                       # a stuck device's own status.json, via the agent it has now
nvoke migrate-project retry <project> --device <name> [--other-path] [--checked]   # then launch the wave again; --checked: you looked and an earlier launch with a lost answer did not run
nvoke migrate-project resolve <project> --device <name>              # fixed outside N-voke
nvoke migrate-project choose-match <project> --device <name> --dest-device <number>
nvoke migrate-project show <project>
nvoke migrate-project report <project> --format html
```

`launch` removes the old agent from every device in the wave. It only launches devices still waiting, so running it twice does nothing the second time. The project is locked to one `nvoke` at a time. Nothing has been run against a lab yet: expect to tune the thresholds in [Tracking](../../docs/migration/06-tracking.md).

The endpoint script is built into `nvoke` from the same two source files `build.mjs` uses (a test checks the bytes match). `--script <file>` on `publish`, `launch` and `probe`, or `NVOKE_MIGRATION_SCRIPT` for the Migrate page, uses a built file instead, to try a change without rebuilding `nvoke`. Any change to the scripts changes the version. A launch carries the version N-voke expects, and an uploaded copy that is a different version refuses to run and says to upload again, so a stale file fails clearly instead of running old logic.
