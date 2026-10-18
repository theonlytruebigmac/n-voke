//! Application state and the update function.
//!
//! Everything that can happen arrives as an `Event` on one channel: key
//! presses and ticks from an input thread, and results from async work (the
//! core's API calls) started by `App`. `run` draws, waits for the next event,
//! updates the state, and repeats. Drawing (ui.rs) only reads the state.

use std::{io, sync::{Arc, mpsc}, thread, time::Duration};

use nvoke_core::{
    AuditLogView, Core, DryRunEntry, Settings,
    collections::Collections,
    run::Resolved,
    job_store::JobStore,
    jobs::Job,
    task_state::{PhaseCounts, ResultGroup, count_phases, group_results},
};
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    widgets::TableState,
};
use tokio::runtime::Handle;

use crate::{input::TextInput, picker::PickItem, ui};

pub(crate) mod run;
pub use run::{NoticeKind, RunField, RunPane, RunState};

/// Also the animation frame rate: spinners and the logo shimmer advance once per tick.
const TICK: Duration = Duration::from_millis(100);

pub enum Event {
    Key(KeyEvent),
    /// Sent every `TICK` when nothing else happened; drives animation and, later, polling.
    Tick,
    Msg(Box<Msg>),
}

/// Results of async work.
pub enum Msg {
    Settings(Result<Settings, String>),
    Saved(Result<Settings, String>),
    Audit(Result<AuditLogView, String>),
    /// Results of a picker search; `seq` says which search they answer.
    PickResults { seq: u64, result: Result<Vec<PickItem>, String> },
    /// The devices the Run page's targets resolve to.
    Resolved { seq: u64, result: Result<(Vec<Resolved>, bool), String> },
    RunFinished(Result<Box<nvoke_core::jobs::Job>, String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Run,
    Activity,
    Tags,
    Audit,
    Settings,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::Run, Page::Activity, Page::Tags, Page::Audit, Page::Settings];

    pub fn title(self) -> &'static str {
        match self {
            Page::Run => "Run",
            Page::Activity => "Activity",
            Page::Tags => "Tags",
            Page::Audit => "Audit log",
            Page::Settings => "Settings",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Page::Run => "▶",
            Page::Activity => "◉",
            Page::Tags => "◆",
            Page::Audit => "≡",
            Page::Settings => "✦",
        }
    }

    /// What the page will do, shown until it is built.
    pub fn blurb(self) -> &'static str {
        match self {
            Page::Run => "Pick an organization and a script, then devices and tags, and run it.",
            Page::Activity => "Follow every run to the end, with results grouped by outcome.",
            Page::Tags => "Browse, create and edit static and dynamic tags.",
            Page::Audit => "Everything this app has changed in N-able.",
            Page::Settings => "API endpoint, key and history retention.",
        }
    }

    /// Whether the page has anything to move around in (so focus can enter it).
    pub fn interactive(self) -> bool {
        matches!(self, Page::Run | Page::Activity | Page::Audit | Page::Settings)
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap()
    }
}

/// Which panel the arrow keys act on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Content,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Field {
    Endpoint,
    Token,
}

/// The first-launch screen and the settings editor are the same form.
pub struct SettingsForm {
    pub endpoint: TextInput,
    pub token: TextInput,
    pub focus: Field,
    pub error: Option<String>,
    pub busy: bool,
    /// No API key yet: the token is required and Esc quits instead of going back.
    pub first_run: bool,
}

impl SettingsForm {
    pub fn new(settings: &Settings) -> Self {
        Self {
            endpoint: TextInput::new(&settings.api_url, false),
            token: TextInput::new("", true),
            focus: if settings.has_api_key { Field::Endpoint } else { Field::Token },
            error: None,
            busy: false,
            first_run: !settings.has_api_key,
        }
    }

    pub fn focused(&mut self) -> &mut TextInput {
        match self.focus {
            Field::Endpoint => &mut self.endpoint,
            Field::Token => &mut self.token,
        }
    }
}

