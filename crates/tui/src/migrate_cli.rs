//! Non-interactive migration commands, used to set up servers and to check what a
//! plan would be before the Migrate screens exist:
//!
//!   nvoke migrate-server list | add | remove
//!   nvoke migrate-plan
//!   nvoke migrate-project ...   (drives a whole project; changes things, honours --dry-run)
//!
//! migrate-server and migrate-plan only read. A server's JWT is read
//! from `NVOKE_NCENTRAL_JWT` or from standard input, never from an argument,
//! which would end up in shell history.

use std::{io::BufRead, sync::Arc};

use nvoke_core::{
    Core,
    migration::{
        launcher::LaunchReport,
        packager::{Operation, Package},
        poller::PollReport,
        project::{DestEcoverse, Project},
        report::{Filter, Format},
        survey::SurveyOptions,
        tracker::Thresholds,
        mapping::Override,
        workflow::DeviceOp,
    },
};

pub const HELP: &str = "\
MIGRATION COMMANDS
Read-only (nothing is changed on any server):
    nvoke migrate-server list
    nvoke migrate-server add --host <host> [--label <name>] [--ecoverse-tenant <id>]
        The server's JWT is read from NVOKE_NCENTRAL_JWT, or from the first line of
        standard input. It is stored in the system keychain.
    nvoke migrate-server remove <id>
    nvoke migrate-plan --source <id> --dest <id> [--customer <name>]
                       [--ecoverse-org <id>]... [--rest-script-ready]
        Connects to both servers, maps customers and sites, reads the devices and
        the destination's registration tokens, and prints the plan.
        --customer limits it to source customers whose name contains the text;
        --ecoverse-org searches that source Ecoverse organization for devices.

Projects (the endpoint script is built into nvoke; --script <file> on publish, launch and probe uses a built file instead). These change things: launching removes agents from devices. Add --dry-run to
see exactly what would be sent without sending anything):
    nvoke migrate-project list
    nvoke migrate-project create --name <name> --source <id> --dest <id>
                       [--dest-ecoverse not-linked|same|<tenant id>]
                       [--source-ecoverse-org <id>]... [--dest-ecoverse-org <id>]...
    nvoke migrate-project show <project>
    nvoke migrate-project plan <project> [--customer <name>]
        Adds the devices that are ready; reports the rest and why.
    nvoke migrate-project generate-tokens <project> [--customer <name>]
        Creates a registration token on the destination for every customer or site the devices are going
        to that has none (a new server has none). Valid tokens are left alone; expired ones are reported.
    nvoke migrate-project map <project> --source <id> (--dest <id> | --exclude)
        Overrides the automatic mapping for a source customer or site (ids are in square brackets in `plan`).
    nvoke migrate-project wave <project> [--label <text>] [--max <n>]
    nvoke migrate-project rest-script <project> --item <number> --version <version>
        Records the repository item you uploaded the script as, on the source server.
    nvoke migrate-project export-script [--out <folder>]
        Saves the endpoint script (built into nvoke) for the N-central path: upload it by hand, with Enable API on.
    nvoke migrate-project publish <project> [--ecoverse-org <id>]
    nvoke migrate-project launch <project> --wave <n> [--dry-run]
    nvoke migrate-project poll <project> [--watch] [--every <seconds>]
    nvoke migrate-project probe <project> --device <name or id> [--cleanup] [--wait <seconds>]
        Reads the device's own record (Status), or removes what the migration left on it (Cleanup),
        through whichever agent it has now. Nothing can reach a device between the old agent's
        removal and the new one's first check-in.
    nvoke migrate-project retry <project> --device <name or id> [--other-path] [--checked]
        Puts a device whose launch failed, wasn't picked up or is stuck back in line; launch the wave to send it.
    nvoke migrate-project resolve <project> --device <name or id>
        The device was fixed outside N-voke: stop tracking it.
    nvoke migrate-project choose-match <project> --device <name or id> --dest-device <number>
        More than one destination device fits: say which one is it.
    nvoke migrate-project report <project> --format csv|html [--wave <n>] [--out <folder>]
    nvoke migrate-project delete <project> --yes
";

