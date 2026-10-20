//! The Migrate page: the project list, and for the open project a live dashboard (stage counts,
//! every device with how long it has been in its stage, the selected device's timeline) with the
//! actions an operator needs while a migration runs: poll now, launch a wave, probe a device,
//! export a report. Everything real is `nvoke-core`'s `migration` modules; this file is state,
//! keys and the hand-off to async work. Drawing is in `migrate_ui.rs`.
//!
//! The open project is held under its lease, so a second `nvoke` shows it read-only. While open
//! and writable it polls itself every 30 seconds, saving after each pass.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use nvoke_core::{
    is_dry_run_error,
    ncentral::ServerView,
    migration::{
        launcher::{LaunchReport, plan_wave},
        workflow::DeviceOp,
        packager::{Operation, Package},
        project::DestEcoverse,
        workflow::PlanIntoReport,
        planner::DeliveryPath,
        poller::PollReport,
        project::{Project, Stage},
        report::{Filter, Format},
        store::ProjectSummary,
        tracker::Thresholds,
    },
};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    widgets::TableState,
};

use super::{App, Confirm, Focus, Load, Msg, Page, run::NoticeKind};
use crate::input::TextInput;
use super::servers::{FormServers, SavedServer, ServersState};

/// How often an open project polls itself: 30 seconds at 10 ticks a second.
pub const AUTO_POLL_TICKS: u64 = 300;
/// How long a probe waits for the device's own report.
const PROBE_WAIT_SECONDS: u64 = 120;

pub enum MigrateMsg {
    Projects(Result<Vec<ProjectSummary>, String>),
    /// The project, whether this window may change it, and the labels of its two servers.
    Opened(Result<(Project, bool, (String, String)), String>),
    Polled(Result<(Project, PollReport), String>),
    Launched(Result<(Project, LaunchReport), String>),
    Probed(Result<(Project, String), String>),
    Exported(Result<String, String>),
    /// A retry, resolve or match: the project and a sentence about what happened.
    Operated(Result<(Project, String), String>),
    /// The servers to choose from, for the new-project form.
    Servers(Result<Vec<ServerView>, String>),
    Created(Result<Project, String>),
    Planned(Result<(Project, PlanIntoReport), String>),
    Tokens(Result<nvoke_core::migration::workflow::TokenReport, String>),
    ServerList(Result<Vec<ServerView>, String>),
    ServerSaved(Result<SavedServer, String>),
    ServerRemoved(Result<(), String>),
    ServerTested(Result<String, String>),
    /// The servers for the new-project form, and the server just added with whether it is the destination.
    FormServers(FormServers),
}

/// A message to read, in an overlay (what planning found, say).
pub struct Notes {
    pub title: String,
    pub lines: Vec<String>,
    pub scroll: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NpField {
    Name,
    Source,
    Dest,
    Eco,
    Tenant,
    SourceOrg,
    DestOrg,
}

/// The new-project form.
pub struct NewProject {
    pub name: TextInput,
    pub servers: Vec<ServerView>,
    pub source: usize,
    pub dest: usize,
    /// 0: destination not linked to Ecoverse yet; 1: same tenant as the source; 2: another tenant.
    pub eco: usize,
    pub tenant: TextInput,
    pub source_org: TextInput,
    pub dest_org: TextInput,
    pub field: NpField,
    pub error: Option<String>,
    pub busy: bool,
}

pub struct NewProjectArgs {
    pub name: String,
    pub source: String,
    pub dest: String,
    pub dest_ecoverse: DestEcoverse,
    pub source_orgs: Vec<String>,
    pub dest_orgs: Vec<String>,
}

impl NewProject {
    pub const ECO: [&'static str; 3] = ["Not yet (the usual case for a new server)", "The same tenant as the source", "Another tenant"];

    pub fn new(servers: Vec<ServerView>) -> Self {
        let dest = if servers.len() > 1 { 1 } else { 0 };
        Self { name: TextInput::default(), servers, source: 0, dest, eco: 0, tenant: TextInput::default(), source_org: TextInput::default(), dest_org: TextInput::default(), field: NpField::Name, error: None, busy: false }
    }

    /// Replaces the servers after one was added. `choose` is the new server's ID and whether it becomes the
    /// destination (otherwise the source): whichever field `a` was pressed on. Other choices are kept.
    pub fn set_servers(&mut self, servers: Vec<ServerView>, choose: Option<(&str, bool)>) {
        let (old_source, old_dest) = (self.servers.get(self.source).map(|s| s.id.clone()), self.servers.get(self.dest).map(|s| s.id.clone()));
        self.servers = servers;
        let find = |id: Option<String>| id.and_then(|id| self.servers.iter().position(|s| s.id == id));
        self.source = find(old_source).unwrap_or(0);
        self.dest = find(old_dest).unwrap_or(if self.servers.len() > 1 { 1 } else { 0 });
        if let Some((id, as_dest)) = choose
            && let Some(at) = self.servers.iter().position(|s| s.id == id)
        {
            if as_dest {
                self.dest = at;
            } else {
                self.source = at;
            }
        }
    }

    /// The fields shown, top to bottom: the tenant only when it is another tenant, the destination's organization only when it is linked.
    pub fn fields(&self) -> Vec<NpField> {
        let mut fields = vec![NpField::Name, NpField::Source, NpField::Dest, NpField::Eco];
        if self.eco == 2 {
            fields.push(NpField::Tenant);
        }
        fields.push(NpField::SourceOrg);
        if self.eco != 0 {
            fields.push(NpField::DestOrg);
        }
        fields
    }

    pub fn step(&mut self, delta: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.field).unwrap_or(0) as isize;
        self.field = fields[(at + delta).rem_euclid(fields.len() as isize) as usize];
    }

    pub fn input(&mut self) -> Option<&mut TextInput> {
        match self.field {
            NpField::Name => Some(&mut self.name),
            NpField::Tenant => Some(&mut self.tenant),
            NpField::SourceOrg => Some(&mut self.source_org),
            NpField::DestOrg => Some(&mut self.dest_org),
            _ => None,
        }
    }

    /// Changes the choice on a select field by one, wrapping.
    pub fn cycle(&mut self, forward: bool) {
        let next = |at: usize, len: usize| if forward { (at + 1) % len } else { (at + len - 1) % len };
        match self.field {
            NpField::Source => self.source = next(self.source, self.servers.len()),
            NpField::Dest => self.dest = next(self.dest, self.servers.len()),
            NpField::Eco => self.eco = next(self.eco, Self::ECO.len()),
            _ => {}
        }
    }

    pub fn validate(&self) -> Result<NewProjectArgs, String> {
        let name = self.name.value.trim().to_string();
        if name.is_empty() {
            return Err("Give the project a name.".into());
        }
        let ids = |text: &str| text.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect::<Vec<_>>();
        let dest_ecoverse = match self.eco {
            0 => DestEcoverse::NotLinked,
            1 => DestEcoverse::SameTenant,
            _ => match self.tenant.value.trim() {
                "" => return Err("Enter the other tenant's ID.".into()),
                tenant => DestEcoverse::OtherTenant { tenant_id: tenant.into() },
            },
        };
        Ok(NewProjectArgs {
            name,
            source: self.servers[self.source].id.clone(),
            dest: self.servers[self.dest].id.clone(),
            dest_ecoverse,
            source_orgs: ids(&self.source_org.value),
            dest_orgs: if self.eco == 0 { vec![] } else { ids(&self.dest_org.value) },
        })
    }
}

pub struct OpenProject {
    pub project: Project,
    /// Another window holds the project: look, don't touch.
    pub read_only: bool,
    pub server_labels: (String, String),
    pub table: TableState,
    pub last_report: Option<PollReport>,
    /// When the last poll finished (ticks), for "polled 12 s ago".
    pub polled_at_tick: Option<u64>,
}

impl OpenProject {
    /// Device indexes in display order: what needs attention first, then by stage, then by name.
    pub fn order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.project.devices.len()).collect();
        order.sort_by(|&a, &b| {
            let (x, y) = (&self.project.devices[a], &self.project.devices[b]);
            (y.stage.needs_attention() || !y.match_candidates.is_empty()).cmp(&(x.stage.needs_attention() || !x.match_candidates.is_empty())).then(x.stage.cmp(&y.stage)).then(x.name.to_lowercase().cmp(&y.name.to_lowercase()))
        });
        order
    }

    /// The device index the table's selection points at.
    pub fn selected_device(&self) -> Option<usize> {
        self.table.selected().and_then(|row| self.order().get(row).copied())
    }
}

/// The list of destination devices to choose between, for a device whose match is ambiguous.
pub struct Choosing {
    pub device: usize,
    pub table: TableState,
}

pub struct MigrateState {
    pub servers: ServersState,
    pub choosing: Option<Choosing>,
    pub notes: Option<Notes>,
    pub form: Option<NewProject>,
    pub projects: Load<Vec<ProjectSummary>>,
    pub list: TableState,
    pub open: Option<OpenProject>,
    /// What the page is waiting for ("Polling…"), if anything. Only one thing at a time.
    pub busy: Option<&'static str>,
    pub last_poll_tick: u64,
}

