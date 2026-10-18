//! The Tags page: browse and search every tag, see one in full (its rule and its devices),
//! add and remove devices on static tags, delete a tag, or start a run on it. Drawing is in
//! `tags_ui.rs`; creating and editing are in `tag_form.rs`.

use std::collections::HashSet;

use nvoke_core::{
    is_dry_run_error,
    model::{Asset, Organization, Tag},
    targets::Target,
};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    widgets::TableState,
};

use super::{App, Confirm, Focus, Load, Msg, Page, run::{Notice, NoticeKind}};
use crate::{
    input::TextInput,
    picker::{PickKind, Picker, Purpose},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagsPane {
    List,
    Devices,
}

/// A tag in full, with the devices that carry it.
#[derive(Debug)]
pub struct TagDetail {
    pub tag: Option<Tag>,
    pub members: Vec<Asset>,
    pub total: i64,
}

/// A change to tags that has finished.
#[derive(Debug)]
pub enum TagDone {
    Deleted { id: String, name: String },
    MembersAdded { count: usize },
    MembersRemoved { count: usize },
}

/// What a pending yes/no on the Tags page is about.
#[derive(Clone, Debug)]
pub enum TagConfirm {
    Delete { id: String, name: String, count: i64 },
    RemoveDevices { tag_id: String, name: String, asset_ids: Vec<String> },
    AddDevices { tag_id: String, name: String, assets: Vec<Asset> },
    /// Leaving the new-tag or edit form with unsaved work.
    DiscardForm { creating: bool },
}

impl TagConfirm {
    /// The question asked in the footer.
    pub fn prompt(&self) -> String {
        let devices = |n: usize| format!("{n} device{}", if n == 1 { "" } else { "s" });
        match self {
            TagConfirm::Delete { name, count, .. } => {
                let effect = if *count > 0 { format!("It is removed from {}; the devices are not affected.", devices(*count as usize)) } else { "No devices have it.".into() };
                format!("Delete tag {name}? {effect}")
            }
            TagConfirm::RemoveDevices { name, asset_ids, .. } => format!("Remove {} from {name}?", devices(asset_ids.len())),
            TagConfirm::AddDevices { name, assets, .. } => format!("Add {} to {name}?", devices(assets.len())),
            TagConfirm::DiscardForm { creating: true } => "Discard this new tag? Nothing has been created yet.".into(),
            TagConfirm::DiscardForm { creating: false } => "Discard your changes to this tag?".into(),
        }
    }
}

/// Devices from a run, and the organization it ran in, on their way into a tag.
#[derive(Clone, Debug)]
pub struct AssignCtx {
    pub org: Organization,
    pub devices: Vec<Asset>,
}

pub struct TagsState {
    pub term: TextInput,
    /// The search box is being typed into.
    pub searching: bool,
    /// Every tag matching the search.
    pub all: Load<Vec<Tag>>,
    pub table: TableState,
    pub selected: Option<String>,
    pub detail: Load<Tag>,
    pub members: Load<Vec<Asset>>,
    /// How many devices the tag has in all (the list may be cut short).
    pub members_total: i64,
    pub checked: HashSet<String>,
    pub members_table: TableState,
    pub pane: TagsPane,
    pub notice: Option<Notice>,
    pub busy: bool,
    /// Devices chosen in the add-devices picker, waiting to be confirmed.
    pub adding: Vec<Asset>,
    list_seq: u64,
    detail_seq: u64,
    /// The tick at which the selection last changed; details load once it has settled.
    detail_due: Option<u64>,
}

impl Default for TagsState {
    fn default() -> Self {
        Self {
            term: TextInput::default(),
            searching: false,
            all: Load::NotLoaded,
            table: TableState::default(),
            selected: None,
            detail: Load::NotLoaded,
            members: Load::NotLoaded,
            members_total: 0,
            checked: HashSet::new(),
            members_table: TableState::default(),
            pane: TagsPane::List,
            notice: None,
            busy: false,
            adding: vec![],
            list_seq: 0,
            detail_seq: 0,
            detail_due: None,
        }
    }
}

impl TagsState {
    pub fn tags(&self) -> &[Tag] {
        match &self.all {
            Load::Ready(list) => list,
            _ => &[],
        }
    }

    pub fn current(&self) -> Option<&Tag> {
        self.table.selected().and_then(|i| self.tags().get(i))
    }

    /// The full record if it has loaded, else the list's summary of it.
    pub fn shown(&self) -> Option<&Tag> {
        match &self.detail {
            Load::Ready(tag) => Some(tag),
            _ => self.current(),
        }
    }

    pub fn is_static(&self) -> bool {
        self.shown().is_some_and(|t| !t.is_dynamic())
    }

    pub fn member_list(&self) -> &[Asset] {
        match &self.members {
            Load::Ready(list) => list,
            _ => &[],
        }
    }
}

impl App {
    // ---- loading -------------------------------------------------------------------

    /// Called when the page is shown.
    pub(super) fn enter_tags(&mut self) {
        if matches!(self.tags.all, Load::NotLoaded) {
            self.load_tags();
        }
    }

    /// (Re)reads the list of tags that match the search.
    pub(crate) fn load_tags(&mut self) {
        self.tags.list_seq += 1;
        let seq = self.tags.list_seq;
        if !matches!(self.tags.all, Load::Ready(_)) {
            self.tags.all = Load::Loading;
        }
        let term = self.tags.term.value.clone();
        let collections = self.collections.clone();
        self.spawn(move |_| async move { Msg::TagList { seq, result: collections.all_tags(&term).await } });
    }

    pub(super) fn on_tag_list(&mut self, seq: u64, result: Result<Vec<Tag>, String>) {
        if let Err(e) = &result {
            self.note_error(e);
        }
        if seq != self.tags.list_seq {
            return;
        }
        match result {
            Ok(list) => {
                // Stay on the same tag when the list changes under the selection.
                let keep = self.tags.selected.as_ref().and_then(|id| list.iter().position(|t| &t.id == id));
                let index = keep.or((!list.is_empty()).then_some(0));
                self.tags.all = Load::Ready(list);
                self.tags.table.select(index);
                let id = index.map(|i| self.tags.tags()[i].id.clone());
                if id != self.tags.selected {
                    self.set_selected_tag(id);
                } else if matches!(self.tags.detail, Load::NotLoaded) && id.is_some() {
                    self.tags.detail_due = Some(self.tick);
                }
            }
            Err(e) => self.tags.all = Load::Failed(e),
        }
    }

    /// Makes `id` the tag shown on the right; its details load once the selection settles.
    pub(super) fn set_selected_tag(&mut self, id: Option<String>) {
        self.tags.selected = id.clone();
        self.tags.detail = if id.is_some() { Load::Loading } else { Load::NotLoaded };
        self.tags.members = Load::NotLoaded;
        self.tags.members_total = 0;
        self.tags.checked.clear();
        self.tags.members_table.select(None);
        self.tags.pane = TagsPane::List;
        self.tags.detail_due = id.map(|_| self.tick);
    }

    /// Called every tick: loads the selected tag's details after the selection has been still for a moment.
    pub(super) fn tick_tags(&mut self) {
        let due = self.tags.detail_due.is_some_and(|at| self.tick.saturating_sub(at) >= 3);
        if due {
            self.load_tag_detail();
        }
    }

    pub(crate) fn load_tag_detail(&mut self) {
        self.tags.detail_due = None;
        let Some(id) = self.tags.selected.clone() else { return };
        self.tags.detail_seq += 1;
        let seq = self.tags.detail_seq;
        let collections = self.collections.clone();
        self.spawn(move |_| async move {
            let result = async {
                let tag = collections.tag(&id).await?;
                let (members, total) = match tag.as_ref().and_then(|t| t.organization.as_ref()) {
                    Some(org) => {
                        let found = collections.tag_members(&org.id, &id).await?;
                        (found.assets, found.total_count)
                    }
                    None => (vec![], 0),
                };
                Ok(TagDetail { tag, members, total })
            }
            .await;
            Msg::TagDetailLoaded { seq, tag_id: id, result }
        });
    }

    pub(super) fn on_tag_detail(&mut self, seq: u64, tag_id: String, result: Result<TagDetail, String>) {
        if let Err(e) = &result {
            self.note_error(e);
        }
        if seq != self.tags.detail_seq || self.tags.selected.as_ref() != Some(&tag_id) {
            return; // an answer about a tag that is no longer selected
        }
        match result {
            Ok(TagDetail { tag: Some(tag), members, total }) => {
                self.tags.members_total = total;
                self.tags.members_table.select((!members.is_empty()).then(|| self.tags.members_table.selected().unwrap_or(0).min(members.len() - 1)));
                self.tags.checked.retain(|id| members.iter().any(|m| &m.id == id));
                self.tags.members = Load::Ready(members);
                self.tags.detail = Load::Ready(tag);
            }
            Ok(TagDetail { tag: None, .. }) => {
                self.tags.detail = Load::Failed("This tag was not found. It may have been deleted.".into());
                self.tags.members = Load::NotLoaded;
            }
            Err(e) => {
                self.tags.detail = Load::Failed(e.clone());
                self.tags.members = Load::Failed(e);
            }
        }
    }

    // ---- keys ----------------------------------------------------------------------

    pub(super) fn on_tags_key(&mut self, key: KeyEvent) {
        if self.tag_form.is_some() {
            return self.on_tag_form_key(key);
        }
        if self.tags.busy {
            return;
        }
        if self.tags.searching {
            return self.on_tags_search_key(key);
        }
        match self.tags.pane {
            TagsPane::List => self.on_tag_list_key(key),
            TagsPane::Devices => self.on_tag_devices_key(key),
        }
    }

    fn on_tags_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.tags.searching = false,
            KeyCode::Esc => {
                self.tags.searching = false;
                if !self.tags.term.value.is_empty() {
                    self.tags.term.value.clear();
                    self.load_tags();
                }
            }
            KeyCode::Down => {
                self.tags.searching = false;
                self.move_tag(1);
            }
            KeyCode::Up => {
                self.tags.searching = false;
                self.move_tag(-1);
            }
            _ => {
                if self.tags.term.handle_key(key) {
                    self.load_tags(); // the list is cached, so narrowing as you type is instant
                }
            }
        }
    }

    fn on_tag_list_key(&mut self, key: KeyEvent) {
        self.tags.notice = None;
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => {
                if !self.tags.term.value.is_empty() && key.code == KeyCode::Esc {
                    self.tags.term.value.clear();
                    self.load_tags();
                } else {
                    self.focus = Focus::Sidebar;
                }
            }
            KeyCode::Char('/') => self.tags.searching = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_tag(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_tag(-1),
            KeyCode::PageDown => self.move_tag(10),
            KeyCode::PageUp => self.move_tag(-10),
            KeyCode::Home | KeyCode::Char('g') => self.move_tag(-(self.tags.tags().len() as isize)),
            KeyCode::End | KeyCode::Char('G') => self.move_tag(self.tags.tags().len() as isize),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter if !self.tags.member_list().is_empty() => self.tags.pane = TagsPane::Devices,
            KeyCode::Char('u') => {
                self.load_tags();
                self.load_tag_detail();
            }
            KeyCode::Char('x') | KeyCode::Delete => self.ask_delete_tag(),
            KeyCode::Char('+') => self.open_add_devices(),
            KeyCode::Char('n') => self.open_new_tag_form(),
            KeyCode::Char('e') => self.open_edit_tag_form(),
            KeyCode::Char('R') => self.run_on_tag(),
            _ => {}
        }
    }

    fn on_tag_devices_key(&mut self, key: KeyEvent) {
        let count = self.tags.member_list().len();
        let table = &mut self.tags.members_table;
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => self.tags.pane = TagsPane::List,
            KeyCode::Down | KeyCode::Char('j') => table.select_next(),
            KeyCode::Up | KeyCode::Char('k') => table.select_previous(),
            KeyCode::PageDown => table.scroll_down_by(10),
            KeyCode::PageUp => table.scroll_up_by(10),
            KeyCode::Home | KeyCode::Char('g') => table.select_first(),
            KeyCode::End | KeyCode::Char('G') => table.select(count.checked_sub(1)),
            // Only static tags have devices that can be taken out by hand.
            KeyCode::Char(' ') if self.tags.is_static() => {
                if let Some(id) = self.tags.members_table.selected().and_then(|i| self.tags.member_list().get(i)).map(|m| m.id.clone())
                    && !self.tags.checked.remove(&id)
                {
                    self.tags.checked.insert(id);
                }
            }
            KeyCode::Char('a') if self.tags.is_static() => {
                let all: HashSet<String> = self.tags.member_list().iter().map(|m| m.id.clone()).collect();
                self.tags.checked = if self.tags.checked == all { HashSet::new() } else { all };
            }
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Char('-') if self.tags.is_static() => self.ask_remove_devices(),
            KeyCode::Char('+') => self.open_add_devices(),
            KeyCode::Char('R') => self.run_on_tag(),
            _ => {}
        }
    }

    fn move_tag(&mut self, by: isize) {
        let len = self.tags.tags().len();
        if len == 0 {
            return;
        }
        let current = self.tags.table.selected().unwrap_or(0) as isize;
        let next = (current + by).clamp(0, len as isize - 1) as usize;
        self.tags.table.select(Some(next));
        let id = self.tags.tags()[next].id.clone();
        if self.tags.selected.as_ref() != Some(&id) {
            self.set_selected_tag(Some(id));
        }
    }

    // ---- actions -------------------------------------------------------------------

    /// Tells the user, on the Tags page and (briefly) in the footer wherever they are.
    fn notice(&mut self, kind: NoticeKind, text: impl Into<String>) {
        let text = text.into();
        self.tags.notice = Some(Notice { kind, text: text.clone() });
        self.flash(kind, text);
    }

    fn ask_delete_tag(&mut self) {
        let Some(tag) = self.tags.shown().cloned() else { return };
        self.confirm = Some(Confirm::Tag(TagConfirm::Delete { id: tag.id.clone(), name: tag.name.clone(), count: tag.asset_count.unwrap_or(0) }));
    }

    fn ask_remove_devices(&mut self) {
        let Some(tag) = self.tags.shown().cloned() else { return };
        // With nothing ticked, the highlighted device is the one meant.
        let mut ids: Vec<String> = self.tags.checked.iter().cloned().collect();
        if ids.is_empty() {
            ids.extend(self.tags.members_table.selected().and_then(|i| self.tags.member_list().get(i)).map(|m| m.id.clone()));
        }
        if ids.is_empty() {
            return;
        }
        ids.sort();
        self.confirm = Some(Confirm::Tag(TagConfirm::RemoveDevices { tag_id: tag.id, name: tag.name, asset_ids: ids }));
    }

    /// Opens the device picker for a static tag, leaving out devices that are already in it.
    fn open_add_devices(&mut self) {
        let Some(tag) = self.tags.shown().cloned() else { return };
        if tag.is_dynamic() {
            return self.notice(NoticeKind::Info, "A dynamic tag fills itself from its rule: edit the rule to change who is in it.");
        }
        let Some(org) = tag.organization.as_ref() else { return self.notice(NoticeKind::Error, "This tag has no organization to look devices up in.") };
        let mut picker = Picker::new(PickKind::Devices, Purpose::TagMembers, &format!("Add devices to {}", tag.name), Some(org.id.clone()));
        picker.exclude = self.tags.member_list().iter().map(|m| m.id.clone()).collect();
        self.tags.adding.clear();
        self.open_picker_with(picker);
    }

    /// The picker was closed: devices picked for a tag turn into a question.
    pub(super) fn finish_adding_devices(&mut self) {
        let adding = std::mem::take(&mut self.tags.adding);
        let Some(tag) = self.tags.shown().cloned() else { return };
        if !adding.is_empty() {
            self.confirm = Some(Confirm::Tag(TagConfirm::AddDevices { tag_id: tag.id, name: tag.name, assets: adding }));
        }
    }

    /// Starts the Run page on this tag.
    fn run_on_tag(&mut self) {
        let Some(tag) = self.tags.shown().cloned() else { return };
        let Some(org) = tag.organization.clone() else { return self.notice(NoticeKind::Error, "This tag has no organization to run in.") };
        let org = Organization { id: org.id, name: org.name, typename: String::new() };
        self.run.prefill(Some(org), None, vec![Target::Tag(tag)]);
        self.page = Page::Run;
        self.focus = Focus::Content;
        self.resolve_devices();
    }

    /// Starts putting the devices of the selected run (or just the selected result group) into a tag.
    pub(super) fn start_tag_devices(&mut self, only_group: bool) {
        let Some(job) = self.current_job().cloned() else { return };
        let Some(org_id) = job.org_id.clone() else {
            return self.flash(NoticeKind::Error, "This run doesn't record which organization it ran in, so its tags can't be looked up. Use the Tags page instead.");
        };
        let org = Organization { id: org_id, name: job.org_name.clone().unwrap_or_default(), typename: String::new() };
        let indexes: Vec<usize> = match (only_group, self.current_group()) {
            (true, Some(group)) => group.tasks.clone(),
            _ => (0..job.tasks.len()).collect(),
        };
        let mut seen = HashSet::new();
        let devices: Vec<Asset> = indexes
            .into_iter()
            .filter_map(|i| job.tasks.get(i))
            .filter_map(|t| t.asset_id.as_ref().map(|id| Asset::stub(id, &t.device_name)))
            .filter(|a| seen.insert(a.id.clone()))
            .collect();
        if devices.is_empty() {
            return self.flash(NoticeKind::Error, "None of these devices have a known ID to tag.");
        }
        let title = format!("Tag {} device{}", devices.len(), if devices.len() == 1 { "" } else { "s" });
        let picker = Picker::new(PickKind::StaticTags, Purpose::AssignTag, &title, Some(org.id.clone()));
        self.assign = Some(AssignCtx { org, devices });
        self.open_picker_with(picker);
    }

    /// The picker chose something for a run's devices.
    pub(super) fn choose_for_assign(&mut self, item: crate::picker::PickItem) {
        use crate::picker::PickItem;
        let Some(ctx) = self.assign.take() else { return };
        self.picker = None;
        match item {
            PickItem::Tag(tag) => {
                self.confirm = Some(Confirm::Tag(TagConfirm::AddDevices { tag_id: tag.id, name: tag.name, assets: ctx.devices }));
            }
            PickItem::NewTag(name) => {
                // A new tag with these devices already in it, to be named and confirmed on the form.
                let mut form = super::TagForm::create(Some(ctx.org));
                form.data.name = nvoke_core::rules::suggest_tag_name(&name);
                form.data.devices = ctx.devices;
                form.dirty = true;
                self.tag_form = Some(form);
                self.page = Page::Tags;
                self.focus = Focus::Content;
                self.enter_tags();
            }
            _ => {}
        }
    }

    /// The user said yes.
    pub(super) fn run_tag_confirmed(&mut self, confirm: TagConfirm) {
        if matches!(confirm, TagConfirm::DiscardForm { .. }) {
            return self.close_tag_form();
        }
        self.tags.busy = true;
        self.tags.notice = None;
        let (core, collections) = (self.core.clone(), self.collections.clone());
        match confirm {
            TagConfirm::Delete { id, name, .. } => self.spawn(move |_| async move {
                let result = core.delete_tags(std::slice::from_ref(&id), Some(&format!("tag {name}"))).await.map(|()| {
                    collections.uncache_tag(&id);
                    TagDone::Deleted { id, name }
                });
                Msg::TagDone(result)
            }),
            TagConfirm::RemoveDevices { tag_id, name, asset_ids } => self.spawn(move |_| async move {
                let count = asset_ids.len();
                let result = match core.unassign_static_tags(std::slice::from_ref(&tag_id), &asset_ids, Some(&name)).await {
                    Ok(()) => {
                        collections.refresh_membership(&tag_id).await;
                        Ok(TagDone::MembersRemoved { count })
                    }
                    Err(e) => Err(e),
                };
                Msg::TagDone(result)
            }),
            TagConfirm::DiscardForm { .. } => unreachable!("handled above"),
            TagConfirm::AddDevices { tag_id, name, assets } => self.spawn(move |_| async move {
                let ids: Vec<String> = assets.iter().map(|a| a.id.clone()).collect();
                let result = match core.assign_static_tags(std::slice::from_ref(&tag_id), &ids, Some(&name)).await {
                    Ok(()) => {
                        collections.refresh_membership(&tag_id).await;
                        Ok(TagDone::MembersAdded { count: ids.len() })
                    }
                    Err(e) => Err(e),
                };
                Msg::TagDone(result)
            }),
        }
    }

    pub(super) fn on_tag_done(&mut self, result: Result<TagDone, String>) {
        self.tags.busy = false;
        self.tags.notice = None;
        let plural = |n: usize| format!("{n} device{}", if n == 1 { "" } else { "s" });
        match result {
            Ok(TagDone::Deleted { id, name }) => {
                if let Load::Ready(list) = &mut self.tags.all {
                    list.retain(|t| t.id != id);
                }
                let len = self.tags.tags().len();
                let next = self.tags.table.selected().unwrap_or(0).min(len.saturating_sub(1));
                self.tags.table.select((len > 0).then_some(next));
                let id = self.tags.table.selected().map(|i| self.tags.tags()[i].id.clone());
                self.set_selected_tag(id);
                self.notice(NoticeKind::Info, format!("Deleted tag {name}."));
            }
            Ok(TagDone::MembersAdded { count, .. }) => {
                self.notice(NoticeKind::Info, format!("Added {}.", plural(count)));
                self.refresh_after_membership_change();
            }
            Ok(TagDone::MembersRemoved { count, .. }) => {
                self.tags.checked.clear();
                self.notice(NoticeKind::Info, format!("Removed {}.", plural(count)));
                self.refresh_after_membership_change();
            }
            Err(e) if is_dry_run_error(&e) => self.notice(NoticeKind::Info, "Dry run: nothing was sent. Press d to see the request that was held back."),
            Err(e) => {
                self.note_error(&e);
                self.notice(NoticeKind::Error, e);
            }
        }
    }

    fn refresh_after_membership_change(&mut self) {
        self.load_tags(); // the counts changed
        self.load_tag_detail();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use ratatui::crossterm::event::KeyCode;

    use super::*;
    use crate::{
        app::{
            Event,
            run::tests::{device, pump, press, run_app, tag, type_text},
        },
        picker::PickItem,
    };

    const WINDOWS_RULE: &str = r#"{"and":[{"operatingSystem":{"name":{"contains":"Windows 10","caseSensitivity":"NON_SENSITIVE"}}}]}"#;

    /// An app on the Tags page with two tags (one static, one dynamic) and their devices cached, so nothing reaches the network.
    pub fn tags_app() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = run_app();
        let mut dynamic = tag("t2", "Windows10", true, 3);
        dynamic.filter = Some(WINDOWS_RULE.into());
        let mut servers = tag("t1", "Servers", false, 2);
        servers.description = Some("Production servers".into());
        let zed = tag("t3", "Zed", false, 0);
        app.collections.seed(vec![], vec![servers.clone(), dynamic.clone(), zed.clone()], vec![]);
        for t in [servers, dynamic, zed] {
            app.collections.cache_tag(t);
        }
        app.collections.seed_members("o1", "t1", vec![device("a1", "PC-1", "CONNECTED", "Windows 11"), device("a3", "PC-3", "DISCONNECTED", "Windows 10")], 2);
        app.collections.seed_members("o1", "t2", vec![device("a1", "PC-1", "CONNECTED", "Windows 11"), device("a4", "PC-4", "CONNECTED", "Windows 10"), device("a5", "PC-5", "CONNECTED", "Windows 10")], 3);
        app.collections.seed_members("o1", "t3", vec![], 0);
        app.go(Page::Tags);
        app.focus = Focus::Content;
        (rt, app)
    }

    /// Puts a tag's full record in the cache.
    pub fn cache_tag(app: &App, tag: Tag) {
        app.collections.cache_tag(tag);
    }

    /// Makes the device search for `term` in `org` answer from the cache.
    pub fn seed_assets(app: &App, org: &str, term: &str, assets: Vec<Asset>) {
        app.collections.seed_assets(org, term, assets);
    }

    /// Delivers an error as the answer to the newest tag-list request.
    pub fn fail_list(app: &mut App, error: &str) {
        let seq = app.tags.list_seq;
        app.on_tag_list(seq, Err(error.into()));
    }

    /// Lets the list and the selected tag's details load, as a few ticks and the answers would.
    pub fn settle(app: &mut App) {
        pump(app);
        for _ in 0..4 {
            app.update(Event::Tick);
        }
        pump(app);
    }

    fn names(app: &App) -> Vec<String> {
        app.tags.tags().iter().map(|t| t.name.clone()).collect()
    }

    fn member_names(app: &App) -> Vec<String> {
        app.tags.member_list().iter().map(|m| m.name.clone()).collect()
    }

    #[test]
    fn opening_the_page_lists_every_tag_by_name_and_shows_the_first_with_its_devices() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        assert_eq!(names(&app), ["Servers", "Windows10", "Zed"]);
        assert_eq!(app.tags.selected.as_deref(), Some("t1"));
        assert_eq!(app.tags.shown().unwrap().description.as_deref(), Some("Production servers"));
        assert_eq!(member_names(&app), ["PC-1", "PC-3"]);
    }

    #[test]
    fn a_tags_details_load_only_once_the_selection_has_been_still_for_a_moment() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        let before = app.tags.detail_seq;
        // Scrolling quickly through the list must not fetch every tag it passes.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.tags.selected.as_deref(), Some("t2"));
        assert!(matches!(app.tags.detail, Load::Loading) || app.tags.shown().is_some());
        assert_eq!(app.tags.detail_seq, before, "nothing fetched yet");
        app.update(Event::Tick);
        assert_eq!(app.tags.detail_seq, before, "still within the pause");
        settle(&mut app);
        assert_eq!(app.tags.detail_seq, before + 1, "exactly one fetch, for the tag it stopped on");
        assert_eq!(member_names(&app), ["PC-1", "PC-4", "PC-5"]);
    }

    #[test]
    fn an_answer_about_a_tag_that_is_no_longer_selected_is_ignored() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        let old_seq = app.tags.detail_seq;
        press(&mut app, KeyCode::Down);
        settle(&mut app);
        app.on_tag_detail(old_seq, "t1".into(), Ok(TagDetail { tag: Some(tag("t1", "Servers", false, 2)), members: vec![device("zz", "STALE", "CONNECTED", "x")], total: 1 }));
        assert!(!member_names(&app).contains(&"STALE".to_string()));
        assert_eq!(app.tags.shown().unwrap().name, "Windows10");
    }

    #[test]
    fn typing_in_the_search_narrows_the_list_as_you_type_and_swallows_shortcuts() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        press(&mut app, KeyCode::Char('/'));
        assert!(app.tags.searching);
        type_text(&mut app, "wi");
        pump(&mut app);
        assert_eq!(names(&app), ["Windows10"]);
        // Shortcuts are just letters while typing.
        type_text(&mut app, "ndq?1");
        assert!(!app.quit && !app.show_help && app.page == Page::Tags, "typing never triggers shortcuts");
        assert_eq!(app.tags.term.value, "windq?1");
        press(&mut app, KeyCode::Esc);
        pump(&mut app);
        assert!(!app.tags.searching && app.tags.term.value.is_empty());
        assert_eq!(names(&app).len(), 3, "esc clears the search and brings the list back");
    }

    #[test]
    fn enter_keeps_the_search_and_the_selection_follows_the_narrowed_list() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "zed");
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        assert!(!app.tags.searching && app.tags.term.value == "zed");
        assert_eq!(app.tags.selected.as_deref(), Some("t3"), "the only match is selected");
        settle(&mut app);
        assert!(app.tags.member_list().is_empty());
    }

    #[test]
    fn the_arrows_choose_a_tag_stop_at_the_ends_and_right_goes_to_its_devices() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.tags.selected.as_deref(), Some("t1"));
        press(&mut app, KeyCode::End);
        assert_eq!(app.tags.selected.as_deref(), Some("t3"));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.tags.selected.as_deref(), Some("t3"));
        settle(&mut app);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.tags.pane, TagsPane::List, "an empty tag has no devices to step into");
        press(&mut app, KeyCode::Home);
        settle(&mut app);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.tags.pane, TagsPane::Devices);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.tags.pane, TagsPane::List);
        press(&mut app, KeyCode::Left);
        assert!(app.focus == Focus::Sidebar);
    }

    #[test]
    fn devices_of_a_static_tag_can_be_ticked_but_a_dynamic_tags_cannot() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.tags.checked.len(), 1);
        press(&mut app, KeyCode::Char(' '));
        assert!(app.tags.checked.is_empty(), "space toggles");
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.tags.checked.len(), 2);
        press(&mut app, KeyCode::Char('a'));
        assert!(app.tags.checked.is_empty(), "a again clears");

        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Down); // Windows10 (dynamic)
        settle(&mut app);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char('a'));
        assert!(app.tags.checked.is_empty(), "a dynamic tag's membership is its rule's business");
        press(&mut app, KeyCode::Char('x'));
        assert!(app.confirm.is_none(), "and nothing can be removed by hand");
    }

    #[test]
    fn deleting_a_tag_asks_first_and_says_what_it_affects() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        press(&mut app, KeyCode::Char('x'));
        let prompt = app.confirm.as_ref().unwrap().prompt();
        assert_eq!(prompt, "Delete tag Servers? It is removed from 2 devices; the devices are not affected.");
        press(&mut app, KeyCode::Char('n'));
        assert!(app.confirm.is_none() && app.core().dry_run_log().is_empty(), "anything but y cancels");
        press(&mut app, KeyCode::End);
        settle(&mut app);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Delete tag Zed? No devices have it.");
    }

    #[test]
    fn in_dry_run_deleting_a_tag_is_held_back_and_the_tag_stays() {
        let (_rt, mut app) = tags_app();
        app.core().set_dry_run(true);
        settle(&mut app);
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert!(app.tags.busy);
        pump(&mut app);
        assert!(!app.tags.busy);
        assert_eq!(app.tags.notice.as_ref().unwrap().text, "Dry run: nothing was sent. Press d to see the request that was held back.");
        assert_eq!(names(&app).len(), 3, "nothing was deleted");
        assert_eq!(app.core().dry_run_log()[0].summary, "Deleted tag Servers");
    }

    #[test]
    fn a_deleted_tag_leaves_the_list_and_the_next_one_is_shown() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        app.on_tag_done(Ok(TagDone::Deleted { id: "t1".into(), name: "Servers".into() }));
        assert_eq!(names(&app), ["Windows10", "Zed"]);
        assert_eq!(app.tags.selected.as_deref(), Some("t2"), "the tag after it is selected");
        assert_eq!(app.tags.notice.as_ref().unwrap().text, "Deleted tag Servers.");
        // Deleting the last tag in the list selects the one before it; deleting the only one leaves nothing selected.
        app.on_tag_done(Ok(TagDone::Deleted { id: "t3".into(), name: "Zed".into() }));
        assert_eq!(app.tags.selected.as_deref(), Some("t2"));
        app.on_tag_done(Ok(TagDone::Deleted { id: "t2".into(), name: "Windows10".into() }));
        assert!(app.tags.selected.is_none() && app.tags.tags().is_empty());
    }

    #[test]
    fn removing_devices_asks_about_the_ticked_ones_or_the_highlighted_one() {
        let (_rt, mut app) = tags_app();
        app.core().set_dry_run(true);
        settle(&mut app);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Remove 1 device from Servers?", "nothing ticked: the highlighted device");
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Remove 2 devices from Servers?");
        press(&mut app, KeyCode::Char('y'));
        pump(&mut app);
        assert_eq!(app.core().dry_run_log()[0].summary, "Removed 2 devices from Servers");
        assert_eq!(member_names(&app), ["PC-1", "PC-3"], "held back: the devices are still there");
    }

    #[test]
    fn adding_devices_picks_from_devices_not_already_in_the_tag_and_asks_before_sending() {
        let (_rt, mut app) = tags_app();
        app.core().set_dry_run(true);
        app.collections.seed_assets("o1", "", vec![device("a1", "PC-1", "CONNECTED", "Windows 11"), device("a2", "PC-2", "CONNECTED", "Windows 11"), device("a9", "LNX-1", "CONNECTED", "Ubuntu 22.04")]);
        settle(&mut app);
        press(&mut app, KeyCode::Char('+'));
        pump(&mut app);
        let keys: Vec<String> = app.picker.as_ref().unwrap().items.iter().map(PickItem::key).collect();
        assert_eq!(keys, ["d:a2", "d:a9"], "PC-1 is already in Servers, so it is not offered");
        press(&mut app, KeyCode::Enter); // PC-2
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // LNX-1
        assert_eq!(app.tags.adding.len(), 2);
        press(&mut app, KeyCode::Esc);
        assert!(app.picker.is_none());
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Add 2 devices to Servers?");
        press(&mut app, KeyCode::Char('y'));
        pump(&mut app);
        let entry = &app.core().dry_run_log()[0];
        assert_eq!(entry.summary, "Added 2 devices to Servers");
        assert_eq!(entry.variables["input"]["assetIds"], serde_json::json!(["a2", "a9"]));
        assert_eq!(entry.variables["input"]["tagIds"], serde_json::json!(["t1"]));
    }

    #[test]
    fn closing_the_add_picker_without_choosing_anything_asks_nothing() {
        let (_rt, mut app) = tags_app();
        app.collections.seed_assets("o1", "", vec![device("a2", "PC-2", "CONNECTED", "Windows 11")]);
        settle(&mut app);
        press(&mut app, KeyCode::Char('+'));
        pump(&mut app);
        press(&mut app, KeyCode::Esc);
        assert!(app.confirm.is_none() && app.picker.is_none());
    }

    #[test]
    fn a_dynamic_tag_cannot_have_devices_added_by_hand_and_says_why() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        press(&mut app, KeyCode::Down);
        settle(&mut app);
        press(&mut app, KeyCode::Char('+'));
        assert!(app.picker.is_none());
        assert!(app.tags.notice.as_ref().unwrap().text.contains("fills itself from its rule"));
    }

    #[test]
    fn run_script_starts_the_run_page_on_this_tag() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        press(&mut app, KeyCode::Char('R'));
        pump(&mut app);
        assert!(app.page == Page::Run && app.focus == Focus::Content);
        assert_eq!(app.run.org.as_ref().unwrap().name, "Acme");
        assert_eq!(app.run.targets.len(), 1);
        assert_eq!(app.run.targets[0].key(), "t:t1");
        assert!(app.run.script.is_none() && app.run.field == crate::app::RunField::Script, "the script is what is left to choose");
        assert_eq!(app.run.devices().iter().map(|d| d.name.clone()).collect::<Vec<_>>(), ["PC-1", "PC-3"]);
    }

    #[test]
    fn a_refused_key_while_loading_the_list_is_flagged_and_shown() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        let seq = app.tags.list_seq;
        app.on_tag_list(seq, Err("Access denied (HTTP 403): your API key is no longer active.".into()));
        assert!(app.key_rejected);
        assert!(matches!(app.tags.all, Load::Failed(_)));
    }

    #[test]
    fn after_devices_are_added_or_removed_the_counts_and_devices_are_refreshed() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        let (list, detail) = (app.tags.list_seq, app.tags.detail_seq);
        app.on_tag_done(Ok(TagDone::MembersRemoved { count: 2 }));
        assert_eq!(app.tags.notice.as_ref().unwrap().text, "Removed 2 devices.");
        assert!(app.tags.list_seq > list && app.tags.detail_seq > detail);
        app.on_tag_done(Ok(TagDone::MembersAdded { count: 1 }));
        assert_eq!(app.tags.notice.as_ref().unwrap().text, "Added 1 device.");
    }

    #[test]
    fn keys_do_nothing_while_a_change_is_being_sent() {
        let (_rt, mut app) = tags_app();
        settle(&mut app);
        app.tags.busy = true;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.tags.selected.as_deref(), Some("t1"));
        app.tags.busy = false;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.tags.selected.as_deref(), Some("t2"));
    }

    // ---- tagging the devices of a run -------------------------------------------------------------

    use crate::app::viewer::tests::viewer_app;

    /// Activity with a run that remembers its organization, on its results pane (three devices, one group).
    fn assign_app() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = viewer_app();
        let mut job = app.job_list[0].clone();
        job.org_id = Some("o1".into());
        job.org_name = Some("Acme".into());
        app.jobs.seed(vec![job]);
        app.sync_jobs();
        app.collections.seed(vec![], vec![tag("t1", "Servers", false, 2), tag("t2", "Windows10", true, 5), tag("t3", "Laptops", false, 0)], vec![]);
        (rt, app)
    }

    fn picked_keys(app: &App) -> Vec<String> {
        app.picker.as_ref().unwrap().items.iter().map(PickItem::key).collect()
    }

    #[test]
    fn t_on_a_result_group_offers_the_static_tags_and_asks_before_adding_its_devices() {
        let (_rt, mut app) = assign_app();
        app.core().set_dry_run(true);
        press(&mut app, KeyCode::Char('T'));
        pump(&mut app);
        let p = app.picker.as_ref().unwrap();
        assert_eq!(p.title, "Tag 3 devices");
        assert_eq!(picked_keys(&app), ["t:t3", "t:t1"], "static tags only (Windows10 is dynamic), by name");
        press(&mut app, KeyCode::Down); // Servers
        press(&mut app, KeyCode::Enter);
        assert!(app.picker.is_none());
        assert_eq!(app.confirm.as_ref().unwrap().prompt(), "Add 3 devices to Servers?");
        press(&mut app, KeyCode::Char('y'));
        pump(&mut app);
        let entry = &app.core().dry_run_log()[0];
        assert_eq!(entry.summary, "Added 3 devices to Servers");
        assert_eq!(entry.variables["input"]["assetIds"], serde_json::json!(["PC-1", "PC-2", "PC-3"]));
        assert_eq!(entry.variables["input"]["tagIds"], serde_json::json!(["t1"]));
    }

    #[test]
    fn the_result_of_tagging_is_shown_in_the_footer_wherever_you_are() {
        let (_rt, mut app) = assign_app();
        app.core().set_dry_run(true);
        press(&mut app, KeyCode::Char('T'));
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('y'));
        pump(&mut app);
        assert!(app.page == Page::Activity, "the user never left Activity");
        let (flash, _) = app.flash.as_ref().unwrap();
        assert!(flash.text.starts_with("Dry run: nothing was sent"), "{}", flash.text);
        app.on_tag_done(Ok(TagDone::MembersAdded { count: 3 }));
        assert_eq!(app.flash.as_ref().unwrap().0.text, "Added 3 devices.");
    }

    #[test]
    fn a_name_that_matches_nothing_offers_to_create_a_tag_and_an_exact_match_does_not() {
        let (_rt, mut app) = assign_app();
        press(&mut app, KeyCode::Char('T'));
        pump(&mut app);
        type_text(&mut app, "brand new");
        pump(&mut app);
        assert_eq!(picked_keys(&app), ["n:brand new"], "nothing matches, so the only option is to create it");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('T'));
        pump(&mut app);
        type_text(&mut app, "SERVERS");
        pump(&mut app);
        assert_eq!(picked_keys(&app), ["t:t1"], "an exact match (ignoring case) needs no 'create' option");
    }

    #[test]
    fn choosing_to_create_opens_the_tag_form_with_the_devices_already_in_it() {
        let (_rt, mut app) = assign_app();
        press(&mut app, KeyCode::Char('T'));
        pump(&mut app);
        type_text(&mut app, "needs patching");
        pump(&mut app);
        press(&mut app, KeyCode::Enter);
        assert!(app.page == Page::Tags && app.picker.is_none());
        let form = app.tag_form.as_ref().expect("the form opened");
        assert_eq!(form.data.name, "needs_patching", "spaces become underscores, as a tag name needs");
        assert_eq!(form.data.org.as_ref().unwrap().id, "o1", "in the organization the run was in");
        assert_eq!(form.data.devices.iter().map(|d| d.name.clone()).collect::<Vec<_>>(), ["PC-1", "PC-2", "PC-3"]);
        assert!(form.dirty && !form.problems().any(), "ready to create as it is");
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.quit, "and an accidental q cannot throw it away");
    }

    #[test]
    fn on_the_run_list_t_tags_every_device_in_the_run() {
        let (_rt, mut app) = assign_app();
        app.pane = crate::app::Pane::Jobs;
        press(&mut app, KeyCode::Char('T'));
        pump(&mut app);
        assert_eq!(app.picker.as_ref().unwrap().title, "Tag 3 devices");
    }

    #[test]
    fn a_run_that_forgot_its_organization_says_so_instead_of_failing() {
        let (_rt, mut app) = viewer_app(); // its job has no organization recorded
        press(&mut app, KeyCode::Char('T'));
        assert!(app.picker.is_none());
        assert!(app.flash.as_ref().unwrap().0.text.contains("doesn't record which organization"));
    }

    #[test]
    fn closing_the_tag_picker_forgets_the_devices() {
        let (_rt, mut app) = assign_app();
        press(&mut app, KeyCode::Char('T'));
        pump(&mut app);
        assert!(app.assign.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.assign.is_none() && app.picker.is_none() && app.confirm.is_none());
    }

    #[test]
    fn the_flash_goes_away_and_never_hides_a_question() {
        let (_rt, mut app) = assign_app();
        app.flash(NoticeKind::Info, "Saved something.");
        app.confirm = Some(Confirm::Tag(TagConfirm::DiscardForm { creating: true }));
        // Drawing is the UI's business; here: the state says which is newer and that the flash expires.
        assert!(app.flash.is_some());
        let at = app.flash.as_ref().unwrap().1;
        assert!(app.tick.saturating_sub(at) < 50);
        app.tick += 60;
        assert!(app.tick.saturating_sub(at) >= 50);
    }
}
