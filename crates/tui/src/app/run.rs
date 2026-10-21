//! The Run page's state and behaviour: which field is focused, the pickers, the
//! devices that will be run on, and the confirm-then-run step. Drawing is in
//! `run_ui.rs`.

use std::collections::HashSet;

use nvoke_core::{
    DRY_RUN_ERROR, is_dry_run_error,
    jobs::Job,
    model::{Asset, Organization, Script},
    run::{FieldValue, InputValues, Problems, Resolved, RunRequest, check, resolve_targets, start_run},
    targets::{Target, target_label},
};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    widgets::TableState,
};

use super::{App, Focus, Load, Msg, Page};
use crate::{
    input::TextInput,
    picker::{PickItem, PickKind, Picker, Purpose},
};

/// A row of the form that can take focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunField {
    Org,
    Script,
    Targets,
    /// The nth input of the chosen script.
    Input(usize),
    Name,
    Timeout,
    Run,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunPane {
    Form,
    Devices,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Error,
}

pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

pub struct RunState {
    pub org: Option<Organization>,
    pub script: Option<Script>,
    pub targets: Vec<Target>,
    pub values: InputValues,
    pub task_name: TextInput,
    pub timeout: TextInput,
    pub field: RunField,
    /// A text field is being typed into: keys go to it instead of acting as shortcuts.
    pub editing: bool,
    pub pane: RunPane,
    /// The devices the targets resolve to.
    pub resolved: Load<Vec<Resolved>>,
    /// A tag has more devices than could be listed.
    pub truncated: bool,
    /// Devices unticked in the review table.
    pub excluded: HashSet<String>,
    pub devices_table: TableState,
    /// Show what is wrong with the inputs (after a first attempt to run).
    pub show_errors: bool,
    /// Waiting for a second Enter to really run.
    pub confirming: bool,
    pub running: bool,
    pub notice: Option<Notice>,
    resolve_seq: u64,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            org: None,
            script: None,
            targets: vec![],
            values: InputValues::new(),
            task_name: TextInput::default(),
            timeout: TextInput::default(),
            field: RunField::Org,
            editing: false,
            pane: RunPane::Form,
            resolved: Load::NotLoaded,
            truncated: false,
            excluded: HashSet::new(),
            devices_table: TableState::default(),
            show_errors: false,
            confirming: false,
            running: false,
            notice: None,
            resolve_seq: 0,
        }
    }
}

impl RunState {
    /// The fields that exist right now, top to bottom.
    pub fn fields(&self) -> Vec<RunField> {
        let mut fields = vec![RunField::Org];
        if self.org.is_some() {
            fields.extend([RunField::Script, RunField::Targets]);
        }
        if let Some(script) = &self.script {
            fields.extend((0..script.inputs.as_ref().map_or(0, Vec::len)).map(RunField::Input));
            fields.extend([RunField::Name, RunField::Timeout]);
        }
        fields.push(RunField::Run);
        fields
    }

    /// The devices that will be run on: the resolved ones the user hasn't unticked.
    /// Empty while the list is loading or failed, since it would belong to the previous targets.
    pub fn devices(&self) -> Vec<Asset> {
        match &self.resolved {
            Load::Ready(list) => list.iter().filter(|r| !self.excluded.contains(&r.asset.id)).map(|r| r.asset.clone()).collect(),
            _ => vec![],
        }
    }

    pub fn problems(&self) -> Problems {
        self.script.as_ref().map(|s| check(s, &self.values, self.timeout.value.trim())).unwrap_or_default()
    }

    /// Why the run can't start yet, if it can't.
    pub fn blocker(&self) -> Option<&'static str> {
        if self.org.is_none() {
            Some("Choose an organization")
        } else if self.script.is_none() {
            Some("Choose a script")
        } else if self.targets.is_empty() {
            Some("Choose devices or tags to run on")
        } else if matches!(self.resolved, Load::Loading | Load::NotLoaded) {
            Some("Finding devices…")
        } else if matches!(self.resolved, Load::Failed(_)) {
            Some("The device list failed to load")
        } else if self.devices().is_empty() {
            Some("No devices selected")
        } else {
            None
        }
    }

    /// Any change to what would run cancels a pending confirmation.
    fn touch(&mut self) {
        self.confirming = false;
    }

    fn tidy_focus(&mut self) {
        let fields = self.fields();
        if !fields.contains(&self.field) {
            self.field = *fields.last().unwrap();
        }
    }

    fn step(&mut self, delta: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.field).unwrap_or(0) as isize;
        self.field = fields[(at + delta).clamp(0, fields.len() as isize - 1) as usize];
    }

    /// The text of an input value, for editing.
    fn text_of(&self, variable: &str) -> String {
        match self.values.get(variable) {
            Some(FieldValue::Text(t)) => t.clone(),
            _ => String::new(),
        }
    }

    pub fn is_target(&self, key: &str) -> bool {
        self.targets.iter().any(|t| t.key() == key)
    }

    /// Replaces the form with a pre-filled one (for "run again").
    pub fn prefill(&mut self, org: Option<Organization>, script: Option<Script>, targets: Vec<Target>) {
        *self = Self::default();
        self.org = org;
        self.script = script;
        self.targets = targets;
        self.field = if self.script.is_none() { RunField::Script } else if self.targets.is_empty() { RunField::Targets } else { RunField::Run };
        self.tidy_focus();
    }

    pub fn label(&self) -> String {
        target_label(&self.targets)
    }
}

/// Applies a typing key to a string. Returns whether the key was an editing key.
fn edit(text: &mut String, key: KeyEvent) -> bool {
    let mut input = TextInput::new(text, false);
    let handled = input.handle_key(key);
    *text = input.value;
    handled
}

impl App {
    // ---- keys ----------------------------------------------------------------

    /// Whether the Run page currently owns every key (a popup or a text field is open).
    pub(super) fn run_captures_keys(&self) -> bool {
        let in_content = self.focus == Focus::Content;
        let form_editing = self.tag_form.as_ref().is_some_and(|f| f.editing);
        self.picker.is_some() || (in_content && ((self.page == Page::Run && self.run.editing) || (self.page == Page::Tags && (self.tags.searching || form_editing))))
    }

    pub(super) fn on_run_modal_key(&mut self, key: KeyEvent) {
        if self.picker.is_some() {
            self.on_picker_key(key);
        } else if self.page == Page::Tags {
            self.on_tags_key(key);
        } else {
            self.on_run_edit_key(key);
        }
    }

    /// Keys for the Run page when nothing modal is open.
    pub(super) fn on_run_key(&mut self, key: KeyEvent) {
        if self.run.running {
            return; // the request is in flight
        }
        match self.run.pane {
            RunPane::Form => self.on_run_form_key(key),
            RunPane::Devices => self.on_run_devices_key(key),
        }
    }

