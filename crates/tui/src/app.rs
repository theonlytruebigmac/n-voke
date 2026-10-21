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

use crate::{
    input::TextInput,
    output_view::Viewer,
    picker::{PickItem, Picker},
    ui,
};

pub(crate) mod run;
pub(crate) mod tag_form;
pub(crate) mod watch;
pub use watch::WatchState;
pub(crate) mod tags;
use tags::{TagConfirm, TagDetail, TagDone};
pub use tags::{TagsPane, TagsState};
use tag_form::Saved;
pub use tag_form::{CondPart, FormMode, TagField, TagForm};
pub(crate) mod viewer;
pub use viewer::TOAST_TICKS;
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
    TagList { seq: u64, result: Result<Vec<nvoke_core::model::Tag>, String> },
    TagDetailLoaded { seq: u64, tag_id: String, result: Result<TagDetail, String> },
    TagDone(Result<TagDone, String>),
    TagPreview { seq: u64, key: String, result: Result<(Vec<nvoke_core::model::Asset>, i64), String> },
    TagFormDone(Result<Saved, nvoke_core::tags::CreateFailure>),
    Watched(Result<Box<nvoke_core::jobs::Job>, String>),
    DataCleared(Result<(), String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    /// Whether the page has anything to move around in (so focus can enter it).
    pub fn interactive(self) -> bool {
        matches!(self, Page::Run | Page::Activity | Page::Tags | Page::Audit | Page::Settings)
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

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Field {
    Endpoint,
    Token,
    /// How long run history is kept (not asked for on first launch).
    History,
}

/// The retention choices: days, with 0 meaning "don't save history".
pub const HISTORY_CHOICES: [u32; 4] = [7, 30, 90, 0];

/// The next (or previous) retention choice, wrapping round. A value that isn't one of the choices starts from 30.
pub fn step_history(current: u32, forward: bool) -> u32 {
    let at = HISTORY_CHOICES.iter().position(|d| *d == current).unwrap_or(1) as isize;
    let next = (at + if forward { 1 } else { -1 }).rem_euclid(HISTORY_CHOICES.len() as isize) as usize;
    HISTORY_CHOICES[next]
}

/// The first-launch screen and the settings editor are the same form.
pub struct SettingsForm {
    pub endpoint: TextInput,
    pub token: TextInput,
    pub focus: Field,
    pub history_days: u32,
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
            history_days: settings.history_days,
            error: None,
            busy: false,
            first_run: !settings.has_api_key,
        }
    }

    /// The fields shown, top to bottom.
    pub fn fields(&self) -> Vec<Field> {
        if self.first_run { vec![Field::Endpoint, Field::Token] } else { vec![Field::Endpoint, Field::Token, Field::History] }
    }

    fn step(&mut self, delta: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        self.focus = fields[(at + delta).rem_euclid(fields.len() as isize) as usize];
    }

    /// The text field with focus, if the focus is on one.
    pub fn focused(&mut self) -> Option<&mut TextInput> {
        match self.focus {
            Field::Endpoint => Some(&mut self.endpoint),
            Field::Token => Some(&mut self.token),
            Field::History => None,
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
    Tag(TagConfirm),
    /// Clear saved run history and cached data.
    ClearData,
}

impl Confirm {
    /// The question shown in the footer.
    pub fn prompt(&self) -> String {
        match self {
            Confirm::RemoveJob(_) => "Remove this run from your history?".into(),
            Confirm::ClearData => "Clear saved run history and cached data? Runs still being watched are kept; the audit log and your settings are not touched.".into(),
            Confirm::Tag(t) => t.prompt(),
        }
    }
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
    pub tags: TagsState,
    /// The new-tag / edit-tag form, when open.
    pub tag_form: Option<TagForm>,
    /// The "watch a task" box, when open.
    pub watch: Option<WatchState>,
    /// A short message about something that just happened, shown in the footer for a few seconds.
    pub flash: Option<(run::Notice, u64)>,
    /// Devices from a run, waiting to be put in a tag.
    pub(crate) assign: Option<tags::AssignCtx>,
    /// The open search popup, whichever page opened it.
    pub picker: Option<Picker>,
    /// The full-screen output reader.
    pub viewer: Option<Viewer>,
    /// Raw terminal sequences to send before the next draw (for the clipboard).
    pub(crate) outbox: Vec<String>,
    /// Where saved output goes (the downloads folder; tests point it somewhere disposable).
    pub(crate) export_dir: std::path::PathBuf,
    /// N-able has refused the saved API key (expired or revoked), as far as we have seen.
    pub key_rejected: bool,
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
    /// How far the help overlay is scrolled.
    pub help_scroll: u16,
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
            tags: TagsState::default(),
            tag_form: None,
            watch: None,
            flash: None,
            assign: None,
            picker: None,
            viewer: None,
            outbox: vec![],
            export_dir: viewer::export_dir(),
            key_rejected: false,
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
            help_scroll: 0,
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
                Ok(event) => {
                    self.update(event);
                    self.flush_outbox();
                }
                Err(_) => break,
            }
        }
        Ok(())
    }

    /// Sends queued raw sequences (clipboard requests) to the terminal.
    fn flush_outbox(&mut self) {
        if self.outbox.is_empty() {
            return;
        }
        use std::io::Write;
        let mut out = io::stdout();
        for sequence in self.outbox.drain(..) {
            let _ = out.write_all(sequence.as_bytes());
        }
        let _ = out.flush();
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
                self.tick_tags();
                self.tick_tag_form();
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
                    // Start from the lists the last session saved, so the pickers only ask what changed.
                    let collections = self.collections.clone();
                    self.runtime.spawn(async move { collections.load_persisted().await });
                }
                self.enter_page();
            }
            Msg::Settings(Err(e)) => self.screen = Screen::Fatal(e),
            Msg::Saved(Ok(settings)) => {
                // A fresh key gets a fresh start; if N-able refuses it too, the next request says so.
                self.key_rejected = false;
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
            Msg::TagList { seq, result } => self.on_tag_list(seq, result),
            Msg::TagDetailLoaded { seq, tag_id, result } => self.on_tag_detail(seq, tag_id, result),
            Msg::TagDone(result) => self.on_tag_done(result),
            Msg::TagPreview { seq, key, result } => self.on_tag_preview(seq, key, result),
            Msg::TagFormDone(result) => self.on_tag_form_done(result),
            Msg::Watched(result) => self.on_watched(result),
            Msg::DataCleared(result) => match result {
                Ok(()) => {
                    self.jobs.clear_finished();
                    self.sync_jobs();
                    let collections = self.collections.clone();
                    self.runtime.spawn(async move { collections.forget().await });
                    self.flash(NoticeKind::Info, "Cleared saved run history and cached data.");
                }
                Err(e) => self.flash(NoticeKind::Error, e),
            },
        }
    }

    /// Starts whatever the current page needs that it doesn't have yet.
    fn enter_page(&mut self) {
        if self.page == Page::Tags {
            self.enter_tags();
        }
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
            KeyCode::Tab | KeyCode::Down => form.step(1),
            KeyCode::BackTab | KeyCode::Up => form.step(-1),
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if form.focus == Field::History => {
                form.history_days = step_history(form.history_days, key.code != KeyCode::Left);
            }
            KeyCode::Enter => {
                if form.first_run && form.token.value.trim().is_empty() {
                    form.error = Some("Paste your N-able API-Access token to continue.".into());
                    form.focus = Field::Token;
                    return;
                }
                form.busy = true;
                form.error = None;
                let endpoint = form.endpoint.value.clone();
                let token = Some(form.token.value.clone()).filter(|t| !t.trim().is_empty());
                let history = (!form.first_run).then_some(form.history_days);
                // Another account or endpoint means other data: drop the lists held for the old one first.
                let credentials_changed = token.is_some() || self.settings.as_ref().is_some_and(|s| s.api_url != endpoint.trim());
                let collections = self.collections.clone();
                self.spawn(move |core| async move {
                    if credentials_changed {
                        collections.forget().await;
                    }
                    Msg::Saved(core.save_settings(endpoint, token, history).await)
                });
            }
            _ => {
                if let Some(input) = form.focused() {
                    input.handle_key(key);
                }
            }
        }
    }

    /// True while anything is waiting on the network or keychain.
    pub fn busy(&self) -> bool {
        matches!(self.screen, Screen::Loading) || matches!(self.audit, Load::Loading) || (self.jobs_started && !self.jobs.is_loaded())
    }

    fn on_main_key(&mut self, key: KeyEvent) {
        if let Some(confirm) = self.confirm.take() {
            if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                match confirm {
                    Confirm::RemoveJob(id) => {
                        self.jobs.remove(&id);
                        self.sync_jobs();
                    }
                    Confirm::Tag(tag) => self.run_tag_confirmed(tag),
                    Confirm::ClearData => self.spawn(|core| async move { Msg::DataCleared(core.clear_local_data().await) }),
                }
            }
            return; // any other key cancels
        }
        if self.show_help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?' | 'q') | KeyCode::F(1) => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => self.help_scroll = self.help_scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => self.help_scroll = self.help_scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => self.help_scroll = self.help_scroll.saturating_add(8),
                KeyCode::PageUp | KeyCode::Char('b') => self.help_scroll = self.help_scroll.saturating_sub(8),
                KeyCode::Home => self.help_scroll = 0,
                KeyCode::End => self.help_scroll = u16::MAX, // the drawing clamps it to the end
                _ => {} // other keys do nothing: the help is modal, and nothing leaks to the page behind it
            }
            return;
        }
        if self.watch.is_some() {
            return self.on_watch_key(key);
        }
        if self.viewer.is_some() {
            return self.on_viewer_key(key);
        }
        if self.show_dry_log {
            return self.on_dry_log_key(key);
        }
        if self.run_captures_keys() {
            return self.on_run_modal_key(key);
        }
        match key.code {
            // With a tag form open, q would throw away unsaved work: esc leaves it, asking first.
            KeyCode::Char('q') if self.tag_form.is_none() => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.show_help = true,
            KeyCode::Char('d') if self.core.is_dry_run() => {
                self.show_dry_log = true;
                self.dry_table.select((!self.core.dry_run_log().is_empty()).then_some(0));
                self.dry_scroll = 0;
            }
            KeyCode::Char(c @ '1'..='6') => self.go(Page::ALL[c as usize - '1' as usize]),
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
        if self.page == Page::Tags {
            return self.on_tags_key(key);
        }
        if matches!(key.code, KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc) {
            self.focus = Focus::Sidebar;
            return;
        }
        match self.page {
            Page::Audit => self.on_audit_key(key),
            Page::Settings if key.code == KeyCode::Char('c') => self.confirm = Some(Confirm::ClearData),
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

    /// Puts tags in the cache (for tests of code built on it).
    #[cfg(test)]
    pub(crate) fn collections_seed_tags(&self, tags: Vec<nvoke_core::model::Tag>) {
        self.collections.seed(vec![], tags, vec![]);
    }

    /// Shows a short message in the footer for a few seconds.
    pub(crate) fn flash(&mut self, kind: NoticeKind, text: impl Into<String>) {
        self.flash = Some((run::Notice { kind, text: text.into() }, self.tick));
    }

    /// Remembers that N-able refused the API key, if that is what this error is.
    pub(crate) fn note_error(&mut self, error: &str) {
        if nvoke_core::api::is_access_denied(error) {
            self.key_rejected = true;
        }
    }

    /// Picks up changes the store made (polling, saved history arriving) so the screen shows them.
    pub fn sync_jobs(&mut self) {
        let version = self.jobs.version();
        if version == self.jobs_version {
            return;
        }
        self.jobs_version = version;
        self.job_list = self.jobs.jobs();
        // Polling a run in the background is often the first thing to notice an expired key.
        let denied = self.job_list.iter().flat_map(|j| &j.tasks).filter(|t| t.watching).filter_map(|t| t.last_error.as_deref()).any(nvoke_core::api::is_access_denied);
        if denied {
            self.key_rejected = true;
        }
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
            KeyCode::Char('?') | KeyCode::F(1) => self.show_help = true,
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

    /// Whether `?` opens the help here: not while a text box is taking every key, where it is a character.
    pub fn help_reachable(&self) -> bool {
        self.watch.is_none() && !self.run_captures_keys() && !self.viewer.as_ref().is_some_and(|v| v.searching)
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
            (Pane::Results, KeyCode::Enter | KeyCode::Char('o')) => self.open_viewer(),
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
            (_, KeyCode::Char('W')) => self.open_watch(),
            (pane, KeyCode::Char('T')) => self.start_tag_devices(pane == Pane::Results),
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

    /// Goes to a page, for tests of code that depends on where the user is.
    #[cfg(test)]
    pub(crate) fn go_to_for_tests(&mut self, page: Page) {
        self.go(page);
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
        // Every page can be entered now, so Right always steps into the page.
        assert!(Page::ALL.iter().all(|p| p.interactive()));
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
    fn the_help_is_modal_scrolls_and_closes_with_esc_or_question_mark() {
        let (_rt, mut app) = app();
        press(&mut app, KeyCode::Char('?'));
        assert!(app.show_help);
        // Other keys do nothing: the help is a popup, so nothing leaks to the page behind it.
        for c in ['x', '1', 'd', 'R'] {
            press(&mut app, KeyCode::Char(c));
        }
        assert!(app.show_help && !app.quit && app.page == Page::Run);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.help_scroll, 9);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.help_scroll, 8);
        press(&mut app, KeyCode::Home);
        assert_eq!(app.help_scroll, 0);
        press(&mut app, KeyCode::Char('?'));
        assert!(!app.show_help && !app.quit, "the key that closes help must not also act");
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Esc);
        assert!(!app.show_help);
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.show_help && !app.quit, "q closes the help, it does not quit the app");
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
            migration_project: None,
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

    // ---- settings --------------------------------------------------------------------------------

    fn open_settings_form(app: &mut App) {
        app.settings = Some(Settings { api_url: "https://api.n-able.com/graphql".into(), has_api_key: true, history_days: 30, encryption_available: true, encryption_error: None });
        app.screen = Screen::Main;
        app.go(Page::Settings);
        app.focus = Focus::Content;
        press(app, KeyCode::Enter);
    }

    #[test]
    fn the_first_launch_form_asks_only_for_the_endpoint_and_token_but_editing_also_asks_how_long_to_keep_history() {
        let first = SettingsForm::new(&Settings { api_url: String::new(), has_api_key: false, history_days: 30, encryption_available: true, encryption_error: None });
        assert_eq!(first.fields(), [Field::Endpoint, Field::Token]);
        let edit = SettingsForm::new(&Settings { api_url: String::new(), has_api_key: true, history_days: 30, encryption_available: true, encryption_error: None });
        assert_eq!(edit.fields(), [Field::Endpoint, Field::Token, Field::History]);
        assert_eq!(edit.history_days, 30, "it starts at what is saved");
    }

    #[test]
    fn the_retention_steps_through_its_choices_and_wraps_round_both_ways() {
        assert_eq!(step_history(7, true), 30);
        assert_eq!(step_history(30, true), 90);
        assert_eq!(step_history(90, true), 0, "never saved comes last");
        assert_eq!(step_history(0, true), 7, "and wraps");
        assert_eq!(step_history(7, false), 0);
        assert_eq!(step_history(0, false), 90);
        assert_eq!(step_history(14, true), 90, "a value that isn't a choice starts from 30");
    }

    #[test]
    fn tab_and_arrows_move_through_the_settings_fields_and_the_retention_field_takes_arrows_not_typing() {
        let (_rt, mut app) = app();
        open_settings_form(&mut app);
        let focus = |app: &App| match &app.screen { Screen::Form(f) => f.focus, _ => panic!("no form") };
        assert!(focus(&app) == Field::Endpoint);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Down);
        assert!(focus(&app) == Field::History);
        press(&mut app, KeyCode::Down);
        assert!(focus(&app) == Field::Endpoint, "wraps round");
        press(&mut app, KeyCode::Up);
        assert!(focus(&app) == Field::History);
        let Screen::Form(before) = &app.screen else { panic!() };
        let (endpoint, token) = (before.endpoint.value.clone(), before.token.value.clone());
        for c in "abc".chars() {
            press(&mut app, KeyCode::Char(c)); // letters mean nothing on the retention field
        }
        press(&mut app, KeyCode::Right);
        let Screen::Form(f) = &app.screen else { panic!() };
        assert_eq!((f.endpoint.value.clone(), f.token.value.clone()), (endpoint, token), "typing did not leak into the text fields");
        assert_eq!(f.history_days, 90);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        let Screen::Form(f) = &app.screen else { panic!() };
        assert_eq!(f.history_days, 7);
    }

    #[test]
    fn saving_a_new_retention_takes_effect_and_is_remembered() {
        let (_rt, mut app) = app();
        open_settings_form(&mut app);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right); // 90 days
        press(&mut app, KeyCode::Enter);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !matches!(app.screen, Screen::Main) && std::time::Instant::now() < deadline {
            if let Ok(event) = app.rx.recv_timeout(std::time::Duration::from_millis(100)) {
                app.update(event);
            }
        }
        assert!(matches!(app.screen, Screen::Main), "the form closed after saving");
        assert_eq!(app.settings.as_ref().unwrap().history_days, 90);
    }

    #[test]
    fn c_on_settings_asks_before_clearing_saved_data_and_says_what_is_kept() {
        let (_rt, mut app) = app();
        app.settings = Some(Settings { api_url: "https://x/graphql".into(), has_api_key: true, history_days: 30, encryption_available: true, encryption_error: None });
        app.screen = Screen::Main;
        app.go(Page::Settings);
        app.focus = Focus::Content;
        press(&mut app, KeyCode::Char('c'));
        let prompt = app.confirm.as_ref().unwrap().prompt();
        assert!(prompt.starts_with("Clear saved run history and cached data?") && prompt.contains("still being watched are kept") && prompt.contains("audit log"), "{prompt}");
        press(&mut app, KeyCode::Char('n'));
        assert!(app.confirm.is_none() && app.flash.is_none(), "anything but y cancels");
    }

    #[test]
    fn clearing_removes_finished_runs_but_keeps_the_ones_still_being_watched() {
        let (_rt, mut app) = app();
        app.settings = Some(Settings { api_url: "https://x/graphql".into(), has_api_key: true, history_days: 30, encryption_available: true, encryption_error: None });
        app.screen = Screen::Main;
        let mut done = nvoke_core::task_state::JobTask::new("t1", None, "PC", 0);
        done.watching = false;
        let running = nvoke_core::task_state::JobTask::new("t2", None, "PC", 0);
        let job = |id: &str, task| nvoke_core::jobs::Job { id: id.into(), name: id.into(), script_id: None, script: None, script_name: id.into(), org_id: None, org_name: None, target_label: "PC".into(), created_at: "2026-10-17T12:00:00.000Z".into(), timeout_seconds: 0, warnings: None, migration_project: None, tasks: vec![task] };
        app.jobs.seed(vec![job("finished", done), job("running", running)]);
        app.sync_jobs();
        app.go(Page::Settings);
        app.focus = Focus::Content;
        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Char('y'));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.flash.is_none() && std::time::Instant::now() < deadline {
            if let Ok(event) = app.rx.recv_timeout(std::time::Duration::from_millis(100)) {
                app.update(event);
            }
        }
        assert_eq!(app.job_list.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), ["running"]);
        assert_eq!(app.flash.as_ref().unwrap().0.text, "Cleared saved run history and cached data.");
    }
}