#[derive(Debug, PartialEq, Eq)]
pub enum ProjectCommand {
    List,
    Create { name: String, source: String, dest: String, dest_ecoverse: DestEcoverseArg, source_orgs: Vec<String>, dest_orgs: Vec<String> },
    Show { id: String },
    Plan { id: String, customer: Option<String> },
    Wave { id: String, label: String, max: Option<usize> },
    RestScript { id: String, item: String, version: String },
    Publish { id: String, script: Option<String>, org: Option<String> },
    Launch { id: String, wave: u32, script: Option<String> },
    ExportScript { out: Option<String> },
    Poll { id: String, watch: bool, every: u64 },
    Probe { id: String, device: String, script: Option<String>, cleanup: bool, wait: u64 },
    Retry { id: String, device: String, other_path: bool, checked: bool },
    GenerateTokens { id: String, customer: Option<String> },
    Map { id: String, source: String, decision: MapDecision },
    Resolve { id: String, device: String },
    ChooseMatch { id: String, device: String, dest_device: String },
    Report { id: String, format: String, wave: Option<u32>, out: Option<String> },
    Delete { id: String },
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum MapDecision {
    To(String),
    Exclude,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum DestEcoverseArg {
    NotLinked,
    Same,
    Tenant(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Project(ProjectCommand),
    ServerList,
    ServerAdd { host: String, label: String, ecoverse_tenant: Option<String> },
    ServerRemove { id: String },
    Plan { source: String, dest: String, customer: Option<String>, ecoverse_orgs: Vec<String>, rest_script_ready: bool },
}

/// Whether the first argument names a migration command.
pub fn is_migrate_command(first: Option<&str>) -> bool {
    matches!(first, Some("migrate-server" | "migrate-plan" | "migrate-project"))
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let usage = |what: &str| format!("{what}\n\n{HELP}");
    let mut it = args.iter().map(String::as_str);
    match it.next() {
        Some("migrate-server") => match it.next() {
            Some("list") => match it.next() {
                None => Ok(Command::ServerList),
                Some(extra) => Err(usage(&format!("Unexpected argument: {extra}"))),
            },
            Some("remove") => match (it.next(), it.next()) {
                (Some(id), None) => Ok(Command::ServerRemove { id: id.into() }),
                _ => Err(usage("migrate-server remove takes exactly one server id.")),
            },
            Some("add") => {
                let (mut host, mut label, mut tenant) = (None, None, None);
                while let Some(flag) = it.next() {
                    let value = it.next().ok_or_else(|| usage(&format!("{flag} needs a value.")))?.to_string();
                    match flag {
                        "--host" => host = Some(value),
                        "--label" => label = Some(value),
                        "--ecoverse-tenant" => tenant = Some(value),
                        other => return Err(usage(&format!("Unknown option: {other}"))),
                    }
                }
                let host = host.ok_or_else(|| usage("--host is required."))?;
                Ok(Command::ServerAdd { label: label.unwrap_or_default(), host, ecoverse_tenant: tenant })
            }
            _ => Err(usage("Use migrate-server list, add or remove.")),
        },
        Some("migrate-plan") => {
            let (mut source, mut dest, mut customer, mut orgs, mut rest_ready) = (None, None, None, vec![], false);
            while let Some(flag) = it.next() {
                if flag == "--rest-script-ready" {
                    rest_ready = true;
                    continue;
                }
                let value = it.next().ok_or_else(|| usage(&format!("{flag} needs a value.")))?.to_string();
                match flag {
                    "--source" => source = Some(value),
                    "--dest" => dest = Some(value),
                    "--customer" => customer = Some(value),
                    "--ecoverse-org" => orgs.push(value),
                    other => return Err(usage(&format!("Unknown option: {other}"))),
                }
            }
            Ok(Command::Plan {
                source: source.ok_or_else(|| usage("--source is required (see migrate-server list)."))?,
                dest: dest.ok_or_else(|| usage("--dest is required (see migrate-server list)."))?,
                customer,
                ecoverse_orgs: orgs,
                rest_script_ready: rest_ready,
            })
        }
        Some("migrate-project") => parse_project(&args[1..]).map(Command::Project),
        _ => Err(usage("Not a migration command.")),
    }
}

/// `--flag value` pairs and bare `--flag`s, after the positional arguments.
struct Flags {
    pairs: Vec<(String, Option<String>)>,
}

impl Flags {
    fn parse(args: &[String], bare: &[&str]) -> Result<Self, String> {
        let mut pairs = vec![];
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            if !flag.starts_with("--") {
                return Err(format!("Unexpected argument: {flag}"));
            }
            if bare.contains(&flag.as_str()) {
                pairs.push((flag.clone(), None));
            } else {
                let value = it.next().ok_or_else(|| format!("{flag} needs a value."))?;
                pairs.push((flag.clone(), Some(value.clone())));
            }
        }
        Ok(Self { pairs })
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.pairs.iter().filter(|(k, _)| k == name).filter_map(|(_, v)| v.clone()).collect()
    }

    fn one(&self, name: &str) -> Option<String> {
        self.all(name).into_iter().next()
    }

    fn has(&self, name: &str) -> bool {
        self.pairs.iter().any(|(k, _)| k == name)
    }

    fn require(&self, name: &str) -> Result<String, String> {
        self.one(name).ok_or_else(|| format!("{name} is required."))
    }

    fn number<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, String> {
        self.one(name).map(|v| v.parse().map_err(|_| format!("{name} must be a number."))).transpose()
    }

    /// Refuses a flag the command doesn't take, so a typo can't silently do something else.
    fn only(&self, allowed: &[&str]) -> Result<(), String> {
        match self.pairs.iter().find(|(k, _)| !allowed.contains(&k.as_str())) {
            Some((k, _)) => Err(format!("Unknown option: {k}")),
            None => Ok(()),
        }
    }
}

fn parse_project(args: &[String]) -> Result<ProjectCommand, String> {
    let usage = |what: &str| format!("{what}\n\n{HELP}");
    let sub = args.first().map(String::as_str).ok_or_else(|| usage("Use migrate-project list, create, show, plan, wave, rest-script, publish, launch, poll, probe, retry, resolve, choose-match, generate-tokens, map, export-script, report or delete."))?;
    let rest = &args[1..];
    // Everything but list and create names a project first.
    let (id, flag_args) = if matches!(sub, "list" | "create" | "export-script") {
        (String::new(), rest)
    } else {
        let id = rest.first().filter(|a| !a.starts_with("--")).ok_or_else(|| usage(&format!("migrate-project {sub} needs a project id (see migrate-project list).")))?;
        (id.clone(), &rest[1..])
    };
    let flags = Flags::parse(flag_args, &["--watch", "--yes", "--cleanup", "--other-path", "--checked", "--exclude"]).map_err(|e| usage(&e))?;
    let fail = |e: String| usage(&e);
    let command = match sub {
        "list" => {
            flags.only(&[]).map_err(fail)?;
            ProjectCommand::List
        }
        "create" => {
            flags.only(&["--name", "--source", "--dest", "--dest-ecoverse", "--source-ecoverse-org", "--dest-ecoverse-org"]).map_err(fail)?;
            let dest_ecoverse = match flags.one("--dest-ecoverse").as_deref() {
                None | Some("not-linked") => DestEcoverseArg::NotLinked,
                Some("same") => DestEcoverseArg::Same,
                Some(tenant) => DestEcoverseArg::Tenant(tenant.to_string()),
            };
            ProjectCommand::Create {
                name: flags.require("--name").map_err(fail)?,
                source: flags.require("--source").map_err(fail)?,
                dest: flags.require("--dest").map_err(fail)?,
                dest_ecoverse,
                source_orgs: flags.all("--source-ecoverse-org"),
                dest_orgs: flags.all("--dest-ecoverse-org"),
            }
        }
        "show" => {
            flags.only(&[]).map_err(fail)?;
            ProjectCommand::Show { id }
        }
        "plan" => {
            flags.only(&["--customer"]).map_err(fail)?;
            ProjectCommand::Plan { id, customer: flags.one("--customer") }
        }
        "wave" => {
            flags.only(&["--label", "--max"]).map_err(fail)?;
            ProjectCommand::Wave { id, label: flags.one("--label").unwrap_or_default(), max: flags.number("--max").map_err(fail)? }
        }
        "rest-script" => {
            flags.only(&["--item", "--version"]).map_err(fail)?;
            ProjectCommand::RestScript { id, item: flags.require("--item").map_err(fail)?, version: flags.require("--version").map_err(fail)? }
        }
        "export-script" => {
            flags.only(&["--out"]).map_err(fail)?;
            ProjectCommand::ExportScript { out: flags.one("--out") }
        }
        "publish" => {
            flags.only(&["--script", "--ecoverse-org"]).map_err(fail)?;
            ProjectCommand::Publish { id, script: flags.one("--script"), org: flags.one("--ecoverse-org") }
        }
        "launch" => {
            flags.only(&["--wave", "--script"]).map_err(fail)?;
            ProjectCommand::Launch { id, wave: flags.number("--wave").map_err(fail)?.ok_or_else(|| usage("--wave is required."))?, script: flags.one("--script") }
        }
        "poll" => {
            flags.only(&["--watch", "--every"]).map_err(fail)?;
            ProjectCommand::Poll { id, watch: flags.has("--watch"), every: flags.number("--every").map_err(fail)?.unwrap_or(30).max(5) }
        }
        "probe" => {
            flags.only(&["--device", "--script", "--cleanup", "--wait"]).map_err(fail)?;
            ProjectCommand::Probe { id, device: flags.require("--device").map_err(fail)?, script: flags.one("--script"), cleanup: flags.has("--cleanup"), wait: flags.number("--wait").map_err(fail)?.unwrap_or(600) }
        }
        "generate-tokens" => {
            flags.only(&["--customer"]).map_err(fail)?;
            ProjectCommand::GenerateTokens { id, customer: flags.one("--customer") }
        }
        "map" => {
            flags.only(&["--source", "--dest", "--exclude"]).map_err(fail)?;
            let decision = match (flags.one("--dest"), flags.has("--exclude")) {
                (Some(dest), false) => MapDecision::To(dest),
                (None, true) => MapDecision::Exclude,
                _ => return Err(usage("Give either --dest <id> or --exclude.")),
            };
            ProjectCommand::Map { id, source: flags.require("--source").map_err(fail)?, decision }
        }
        "retry" => {
            flags.only(&["--device", "--other-path", "--checked"]).map_err(fail)?;
            ProjectCommand::Retry { id, device: flags.require("--device").map_err(fail)?, other_path: flags.has("--other-path"), checked: flags.has("--checked") }
        }
        "resolve" => {
            flags.only(&["--device"]).map_err(fail)?;
            ProjectCommand::Resolve { id, device: flags.require("--device").map_err(fail)? }
        }
        "choose-match" => {
            flags.only(&["--device", "--dest-device"]).map_err(fail)?;
            ProjectCommand::ChooseMatch { id, device: flags.require("--device").map_err(fail)?, dest_device: flags.require("--dest-device").map_err(fail)? }
        }
        "report" => {
            flags.only(&["--format", "--wave", "--out"]).map_err(fail)?;
            ProjectCommand::Report { id, format: flags.require("--format").map_err(fail)?, wave: flags.number("--wave").map_err(fail)?, out: flags.one("--out") }
        }
        "delete" => {
            flags.only(&["--yes"]).map_err(fail)?;
            if !flags.has("--yes") {
                return Err(usage("Deleting a project removes its history for good. Add --yes to confirm."));
            }
            ProjectCommand::Delete { id }
        }
        other => return Err(usage(&format!("Unknown migrate-project command: {other}"))),
    };
    Ok(command)
}

fn read_jwt() -> Result<String, String> {
    if let Ok(jwt) = std::env::var("NVOKE_NCENTRAL_JWT")
        && !jwt.trim().is_empty()
    {
        return Ok(jwt);
    }
    eprintln!("Paste the server's API JWT and press Enter (set NVOKE_NCENTRAL_JWT to avoid typing it here):");
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).map_err(|e| format!("Could not read the JWT: {e}"))?;
    if line.trim().is_empty() { Err("No JWT given.".into()) } else { Ok(line) }
}

/// Runs a parsed command and returns what to print.
pub async fn run(core: &Arc<Core>, command: Command) -> Result<String, String> {
    match command {
        Command::ServerList => {
            let servers = core.ncentral_servers().await?;
            if servers.is_empty() {
                return Ok("No N-central servers yet. Add one with: nvoke migrate-server add --host <host>\n".into());
            }
            Ok(servers
                .iter()
                .map(|s| format!("{:<14} {:<24} {:<32} {}{}\n", s.id, s.label, s.host, if s.has_jwt { "JWT saved" } else { "NO JWT" }, s.ecoverse_tenant.as_deref().map(|t| format!(", Ecoverse tenant {t}")).unwrap_or_default()))
                .collect())
        }
        Command::ServerAdd { host, label, ecoverse_tenant } => {
            let jwt = read_jwt()?;
            let server = core.save_ncentral_server(None, &label, &host, Some(jwt), ecoverse_tenant).await?;
            Ok(format!("Added {} ({}) as {}. Its JWT is in the keychain.\n", server.label, server.host, server.id))
        }
        Command::ServerRemove { id } => {
            core.remove_ncentral_server(&id).await?;
            Ok(format!("Removed {id} and its saved JWT.\n"))
        }
        Command::Project(command) => run_project(core, command).await,
        Command::Plan { source, dest, customer, ecoverse_orgs, rest_script_ready } => {
            let options = SurveyOptions { source_server_id: source, dest_server_id: dest, ecoverse_org_ids: ecoverse_orgs, customer, rest_script_ready };
            Ok(core.migration_survey(&options, None).await?.render())
        }
    }
}

fn instance() -> String {
    format!("cli-{}", std::process::id())
}

/// The script this build ships, or a built file given with `--script` (for trying a change without rebuilding).
fn load_package(path: Option<&str>) -> Result<Package, String> {
    match path {
        None => Package::embedded(),
        Some(path) => {
            let content = std::fs::read_to_string(path).map_err(|e| format!("Could not read {path}: {e}"))?;
            Package::from_built(&content)
        }
    }
}

/// What dry-run held back, with the exact (redacted) request, or nothing if dry-run is off.
fn held_back(core: &Core) -> String {
    let log = core.dry_run_log();
    if log.is_empty() {
        return String::new();
    }
    let mut out = format!("\nDry run: nothing was sent. {} change(s) held back:\n", log.len());
    for entry in log {
        let shown: String = entry.variables.to_string().chars().take(700).collect();
        out.push_str(&format!("  {} ({})\n    {}\n", entry.summary, entry.operation.as_deref().unwrap_or(&entry.action), shown));
    }
    out
}

pub fn render_status(project: &Project) -> String {
    use nvoke_core::migration::report::stage_label;
    let mut out = format!("{} ({})\n  source {} -> destination {}{}\n", project.name, project.id, project.source_server_id, project.dest_server_id, if project.is_reinstall() { " (reinstall)" } else { "" });
    if project.devices.is_empty() {
        out.push_str("  No devices yet. Run: nvoke migrate-project plan ");
        out.push_str(&project.id);
        out.push('\n');
        return out;
    }
    let counts = project.stage_counts();
    out.push_str(&format!("  {} devices: {}\n", project.devices.len(), counts.iter().map(|(s, n)| format!("{n} {}", stage_label(*s))).collect::<Vec<_>>().join(", ")));
    if project.attention_count() > 0 {
        out.push_str(&format!("  {} need attention\n", project.attention_count()));
    }
    for wave in &project.waves {
        out.push_str(&format!("  wave {}: {}{}\n", wave.number, wave.label, if wave.launched_at.is_some() { " (launched)" } else { "" }));
    }
    out.push('\n');
    for d in project.devices.iter().take(100) {
        let marker = if d.stage.needs_attention() { "!" } else { " " };
        let error = d.last_error.as_deref().map(|e| format!("  {e}")).unwrap_or_default();
        out.push_str(&format!("{marker} {:<28} wave {:<3} {:<9} {:<14} since {}{}\n", d.name, d.wave.map_or("-".to_string(), |w| w.to_string()), d.path.map_or("-".to_string(), |p| format!("{p:?}")), stage_label(d.stage), d.entered_stage_at, error));
    }
    if project.devices.len() > 100 {
        out.push_str(&format!("  ... and {} more (export a report for the full list)\n", project.devices.len() - 100));
    }
    out
}

pub fn render_poll(report: &PollReport) -> String {
    use nvoke_core::migration::report::stage_label;
    let mut out = format!("Checked {} device(s).\n", report.checked);
    for (name, from, to) in &report.moved {
        out.push_str(&format!("  {name}: {} -> {}\n", stage_label(*from), stage_label(*to)));
    }
    for (name, flag) in &report.attention {
        out.push_str(&format!("  ! {name}: {flag:?}\n"));
    }
    for (name, ids) in &report.ambiguous {
        out.push_str(&format!("  ! {name}: more than one destination device fits ({}). Choose one.\n", ids.join(", ")));
    }
    for error in &report.errors {
        out.push_str(&format!("  could not read: {error}\n"));
    }
    out
}

/// What the device reported, readable: the message, the task's state, and its status file's stage and history.
pub fn render_action(result: &nvoke_core::migration::endpoint_result::EndpointResult) -> String {
    let mut out = format!("{}{}\n", result.result, if result.message.is_empty() { String::new() } else { format!(": {}", result.message) });
    if !result.task_state.is_empty() {
        out.push_str(&format!("  scheduled task: {}\n", result.task_state));
    }
    if let Ok(status) = serde_json::from_str::<serde_json::Value>(&result.status_json) {
        if let Some(stage) = status["stage"].as_str() {
            out.push_str(&format!("  device stage: {stage} (attempt {})\n", status["attempts"]));
        }
        for entry in status["history"].as_array().into_iter().flatten().rev().take(10).collect::<Vec<_>>().into_iter().rev() {
            out.push_str(&format!("    {} {} {}\n", entry["at"].as_str().unwrap_or(""), entry["event"].as_str().unwrap_or(""), entry["detail"].as_str().unwrap_or("")));
        }
    }
    out
}

pub fn render_tokens(report: &nvoke_core::migration::workflow::TokenReport) -> String {
    let mut out = String::new();
    if !report.generated.is_empty() {
        out.push_str(&format!("Generated a registration token for {}.\n", report.generated.join(", ")));
    }
    if report.already_fine > 0 {
        out.push_str(&format!("{} already had a valid token (left alone).\n", report.already_fine));
    }
    if !report.expired.is_empty() {
        out.push_str(&format!("Expired, renew in N-central: {}.\n", report.expired.join(", ")));
    }
    if report.held_back > 0 {
        out.push_str(&format!("{} held back by dry-run.\n", report.held_back));
    }
    for (name, why) in &report.failed {
        out.push_str(&format!("! {name}: {why}\n"));
    }
    if out.is_empty() {
        out.push_str("No destination customer or site is involved yet: plan the devices first.\n");
    }
    out
}

pub fn render_launch(report: &LaunchReport) -> String {
    let mut out = format!("Launched {} through Ecoverse and {} through N-central", report.launched_ecoverse, report.launched_rest);
    if report.fell_back > 0 {
        out.push_str(&format!(" ({} of them fell back because Ecoverse couldn't reach them)", report.fell_back));
    }
    out.push_str(".\n");
    if report.held_back > 0 {
        out.push_str(&format!("{} device(s) held back by dry-run.\n", report.held_back));
    }
    for (name, why) in &report.failed {
        out.push_str(&format!("  ! {name}: {why}\n"));
    }
    if let Some(stopped) = &report.stopped {
        out.push_str(&format!("Stopped early: {stopped} The devices not reached are still waiting in the wave.\n"));
    }
    out
}

/// The one device a name or source device ID refers to.
fn find_device(project: &Project, device: &str) -> Result<usize, String> {
    let wanted = device.to_lowercase();
    let matches: Vec<usize> = project.devices.iter().enumerate().filter(|(_, d)| d.source_device_id == device || d.name.to_lowercase() == wanted).map(|(i, _)| i).collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!("No device named or numbered {device} in {}.", project.name)),
        _ => Err(format!("More than one device matches {device}: use its source device ID.")),
    }
}

