//! The N-central servers migration uses, managed from the terminal app: list them, add one (with its API
//! JWT, which goes straight to the keychain and is never shown again), edit one, remove one that no project
//! uses, and test a connection. They can be reached from the Migrate page's project list (`S`) and from the
//! new-project form (`a` on a server choice), so a project can be set up without leaving the app.
//! The operations are the core's (`save_ncentral_server` and friends); this file is state and keys.

use nvoke_core::ncentral::ServerView;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    widgets::TableState,
};

use super::{
    App, Confirm, Load, Msg,
    migrate::{MigrateConfirm, MigrateMsg, NewProject},
    run::NoticeKind,
};
use crate::input::TextInput;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerFormMode {
    Add,
    /// Changing the server with this ID.
    Edit(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SfField {
    Label,
    Host,
    Jwt,
    Tenant,
}

pub struct ServerForm {
    pub mode: ServerFormMode,
    pub label: TextInput,
    pub host: TextInput,
    /// Masked. Empty when editing keeps the saved one.
    pub jwt: TextInput,
    pub tenant: TextInput,
    pub field: SfField,
    pub error: Option<String>,
    pub busy: bool,
    /// Opened from the new-project form: go back to it when saved, with the new server as the destination (true) or the source (false).
    pub back_to_project: Option<bool>,
}

pub struct ServerArgs {
    pub id: Option<String>,
    pub label: String,
    pub host: String,
    pub jwt: Option<String>,
    pub tenant: Option<String>,
}

impl ServerForm {
    pub fn add(back_to_project: Option<bool>) -> Self {
        Self { mode: ServerFormMode::Add, label: TextInput::default(), host: TextInput::default(), jwt: TextInput::new("", true), tenant: TextInput::default(), field: SfField::Host, error: None, busy: false, back_to_project }
    }

    pub fn edit(server: &ServerView) -> Self {
        Self {
            mode: ServerFormMode::Edit(server.id.clone()),
            label: TextInput::new(&server.label, false),
            host: TextInput::new(&server.host, false),
            jwt: TextInput::new("", true),
            tenant: TextInput::new(server.ecoverse_tenant.as_deref().unwrap_or(""), false),
            field: SfField::Label,
            error: None,
            busy: false,
            back_to_project: None,
        }
    }

    pub const FIELDS: [SfField; 4] = [SfField::Host, SfField::Label, SfField::Jwt, SfField::Tenant];

    pub fn step(&mut self, delta: isize) {
        let at = Self::FIELDS.iter().position(|f| *f == self.field).unwrap_or(0) as isize;
        self.field = Self::FIELDS[(at + delta).rem_euclid(Self::FIELDS.len() as isize) as usize];
    }

    pub fn input(&mut self) -> &mut TextInput {
        match self.field {
            SfField::Label => &mut self.label,
            SfField::Host => &mut self.host,
            SfField::Jwt => &mut self.jwt,
            SfField::Tenant => &mut self.tenant,
        }
    }

    /// What to save. A new server needs its JWT; an edit without one keeps the saved one.
    pub fn validate(&self) -> Result<ServerArgs, String> {
        if self.host.value.trim().is_empty() {
            return Err("Enter the server's host name, for example n-central.example.com.".into());
        }
        let jwt = Some(self.jwt.value.trim().to_string()).filter(|j| !j.is_empty());
        if self.mode == ServerFormMode::Add && jwt.is_none() {
            return Err("Paste the API JWT of a user on that server (N-central: Administration, User Management, API Access). It is kept in your keychain.".into());
        }
        Ok(ServerArgs {
            id: match &self.mode {
                ServerFormMode::Add => None,
                ServerFormMode::Edit(id) => Some(id.clone()),
            },
            label: self.label.value.trim().to_string(),
            host: self.host.value.trim().to_string(),
            jwt,
            tenant: Some(self.tenant.value.trim().to_string()).filter(|t| !t.is_empty()),
        })
    }
}

/// The servers to show on the new-project form, and the server just added with whether it is the destination.
pub type FormServers = Result<(Vec<ServerView>, Option<(String, bool)>), String>;

/// A server that was saved, and whether signing in to it worked.
pub struct SavedServer {
    pub server: ServerView,
    pub test: Result<String, String>,
}

pub struct ServersState {
    pub open: bool,
    pub list: Load<Vec<ServerView>>,
    pub table: TableState,
    pub form: Option<ServerForm>,
}

impl Default for ServersState {
    fn default() -> Self {
        Self { open: false, list: Load::NotLoaded, table: TableState::default(), form: None }
    }
}

/// "Signed in: N-central 2026.4.1.11 (REST API 1.3.23)", with a warning for a version older than what was tested.
pub fn describe_info(info: &nvoke_core::ncentral::model::ServerInfo) -> String {
    let old = info.release().is_none_or(|r| r < (2026, 4));
    format!("Signed in: N-central {} (REST API {}){}", info.ncentral, info.version, if old { ". Migration was tested on 2026.4 and later." } else { "" })
}

impl App {
    /// Whether the Migrate page is showing something that takes typed text or covers the page.
    pub(crate) fn migrate_modal_open(&self) -> bool {
        self.migrate.form.is_some() || self.migrate.servers.open || self.migrate.servers.form.is_some()
    }

    pub(crate) fn on_migrate_modal_key(&mut self, key: KeyEvent) {
        if self.migrate.servers.open || self.migrate.servers.form.is_some() {
            self.on_servers_key(key);
        } else {
            self.on_new_project_key(key);
        }
    }

    pub(crate) fn open_servers(&mut self) {
        self.migrate.servers.open = true;
        self.reload_servers();
    }

    fn reload_servers(&mut self) {
        self.migrate.servers.list = Load::Loading;
        self.spawn(|core| async move { Msg::Migrate(Box::new(MigrateMsg::ServerList(core.ncentral_servers().await))) });
    }

    /// Opens the add-server form. From the new-project form, saving comes back to it.
    pub(crate) fn start_add_server(&mut self, back_to_project: Option<bool>) {
        self.migrate.servers.form = Some(ServerForm::add(back_to_project));
        if back_to_project.is_none() {
            self.migrate.servers.open = true;
        }
    }

    fn selected_server(&self) -> Option<ServerView> {
        match &self.migrate.servers.list {
            Load::Ready(list) => self.migrate.servers.table.selected().and_then(|i| list.get(i).cloned()),
            _ => None,
        }
    }

    pub(crate) fn on_servers_key(&mut self, key: KeyEvent) {
        if self.migrate.servers.form.is_some() {
            return self.on_server_form_key(key);
        }
        let len = match &self.migrate.servers.list {
            Load::Ready(list) => list.len(),
            _ => 0,
        };
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h' | 'q') => self.migrate.servers.open = false,
            KeyCode::Down | KeyCode::Char('j') => self.migrate.servers.table.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.migrate.servers.table.select_previous(),
            KeyCode::Home | KeyCode::Char('g') => self.migrate.servers.table.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.migrate.servers.table.select(len.checked_sub(1)),
            KeyCode::Char('a') => self.start_add_server(None),
            KeyCode::Char('e') | KeyCode::Enter => {
                if let Some(server) = self.selected_server() {
                    self.migrate.servers.form = Some(ServerForm::edit(&server));
                }
            }
            KeyCode::Char('t') => {
                if let Some(server) = self.selected_server() {
                    self.migrate.busy = Some("Signing in…");
                    self.spawn(move |core| async move {
                        let result = core.ncentral_server_info(&server.id).await.map(|info| format!("{}: {}", server.label, describe_info(&info))).map_err(|e| format!("{}: {e}", server.label));
                        Msg::Migrate(Box::new(MigrateMsg::ServerTested(result)))
                    });
                }
            }
            KeyCode::Char('x') => self.ask_remove_server(),
            _ => {}
        }
    }

    fn ask_remove_server(&mut self) {
        let Some(server) = self.selected_server() else { return };
        // A server a project uses can't go: the project would lose its source or destination.
        if let Load::Ready(projects) = &self.migrate.projects
            && let Some(project) = projects.iter().find(|p| p.source_server_id == server.id || p.dest_server_id == server.id)
        {
            return self.flash(NoticeKind::Error, format!("{} is used by the project {}: delete that project first.", server.label, project.name));
        }
        self.confirm = Some(Confirm::Migrate(MigrateConfirm::RemoveServer { id: server.id, label: server.label }));
    }

    pub(crate) fn run_remove_server(&mut self, id: String) {
        self.migrate.busy = Some("Removing…");
        self.spawn(move |core| async move { Msg::Migrate(Box::new(MigrateMsg::ServerRemoved(core.remove_ncentral_server(&id).await))) });
    }

    fn on_server_form_key(&mut self, key: KeyEvent) {
        let Some(form) = self.migrate.servers.form.as_mut() else { return };
        if form.busy {
            return;
        }
        match key.code {
            KeyCode::Esc => self.migrate.servers.form = None,
            KeyCode::Tab | KeyCode::Down => form.step(1),
            KeyCode::BackTab | KeyCode::Up => form.step(-1),
            KeyCode::Enter => match form.validate() {
                Err(e) => form.error = Some(e),
                Ok(args) => {
                    form.error = None;
                    form.busy = true;
                    self.spawn(move |core| async move {
                        let saved = core.save_ncentral_server(args.id, &args.label, &args.host, args.jwt, args.tenant).await;
                        let result = match saved {
                            Err(e) => Err(e),
                            Ok(server) => {
                                let test = core.ncentral_server_info(&server.id).await.map(|info| describe_info(&info)).map_err(|e| e.message);
                                Ok(SavedServer { server, test })
                            }
                        };
                        Msg::Migrate(Box::new(MigrateMsg::ServerSaved(result)))
                    });
                }
            },
            _ => {
                let input = form.input();
                if input.handle_key(key) {
                    form.error = None;
                }
            }
        }
    }

    // ---- results -----------------------------------------------------------

    pub(crate) fn on_server_list(&mut self, result: Result<Vec<ServerView>, String>) {
        match result {
            Ok(list) => {
                let keep = self.migrate.servers.table.selected().unwrap_or(0).min(list.len().saturating_sub(1));
                self.migrate.servers.table.select((!list.is_empty()).then_some(keep));
                self.migrate.servers.list = Load::Ready(list);
            }
            Err(e) => self.migrate.servers.list = Load::Failed(e),
        }
    }

    pub(crate) fn on_server_saved(&mut self, result: Result<SavedServer, String>) {
        match result {
            Err(e) => {
                if let Some(form) = &mut self.migrate.servers.form {
                    form.busy = false;
                    form.error = Some(e);
                }
            }
            Ok(saved) => {
                let back = self.migrate.servers.form.take().and_then(|f| f.back_to_project);
                let (text, kind) = match &saved.test {
                    Ok(message) => (format!("Saved {}. {message}.", saved.server.label), NoticeKind::Info),
                    Err(e) => (format!("Saved {}, but it couldn't be signed in to: {e}", saved.server.label), NoticeKind::Error),
                };
                self.flash(kind, text);
                match back {
                    Some(as_dest) => {
                        self.migrate.servers.open = false;
                        self.refresh_new_project_servers(Some((saved.server.id.clone(), as_dest)));
                    }
                    None => self.reload_servers(),
                }
            }
        }
    }

    /// Re-reads the servers for the new-project form (opening it if the server was added from the project list's `n`).
    fn refresh_new_project_servers(&mut self, select: Option<(String, bool)>) {
        self.migrate.busy = Some("Reading servers…");
        self.spawn(move |core| async move {
            let result = core.ncentral_servers().await.map(|servers| (servers, select));
            Msg::Migrate(Box::new(MigrateMsg::FormServers(result)))
        });
    }

    pub(crate) fn on_form_servers(&mut self, result: FormServers) {
        self.migrate.busy = None;
        match result {
            Err(e) => self.flash(NoticeKind::Error, e),
            Ok((servers, select)) => {
                let choose = select.as_ref().map(|(id, as_dest)| (id.as_str(), *as_dest));
                match self.migrate.form.as_mut() {
                    Some(form) => form.set_servers(servers, choose),
                    None => {
                        let mut form = NewProject::new(servers.clone());
                        form.set_servers(servers, choose);
                        self.migrate.form = Some(form);
                    }
                }
            }
        }
    }

    pub(crate) fn on_server_removed(&mut self, result: Result<(), String>) {
        self.migrate.busy = None;
        match result {
            Ok(()) => {
                self.flash(NoticeKind::Info, "Removed the server and its saved JWT.");
                self.reload_servers();
            }
            Err(e) => self.flash(NoticeKind::Error, e),
        }
    }

    pub(crate) fn on_server_tested(&mut self, result: Result<String, String>) {
        self.migrate.busy = None;
        match result {
            Ok(text) => self.flash(NoticeKind::Info, text),
            Err(e) => self.flash(NoticeKind::Error, e),
        }
    }
}
