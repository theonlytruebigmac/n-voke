//! Creating and editing a tag: one form for both. A static tag chooses its devices; a dynamic tag
//! has a rule, with a live preview of the devices it matches. Drawing is in `tag_form_ui.rs`.

use nvoke_core::{
    DRY_RUN_ERROR, is_dry_run_error,
    collections::AccessCategory,
    model::{Asset, Organization, Tag},
    rules::{Match, RuleCondition, filter_to_rule, rule_to_filter},
    tags::{CreateFailure, EditTag, NewTag, TagKind, TagProblems, check_edit, check_new, create_tag, update_tag},
};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Focus, Msg, run::{Notice, NoticeKind}, tags::TagConfirm};
use crate::{
    input::TextInput,
    picker::{PickKind, Picker, Purpose},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormMode {
    Create,
    Edit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagField {
    Kind,
    Name,
    Description,
    Category,
    Access,
    Devices,
    Match,
    Condition(usize),
    Submit,
}

/// Which part of a rule's condition row has focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CondPart {
    Field,
    Op,
    Value,
}

/// The devices a dynamic rule matches right now.
#[derive(Default)]
pub struct Preview {
    /// What was last asked about (the organization and the filter), to notice when it changes.
    pub key: String,
    /// The tick at which the rule last changed; the question is asked once typing has paused.
    pub due: Option<u64>,
    pub loading: bool,
    pub result: Option<(Vec<Asset>, i64)>,
    pub error: Option<String>,
    pub seq: u64,
}

pub struct TagForm {
    pub mode: FormMode,
    /// The tag being edited.
    pub tag: Option<Tag>,
    pub data: NewTag,
    /// A dynamic tag's stored rule can only be changed if the editor can represent it.
    pub rule_editable: bool,
    pub field: TagField,
    pub part: CondPart,
    /// A text field is being typed into.
    pub editing: bool,
    pub show_errors: bool,
    pub confirming: bool,
    pub busy: bool,
    pub notice: Option<Notice>,
    /// A static tag that was created but couldn't get its devices; a retry only adds them.
    pub created: Option<Tag>,
    pub preview: Preview,
    pub dirty: bool,
}

/// What the Tags page does with a finished save.
#[derive(Debug)]
pub struct Saved {
    pub tag: Tag,
    pub mode: FormMode,
    pub devices: usize,
}

impl TagForm {
    pub fn create(org: Option<Organization>) -> Self {
        Self {
            mode: FormMode::Create,
            tag: None,
            data: NewTag::new(TagKind::Static, org),
            rule_editable: true,
            field: TagField::Name,
            part: CondPart::Field,
            editing: false,
            show_errors: false,
            confirming: false,
            busy: false,
            notice: None,
            created: None,
            preview: Preview::default(),
            dirty: false,
        }
    }

    pub fn edit(tag: Tag) -> Self {
        let org = tag.organization.as_ref().map(|o| Organization { id: o.id.clone(), name: o.name.clone(), typename: String::new() });
        let kind = if tag.is_dynamic() { TagKind::Dynamic } else { TagKind::Static };
        let stored = tag.filter.as_ref().and_then(|f| filter_to_rule(&serde_json::Value::String(f.clone())));
        let mut data = NewTag::new(kind, org);
        data.name = tag.name.clone();
        data.description = tag.description.clone().unwrap_or_default();
        if let Some(rule) = &stored {
            data.rule = rule.clone();
        }
        Self { mode: FormMode::Edit, rule_editable: !tag.is_dynamic() || stored.is_some(), tag: Some(tag), data, ..Self::create(None) }
    }

    pub fn is_dynamic(&self) -> bool {
        self.data.kind == TagKind::Dynamic
    }

    /// The fields that exist right now, top to bottom.
    pub fn fields(&self) -> Vec<TagField> {
        let mut fields = vec![];
        if self.mode == FormMode::Create {
            fields.push(TagField::Kind);
        }
        fields.extend([TagField::Name, TagField::Description]);
        if self.mode == FormMode::Create {
            fields.extend([TagField::Category, TagField::Access]);
        }
        if self.is_dynamic() {
            if self.rule_editable {
                fields.push(TagField::Match);
                fields.extend((0..self.data.rule.conditions.len()).map(TagField::Condition));
            }
        } else if self.mode == FormMode::Create {
            fields.push(TagField::Devices);
        }
        fields.push(TagField::Submit);
        fields
    }

    pub fn problems(&self) -> TagProblems {
        match (&self.mode, &self.tag) {
            (FormMode::Edit, Some(tag)) => check_edit(&EditTag { tag: tag.clone(), name: self.data.name.clone(), description: self.data.description.clone(), rule: self.rule_editable.then(|| self.data.rule.clone()) }),
            _ => check_new(&self.data),
        }
    }

    fn step(&mut self, delta: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.field).unwrap_or(0) as isize;
        self.field = fields[(at + delta).clamp(0, fields.len() as isize - 1) as usize];
    }

    /// Anything the user changes cancels a pending confirmation and counts as unsaved work.
    fn changed(&mut self) {
        self.confirming = false;
        self.dirty = true;
        self.notice = None;
    }

    fn tidy_focus(&mut self) {
        let fields = self.fields();
        if !fields.contains(&self.field) {
            self.field = *fields.last().unwrap();
        }
    }

    /// The first field with a problem, to take focus to.
    pub fn first_problem(&self) -> Option<TagField> {
        let p = self.problems();
        self.fields().into_iter().find(|f| match f {
            TagField::Name => p.name.is_some(),
            TagField::Access => p.access.is_some(),
            TagField::Match | TagField::Condition(_) => p.rule.is_some(),
            _ => false,
        })
    }

    /// The organization the rule is previewed in.
    pub fn org_id(&self) -> Option<String> {
        self.data.org.as_ref().map(|o| o.id.clone())
    }

    /// The text of a one-line field, for editing.
    fn text_mut(&mut self) -> Option<&mut String> {
        match self.field {
            TagField::Name => Some(&mut self.data.name),
            TagField::Description => Some(&mut self.data.description),
            TagField::Category => Some(&mut self.data.category),
            TagField::Condition(i) if self.part == CondPart::Value => self.data.rule.conditions.get_mut(i).map(|c| &mut c.value),
            _ => None,
        }
    }

    /// Asks again about the matching devices if the rule or organization changed.
    pub fn touch_preview(&mut self, tick: u64) {
        if !self.is_dynamic() || !self.rule_editable {
            return;
        }
        let key = format!("{}|{}", self.org_id().unwrap_or_default(), rule_to_filter(&self.data.rule));
        if key == self.preview.key {
            return;
        }
        self.preview.key = key;
        self.preview.result = None;
        self.preview.error = None;
        self.preview.loading = false;
        self.preview.due = Some(tick);
    }
}

