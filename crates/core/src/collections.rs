//! Cached lists for the pickers. A port of `collections.ts`: small reference
//! lists (organizations, tags, scripts) are held in full and searched locally;
//! tags and scripts refresh by delta (only what changed since the last sync).
//! Device lists and tag members are cached per query, in memory for the
//! session. The reference lists are also kept (encrypted) between runs for up
//! to a day, so a new session starts with a delta, not a full load.

use std::{collections::HashMap, future::Future, sync::{Arc, Mutex}};

use chrono::Utc;
use serde::{Deserialize, Serialize};

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
/// After N-able refuses a request as rate limited, don't ask for the same thing again for this long.
const RATE_LIMIT_COOLDOWN: i64 = 15_000;

/// The encrypted file the lists are kept in between runs.
pub(crate) const CACHE_NAME: &str = "collections-cache";
/// A saved copy older than this is ignored and the lists are loaded in full.
const CACHE_MAX_AGE: i64 = 24 * HOUR;
const CACHE_VERSION: u32 = 1;

/// The reference lists as saved to disk. Device lists and tag members are left out: they change too often to be worth keeping.
#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    saved_at: i64,
    /// The API the lists came from; a different endpoint means a different set of data.
    endpoint: String,
    orgs: Option<OrgDirectory>,
    tags: Option<Collection<Tag>>,
    scripts: HashMap<String, Collection<Script>>,
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    if let Some(prev) = prev
        && now - prev.full_at < FULL_RELOAD_EVERY
        && count().await? == prev.items.len() as i64
    {
        return Ok(prev);
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

/// A cache key made of an organization id and a second id (script or tag).
type OrgKey = (String, String);

pub struct Collections {
    core: Arc<Core>,
    orgs: Mutex<Option<Fresh<OrgDirectory>>>,
    tags: Mutex<Option<Fresh<Collection<Tag>>>>,
    scripts: Mutex<HashMap<String, Fresh<Collection<Script>>>>,
    assets: Mutex<HashMap<OrgKey, Fresh<Vec<Asset>>>>,
    members: Mutex<HashMap<OrgKey, Fresh<Devices>>>,
    tag_details: Mutex<HashMap<String, Fresh<Option<Tag>>>>,
    /// One list refresh at a time. A picker searches on every keystroke, so without this a cold or
    /// stale list would be fetched once per key typed, all at once.
    refresh: tokio::sync::Mutex<()>,
    /// What was last refused as rate limited, by what was asked for: when, and the error to repeat.
    cooling: Mutex<HashMap<String, (i64, String)>>,
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
            refresh: tokio::sync::Mutex::new(()),
            cooling: Mutex::new(HashMap::new()),
        })
    }

    /// Starts from the lists saved by an earlier run, so the first picker only has to ask what changed.
    /// A missing, unreadable, old or foreign copy is ignored. Entries count as stale, so the next use
    /// refreshes them (by delta where it can) before showing anything.
    pub async fn load_persisted(&self) {
        let Ok(Some(text)) = self.core.store_get(CACHE_NAME).await else { return };
        let Ok(snapshot) = serde_json::from_str::<Snapshot>(&text) else { return };
        let now = now_ms();
        if snapshot.version != CACHE_VERSION || now - snapshot.saved_at > CACHE_MAX_AGE || snapshot.saved_at > now + MINUTE || snapshot.endpoint != self.core.api_url() {
            return;
        }
        // Whatever the session already fetched is newer than the file.
        let mut orgs = self.orgs.lock().unwrap();
        if orgs.is_none() {
            *orgs = snapshot.orgs.map(|value| Fresh { value, checked_at: 0 });
        }
        let mut tags = self.tags.lock().unwrap();
        if tags.is_none() {
            *tags = snapshot.tags.map(|value| Fresh { value, checked_at: 0 });
        }
        let mut scripts = self.scripts.lock().unwrap();
        for (org, value) in snapshot.scripts {
            scripts.entry(org).or_insert(Fresh { value, checked_at: 0 });
        }
    }

    /// Saves the lists for the next run. A failure only costs a full load next time.
    async fn persist(&self) {
        let snapshot = Snapshot {
            version: CACHE_VERSION,
            saved_at: now_ms(),
            endpoint: self.core.api_url(),
            orgs: self.orgs.lock().unwrap().as_ref().map(|f| f.value.clone()),
            tags: self.tags.lock().unwrap().as_ref().map(|f| f.value.clone()),
            scripts: self.scripts.lock().unwrap().iter().map(|(k, f)| (k.clone(), f.value.clone())).collect(),
        };
        if let Ok(json) = serde_json::to_string(&snapshot) {
            let _ = self.core.store_set(CACHE_NAME, json).await;
        }
    }

    /// Forgets everything held, in memory and on disk (another account or endpoint, or the data was cleared).
    pub async fn forget(&self) {
        *self.orgs.lock().unwrap() = None;
        *self.tags.lock().unwrap() = None;
        self.scripts.lock().unwrap().clear();
        self.assets.lock().unwrap().clear();
        self.members.lock().unwrap().clear();
        self.tag_details.lock().unwrap().clear();
        self.cooling.lock().unwrap().clear();
        let _ = self.core.store_remove(CACHE_NAME).await;
    }

    /// The error to repeat, if what `key` names was refused as rate limited a moment ago.
    fn cooling_down(&self, key: &str, now: i64) -> Option<String> {
        self.cooling.lock().unwrap().get(key).filter(|(at, _)| now - at < RATE_LIMIT_COOLDOWN).map(|(_, error)| error.clone())
    }

    /// Remembers a rate-limit refusal for `key` (and forgets it on success). Other errors aren't held against it.
    fn note_outcome<T>(&self, key: &str, result: &Result<T, String>) {
        let mut cooling = self.cooling.lock().unwrap();
        match result {
            Ok(_) => {
                cooling.remove(key);
            }
            Err(error) if crate::is_rate_limited(error) => {
                cooling.insert(key.to_string(), (now_ms(), error.clone()));
            }
            Err(_) => {}
        }
    }

    async fn org_directory(&self) -> Result<OrgDirectory, String> {
        let fresh_copy = || {
            let now = now_ms();
            self.orgs.lock().unwrap().as_ref().filter(|f| f.is_fresh(ORGS_STALE, now)).map(|f| f.value.clone())
        };
        if let Some(dir) = fresh_copy() {
            return Ok(dir);
        }
        let _turn = self.refresh.lock().await;
        // A search typed a moment ago may have refreshed it while this one waited.
        if let Some(dir) = fresh_copy() {
            return Ok(dir);
        }
        let now = now_ms();
        if let Some(error) = self.cooling_down("orgs", now) {
            return Err(error);
        }
        let prev = self.orgs.lock().unwrap().as_ref().map(|f| f.value.clone());
        let result = refresh_organizations(prev, now, || self.core.count_organizations(), || self.core.fetch_all_organizations()).await;
        self.note_outcome("orgs", &result);
        let dir = result?;
        *self.orgs.lock().unwrap() = Some(Fresh { value: dir.clone(), checked_at: now });
        self.persist().await;
        Ok(dir)
    }

    pub async fn organizations(&self, term: &str) -> Result<Vec<Organization>, String> {
        Ok(search_organizations(&self.org_directory().await?.items, term))
    }

    pub async fn scripts(&self, org_id: &str, term: &str) -> Result<Vec<Script>, String> {
        let fresh_copy = || {
            let now = now_ms();
            self.scripts.lock().unwrap().get(org_id).filter(|f| f.is_fresh(SCRIPTS_STALE, now)).map(|f| f.value.clone())
        };
        if let Some(c) = fresh_copy() {
            return Ok(search_scripts(&c.items, term));
        }
        let _turn = self.refresh.lock().await;
        if let Some(c) = fresh_copy() {
            return Ok(search_scripts(&c.items, term));
        }
        let (now, key) = (now_ms(), format!("scripts:{org_id}"));
        if let Some(error) = self.cooling_down(&key, now) {
            return Err(error);
        }
        let prev = self.scripts.lock().unwrap().get(org_id).map(|f| f.value.clone());
        let result = sync_collection(prev, now, || self.core.fetch_all_scripts(org_id), |since| async move { self.core.fetch_scripts_changed_since(org_id, &since).await }).await;
        self.note_outcome(&key, &result);
        let fresh = result?;
        self.scripts.lock().unwrap().insert(org_id.to_string(), Fresh { value: fresh.clone(), checked_at: now });
        self.persist().await;
        Ok(search_scripts(&fresh.items, term))
    }

    pub async fn tags(&self, term: &str, prefer_org: Option<&str>) -> Result<Vec<Tag>, String> {
        let fresh_copy = || {
            let now = now_ms();
            self.tags.lock().unwrap().as_ref().filter(|f| f.is_fresh(TAGS_STALE, now)).map(|f| f.value.clone())
        };
        if let Some(c) = fresh_copy() {
            return Ok(search_tags(&c.items, term, prefer_org));
        }
        let _turn = self.refresh.lock().await;
        if let Some(c) = fresh_copy() {
            return Ok(search_tags(&c.items, term, prefer_org));
        }
        let now = now_ms();
        if let Some(error) = self.cooling_down("tags", now) {
            return Err(error);
        }
        let prev = self.tags.lock().unwrap().as_ref().map(|f| f.value.clone());
        let result = sync_collection(prev, now, || self.core.fetch_all_tags(), |since| async move { self.core.fetch_tags_changed_since(&since).await }).await;
        self.note_outcome("tags", &result);
        let fresh = result?;
        *self.tags.lock().unwrap() = Some(Fresh { value: fresh.clone(), checked_at: now });
        self.persist().await;
        Ok(search_tags(&fresh.items, term, prefer_org))
    }

    /// Devices can number in the thousands, so they're searched on the server, with each search cached.
    pub async fn assets(&self, org_id: &str, term: &str) -> Result<Vec<Asset>, String> {
        let key = (org_id.to_string(), term.trim().to_string());
        let now = now_ms();
        if let Some(hit) = self.assets.lock().unwrap().get(&key).filter(|f| f.is_fresh(QUERY_STALE, now)) {
            return Ok(hit.value.clone());
        }
        let cool_key = format!("assets:{}:{}", key.0, key.1);
        if let Some(error) = self.cooling_down(&cool_key, now) {
            return Err(error);
        }
        let result = self.core.search_assets(org_id, &key.1).await;
        self.note_outcome(&cool_key, &result);
        let found = result?;
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

    // ---- the copy kept between runs ----

    async fn saved_lists(core: &Arc<Core>) -> Arc<Collections> {
        let first = Collections::new(core.clone());
        first.seed(vec![org("1", "Acme")], vec![tag_named("t1", "Servers")], vec![("1", vec![script("s1", "Cleanup", None)])]);
        first.persist().await;
        first
    }

    #[tokio::test]
    async fn the_lists_come_back_in_the_next_session_as_stale_so_they_refresh_by_delta() {
        let core = Core::for_tests("persist");
        saved_lists(&core).await;

        let next = Collections::new(core);
        assert!(next.tags.lock().unwrap().is_none());
        next.load_persisted().await;
        let tags = next.tags.lock().unwrap();
        let tags = tags.as_ref().expect("tags were restored");
        assert_eq!(tags.value.items[0].name, "Servers");
        assert_eq!(tags.checked_at, 0, "restored lists are checked again before they are trusted");
        assert!(next.orgs.lock().unwrap().is_some() && next.scripts.lock().unwrap().contains_key("1"));
    }

    #[tokio::test]
    async fn a_copy_that_is_too_old_or_from_another_endpoint_is_ignored() {
        let core = Core::for_tests("persist-old");
        let first = saved_lists(&core).await;

        // Too old: rewrite the file with an old timestamp.
        let text = core.store_get(CACHE_NAME).await.unwrap().unwrap();
        let mut snapshot: Snapshot = serde_json::from_str(&text).unwrap();
        snapshot.saved_at -= CACHE_MAX_AGE + 1;
        core.store_set(CACHE_NAME, serde_json::to_string(&snapshot).unwrap()).await.unwrap();
        let next = Collections::new(core.clone());
        next.load_persisted().await;
        assert!(next.tags.lock().unwrap().is_none(), "older than a day");

        // Another endpoint: the same lists, saved for a different API.
        snapshot.saved_at += CACHE_MAX_AGE + 1;
        snapshot.endpoint = "https://other.example/graphql".into();
        core.store_set(CACHE_NAME, serde_json::to_string(&snapshot).unwrap()).await.unwrap();
        let next = Collections::new(core.clone());
        next.load_persisted().await;
        assert!(next.tags.lock().unwrap().is_none(), "a different endpoint is different data");

        // Damaged contents are not an error either.
        core.store_set(CACHE_NAME, "not json".into()).await.unwrap();
        Collections::new(core).load_persisted().await;
        drop(first);
    }

    #[tokio::test]
    async fn what_this_session_already_fetched_is_not_overwritten_by_the_file() {
        let core = Core::for_tests("persist-newer");
        saved_lists(&core).await;
        let next = Collections::new(core);
        next.seed(vec![], vec![tag_named("t9", "Newer")], vec![]);
        next.load_persisted().await;
        assert_eq!(next.tags.lock().unwrap().as_ref().unwrap().value.items[0].name, "Newer");
    }

    #[tokio::test]
    async fn after_a_rate_limit_refusal_the_same_ask_is_not_repeated_straight_away() {
        let core = Core::for_tests("cooling");
        let c = Collections::new(core);
        let refused = "HTTP 429: N-able is rate limiting requests. Wait a moment and try again.";
        c.cooling.lock().unwrap().insert("orgs".into(), (now_ms(), refused.into()));
        c.cooling.lock().unwrap().insert("tags".into(), (now_ms() - RATE_LIMIT_COOLDOWN - 1, refused.into()));
        // Cooling down: the refusal is repeated and nothing is sent (a real send would fail to connect instead).
        assert_eq!(c.organizations("a").await.unwrap_err(), refused);
        // Cooled off: it asks again (and here fails to connect, since nothing listens).
        assert_ne!(c.tags("a", None).await.unwrap_err(), refused);
    }

    #[test]
    fn only_rate_limit_refusals_are_held_against_a_request() {
        let c = Collections::new(Core::for_tests("outcome"));
        c.note_outcome::<()>("k", &Err("Request to x failed".into()));
        assert!(c.cooling_down("k", now_ms()).is_none());
        c.note_outcome::<()>("k", &Err("HTTP 429: N-able is rate limiting requests.".into()));
        assert!(c.cooling_down("k", now_ms()).is_some());
        assert!(c.cooling_down("k", now_ms() + RATE_LIMIT_COOLDOWN).is_none());
        c.note_outcome("k", &Ok(()));
        assert!(c.cooling_down("k", now_ms()).is_none());
    }

    #[tokio::test]
    async fn forgetting_clears_memory_and_the_saved_copy() {
        let core = Core::for_tests("persist-forget");
        let first = saved_lists(&core).await;
        first.forget().await;
        assert!(first.tags.lock().unwrap().is_none() && core.store_get(CACHE_NAME).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn changing_the_key_or_endpoint_drops_the_saved_copy_but_changing_history_does_not() {
        let core = Core::for_tests("persist-settings");
        saved_lists(&core).await;
        let url = core.api_url();
        core.save_settings(url.clone(), None, Some(7)).await.unwrap();
        assert!(core.store_get(CACHE_NAME).await.unwrap().is_some(), "history length says nothing about the data");
        core.save_settings("https://other.example/graphql".into(), None, None).await.unwrap();
        assert!(core.store_get(CACHE_NAME).await.unwrap().is_none(), "another endpoint means other data");
    }
}