/// Which half of the Activity page the arrow keys act on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Jobs,
    Results,
}

/// A question waiting for y or n.
pub enum Confirm {
    RemoveJob(String),
}

pub enum Screen {
    Loading,
    /// Settings couldn't be read at all (usually: no keychain).
    Fatal(String),
    Form(SettingsForm),
    Main,
}

/// Data for a page that is fetched on demand.
pub enum Load<T> {
    NotLoaded,
    Loading,
    Ready(T),
    Failed(String),
}

pub struct App {
    core: Arc<Core>,
    collections: Arc<Collections>,
    pub run: RunState,
    runtime: Handle,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    pub screen: Screen,
    pub page: Page,
    pub settings: Option<Settings>,
    pub audit: Load<AuditLogView>,
    pub audit_table: TableState,
    pub jobs: Arc<JobStore>,
    jobs_started: bool,
    jobs_version: u64,
    /// Newest first, as the store keeps them; refreshed when the store changes.
    pub job_list: Vec<Job>,
    pub job_table: TableState,
    pub selected_job: Option<String>,
    /// Results of the selected job, grouped by outcome.
    pub groups: Vec<ResultGroup>,
    pub counts: PhaseCounts,
    pub group_table: TableState,
    pub pane: Pane,
    pub output_scroll: u16,
    pub confirm: Option<Confirm>,
    /// The list of changes dry-run mode held back.
    pub show_dry_log: bool,
    pub dry_table: TableState,
    pub dry_scroll: u16,
    pub focus: Focus,
    pub show_help: bool,
    /// Counts ticks since start; animations are a function of it.
    pub tick: u64,
    quit: bool,
}