/// Longest tag name the API accepts.
const MAX_NAME: usize = 64;

impl App {
    // ---- opening and closing ----------------------------------------------------------------

    pub(super) fn open_new_tag_form(&mut self) {
        self.tag_form = Some(TagForm::create(None));
    }

    pub(super) fn open_edit_tag_form(&mut self) {
        // The full record carries a dynamic tag's rule, so wait for it.
        match &self.tags.detail {
            super::Load::Ready(tag) => self.tag_form = Some(TagForm::edit(tag.clone())),
            _ => self.tags.notice = Some(Notice { kind: NoticeKind::Info, text: "Wait for the tag to finish loading, then press e.".into() }),
        }
    }

    pub(super) fn close_tag_form(&mut self) {
        self.tag_form = None;
        self.picker = None;
    }

    // ---- keys ---------------------------------------------------------------------------------

    pub(super) fn on_tag_form_key(&mut self, key: KeyEvent) {
        let Some(form) = self.tag_form.as_mut() else { return };
        if form.busy {
            return;
        }
        if form.editing {
            return self.on_tag_form_edit_key(key);
        }
        let field = form.field;
        match key.code {
            KeyCode::Esc | KeyCode::Left if !matches!(field, TagField::Kind | TagField::Match | TagField::Condition(_)) || key.code == KeyCode::Esc => self.leave_tag_form(),
            KeyCode::Up | KeyCode::Char('k') => {
                form.notice = None;
                form.step(-1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                form.notice = None;
                form.step(1);
            }
            _ => self.on_tag_form_field_key(key),
        }
    }

    /// Esc: leave, asking first if there is unsaved work.
    fn leave_tag_form(&mut self) {
        let Some(form) = self.tag_form.as_ref() else { return };
        if form.confirming {
            self.tag_form.as_mut().unwrap().confirming = false;
        } else if form.dirty {
            self.confirm = Some(super::Confirm::Tag(TagConfirm::DiscardForm { creating: form.mode == FormMode::Create }));
        } else {
            self.close_tag_form();
        }
    }

    fn on_tag_form_field_key(&mut self, key: KeyEvent) {
        let tick = self.tick;
        let Some(form) = self.tag_form.as_mut() else { return };
        let enter = matches!(key.code, KeyCode::Enter);
        match form.field {
            TagField::Kind => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right) {
                    form.data.kind = if form.is_dynamic() { TagKind::Static } else { TagKind::Dynamic };
                    form.changed();
                    form.touch_preview(tick);
                    form.tidy_focus();
                }
            }
            TagField::Name | TagField::Description | TagField::Category => {
                if enter {
                    form.editing = true;
                }
            }
            TagField::Access => {
                if enter {
                    self.open_access_picker();
                }
            }
            TagField::Devices => match key.code {
                KeyCode::Enter => self.open_form_devices_picker(),
                KeyCode::Char('c') => {
                    form.data.devices.clear();
                    form.changed();
                }
                _ => {}
            },
            TagField::Match => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right) {
                    form.data.rule.r#match = if form.data.rule.r#match == Match::All { Match::Any } else { Match::All };
                    form.changed();
                    form.touch_preview(tick);
                }
            }
            TagField::Condition(i) => self.on_condition_key(i, key),
            TagField::Submit => {
                if enter {
                    self.submit_tag_form();
                }
            }
        }
    }

    fn on_condition_key(&mut self, i: usize, key: KeyEvent) {
        let tick = self.tick;
        let Some(form) = self.tag_form.as_mut() else { return };
        match key.code {
            KeyCode::Left => {
                form.part = match form.part {
                    CondPart::Value => CondPart::Op,
                    _ => CondPart::Field,
                }
            }
            KeyCode::Right | KeyCode::Tab => {
                form.part = match form.part {
                    CondPart::Field => CondPart::Op,
                    _ => CondPart::Value,
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match form.part {
                    CondPart::Value => {
                        if key.code == KeyCode::Enter {
                            form.editing = true;
                        }
                        return;
                    }
                    CondPart::Field => {
                        if let Some(c) = form.data.rule.conditions.get_mut(i) {
                            let next = c.field.next();
                            c.set_field(next);
                        }
                    }
                    CondPart::Op => {
                        if let Some(c) = form.data.rule.conditions.get_mut(i) {
                            c.next_op();
                        }
                    }
                }
                form.changed();
                form.touch_preview(tick);
            }
            KeyCode::Char('+') => {
                form.data.rule.conditions.insert(i + 1, RuleCondition::blank());
                form.field = TagField::Condition(i + 1);
                form.part = CondPart::Field;
                form.changed();
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if form.data.rule.conditions.len() > 1 {
                    form.data.rule.conditions.remove(i);
                    form.field = TagField::Condition(i.min(form.data.rule.conditions.len() - 1));
                    form.changed();
                    form.touch_preview(tick);
                } else {
                    form.notice = Some(Notice { kind: NoticeKind::Info, text: "A rule needs at least one condition row. Clear its value to ignore it.".into() });
                }
            }
            _ => {}
        }
    }

    fn on_tag_form_edit_key(&mut self, key: KeyEvent) {
        let tick = self.tick;
        let Some(form) = self.tag_form.as_mut() else { return };
        match key.code {
            KeyCode::Enter | KeyCode::Esc => form.editing = false,
            KeyCode::Tab | KeyCode::Down => {
                form.editing = false;
                form.step(1);
            }
            KeyCode::BackTab | KeyCode::Up => {
                form.editing = false;
                form.step(-1);
            }
            _ => {
                let is_name = form.field == TagField::Name;
                let Some(text) = form.text_mut() else { return };
                let mut input = TextInput::new(text, false);
                if input.handle_key(key) {
                    *text = if is_name { input.value.chars().take(MAX_NAME).collect() } else { input.value };
                    form.changed();
                    form.touch_preview(tick);
                }
            }
        }
    }

    // ---- pickers ------------------------------------------------------------------------------

    fn open_access_picker(&mut self) {
        let Some(form) = self.tag_form.as_ref() else { return };
        let mut picker = Picker::new(PickKind::Org, Purpose::TagAccess, "Where can this tag be used?", None);
        picker.category = Some(form.data.org.as_ref().map_or(AccessCategory::Organization, AccessCategory::of));
        self.open_picker_with(picker);
    }

    fn open_form_devices_picker(&mut self) {
        let Some(org_id) = self.tag_form.as_ref().and_then(TagForm::org_id) else {
            if let Some(form) = self.tag_form.as_mut() {
                form.notice = Some(Notice { kind: NoticeKind::Error, text: "Choose where the tag can be used first: devices are looked up there.".into() });
            }
            return;
        };
        let picker = Picker::new(PickKind::Devices, Purpose::TagFormDevices, "Devices for the new tag", Some(org_id));
        self.open_picker_with(picker);
    }

    /// The picker chose an organization for the form.
    pub(super) fn choose_form_org(&mut self, org: Organization) {
        let tick = self.tick;
        self.picker = None;
        if let Some(form) = self.tag_form.as_mut() {
            let changed = form.data.org.as_ref().map(|o| &o.id) != Some(&org.id);
            form.data.org = Some(org);
            if changed {
                // Devices chosen in the old organization don't belong to the new one.
                form.data.devices.clear();
                form.changed();
                form.touch_preview(tick);
            }
        }
    }

    /// The picker toggled a device for a new static tag.
    pub(super) fn toggle_form_device(&mut self, asset: Asset) {
        if let Some(form) = self.tag_form.as_mut() {
            match form.data.devices.iter().position(|d| d.id == asset.id) {
                Some(at) => {
                    form.data.devices.remove(at);
                }
                None => form.data.devices.push(asset),
            }
            form.changed();
        }
    }

    // ---- previewing a rule ---------------------------------------------------------------------

    /// Called every tick: asks which devices the rule matches once it has stopped changing.
    pub(super) fn tick_tag_form(&mut self) {
        let tick = self.tick;
        let Some(form) = self.tag_form.as_mut() else { return };
        let due = form.preview.due.is_some_and(|at| tick.saturating_sub(at) >= 4);
        if !due {
            return;
        }
        form.preview.due = None;
        let filter = rule_to_filter(&form.data.rule);
        let has_conditions = nvoke_core::rules::active_condition_count(&form.data.rule) > 0;
        let Some(org_id) = form.org_id().filter(|_| has_conditions) else { return };
        form.preview.loading = true;
        form.preview.seq += 1;
        let (seq, key) = (form.preview.seq, form.preview.key.clone());
        let core = self.core.clone();
        self.spawn(move |_| async move {
            let result = core.preview_assets(&org_id, &filter, 50).await.map(|d| (d.assets, d.total_count));
            Msg::TagPreview { seq, key, result }
        });
    }

    pub(super) fn on_tag_preview(&mut self, seq: u64, key: String, result: Result<(Vec<Asset>, i64), String>) {
        if let Err(e) = &result {
            self.note_error(e);
        }
        let Some(form) = self.tag_form.as_mut() else { return };
        if seq != form.preview.seq || key != form.preview.key {
            return; // the rule has changed since this was asked
        }
        form.preview.loading = false;
        match result {
            Ok(found) => form.preview.result = Some(found),
            Err(e) => form.preview.error = Some(e),
        }
    }

    // ---- saving ----------------------------------------------------------------------------------

    /// Enter on the button: check, then ask for a second Enter, then send.
    fn submit_tag_form(&mut self) {
        let Some(form) = self.tag_form.as_mut() else { return };
        form.notice = None;
        form.show_errors = true;
        if form.problems().any() {
            form.notice = Some(Notice { kind: NoticeKind::Error, text: "Fix the highlighted fields first.".into() });
            if let Some(f) = form.first_problem() {
                form.field = f;
            }
            return;
        }
        if !form.confirming {
            form.confirming = true;
            return;
        }
        form.busy = true;
        form.confirming = false;
        let core = self.core.clone();
        let (mode, data, tag, created, rule_editable) = (form.mode, form.data.clone(), form.tag.clone(), form.created.clone(), form.rule_editable);
        self.spawn(move |_| async move {
            let result = match (mode, tag) {
                (FormMode::Edit, Some(tag)) => {
                    let edit = EditTag { tag, name: data.name.clone(), description: data.description.clone(), rule: rule_editable.then(|| data.rule.clone()) };
                    update_tag(&*core, &edit).await.map(|tag| Saved { tag, mode, devices: 0 }).map_err(|message| CreateFailure { message, created: None })
                }
                _ => create_tag(&*core, &data, created).await.map(|tag| Saved { devices: data.devices.len(), tag, mode }),
            };
            Msg::TagFormDone(result)
        });
    }

    pub(super) fn on_tag_form_done(&mut self, result: Result<Saved, CreateFailure>) {
        match result {
            Ok(Saved { tag, mode, devices }) => {
                let id = tag.id.clone();
                let verb = if mode == FormMode::Create { "Created" } else { "Saved" };
                let text = format!("{verb} tag {}.", tag.name);
                self.collections.cache_tag(tag);
                if devices > 0 {
                    let collections = self.collections.clone();
                    let id = id.clone();
                    self.runtime.spawn(async move { collections.refresh_membership(&id).await });
                }
                self.close_tag_form();
                self.tags.selected = Some(id.clone());
                self.tags.term.value.clear();
                self.load_tags();
                self.set_selected_tag(Some(id));
                self.tags.notice = Some(Notice { kind: NoticeKind::Info, text });
                self.focus = Focus::Content;
            }
            Err(failure) => {
                self.note_error(&failure.message);
                let Some(form) = self.tag_form.as_mut() else { return };
                form.busy = false;
                form.confirming = false;
                if failure.created.is_some() {
                    form.created = failure.created;
                }
                form.notice = Some(if is_dry_run_error(&failure.message) {
                    debug_assert_eq!(failure.message, DRY_RUN_ERROR);
                    Notice { kind: NoticeKind::Info, text: "Dry run: nothing was sent. Press d to see the request that was held back.".into() }
                } else if form.created.is_some() {
                    Notice { kind: NoticeKind::Error, text: format!("{} Press enter on the button to try adding them again.", failure.message) }
                } else {
                    Notice { kind: NoticeKind::Error, text: failure.message }
                });
            }
        }
    }
}

