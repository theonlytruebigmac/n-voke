//! Application state and the update function.
//!
//! Everything that can happen arrives as an `Event` on one channel: key
//! presses and ticks from an input thread, and results from async work (the
//! core's API calls) started by `App`. `run` draws, waits for the next event,
//! updates the state, and repeats. Drawing (ui.rs) only reads the state.

use std::{io, sync::{Arc, mpsc}, thread, time::Duration};

use nvoke_core::{AuditLogView, Core, Settings};
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    widgets::TableState,
};
use tokio::runtime::Handle;

use crate::{input::TextInput, ui};

/// Also the animation frame rate: spinners and the logo shimmer advance once per tick.
const TICK: Duration = Duration::from_millis(100);

pub enum Event {
    Key(KeyEvent),
    /// Sent every `TICK` when nothing else happened; drives animation and, later, polling.
    Tick,
    Msg(Msg),
}

/// Results of async work.
pub enum Msg {
    Settings(Result<Settings, String>),
    Saved(Result<Settings, String>),
    Audit(Result<AuditLogView, String>),
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
        matches!(self, Page::Audit | Page::Settings)
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
    runtime: Handle,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    pub screen: Screen,
    pub page: Page,
    pub settings: Option<Settings>,
    pub audit: Load<AuditLogView>,
    pub audit_table: TableState,
    pub focus: Focus,
    pub show_help: bool,
    /// Counts ticks since start; animations are a function of it.
    pub tick: u64,
    quit: bool,
}

impl App {
    pub fn new(core: Arc<Core>, runtime: Handle) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            core,
            runtime,
            tx,
            rx,
            screen: Screen::Loading,
            page: Page::Run,
            settings: None,
            audit: Load::NotLoaded,
            audit_table: TableState::default(),
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
            let _ = tx.send(Event::Msg(future.await));
        });
    }

    fn update(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Tick => self.tick = self.tick.wrapping_add(1),
            Event::Msg(msg) => self.on_msg(msg),
        }
    }

    // ---- results -----------------------------------------------------------

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Settings(Ok(settings)) => {
                self.screen = if settings.has_api_key { Screen::Main } else { Screen::Form(SettingsForm::new(&settings)) };
                self.settings = Some(settings);
                self.enter_page();
            }
            Msg::Settings(Err(e)) => self.screen = Screen::Fatal(e),
            Msg::Saved(Ok(settings)) => {
                self.settings = Some(settings);
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
        matches!(self.screen, Screen::Loading) || matches!(self.audit, Load::Loading)
    }

    fn on_main_key(&mut self, key: KeyEvent) {
        if self.show_help {
            // Any key closes the help overlay.
            self.show_help = false;
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.show_help = true,
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
        if matches!(key.code, KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc) {
            self.focus = Focus::Sidebar;
            return;
        }
        match self.page {
            Page::Audit => self.on_audit_key(key),
            Page::Settings => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Char('e')) {
                    if let Some(settings) = &self.settings {
                        let mut form = SettingsForm::new(settings);
                        form.first_run = false;
                        self.screen = Screen::Form(form);
                    }
                }
            }
            _ => {}
        }
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
        let mut app = App::new(Arc::new(Core::new("test").unwrap()), runtime.handle().clone());
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
        press(&mut app, KeyCode::Right);
        assert!(app.focus == Focus::Sidebar, "Run isn't interactive yet, so focus stays in the menu");

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
}