impl App {
    pub fn new(core: Arc<Core>, runtime: Handle) -> Self {
        let (tx, rx) = mpsc::channel();
        let jobs = JobStore::new(core.clone(), 30);
        let collections = Collections::new(core.clone());
        Self {
            core,
            collections,
            run: RunState::default(),
            runtime,
            tx,
            rx,
            screen: Screen::Loading,
            page: Page::Run,
            settings: None,
            audit: Load::NotLoaded,
            audit_table: TableState::default(),
            jobs,
            jobs_started: false,
            jobs_version: u64::MAX,
            job_list: vec![],
            job_table: TableState::default(),
            selected_job: None,
            groups: vec![],
            counts: PhaseCounts::default(),
            group_table: TableState::default(),
            pane: Pane::Jobs,
            output_scroll: 0,
            confirm: None,
            show_dry_log: false,
            dry_table: TableState::default(),
            dry_scroll: 0,
            focus: Focus::Sidebar,
            show_help: false,
            tick: 0,
            quit: false,
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        self.spawn_input_thread();
        self.spawn(|core| async move { Msg::Settings(core.get_settings().await) });

        while !self.quit {
            terminal.draw(|frame| ui::draw(frame, &mut self))?;
            match self.rx.recv() {
                Ok(event) => self.update(event),
                Err(_) => break,
            }
        }
        Ok(())
    }

    /// Reading terminal events blocks, so it gets its own thread. `poll` with a
    /// timeout doubles as the tick timer.
    fn spawn_input_thread(&self) {
        let tx = self.tx.clone();
        thread::spawn(move || {
            loop {
                let sent = match event::poll(TICK) {
                    Ok(true) => match event::read() {
                        // Windows also reports key releases; only presses count.
                        Ok(TermEvent::Key(key)) if key.kind == KeyEventKind::Press => tx.send(Event::Key(key)),
                        Ok(_) => Ok(()),
                        Err(_) => break,
                    },
                    Ok(false) => tx.send(Event::Tick),
                    Err(_) => break,
                };
                if sent.is_err() {
                    break; // the app has quit
                }
            }
        });
    }

    /// Runs async work on the runtime; its result comes back as an `Event::Msg`.
    fn spawn<F, Fut>(&self, work: F)
    where
        F: FnOnce(Arc<Core>) -> Fut,
        Fut: std::future::Future<Output = Msg> + Send + 'static,
    {
        let future = work(self.core.clone());
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let _ = tx.send(Event::Msg(Box::new(future.await)));
        });
    }

    fn update(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Tick => {
                self.tick = self.tick.wrapping_add(1);
                self.sync_jobs();
                self.tick_picker();
            }
            Event::Msg(msg) => self.on_msg(*msg),
        }
    }

    // ---- results -----------------------------------------------------------

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Settings(Ok(settings)) => {
                self.screen = if settings.has_api_key { Screen::Main } else { Screen::Form(SettingsForm::new(&settings)) };
                self.jobs.set_history_days(settings.history_days);
                let has_key = settings.has_api_key;
                self.settings = Some(settings);
                if has_key {
                    self.start_jobs();
                }
                self.enter_page();
            }
            Msg::Settings(Err(e)) => self.screen = Screen::Fatal(e),
            Msg::Saved(Ok(settings)) => {
                self.jobs.set_history_days(settings.history_days);
                self.settings = Some(settings);
                self.start_jobs();
                self.screen = Screen::Main;
                self.audit = Load::NotLoaded; // saving a setting adds an audit entry
                self.enter_page();
            }
            Msg::Saved(Err(e)) => {
                if let Screen::Form(form) = &mut self.screen {
                    form.busy = false;
                    form.error = Some(e);
                }
            }
            Msg::Audit(Ok(log)) => {
                self.audit_table.select((!log.entries.is_empty()).then_some(0));
                self.audit = Load::Ready(log);
            }
            Msg::Audit(Err(e)) => self.audit = Load::Failed(e),
            Msg::PickResults { seq, result } => self.on_pick_results(seq, result),
            Msg::Resolved { seq, result } => self.on_resolved(seq, result),
            Msg::RunFinished(result) => self.on_run_finished(result),
        }
    }

    /// Starts whatever the current page needs that it doesn't have yet.
    fn enter_page(&mut self) {
        if self.page == Page::Audit && matches!(self.audit, Load::NotLoaded) {
            self.reload_audit();
        }
    }

    fn reload_audit(&mut self) {
        self.audit = Load::Loading;
        self.spawn(|core| async move { Msg::Audit(core.audit_log(None)) });
    }

    // ---- keys --------------------------------------------------------------

    fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        match &self.screen {
            Screen::Loading => {}
            Screen::Fatal(_) => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    self.quit = true;
                }
            }
            Screen::Form(_) => self.on_form_key(key),
            Screen::Main => self.on_main_key(key),
        }
    }

    fn on_form_key(&mut self, key: KeyEvent) {
        let Screen::Form(form) = &mut self.screen else { return };
        if form.busy {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                if form.first_run {
                    self.quit = true;
                } else {
                    self.screen = Screen::Main;
                }
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                form.focus = if form.focus == Field::Endpoint { Field::Token } else { Field::Endpoint };
            }
            KeyCode::Enter => {
                if form.first_run && form.token.value.trim().is_empty() {
                    form.error = Some("Paste your N-able API-Access token to continue.".into());
                    return;
                }
                form.busy = true;
                form.error = None;
                let endpoint = form.endpoint.value.clone();
                let token = Some(form.token.value.clone()).filter(|t| !t.trim().is_empty());
                self.spawn(|core| async move { Msg::Saved(core.save_settings(endpoint, token, None).await) });
            }
            _ => {
                form.focused().handle_key(key);
            }
        }
    }

    /// True while anything is waiting on the network or keychain.
    pub fn busy(&self) -> bool {
        matches!(self.screen, Screen::Loading) || matches!(self.audit, Load::Loading) || (self.jobs_started && !self.jobs.is_loaded())
    }

    fn on_main_key(&mut self, key: KeyEvent) {
        if let Some(Confirm::RemoveJob(id)) = self.confirm.take() {
            if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                self.jobs.remove(&id);
                self.sync_jobs();
            }
            return; // any other key cancels
        }
        if self.show_dry_log {
            return self.on_dry_log_key(key);
        }
        if self.run_captures_keys() {
            return self.on_run_modal_key(key);
        }
        if self.show_help {
            // Any key closes the help overlay.
            self.show_help = false;
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.show_help = true,
            KeyCode::Char('d') if self.core.is_dry_run() => {
                self.show_dry_log = true;
                self.dry_table.select((!self.core.dry_run_log().is_empty()).then_some(0));
                self.dry_scroll = 0;
            }
            KeyCode::Char(c @ '1'..='5') => self.go(Page::ALL[c as usize - '1' as usize]),
            KeyCode::Tab => self.go(Page::ALL[(self.page.index() + 1) % Page::ALL.len()]),
            KeyCode::BackTab => self.go(Page::ALL[(self.page.index() + Page::ALL.len() - 1) % Page::ALL.len()]),
            _ => match self.focus {
                Focus::Sidebar => self.on_sidebar_key(key),
                Focus::Content => self.on_content_key(key),
            },
        }
    }

    /// Up/down choose a page (it opens as you move); right or enter steps into it.
    fn on_sidebar_key(&mut self, key: KeyEvent) {
        let i = self.page.index();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.go(Page::ALL[i.saturating_sub(1)]),
            KeyCode::Down | KeyCode::Char('j') => self.go(Page::ALL[(i + 1).min(Page::ALL.len() - 1)]),
            KeyCode::Home => self.go(Page::ALL[0]),
            KeyCode::End => self.go(Page::ALL[Page::ALL.len() - 1]),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter if self.page.interactive() => self.focus = Focus::Content,
            _ => {}
        }
    }

    /// Left or esc steps back out to the sidebar; everything else is the page's.
    fn on_content_key(&mut self, key: KeyEvent) {
        if self.page == Page::Activity {
            return self.on_activity_key(key);
        }
        if self.page == Page::Run {
            return self.on_run_key(key);
        }
        if matches!(key.code, KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc) {
            self.focus = Focus::Sidebar;
            return;
        }
        match self.page {
            Page::Audit => self.on_audit_key(key),
            Page::Settings => {
                if let (true, Some(settings)) = (matches!(key.code, KeyCode::Enter | KeyCode::Char('e')), &self.settings) {
                    let mut form = SettingsForm::new(settings);
                    form.first_run = false;
                    self.screen = Screen::Form(form);
                }
            }
            _ => {}
        }
    }

    // ---- activity ----------------------------------------------------------

    /// Loads saved history and starts watching unfinished tasks. Safe to call more than once.
    fn start_jobs(&mut self) {
        if std::mem::replace(&mut self.jobs_started, true) {
            return;
        }
        let store = self.jobs.clone();
        self.runtime.spawn(async move {
            store.load().await;
            store.run().await;
        });
    }

    /// Picks up changes the store made (polling, saved history arriving) so the screen shows them.
    pub fn sync_jobs(&mut self) {
        let version = self.jobs.version();
        if version == self.jobs_version {
            return;
        }
        self.jobs_version = version;
        self.job_list = self.jobs.jobs();
        self.reselect_job();
    }

    /// Keeps the same job selected when new ones arrive above it, and rebuilds its result groups.
    fn reselect_job(&mut self) {
        let index = self.selected_job.as_ref().and_then(|id| self.job_list.iter().position(|j| &j.id == id)).or((!self.job_list.is_empty()).then_some(0));
        self.job_table.select(index);
        let previous = self.selected_job.take();
        self.selected_job = index.map(|i| self.job_list[i].id.clone());
        if previous != self.selected_job {
            self.output_scroll = 0;
            self.group_table.select(None);
        }
        self.refresh_groups();
    }

    fn refresh_groups(&mut self) {
        let job = self.job_table.selected().and_then(|i| self.job_list.get(i));
        (self.groups, self.counts) = match job {
            Some(j) => (group_results(&j.tasks), count_phases(&j.tasks)),
            None => (vec![], PhaseCounts::default()),
        };
        let selected = self.group_table.selected().filter(|&i| i < self.groups.len()).or((!self.groups.is_empty()).then_some(0));
        self.group_table.select(selected);
    }

    #[cfg(test)]
    pub fn core(&self) -> &Arc<Core> {
        &self.core
    }

    pub fn is_dry_run(&self) -> bool {
        self.core.is_dry_run()
    }

    /// The changes held back so far, newest first.
    pub fn dry_run_entries(&self) -> Vec<DryRunEntry> {
        let mut entries = self.core.dry_run_log();
        entries.reverse();
        entries
    }

    fn on_dry_log_key(&mut self, key: KeyEvent) {
        let count = self.core.dry_run_log().len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('d' | 'q') => self.show_dry_log = false,
            KeyCode::Down | KeyCode::Char('j') => {
                self.dry_table.select(Some((self.dry_table.selected().unwrap_or(0) + 1).min(count.saturating_sub(1))));
                self.dry_scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.dry_table.select(Some(self.dry_table.selected().unwrap_or(0).saturating_sub(1)));
                self.dry_scroll = 0;
            }
            KeyCode::PageDown => self.dry_scroll = self.dry_scroll.saturating_add(8),
            KeyCode::PageUp => self.dry_scroll = self.dry_scroll.saturating_sub(8),
            _ => {}
        }
    }

    pub fn jobs_loaded(&self) -> bool {
        self.jobs.is_loaded()
    }

    pub fn current_job(&self) -> Option<&Job> {
        self.job_table.selected().and_then(|i| self.job_list.get(i))
    }

    pub fn current_group(&self) -> Option<&ResultGroup> {
        self.group_table.selected().and_then(|i| self.groups.get(i))
    }

    /// Left/right move between the menu, the jobs and their results; up/down move within.
    fn on_activity_key(&mut self, key: KeyEvent) {
        let back = matches!(key.code, KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc);
        if back {
            match self.pane {
                Pane::Results => self.pane = Pane::Jobs,
                Pane::Jobs => self.focus = Focus::Sidebar,
            }
            return;
        }
        match (self.pane, key.code) {
            (Pane::Jobs, KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter) if !self.groups.is_empty() => self.pane = Pane::Results,
            (Pane::Jobs, KeyCode::Down | KeyCode::Char('j')) => self.move_job(1),
            (Pane::Jobs, KeyCode::Up | KeyCode::Char('k')) => self.move_job(-1),
            (Pane::Jobs, KeyCode::PageDown) => self.move_job(10),
            (Pane::Jobs, KeyCode::PageUp) => self.move_job(-10),
            (Pane::Jobs, KeyCode::Home | KeyCode::Char('g')) => self.select_job(0),
            (Pane::Jobs, KeyCode::End | KeyCode::Char('G')) => self.select_job(self.job_list.len().saturating_sub(1)),
            (Pane::Results, KeyCode::Down | KeyCode::Char('j')) => self.move_group(1),
            (Pane::Results, KeyCode::Up | KeyCode::Char('k')) => self.move_group(-1),
            (Pane::Results, KeyCode::PageDown) => self.output_scroll = self.output_scroll.saturating_add(8),
            (Pane::Results, KeyCode::PageUp) => self.output_scroll = self.output_scroll.saturating_sub(8),
            (Pane::Results, KeyCode::Home | KeyCode::Char('g')) => self.output_scroll = 0,
            (_, KeyCode::Char('r')) => {
                let store = self.jobs.clone();
                self.runtime.spawn(async move { store.poll_once().await });
            }
            (_, KeyCode::Char('s')) => {
                if let Some(id) = self.selected_job.clone() {
                    self.jobs.stop(&id);
                    self.sync_jobs();
                }
            }
            (_, KeyCode::Char('w')) => {
                if let Some(id) = self.selected_job.clone() {
                    let store = self.jobs.clone();
                    self.runtime.spawn(async move { store.resume(&id, None).await });
                }
            }
            (_, KeyCode::Char('R')) => self.run_again(false),
            (_, KeyCode::Char('F')) => self.run_again(true),
            (_, KeyCode::Char('x') | KeyCode::Delete) => {
                if let Some(id) = self.selected_job.clone() {
                    self.confirm = Some(Confirm::RemoveJob(id));
                }
            }
            _ => {}
        }
    }

    fn select_job(&mut self, index: usize) {
        if self.job_list.is_empty() {
            return;
        }
        self.selected_job = Some(self.job_list[index.min(self.job_list.len() - 1)].id.clone());
        self.job_table.select(self.job_list.iter().position(|j| Some(&j.id) == self.selected_job.as_ref()));
        self.output_scroll = 0;
        self.group_table.select(None);
        self.refresh_groups();
    }

    fn move_job(&mut self, by: isize) {
        let current = self.job_table.selected().unwrap_or(0) as isize;
        self.select_job((current + by).clamp(0, self.job_list.len().saturating_sub(1) as isize) as usize);
    }

    fn move_group(&mut self, by: isize) {
        let current = self.group_table.selected().unwrap_or(0) as isize;
        self.group_table.select(Some((current + by).clamp(0, self.groups.len().saturating_sub(1) as isize) as usize));
        self.output_scroll = 0;
    }

    fn go(&mut self, page: Page) {
        self.page = page;
        if !page.interactive() {
            self.focus = Focus::Sidebar;
        }
        self.enter_page();
    }

    fn on_audit_key(&mut self, key: KeyEvent) {
        let len = match &self.audit {
            Load::Ready(log) => log.entries.len(),
            _ => 0,
        };
        let table = &mut self.audit_table;
        match key.code {
            KeyCode::Char('r') => self.reload_audit(),
            KeyCode::Down | KeyCode::Char('j') => table.select_next(),
            KeyCode::Up | KeyCode::Char('k') => table.select_previous(),
            KeyCode::PageDown => table.scroll_down_by(10),
            KeyCode::PageUp => table.scroll_up_by(10),
            KeyCode::Home | KeyCode::Char('g') => table.select_first(),
            KeyCode::End | KeyCode::Char('G') => table.select(len.checked_sub(1)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyEventState;

    use super::*;

    fn app() -> (tokio::runtime::Runtime, App) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut app = App::new(Core::for_tests("tui"), runtime.handle().clone());
        app.screen = Screen::Main;
        // Pre-load the audit page so entering it doesn't start a real read.
        app.audit = Load::Ready(AuditLogView { path: String::new(), entries: vec![] });
        (runtime, app)
    }

    fn press(app: &mut App, code: KeyCode) {
        let key = KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE };
        app.update(Event::Key(key));
    }

    #[test]
    fn arrows_walk_the_menu_and_stop_at_the_ends() {
        let (_rt, mut app) = app();
        assert!(app.page == Page::Run);
        press(&mut app, KeyCode::Up);
        assert!(app.page == Page::Run, "can't go above the first page");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert!(app.page == Page::Tags);
        press(&mut app, KeyCode::End);
        assert!(app.page == Page::Settings);
        press(&mut app, KeyCode::Down);
        assert!(app.page == Page::Settings, "can't go below the last page");
    }

    #[test]
    fn right_enters_a_page_and_left_leaves_it() {
        let (_rt, mut app) = app();
        app.go(Page::Tags);
        press(&mut app, KeyCode::Right);
        assert!(app.focus == Focus::Sidebar, "Tags isn't built yet, so focus stays in the menu");
        app.go(Page::Run);

        app.go(Page::Audit);
        press(&mut app, KeyCode::Right);
        assert!(app.focus == Focus::Content);
        press(&mut app, KeyCode::Left);
        assert!(app.focus == Focus::Sidebar);

        press(&mut app, KeyCode::Enter);
        assert!(app.focus == Focus::Content);
        press(&mut app, KeyCode::Esc);
        assert!(app.focus == Focus::Sidebar);
    }

    #[test]
    fn arrows_scroll_the_audit_table_only_when_it_has_focus() {
        let (_rt, mut app) = app();
        let entries = (0..5).map(|i| nvoke_core::audit::AuditEntry::new("tag.create", &format!("e{i}"), Ok(()), serde_json::Value::Null)).collect();
        app.audit = Load::Ready(AuditLogView { path: String::new(), entries });
        app.audit_table.select(Some(0));
        app.go(Page::Audit);

        // Focus in the menu: down changes the page, not the row.
        press(&mut app, KeyCode::Down);
        assert!(app.page == Page::Settings);
        app.go(Page::Audit);

        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.audit_table.selected(), Some(2));
        press(&mut app, KeyCode::Up);
        assert_eq!(app.audit_table.selected(), Some(1));
        press(&mut app, KeyCode::End);
        assert_eq!(app.audit_table.selected(), Some(4));
        press(&mut app, KeyCode::Home);
        assert_eq!(app.audit_table.selected(), Some(0));
        assert!(app.page == Page::Audit);
    }

    #[test]
    fn typing_in_the_form_never_triggers_global_keys() {
        let (_rt, mut app) = app();
        let settings = Settings { api_url: String::new(), has_api_key: false, history_days: 30, encryption_available: true, encryption_error: None };
        app.screen = Screen::Form(SettingsForm::new(&settings));
        for c in "q?1".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert!(!app.quit);
        let Screen::Form(form) = &app.screen else { panic!("left the form") };
        assert_eq!(form.token.value, "q?1");
    }

    #[test]
    fn question_mark_toggles_help_and_any_key_closes_it() {
        let (_rt, mut app) = app();
        press(&mut app, KeyCode::Char('?'));
        assert!(app.show_help);
        press(&mut app, KeyCode::Char('x'));
        assert!(!app.show_help);
        assert!(!app.quit, "the key that closes help must not also act");
    }

    // ---- activity -----------------------------------------------------------

    use nvoke_core::{jobs::Job, task_state::JobTask};

    fn job(id: &str, watching: bool) -> Job {
        let mut task = JobTask::new(&format!("t-{id}"), None, "PC", 0);
        task.watching = watching;
        if !watching {
            task.task_status = Some("SUCCEEDED".into());
            task.execution = Some(nvoke_core::model::Execution { id: "e".into(), status: "SUCCEEDED".into(), exit_code: None, error_message: None, started_at: None, updated_at: None, duration_milliseconds: None, output: None });
        }
        Job {
            id: id.into(),
            name: id.into(),
            script_id: None,
            script: None,
            script_name: id.into(),
            org_id: None,
            org_name: None,
            target_label: "PC".into(),
            created_at: format!("2026-10-17T12:0{}:00Z", id.len()),
            timeout_seconds: 0,
            warnings: None,
            tasks: vec![task],
        }
    }

    fn activity() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = app();
        app.jobs.seed(vec![job("a", false), job("bb", true), job("ccc", false)]);
        app.go(Page::Activity);
        app.sync_jobs();
        press(&mut app, KeyCode::Right); // menu -> jobs
        (rt, app)
    }

    #[test]
    fn up_and_down_choose_a_run_and_the_selection_follows_it() {
        let (_rt, mut app) = activity();
        assert!(app.focus == Focus::Content);
        assert_eq!(app.selected_job.as_deref(), Some("a"));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_job.as_deref(), Some("bb"));
        press(&mut app, KeyCode::End);
        assert_eq!(app.selected_job.as_deref(), Some("ccc"));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_job.as_deref(), Some("ccc"), "stops at the last run");
        press(&mut app, KeyCode::Home);
        assert_eq!(app.selected_job.as_deref(), Some("a"));
        assert_eq!(app.groups.len(), 1, "the selected run's results are grouped");
    }

    #[test]
    fn right_goes_into_the_results_and_left_comes_back_out_one_level_at_a_time() {
        let (_rt, mut app) = activity();
        press(&mut app, KeyCode::Right);
        assert!(app.pane == Pane::Results);
        press(&mut app, KeyCode::Left);
        assert!(app.pane == Pane::Jobs && app.focus == Focus::Content);
        press(&mut app, KeyCode::Left);
        assert!(app.focus == Focus::Sidebar);
    }

    #[test]
    fn the_selected_run_stays_selected_when_new_runs_arrive_above_it() {
        let (_rt, mut app) = activity();
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_job.as_deref(), Some("bb"));
        app.jobs.seed(vec![job("new", false), job("a", false), job("bb", true), job("ccc", false)]);
        app.sync_jobs();
        assert_eq!(app.selected_job.as_deref(), Some("bb"));
        assert_eq!(app.job_table.selected(), Some(2));
    }

    #[test]
    fn s_stops_watching_the_selected_run() {
        let (_rt, mut app) = activity();
        press(&mut app, KeyCode::Down); // "bb" is the one being watched
        assert!(app.job_list[1].is_active());
        press(&mut app, KeyCode::Char('s'));
        assert!(!app.job_list[1].is_active());
        assert_eq!(app.job_list[1].tasks[0].stop_reason, Some(nvoke_core::task_state::StopReason::Manual));
    }

    #[test]
    fn removing_a_run_needs_a_y_and_any_other_key_cancels() {
        let (_rt, mut app) = activity();
        press(&mut app, KeyCode::Char('x'));
        assert!(app.confirm.is_some());
        press(&mut app, KeyCode::Char('n'));
        assert!(app.confirm.is_none());
        assert_eq!(app.job_list.len(), 3, "cancelled: nothing removed");

        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.job_list.len(), 2);
        assert!(app.job_list.iter().all(|j| j.id != "a"));
        assert!(app.selected_job.is_some(), "selection moves to a remaining run");
    }

    #[test]
    fn the_keys_that_act_on_a_run_do_nothing_when_there_are_none() {
        let (_rt, mut app) = app();
        app.jobs.seed(vec![]);
        app.go(Page::Activity);
        app.sync_jobs();
        press(&mut app, KeyCode::Right);
        for key in [KeyCode::Down, KeyCode::Right, KeyCode::Char('s'), KeyCode::Char('x'), KeyCode::End, KeyCode::PageDown] {
            press(&mut app, key);
        }
        assert!(app.confirm.is_none() && app.selected_job.is_none() && app.pane == Pane::Jobs);
    }

    // ---- dry run ------------------------------------------------------------

    #[test]
    fn d_opens_the_held_back_list_only_in_dry_run_and_esc_closes_it() {
        let (_rt, mut app) = app();
        app.core().set_dry_run(false);
        press(&mut app, KeyCode::Char('d'));
        assert!(!app.show_dry_log, "there is no list to open in a live session");

        app.core().set_dry_run(true);
        press(&mut app, KeyCode::Char('d'));
        assert!(app.show_dry_log);
        press(&mut app, KeyCode::Esc);
        assert!(!app.show_dry_log);
    }

    #[test]
    fn keys_inside_the_held_back_list_do_not_leak_to_the_app_behind_it() {
        let (_rt, mut app) = app();
        app.core().set_dry_run(true);
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('2')); // would switch page
        press(&mut app, KeyCode::Char('x')); // would ask to remove a run
        assert!(app.page == Page::Run && app.confirm.is_none());
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.quit, "q closes the list; it must not quit the app");
        assert!(!app.show_dry_log);
    }

    #[test]
    fn arrows_choose_between_held_back_changes() {
        let (rt, mut app) = app();
        app.core().set_dry_run(true);
        rt.block_on(async {
            for name in ["One", "Two", "Three"] {
                let _ = app.core().create_static_tag("o", name, "", "").await;
            }
        });
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.dry_table.selected(), Some(0), "newest first");
        assert!(app.dry_run_entries()[0].summary.contains("Three"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.dry_table.selected(), Some(2), "stops at the last entry");
        press(&mut app, KeyCode::Up);
        assert_eq!(app.dry_table.selected(), Some(1));
    }
}
