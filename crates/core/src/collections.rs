//! Cached lists for the pickers. A port of `collections.ts`: small reference
//! lists (organizations, tags, scripts) are held in full and searched locally;
//! tags and scripts refresh by delta (only what changed since the last sync).
//! Device lists and tag members are cached per query. Held in memory for the
//! session.

use std::{collections::HashMap, future::Future, sync::{Arc, Mutex}};

use chrono::Utc;

use crate::{
    Core,
    api::Devices,
    model::{Asset, Organization, Script, Tag},
    sync::{Collection, DELTA_LIMIT, Synced, by_name, delta_since, matches, max_updated_at, merge_delta},
};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
/// Even with deltas, do a full reload this often to correct any drift.
const FULL_RELOAD_EVERY: i64 = 6 * HOUR;
/// How many matches a picker shows.
pub const RESULTS: usize = 50;

const TAGS_STALE: i64 = 30_000;
const SCRIPTS_STALE: i64 = 2 * MINUTE;
const ORGS_STALE: i64 = 10 * MINUTE;
const QUERY_STALE: i64 = MINUTE;

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Full load first time (or when stale or out of sync), otherwise merge a delta.
pub async fn sync_collection<T, FA, FutA, FD, FutD>(prev: Option<Collection<T>>, now: i64, fetch_all: FA, fetch_delta: FD) -> Result<Collection<T>, String>
where
    T: Synced + Clone,
    FA: FnOnce() -> FutA,
    FutA: Future<Output = Result<Vec<T>, String>>,
    FD: FnOnce(String) -> FutD,
    FutD: Future<Output = Result<(Vec<T>, i64), String>>,
{
    let full = |mut items: Vec<T>| {
        items.sort_by(by_name);
        let cursor = max_updated_at(&items);
        Collection { items, cursor, full_at: now }
    };
    let Some(prev) = prev.filter(|p| p.cursor.is_some() && now - p.full_at <= FULL_RELOAD_EVERY) else {
        return Ok(full(fetch_all().await?));
    };
    let since = delta_since(prev.cursor.as_deref().unwrap()).ok_or("The saved sync point is not a valid time.")?;
    let (changed, total) = fetch_delta(since).await?;
    // A full page may be cut short, and moving the cursor past it would skip the rest for good.
    if changed.len() >= DELTA_LIMIT {
        return Ok(full(fetch_all().await?));
    }
    let Some(items) = merge_delta(&prev.items, changed) else {
        // Nothing changed, but a count mismatch still means something was deleted.
        return if prev.items.len() as i64 == total { Ok(prev) } else { Ok(full(fetch_all().await?)) };
    };
    // Deltas can't report deletions; a count mismatch means something went away.
    if items.len() as i64 != total {
        return Ok(full(fetch_all().await?));
    }
    let cursor = max_updated_at(&items).or(prev.cursor.clone());
    Ok(Collection { items, cursor, full_at: prev.full_at })
}

#[derive(Clone, Debug)]
pub struct OrgDirectory {
    pub items: Vec<Organization>,
    pub full_at: i64,
}

/// Organizations have no `updatedAt` filter: a cheap count check decides whether the full list needs reloading.
pub async fn refresh_organizations<FC, FutC, FA, FutA>(prev: Option<OrgDirectory>, now: i64, count: FC, fetch_all: FA) -> Result<OrgDirectory, String>
where
    FC: FnOnce() -> FutC,
    FutC: Future<Output = Result<i64, String>>,
    FA: FnOnce() -> FutA,
    FutA: Future<Output = Result<Vec<Organization>, String>>,
{
    if let Some(prev) = prev {
        if now - prev.full_at < FULL_RELOAD_EVERY && count().await? == prev.items.len() as i64 {
            return Ok(prev);
        }
    }
    let mut items = fetch_all().await?;
    items.sort_by(by_name);
    Ok(OrgDirectory { items, full_at: now })
}

// ---- local search (pure) ----------------------------------------------------