    fn on_run_form_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => {
                if self.run.confirming {
                    self.run.confirming = false;
                } else {
                    self.focus = Focus::Sidebar;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.run.touch_notice();
                self.run.step(-1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.run.touch_notice();
                self.run.step(1);
            }
            KeyCode::Right | KeyCode::Char('l') if matches!(self.run.resolved, Load::Ready(_)) && !self.run.targets.is_empty() => self.run.pane = RunPane::Devices,
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_field(key.code == KeyCode::Enter),
            KeyCode::Char('c') if self.run.field == RunField::Targets => {
                self.run.targets.clear();
                self.run.touch();
                self.resolve_devices();
            }
            _ => {}
        }
    }

    /// Enter (or space, for toggles) on the focused field.
    fn activate_field(&mut self, is_enter: bool) {
        match self.run.field {
            RunField::Org => self.open_picker(PickKind::Org),
            RunField::Script => self.open_picker(PickKind::Script),
            RunField::Targets => self.open_picker(PickKind::Targets),
            RunField::Input(i) => {
                let Some(decl) = self.run.script.as_ref().and_then(|s| s.inputs.as_ref()).and_then(|v| v.get(i)).cloned() else { return };
                if decl.kind == "BOOLEAN" {
                    let on = matches!(self.run.values.get(&decl.variable), Some(FieldValue::Bool(true)));
                    self.run.values.insert(decl.variable, FieldValue::Bool(!on));
                    self.run.touch();
                } else if is_enter {
                    self.run.editing = true;
                }
            }
            RunField::Name | RunField::Timeout => {
                if is_enter {
                    self.run.editing = true;
                }
            }
            RunField::Run => {
                if is_enter {
                    self.try_run();
                }
            }
        }
    }