impl Default for MigrateState {
    fn default() -> Self {
        Self { servers: ServersState::default(), choosing: None, notes: None, form: None, projects: Load::NotLoaded, list: TableState::default(), open: None, busy: None, last_poll_tick: 0 }
    }
}

/// What a pending yes on the Migrate page does.
#[derive(Clone, Debug)]
pub enum MigrateConfirm {
    Launch { wave: u32, label: String, ecoverse: usize, rest: usize },
    Cleanup { device: usize, name: String },
    /// Make a wave of the next waiting devices.
    Wave { count: usize, max: usize },
    /// Create the destination's missing registration tokens.
    Tokens,
    /// Forget an N-central server and its saved JWT.
    RemoveServer { id: String, label: String },
    /// Publish the endpoint script to Ecoverse.
    Publish { script: String },
    /// A decision about one device: retry it, retry it elsewhere, or mark it resolved.
    Device { device: usize, op: DeviceOp, question: String },
}

impl MigrateConfirm {
    pub fn prompt(&self) -> String {
        match self {
            MigrateConfirm::Launch { wave, label, ecoverse, rest } => {
                let total = ecoverse + rest;
                format!("Launch wave {wave} ({label}) on {total} device{}? This removes their old agents.", if total == 1 { "" } else { "s" })
            }
            MigrateConfirm::Device { question, .. } => question.clone(),
            MigrateConfirm::Wave { count, max } => format!("Make a wave of the next {} waiting device{} (at most {max})?", count.min(max), if *count.min(max) == 1 { "" } else { "s" }),
            MigrateConfirm::RemoveServer { label, .. } => format!("Remove the server {label} and its saved JWT? Projects that used it are not affected (none do)."),
            MigrateConfirm::Tokens => "Create registration tokens on the destination for the customers and sites that have none?".into(),
            MigrateConfirm::Publish { script } => format!("Publish {script} to Ecoverse? It replaces an older version of the script."),
            MigrateConfirm::Cleanup { name, .. } => format!("Run cleanup on {name} now? Removes the migration's task and folder."),
        }
    }
}

/// "5 min", "2 h 10 min", "3 d": how long ago an RFC 3339 time was, for the stage column.
pub fn since(entered: &str, now: DateTime<Utc>) -> String {
    let Ok(at) = DateTime::parse_from_rfc3339(entered) else { return "-".into() };
    let minutes = (now - at.with_timezone(&Utc)).num_minutes().max(0);
    match minutes {
        0 => "now".into(),
        1..=59 => format!("{minutes} min"),
        60..=1439 => format!("{} h {} min", minutes / 60, minutes % 60),
        _ => format!("{} d", minutes / 1440),
    }
}

fn instance() -> String {
    format!("tui-{}", std::process::id())
}

/// The endpoint script: the one built into this binary, or (for trying a change without rebuilding) the built file
/// named by `NVOKE_MIGRATION_SCRIPT`.
fn load_package() -> Result<Package, String> {
    match std::env::var_os("NVOKE_MIGRATION_SCRIPT").map(PathBuf::from) {
        None => Package::embedded(),
        Some(path) => {
            let content = std::fs::read_to_string(&path).map_err(|e| format!("Could not read {}: {e}.", path.display()))?;
            Package::from_built(&content)
        }
    }
}

impl App {
    pub(crate) fn enter_migrate(&mut self) {
        if matches!(self.migrate.projects, Load::NotLoaded) {
            self.reload_projects();
        }
    }

    fn reload_projects(&mut self) {
        self.migrate.projects = Load::Loading;
        self.spawn(|core| async move { Msg::Migrate(Box::new(MigrateMsg::Projects(core.migration_list().await))) });
    }

    /// Whether the page is showing a project that this window may change.
    fn writable_open(&self) -> bool {
        self.migrate.open.as_ref().is_some_and(|o| !o.read_only)
    }

    // ---- keys --------------------------------------------------------------