impl Synced for Organization {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn updated_at(&self) -> Option<&str> {
        None
    }
}

pub fn search_organizations(items: &[Organization], term: &str) -> Vec<Organization> {
    items.iter().filter(|o| matches(term, &[Some(&o.name)])).take(RESULTS).cloned().collect()
}

/// Name or description substring match (better than the API's whole-word search).
pub fn search_scripts(items: &[Script], term: &str) -> Vec<Script> {
    items.iter().filter(|s| matches(term, &[Some(&s.name), s.description.as_deref()])).take(RESULTS).cloned().collect()
}

/// Tags from `prefer_org` first, then the rest.
pub fn search_tags(items: &[Tag], term: &str, prefer_org: Option<&str>) -> Vec<Tag> {
    let hits = items.iter().filter(|t| matches(term, &[Some(&t.name)]));
    let Some(org) = prefer_org else { return hits.take(RESULTS).cloned().collect() };
    let (own, rest): (Vec<&Tag>, Vec<&Tag>) = hits.partition(|t| t.organization.as_ref().is_some_and(|o| o.id == org));
    own.into_iter().chain(rest).take(RESULTS).cloned().collect()
}

/// Where a tag can be used: an organization, a customer or a site (and everything under it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessCategory {
    Organization,
    Customer,
    Site,
}

impl AccessCategory {
    pub const ALL: [AccessCategory; 3] = [Self::Organization, Self::Customer, Self::Site];

    pub fn label(self) -> &'static str {
        match self {
            Self::Organization => "Organization",
            Self::Customer => "Customer",
            Self::Site => "Site",
        }
    }

    /// The API's organization types that belong to this category.
    pub fn types(self) -> &'static [&'static str] {
        match self {
            Self::Organization => &["ServiceOrganization", "Partner"],
            Self::Customer => &["Customer"],
            Self::Site => &["Site"],
        }
    }

    /// The category an organization falls in (organizations of unknown type count as "organization").
    pub fn of(org: &Organization) -> Self {
        Self::ALL.into_iter().find(|c| c.types().contains(&org.typename.as_str())).unwrap_or(Self::Organization)
    }

    pub fn next(self) -> Self {
        Self::ALL[(Self::ALL.iter().position(|c| *c == self).unwrap_or(0) + 1) % Self::ALL.len()]
    }
}

/// Organizations of one category matching `term`.
pub fn search_organizations_in(items: &[Organization], term: &str, category: AccessCategory) -> Vec<Organization> {
    items.iter().filter(|o| category.types().contains(&o.typename.as_str()) && matches(term, &[Some(&o.name)])).take(RESULTS).cloned().collect()
}

// ---- the cache ----------------------------------------------------------------

struct Fresh<T> {
    value: T,
    checked_at: i64,
}

impl<T> Fresh<T> {
    fn is_fresh(&self, stale_after: i64, now: i64) -> bool {
        now - self.checked_at < stale_after
    }
}

pub struct Collections {
    core: Arc<Core>,
    orgs: Mutex<Option<Fresh<OrgDirectory>>>,
    tags: Mutex<Option<Fresh<Collection<Tag>>>>,
    scripts: Mutex<HashMap<String, Fresh<Collection<Script>>>>,
    assets: Mutex<HashMap<(String, String), Fresh<Vec<Asset>>>>,
    members: Mutex<HashMap<(String, String), Fresh<Devices>>>,
    tag_details: Mutex<HashMap<String, Fresh<Option<Tag>>>>,
}

impl Collections {
    pub fn new(core: Arc<Core>) -> Arc<Self> {
        Arc::new(Self {
            core,
            orgs: Mutex::new(None),
            tags: Mutex::new(None),
            scripts: Mutex::new(HashMap::new()),
            assets: Mutex::new(HashMap::new()),
            members: Mutex::new(HashMap::new()),
            tag_details: Mutex::new(HashMap::new()),
        })
    }

