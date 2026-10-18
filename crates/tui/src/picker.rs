//! A searchable list in a popup: type to narrow it, arrows to move, enter to
//! choose. One picker serves organizations, scripts, and the devices and tags a
//! script is run on.

use nvoke_core::{
    model::{Asset, Organization, Script, Tag},
    targets::Target,
};
use ratatui::widgets::TableState;

use crate::input::TextInput;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    Org,
    Script,
    /// Devices and tags to run on: several can be picked.
    Targets,
    /// Devices to put in a tag: several can be picked, and ones already in it are left out.
    Devices,
    /// One static tag.
    StaticTags,
}

/// What choosing in the picker does, so one picker serves every page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    RunOrg,
    RunScript,
    RunTargets,
    /// Where a new tag can be used.
    TagAccess,
    /// Devices to add to the tag being viewed.
    TagMembers,
    /// Devices for a tag that is being created.
    TagFormDevices,
    /// Devices from a run result, to be put in a tag.
    AssignTag,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PickItem {
    Org(Organization),
    Script(Script),
    Asset(Asset),
    Tag(Tag),
    /// "Create a static tag with this name", offered when nothing matches exactly.
    NewTag(String),
}

impl PickItem {
    /// Unique across kinds, so a device and a tag never collide.
    pub fn key(&self) -> String {
        match self {
            PickItem::Org(o) => format!("o:{}", o.id),
            PickItem::Script(s) => format!("s:{}", s.id),
            PickItem::Asset(a) => format!("d:{}", a.id),
            PickItem::Tag(t) => format!("t:{}", t.id),
            PickItem::NewTag(name) => format!("n:{name}"),
        }
    }

    pub fn as_target(&self) -> Option<Target> {
        match self {
            PickItem::Asset(a) => Some(Target::Device(a.clone())),
            PickItem::Tag(t) => Some(Target::Tag(t.clone())),
            _ => None,
        }
    }
}

pub struct Picker {
    pub kind: PickKind,
    pub purpose: Purpose,
    /// The organization that scripts, devices and tags are looked up in.
    pub org_id: Option<String>,
    /// Access pickers: which kind of organization is listed. Tab changes it.
    pub category: Option<nvoke_core::collections::AccessCategory>,
    /// Ids that are never listed (devices already in the tag).
    pub exclude: std::collections::HashSet<String>,
    pub title: String,
    pub input: TextInput,
    pub items: Vec<PickItem>,
    pub table: TableState,
    /// Targets only: listing tags rather than devices.
    pub tags_mode: bool,
    /// A search is running.
    pub loading: bool,
    pub error: Option<String>,
    /// Identifies the newest search, so a slow answer to an old one is ignored.
    pub seq: u64,
    /// The tick at which the search term last changed, for searches that wait for a pause in typing.
    pub typed_at: Option<u64>,
}

impl Picker {
    pub fn new(kind: PickKind, purpose: Purpose, title: &str, org_id: Option<String>) -> Self {
        Self { kind, purpose, org_id, category: None, exclude: Default::default(), title: title.into(), input: TextInput::default(), items: vec![], table: TableState::default(), tags_mode: false, loading: true, error: None, seq: 0, typed_at: None }
    }

    pub fn term(&self) -> &str {
        &self.input.value
    }

    /// Whether searches wait for a pause in typing: device searches go to the server, the rest are local.
    pub fn debounces(&self) -> bool {
        matches!(self.kind, PickKind::Targets | PickKind::Devices) && !self.tags_mode
    }

    pub fn set_items(&mut self, items: Vec<PickItem>) {
        // Keep the highlighted row on the same item when the list is refreshed under it.
        let keep = self.selected_item().map(PickItem::key);
        self.items = items;
        let index = keep.and_then(|k| self.items.iter().position(|i| i.key() == k)).or((!self.items.is_empty()).then_some(0));
        self.table.select(index);
    }

    pub fn selected_item(&self) -> Option<&PickItem> {
        self.table.selected().and_then(|i| self.items.get(i))
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let current = self.table.selected().unwrap_or(0) as isize;
        self.table.select(Some((current + delta).clamp(0, self.items.len() as isize - 1) as usize));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org(id: &str) -> PickItem {
        PickItem::Org(Organization { id: id.into(), name: id.into(), typename: String::new() })
    }

    #[test]
    fn keys_never_collide_across_kinds() {
        let keys = [org("1").key(), PickItem::Asset(Asset::stub("1", "a")).key()];
        assert_ne!(keys[0], keys[1]);
    }

    #[test]
    fn only_devices_and_tags_can_be_run_on() {
        assert!(org("1").as_target().is_none());
        assert!(PickItem::Asset(Asset::stub("1", "a")).as_target().is_some());
    }

    #[test]
    fn the_highlight_follows_an_item_when_the_list_is_refreshed() {
        let mut p = Picker::new(PickKind::Org, Purpose::RunOrg, "Organization", None);
        p.set_items(vec![org("a"), org("b"), org("c")]);
        assert_eq!(p.table.selected(), Some(0));
        p.move_by(2);
        assert_eq!(p.selected_item().unwrap().key(), "o:c");
        p.set_items(vec![org("x"), org("c"), org("y")]);
        assert_eq!(p.selected_item().unwrap().key(), "o:c", "still on c after it moved");
        p.set_items(vec![org("x")]);
        assert_eq!(p.table.selected(), Some(0), "falls back to the first when the item is gone");
        p.set_items(vec![]);
        assert_eq!(p.table.selected(), None);
    }

    #[test]
    fn moving_stops_at_the_ends_and_is_safe_on_an_empty_list() {
        let mut p = Picker::new(PickKind::Org, Purpose::RunOrg, "Organization", None);
        p.move_by(5);
        assert_eq!(p.table.selected(), None);
        p.set_items(vec![org("a"), org("b")]);
        p.move_by(10);
        assert_eq!(p.table.selected(), Some(1));
        p.move_by(-10);
        assert_eq!(p.table.selected(), Some(0));
    }

    #[test]
    fn only_device_searches_wait_for_a_pause_in_typing() {
        let mut p = Picker::new(PickKind::Targets, Purpose::RunTargets, "Run on", None);
        assert!(p.debounces(), "devices are searched on the server");
        p.tags_mode = true;
        assert!(!p.debounces(), "tags are searched locally");
        assert!(!Picker::new(PickKind::Org, Purpose::RunOrg, "Organization", None).debounces());
    }
}
