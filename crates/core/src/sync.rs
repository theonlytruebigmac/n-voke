//! Delta sync for cached collections. N-able has no change feed, but tag and
//! script searches accept `updatedAt: { gt }`, so after one full load only
//! what changed is asked for. Deletions don't show up in a delta, so the total
//! count is checked too and any mismatch triggers a full reload.
//! A port of `sync.ts`.

use std::cmp::Ordering;

use chrono::{DateTime, Duration, SecondsFormat};

/// Re-ask for this much before the cursor, so same-second edits aren't missed.
const OVERLAP_SECONDS: i64 = 2;

/// Most items one delta request returns; a full delta means there may be more.
pub const DELTA_LIMIT: usize = 100;

pub trait Synced {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn updated_at(&self) -> Option<&str>;
}

/// A collection held in full, with what is needed to refresh it by delta.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Collection<T> {
    pub items: Vec<T>,
    /// Latest `updatedAt` seen; deltas ask for anything newer.
    pub cursor: Option<String>,
    /// When the last full (not delta) load happened, ms since the epoch.
    pub full_at: i64,
}

pub fn max_updated_at<T: Synced>(items: &[T]) -> Option<String> {
    items.iter().filter_map(|i| i.updated_at()).max().map(String::from)
}

/// Timestamp to send as `updatedAt: { gt }` for the next delta.
pub fn delta_since(cursor: &str) -> Option<String> {
    let at = DateTime::parse_from_rfc3339(cursor).ok()?;
    Some((at - Duration::seconds(OVERLAP_SECONDS)).to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// Case-insensitive by name, ignoring accents' case only (close to `localeCompare` with base sensitivity).
pub fn by_name<T: Synced>(a: &T, b: &T) -> Ordering {
    a.name().to_lowercase().cmp(&b.name().to_lowercase())
}

/// Upserts changed items by id; returns the list sorted by name. `None` means nothing changed.
pub fn merge_delta<T: Synced + Clone>(items: &[T], changed: Vec<T>) -> Option<Vec<T>> {
    if changed.is_empty() {
        return None;
    }
    let mut merged: Vec<T> = items.to_vec();
    for c in changed {
        match merged.iter().position(|i| i.id() == c.id()) {
            Some(p) => merged[p] = c,
            None => merged.push(c),
        }
    }
    merged.sort_by(by_name);
    Some(merged)
}

/// Case-insensitive substring match on any of the given fields; a blank term matches everything.
pub fn matches(term: &str, fields: &[Option<&str>]) -> bool {
    let t = term.trim().to_lowercase();
    t.is_empty() || fields.iter().flatten().any(|f| f.to_lowercase().contains(&t))
}

impl Synced for crate::model::Tag {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn updated_at(&self) -> Option<&str> {
        self.updated_at.as_deref()
    }
}

impl Synced for crate::model::Script {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn updated_at(&self) -> Option<&str> {
        self.updated_at.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Item {
        id: &'static str,
        name: &'static str,
        updated: Option<&'static str>,
    }
    impl Synced for Item {
        fn id(&self) -> &str {
            self.id
        }
        fn name(&self) -> &str {
            self.name
        }
        fn updated_at(&self) -> Option<&str> {
            self.updated
        }
    }
    const fn item(id: &'static str, name: &'static str, updated: &'static str) -> Item {
        Item { id, name, updated: Some(updated) }
    }

    #[test]
    fn updates_changed_items_adds_new_ones_and_keeps_the_list_sorted() {
        let items = [item("1", "Alpha", "2026-10-01T00:00:00Z"), item("2", "charlie", "2026-10-01T00:00:00Z")];
        let merged = merge_delta(&items, vec![item("2", "Charlie", "2026-10-17T10:00:00Z"), item("3", "bravo", "2026-10-17T11:00:00Z")]).unwrap();
        assert_eq!(merged.iter().map(|t| t.name).collect::<Vec<_>>(), ["Alpha", "bravo", "Charlie"]);
        assert_eq!(merged.iter().find(|t| t.id == "2").unwrap().updated, Some("2026-10-17T10:00:00Z"));
    }

    #[test]
    fn reports_no_change_when_the_delta_is_empty() {
        assert!(merge_delta(&[item("1", "Alpha", "2026-10-01T00:00:00Z")], vec![]).is_none());
    }

    #[test]
    fn tracks_the_newest_updated_at_and_overlaps_the_next_delta_slightly() {
        let items = [item("1", "a", "2026-10-15T06:56:33Z"), item("2", "b", "2026-10-16T07:53:11Z"), Item { id: "3", name: "c", updated: None }];
        let cursor = max_updated_at(&items).unwrap();
        assert_eq!(cursor, "2026-10-16T07:53:11Z");
        assert_eq!(delta_since(&cursor).unwrap(), "2026-10-16T07:53:09.000Z");
    }

    #[test]
    fn an_empty_collection_has_no_cursor() {
        assert_eq!(max_updated_at::<Item>(&[]), None);
    }

    #[test]
    fn matches_case_insensitive_substrings_on_any_field() {
        assert!(matches("bit", &[Some("BitlockerPreChecker"), None]));
        assert!(matches("usage", &[Some("App Usage Tracker")]));
        assert!(matches("track", &[Some("Other"), Some("This will track the use")]));
        assert!(!matches("zzz", &[Some("Other")]));
        assert!(matches("  ", &[Some("anything")]));
    }
}