    pub(crate) fn on_migrate_key(&mut self, key: KeyEvent) {
        if self.migrate_modal_open() {
            return self.on_migrate_modal_key(key);
        }
        if self.migrate.open.is_some() {
            return self.on_project_key(key);
        }
        let len = match &self.migrate.projects {
            Load::Ready(list) => list.len(),
            _ => 0,
        };
        if self.migrate.form.is_some() {
            return self.on_new_project_key(key);
        }
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => self.focus = Focus::Sidebar,
            KeyCode::Char('n') => self.open_new_project(),
            KeyCode::Char('S') => self.open_servers(),
            KeyCode::Char('r') => self.reload_projects(),
            KeyCode::Down | KeyCode::Char('j') => self.migrate.list.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.migrate.list.select_previous(),
            KeyCode::Home | KeyCode::Char('g') => self.migrate.list.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.migrate.list.select(len.checked_sub(1)),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.open_selected_project(),
            _ => {}
        }
    }

    fn open_selected_project(&mut self) {
        let Load::Ready(list) = &self.migrate.projects else { return };
        let Some(summary) = self.migrate.list.selected().and_then(|i| list.get(i)) else { return };
        let id = summary.id.clone();
        self.migrate.busy = Some("Opening…");
        self.spawn(move |core| async move {
            let result = async {
                let project = core.migration_load(&id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
                let writable = core.migration_lease(&id, &instance()).await?;
                let servers = core.ncentral_servers().await.unwrap_or_default();
                let label = |server_id: &str| servers.iter().find(|s| s.id == server_id).map_or_else(|| server_id.to_string(), |s| s.label.clone());
                let labels = (label(&project.source_server_id), label(&project.dest_server_id));
                Ok((project, !writable, labels))
            }
            .await;
            Msg::Migrate(Box::new(MigrateMsg::Opened(result)))
        });
    }

    fn close_project(&mut self) {
        if let Some(open) = self.migrate.open.take().filter(|o| !o.read_only) {
            let id = open.project.id;
            self.spawn(move |core| async move {
                let _ = core.migration_release_lease(&id, &instance()).await;
                Msg::Migrate(Box::new(MigrateMsg::Projects(core.migration_list().await)))
            });
        }
    }

    fn on_project_key(&mut self, key: KeyEvent) {
        if self.migrate.notes.is_some() {
            return self.on_notes_key(key);
        }
        if self.migrate.choosing.is_some() {
            return self.on_choose_key(key);
        }
        let rows = self.migrate.open.as_ref().map_or(0, |o| o.project.devices.len());
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => self.close_project(),
            KeyCode::Down | KeyCode::Char('j') => self.open_mut().table.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.open_mut().table.select_previous(),
            KeyCode::PageDown => self.open_mut().table.scroll_down_by(10),
            KeyCode::PageUp => self.open_mut().table.scroll_up_by(10),
            KeyCode::Home | KeyCode::Char('g') => self.open_mut().table.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.open_mut().table.select(rows.checked_sub(1)),
            KeyCode::Char('p') => self.start_poll(true),
            KeyCode::Char('L') => self.ask_launch(),
            KeyCode::Char('s') => self.start_probe(false),
            KeyCode::Char('C') => self.ask_cleanup(),
            KeyCode::Char('x') => self.start_export(),
            KeyCode::Char('P') => self.start_plan(),
            KeyCode::Char('w') => self.ask_wave(),
            KeyCode::Char('U') => self.ask_publish(),
            KeyCode::Char('T') => {
                if self.can_change() {
                    self.confirm = Some(Confirm::Migrate(MigrateConfirm::Tokens));
                }
            }
            KeyCode::Char('R') => self.ask_device_op(false),
            KeyCode::Char('O') => self.ask_device_op(true),
            KeyCode::Char('v') => self.ask_resolve(),
            KeyCode::Char('m') => self.start_choose(),
            _ => {}
        }
    }

    // ---- planning, waves, publishing -----------------------------------------

    fn start_plan(&mut self) {
        if !self.can_change() {
            return;
        }
        let Some(open) = &self.migrate.open else { return };
        let id = open.project.id.clone();
        self.migrate.busy = Some("Planning…");
        self.spawn(move |core| async move { Msg::Migrate(Box::new(MigrateMsg::Planned(core.migration_plan_saved(&id, &instance(), None).await))) });
    }

    fn ask_wave(&mut self) {
        if !self.can_change() {
            return;
        }
        let Some(open) = &self.migrate.open else { return };
        let waiting = open.project.devices.iter().filter(|d| d.stage == Stage::Planned && d.wave.is_none()).count();
        if waiting == 0 {
            return self.flash(NoticeKind::Info, "No device is waiting for a wave. Press P to plan the source's devices first.");
        }
        let max = open.project.options.wave_size as usize;
        self.confirm = Some(Confirm::Migrate(MigrateConfirm::Wave { count: waiting, max }));
    }

    fn ask_publish(&mut self) {
        if !self.can_change() {
            return;
        }
        let Some(open) = &self.migrate.open else { return };
        if open.project.source_ecoverse_org_ids.is_empty() {
            return self.flash(NoticeKind::Info, "This project doesn't use Ecoverse (no source organization is set), so there's nothing to publish: devices go through N-central.");
        }
        match Package::embedded() {
            Ok(package) => self.confirm = Some(Confirm::Migrate(MigrateConfirm::Publish { script: package.script_name() })),
            Err(e) => self.flash(NoticeKind::Error, e),
        }
    }

    fn on_notes_key(&mut self, key: KeyEvent) {
        let Some(notes) = self.migrate.notes.as_mut() else { return };
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => notes.scroll = notes.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => notes.scroll = notes.scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => notes.scroll = notes.scroll.saturating_add(8),
            KeyCode::PageUp => notes.scroll = notes.scroll.saturating_sub(8),
            _ => self.migrate.notes = None,
        }
    }

    // ---- the new-project form ------------------------------------------------

    fn open_new_project(&mut self) {
        if self.migrate.busy.is_some() {
            return;
        }
        self.migrate.busy = Some("Reading servers…");
        self.spawn(|core| async move { Msg::Migrate(Box::new(MigrateMsg::Servers(core.ncentral_servers().await))) });
    }

    pub(crate) fn on_new_project_key(&mut self, key: KeyEvent) {
        let Some(form) = self.migrate.form.as_mut() else { return };
        if form.busy {
            return;
        }
        match key.code {
            KeyCode::Esc => self.migrate.form = None,
            KeyCode::Tab | KeyCode::Down => form.step(1),
            KeyCode::BackTab | KeyCode::Up => form.step(-1),
            KeyCode::Char('a') if matches!(form.field, NpField::Source | NpField::Dest) => {
                let as_dest = form.field == NpField::Dest;
                self.start_add_server(Some(as_dest));
            }
            KeyCode::Left if form.input().is_none() => form.cycle(false),
            KeyCode::Right if form.input().is_none() => form.cycle(true),
            KeyCode::Enter => match form.validate() {
                Err(e) => form.error = Some(e),
                Ok(args) => {
                    form.error = None;
                    form.busy = true;
                    self.spawn(move |core| async move {
                        Msg::Migrate(Box::new(MigrateMsg::Created(core.migration_create_full(&args.name, &args.source, &args.dest, args.dest_ecoverse, args.source_orgs, args.dest_orgs).await)))
                    });
                }
            },
            _ => {
                if let Some(input) = form.input() {
                    input.handle_key(key);
                    form.error = None;
                }
            }
        }
    }

    // ---- decisions about one device ------------------------------------------

    fn selected_device_info(&self) -> Option<(usize, String, Stage, Option<DeliveryPath>, bool)> {
        let open = self.migrate.open.as_ref()?;
        let index = open.selected_device()?;
        let d = &open.project.devices[index];
        Some((index, d.name.clone(), d.stage, d.path, d.launch_unsure))
    }

    fn ask_device_op(&mut self, other_path: bool) {
        if !self.can_change() {
            return;
        }
        let Some((device, name, stage, path, unsure)) = self.selected_device_info() else { return };
        if !nvoke_core::migration::workflow::retryable(stage, unsure) {
            return self.flash(NoticeKind::Info, "Retry is for a device whose launch failed, wasn't picked up, or is stuck.");
        }
        let target = match (other_path, path) {
            (false, Some(p)) => p,
            (true, Some(DeliveryPath::Ecoverse)) => DeliveryPath::Rest,
            (true, Some(DeliveryPath::Rest)) => DeliveryPath::Ecoverse,
            (_, None) => return self.flash(NoticeKind::Error, format!("{name} has no delivery path: plan it again.")),
        };
        let via = if target == DeliveryPath::Rest { "N-central" } else { "Ecoverse" };
        let cancel = if stage == Stage::NotPickedUp && path == Some(DeliveryPath::Ecoverse) { " Its waiting launch is cancelled first." } else { "" };
        let question = if unsure {
            format!("{name}'s last launch may have run. Retry through {via} only if Activity shows it didn't?")
        } else {
            format!("Retry {name} through {via}?{cancel}")
        };
        self.confirm = Some(Confirm::Migrate(MigrateConfirm::Device { device, op: DeviceOp::Retry { other_path, checked: unsure }, question }));
    }

    fn ask_resolve(&mut self) {
        if !self.can_change() {
            return;
        }
        let Some((device, name, stage, _, _)) = self.selected_device_info() else { return };
        if !(stage.needs_attention() || stage == Stage::Unreachable) {
            return self.flash(NoticeKind::Info, "Mark resolved is for a device that needs attention and was fixed outside N-voke.");
        }
        let question = format!("Mark {name} resolved? N-voke stops tracking it: fix it outside N-voke first.");
        self.confirm = Some(Confirm::Migrate(MigrateConfirm::Device { device, op: DeviceOp::Resolve, question }));
    }

    fn start_choose(&mut self) {
        if !self.can_change() {
            return;
        }
        let Some(open) = &self.migrate.open else { return };
        let Some(index) = open.selected_device() else { return };
        if open.project.devices[index].match_candidates.is_empty() {
            return self.flash(NoticeKind::Info, "Nothing to choose: only one destination device fits this one, or none has appeared yet.");
        }
        let mut table = TableState::default();
        table.select(Some(0));
        self.migrate.choosing = Some(Choosing { device: index, table });
    }

    fn on_choose_key(&mut self, key: KeyEvent) {
        let Some(choosing) = self.migrate.choosing.as_mut() else { return };
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h' | 'q') => self.migrate.choosing = None,
            KeyCode::Down | KeyCode::Char('j') => choosing.table.select_next(),
            KeyCode::Up | KeyCode::Char('k') => choosing.table.select_previous(),
            KeyCode::Enter => {
                let device = choosing.device;
                let pick = choosing.table.selected();
                self.migrate.choosing = None;
                let candidate = self.migrate.open.as_ref().and_then(|o| o.project.devices.get(device)).and_then(|d| pick.and_then(|i| d.match_candidates.get(i))).map(|c| c.device_id.clone());
                if let Some(dest_device_id) = candidate {
                    self.spawn_device_op(device, DeviceOp::ChooseMatch { dest_device_id });
                }
            }
            _ => {}
        }
    }

    fn spawn_device_op(&mut self, device: usize, op: DeviceOp) {
        let Some(open) = &self.migrate.open else { return };
        let id = open.project.id.clone();
        self.migrate.busy = Some("Applying…");
        self.spawn(move |core| async move { Msg::Migrate(Box::new(MigrateMsg::Operated(core.migration_device_op_saved(&id, &instance(), device, op).await))) });
    }

    fn open_mut(&mut self) -> &mut OpenProject {
        self.migrate.open.as_mut().expect("a project is open")
    }

    /// Refuses with a message when something else is under way or the project is read-only.
    fn can_change(&mut self) -> bool {
        if self.migrate.busy.is_some() {
            self.flash(NoticeKind::Info, "Wait for the current step to finish.");
            return false;
        }
        if !self.writable_open() {
            self.flash(NoticeKind::Error, "Another N-voke window is working on this project, so this one can only look.");
            return false;
        }
        true
    }

    // ---- polling -----------------------------------------------------------

    pub(crate) fn tick_migrate(&mut self) {
        if self.page != Page::Migrate || self.migrate.busy.is_some() || !self.writable_open() {
            return;
        }
        if self.tick.saturating_sub(self.migrate.last_poll_tick) >= AUTO_POLL_TICKS {
            self.start_poll(false);
        }
    }

    fn start_poll(&mut self, manual: bool) {
        self.migrate.last_poll_tick = self.tick;
        if manual && !self.can_change() || !manual && !self.writable_open() {
            return;
        }
        let Some(open) = &self.migrate.open else { return };
        let id = open.project.id.clone();
        self.migrate.busy = Some("Polling…");
        self.spawn(move |core| async move { Msg::Migrate(Box::new(MigrateMsg::Polled(core.migration_poll_saved(&id, &instance(), &Thresholds::default()).await))) });
    }

    // ---- launching ---------------------------------------------------------

    /// The lowest-numbered wave that still has devices waiting, and what launching it would do.
    pub fn next_launch(project: &Project) -> Option<MigrateConfirm> {
        let mut waves: Vec<u32> = project.waves.iter().map(|w| w.number).collect();
        waves.sort_unstable();
        waves.into_iter().find_map(|wave| {
            let plan = plan_wave(project, wave);
            let rest = plan.rest.len();
            let ecoverse: usize = plan.ecoverse.iter().map(|g| g.devices.len()).sum();
            (ecoverse + rest > 0).then(|| MigrateConfirm::Launch { wave, label: project.waves.iter().find(|w| w.number == wave).map(|w| w.label.clone()).unwrap_or_default(), ecoverse, rest })
        })
    }

    fn ask_launch(&mut self) {
        if !self.can_change() {
            return;
        }
        match self.migrate.open.as_ref().and_then(|o| Self::next_launch(&o.project)) {
            Some(confirm) => self.confirm = Some(Confirm::Migrate(confirm)),
            None => self.flash(NoticeKind::Info, "No wave has devices waiting. Make one with: nvoke migrate-project wave <project>"),
        }
    }

    fn ask_cleanup(&mut self) {
        if !self.can_change() {
            return;
        }
        let Some(open) = &self.migrate.open else { return };
        let Some(index) = open.selected_device() else { return };
        let device = &open.project.devices[index];
        if device.stage != Stage::Healthy {
            return self.flash(NoticeKind::Info, "Cleanup is for healthy devices: it removes what the migration left on them.");
        }
        self.confirm = Some(Confirm::Migrate(MigrateConfirm::Cleanup { device: index, name: device.name.clone() }));
    }

    pub(crate) fn run_migrate_confirmed(&mut self, confirm: MigrateConfirm) {
        if let MigrateConfirm::RemoveServer { id, .. } = confirm {
            return self.run_remove_server(id);
        }
        let Some(open) = &self.migrate.open else { return };
        let id = open.project.id.clone();
        match confirm {
            MigrateConfirm::Launch { wave, .. } => {
                self.migrate.busy = Some("Launching…");
                self.spawn(move |core| async move {
                    let result = async {
                        let package = load_package()?;
                        let mut project = core.migration_open_for_write(&id, &instance()).await?;
                        let script_id = project.ecoverse_script_id.clone();
                        let report = core.migration_launch_wave(&mut project, wave, &package, script_id.as_deref()).await;
                        core.migration_save(&mut project).await?;
                        let report = report?;
                        Ok((project, report))
                    }
                    .await;
                    Msg::Migrate(Box::new(MigrateMsg::Launched(result)))
                });
            }
            MigrateConfirm::Cleanup { device, .. } => self.spawn_action(id, device, Operation::Cleanup),
            MigrateConfirm::Device { device, op, .. } => self.spawn_device_op(device, op),
            MigrateConfirm::Wave { max, .. } => {
                self.migrate.busy = Some("Making a wave…");
                self.spawn(move |core| async move { Msg::Migrate(Box::new(MigrateMsg::Operated(core.migration_wave_saved(&id, &instance(), "", Some(max)).await))) });
            }
            MigrateConfirm::RemoveServer { .. } => {}
            MigrateConfirm::Tokens => {
                let Some(project) = self.migrate.open.as_ref().map(|o| o.project.clone()) else { return };
                self.migrate.busy = Some("Creating tokens…");
                self.spawn(move |core| async move { Msg::Migrate(Box::new(MigrateMsg::Tokens(core.migration_generate_tokens(&project, None).await))) });
            }
            MigrateConfirm::Publish { .. } => {
                self.migrate.busy = Some("Publishing…");
                self.spawn(move |core| async move { Msg::Migrate(Box::new(MigrateMsg::Operated(core.migration_publish_saved(&id, &instance()).await))) });
            }
        }
    }

    // ---- probing -----------------------------------------------------------

    fn start_probe(&mut self, _cleanup: bool) {
        if !self.can_change() {
            return;
        }
        let Some(index) = self.migrate.open.as_ref().and_then(|o| o.selected_device()) else { return };
        let id = self.migrate.open.as_ref().map(|o| o.project.id.clone()).unwrap_or_default();
        self.spawn_action(id, index, Operation::Status);
    }

    fn spawn_action(&mut self, id: String, device: usize, operation: Operation) {
        self.migrate.busy = Some(if operation == Operation::Cleanup { "Running cleanup…" } else { "Asking the device…" });
        self.spawn(move |core| async move {
            let result = async {
                let package = load_package()?;
                let mut project = core.migration_open_for_write(&id, &instance()).await?;
                let outcome = core.migration_run_action(&mut project, device, operation, &package, std::time::Duration::from_secs(PROBE_WAIT_SECONDS)).await;
                core.migration_save(&mut project).await?;
                let outcome = outcome?;
                let summary = format!("{} via {}: {}", project.devices[device].name, outcome.via, if outcome.result.message.is_empty() { outcome.result.result.clone() } else { outcome.result.message.clone() });
                Ok((project, summary))
            }
            .await;
            Msg::Migrate(Box::new(MigrateMsg::Probed(result)))
        });
    }

    // ---- reports -----------------------------------------------------------

    fn start_export(&mut self) {
        let Some(open) = &self.migrate.open else { return };
        let project = open.project.clone();
        let dir = self.export_dir.clone();
        self.spawn(move |core| async move { Msg::Migrate(Box::new(MigrateMsg::Exported(core.migration_write_report(&project, &Filter::default(), Format::Html, &dir).map(|p| p.display().to_string())))) });
    }

    // ---- results -----------------------------------------------------------

    pub(crate) fn on_migrate_msg(&mut self, msg: MigrateMsg) {
        match msg {
            MigrateMsg::Projects(Ok(list)) => {
                let keep = self.migrate.list.selected().unwrap_or(0).min(list.len().saturating_sub(1));
                self.migrate.list.select((!list.is_empty()).then_some(keep));
                self.migrate.projects = Load::Ready(list);
            }
            MigrateMsg::Projects(Err(e)) => self.migrate.projects = Load::Failed(e),
            MigrateMsg::Opened(Ok((project, read_only, server_labels))) => {
                self.migrate.busy = None;
                let mut table = TableState::default();
                table.select((!project.devices.is_empty()).then_some(0));
                self.migrate.last_poll_tick = self.tick;
                if read_only {
                    self.flash(NoticeKind::Info, "Another N-voke window is working on this project: showing it read-only.");
                }
                self.migrate.open = Some(OpenProject { project, read_only, server_labels, table, last_report: None, polled_at_tick: None });
            }
            MigrateMsg::Opened(Err(e)) => {
                self.migrate.busy = None;
                self.flash(NoticeKind::Error, e);
            }
            MigrateMsg::Polled(result) => {
                self.migrate.busy = None;
                match result {
                    Ok((project, report)) => {
                        let tick = self.tick;
                        if let Some(open) = &mut self.migrate.open {
                            open.project = project;
                            open.polled_at_tick = Some(tick);
                            if let Some(first) = report.errors.first() {
                                let text = first.clone();
                                self.flash(NoticeKind::Error, text);
                            }
                            if let Some(open) = &mut self.migrate.open {
                                open.last_report = Some(report);
                            }
                        }
                    }
                    Err(e) => self.flash(NoticeKind::Error, e),
                }
            }
            MigrateMsg::Launched(result) => {
                self.migrate.busy = None;
                match result {
                    Ok((project, report)) => {
                        let text = if let Some(stopped) = &report.stopped {
                            format!("Launch stopped: {stopped} {} launched before it stopped.", report.launched_ecoverse + report.launched_rest)
                        } else if report.held_back > 0 {
                            format!("Dry run: {} device(s) held back; nothing was sent. Press d to see the requests.", report.held_back)
                        } else {
                            format!("Launched {} through Ecoverse and {} through N-central{}.", report.launched_ecoverse, report.launched_rest, if report.failed.is_empty() { String::new() } else { format!("; {} failed (see the device's timeline)", report.failed.len()) })
                        };
                        if let Some(open) = &mut self.migrate.open {
                            open.project = project;
                        }
                        self.flash(if report.failed.is_empty() && report.stopped.is_none() { NoticeKind::Info } else { NoticeKind::Error }, text);
                    }
                    Err(e) if is_dry_run_error(&e) => self.flash(NoticeKind::Info, "Dry run: nothing was sent."),
                    Err(e) => self.flash(NoticeKind::Error, e),
                }
            }
            MigrateMsg::Probed(result) => {
                self.migrate.busy = None;
                match result {
                    Ok((project, summary)) => {
                        if let Some(open) = &mut self.migrate.open {
                            open.project = project;
                        }
                        self.flash(NoticeKind::Info, summary);
                    }
                    Err(e) if is_dry_run_error(&e) => self.flash(NoticeKind::Info, "Dry run: nothing was sent."),
                    Err(e) => self.flash(NoticeKind::Error, e),
                }
            }
            MigrateMsg::Operated(result) => {
                self.migrate.busy = None;
                match result {
                    Ok((project, message)) => {
                        if let Some(open) = &mut self.migrate.open {
                            open.project = project;
                        }
                        self.flash(NoticeKind::Info, message);
                    }
                    Err(e) if is_dry_run_error(&e) => self.flash(NoticeKind::Info, "Dry run: nothing was sent. Press d to see what would have been."),
                    Err(e) => self.flash(NoticeKind::Error, e),
                }
            }
            MigrateMsg::Servers(result) => {
                self.migrate.busy = None;
                match result {
                    Ok(servers) if servers.is_empty() => {
                        self.flash(NoticeKind::Info, "No servers yet: add the one devices are moving from (add the destination next).");
                        self.start_add_server(Some(false));
                    }
                    Ok(servers) => self.migrate.form = Some(NewProject::new(servers)),
                    Err(e) => self.flash(NoticeKind::Error, e),
                }
            }
            MigrateMsg::Created(result) => match result {
                Ok(project) => {
                    self.migrate.form = None;
                    self.reload_projects();
                    self.flash(NoticeKind::Info, format!("Created {}. Open it and press P to plan its devices.", project.name));
                }
                Err(e) => {
                    if let Some(form) = &mut self.migrate.form {
                        form.busy = false;
                        form.error = Some(e);
                    }
                }
            },
            MigrateMsg::ServerList(result) => self.on_server_list(result),
            MigrateMsg::ServerSaved(result) => self.on_server_saved(result),
            MigrateMsg::ServerRemoved(result) => self.on_server_removed(result),
            MigrateMsg::ServerTested(result) => self.on_server_tested(result),
            MigrateMsg::FormServers(result) => self.on_form_servers(result),
            MigrateMsg::Tokens(result) => {
                self.migrate.busy = None;
                match result {
                    Ok(report) => {
                        let mut lines: Vec<String> = crate::migrate_cli::render_tokens(&report).lines().map(String::from).collect();
                        lines.push(String::new());
                        lines.push("Press P to plan again: devices that were waiting on a token can now be added.".into());
                        self.migrate.notes = Some(Notes { title: "registration tokens".into(), lines, scroll: 0 });
                    }
                    Err(e) => self.flash(NoticeKind::Error, e),
                }
            }
            MigrateMsg::Planned(result) => {
                self.migrate.busy = None;
                match result {
                    Ok((project, report)) => {
                        let mut lines = vec![format!("Added {} ready device{} to the project ({} already in it).", report.added, if report.added == 1 { "" } else { "s" }, report.already_in)];
                        if !report.warnings.is_empty() {
                            lines.push(String::new());
                            lines.extend(report.warnings.iter().map(|w| format!("! {w}")));
                        }
                        if !report.blocked.is_empty() {
                            lines.push(String::new());
                            lines.push(format!("{} device{} can't be launched yet:", report.blocked.len(), if report.blocked.len() == 1 { "" } else { "s" }));
                            for (name, reasons) in &report.blocked {
                                lines.push(format!("  {name}"));
                                lines.extend(reasons.iter().map(|r| format!("      {r}")));
                            }
                            lines.push(String::new());
                            lines.push("Fix what's listed and press P again, or leave those devices out.".into());
                        }
                        if report.added > 0 {
                            lines.push(String::new());
                            lines.push("Press w to put the waiting devices in a wave.".into());
                        }
                        if let Some(open) = &mut self.migrate.open {
                            open.project = project;
                            if open.table.selected().is_none() && !open.project.devices.is_empty() {
                                open.table.select(Some(0));
                            }
                        }
                        self.migrate.notes = Some(Notes { title: "planned".into(), lines, scroll: 0 });
                    }
                    Err(e) => self.flash(NoticeKind::Error, e),
                }
            }
            MigrateMsg::Exported(Ok(path)) => self.flash(NoticeKind::Info, format!("Saved the report to {path}")),
            MigrateMsg::Exported(Err(e)) => self.flash(NoticeKind::Error, e),
        }
    }
}