async fn device_op(core: &Arc<Core>, id: &str, device: &str, op: DeviceOp) -> Result<String, String> {
    let project = core.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
    let index = find_device(&project, device)?;
    match core.migration_device_op_saved(id, &instance(), index, op).await {
        Ok((_, message)) => Ok(format!("{message}\n{}", held_back(core))),
        Err(e) if nvoke_core::is_dry_run_error(&e) => Ok(held_back(core)),
        Err(e) => Err(e),
    }
}

async fn run_project(core: &Arc<Core>, command: ProjectCommand) -> Result<String, String> {
    let me = instance();
    match command {
        ProjectCommand::List => {
            let list = core.migration_list().await?;
            if list.is_empty() {
                return Ok("No migration projects yet. Create one with: nvoke migrate-project create --name <name> --source <id> --dest <id>\n".into());
            }
            Ok(list.iter().map(|p| format!("{:<14} {:<28} {} devices, {} need attention{}\n", p.id, p.name, p.devices, p.attention, if p.closed { ", finished" } else { "" })).collect())
        }
        ProjectCommand::Create { name, source, dest, dest_ecoverse, source_orgs, dest_orgs } => {
            let kind = match dest_ecoverse {
                DestEcoverseArg::NotLinked => DestEcoverse::NotLinked,
                DestEcoverseArg::Same => DestEcoverse::SameTenant,
                DestEcoverseArg::Tenant(tenant_id) => DestEcoverse::OtherTenant { tenant_id },
            };
            let mut project = core.migration_create(&name, &source, &dest, kind).await?;
            project.source_ecoverse_org_ids = source_orgs;
            project.dest_ecoverse_org_ids = dest_orgs;
            core.migration_save(&mut project).await?;
            Ok(format!("Created project {}\n  id: {}\nNext: nvoke migrate-project plan {}\n", project.name, project.id, project.id))
        }
        ProjectCommand::Show { id } => Ok(render_status(&core.migration_load(&id).await?.ok_or_else(|| format!("There is no migration project {id}."))?)),
        ProjectCommand::Plan { id, customer } => {
            let mut project = core.migration_open_for_write(&id, &me).await?;
            let (report, survey) = core.migration_plan_into(&mut project, customer.as_deref()).await?;
            core.migration_save(&mut project).await?;
            Ok(format!("{}\nAdded {} ready device(s) to the project ({} were already in it); {} can't be launched yet.\n", survey.render(), report.added, report.already_in, report.blocked.len()))
        }
        ProjectCommand::Wave { id, label, max } => {
            let mut project = core.migration_open_for_write(&id, &me).await?;
            let number = project.auto_wave(&label, max, chrono::Utc::now())?;
            core.migration_save(&mut project).await?;
            let size = project.devices.iter().filter(|d| d.wave == Some(number)).count();
            Ok(format!("Created wave {number} with {size} device(s). Launch it with: nvoke migrate-project launch {id} --wave {number} --script <path>\n"))
        }
        ProjectCommand::RestScript { id, item, version } => {
            let mut project = core.migration_open_for_write(&id, &me).await?;
            core.migration_set_rest_script(&mut project, &item, &version)?;
            core.migration_save(&mut project).await?;
            Ok(format!("Recorded script item {item} (version {version}) for the source server.\n"))
        }
        ProjectCommand::Publish { id, script, org } => {
            let package = load_package(script.as_deref())?;
            let mut project = core.migration_open_for_write(&id, &me).await?;
            match core.migration_publish(&mut project, &package, org.as_deref()).await {
                Ok(published) => {
                    core.migration_save(&mut project).await?;
                    Ok(format!("{} as script {} ({:?}).\n", package.script_name(), published.script_id, published.action))
                }
                Err(e) if nvoke_core::is_dry_run_error(&e) => Ok(held_back(core)),
                Err(e) => Err(e),
            }
        }
        ProjectCommand::Launch { id, wave, script } => {
            let package = load_package(script.as_deref())?;
            let mut project = core.migration_open_for_write(&id, &me).await?;
            let script_id = project.ecoverse_script_id.clone();
            let report = core.migration_launch_wave(&mut project, wave, &package, script_id.as_deref()).await;
            core.migration_save(&mut project).await?;
            let report = report?;
            Ok(format!("{}{}", render_launch(&report), held_back(core)))
        }
        ProjectCommand::Poll { id, watch, every } => {
            let thresholds = Thresholds::default();
            let mut transcript = String::new();
            loop {
                let (project, report) = core.migration_poll_saved(&id, &me, &thresholds).await?;
                let text = format!("{}  {}\n", render_poll(&report), project.stage_counts().iter().map(|(s, n)| format!("{n} {s:?}")).collect::<Vec<_>>().join(", "));
                if !watch || report.checked == 0 {
                    transcript.push_str(&text);
                    break;
                }
                print!("{text}");
                tokio::time::sleep(std::time::Duration::from_secs(every)).await;
            }
            Ok(transcript)
        }
        ProjectCommand::GenerateTokens { id, customer } => {
            let project = core.migration_load(&id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
            let report = core.migration_generate_tokens(&project, customer.as_deref()).await?;
            Ok(format!("{}{}", render_tokens(&report), held_back(core)))
        }
        ProjectCommand::Map { id, source, decision } => {
            let mut project = core.migration_open_for_write(&id, &me).await?;
            let (decision, text) = match decision {
                MapDecision::To(dest) => (Override::MapTo(dest.clone()), format!("Source {source} now goes to destination {dest}.")),
                MapDecision::Exclude => (Override::Exclude, format!("Source {source} is left out of the migration.")),
            };
            core.migration_set_mapping(&mut project, &source, decision)?;
            core.migration_save(&mut project).await?;
            Ok(format!("{text} Run plan again to apply it.\n"))
        }
        ProjectCommand::Retry { id, device, other_path, checked } => device_op(core, &id, &device, DeviceOp::Retry { other_path, checked }).await,
        ProjectCommand::Resolve { id, device } => device_op(core, &id, &device, DeviceOp::Resolve).await,
        ProjectCommand::ChooseMatch { id, device, dest_device } => device_op(core, &id, &device, DeviceOp::ChooseMatch { dest_device_id: dest_device }).await,
        ProjectCommand::Probe { id, device, script, cleanup, wait } => {
            let package = load_package(script.as_deref())?;
            let mut project = core.migration_open_for_write(&id, &me).await?;
            let index = find_device(&project, &device)?;
            let operation = if cleanup { Operation::Cleanup } else { Operation::Status };
            let outcome = core.migration_run_action(&mut project, index, operation, &package, std::time::Duration::from_secs(wait)).await;
            core.migration_save(&mut project).await?;
            match outcome {
                Ok(o) => Ok(format!("{} via {}: {}\n{}", if cleanup { "Cleanup" } else { "Status" }, o.via, render_action(&o.result), held_back(core))),
                Err(e) if nvoke_core::is_dry_run_error(&e) => Ok(held_back(core)),
                Err(e) => Err(e),
            }
        }
        ProjectCommand::ExportScript { out } => {
            let package = Package::embedded()?;
            let dir = out.map(std::path::PathBuf::from).or_else(dirs::download_dir).ok_or("No downloads folder: pass --out <folder>.")?;
            let path = core.migration_export_script(&package, &dir)?;
            Ok(format!(
                "Wrote {}\nVersion {}\n\nTo use the N-central path, upload this file to each server's Script/Software Repository (the source server, and the destination if you want Status/Cleanup there),\nturn on Enable API for it, then tell N-voke the item number it was given:\n  nvoke migrate-project rest-script <project> --item <number> --version {}\nA new version means deleting the old item and uploading again: it gets a new number.\n",
                path.display(),
                package.version,
                package.version
            ))
        }
        ProjectCommand::Report { id, format, wave, out } => {
            let project = core.migration_load(&id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
            let format = match format.as_str() {
                "csv" => Format::Csv,
                "html" => Format::Html,
                other => return Err(format!("--format must be csv or html, not {other}.")),
            };
            let dir = out.map(std::path::PathBuf::from).or_else(dirs::download_dir).ok_or("No downloads folder: pass --out <folder>.")?;
            let path = core.migration_write_report(&project, &Filter { wave, dest_org_unit: None }, format, &dir)?;
            Ok(format!("Wrote {}\n", path.display()))
        }
        ProjectCommand::Delete { id } => {
            core.migration_delete(&id).await?;
            Ok(format!("Deleted project {id}. The audit log keeps its entries.\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn only_the_two_command_names_are_migration_commands() {
        assert!(is_migrate_command(Some("migrate-plan")) && is_migrate_command(Some("migrate-server")));
        assert!(!is_migrate_command(Some("--dry-run")) && !is_migrate_command(None));
    }

    #[test]
    fn server_commands_parse() {
        assert_eq!(parse(&args(&["migrate-server", "list"])), Ok(Command::ServerList));
        assert_eq!(parse(&args(&["migrate-server", "remove", "srv-1"])), Ok(Command::ServerRemove { id: "srv-1".into() }));
        assert_eq!(
            parse(&args(&["migrate-server", "add", "--host", "new.example.com", "--label", "New", "--ecoverse-tenant", "t1"])),
            Ok(Command::ServerAdd { host: "new.example.com".into(), label: "New".into(), ecoverse_tenant: Some("t1".into()) })
        );
        assert!(parse(&args(&["migrate-server", "add"])).unwrap_err().contains("--host is required"));
        assert!(parse(&args(&["migrate-server", "add", "--host"])).unwrap_err().contains("needs a value"));
        assert!(parse(&args(&["migrate-server", "add", "--jwt", "abc"])).unwrap_err().contains("Unknown option: --jwt"), "a JWT must never be an argument");
        assert!(parse(&args(&["migrate-server", "remove"])).is_err());
        assert!(parse(&args(&["migrate-server"])).is_err());
    }

    #[test]
    fn the_plan_command_needs_both_servers_and_takes_repeated_orgs() {
        let parsed = parse(&args(&["migrate-plan", "--source", "a", "--dest", "b", "--customer", "Acme", "--ecoverse-org", "o1", "--ecoverse-org", "o2", "--rest-script-ready"]));
        assert_eq!(parsed, Ok(Command::Plan { source: "a".into(), dest: "b".into(), customer: Some("Acme".into()), ecoverse_orgs: vec!["o1".into(), "o2".into()], rest_script_ready: true }));
        assert!(parse(&args(&["migrate-plan", "--source", "a"])).unwrap_err().contains("--dest is required"));
        assert!(parse(&args(&["migrate-plan", "--dest", "b"])).unwrap_err().contains("--source is required"));
        assert!(parse(&args(&["migrate-plan", "--source", "a", "--dest", "b", "--wat", "x"])).is_err());
    }

    #[tokio::test]
    async fn listing_with_no_servers_says_how_to_add_one() {
        let core = Core::for_tests("cli");
        let text = run(&core, Command::ServerList).await.unwrap();
        assert!(text.contains("nvoke migrate-server add --host"), "{text}");
        core.save_ncentral_server(None, "Old", "old.example.com", Some("j".into()), None).await.unwrap();
        let text = run(&core, Command::ServerList).await.unwrap();
        assert!(text.contains("old.example.com") && text.contains("JWT saved") && !text.contains("NO JWT"), "{text}");
    }

    fn project_cmd(list: &[&str]) -> Result<ProjectCommand, String> {
        match parse(&args(&[&["migrate-project"], list].concat()))? {
            Command::Project(c) => Ok(c),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn project_commands_parse_with_their_flags() {
        assert_eq!(project_cmd(&["list"]), Ok(ProjectCommand::List));
        assert_eq!(
            project_cmd(&["create", "--name", "Acme", "--source", "a", "--dest", "b", "--dest-ecoverse", "t-9", "--source-ecoverse-org", "o1", "--source-ecoverse-org", "o2"]),
            Ok(ProjectCommand::Create { name: "Acme".into(), source: "a".into(), dest: "b".into(), dest_ecoverse: DestEcoverseArg::Tenant("t-9".into()), source_orgs: vec!["o1".into(), "o2".into()], dest_orgs: vec![] })
        );
        assert_eq!(project_cmd(&["plan", "mig-1", "--customer", "Acme"]), Ok(ProjectCommand::Plan { id: "mig-1".into(), customer: Some("Acme".into()) }));
        assert_eq!(project_cmd(&["wave", "mig-1", "--max", "5", "--label", "Pilot"]), Ok(ProjectCommand::Wave { id: "mig-1".into(), label: "Pilot".into(), max: Some(5) }));
        assert_eq!(project_cmd(&["launch", "mig-1", "--wave", "2", "--script", "x.ps1"]), Ok(ProjectCommand::Launch { id: "mig-1".into(), wave: 2, script: Some("x.ps1".into()) }));
        assert_eq!(project_cmd(&["launch", "mig-1", "--wave", "2"]), Ok(ProjectCommand::Launch { id: "mig-1".into(), wave: 2, script: None }), "the script built into nvoke is the default");
        assert_eq!(project_cmd(&["publish", "mig-1"]), Ok(ProjectCommand::Publish { id: "mig-1".into(), script: None, org: None }));
        assert_eq!(project_cmd(&["export-script"]), Ok(ProjectCommand::ExportScript { out: None }));
        assert_eq!(project_cmd(&["export-script", "--out", "/tmp/x"]), Ok(ProjectCommand::ExportScript { out: Some("/tmp/x".into()) }));
        assert_eq!(project_cmd(&["poll", "mig-1", "--watch", "--every", "2"]), Ok(ProjectCommand::Poll { id: "mig-1".into(), watch: true, every: 5 }), "never faster than every 5 seconds");
        assert_eq!(project_cmd(&["poll", "mig-1"]), Ok(ProjectCommand::Poll { id: "mig-1".into(), watch: false, every: 30 }));
        assert_eq!(project_cmd(&["report", "mig-1", "--format", "csv"]), Ok(ProjectCommand::Report { id: "mig-1".into(), format: "csv".into(), wave: None, out: None }));
    }

    #[test]
    fn a_typo_a_missing_value_or_a_missing_project_is_an_error_not_a_different_command() {
        assert!(project_cmd(&["launch", "mig-1", "--wave", "2", "--scirpt", "x"]).unwrap_err().contains("Unknown option: --scirpt"));
        assert!(project_cmd(&["launch", "mig-1"]).unwrap_err().contains("--wave is required"));
        assert!(project_cmd(&["launch", "mig-1", "--wave", "two", "--script", "x"]).unwrap_err().contains("must be a number"));
        assert!(project_cmd(&["plan"]).unwrap_err().contains("needs a project id"));
        assert!(project_cmd(&["plan", "--customer", "Acme"]).unwrap_err().contains("needs a project id"));
        assert!(project_cmd(&["frobnicate", "x"]).unwrap_err().contains("Unknown migrate-project command"));
        assert!(project_cmd(&[]).is_err());
        assert!(project_cmd(&["create", "--name", "x", "--source", "a"]).unwrap_err().contains("--dest is required"));
        assert!(project_cmd(&["launch", "mig-1", "--wave", "1", "--script", "x", "--dry-run"]).is_err(), "--dry-run is taken out before parsing");
    }

    #[test]
    fn probe_takes_a_device_and_a_script_and_cleanup_is_a_bare_flag() {
        assert_eq!(project_cmd(&["probe", "mig-1", "--device", "PC-1"]), Ok(ProjectCommand::Probe { id: "mig-1".into(), device: "PC-1".into(), script: None, cleanup: false, wait: 600 }));
        assert_eq!(project_cmd(&["probe", "mig-1", "--device", "9001", "--cleanup", "--wait", "30"]), Ok(ProjectCommand::Probe { id: "mig-1".into(), device: "9001".into(), script: None, cleanup: true, wait: 30 }));
        assert!(project_cmd(&["probe", "mig-1"]).unwrap_err().contains("--device is required"));
    }

    #[test]
    fn a_status_report_shows_the_stage_and_the_last_events_in_order() {
        use nvoke_core::migration::endpoint_result::EndpointResult;
        let history: Vec<serde_json::Value> = (1..=12).map(|i| serde_json::json!({ "at": format!("t{i}"), "event": "e", "detail": format!("d{i}") })).collect();
        let result = EndpointResult { result: "ok".into(), message: "Last log: x".into(), task_state: "Ready".into(), status_json: serde_json::json!({ "stage": "installing", "attempts": 2, "history": history }).to_string(), ..Default::default() };
        let text = render_action(&result);
        assert!(text.starts_with("ok: Last log: x") && text.contains("scheduled task: Ready") && text.contains("device stage: installing (attempt 2)"), "{text}");
        assert!(text.contains("t3 e d3") && text.contains("t12 e d12") && !text.contains("t2 e d2"), "only the last ten, oldest first: {text}");
        assert!(text.find("t3 e").unwrap() < text.find("t12 e").unwrap());
        assert_eq!(render_action(&EndpointResult { result: "failed".into(), ..Default::default() }), "failed\n");
    }

    #[test]
    fn retry_resolve_and_choose_match_parse() {
        assert_eq!(project_cmd(&["retry", "mig-1", "--device", "PC-1", "--other-path"]), Ok(ProjectCommand::Retry { id: "mig-1".into(), device: "PC-1".into(), other_path: true, checked: false }));
        assert_eq!(project_cmd(&["retry", "mig-1", "--device", "PC-1"]), Ok(ProjectCommand::Retry { id: "mig-1".into(), device: "PC-1".into(), other_path: false, checked: false }));
        assert_eq!(project_cmd(&["retry", "mig-1", "--device", "PC-1", "--checked"]), Ok(ProjectCommand::Retry { id: "mig-1".into(), device: "PC-1".into(), other_path: false, checked: true }));
        assert_eq!(project_cmd(&["resolve", "mig-1", "--device", "9001"]), Ok(ProjectCommand::Resolve { id: "mig-1".into(), device: "9001".into() }));
        assert_eq!(project_cmd(&["choose-match", "mig-1", "--device", "PC-1", "--dest-device", "5002"]), Ok(ProjectCommand::ChooseMatch { id: "mig-1".into(), device: "PC-1".into(), dest_device: "5002".into() }));
        assert!(project_cmd(&["choose-match", "mig-1", "--device", "PC-1"]).unwrap_err().contains("--dest-device is required"));
        assert!(project_cmd(&["resolve", "mig-1"]).unwrap_err().contains("--device is required"));
    }

    #[test]
    fn generate_tokens_and_map_parse() {
        assert_eq!(project_cmd(&["generate-tokens", "mig-1", "--customer", "Acme"]), Ok(ProjectCommand::GenerateTokens { id: "mig-1".into(), customer: Some("Acme".into()) }));
        assert_eq!(project_cmd(&["generate-tokens", "mig-1"]), Ok(ProjectCommand::GenerateTokens { id: "mig-1".into(), customer: None }));
        assert_eq!(project_cmd(&["map", "mig-1", "--source", "10", "--dest", "110"]), Ok(ProjectCommand::Map { id: "mig-1".into(), source: "10".into(), decision: MapDecision::To("110".into()) }));
        assert_eq!(project_cmd(&["map", "mig-1", "--source", "10", "--exclude"]), Ok(ProjectCommand::Map { id: "mig-1".into(), source: "10".into(), decision: MapDecision::Exclude }));
        assert!(project_cmd(&["map", "mig-1", "--source", "10"]).unwrap_err().contains("either --dest <id> or --exclude"));
        assert!(project_cmd(&["map", "mig-1", "--source", "10", "--dest", "1", "--exclude"]).is_err());
        assert!(project_cmd(&["map", "mig-1", "--dest", "1"]).unwrap_err().contains("--source is required"));
    }

    #[test]
    fn the_token_report_says_what_was_made_left_alone_expired_or_refused() {
        use nvoke_core::migration::workflow::TokenReport;
        let text = render_tokens(&TokenReport { generated: vec!["Acme".into(), "Globex".into()], already_fine: 3, expired: vec!["Initech".into()], held_back: 0, failed: vec![("Umbrella".into(), "HTTP 403".into())] });
        assert!(text.contains("Generated a registration token for Acme, Globex.") && text.contains("3 already had a valid token") && text.contains("Expired, renew in N-central: Initech.") && text.contains("! Umbrella: HTTP 403"), "{text}");
        assert!(render_tokens(&TokenReport::default()).contains("plan the devices first"));
    }

    #[test]
    fn deleting_needs_an_explicit_yes() {
        assert!(project_cmd(&["delete", "mig-1"]).unwrap_err().contains("Add --yes"));
        assert_eq!(project_cmd(&["delete", "mig-1", "--yes"]), Ok(ProjectCommand::Delete { id: "mig-1".into() }));
    }

    #[tokio::test]
    async fn a_project_can_be_created_shown_waved_scripted_and_deleted_through_the_runner() {
        let core = Core::for_tests("project-cli");
        let src = core.save_ncentral_server(None, "Old", "old.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "new.example.com", Some("j2".into()), None).await.unwrap();
        assert!(run_project(&core, ProjectCommand::List).await.unwrap().contains("No migration projects yet"));

        let created = run_project(&core, ProjectCommand::Create { name: "Acme".into(), source: src.id.clone(), dest: dst.id.clone(), dest_ecoverse: DestEcoverseArg::NotLinked, source_orgs: vec!["org-1".into()], dest_orgs: vec![] }).await.unwrap();
        let id = created.split_whitespace().find(|w| w.starts_with("mig-")).unwrap().to_string();
        assert!(run_project(&core, ProjectCommand::List).await.unwrap().contains("Acme"));
        let shown = run_project(&core, ProjectCommand::Show { id: id.clone() }).await.unwrap();
        assert!(shown.contains("No devices yet") && shown.contains(&format!("{} -> destination {}", src.id, dst.id)), "{shown}");

        assert!(run_project(&core, ProjectCommand::Wave { id: id.clone(), label: String::new(), max: None }).await.unwrap_err().contains("No device is waiting"));
        assert!(run_project(&core, ProjectCommand::RestScript { id: id.clone(), item: "x".into(), version: "v".into() }).await.is_err());
        assert!(run_project(&core, ProjectCommand::RestScript { id: id.clone(), item: "77".into(), version: "v".into() }).await.unwrap().contains("script item 77"));
        assert!(run_project(&core, ProjectCommand::Show { id: "mig-nope".into() }).await.unwrap_err().contains("no migration project"));
        assert!(run_project(&core, ProjectCommand::Launch { id: id.clone(), wave: 1, script: Some("/nonexistent/x.ps1".into()) }).await.unwrap_err().contains("Could not read /nonexistent/x.ps1"));
        assert!(run_project(&core, ProjectCommand::Launch { id: id.clone(), wave: 1, script: None }).await.unwrap_err().contains("There is no wave 1"), "without --script the embedded one is used, and the launch gets as far as the wave");
        let exported = std::env::temp_dir().join(format!("nvoke-cli-script-{}", std::process::id()));
        let text = run_project(&core, ProjectCommand::ExportScript { out: Some(exported.display().to_string()) }).await.unwrap();
        assert!(text.contains("Wrote ") && text.contains("Enable API") && text.contains("rest-script <project> --item <number> --version 0.1.0+"), "{text}");
        assert_eq!(std::fs::read_dir(&exported).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(exported);

        let report = run_project(&core, ProjectCommand::Report { id: id.clone(), format: "pdf".into(), wave: None, out: None }).await;
        assert!(report.unwrap_err().contains("csv or html"));
        let dir = std::env::temp_dir().join(format!("nvoke-cli-report-{}", std::process::id()));
        let wrote = run_project(&core, ProjectCommand::Report { id: id.clone(), format: "csv".into(), wave: None, out: Some(dir.display().to_string()) }).await.unwrap();
        assert!(wrote.contains("nvoke-migration-Acme-"), "{wrote}");
        let _ = std::fs::remove_dir_all(dir);

        assert!(run_project(&core, ProjectCommand::Delete { id: id.clone() }).await.unwrap().contains("Deleted project"));
        assert!(run_project(&core, ProjectCommand::List).await.unwrap().contains("No migration projects yet"));
    }

    #[test]
    fn the_status_launch_and_poll_text_say_what_happened_and_what_needs_attention() {
        use chrono::Utc;
        use nvoke_core::migration::{planner::{DeliveryPath, PlannedDevice}, project::Stage, tracker::Attention};
        let mut project = Project::new("mig-1".into(), "Acme", "a", "b", DestEcoverse::NotLinked, Utc::now());
        let planned = |id: &str| PlannedDevice { device_id: id.into(), name: format!("PC-{id}"), source_customer_id: None, source_site_id: None, dest_org_unit_id: Some("110".into()), path: Some(DeliveryPath::Rest), asset_id: None, blockers: vec![], warnings: vec![], excluded: false };
        project.add_planned(&[planned("1"), planned("2")], Utc::now());
        project.create_wave("Pilot", &["1".into(), "2".into()], Utc::now()).unwrap();
        project.devices[0].stage = Stage::Healthy;
        project.devices[1].stage = Stage::Lost;
        let text = render_status(&project);
        assert!(text.contains("2 devices: 1 Lost, 1 Healthy") || text.contains("2 devices: 1 Healthy, 1 Lost"), "{text}");
        assert!(text.contains("1 need attention") && text.contains("wave 1: Pilot"), "{text}");
        assert!(text.lines().any(|l| l.starts_with("! PC-2")) && text.lines().any(|l| l.starts_with("  PC-1")), "{text}");

        let launch = render_launch(&LaunchReport { launched_ecoverse: 3, launched_rest: 2, fell_back: 1, held_back: 0, failed: vec![("PC-9".into(), "no token".into())], stopped: None });
        assert!(launch.contains("Launched 3 through Ecoverse and 2 through N-central (1 of them fell back") && launch.contains("! PC-9: no token"), "{launch}");
        assert!(render_launch(&LaunchReport { held_back: 4, ..Default::default() }).contains("4 device(s) held back by dry-run"));
        let stopped = render_launch(&LaunchReport { stopped: Some("Stopped before sending anything more: the save was refused.".into()), ..Default::default() });
        assert!(stopped.contains("Stopped early: Stopped before sending anything more") && stopped.contains("still waiting in the wave"), "{stopped}");

        let poll = render_poll(&PollReport { checked: 3, moved: vec![("PC-1".into(), Stage::Staged, Stage::AgentRemoved)], attention: vec![("PC-2".into(), Attention::EcoverseNotMoved)], ambiguous: vec![("PC-3".into(), vec!["5".into(), "6".into()])], errors: vec!["boom".into()] });
        assert!(poll.contains("PC-1: Staged -> Agent removed") && poll.contains("! PC-3: more than one destination device fits (5, 6)") && poll.contains("could not read: boom"), "{poll}");
    }
}