    pub(super) fn on_run_edit_key(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
            self.run.editing = false;
            return;
        }
        // Moving on while typing: tab/down to the next field, as in a form.
        if matches!(key.code, KeyCode::Tab | KeyCode::Down) {
            self.run.editing = false;
            self.run.step(1);
            return;
        }
        if matches!(key.code, KeyCode::BackTab | KeyCode::Up) {
            self.run.editing = false;
            self.run.step(-1);
            return;
        }
        let changed = match self.run.field {
            RunField::Name => self.run.task_name.handle_key(key),
            RunField::Timeout => {
                // Only digits make sense; anything else is shown as an error rather than silently dropped.
                self.run.timeout.handle_key(key)
            }
            RunField::Input(i) => {
                let Some(variable) = self.run.script.as_ref().and_then(|s| s.inputs.as_ref()).and_then(|v| v.get(i)).map(|d| d.variable.clone()) else { return };
                let mut text = self.run.text_of(&variable);
                let handled = edit(&mut text, key);
                if handled {
                    self.run.values.insert(variable, FieldValue::Text(text));
                }
                handled
            }
            _ => false,
        };
        if changed {
            self.run.touch();
        }
    }

    fn on_run_devices_key(&mut self, key: KeyEvent) {
        let count = match &self.run.resolved {
            Load::Ready(list) => list.len(),
            _ => 0,
        };
        let table = &mut self.run.devices_table;
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => self.run.pane = RunPane::Form,
            KeyCode::Down | KeyCode::Char('j') => table.select_next(),
            KeyCode::Up | KeyCode::Char('k') => table.select_previous(),
            KeyCode::PageDown => table.scroll_down_by(10),
            KeyCode::PageUp => table.scroll_up_by(10),
            KeyCode::Home | KeyCode::Char('g') => table.select_first(),
            KeyCode::End | KeyCode::Char('G') => table.select(count.checked_sub(1)),
            KeyCode::Char(' ') | KeyCode::Enter => {
                if let (Some(i), Load::Ready(list)) = (table.selected(), &self.run.resolved)
                    && let Some(r) = list.get(i)
                {
                    let id = r.asset.id.clone();
                    if !self.run.excluded.remove(&id) {
                        self.run.excluded.insert(id);
                    }
                    self.run.touch();
                }
            }
            KeyCode::Char('a') => {
                if let Load::Ready(list) = &self.run.resolved {
                    let all_in = self.run.excluded.is_empty();
                    self.run.excluded = if all_in { list.iter().map(|r| r.asset.id.clone()).collect() } else { HashSet::new() };
                    self.run.touch();
                }
            }
            _ => {}
        }
    }

    // ---- pickers -------------------------------------------------------------

    /// Opens a picker for one of the Run page's fields.
    fn open_picker(&mut self, kind: PickKind) {
        let (title, purpose) = match kind {
            PickKind::Org => ("Choose an organization", Purpose::RunOrg),
            PickKind::Script => ("Choose a script", Purpose::RunScript),
            _ => ("Run on", Purpose::RunTargets),
        };
        let org_id = self.run.org.as_ref().map(|o| o.id.clone());
        self.run.touch_notice();
        self.open_picker_with(Picker::new(kind, purpose, title, org_id));
    }

    /// Opens any picker and starts listing its first results.
    pub(crate) fn open_picker_with(&mut self, picker: Picker) {
        self.picker = Some(picker);
        self.picker_search();
    }

    fn on_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = self.picker.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.close_picker(),
            KeyCode::Down => picker.move_by(1),
            KeyCode::Up => picker.move_by(-1),
            KeyCode::PageDown => picker.move_by(10),
            KeyCode::PageUp => picker.move_by(-10),
            KeyCode::Home => picker.move_by(-(picker.items.len() as isize)),
            KeyCode::End => picker.move_by(picker.items.len() as isize),
            KeyCode::Tab | KeyCode::BackTab if picker.purpose == Purpose::TagAccess => {
                picker.category = picker.category.map(|c| c.next());
                picker.items.clear();
                picker.table.select(None);
                self.picker_search();
            }
            KeyCode::Tab | KeyCode::BackTab if picker.kind == PickKind::Targets => {
                picker.tags_mode = !picker.tags_mode;
                picker.items.clear();
                picker.table.select(None);
                self.picker_search();
            }
            KeyCode::Enter => self.choose_picked(),
            _ => {
                if picker.input.handle_key(key) {
                    picker.typed_at = Some(self.tick);
                    // Local lists answer instantly; device searches wait for a pause in typing (see `tick_picker`).
                    if !picker.debounces() {
                        self.picker_search();
                    }
                }
            }
        }
    }

    /// Closes the picker. Devices picked for a tag turn into a question about adding them.
    fn close_picker(&mut self) {
        let purpose = self.picker.take().map(|p| p.purpose);
        if purpose == Some(Purpose::TagMembers) {
            self.finish_adding_devices();
        }
        if purpose == Some(Purpose::AssignTag) {
            self.assign = None;
        }
    }

    /// The items already chosen in the open picker (they lead its list and carry a tick).
    fn picker_chosen_items(&self) -> Vec<PickItem> {
        match self.picker.as_ref().map(|p| p.purpose) {
            Some(Purpose::RunTargets) => self.run.targets.iter().map(|t| match t {
                Target::Device(a) => PickItem::Asset(a.clone()),
                Target::Tag(t) => PickItem::Tag(t.clone()),
            }).collect(),
            Some(Purpose::TagMembers) => self.tags.adding.iter().cloned().map(PickItem::Asset).collect(),
            Some(Purpose::TagFormDevices) => self.tag_form.as_ref().map(|f| f.data.devices.iter().cloned().map(PickItem::Asset).collect()).unwrap_or_default(),
            Some(Purpose::TagAccess) => self.tag_form.as_ref().and_then(|f| f.data.org.clone()).map(|o| vec![PickItem::Org(o)]).unwrap_or_default(),
            _ => vec![],
        }
    }

    pub(crate) fn picker_chosen(&self) -> HashSet<String> {
        self.picker_chosen_items().iter().map(PickItem::key).collect()
    }

    /// Enter in a picker: choose and close, or (for several-at-a-time pickers) toggle and stay open.
    fn choose_picked(&mut self) {
        let Some(item) = self.picker.as_ref().and_then(|p| p.selected_item().cloned()) else { return };
        match self.picker.as_ref().map(|p| p.purpose) {
            Some(Purpose::AssignTag) => {
                self.choose_for_assign(item);
                return;
            }
            Some(Purpose::TagAccess) => {
                if let PickItem::Org(org) = item {
                    self.choose_form_org(org);
                }
                return;
            }
            Some(Purpose::TagFormDevices) => {
                if let PickItem::Asset(asset) = item {
                    self.toggle_form_device(asset);
                    self.refresh_picker_marks();
                }
                return;
            }
            _ => {}
        }
        if self.picker.as_ref().is_some_and(|p| p.purpose == Purpose::TagMembers) {
            if let PickItem::Asset(asset) = item {
                match self.tags.adding.iter().position(|a| a.id == asset.id) {
                    Some(at) => {
                        self.tags.adding.remove(at);
                    }
                    None => self.tags.adding.push(asset),
                }
                self.refresh_picker_marks();
            }
            return;
        }
        match item {
            PickItem::Org(org) => {
                self.picker = None;
                if self.run.org.as_ref().map(|o| &o.id) != Some(&org.id) {
                    self.run.org = Some(org);
                    self.run.script = None;
                    self.run.targets.clear();
                    self.run.values.clear();
                    self.run.excluded.clear();
                    self.run.show_errors = false;
                    self.run.touch();
                    self.resolve_devices();
                }
                self.run.field = RunField::Script;
                self.open_picker(PickKind::Script); // the next thing to fill in
            }
            PickItem::Script(script) => {
                self.picker = None;
                self.run.script = Some(script);
                self.run.values.clear();
                self.run.show_errors = false;
                self.run.touch();
                self.run.field = RunField::Targets;
                if self.run.targets.is_empty() {
                    self.open_picker(PickKind::Targets);
                }
            }
            target_item => {
                let Some(target) = target_item.as_target() else { return };
                let key = target.key();
                if self.run.is_target(&key) {
                    self.run.targets.retain(|t| t.key() != key);
                } else {
                    self.run.targets.push(target);
                }
                self.run.touch();
                self.resolve_devices();
                // Show the change in the list straight away: chosen items stay at the top.
                self.refresh_picker_marks();
            }
        }
        self.run.tidy_focus();
    }

    /// Re-sorts the open picker so chosen items lead and the highlight stays put.
    fn refresh_picker_marks(&mut self) {
        let chosen = self.picker_chosen_items();
        if let Some(p) = self.picker.as_mut() {
            let selected = p.selected_item().map(PickItem::key);
            p.items = lead_with_chosen(std::mem::take(&mut p.items), &chosen, "");
            p.table.select(selected.and_then(|k| p.items.iter().position(|i| i.key() == k)).or((!p.items.is_empty()).then_some(0)));
        }
    }

    /// Starts a search for the open picker's current term.
    pub(super) fn picker_search(&mut self) {
        let Some(p) = self.picker.as_mut() else { return };
        p.seq += 1;
        p.loading = true;
        p.error = None;
        p.typed_at = None;
        let (seq, kind, term, tags_mode, org_id, exclude, category) = (p.seq, p.kind, p.input.value.clone(), p.tags_mode, p.org_id.clone(), p.exclude.clone(), p.category);
        let collections = self.collections.clone();
        self.spawn(move |_| async move {
            let result: Result<Vec<PickItem>, String> = async {
                Ok(match (kind, org_id) {
                    (PickKind::Org, _) => match category {
                        Some(c) => collections.organizations_in(&term, c).await?,
                        None => collections.organizations(&term).await?,
                    }
                    .into_iter()
                    .map(PickItem::Org)
                    .collect(),
                    (PickKind::Script, Some(org)) => collections.scripts(&org, &term).await?.into_iter().map(PickItem::Script).collect(),
                    (PickKind::Targets, Some(org)) if tags_mode => collections.tags(&term, Some(&org)).await?.into_iter().map(PickItem::Tag).collect(),
                    (PickKind::Targets, Some(org)) => collections.assets(&org, &term).await?.into_iter().map(PickItem::Asset).collect(),
                    (PickKind::Devices, Some(org)) => collections.assets(&org, &term).await?.into_iter().filter(|a| !exclude.contains(&a.id)).map(PickItem::Asset).collect(),
                    (PickKind::StaticTags, Some(org)) => collections.tags(&term, Some(&org)).await?.into_iter().filter(|t| !t.is_dynamic()).map(PickItem::Tag).collect(),
                    (_, None) => vec![],
                })
            }
            .await;
            Msg::PickResults { seq, result }
        });
    }

    /// Called every tick: starts a device search once typing has paused.
    pub(super) fn tick_picker(&mut self) {
        let due = self.picker.as_ref().is_some_and(|p| p.debounces() && p.typed_at.is_some_and(|at| self.tick.saturating_sub(at) >= 3));
        if due {
            self.picker_search();
        }
    }

    pub(super) fn on_pick_results(&mut self, seq: u64, result: Result<Vec<PickItem>, String>) {
        if let Err(e) = &result {
            self.note_error(e);
        }
        let chosen = self.picker_chosen_items();
        let Some(p) = self.picker.as_mut() else { return };
        if seq != p.seq {
            return; // an answer to an older search
        }
        p.loading = false;
        match result {
            Ok(items) => {
                let mut items = if matches!(p.kind, PickKind::Targets | PickKind::Devices) { lead_with_chosen(items, &chosen, p.input.value.trim()) } else { items };
                if p.kind == PickKind::StaticTags {
                    // Nothing matches exactly: offer to make it (the name tidied into one a tag may have).
                    let term = p.input.value.trim();
                    let exact = items.iter().any(|i| matches!(i, PickItem::Tag(t) if t.name.eq_ignore_ascii_case(term)));
                    if !term.is_empty() && !exact {
                        items.push(PickItem::NewTag(term.to_string()));
                    }
                }
                p.set_items(items);
            }
            Err(e) => {
                p.error = Some(e);
                p.items.clear();
                p.table.select(None);
            }
        }
    }

    // ---- devices ---------------------------------------------------------------

    /// Works out which devices the chosen targets add up to.
    pub(super) fn resolve_devices(&mut self) {
        self.run.resolve_seq += 1;
        let seq = self.run.resolve_seq;
        let Some(org) = self.run.org.clone().filter(|_| !self.run.targets.is_empty()) else {
            self.run.resolved = Load::NotLoaded;
            self.run.truncated = false;
            return;
        };
        self.run.resolved = Load::Loading;
        let targets = self.run.targets.clone();
        let collections = self.collections.clone();
        self.spawn(move |_| async move { Msg::Resolved { seq, result: resolve_targets(&collections, &org.id, &targets).await } });
    }

    pub(super) fn on_resolved(&mut self, seq: u64, result: Result<(Vec<Resolved>, bool), String>) {
        if let Err(e) = &result {
            self.note_error(e);
        }
        if seq != self.run.resolve_seq {
            return;
        }
        match result {
            Ok((list, truncated)) => {
                // Devices that just (re)joined the list start ticked, even if they were unticked before they left.
                let before: HashSet<String> = match &self.run.resolved {
                    Load::Ready(prev) => prev.iter().map(|r| r.asset.id.clone()).collect(),
                    _ => HashSet::new(),
                };
                for r in list.iter().filter(|r| !before.contains(&r.asset.id)) {
                    self.run.excluded.remove(&r.asset.id);
                }
                self.run.devices_table.select((!list.is_empty()).then(|| self.run.devices_table.selected().unwrap_or(0).min(list.len() - 1)));
                self.run.truncated = truncated;
                self.run.resolved = Load::Ready(list);
            }
            Err(e) => self.run.resolved = Load::Failed(e),
        }
        self.run.touch();
        if !self.run.devices().is_empty() || self.run.targets.is_empty() {
            return;
        }
        if self.run.pane == RunPane::Devices {
            self.run.pane = RunPane::Form;
        }
    }

    // ---- running ---------------------------------------------------------------

    /// Enter on the Run button: check, then ask for a second Enter, then send.
    fn try_run(&mut self) {
        self.run.notice = None;
        if let Some(reason) = self.run.blocker() {
            self.run.notice = Some(Notice { kind: NoticeKind::Error, text: format!("{reason}.") });
            return;
        }
        self.run.show_errors = true;
        if self.run.problems().any() {
            self.run.notice = Some(Notice { kind: NoticeKind::Error, text: "Fix the highlighted fields first.".into() });
            // Take the user to the first field with a problem.
            let first = self.run.fields().into_iter().find(|f| match f {
                RunField::Timeout => self.run.problems().timeout.is_some(),
                RunField::Input(i) => self.run.script.as_ref().and_then(|s| s.inputs.as_ref()).and_then(|v| v.get(*i)).is_some_and(|d| self.run.problems().inputs.contains_key(&d.variable)),
                _ => false,
            });
            if let Some(f) = first {
                self.run.field = f;
            }
            return;
        }
        if !self.run.confirming {
            self.run.confirming = true;
            return;
        }
        let (Some(org), Some(script)) = (self.run.org.clone(), self.run.script.clone()) else { return };
        let request = RunRequest {
            org,
            script,
            targets: self.run.targets.clone(),
            devices: self.run.devices(),
            values: self.run.values.clone(),
            task_name: self.run.task_name.value.clone(),
            timeout: self.run.timeout.value.trim().to_string(),
        };
        self.run.running = true;
        self.run.confirming = false;
        self.spawn(move |core| async move { Msg::RunFinished(start_run(&core, &request).await.map(Box::new)) });
    }

    pub(super) fn on_run_finished(&mut self, result: Result<Box<Job>, String>) {
        self.run.running = false;
        self.run.confirming = false;
        match result {
            Ok(job) => {
                let job = *job;
                let (id, devices, script) = (job.id.clone(), job.tasks.len(), job.script_name.clone());
                let store = self.jobs.clone();
                self.runtime.spawn(async move { store.add(job).await });
                // Follow it: the new run is the one to look at.
                self.selected_job = Some(id);
                self.run.notice = Some(Notice { kind: NoticeKind::Info, text: format!("Started {script} on {devices} device{}.", if devices == 1 { "" } else { "s" }) });
                self.page = Page::Activity;
                self.pane = super::Pane::Jobs;
                self.focus = Focus::Content;
                self.jobs_version = u64::MAX; // refresh as soon as the store has the job
            }
            Err(e) if is_dry_run_error(&e) => {
                debug_assert_eq!(e, DRY_RUN_ERROR);
                self.run.notice = Some(Notice { kind: NoticeKind::Info, text: "Dry run: nothing was sent. Press d to see the request that was held back.".into() });
            }
            Err(e) => {
                self.note_error(&e);
                self.run.notice = Some(Notice { kind: NoticeKind::Error, text: e });
            }
        }
    }

    /// Starts the form pre-filled from a past run (all its devices, or only the ones that failed).
    pub(super) fn run_again(&mut self, only_failed: bool) {
        use nvoke_core::task_state::{TaskPhase, task_phase};
        let Some(job) = self.current_job().cloned() else { return };
        let org = job.org_id.as_ref().map(|id| Organization { id: id.clone(), name: job.org_name.clone().unwrap_or_default(), typename: String::new() });
        let targets: Vec<Target> = job
            .tasks
            .iter()
            .filter(|t| !only_failed || matches!(task_phase(t), TaskPhase::Failed | TaskPhase::NonZero))
            .filter_map(|t| t.asset_id.as_ref().map(|id| Target::Device(Asset::stub(id, &t.device_name))))
            .collect();
        if targets.is_empty() {
            return;
        }
        self.run.prefill(org, job.script.clone(), targets);
        self.page = Page::Run;
        self.focus = Focus::Content;
        self.resolve_devices();
    }
}