    pub async fn organizations(&self, term: &str) -> Result<Vec<Organization>, String> {
        let now = now_ms();
        let cached = self.orgs.lock().unwrap().as_ref().map(|f| (f.value.clone(), f.is_fresh(ORGS_STALE, now)));
        let dir = match cached {
            Some((dir, true)) => dir,
            other => {
                let fresh = refresh_organizations(other.map(|(d, _)| d), now, || self.core.count_organizations(), || self.core.fetch_all_organizations()).await?;
                *self.orgs.lock().unwrap() = Some(Fresh { value: fresh.clone(), checked_at: now });
                fresh
            }
        };
        Ok(search_organizations(&dir.items, term))
    }

    pub async fn scripts(&self, org_id: &str, term: &str) -> Result<Vec<Script>, String> {
        let now = now_ms();
        let cached = self.scripts.lock().unwrap().get(org_id).map(|f| (f.value.clone(), f.is_fresh(SCRIPTS_STALE, now)));
        let collection = match cached {
            Some((c, true)) => c,
            other => {
                let fresh = sync_collection(other.map(|(c, _)| c), now, || self.core.fetch_all_scripts(org_id), |since| async move { self.core.fetch_scripts_changed_since(org_id, &since).await }).await?;
                self.scripts.lock().unwrap().insert(org_id.to_string(), Fresh { value: fresh.clone(), checked_at: now });
                fresh
            }
        };
        Ok(search_scripts(&collection.items, term))
    }

    pub async fn tags(&self, term: &str, prefer_org: Option<&str>) -> Result<Vec<Tag>, String> {
        let now = now_ms();
        let cached = self.tags.lock().unwrap().as_ref().map(|f| (f.value.clone(), f.is_fresh(TAGS_STALE, now)));
        let collection = match cached {
            Some((c, true)) => c,
            other => {
                let fresh = sync_collection(other.map(|(c, _)| c), now, || self.core.fetch_all_tags(), |since| async move { self.core.fetch_tags_changed_since(&since).await }).await?;
                *self.tags.lock().unwrap() = Some(Fresh { value: fresh.clone(), checked_at: now });
                fresh
            }
        };
        Ok(search_tags(&collection.items, term, prefer_org))
    }

    /// Devices can number in the thousands, so they're searched on the server, with each search cached.
    pub async fn assets(&self, org_id: &str, term: &str) -> Result<Vec<Asset>, String> {
        let key = (org_id.to_string(), term.trim().to_string());
        let now = now_ms();
        if let Some(hit) = self.assets.lock().unwrap().get(&key).filter(|f| f.is_fresh(QUERY_STALE, now)) {
            return Ok(hit.value.clone());
        }
        let found = self.core.search_assets(org_id, &key.1).await?;
        self.assets.lock().unwrap().insert(key, Fresh { value: found.clone(), checked_at: now });
        Ok(found)
    }

    /// Every device carrying a tag (up to the API paging limit).
    pub async fn tag_members(&self, org_id: &str, tag_id: &str) -> Result<Devices, String> {
        let key = (org_id.to_string(), tag_id.to_string());
        let now = now_ms();
        if let Some(hit) = self.members.lock().unwrap().get(&key).filter(|f| f.is_fresh(QUERY_STALE, now)) {
            return Ok(hit.value.clone());
        }
        let devices = self.core.find_all_assets(org_id, &serde_json::json!({ "tags": { "id": { "in": [tag_id] } } })).await?;
        self.members.lock().unwrap().insert(key, Fresh { value: devices.clone(), checked_at: now });
        Ok(devices)
    }

    /// Organizations of one category (for choosing where a tag can be used).
    pub async fn organizations_in(&self, term: &str, category: AccessCategory) -> Result<Vec<Organization>, String> {
        self.organizations("").await?; // makes sure the full directory is loaded and fresh
        let items = self.orgs.lock().unwrap().as_ref().map(|f| f.value.items.clone()).unwrap_or_default();
        Ok(search_organizations_in(&items, term, category))
    }