/// What the path says about a device, for the table: its delivery path as a word.
pub fn path_word(path: Option<DeliveryPath>) -> &'static str {
    match path {
        Some(DeliveryPath::Ecoverse) => "Ecoverse",
        Some(DeliveryPath::Rest) => "N-central",
        None => "-",
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use chrono::Duration;
    use nvoke_core::migration::{
        planner::PlannedDevice,
        project::{DestEcoverse, stamp},
    };
    use ratatui::{Terminal, backend::TestBackend, crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers}};

    use super::*;
    use crate::app::run::tests::{pump, run_app};

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    }

    fn planned(id: &str, name: &str, path: DeliveryPath) -> PlannedDevice {
        PlannedDevice {
            device_id: id.into(),
            name: name.into(),
            source_customer_id: Some("10".into()),
            source_site_id: None,
            dest_org_unit_id: Some("110".into()),
            path: Some(path),
            asset_id: (path == DeliveryPath::Ecoverse).then(|| format!("asset-{id}")),
            blockers: vec![],
            warnings: vec![],
            excluded: false,
        }
    }

    /// A project of four devices in two waves: a healthy one, a lost one, and two waiting in wave 2.
    pub(crate) fn project() -> Project {
        let now = Utc::now();
        let mut p = Project::new("mig-test".into(), "Acme move", "srv-a", "srv-b", DestEcoverse::NotLinked, now);
        p.add_planned(&[planned("1", "ALPHA", DeliveryPath::Rest), planned("2", "BRAVO", DeliveryPath::Rest), planned("3", "CHARLIE", DeliveryPath::Ecoverse), planned("4", "DELTA", DeliveryPath::Rest)], now);
        p.create_wave("Pilot", &["1".into(), "2".into()], now).unwrap();
        p.create_wave("Main", &["3".into(), "4".into()], now).unwrap();
        p.devices[0].stage = Stage::Healthy;
        p.devices[0].entered_stage_at = stamp(now - Duration::hours(3));
        p.devices[1].stage = Stage::Lost;
        p.devices[1].last_error = Some("nothing on the destination".into());
        p.devices[1].entered_stage_at = stamp(now - Duration::minutes(260));
        p
    }

    /// An app whose core already holds the test project (and servers), on the Migrate page with focus in it.
    fn migrate_app() -> (tokio::runtime::Runtime, App) {
        migrate_app_with(|_| {})
    }

    /// The same, with the saved project changed first.
    fn migrate_app_with(change: impl FnOnce(&mut Project)) -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = run_app();
        let core = app.core().clone();
        rt.block_on(async {
            let a = core.save_ncentral_server(None, "Old server", "old.example.com", Some("j1".into()), None).await.unwrap();
            let b = core.save_ncentral_server(None, "New server", "new.example.com", Some("j2".into()), None).await.unwrap();
            let mut p = project();
            p.source_server_id = a.id;
            p.dest_server_id = b.id;
            change(&mut p);
            core.migration_save(&mut p).await.unwrap();
        });
        app.go(Page::Migrate);
        app.focus = Focus::Content;
        (rt, app)
    }

    fn render(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(130, 30)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn the_page_is_reachable_by_number_and_lists_the_saved_projects() {
        let (_rt, mut app) = migrate_app();
        app.focus = Focus::Sidebar;
        press(&mut app, KeyCode::Char('4'));
        assert!(app.page == Page::Migrate);
        pump(&mut app);
        let screen = render(&mut app);
        assert!(screen.contains("migration projects") && screen.contains("Acme move") && screen.contains("4"), "{screen}");
    }

    #[test]
    fn an_empty_list_says_how_to_make_a_project() {
        let (_rt, mut app) = run_app();
        app.go(Page::Migrate);
        pump(&mut app);
        let screen = render(&mut app);
        assert!(screen.contains("No migration projects yet") && screen.contains("nvoke migrate-project create"), "{screen}");
    }

    #[test]
    fn opening_a_project_shows_its_devices_attention_first_with_the_selected_ones_timeline() {
        let (_rt, mut app) = migrate_app();
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        let open = app.migrate.open.as_ref().expect("opened");
        assert!(!open.read_only);
        let names: Vec<&str> = open.order().iter().map(|&i| open.project.devices[i].name.as_str()).collect();
        assert_eq!(names[0], "BRAVO", "the lost device comes first: {names:?}");
        let screen = render(&mut app);
        assert!(screen.contains("Acme move") && screen.contains("Old server") && screen.contains("New server"), "{screen}");
        assert!(screen.contains("1 need attention") && screen.contains("Lost") && screen.contains("Healthy"), "{screen}");
        assert!(screen.contains("nothing on the destination") && screen.contains("timeline"), "the lost device's reason is in its timeline: {screen}");
        assert!(screen.contains("4 h 20 min"), "how long it has been lost: {screen}");
        assert!(app.page == Page::Migrate && render(&mut app).contains("Migrate / Acme move"), "the breadcrumb names the project");
        press(&mut app, KeyCode::Esc);
        assert!(app.migrate.open.is_none(), "esc closes the project, not the page");
    }

    #[test]
    fn a_project_another_window_holds_opens_read_only_and_refuses_changes() {
        let (rt, mut app) = migrate_app();
        let core = app.core().clone();
        assert!(rt.block_on(core.migration_lease("mig-test", "another-window")).unwrap());
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        assert!(app.migrate.open.as_ref().unwrap().read_only);
        assert!(render(&mut app).contains("read-only"));
        press(&mut app, KeyCode::Char('L'));
        assert!(app.confirm.is_none(), "nothing to confirm: this window can only look");
        assert!(app.flash.as_ref().unwrap().0.text.contains("can only look"));
        press(&mut app, KeyCode::Char('p'));
        assert!(app.migrate.busy.is_none(), "and it doesn't poll");
    }

    #[test]
    fn launch_asks_first_naming_the_wave_the_count_and_that_agents_are_removed() {
        let (_rt, mut app) = migrate_app();
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        press(&mut app, KeyCode::Char('L'));
        let prompt = app.confirm.as_ref().expect("asks first").prompt();
        assert!(prompt.contains("wave 1") || prompt.contains("wave 2"), "{prompt}");
        assert!(prompt.contains("removes their old agents"), "{prompt}");
        // Pilot's devices are no longer waiting (healthy and lost), so the next wave is Main: 1 Ecoverse, 1 N-central.
        assert_eq!(prompt, "Launch wave 2 (Main) on 2 devices? This removes their old agents.");
        // Anything but y cancels, and nothing starts.
        press(&mut app, KeyCode::Char('n'));
        assert!(app.confirm.is_none() && app.migrate.busy.is_none());
    }

    #[test]
    fn confirming_a_launch_runs_it_with_the_script_built_into_nvoke_and_reports_what_stopped_it() {
        let (_rt, mut app) = migrate_app();
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        press(&mut app, KeyCode::Char('L'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.migrate.busy, Some("Launching…"));
        pump(&mut app);
        assert!(app.migrate.busy.is_none());
        // The script loaded; the test project has no Ecoverse script published, so the launch is stopped with a clear message.
        let flash = &app.flash.as_ref().unwrap().0;
        assert!(flash.text.contains("Publish the endpoint script to Ecoverse first"), "{}", flash.text);
        let open = app.migrate.open.as_ref().unwrap();
        assert!(open.project.devices.iter().all(|d| d.stage != Stage::Launched), "nothing was launched");
    }

    #[test]
    fn one_step_at_a_time_and_cleanup_is_only_for_healthy_devices() {
        let (_rt, mut app) = migrate_app();
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        // The selected row is the lost device: cleanup isn't for it.
        press(&mut app, KeyCode::Char('C'));
        assert!(app.confirm.is_none() && app.flash.as_ref().unwrap().0.text.contains("healthy devices"));
        // Down to ALPHA (healthy): cleanup asks.
        let target = app.migrate.open.as_ref().unwrap().order().iter().position(|&i| app.migrate.open.as_ref().unwrap().project.devices[i].name == "ALPHA").unwrap();
        for _ in 0..target {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Char('C'));
        assert!(app.confirm.as_ref().unwrap().prompt().starts_with("Run cleanup on ALPHA now?"));
        press(&mut app, KeyCode::Char('x'));
        // While something is under way a second step is refused.
        app.migrate.busy = Some("Polling…");
        press(&mut app, KeyCode::Char('L'));
        assert!(app.confirm.is_none() && app.flash.as_ref().unwrap().0.text.contains("Wait for the current step"));
    }

    #[test]
    fn a_dry_run_reminds_you_in_the_confirmation_that_nothing_is_sent() {
        let (_rt, mut app) = migrate_app();
        app.core().set_dry_run(true);
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        press(&mut app, KeyCode::Char('L'));
        let screen = render(&mut app);
        assert!(screen.contains("dry run: nothing is sent") && screen.contains("y to confirm"), "the whole question fits on the line: {screen}");
    }

    #[test]
    fn an_open_writable_project_polls_itself_every_thirty_seconds_and_only_on_this_page() {
        let (_rt, mut app) = migrate_app();
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        app.migrate.last_poll_tick = app.tick;
        app.tick += AUTO_POLL_TICKS - 1;
        app.tick_migrate();
        assert!(app.migrate.busy.is_none(), "not yet");
        app.tick += 1;
        app.page = Page::Audit;
        app.tick_migrate();
        assert!(app.migrate.busy.is_none(), "not while looking at another page");
        app.page = Page::Migrate;
        app.tick_migrate();
        assert_eq!(app.migrate.busy, Some("Polling…"));
        pump(&mut app);
        assert!(app.migrate.busy.is_none());
        assert!(app.migrate.open.as_ref().unwrap().polled_at_tick.is_some());
        assert!(render(&mut app).contains("polled 0 s ago"));
    }

    #[test]
    fn how_long_ago_reads_naturally() {
        let now: DateTime<Utc> = "2026-10-20T12:00:00Z".parse().unwrap();
        let ago = |m: i64| since(&stamp(now - Duration::minutes(m)), now);
        assert_eq!((ago(0).as_str(), ago(1).as_str(), ago(59).as_str(), ago(60).as_str(), ago(130).as_str(), ago(60 * 24 * 3).as_str()), ("now", "1 min", "59 min", "1 h 0 min", "2 h 10 min", "3 d"));
        assert_eq!(since("garbage", now), "-");
        assert_eq!(since(&stamp(now + Duration::minutes(5)), now), "now", "a clock skew never shows a negative time");
    }

    #[test]
    fn the_next_wave_to_launch_is_the_lowest_with_devices_waiting() {
        let mut p = project();
        match App::next_launch(&p) {
            Some(MigrateConfirm::Launch { wave, ecoverse, rest, .. }) => assert_eq!((wave, ecoverse, rest), (2, 1, 1)),
            other => panic!("{other:?}"),
        }
        p.devices[2].stage = Stage::Launched;
        p.devices[3].stage = Stage::Launched;
        assert!(App::next_launch(&p).is_none(), "everything is launched");
        p.devices[0].stage = Stage::Planned;
        match App::next_launch(&p) {
            Some(MigrateConfirm::Launch { wave, .. }) => assert_eq!(wave, 1),
            other => panic!("{other:?}"),
        }
    }

    /// Opens the test project and moves the highlight to the named device.
    fn open_and_select(app: &mut App, name: &str) {
        pump(app);
        press(app, KeyCode::Enter);
        pump(app);
        let open = app.migrate.open.as_ref().unwrap();
        let row = open.order().iter().position(|&i| open.project.devices[i].name == name).unwrap();
        app.migrate.open.as_mut().unwrap().table.select(Some(row));
    }

    fn stage_of(app: &App, name: &str) -> Stage {
        app.migrate.open.as_ref().unwrap().project.devices.iter().find(|d| d.name == name).unwrap().stage
    }

    #[test]
    fn a_failed_launch_is_retried_after_asking_and_goes_back_to_its_wave() {
        let (_rt, mut app) = migrate_app_with(|p| {
            p.devices[2].stage = Stage::LaunchFailed;
            p.devices[2].last_error = Some("boom".into());
            p.ecoverse_script_id = Some("script-1".into());
        });
        open_and_select(&mut app, "CHARLIE");
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Retry CHARLIE through Ecoverse?");
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.migrate.busy, Some("Applying…"));
        pump(&mut app);
        assert_eq!(stage_of(&app, "CHARLIE"), Stage::Planned);
        assert!(app.flash.as_ref().unwrap().0.text.contains("back in its wave"));
    }

    #[test]
    fn retrying_on_the_other_path_names_it_and_a_refusal_comes_back_as_a_message() {
        let (_rt, mut app) = migrate_app_with(|p| {
            p.devices[2].stage = Stage::NotPickedUp;
            p.devices[2].task_id = Some("t".into());
        });
        open_and_select(&mut app, "CHARLIE");
        press(&mut app, KeyCode::Char('O'));
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Retry CHARLIE through N-central? Its waiting launch is cancelled first.");
        press(&mut app, KeyCode::Char('y'));
        pump(&mut app);
        assert_eq!(stage_of(&app, "CHARLIE"), Stage::NotPickedUp, "the endpoint script isn't uploaded to the source server, so nothing changed");
        assert!(app.flash.as_ref().unwrap().0.text.contains("hasn't been uploaded"), "{}", app.flash.as_ref().unwrap().0.text);
        // The same path on a launch still waiting in Ecoverse says it will cancel it first.
        press(&mut app, KeyCode::Char('R'));
        assert!(app.confirm.as_ref().unwrap().prompt().contains("Its waiting launch is cancelled first."));
    }

    #[test]
    fn retry_is_only_for_devices_that_can_be_retried() {
        let (_rt, mut app) = migrate_app();
        open_and_select(&mut app, "ALPHA");
        press(&mut app, KeyCode::Char('R'));
        assert!(app.confirm.is_none() && app.flash.as_ref().unwrap().0.text.contains("Retry is for a device whose launch failed"));
    }

    #[test]
    fn a_lost_device_fixed_by_hand_is_marked_resolved_after_asking() {
        let (_rt, mut app) = migrate_app();
        open_and_select(&mut app, "BRAVO");
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Mark BRAVO resolved? N-voke stops tracking it: fix it outside N-voke first.");
        press(&mut app, KeyCode::Char('y'));
        pump(&mut app);
        assert_eq!(stage_of(&app, "BRAVO"), Stage::Resolved);
        assert_eq!(app.migrate.open.as_ref().unwrap().project.attention_count(), 0);
        open_and_select(&mut app, "ALPHA");
        press(&mut app, KeyCode::Char('v'));
        assert!(app.confirm.is_none() && app.flash.as_ref().unwrap().0.text.contains("needs attention and was fixed"));
    }

    #[test]
    fn an_ambiguous_match_shows_up_as_attention_and_is_settled_from_a_list() {
        use nvoke_core::migration::project::MatchCandidate;
        let (_rt, mut app) = migrate_app_with(|p| {
            p.devices[3].stage = Stage::AgentRemoved;
            p.devices[3].match_candidates = vec![MatchCandidate { device_id: "5001".into(), name: "DELTA".into() }, MatchCandidate { device_id: "5002".into(), name: "delta".into() }];
        });
        open_and_select(&mut app, "DELTA");
        let screen = render(&mut app);
        assert!(screen.contains("2 need attention") && screen.contains("2 devices fit. Press m to choose."), "{screen}");
        press(&mut app, KeyCode::Char('m'));
        let popup = render(&mut app);
        assert!(popup.contains("which one is DELTA?") && popup.contains("device 5001") && popup.contains("device 5002"), "{popup}");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(app.migrate.choosing.is_none());
        pump(&mut app);
        let device = app.migrate.open.as_ref().unwrap().project.devices.iter().find(|d| d.name == "DELTA").unwrap().clone();
        assert_eq!((device.dest_device_id.as_deref(), device.match_candidates.len()), (Some("5002"), 0));
        assert!(app.flash.as_ref().unwrap().0.text.contains("matched to destination device 5002"));
    }

    #[test]
    fn escape_leaves_the_chooser_without_choosing_and_a_device_with_no_question_says_so() {
        use nvoke_core::migration::project::MatchCandidate;
        let (_rt, mut app) = migrate_app_with(|p| {
            p.devices[3].stage = Stage::Lost;
            p.devices[3].match_candidates = vec![MatchCandidate { device_id: "5001".into(), name: "DELTA".into() }, MatchCandidate { device_id: "5002".into(), name: "delta".into() }];
        });
        open_and_select(&mut app, "ALPHA");
        press(&mut app, KeyCode::Char('m'));
        assert!(app.migrate.choosing.is_none() && app.flash.as_ref().unwrap().0.text.contains("Nothing to choose"));
        open_and_select(&mut app, "DELTA");
        press(&mut app, KeyCode::Char('m'));
        assert!(app.migrate.choosing.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.migrate.choosing.is_none() && app.migrate.busy.is_none());
        assert!(app.migrate.open.is_some(), "esc closed the list, not the project");
    }

    #[test]
    fn a_read_only_window_cant_retry_resolve_or_choose() {
        let (rt, mut app) = migrate_app_with(|p| p.devices[2].stage = Stage::LaunchFailed);
        let core = app.core().clone();
        assert!(rt.block_on(core.migration_lease("mig-test", "another-window")).unwrap());
        open_and_select(&mut app, "CHARLIE");
        for key in ['R', 'O', 'v', 'm'] {
            press(&mut app, KeyCode::Char(key));
            assert!(app.confirm.is_none() && app.migrate.choosing.is_none(), "{key}");
            assert!(app.flash.as_ref().unwrap().0.text.contains("can only look"), "{key}");
        }
    }

    // ---- the new-project form, planning, waves, publishing -------------------

    use nvoke_core::ncentral::rest::{BoxFuture, RestRequest, RestResponse, RestTransport};
    use serde_json::{Value, json};

    /// An N-central pair for planning: Acme on both, two Windows devices on the source (ids 11, 12), a valid token on the destination.
    struct Lab;

    impl RestTransport for Lab {
        fn send<'a>(&'a self, request: RestRequest) -> BoxFuture<'a, Result<RestResponse, String>> {
            let url = request.url.trim_start_matches("https://").to_string();
            let (host, path) = url.split_once('/').unwrap();
            let path = path.split('?').next().unwrap().to_string();
            let host = host.to_string();
            let body: Value = match path.as_str() {
                "api/auth/authenticate" => json!({ "tokens": { "access": { "token": "a", "expirySeconds": 3600 } } }),
                "api/server-info" => json!({ "ncentral": "2026.4.1.1", "version": "1.3.23" }),
                "api/org-units" => {
                    let p = if host.starts_with("old") { 0 } else { 100 };
                    json!({ "data": [{ "orgUnitId": p.to_string(), "orgUnitName": "SO", "orgUnitType": "SO" }, { "orgUnitId": (p + 10).to_string(), "orgUnitName": "Acme", "orgUnitType": "CUSTOMER", "parentId": p.to_string() }], "totalPages": 1 })
                }
                "api/org-units/10/devices" => json!({ "data": [
                    { "deviceId": 11, "longName": "ECHO", "osId": "winnt", "customerId": 10 },
                    { "deviceId": 12, "longName": "FOXTROT", "osId": "winnt", "customerId": 10 }], "totalPages": 1 }),
                "api/org-units/110/registration-token" => json!({ "data": { "registrationToken": "abcd-1234-efgh", "registrationTokenExpiryDate": "2999-01-01T00:00:00-05:00" } }),
                other => panic!("unexpected {other}"),
            };
            Box::pin(async move { Ok(RestResponse { status: 200, body: body.to_string().into_bytes(), retry_after: None }) })
        }
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn with_no_servers_the_new_project_key_opens_the_add_server_form() {
        let (_rt, mut app) = run_app();
        app.go(Page::Migrate);
        app.focus = Focus::Content;
        pump(&mut app);
        press(&mut app, KeyCode::Char('n'));
        pump(&mut app);
        assert!(app.migrate.form.is_none());
        assert_eq!(app.migrate.servers.form.as_ref().map(|f| f.back_to_project), Some(Some(false)));
        assert!(app.flash.as_ref().unwrap().0.text.contains("No servers yet"));
        let screen = render(&mut app);
        assert!(screen.contains("add an N-central server"), "{screen}");
    }

    #[test]
    fn the_form_validates_shows_only_the_fields_that_apply_and_creates_the_project() {
        let (_rt, mut app) = migrate_app();
        pump(&mut app);
        press(&mut app, KeyCode::Char('n'));
        pump(&mut app);
        let form = app.migrate.form.as_ref().expect("form opens once the servers are read");
        assert_eq!((form.fields().len(), form.source, form.dest), (5, 0, 1), "name, from, to, linked?, source org");
        assert!(render(&mut app).contains("new migration project") && render(&mut app).contains("Old server (old.example.com)"));

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.migrate.form.as_ref().unwrap().error.as_deref(), Some("Give the project a name."));
        type_text(&mut app, "Lab move");
        assert!(app.migrate.form.as_ref().unwrap().error.is_none(), "typing clears the message");

        // Down to "Is the destination linked to Ecoverse?" and choose another tenant: its ID is then required.
        for _ in 0..3 {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.migrate.form.as_ref().unwrap().fields().len(), 7, "tenant and destination organization appear");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.migrate.form.as_ref().unwrap().error.as_deref(), Some("Enter the other tenant's ID."));
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.migrate.form.as_ref().unwrap().fields().len(), 5);

        // Source organization, then create.
        press(&mut app, KeyCode::Down);
        type_text(&mut app, "org-1, org-2");
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        assert!(app.migrate.form.is_none());
        assert!(app.flash.as_ref().unwrap().0.text.contains("Created Lab move"));
        let Load::Ready(list) = &app.migrate.projects else { panic!("list reloaded") };
        assert!(list.iter().any(|p| p.name == "Lab move"));
        let core = app.core().clone();
        let id = list.iter().find(|p| p.name == "Lab move").unwrap().id.clone();
        let saved = _rt.block_on(core.migration_load(&id)).unwrap().unwrap();
        assert_eq!(saved.source_ecoverse_org_ids, ["org-1", "org-2"]);
        assert_eq!(saved.dest_ecoverse, DestEcoverse::NotLinked);
    }

    #[test]
    fn typing_in_the_form_never_triggers_a_shortcut() {
        let (_rt, mut app) = migrate_app();
        pump(&mut app);
        press(&mut app, KeyCode::Char('n'));
        pump(&mut app);
        type_text(&mut app, "q1?2 d6");
        assert!(app.page == Page::Migrate && !app.show_help && !app.quit, "digits, q and ? are text here");
        assert_eq!(app.migrate.form.as_ref().unwrap().name.value, "q1?2 d6");
    }

    #[test]
    fn escape_cancels_the_form_and_only_a_select_field_changes_with_the_arrows() {
        let (_rt, mut app) = migrate_app();
        pump(&mut app);
        press(&mut app, KeyCode::Char('n'));
        pump(&mut app);
        type_text(&mut app, "ab");
        press(&mut app, KeyCode::Left);
        assert_eq!(app.migrate.form.as_ref().unwrap().name.value, "ab", "arrows don't move a text cursor away or change anything");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.migrate.form.as_ref().unwrap().source, 1);
        press(&mut app, KeyCode::Esc);
        assert!(app.migrate.form.is_none());
        assert!(app.page == Page::Migrate && app.migrate.open.is_none());
    }

    #[test]
    fn planning_adds_the_ready_devices_and_explains_the_rest_in_an_overlay() {
        let (_rt, mut app) = migrate_app_with(|p| {
            p.rest_scripts.insert(p.source_server_id.clone(), nvoke_core::migration::project::RestScript { item_id: "77".into(), version: "v".into() });
        });
        app.core().set_rest_transport(std::sync::Arc::new(Lab));
        open_and_select(&mut app, "ALPHA");
        press(&mut app, KeyCode::Char('P'));
        assert_eq!(app.migrate.busy, Some("Planning…"));
        pump(&mut app);
        let notes = app.migrate.notes.as_ref().expect("the result is shown");
        let text = notes.lines.join("\n");
        assert!(text.contains("Added 2 ready devices"), "{text}");
        assert!(text.contains("Press w to put the waiting devices in a wave."), "{text}");
        assert_eq!(app.migrate.open.as_ref().unwrap().project.devices.len(), 6);
        assert!(render(&mut app).contains("planned"));
        press(&mut app, KeyCode::Char('x'));
        assert!(app.migrate.notes.is_none(), "any other key closes it");
    }

    #[test]
    fn tokens_are_created_after_asking_and_the_result_is_shown_with_what_to_do_next() {
        let (_rt, mut app) = migrate_app();
        app.core().set_rest_transport(std::sync::Arc::new(Lab));
        open_and_select(&mut app, "ALPHA");
        press(&mut app, KeyCode::Char('T'));
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Create registration tokens on the destination for the customers and sites that have none?");
        press(&mut app, KeyCode::Char('n'));
        assert!(app.migrate.busy.is_none() && app.migrate.notes.is_none(), "anything but y cancels");
        press(&mut app, KeyCode::Char('T'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.migrate.busy, Some("Creating tokens…"));
        pump(&mut app);
        let text = app.migrate.notes.as_ref().expect("the result is shown").lines.join("\n");
        assert!(text.contains("1 already had a valid token") && text.contains("Press P to plan again"), "{text}");
    }

    #[test]
    fn a_wave_is_made_after_asking_and_only_when_devices_are_waiting() {
        let (_rt, mut app) = migrate_app_with(|p| {
            p.options.wave_size = 1;
            p.devices[2].wave = Some(2);
            p.devices[3].wave = Some(2);
        });
        open_and_select(&mut app, "ALPHA");
        press(&mut app, KeyCode::Char('w'));
        assert!(app.confirm.is_none() && app.flash.as_ref().unwrap().0.text.contains("No device is waiting for a wave"));

        let (_rt2, mut app2) = migrate_app_with(|p| {
            p.options.wave_size = 1;
            p.devices[2].wave = None;
            p.devices[3].wave = None;
        });
        open_and_select(&mut app2, "ALPHA");
        press(&mut app2, KeyCode::Char('w'));
        assert_eq!(app2.confirm.as_ref().unwrap().prompt(), "Make a wave of the next 1 waiting device (at most 1)?");
        press(&mut app2, KeyCode::Char('y'));
        pump(&mut app2);
        assert!(app2.flash.as_ref().unwrap().0.text.starts_with("Made wave 3 with 1 device"), "{}", app2.flash.as_ref().unwrap().0.text);
        assert_eq!(app2.migrate.open.as_ref().unwrap().project.waves.len(), 3);
    }

    #[test]
    fn publishing_is_offered_only_for_a_project_that_uses_ecoverse_and_asks_first() {
        let (_rt, mut app) = migrate_app();
        open_and_select(&mut app, "ALPHA");
        press(&mut app, KeyCode::Char('U'));
        assert!(app.confirm.is_none() && app.flash.as_ref().unwrap().0.text.contains("doesn't use Ecoverse"));

        let (_rt2, mut app2) = migrate_app_with(|p| p.source_ecoverse_org_ids = vec!["org-1".into()]);
        app2.core().set_graphql_fake(|query, _| {
            if query.contains("FindScripts") {
                return (200, json!({ "data": { "byName": { "nodes": [] }, "byText": { "nodes": [] } } }).to_string());
            }
            (200, json!({ "data": { "scriptCreate": { "item": { "id": "script-9" }, "errors": [] } } }).to_string())
        });
        open_and_select(&mut app2, "ALPHA");
        press(&mut app2, KeyCode::Char('U'));
        assert!(app2.confirm.as_ref().unwrap().prompt().starts_with("Publish N-voke migration 0.1.0 ("));
        press(&mut app2, KeyCode::Char('y'));
        pump(&mut app2);
        assert!(app2.flash.as_ref().unwrap().0.text.starts_with("Published N-voke migration"), "{}", app2.flash.as_ref().unwrap().0.text);
        assert_eq!(app2.migrate.open.as_ref().unwrap().project.ecoverse_script_id.as_deref(), Some("script-9"));
    }

    fn servers_of(app: &App, rt: &tokio::runtime::Runtime) -> Vec<String> {
        rt.block_on(app.core().ncentral_servers()).unwrap().into_iter().map(|s| s.host).collect()
    }

    fn open_servers_overlay(app: &mut App) {
        pump(app);
        press(app, KeyCode::Char('S'));
        pump(app);
        assert!(app.migrate.servers.open);
    }

    #[test]
    fn the_servers_overlay_lists_tests_and_never_shows_a_jwt() {
        let (rt, mut app) = migrate_app();
        app.core().set_rest_transport(std::sync::Arc::new(Lab));
        open_servers_overlay(&mut app);
        let screen = render(&mut app);
        assert!(screen.contains("N-central servers") && screen.contains("old.example.com") && screen.contains("new.example.com"), "{screen}");
        assert!(!screen.contains("j1") && !screen.contains("j2"), "no JWT on screen: {screen}");

        press(&mut app, KeyCode::Char('t'));
        pump(&mut app);
        let text = app.flash.as_ref().unwrap().0.text.clone();
        assert!(text.contains("Signed in: N-central 2026.4.1.1"), "{text}");
        assert_eq!(servers_of(&app, &rt).len(), 2);
    }

    #[test]
    fn a_server_is_added_from_the_overlay_with_typing_that_never_triggers_shortcuts() {
        let (rt, mut app) = migrate_app();
        app.core().set_rest_transport(std::sync::Arc::new(Lab));
        open_servers_overlay(&mut app);
        press(&mut app, KeyCode::Char('a'));
        type_text(&mut app, "older4.example.com");
        assert_eq!(app.page, Page::Migrate, "digits and letters in the field don't switch pages");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "Older q?");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "secret-jwt");
        let screen = render(&mut app);
        assert!(!screen.contains("secret-jwt"), "the JWT is masked: {screen}");
        press(&mut app, KeyCode::Enter);
        pump(&mut app);

        assert!(servers_of(&app, &rt).contains(&"older4.example.com".to_string()));
        assert!(app.migrate.servers.form.is_none());
        let text = app.flash.as_ref().unwrap().0.text.clone();
        assert!(text.starts_with("Saved Older q?") && text.contains("Signed in"), "{text}");
        assert!(matches!(&app.migrate.servers.list, Load::Ready(list) if list.len() == 3), "the list is refreshed");
    }

    #[test]
    fn the_form_asks_for_what_is_missing_and_escape_leaves_nothing_saved() {
        let (rt, mut app) = migrate_app();
        open_servers_overlay(&mut app);
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Enter);
        assert!(app.migrate.servers.form.as_ref().unwrap().error.as_deref().is_some_and(|e| e.contains("host name")));
        type_text(&mut app, "x.example.com");
        press(&mut app, KeyCode::Enter);
        assert!(app.migrate.servers.form.as_ref().unwrap().error.as_deref().is_some_and(|e| e.contains("JWT")), "a new server needs its JWT");
        press(&mut app, KeyCode::Esc);
        assert!(app.migrate.servers.form.is_none() && app.migrate.servers.open, "escape closes the form, not the overlay");
        assert_eq!(servers_of(&app, &rt).len(), 2);
    }

    #[test]
    fn a_server_a_project_uses_cant_be_removed_and_an_unused_one_can() {
        let (rt, mut app) = migrate_app();
        rt.block_on(app.core().save_ncentral_server(None, "Spare", "spare.example.com", Some("j3".into()), None)).unwrap();
        open_servers_overlay(&mut app);
        press(&mut app, KeyCode::Char('x'));
        assert!(app.confirm.is_none());
        let text = app.flash.as_ref().unwrap().0.text.clone();
        assert!(text.contains("is used by the project Acme move"), "{text}");

        let Load::Ready(list) = &app.migrate.servers.list else { panic!("loaded") };
        let spare = list.iter().position(|s| s.host == "spare.example.com").unwrap();
        app.migrate.servers.table.select(Some(spare));
        press(&mut app, KeyCode::Char('x'));
        assert!(app.confirm.as_ref().unwrap().prompt().starts_with("Remove the server Spare"));
        press(&mut app, KeyCode::Char('y'));
        pump(&mut app);
        assert!(!servers_of(&app, &rt).contains(&"spare.example.com".to_string()));
        assert_eq!(servers_of(&app, &rt).len(), 2);
    }

    #[test]
    fn a_server_added_from_the_new_project_form_comes_back_chosen_as_the_source_or_the_destination() {
        for (field_steps, as_dest) in [(1, false), (2, true)] {
            let (rt, mut app) = migrate_app();
            app.core().set_rest_transport(std::sync::Arc::new(Lab));
            pump(&mut app);
            press(&mut app, KeyCode::Char('n'));
            pump(&mut app);
            for _ in 0..field_steps {
                press(&mut app, KeyCode::Tab);
            }
            press(&mut app, KeyCode::Char('a'));
            assert_eq!(app.migrate.servers.form.as_ref().unwrap().back_to_project, Some(as_dest));
            type_text(&mut app, "third.example.com");
            press(&mut app, KeyCode::Tab);
            press(&mut app, KeyCode::Tab);
            type_text(&mut app, "jwt-3");
            press(&mut app, KeyCode::Enter);
            pump(&mut app);

            let form = app.migrate.form.as_ref().expect("back on the new-project form");
            let chosen = &form.servers[if as_dest { form.dest } else { form.source }];
            assert_eq!(chosen.host, "third.example.com", "dest={as_dest}");
            assert_eq!(servers_of(&app, &rt).len(), 3);
        }
    }

    #[test]
    fn escaping_the_server_form_opened_from_the_project_form_returns_to_it() {
        let (_rt, mut app) = migrate_app();
        pump(&mut app);
        press(&mut app, KeyCode::Char('n'));
        pump(&mut app);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('a'));
        assert!(app.migrate.servers.form.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.migrate.servers.form.is_none() && app.migrate.form.is_some());
    }
}