/// A one-line summary of what the button will do, for the confirm step.
pub fn summary(form: &TagForm) -> String {
    let org = form.data.org.as_ref().map_or("?".to_string(), |o| o.name.clone());
    match (form.mode, form.data.kind) {
        (FormMode::Edit, _) => format!("save changes to {}", form.data.name.trim()),
        (_, TagKind::Static) => {
            let n = form.data.devices.len();
            let with = if n == 0 { String::new() } else { format!(" with {n} device{}", if n == 1 { "" } else { "s" }) };
            format!("create static tag {} in {org}{with}", form.data.name.trim())
        }
        (_, TagKind::Dynamic) => format!("create dynamic tag {} in {org}", form.data.name.trim()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use nvoke_core::{
        model::Organization,
        rules::{RuleField, RuleOp},
    };
    use ratatui::crossterm::event::KeyCode;

    use super::*;
    use crate::{
        app::{
            Event, Page,
            run::tests::{device, pump, press, tag, type_text},
            tags::tests::{settle, tags_app},
        },
        picker::PickItem,
    };

    fn org(id: &str, name: &str, kind: &str) -> Organization {
        Organization { id: id.into(), name: name.into(), typename: kind.into() }
    }

    /// The Tags page with organizations of every category cached, and a new-tag form open.
    pub fn form_app() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = tags_app();
        app.collections.seed(
            vec![org("m1", "MSP Inc", "ServiceOrganization"), org("o1", "Acme Corp", "Customer"), org("s1", "Acme HQ", "Site")],
            vec![tag("t1", "Servers", false, 2)],
            vec![],
        );
        app.collections.seed_assets("o1", "", vec![device("a1", "PC-1", "CONNECTED", "Windows 11"), device("a2", "PC-2", "DISCONNECTED", "Windows 10")]);
        settle(&mut app);
        press(&mut app, KeyCode::Char('n'));
        (rt, app)
    }

    pub(crate) fn form(app: &App) -> &TagForm {
        app.tag_form.as_ref().expect("the form is open")
    }

    pub(crate) fn form_mut(app: &mut App) -> &mut TagForm {
        app.tag_form.as_mut().expect("the form is open")
    }

    /// The sequence number and key of the newest preview request.
    pub(crate) fn preview_ids(app: &App) -> (u64, String) {
        (form(app).preview.seq, form(app).preview.key.clone())
    }

    /// Delivers an answer to a preview request.
    pub(crate) fn deliver_preview(app: &mut App, seq: u64, key: &str, result: Result<(Vec<Asset>, i64), String>) {
        app.on_tag_preview(seq, key.to_string(), result);
    }

    pub(crate) fn type_into(app: &mut App, field: TagField, text: &str) {
        form_mut(app).field = field;
        press(app, KeyCode::Enter);
        type_text(app, text);
        press(app, KeyCode::Enter);
    }

    pub(crate) fn choose_org(app: &mut App, name: &str) {
        form_mut(app).field = TagField::Access;
        press(app, KeyCode::Enter);
        pump(app);
        // Find the organization in whichever category it lives.
        for _ in 0..3 {
            if let Some(at) = app.picker.as_ref().unwrap().items.iter().position(|i| matches!(i, PickItem::Org(o) if o.name == name)) {
                app.picker.as_mut().unwrap().table.select(Some(at));
                press(app, KeyCode::Enter);
                return;
            }
            press(app, KeyCode::Tab);
            pump(app);
        }
        panic!("{name} not found in any category");
    }

    #[test]
    fn n_opens_a_blank_static_form_and_the_fields_follow_the_type() {
        let (_rt, mut app) = form_app();
        let f = form(&app);
        assert!(f.mode == FormMode::Create && !f.is_dynamic() && f.field == TagField::Name);
        assert_eq!(f.fields(), [TagField::Kind, TagField::Name, TagField::Description, TagField::Category, TagField::Access, TagField::Devices, TagField::Submit]);
        form_mut(&mut app).field = TagField::Kind;
        press(&mut app, KeyCode::Char(' '));
        assert!(form(&app).is_dynamic());
        assert_eq!(form(&app).fields(), [TagField::Kind, TagField::Name, TagField::Description, TagField::Category, TagField::Access, TagField::Match, TagField::Condition(0), TagField::Submit]);
        press(&mut app, KeyCode::Right);
        assert!(!form(&app).is_dynamic(), "left and right switch it too");
    }

    #[test]
    fn typing_in_a_text_field_swallows_shortcuts_and_a_name_stops_at_64_characters() {
        let (_rt, mut app) = form_app();
        form_mut(&mut app).field = TagField::Name;
        press(&mut app, KeyCode::Enter);
        assert!(form(&app).editing);
        type_text(&mut app, "q?1dx");
        assert!(!app.quit && !app.show_help && !app.show_dry_log && app.page == Page::Tags);
        type_text(&mut app, &"a".repeat(100));
        assert_eq!(form(&app).data.name.chars().count(), 64);
        press(&mut app, KeyCode::Enter);
        assert!(!form(&app).editing && form(&app).dirty);
    }

    #[test]
    fn q_does_not_quit_while_a_form_is_open_but_esc_leaves_it() {
        let (_rt, mut app) = form_app();
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.quit && app.tag_form.is_some(), "an accidental q must not throw the form away");
        press(&mut app, KeyCode::Esc);
        assert!(app.tag_form.is_none(), "an untouched form closes at once");
    }

    #[test]
    fn leaving_a_form_with_unsaved_work_asks_first() {
        let (_rt, mut app) = form_app();
        type_into(&mut app, TagField::Name, "Servers");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Discard this new tag? Nothing has been created yet.");
        press(&mut app, KeyCode::Char('n'));
        assert!(app.tag_form.is_some(), "anything but y keeps it");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('y'));
        assert!(app.tag_form.is_none() && app.confirm.is_none());
    }

    #[test]
    fn the_access_picker_lists_one_category_at_a_time_and_tab_changes_it() {
        let (_rt, mut app) = form_app();
        form_mut(&mut app).field = TagField::Access;
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        let names = |app: &App| app.picker.as_ref().unwrap().items.iter().map(|i| match i { PickItem::Org(o) => o.name.clone(), _ => String::new() }).collect::<Vec<_>>();
        assert_eq!(names(&app), ["MSP Inc"], "organizations first");
        press(&mut app, KeyCode::Tab);
        pump(&mut app);
        assert_eq!(names(&app), ["Acme Corp"], "then customers");
        press(&mut app, KeyCode::Tab);
        pump(&mut app);
        assert_eq!(names(&app), ["Acme HQ"], "then sites");
        press(&mut app, KeyCode::Enter);
        assert_eq!(form(&app).data.org.as_ref().unwrap().name, "Acme HQ");
        assert!(app.picker.is_none());
    }

    #[test]
    fn reopening_the_access_picker_starts_in_the_chosen_organizations_category() {
        let (_rt, mut app) = form_app();
        choose_org(&mut app, "Acme Corp");
        form_mut(&mut app).field = TagField::Access;
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        assert_eq!(app.picker.as_ref().unwrap().category, Some(AccessCategory::Customer));
        assert!(app.picker_chosen().contains("o:o1"), "the current choice is ticked");
    }

    #[test]
    fn devices_for_a_new_static_tag_need_an_organization_first() {
        let (_rt, mut app) = form_app();
        form_mut(&mut app).field = TagField::Devices;
        press(&mut app, KeyCode::Enter);
        assert!(app.picker.is_none());
        assert!(form(&app).notice.as_ref().unwrap().text.contains("Choose where the tag can be used first"));
    }

    #[test]
    fn devices_are_picked_in_the_chosen_organization_and_cleared_when_it_changes() {
        let (_rt, mut app) = form_app();
        choose_org(&mut app, "Acme Corp");
        form_mut(&mut app).field = TagField::Devices;
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        press(&mut app, KeyCode::Enter); // PC-1
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // PC-2
        assert_eq!(form(&app).data.devices.len(), 2);
        press(&mut app, KeyCode::Esc);
        assert!(app.picker.is_none() && app.confirm.is_none(), "closing the device picker only returns to the form");
        press(&mut app, KeyCode::Char('c'));
        assert!(form(&app).data.devices.is_empty(), "c clears them");
        // Pick one again, then move the tag to another place: its devices no longer apply.
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        assert_eq!(form(&app).data.devices.len(), 1);
        choose_org(&mut app, "Acme HQ");
        assert!(form(&app).data.devices.is_empty());
    }

    #[test]
    fn the_rule_editor_changes_the_match_mode_field_operator_and_value() {
        let (_rt, mut app) = form_app();
        form_mut(&mut app).field = TagField::Kind;
        press(&mut app, KeyCode::Char(' '));
        form_mut(&mut app).field = TagField::Match;
        press(&mut app, KeyCode::Enter);
        assert_eq!(form(&app).data.rule.r#match, Match::Any);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(form(&app).data.rule.r#match, Match::All);

        form_mut(&mut app).field = TagField::Condition(0);
        press(&mut app, KeyCode::Enter); // on the field part: next field
        assert_eq!(form(&app).data.rule.conditions[0].field, RuleField::Os);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter); // on the operator part: next operator
        assert_eq!(form(&app).data.rule.conditions[0].op, RuleOp::Equals);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter); // on the value part: start typing
        assert!(form(&app).editing);
        type_text(&mut app, "Windows 10");
        press(&mut app, KeyCode::Enter);
        let c = &form(&app).data.rule.conditions[0];
        assert_eq!((c.field, c.op, c.value.as_str()), (RuleField::Os, RuleOp::Equals, "Windows 10"));
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert_eq!(form(&app).data.rule.conditions[0].field, RuleField::Tag, "a tag can only be matched exactly, so the operator follows");
        assert_eq!(form(&app).data.rule.conditions[0].op, RuleOp::Equals);
    }

    #[test]
    fn conditions_can_be_added_and_removed_but_never_the_last() {
        let (_rt, mut app) = form_app();
        form_mut(&mut app).field = TagField::Kind;
        press(&mut app, KeyCode::Char(' '));
        form_mut(&mut app).field = TagField::Condition(0);
        press(&mut app, KeyCode::Char('+'));
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(form(&app).data.rule.conditions.len(), 3);
        assert_eq!(form(&app).field, TagField::Condition(2), "the new row takes focus");
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(form(&app).data.rule.conditions.len(), 1);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(form(&app).data.rule.conditions.len(), 1);
        assert!(form(&app).notice.as_ref().unwrap().text.contains("at least one condition row"));
    }

    #[test]
    fn typing_a_rules_value_never_triggers_a_shortcut() {
        let (_rt, mut app) = form_app();
        form_mut(&mut app).field = TagField::Kind;
        press(&mut app, KeyCode::Char(' '));
        form_mut(&mut app).field = TagField::Condition(0);
        form_mut(&mut app).part = CondPart::Value;
        press(&mut app, KeyCode::Enter);
        type_text(&mut app, "x+q?");
        assert_eq!(form(&app).data.rule.conditions[0].value, "x+q?");
        assert_eq!(form(&app).data.rule.conditions.len(), 1, "a + typed into a value is text, not 'add a row'");
        assert!(!app.quit);
    }

    #[test]
    fn the_preview_asks_once_typing_has_paused_and_ignores_answers_to_old_rules() {
        let (_rt, mut app) = form_app();
        choose_org(&mut app, "Acme Corp");
        form_mut(&mut app).field = TagField::Kind;
        press(&mut app, KeyCode::Char(' '));
        form_mut(&mut app).field = TagField::Condition(0);
        form_mut(&mut app).part = CondPart::Value;
        press(&mut app, KeyCode::Enter);
        type_text(&mut app, "PC");
        let seq = form(&app).preview.seq;
        for _ in 0..3 {
            app.update(Event::Tick);
        }
        assert_eq!(form(&app).preview.seq, seq, "still within the pause: nothing asked");
        type_text(&mut app, "-"); // the rule changes again: the pause starts over
        for _ in 0..3 {
            app.update(Event::Tick);
        }
        assert_eq!(form(&app).preview.seq, seq);
        app.update(Event::Tick);
        assert_eq!(form(&app).preview.seq, seq + 1, "asked once, for the rule as it now stands");
        assert!(form(&app).preview.loading);

        let key = form(&app).preview.key.clone();
        // A late answer to the earlier rule is ignored; the right one is shown.
        app.on_tag_preview(seq, "stale".into(), Ok((vec![device("x", "STALE", "CONNECTED", "x")], 1)));
        assert!(form(&app).preview.result.is_none());
        app.on_tag_preview(seq + 1, key, Ok((vec![device("a1", "PC-1", "CONNECTED", "Windows 11")], 9)));
        let (devices, total) = form(&app).preview.result.clone().unwrap();
        assert_eq!((devices.len(), total), (1, 9));
        assert!(!form(&app).preview.loading);
    }

    #[test]
    fn nothing_is_asked_for_a_rule_with_no_conditions_or_no_organization() {
        let (_rt, mut app) = form_app();
        form_mut(&mut app).field = TagField::Kind;
        press(&mut app, KeyCode::Char(' '));
        let seq = form(&app).preview.seq;
        for _ in 0..6 {
            app.update(Event::Tick);
        }
        assert_eq!(form(&app).preview.seq, seq, "no organization and a blank condition: nothing to ask");
    }

    #[test]
    fn saving_a_form_with_problems_names_them_and_takes_you_to_the_first() {
        let (_rt, mut app) = form_app();
        form_mut(&mut app).field = TagField::Submit;
        press(&mut app, KeyCode::Enter);
        let f = form(&app);
        assert!(f.show_errors && !f.confirming);
        assert_eq!(f.notice.as_ref().unwrap().text, "Fix the highlighted fields first.");
        assert_eq!(f.field, TagField::Name);
        let p = f.problems();
        assert_eq!((p.name, p.access), (Some("Enter a name."), Some("Choose where the tag can be used")));
    }

    fn ready_static(app: &mut App) {
        type_into(app, TagField::Name, "Servers");
        choose_org(app, "Acme Corp");
        form_mut(app).field = TagField::Devices;
        press(app, KeyCode::Enter);
        pump(app);
        press(app, KeyCode::Enter);
        press(app, KeyCode::Down);
        press(app, KeyCode::Enter);
        press(app, KeyCode::Esc);
        form_mut(app).field = TagField::Submit;
    }

    #[test]
    fn creating_needs_a_second_enter_which_any_change_cancels() {
        let (_rt, mut app) = form_app();
        app.core().set_dry_run(true);
        ready_static(&mut app);
        assert!(!form(&app).problems().any());
        press(&mut app, KeyCode::Enter);
        assert!(form(&app).confirming && !form(&app).busy);
        assert_eq!(summary(form(&app)), "create static tag Servers in Acme Corp with 2 devices");
        type_into(&mut app, TagField::Description, "x");
        assert!(!form(&app).confirming, "a change cancels the question");
        form_mut(&mut app).field = TagField::Submit;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        assert!(!form(&app).confirming && app.tag_form.is_some(), "esc cancels the question before it leaves");
        assert!(app.core().dry_run_log().is_empty());
    }

    #[test]
    fn in_dry_run_a_confirmed_creation_is_held_back_and_the_form_stays_open() {
        let (_rt, mut app) = form_app();
        app.core().set_dry_run(true);
        ready_static(&mut app);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert!(form(&app).busy);
        pump(&mut app);
        let f = form(&app);
        assert!(!f.busy && !f.confirming);
        assert_eq!(f.notice.as_ref().unwrap().text, "Dry run: nothing was sent. Press d to see the request that was held back.");
        let log = app.core().dry_run_log();
        assert_eq!(log.len(), 1, "the devices are not even attempted for a tag that wasn't made");
        assert_eq!(log[0].summary, "Created static tag Servers");
        assert_eq!(log[0].variables["input"]["organizationId"], "o1");
    }

    #[test]
    fn a_saved_tag_closes_the_form_and_is_selected_in_the_list() {
        let (_rt, mut app) = form_app();
        let made = tag("new-1", "Brand_New", false, 0);
        app.on_tag_form_done(Ok(Saved { tag: made, mode: FormMode::Create, devices: 0 }));
        assert!(app.tag_form.is_none());
        pump(&mut app);
        assert_eq!(app.tags.selected.as_deref(), Some("new-1"));
        assert!(app.tags.tags().iter().any(|t| t.name == "Brand_New"), "it is in the list straight away");
        assert_eq!(app.tags.notice.as_ref().unwrap().text, "Created tag Brand_New.");
    }

    #[test]
    fn if_adding_devices_fails_a_retry_only_adds_them_and_never_makes_a_second_tag() {
        let (_rt, mut app) = form_app();
        app.core().set_dry_run(true);
        ready_static(&mut app);
        let existing = tag("made-1", "Servers", false, 0);
        // The first attempt created the tag, but adding the devices failed.
        app.on_tag_form_done(Err(CreateFailure { message: "Created Servers, but couldn't add the devices: boom".into(), created: Some(existing) }));
        let f = form(&app);
        assert!(f.created.is_some() && !f.busy);
        assert!(f.notice.as_ref().unwrap().text.contains("boom") && f.notice.as_ref().unwrap().text.contains("try adding them again"));
        // Retrying (here held back by dry run) must go straight to adding the devices.
        form_mut(&mut app).field = TagField::Submit;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        let log = app.core().dry_run_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].summary, "Added 2 devices to Servers", "no second 'Created static tag'");
        assert_eq!(log[0].variables["input"]["tagIds"], serde_json::json!(["made-1"]));
    }

    #[test]
    fn a_refused_key_while_saving_is_flagged() {
        let (_rt, mut app) = form_app();
        app.on_tag_form_done(Err(CreateFailure { message: "Access denied (HTTP 403): your API key is no longer active.".into(), created: None }));
        assert!(app.key_rejected);
        assert_eq!(form(&app).notice.as_ref().unwrap().kind, NoticeKind::Error);
    }

    // ---- editing -------------------------------------------------------------------------

    fn open_edit(app: &mut App) {
        settle(app);
        press(app, KeyCode::Char('e'));
    }

    #[test]
    fn e_edits_the_selected_tag_with_only_the_fields_that_can_change() {
        let (_rt, mut app) = tags_app();
        open_edit(&mut app);
        let f = form(&app);
        assert!(f.mode == FormMode::Edit);
        assert_eq!((f.data.name.as_str(), f.data.description.as_str()), ("Servers", "Production servers"));
        assert_eq!(f.fields(), [TagField::Name, TagField::Description, TagField::Submit], "a static tag has no rule, and its place and type are fixed");
    }

    #[test]
    fn editing_a_dynamic_tag_shows_its_rule_for_editing() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        press(&mut app, KeyCode::Down);
        settle(&mut app);
        press(&mut app, KeyCode::Char('e'));
        let f = form(&app);
        assert!(f.rule_editable);
        assert_eq!(f.data.rule.conditions[0].value, "Windows 10");
        assert_eq!(f.fields(), [TagField::Name, TagField::Description, TagField::Match, TagField::Condition(0), TagField::Submit]);
    }

    #[test]
    fn a_rule_the_editor_cannot_show_is_kept_and_not_offered_for_editing() {
        let (_rt, mut app) = tags_app();
        let mut odd = tag("t2", "Windows10", true, 3);
        odd.filter = Some(r#"{"and":[{"cpu":{"cores":{"gte":4}}}]}"#.into());
        app.collections.cache_tag(odd);
        settle(&mut app);
        press(&mut app, KeyCode::Down);
        settle(&mut app);
        press(&mut app, KeyCode::Char('e'));
        let f = form(&app);
        assert!(!f.rule_editable);
        assert_eq!(f.fields(), [TagField::Name, TagField::Description, TagField::Submit]);
    }

    #[test]
    fn editing_waits_until_the_tag_has_loaded() {
        let (_rt, mut app) = tags_app();
        press(&mut app, KeyCode::Char('e'));
        assert!(app.tag_form.is_none());
        assert!(app.tags.notice.as_ref().unwrap().text.contains("Wait for the tag"));
    }

    #[test]
    fn saving_an_edit_sends_the_new_name_and_in_dry_run_is_held_back() {
        let (_rt, mut app) = tags_app();
        app.core().set_dry_run(true);
        open_edit(&mut app);
        type_into(&mut app, TagField::Name, "_Renamed");
        form_mut(&mut app).field = TagField::Submit;
        press(&mut app, KeyCode::Enter);
        assert_eq!(summary(form(&app)), "save changes to Servers_Renamed");
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        let log = app.core().dry_run_log();
        assert_eq!(log[0].summary, "Updated static tag Servers_Renamed");
        assert_eq!(log[0].variables["input"]["id"], "t1");
    }

    #[test]
    fn a_kept_rule_is_sent_back_exactly_as_it_was_stored() {
        let (_rt, mut app) = tags_app();
        app.core().set_dry_run(true);
        let mut odd = tag("t2", "Windows10", true, 3);
        let stored = r#"{"and":[{"cpu":{"cores":{"gte":4}}}]}"#;
        odd.filter = Some(stored.into());
        app.collections.cache_tag(odd);
        settle(&mut app);
        press(&mut app, KeyCode::Down);
        settle(&mut app);
        press(&mut app, KeyCode::Char('e'));
        type_into(&mut app, TagField::Description, "big machines");
        form_mut(&mut app).field = TagField::Submit;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        let log = app.core().dry_run_log();
        assert_eq!(log[0].variables["input"]["filter"], serde_json::json!({ "and": [{ "cpu": { "cores": { "gte": 4 } } }] }));
    }

    #[test]
    fn leaving_an_edit_with_changes_asks_in_edit_words() {
        let (_rt, mut app) = tags_app();
        open_edit(&mut app);
        type_into(&mut app, TagField::Description, "x");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Discard your changes to this tag?");
    }
}