impl RunState {
    fn touch_notice(&mut self) {
        self.notice = None;
    }
}

/// Puts the items already chosen (that match the term) first, followed by the other results.
pub fn lead_with_chosen(items: Vec<PickItem>, chosen: &[PickItem], term: &str) -> Vec<PickItem> {
    let chosen_keys: HashSet<String> = chosen.iter().map(PickItem::key).collect();
    let term = term.to_lowercase();
    // Chosen devices lead a list of devices and chosen tags lead a list of tags, never the other way round.
    let is_tag = items.first().is_some_and(|i| matches!(i, PickItem::Tag(_)));
    let mut lead: Vec<PickItem> = chosen
        .iter()
        .filter(|c| matches!(c, PickItem::Tag(_)) == is_tag || items.is_empty())
        .filter(|c| match c {
            PickItem::Asset(a) => a.name.to_lowercase().contains(&term),
            PickItem::Tag(t) => t.name.to_lowercase().contains(&term),
            _ => false,
        })
        .cloned()
        .collect();
    lead.extend(items.into_iter().filter(|i| !chosen_keys.contains(&i.key())));
    // A chosen item that appears among the results must not be listed twice.
    let mut seen = HashSet::new();
    lead.retain(|i| seen.insert(i.key()));
    lead
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Duration;

    use nvoke_core::{
        Core,
        model::{NameRef, OrgRef, OsRef, ScriptInputDecl, StatusRef, Tag},
    };
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};

    use super::*;
    use crate::app::Event;

    pub fn org(id: &str, name: &str) -> Organization {
        Organization { id: id.into(), name: name.into(), typename: "Customer".into() }
    }

    pub fn decl(name: &str, variable: &str, kind: &str) -> ScriptInputDecl {
        ScriptInputDecl { name: name.into(), variable: variable.into(), kind: kind.into() }
    }

    pub fn script(id: &str, name: &str, inputs: Vec<ScriptInputDecl>) -> Script {
        Script { id: id.into(), name: name.into(), updated_at: None, language: "POWERSHELL".into(), description: None, timeout_seconds: Some(120), inputs: Some(inputs) }
    }

    pub fn device(id: &str, name: &str, status: &str, os: &str) -> Asset {
        Asset {
            id: id.into(),
            name: name.into(),
            customer: Some(NameRef { name: "Acme".into() }),
            site: Some(NameRef { name: "HQ".into() }),
            agent_connection: Some(StatusRef { status: status.into() }),
            operating_system: Some(OsRef { name: Some(os.into()) }),
        }
    }

    pub fn tag(id: &str, name: &str, dynamic: bool, count: i64) -> Tag {
        Tag {
            id: id.into(),
            name: name.into(),
            typename: if dynamic { "DynamicTag" } else { "StaticTag" }.into(),
            asset_count: Some(count),
            updated_at: None,
            description: None,
            organization: Some(OrgRef { id: "o1".into(), name: "Acme".into() }),
            filter: None,
        }
    }

    /// An app on the Run page with everything the pickers need already cached, so no search can reach the network.
    pub fn run_app() -> (tokio::runtime::Runtime, App) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut app = App::new(Core::for_tests("run"), runtime.handle().clone());
        app.screen = super::super::Screen::Main;
        app.settings = Some(nvoke_core::Settings { api_url: "https://x/graphql".into(), has_api_key: true, history_days: 30, encryption_available: true, encryption_error: None });
        app.collections.seed(
            vec![org("o1", "Acme Corp"), org("o2", "Beta Ltd")],
            vec![tag("t1", "PatchGroupTwo", false, 2), tag("t2", "Windows10", true, 40)],
            vec![("o1", vec![script("s1", "BitlockerPreChecker", vec![decl("Count", "n", "INT"), decl("Fast", "f", "BOOLEAN"), decl("Pass", "pw", "PASSWORD")]), script("s2", "Cleanup", vec![])]), ("o2", vec![])],
        );
        app.collections.seed_assets("o1", "", vec![device("a1", "PC-1", "CONNECTED", "Windows 11"), device("a2", "PC-2", "DISCONNECTED", "Windows 10"), device("a9", "LNX-1", "CONNECTED", "Ubuntu 22.04")]);
        app.collections.seed_members("o1", "t1", vec![device("a1", "PC-1", "CONNECTED", "Windows 11"), device("a3", "PC-3", "CONNECTED", "Windows 11")], 2);
        app.go(Page::Run);
        app.focus = Focus::Content;
        (runtime, app)
    }

    pub fn press(app: &mut App, code: KeyCode) {
        let key = KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE };
        app.update(Event::Key(key));
    }

    pub fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    /// Delivers a result to the app, as if an async task had just finished.
    pub fn deliver(app: &mut App, msg: Msg) {
        app.update(Event::Msg(Box::new(msg)));
    }

    pub fn go_to(app: &mut App, page: Page) {
        app.go(page);
    }

    /// Handles every result that arrives until things go quiet.
    pub fn pump(app: &mut App) {
        while let Ok(event) = app.rx.recv_timeout(Duration::from_millis(100)) {
            app.update(event);
        }
    }

    /// Org, script and tag chosen through the keyboard, as a user would.
    fn choose_everything(app: &mut App) {
        press(app, KeyCode::Enter); // open the organization picker
        pump(app);
        press(app, KeyCode::Enter); // Acme Corp; the script picker opens by itself
        pump(app);
        press(app, KeyCode::Enter); // BitlockerPreChecker; the targets picker opens by itself
        pump(app);
        press(app, KeyCode::Tab); // devices -> tags
        pump(app);
        press(app, KeyCode::Enter); // PatchGroupTwo (the first tag listed for Acme)
        press(app, KeyCode::Esc);
        pump(app);
    }

    fn names(app: &App) -> Vec<String> {
        app.run.devices().iter().map(|d| d.name.clone()).collect()
    }

    #[test]
    fn the_form_grows_as_it_is_filled_in() {
        let mut run = RunState::default();
        assert_eq!(run.fields(), [RunField::Org, RunField::Run]);
        run.org = Some(org("o1", "Acme"));
        assert_eq!(run.fields(), [RunField::Org, RunField::Script, RunField::Targets, RunField::Run]);
        run.script = Some(script("s", "S", vec![decl("A", "a", "STRING"), decl("B", "b", "INT")]));
        assert_eq!(run.fields(), [RunField::Org, RunField::Script, RunField::Targets, RunField::Input(0), RunField::Input(1), RunField::Name, RunField::Timeout, RunField::Run]);
    }

    #[test]
    fn the_keyboard_alone_takes_you_from_nothing_to_ready_to_run() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        assert_eq!(app.run.org.as_ref().unwrap().name, "Acme Corp");
        assert_eq!(app.run.script.as_ref().unwrap().name, "BitlockerPreChecker");
        assert_eq!(app.run.targets.len(), 1);
        assert_eq!(names(&app), ["PC-1", "PC-3"]);
        assert_eq!(app.run.blocker(), None);
        assert!(app.picker.is_none());
    }

    #[test]
    fn typing_in_a_picker_narrows_it_and_enter_chooses_what_is_highlighted() {
        let (_rt, mut app) = run_app();
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        assert_eq!(app.picker.as_ref().unwrap().items.len(), 2);
        type_text(&mut app, "be");
        pump(&mut app);
        let keys: Vec<String> = app.picker.as_ref().unwrap().items.iter().map(PickItem::key).collect();
        assert_eq!(keys, ["o:o2"], "only Beta Ltd matches");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.run.org.as_ref().unwrap().name, "Beta Ltd");
    }

    #[test]
    fn a_picker_swallows_every_key_so_typing_never_triggers_a_shortcut() {
        let (_rt, mut app) = run_app();
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        for c in ['q', '?', '1', 'd', 'x', 'j'] {
            press(&mut app, KeyCode::Char(c));
        }
        assert!(!app.quit && !app.show_help && app.page == Page::Run);
        assert_eq!(app.picker.as_ref().unwrap().term(), "q?1dxj");
        press(&mut app, KeyCode::Esc);
        assert!(app.picker.is_none() && app.run.org.is_none(), "esc cancels without choosing");
    }

    #[test]
    fn an_answer_to_an_old_search_is_ignored() {
        let (_rt, mut app) = run_app();
        press(&mut app, KeyCode::Enter);
        let stale = app.picker.as_ref().unwrap().seq;
        pump(&mut app);
        type_text(&mut app, "acme");
        pump(&mut app);
        assert_eq!(app.picker.as_ref().unwrap().items.len(), 1);
        // A late answer to the very first search (all organizations) arrives now.
        app.on_pick_results(stale, Ok(vec![PickItem::Org(org("o2", "Beta Ltd")), PickItem::Org(org("o1", "Acme Corp"))]));
        assert_eq!(app.picker.as_ref().unwrap().items.len(), 1, "the newer search's results stay");
    }

    #[test]
    fn choosing_a_different_organization_clears_what_depended_on_the_old_one() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.field = RunField::Org;
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        press(&mut app, KeyCode::Down); // Beta Ltd
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        assert_eq!(app.run.org.as_ref().unwrap().name, "Beta Ltd");
        assert!(app.run.script.is_none() && app.run.targets.is_empty());
        assert!(matches!(app.run.resolved, Load::NotLoaded));
        assert!(app.run.devices().is_empty());
    }

    #[test]
    fn choosing_the_same_organization_again_keeps_the_rest() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.field = RunField::Org;
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        press(&mut app, KeyCode::Enter); // Acme Corp again
        assert!(app.run.script.is_some() && !app.run.targets.is_empty());
    }

    #[test]
    fn chosen_targets_lead_the_list_and_enter_removes_them() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.field = RunField::Targets;
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        // Devices mode first: pick PC-2 as well.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        assert_eq!(app.run.targets.len(), 2);
        assert_eq!(names(&app), ["PC-1", "PC-2", "PC-3"], "the tag's devices and the picked one, once each");
        // The chosen device now leads the device list, so it can be toggled off again.
        let first = app.picker.as_ref().unwrap().items[0].key();
        assert_eq!(first, "d:a2");
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        assert_eq!(app.run.targets.len(), 1);
        assert_eq!(names(&app), ["PC-1", "PC-3"]);
    }

    #[test]
    fn devices_reached_two_ways_appear_once_and_can_be_left_out() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.pane = RunPane::Form;
        app.run.field = RunField::Targets;
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        // Pick PC-1 directly as well: it is already in the tag.
        for item in app.picker.as_ref().unwrap().items.clone() {
            if item.key() == "d:a1" {
                app.run.targets.push(item.as_target().unwrap());
            }
        }
        press(&mut app, KeyCode::Esc);
        app.resolve_devices();
        pump(&mut app);
        let Load::Ready(list) = &app.run.resolved else { panic!("not resolved") };
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].source(), "Added directly, PatchGroupTwo");

        // Leave PC-3 out from the review table.
        press(&mut app, KeyCode::Esc); // close nothing open -> focus stays in content
        app.focus = Focus::Content;
        app.run.pane = RunPane::Devices;
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(names(&app), ["PC-1"]);
        // Like the desktop app's select-all box: from a partial selection it ticks everything first,
        // and only when everything is ticked does the next press clear it.
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(names(&app), ["PC-1", "PC-3"]);
        press(&mut app, KeyCode::Char('a'));
        assert!(names(&app).is_empty());
        assert_eq!(app.run.blocker(), Some("No devices selected"));
    }

    #[test]
    fn a_device_that_rejoins_the_list_starts_ticked() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.excluded.insert("a3".into());
        assert_eq!(names(&app), ["PC-1"]);
        // The tag is removed, then added again: PC-3 leaves the list and comes back.
        app.run.targets.clear();
        app.resolve_devices();
        app.on_resolved(app.run.resolve_seq, Ok((vec![], false)));
        app.on_resolved(app.run.resolve_seq, Ok((vec![], false)));
        app.run.resolved = Load::Ready(vec![]);
        app.on_resolved(app.run.resolve_seq, Ok((vec![Resolved { asset: device("a3", "PC-3", "CONNECTED", "Windows 11"), via_tags: vec!["PatchGroupTwo".into()], picked: false }], false)));
        assert_eq!(names(&app), ["PC-3"], "it was unticked before it left; it comes back ticked");
    }

    #[test]
    fn nothing_can_be_run_while_the_device_list_is_out_of_date() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.resolved = Load::Loading;
        assert!(app.run.devices().is_empty());
        assert_eq!(app.run.blocker(), Some("Finding devices…"));
        app.run.resolved = Load::Failed("boom".into());
        assert!(app.run.devices().is_empty());
        assert_eq!(app.run.blocker(), Some("The device list failed to load"));
    }

    #[test]
    fn a_late_device_list_for_old_targets_is_ignored() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        let before = names(&app);
        app.on_resolved(app.run.resolve_seq - 1, Ok((vec![], false)));
        assert_eq!(names(&app), before);
    }

    #[test]
    fn text_fields_take_typing_without_triggering_shortcuts_and_enter_finishes() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.field = RunField::Input(0); // Count (INT)
        press(&mut app, KeyCode::Enter);
        assert!(app.run.editing);
        type_text(&mut app, "q?1d");
        assert!(!app.quit && !app.show_help && !app.show_dry_log && app.page == Page::Run);
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Enter);
        assert!(!app.run.editing);
        assert_eq!(app.run.values["n"], FieldValue::Text("q?1".into()));
    }

    #[test]
    fn tab_and_arrows_move_on_while_typing_like_a_form() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.field = RunField::Input(0);
        press(&mut app, KeyCode::Enter);
        type_text(&mut app, "5");
        press(&mut app, KeyCode::Tab);
        assert!(!app.run.editing && app.run.field == RunField::Input(1));
        press(&mut app, KeyCode::Up);
        assert!(app.run.field == RunField::Input(0));
    }

    #[test]
    fn a_boolean_input_toggles_with_space_or_enter() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.field = RunField::Input(1); // Fast (BOOLEAN)
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.run.values["f"], FieldValue::Bool(true));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.run.values["f"], FieldValue::Bool(false));
        assert!(!app.run.editing, "a checkbox is never typed into");
    }

    #[test]
    fn running_is_refused_with_a_reason_until_everything_is_chosen() {
        let (_rt, mut app) = run_app();
        app.run.field = RunField::Run;
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.run.notice.as_ref().unwrap().text, "Choose an organization.");
        app.run.field = RunField::Org;
        choose_everything(&mut app);
        app.run.targets.clear();
        app.resolve_devices();
        assert_eq!(app.run.blocker(), Some("Choose devices or tags to run on"));
    }

    #[test]
    fn invalid_inputs_block_the_run_and_take_focus_to_the_first_problem() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.values.insert("n".into(), FieldValue::Text("abc".into()));
        app.run.field = RunField::Run;
        press(&mut app, KeyCode::Enter);
        assert!(!app.run.confirming && app.run.show_errors);
        assert_eq!(app.run.notice.as_ref().unwrap().text, "Fix the highlighted fields first.");
        assert_eq!(app.run.field, RunField::Input(0), "focus jumps to the offending input");

        app.run.values.insert("n".into(), FieldValue::Text("3".into()));
        app.run.timeout = TextInput::new("9999", false);
        app.run.field = RunField::Run;
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.run.field, RunField::Timeout);
    }

    #[test]
    fn a_run_needs_a_second_enter_and_any_change_cancels_the_first() {
        let (_rt, mut app) = run_app();
        app.core().set_dry_run(true);
        choose_everything(&mut app);
        app.run.field = RunField::Run;
        press(&mut app, KeyCode::Enter);
        assert!(app.run.confirming && !app.run.running, "the first Enter only asks");
        // Changing what would run cancels the question.
        app.run.field = RunField::Input(0);
        press(&mut app, KeyCode::Enter);
        type_text(&mut app, "7");
        assert!(!app.run.confirming);
        // Esc also cancels.
        app.run.editing = false;
        app.run.field = RunField::Run;
        press(&mut app, KeyCode::Enter);
        assert!(app.run.confirming);
        press(&mut app, KeyCode::Esc);
        assert!(!app.run.confirming && app.focus == Focus::Content, "esc cancels the question before it leaves the page");
        assert!(app.core().dry_run_log().is_empty());
    }

    #[test]
    fn in_dry_run_confirming_holds_the_run_back_and_creates_no_job() {
        let (_rt, mut app) = run_app();
        app.core().set_dry_run(true);
        choose_everything(&mut app);
        app.run.values.insert("n".into(), FieldValue::Text("42".into()));
        app.run.values.insert("pw".into(), FieldValue::Text("hunter2-secret".into()));
        app.run.field = RunField::Run;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert!(app.run.running);
        pump(&mut app);
        assert!(!app.run.running && !app.run.confirming);
        let notice = app.run.notice.as_ref().unwrap();
        assert_eq!((notice.kind, notice.text.as_str()), (NoticeKind::Info, "Dry run: nothing was sent. Press d to see the request that was held back."));
        assert!(app.page == Page::Run, "stay on the form: nothing started");
        assert!(app.jobs.jobs().is_empty(), "a held-back run must not become a job");
        let log = app.core().dry_run_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].summary, "Ran BitlockerPreChecker on 2 devices (PatchGroupTwo)");
        assert_eq!(log[0].variables["input"]["assetIds"], serde_json::json!(["a1", "a3"]));
        let text = log[0].variables.to_string();
        assert!(text.contains("\"intValue\":42") && text.contains("[redacted]") && !text.contains("hunter2-secret"), "{text}");
    }

    #[test]
    fn a_failed_run_shows_the_error_and_can_be_tried_again() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.running = true;
        app.on_run_finished(Err("The script run was not created. Device offline".into()));
        let notice = app.run.notice.as_ref().unwrap();
        assert_eq!(notice.kind, NoticeKind::Error);
        assert!(notice.text.contains("Device offline"));
        assert!(!app.run.running && app.page == Page::Run);
    }

    fn started_job() -> Job {
        use nvoke_core::model::Execution;
        let mut task = nvoke_core::task_state::JobTask::new("t-a1", Some("a1"), "PC-1", 0);
        task.watching = false;
        task.task_status = Some("SUCCEEDED".into());
        task.execution = Some(Execution { id: "e".into(), status: "SUCCEEDED".into(), exit_code: None, error_message: None, started_at: None, updated_at: None, duration_milliseconds: None, output: None });
        Job {
            id: "new-job".into(),
            name: "Run".into(),
            script_id: Some("s1".into()),
            script: Some(script("s1", "BitlockerPreChecker", vec![])),
            script_name: "BitlockerPreChecker".into(),
            org_id: Some("o1".into()),
            org_name: Some("Acme Corp".into()),
            target_label: "PC-1".into(),
            created_at: "2026-10-17T12:00:00.000Z".into(),
            timeout_seconds: 120,
            warnings: None,
            migration_project: None,
            tasks: vec![task],
        }
    }

    #[test]
    fn a_started_run_takes_you_to_it_in_activity() {
        let (rt, mut app) = run_app();
        app.on_run_finished(Ok(Box::new(started_job())));
        assert!(app.page == Page::Activity && app.focus == Focus::Content);
        assert_eq!(app.selected_job.as_deref(), Some("new-job"));
        assert!(app.run.notice.as_ref().unwrap().text.starts_with("Started BitlockerPreChecker on 1 device."));
        rt.block_on(async { tokio::time::sleep(Duration::from_millis(100)).await });
        app.sync_jobs();
        assert_eq!(app.job_list.len(), 1);
        assert_eq!(app.selected_job.as_deref(), Some("new-job"));
    }

    fn app_with_past_run() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = run_app();
        let mut ok = started_job();
        ok.tasks.push({
            let mut t = ok.tasks[0].clone();
            t.task_id = "t-a2".into();
            t.asset_id = Some("a2".into());
            t.device_name = "PC-2".into();
            t.execution.as_mut().unwrap().status = "FAILED".into();
            t.task_status = Some("FAILED".into());
            t
        });
        app.jobs.seed(vec![ok]);
        app.go(Page::Activity);
        app.sync_jobs();
        press(&mut app, KeyCode::Right);
        (rt, app)
    }

    #[test]
    fn run_again_refills_the_form_from_a_past_run() {
        let (_rt, mut app) = app_with_past_run();
        press(&mut app, KeyCode::Char('R'));
        pump(&mut app);
        assert!(app.page == Page::Run);
        assert_eq!(app.run.org.as_ref().unwrap().name, "Acme Corp");
        assert_eq!(app.run.script.as_ref().unwrap().name, "BitlockerPreChecker");
        assert_eq!(app.run.targets.len(), 2);
        assert_eq!(names(&app), ["PC-1", "PC-2"]);
        assert_eq!(app.run.field, RunField::Run, "everything is filled in: straight to the button");
    }

    #[test]
    fn re_run_failed_takes_only_the_devices_that_did_not_succeed() {
        let (_rt, mut app) = app_with_past_run();
        press(&mut app, KeyCode::Char('F'));
        pump(&mut app);
        assert_eq!(names(&app), ["PC-2"]);
    }

    #[test]
    fn re_run_failed_does_nothing_when_nothing_failed() {
        let (_rt, mut app) = app_with_past_run();
        let mut all_ok = app.job_list[0].clone();
        all_ok.tasks.truncate(1);
        app.jobs.seed(vec![all_ok]);
        app.sync_jobs();
        press(&mut app, KeyCode::Char('F'));
        assert!(app.page == Page::Activity, "no failed devices: stay put");
    }

    #[test]
    fn leaving_the_run_page_and_coming_back_keeps_the_form() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        press(&mut app, KeyCode::Left);
        assert!(app.focus == Focus::Sidebar);
        app.go(Page::Audit);
        app.go(Page::Run);
        assert_eq!(app.run.org.as_ref().unwrap().name, "Acme Corp");
        assert_eq!(names(&app), ["PC-1", "PC-3"]);
    }

    #[test]
    fn arrows_walk_the_fields_and_stop_at_the_ends() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.field = RunField::Org;
        press(&mut app, KeyCode::Up);
        assert_eq!(app.run.field, RunField::Org);
        for _ in 0..20 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.run.field, RunField::Run);
    }

    #[test]
    fn right_goes_to_the_devices_and_left_comes_back() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        press(&mut app, KeyCode::Right);
        assert!(app.run.pane == RunPane::Devices);
        press(&mut app, KeyCode::Left);
        assert!(app.run.pane == RunPane::Form && app.focus == Focus::Content);
        press(&mut app, KeyCode::Left);
        assert!(app.focus == Focus::Sidebar);
    }

    #[test]
    fn c_clears_the_targets_when_that_field_is_focused() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.field = RunField::Targets;
        press(&mut app, KeyCode::Char('c'));
        assert!(app.run.targets.is_empty());
        pump(&mut app);
        assert!(names(&app).is_empty());
    }

    #[test]
    fn device_searches_wait_for_a_pause_in_typing_but_local_ones_do_not() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.run.field = RunField::Targets;
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        let seq = app.picker.as_ref().unwrap().seq;
        // Seed what is about to be searched, so nothing goes anywhere near the network.
        app.collections.seed_assets("o1", "pc", vec![device("a1", "PC-1", "CONNECTED", "Windows 11")]);
        type_text(&mut app, "pc");
        assert_eq!(app.picker.as_ref().unwrap().seq, seq, "no search per keystroke for devices");
        for _ in 0..3 {
            app.update(Event::Tick);
        }
        assert!(app.picker.as_ref().unwrap().seq > seq, "searches once typing pauses");
        pump(&mut app);
        assert_eq!(app.picker.as_ref().unwrap().items.iter().filter(|i| matches!(i, PickItem::Asset(_))).count(), 1);
    }

    #[test]
    fn lead_with_chosen_never_lists_an_item_twice() {
        let chosen = vec![PickItem::Asset(device("a1", "PC-1", "CONNECTED", "Windows 11"))];
        let results = vec![PickItem::Asset(device("a1", "PC-1", "CONNECTED", "Windows 11")), PickItem::Asset(device("a2", "PC-2", "CONNECTED", "Windows 11"))];
        let keys: Vec<String> = lead_with_chosen(results, &chosen, "").iter().map(PickItem::key).collect();
        assert_eq!(keys, ["d:a1", "d:a2"]);
        let none: Vec<String> = lead_with_chosen(vec![PickItem::Asset(device("a2", "PC-2", "CONNECTED", "Windows 11"))], &chosen, "zzz").iter().map(PickItem::key).collect();
        assert_eq!(none, ["d:a2"], "a chosen device that doesn't match the term is not forced in");
    }

    // ---- an expired key -----------------------------------------------------------

    const DENIED: &str = "Access denied (HTTP 403): your API key is no longer active. It has expired or been revoked: create a new one in the N-able dashboard and replace it in Settings.";

    #[test]
    fn a_refused_search_marks_the_key_as_rejected_but_other_errors_do_not() {
        let (_rt, mut app) = run_app();
        press(&mut app, KeyCode::Enter);
        let seq = app.picker.as_ref().unwrap().seq;
        app.on_pick_results(seq, Err("Request to https://api.n-able.com/graphql failed: timed out".into()));
        assert!(!app.key_rejected, "a timeout says nothing about the key");
        app.on_pick_results(seq, Err(DENIED.into()));
        assert!(app.key_rejected);
        assert_eq!(app.picker.as_ref().unwrap().error.as_deref(), Some(DENIED), "the picker shows the reason in full");
    }

    #[test]
    fn a_refused_device_lookup_or_run_marks_the_key_as_rejected_too() {
        let (_rt, mut app) = run_app();
        choose_everything(&mut app);
        app.on_resolved(app.run.resolve_seq, Err(DENIED.into()));
        assert!(app.key_rejected);
        app.key_rejected = false;
        app.run.running = true;
        app.on_run_finished(Err(DENIED.into()));
        assert!(app.key_rejected);
        assert_eq!(app.run.notice.as_ref().unwrap().kind, NoticeKind::Error);
    }

    #[test]
    fn background_polling_that_is_refused_is_noticed_without_visiting_any_page() {
        let (_rt, mut app) = run_app();
        let mut job = started_job();
        job.tasks[0].watching = true;
        job.tasks[0].last_error = Some(DENIED.into());
        app.jobs.seed(vec![job]);
        app.sync_jobs();
        assert!(app.key_rejected);
    }

    #[test]
    fn a_polling_error_that_is_not_about_the_key_does_not_raise_the_alarm() {
        let (_rt, mut app) = run_app();
        let mut job = started_job();
        job.tasks[0].watching = true;
        job.tasks[0].last_error = Some("The task wasn't found.".into());
        app.jobs.seed(vec![job]);
        app.sync_jobs();
        assert!(!app.key_rejected);
    }

    #[test]
    fn saving_a_new_key_clears_the_warning() {
        let (_rt, mut app) = run_app();
        app.key_rejected = true;
        let settings = nvoke_core::Settings { api_url: "https://x/graphql".into(), has_api_key: true, history_days: 30, encryption_available: true, encryption_error: None };
        app.on_msg(Msg::Saved(Ok(settings)));
        assert!(!app.key_rejected);
    }
}
