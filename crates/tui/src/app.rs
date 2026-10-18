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
    widgets::ListState,
};
use tokio::runtime::Handle;

use crate::{input::TextInput, ui};

const TICK: Duration = Duration::from_millis(250);

pub enum Event {
    Key(KeyEvent),
    /// Sent every `TICK` when nothing else happened; drives spinners and polling later.
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

    fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap()
    }
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
    pub audit_list: ListState,
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
            audit_list: ListState::default(),
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
            Event::Tick => {}
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
                self.audit_list.select((!log.entries.is_empty()).then_some(0));
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

    fn on_main_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char(c @ '1'..='5') => self.go(Page::ALL[c as usize - '1' as usize]),
            KeyCode::Tab => self.go(Page::ALL[(self.page.index() + 1) % Page::ALL.len()]),
            KeyCode::BackTab => self.go(Page::ALL[(self.page.index() + Page::ALL.len() - 1) % Page::ALL.len()]),
            _ => match self.page {
                Page::Audit => self.on_audit_key(key),
                Page::Settings => {
                    if key.code == KeyCode::Char('e') {
                        if let Some(settings) = &self.settings {
                            let mut form = SettingsForm::new(settings);
                            form.first_run = false;
                            self.screen = Screen::Form(form);
                        }
                    }
                }
                _ => {}
            },
        }
    }

    fn go(&mut self, page: Page) {
        self.page = page;
        self.enter_page();
    }

    fn on_audit_key(&mut self, key: KeyEvent) {
        let len = match &self.audit {
            Load::Ready(log) => log.entries.len(),
            _ => 0,
        };
        match key.code {
            KeyCode::Char('r') => self.reload_audit(),
            KeyCode::Down | KeyCode::Char('j') => self.audit_list.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.audit_list.select_previous(),
            KeyCode::Home | KeyCode::Char('g') => self.audit_list.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.audit_list.select(len.checked_sub(1)),
            _ => {}
        }
    }
}