    /// Every tag matching `term`, not just the first page of results (for the Tags page).
    pub async fn all_tags(&self, term: &str) -> Result<Vec<Tag>, String> {
        self.tags("", None).await?; // makes sure the full list is loaded and fresh
        let items = self.tags.lock().unwrap().as_ref().map(|f| f.value.items.clone()).unwrap_or_default();
        Ok(items.into_iter().filter(|t| matches(term, &[Some(&t.name)])).collect())
    }

    /// One tag in full (a dynamic tag's rule is only on the full record).
    pub async fn tag(&self, id: &str) -> Result<Option<Tag>, String> {
        let now = now_ms();
        if let Some(hit) = self.tag_details.lock().unwrap().get(id).filter(|f| f.is_fresh(QUERY_STALE, now)) {
            return Ok(hit.value.clone());
        }
        let tag = self.core.get_tag(id).await?;
        self.tag_details.lock().unwrap().insert(id.to_string(), Fresh { value: tag.clone(), checked_at: now });
        Ok(tag)
    }

    /// A tag was created or edited: put the fresh copy in the list straight away.
    pub fn cache_tag(&self, tag: Tag) {
        let now = now_ms();
        if let Some(fresh) = self.tags.lock().unwrap().as_mut() {
            let items = &mut fresh.value.items;
            items.retain(|t| t.id != tag.id);
            items.push(tag.clone());
            items.sort_by(by_name);
        }
        self.tag_details.lock().unwrap().insert(tag.id.clone(), Fresh { value: Some(tag), checked_at: now });
    }

    pub fn uncache_tag(&self, tag_id: &str) {
        if let Some(fresh) = self.tags.lock().unwrap().as_mut() {
            fresh.value.items.retain(|t| t.id != tag_id);
        }
        self.tag_details.lock().unwrap().remove(tag_id);
        self.members.lock().unwrap().retain(|(_, id), _| id != tag_id);
    }

    /// Membership changed: forget the member lists and refetch the tag (for its device count).
    pub async fn refresh_membership(&self, tag_id: &str) {
        self.members.lock().unwrap().retain(|(_, id), _| id != tag_id);
        self.tag_details.lock().unwrap().remove(tag_id);
        if let Ok(Some(fresh)) = self.tag(tag_id).await {
            self.cache_tag(fresh);
        }
    }

    /// Pre-fills the cache, as if it had just been loaded. For tests of code built on it.
    #[doc(hidden)]
    pub fn seed(&self, orgs: Vec<Organization>, tags: Vec<Tag>, scripts: Vec<(&str, Vec<Script>)>) {
        let now = now_ms();
        *self.orgs.lock().unwrap() = Some(Fresh { value: OrgDirectory { items: orgs, full_at: now }, checked_at: now });
        let mut sorted = tags;
        sorted.sort_by(by_name);
        let cursor = max_updated_at(&sorted);
        *self.tags.lock().unwrap() = Some(Fresh { value: Collection { items: sorted, cursor, full_at: now }, checked_at: now });
        for (org, items) in scripts {
            let cursor = max_updated_at(&items);
            self.scripts.lock().unwrap().insert(org.to_string(), Fresh { value: Collection { items, cursor, full_at: now }, checked_at: now });
        }
    }

    #[doc(hidden)]
    pub fn seed_assets(&self, org_id: &str, term: &str, assets: Vec<Asset>) {
        self.assets.lock().unwrap().insert((org_id.to_string(), term.trim().to_string()), Fresh { value: assets, checked_at: now_ms() });
    }

    #[doc(hidden)]
    pub fn seed_members(&self, org_id: &str, tag_id: &str, assets: Vec<Asset>, total: i64) {
        self.members.lock().unwrap().insert((org_id.to_string(), tag_id.to_string()), Fresh { value: Devices { assets, total_count: total }, checked_at: now_ms() });
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Item {
        id: String,
        name: String,
        updated: Option<String>,
    }
    impl Synced for Item {
        fn id(&self) -> &str {
            &self.id
        }
        fn name(&self) -> &str {
            &self.name
        }
        fn updated_at(&self) -> Option<&str> {
            self.updated.as_deref()
        }
    }
    fn item(id: &str, name: &str, updated: &str) -> Item {
        Item { id: id.into(), name: name.into(), updated: Some(updated.into()) }
    }
    fn collection(items: Vec<Item>, full_at: i64) -> Collection<Item> {
        let cursor = max_updated_at(&items);
        Collection { items, cursor, full_at }
    }

    /// Runs a sync with fake fetchers and reports which ones were used.
    async fn run(prev: Option<Collection<Item>>, now: i64, all: Vec<Item>, delta: (Vec<Item>, i64)) -> (Collection<Item>, bool, Option<String>) {
        let full_used = Cell::new(false);
        let since_used = std::cell::RefCell::new(None);
        let got = sync_collection(
            prev,
            now,
            || async {
                full_used.set(true);
                Ok(all)
            },
            |since| {
                *since_used.borrow_mut() = Some(since);
                async { Ok(delta) }
            },
        )
        .await
        .unwrap();
        (got, full_used.get(), since_used.into_inner())
    }

    #[tokio::test]
    async fn the_first_load_is_a_full_load_sorted_by_name() {
        let (c, full, since) = run(None, 1000, vec![item("2", "bravo", "2026-10-02T00:00:00Z"), item("1", "Alpha", "2026-10-01T00:00:00Z")], (vec![], 0)).await;
        assert!(full && since.is_none());
        assert_eq!(c.items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["Alpha", "bravo"]);
        assert_eq!(c.cursor.as_deref(), Some("2026-10-02T00:00:00Z"));
        assert_eq!(c.full_at, 1000);
    }

    #[tokio::test]
    async fn after_that_only_what_changed_is_asked_for_with_a_small_overlap() {
        let prev = collection(vec![item("1", "Alpha", "2026-10-01T00:00:10Z")], 0);
        let (c, full, since) = run(Some(prev), 1000, vec![], (vec![item("2", "Bravo", "2026-10-02T00:00:00Z")], 2)).await;
        assert!(!full);
        assert_eq!(since.as_deref(), Some("2026-10-01T00:00:08.000Z"));
        assert_eq!(c.items.len(), 2);
        assert_eq!(c.cursor.as_deref(), Some("2026-10-02T00:00:00Z"));
        assert_eq!(c.full_at, 0, "a delta does not count as a full reload");
    }

    #[tokio::test]
    async fn a_count_mismatch_means_something_was_deleted_so_it_reloads_in_full() {
        let prev = collection(vec![item("1", "Alpha", "2026-10-01T00:00:00Z"), item("2", "Bravo", "2026-10-01T00:00:00Z")], 0);
        // Nothing changed, yet the server now reports one item fewer.
        let (c, full, _) = run(Some(prev), 1000, vec![item("1", "Alpha", "2026-10-01T00:00:00Z")], (vec![], 1)).await;
        assert!(full);
        assert_eq!(c.items.len(), 1);
    }

    #[tokio::test]
    async fn a_full_page_of_changes_may_be_cut_short_so_it_reloads_in_full() {
        let prev = collection(vec![item("0", "Zed", "2026-10-01T00:00:00Z")], 0);
        let changed: Vec<Item> = (1..=DELTA_LIMIT).map(|i| item(&i.to_string(), &format!("n{i}"), "2026-10-02T00:00:00Z")).collect();
        let everything = changed.iter().cloned().chain([item("0", "Zed", "2026-10-01T00:00:00Z")]).collect();
        let (c, full, _) = run(Some(prev), 1000, everything, (changed, 101)).await;
        assert!(full, "moving the cursor past a cut-short page would skip the rest for good");
        assert_eq!(c.items.len(), DELTA_LIMIT + 1);
    }

    #[tokio::test]
    async fn nothing_changed_keeps_the_same_collection() {
        let prev = collection(vec![item("1", "Alpha", "2026-10-01T00:00:00Z")], 5);
        let (c, full, _) = run(Some(prev.clone()), 1000, vec![], (vec![], 1)).await;
        assert!(!full);
        assert_eq!((c.items, c.cursor, c.full_at), (prev.items, prev.cursor, 5));
    }

    #[tokio::test]
    async fn a_full_reload_happens_every_six_hours_regardless() {
        let prev = collection(vec![item("1", "Alpha", "2026-10-01T00:00:00Z")], 0);
        let (_, full, since) = run(Some(prev), FULL_RELOAD_EVERY + 1, vec![item("1", "Alpha", "2026-10-01T00:00:00Z")], (vec![], 1)).await;
        assert!(full && since.is_none());
    }

    #[tokio::test]
    async fn an_empty_collection_has_no_cursor_so_it_is_loaded_in_full() {
        let prev = Collection::<Item> { items: vec![], cursor: None, full_at: 0 };
        let (_, full, _) = run(Some(prev), 1000, vec![], (vec![], 0)).await;
        assert!(full);
    }

    fn org(id: &str, name: &str) -> Organization {
        Organization { id: id.into(), name: name.into(), typename: "Customer".into() }
    }

    #[tokio::test]
    async fn organizations_reload_only_when_the_count_changes() {
        let prev = OrgDirectory { items: vec![org("1", "Acme"), org("2", "Beta")], full_at: 0 };
        let loaded = Cell::new(false);
        let same = refresh_organizations(Some(prev.clone()), 1000, || async { Ok(2) }, || async { loaded.set(true); Ok(vec![]) }).await.unwrap();
        assert!(!loaded.get() && same.items.len() == 2);
        let changed = refresh_organizations(Some(prev), 1000, || async { Ok(3) }, || async { Ok(vec![org("3", "Zed"), org("1", "Acme"), org("2", "Beta")]) }).await.unwrap();
        assert_eq!(changed.items.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["Acme", "Beta", "Zed"]);
    }

    fn script(id: &str, name: &str, description: Option<&str>) -> Script {
        Script { id: id.into(), name: name.into(), updated_at: None, language: "POWERSHELL".into(), description: description.map(String::from), timeout_seconds: None, inputs: None }
    }

    fn tag_named(id: &str, name: &str) -> Tag {
        Tag { id: id.into(), name: name.into(), typename: "StaticTag".into(), asset_count: Some(0), updated_at: None, description: None, organization: None, filter: None }
    }

    fn collections() -> Arc<Collections> {
        Collections::new(Core::for_tests("collections"))
    }

    #[tokio::test]
    async fn the_tags_page_lists_every_tag_not_just_a_pickers_first_fifty() {
        let c = collections();
        c.seed(vec![], (0..80).map(|i| tag_named(&format!("t{i}"), &format!("Tag {i:02}"))).collect(), vec![]);
        assert_eq!(c.all_tags("").await.unwrap().len(), 80);
        assert_eq!(c.tags("", None).await.unwrap().len(), RESULTS, "a picker is still limited");
        assert_eq!(c.all_tags("tag 7").await.unwrap().len(), 10);
    }

    #[tokio::test]
    async fn a_created_or_edited_tag_appears_in_the_list_at_once_in_name_order() {
        let c = collections();
        c.seed(vec![], vec![tag_named("1", "Alpha"), tag_named("3", "Charlie")], vec![]);
        c.cache_tag(tag_named("2", "bravo"));
        let names = |v: Vec<Tag>| v.into_iter().map(|t| t.name).collect::<Vec<_>>();
        assert_eq!(names(c.all_tags("").await.unwrap()), ["Alpha", "bravo", "Charlie"]);
        c.cache_tag(Tag { name: "Zulu".into(), ..tag_named("1", "x") });
        assert_eq!(names(c.all_tags("").await.unwrap()), ["bravo", "Charlie", "Zulu"], "the same tag replaced, not duplicated");
        // And its full record is cached, so opening it needs no request.
        assert_eq!(c.tag("1").await.unwrap().unwrap().name, "Zulu");
    }

    #[tokio::test]
    async fn a_deleted_tag_leaves_the_list_and_forgets_its_members() {
        let c = collections();
        c.seed(vec![], vec![tag_named("1", "Alpha"), tag_named("2", "Bravo")], vec![]);
        c.seed_members("o1", "1", vec![Asset::stub("a", "PC")], 1);
        c.cache_tag(tag_named("1", "Alpha"));
        c.uncache_tag("1");
        assert_eq!(c.all_tags("").await.unwrap().len(), 1);
        assert!(c.members.lock().unwrap().is_empty());
        assert!(c.tag_details.lock().unwrap().get("1").is_none());
    }

    #[test]
    fn organizations_sort_into_the_categories_a_tag_can_be_scoped_to() {
        let org = |id: &str, name: &str, kind: &str| Organization { id: id.into(), name: name.into(), typename: kind.into() };
        let all = [org("1", "MSP", "ServiceOrganization"), org("2", "Acme", "Customer"), org("3", "Acme HQ", "Site"), org("4", "Reseller", "Partner"), org("5", "Odd", "SomethingNew")];
        let names = |c| search_organizations_in(&all, "", c).into_iter().map(|o| o.name).collect::<Vec<_>>();
        assert_eq!(names(AccessCategory::Organization), ["MSP", "Reseller"]);
        assert_eq!(names(AccessCategory::Customer), ["Acme"]);
        assert_eq!(names(AccessCategory::Site), ["Acme HQ"]);
        assert_eq!(search_organizations_in(&all, "hq", AccessCategory::Site).len(), 1, "the term still filters");
        assert_eq!(AccessCategory::of(&all[3]), AccessCategory::Organization);
        assert_eq!(AccessCategory::of(&all[2]), AccessCategory::Site);
        assert_eq!(AccessCategory::of(&all[4]), AccessCategory::Organization, "an unknown type is treated as an organization");
        assert_eq!(AccessCategory::Site.next(), AccessCategory::Organization);
    }

    #[test]
    fn script_search_looks_at_names_and_descriptions_as_substrings() {
        let scripts = [script("1", "BitlockerPreChecker", None), script("2", "App Usage Tracker", Some("This will track the use of apps")), script("3", "Cleanup", None)];
        let names = |term: &str| search_scripts(&scripts, term).iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("bit"), ["BitlockerPreChecker"], "a substring, not a whole word");
        assert_eq!(names("track"), ["App Usage Tracker"]);
        assert_eq!(names("USE"), ["App Usage Tracker"], "descriptions count, case does not");
        assert_eq!(names("").len(), 3);
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn tag_search_puts_the_current_organizations_tags_first() {
        let tag = |id: &str, org_id: &str| Tag { id: id.into(), name: id.into(), typename: "StaticTag".into(), asset_count: None, updated_at: None, description: None, organization: Some(crate::model::OrgRef { id: org_id.into(), name: org_id.into() }), filter: None };
        let tags = [tag("a", "o2"), tag("b", "o1"), tag("c", "o2")];
        assert_eq!(search_tags(&tags, "", Some("o1")).iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["b", "a", "c"]);
        assert_eq!(search_tags(&tags, "", None).len(), 3);
        assert_eq!(search_tags(&tags, "a", Some("o1")).len(), 1);
    }

    #[test]
    fn a_picker_never_shows_more_than_the_result_limit() {
        let orgs: Vec<Organization> = (0..80).map(|i| org(&i.to_string(), &format!("Org {i}"))).collect();
        assert_eq!(search_organizations(&orgs, "").len(), RESULTS);
    }
}
