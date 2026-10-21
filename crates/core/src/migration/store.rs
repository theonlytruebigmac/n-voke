//! Saving and listing migration projects. Each project is its own encrypted store
//! (`migration-<id>`), kept until the user deletes it: the run-history retention
//! setting doesn't apply, because a migration can span weeks.
//!
//! Only one app should drive a project at a time. A small lease (instance and
//! heartbeat) in its own store says who; the other app opens the project
//! read-only. The lease is advisory: it stops the common case of two windows
//! open, not two writers racing within the same second.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    sync::{Arc, Mutex, OnceLock},
};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::project::{DestEcoverse, Project, Stage, stamp};
use crate::{Core, audit::AuditEntry};

const PROJECT_PREFIX: &str = "migration-";
const LEASE_PREFIX: &str = "migration-lease-";
/// A lease not renewed for this long is free to take.
pub const LEASE_TTL: Duration = Duration::seconds(120);

/// What the project list shows.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ProjectSummary.ts"))]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub source_server_id: String,
    pub dest_server_id: String,
    pub updated_at: String,
    pub devices: usize,
    pub attention: usize,
    pub closed: bool,
    pub stage_counts: BTreeMap<Stage, usize>,
    /// Healthy in N-central but not yet sending asset data to Ecoverse: registered, not fully working.
    pub ecoverse_waiting: usize,
}

impl ProjectSummary {
    pub fn of(project: &Project) -> Self {
        Self {
            id: project.id.clone(),
            name: project.name.clone(),
            source_server_id: project.source_server_id.clone(),
            dest_server_id: project.dest_server_id.clone(),
            updated_at: project.updated_at.clone(),
            devices: project.devices.len(),
            attention: project.attention_count(),
            closed: project.is_closed(),
            stage_counts: project.stage_counts(),
            ecoverse_waiting: project.ecoverse_waiting(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    pub instance: String,
    pub heartbeat: String,
}

/// Whether `instance` may hold the lease now: it's free, expired, unreadable, or already its own.
pub fn lease_available(existing: Option<&Lease>, instance: &str, now: DateTime<Utc>) -> bool {
    match existing {
        None => true,
        Some(lease) if lease.instance == instance => true,
        Some(lease) => DateTime::parse_from_rfc3339(&lease.heartbeat).map_or(true, |beat| now - beat.with_timezone(&Utc) > LEASE_TTL),
    }
}

/// The modified time and size of a file, which change whenever a project is saved.
type FileStamp = (std::time::SystemTime, u64);

fn file_stamp(path: &std::path::Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn project_cache() -> &'static Mutex<HashMap<String, (FileStamp, Arc<Project>)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (FileStamp, Arc<Project>)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The keychain account holding one project's own data key.
pub(crate) fn project_key_account(id: &str) -> String {
    format!("migration-key:{id}")
}

fn project_store(id: &str) -> String {
    format!("{PROJECT_PREFIX}{id}")
}

pub(crate) fn forget_all_projects() {
    project_cache().lock().unwrap().clear();
}

/// What the app keeps in memory for a project: the parsed copy, the plan, and what it read of the two servers.
fn forget_project_memory(id: &str, servers: Option<(&str, &str)>) {
    project_cache().lock().unwrap().retain(|path, _| !path.contains(&format!("{PROJECT_PREFIX}{id}.")));
    super::planview::forget_plan(id);
    if let Some((source, dest)) = servers {
        for server in [source, dest] {
            super::readcache::forget_server(server);
            super::trackview::forget_names(server);
        }
    }
}

fn lease_store(id: &str) -> String {
    format!("{LEASE_PREFIX}{id}")
}

impl Core {
    /// Creates and saves an empty project.
    pub async fn migration_create(&self, name: &str, source_server_id: &str, dest_server_id: &str, dest_ecoverse: DestEcoverse) -> Result<Project, String> {
        if name.trim().is_empty() {
            return Err("Give the project a name.".into());
        }
        let source = self.ncentral_server(source_server_id)?;
        let dest = self.ncentral_server(dest_server_id)?;
        let id = format!("mig-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let mut project = Project::new(id, name, &source.id, &dest.id, dest_ecoverse, Utc::now());
        project.options = self.migration_settings().defaults;
        self.migration_save(&mut project).await?;
        self.record(&AuditEntry::new(
            "migration.project.create",
            &format!("Created migration project {}", project.name),
            Ok(()),
            json!({ "project": project.id, "source": source.host, "destination": dest.host }),
        ));
        Ok(project)
    }

    /// Saves the project, unless what's stored is newer than the copy being saved (another window saved in between).
    pub async fn migration_save(&self, project: &mut Project) -> Result<(), String> {
        let name = project_store(&project.id);
        // What is stored comes from memory when the file hasn't changed, so a save doesn't decrypt and parse it again.
        let stored = self.migration_load_shared(&project.id).await.ok().flatten().map(|p| p.revision);
        // A project that has been saved before but isn't there now was deleted: saving it would bring it back, with a new key.
        if stored.is_none() && project.revision != 0 {
            return Err(format!("{} was deleted, so it can't be saved.", project.name));
        }
        if let Some(stored) = stored
            && stored != project.revision
        {
            return Err(format!("{} was changed in another N-voke window since this one loaded it, so this save was refused. Close and reopen the project to see what changed.", project.name));
        }
        project.revision += 1;
        let json = serde_json::to_string(&*project).map_err(|e| e.to_string())?;
        self.project_file_write(&project.id, json).await.map_err(|e| {
            project.revision -= 1;
            format!("Could not save the project: {e}")
        })?;
        // What was just saved is what the next screen will ask for.
        if let Ok(path) = self.paths.store_file(&name)
            && let Some(stamp) = file_stamp(&path)
        {
            project_cache().lock().unwrap().insert(path.display().to_string(), (stamp, Arc::new(project.clone())));
        }
        Ok(())
    }

    /// The project's own data key, made on first use when `create` is set. Held with the other secrets in the system
    /// keychain, so destroying it is what makes the project unrecoverable.
    async fn project_key(&self, id: &str, create: bool) -> Result<Option<Vec<u8>>, String> {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        let secrets = self.secrets.clone();
        let account = project_key_account(id);
        crate::blocking(move || {
            if let Some(stored) = secrets.get(&account)? {
                return B64.decode(stored.trim()).map(Some).map_err(|_| "The project's data key is corrupt.".to_string());
            }
            if !create {
                return Ok(None);
            }
            let key = crate::vault::Vault::new_own_key();
            secrets.set(&account, &B64.encode(&key))?;
            Ok(Some(key))
        })
        .await
    }

    /// The project file's text. A file sealed under the project's own key needs that key; one written before projects
    /// had their own (sealed under the app's data key) is read as it was and moves to its own key the next time it is saved.
    async fn project_file_read(&self, id: &str) -> Result<Option<String>, String> {
        let path = self.paths.store_file(&project_store(id))?;
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Couldn't read {}: {e}", path.display())),
        };
        if crate::vault::Vault::is_own_key(&bytes) {
            let key = self.project_key(id, false).await?.ok_or("This project's data key has been destroyed, so its data can't be read.")?;
            let text = crate::vault::Vault::open_own(&key, &bytes)?;
            return String::from_utf8(text).map(Some).map_err(|e| e.to_string());
        }
        self.store_get(&project_store(id)).await
    }

    async fn project_file_write(&self, id: &str, text: String) -> Result<(), String> {
        let path = self.paths.store_file(&project_store(id))?;
        let key = self.project_key(id, true).await?.ok_or("The project's data key could not be made.")?;
        crate::blocking(move || crate::vault::write_private(&path, &crate::vault::Vault::seal_own(&key, text.as_bytes())?)).await
    }

    pub async fn migration_load(&self, id: &str) -> Result<Option<Project>, String> {
        Ok(self.migration_load_shared(id).await?.map(|p| (*p).clone()))
    }

    /// The project as last saved, shared: for the screens that only look. Decrypting and parsing a project of thousands
    /// of devices takes a noticeable moment and every screen asks, so the parsed project is kept in memory for as long as
    /// its file is unchanged (the file's modified time and size are checked on each ask, so another window's save shows).
    pub async fn migration_load_shared(&self, id: &str) -> Result<Option<Arc<Project>>, String> {
        let path = self.paths.store_file(&project_store(id))?;
        let key = path.display().to_string();
        let stamp = file_stamp(&path);
        if let (Some(stamp), Some((kept, project))) = (stamp, project_cache().lock().unwrap().get(&key).cloned())
            && kept == stamp
        {
            return Ok(Some(project));
        }
        match self.project_file_read(id).await? {
            None => {
                project_cache().lock().unwrap().remove(&key);
                Ok(None)
            }
            Some(text) => {
                let project: Arc<Project> = Arc::new(serde_json::from_str(&text).map_err(|e| format!("The project {id} couldn't be read: {e}"))?);
                if let Some(stamp) = stamp {
                    project_cache().lock().unwrap().insert(key, (stamp, project.clone()));
                }
                Ok(Some(project))
            }
        }
    }

    /// Every saved project, most recently changed first. One that can't be read is skipped.
    pub async fn migration_list(&self) -> Result<Vec<ProjectSummary>, String> {
        let dir = self.paths.store_dir();
        let names: Vec<String> = match fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter_map(|f| f.strip_suffix(".enc").map(String::from))
                .filter(|n| n.starts_with(PROJECT_PREFIX) && !n.starts_with(LEASE_PREFIX))
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => return Err(format!("Couldn't read {}: {e}", dir.display())),
        };
        let mut summaries = Vec::new();
        for name in names {
            if let Some(project) = self.migration_load_shared(&name[PROJECT_PREFIX.len()..]).await.ok().flatten() {
                summaries.push(ProjectSummary::of(&project));
            }
        }
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.name.cmp(&b.name)));
        Ok(summaries)
    }

    /// One project's summary, for a header.
    pub async fn migration_summary(&self, id: &str) -> Result<ProjectSummary, String> {
        self.migration_load_shared(id).await?.map(|p| ProjectSummary::of(&p)).ok_or_else(|| "That project no longer exists.".to_string())
    }

    /// The name of a project that uses this server as its source or destination, if any. Such a server can't be
    /// removed: the project would lose it.
    pub async fn migration_project_using_server(&self, server_id: &str) -> Result<Option<String>, String> {
        Ok(self.migration_list().await?.into_iter().find(|p| p.source_server_id == server_id || p.dest_server_id == server_id).map(|p| p.name))
    }

    /// Destroys a project's data on this computer. In this order: the project's own data key is removed from the
    /// keychain (after which the file can't be read by anything, including a copy in a backup); the file is overwritten
    /// and deleted along with its lease and any leftover temporary or set-aside copy; and everything the app kept in
    /// memory for it is dropped. The audit log keeps its entries, which record that the project was deleted and name
    /// no devices. Nothing on the servers or the devices changes.
    pub async fn migration_delete(&self, id: &str) -> Result<(), String> {
        let project = self.migration_load_shared(id).await.ok().flatten();
        // A project that can't be read (its key is already gone) can still be removed.
        let name = project.as_ref().map_or_else(|| id.to_string(), |p| p.name.clone());
        let servers = project.as_ref().map(|p| (p.source_server_id.clone(), p.dest_server_id.clone()));
        let had_own_key = self.project_key(id, false).await.ok().flatten().is_some();
        let file_there = self.paths.store_file(&project_store(id)).is_ok_and(|p| p.exists());
        if project.is_none() && !had_own_key && !file_there {
            return Err("That project no longer exists.".into());
        }
        let result = self.destroy_project_files(id).await;
        forget_project_memory(id, servers.as_ref().map(|(a, b)| (a.as_str(), b.as_str())));
        let how = if had_own_key { "data key destroyed, file overwritten and removed" } else { "file overwritten and removed (a project from before projects had their own key)" };
        self.record(&AuditEntry::new("migration.project.delete", &format!("Deleted migration project {name}: {how}"), result.clone(), json!({ "project": id })));
        result
    }

    /// The key first, so that nothing that survives the overwrite can be read, then the files.
    async fn destroy_project_files(&self, id: &str) -> Result<(), String> {
        let secrets = self.secrets.clone();
        let account = project_key_account(id);
        crate::blocking(move || secrets.delete(&account)).await?;
        let dir = self.paths.store_dir();
        let prefixes = [format!("{PROJECT_PREFIX}{id}."), format!("{LEASE_PREFIX}{id}.")];
        crate::blocking(move || {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(format!("Couldn't read {}: {e}", dir.display())),
            };
            let mut first_error = None;
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if prefixes.iter().any(|p| name.starts_with(p.as_str()))
                    && let Err(e) = crate::vault::shred_file(&entry.path())
                {
                    first_error.get_or_insert(e);
                }
            }
            first_error.map_or(Ok(()), Err)
        })
        .await
    }

    /// Takes (or renews) the project's lease for this app instance. False means another
    /// instance holds it and this one should open the project read-only.
    pub async fn migration_lease(&self, id: &str, instance: &str) -> Result<bool, String> {
        let existing = match self.store_get(&lease_store(id)).await? {
            Some(text) => serde_json::from_str::<Lease>(&text).ok(),
            None => None,
        };
        let now = Utc::now();
        if !lease_available(existing.as_ref(), instance, now) {
            return Ok(false);
        }
        let lease = Lease { instance: instance.into(), heartbeat: stamp(now) };
        self.store_set(&lease_store(id), serde_json::to_string(&lease).map_err(|e| e.to_string())?).await?;
        Ok(true)
    }

    /// Gives the lease up on a clean exit, so the other app doesn't wait out the timeout.
    pub async fn migration_release_lease(&self, id: &str, instance: &str) -> Result<(), String> {
        if let Some(lease) = self.store_get(&lease_store(id)).await?.and_then(|t| serde_json::from_str::<Lease>(&t).ok())
            && lease.instance == instance
        {
            self.store_remove(&lease_store(id)).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn core_with_servers() -> (std::sync::Arc<Core>, String, String) {
        let core = Core::for_tests("projects");
        let src = core.save_ncentral_server(None, "Old", "old.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "new.example.com", Some("j2".into()), None).await.unwrap();
        (core, src.id, dst.id)
    }

    #[test]
    fn a_lease_is_free_expired_or_ours() {
        let now: DateTime<Utc> = "2026-10-20T12:00:00Z".parse().unwrap();
        let held = |secs_ago: i64, who: &str| Lease { instance: who.into(), heartbeat: stamp(now - Duration::seconds(secs_ago)) };
        assert!(lease_available(None, "a", now));
        assert!(lease_available(Some(&held(5, "a")), "a", now), "renewing our own");
        assert!(!lease_available(Some(&held(5, "b")), "a", now), "someone else, recently");
        assert!(lease_available(Some(&held(121, "b")), "a", now), "someone else, gone quiet");
        assert!(lease_available(Some(&Lease { instance: "b".into(), heartbeat: "garbage".into() }), "a", now), "a damaged lease can't lock a project");
    }

    #[tokio::test]
    async fn a_save_from_a_copy_that_is_out_of_date_is_refused_not_applied() {
        let (core, src, dst) = core_with_servers().await;
        let mut mine = core.migration_create("Acme move", &src, &dst, DestEcoverse::NotLinked).await.unwrap();
        let mut theirs = core.migration_load(&mine.id).await.unwrap().unwrap();
        theirs.name = "Renamed elsewhere".into();
        core.migration_save(&mut theirs).await.unwrap();

        mine.name = "My stale rename".into();
        let error = core.migration_save(&mut mine).await.unwrap_err();
        assert!(error.contains("changed in another N-voke window"), "{error}");
        assert_eq!(core.migration_load(&mine.id).await.unwrap().unwrap().name, "Renamed elsewhere");
        assert_eq!(mine.revision, 1, "a refused save doesn't advance the copy");

        let mut fresh = core.migration_load(&mine.id).await.unwrap().unwrap();
        fresh.name = "Now fine".into();
        core.migration_save(&mut fresh).await.unwrap();
        assert_eq!(fresh.revision, 3);
    }

    #[tokio::test]
    async fn projects_are_created_saved_listed_and_deleted() {
        let (core, src, dst) = core_with_servers().await;
        assert!(core.migration_list().await.unwrap().is_empty());
        assert!(core.migration_create("  ", &src, &dst, DestEcoverse::NotLinked).await.is_err());
        assert!(core.migration_create("x", "srv-nope", &dst, DestEcoverse::NotLinked).await.is_err());

        let mut project = core.migration_create("Acme move", &src, &dst, DestEcoverse::NotLinked).await.unwrap();
        assert!(project.id.starts_with("mig-"));
        project.updated_at = "2030-01-01T00:00:00Z".into();
        core.migration_save(&mut project).await.unwrap();
        let second = core.migration_create("Globex move", &src, &src, DestEcoverse::SameTenant).await.unwrap();

        assert_eq!(core.migration_load(&project.id).await.unwrap().unwrap(), project);
        assert!(core.migration_load("mig-missing").await.unwrap().is_none());
        let list = core.migration_list().await.unwrap();
        assert_eq!(list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Acme move", "Globex move"], "most recently changed first");
        assert_eq!((list[0].devices, list[0].closed), (0, false));

        core.migration_delete(&second.id).await.unwrap();
        assert_eq!(core.migration_list().await.unwrap().len(), 1);
        assert!(core.migration_delete(&second.id).await.is_err());
        let log = core.audit_log(None).unwrap().entries;
        assert!(log.iter().any(|e| e.action == "migration.project.delete" && e.summary.starts_with("Deleted migration project Globex move")));
    }

    #[tokio::test]
    async fn a_server_in_use_is_named_and_a_free_one_is_not() {
        let (core, src, dst) = core_with_servers().await;
        let other = core.save_ncentral_server(None, "Spare", "spare.example.com", Some("j3".into()), None).await.unwrap().id;
        let project = core.migration_create("Acme move", &src, &dst, DestEcoverse::NotLinked).await.unwrap();
        assert_eq!(core.migration_project_using_server(&src).await.unwrap().as_deref(), Some("Acme move"));
        assert_eq!(core.migration_project_using_server(&dst).await.unwrap().as_deref(), Some("Acme move"));
        assert_eq!(core.migration_project_using_server(&other).await.unwrap(), None);
        core.migration_delete(&project.id).await.unwrap();
        assert_eq!(core.migration_project_using_server(&src).await.unwrap(), None, "free again once the project is gone");
    }

    #[tokio::test]
    async fn a_project_is_parsed_once_until_its_file_changes_and_a_save_shows_at_once() {
        let (core, src, dst) = core_with_servers().await;
        let mut project = core.migration_create("Acme move", &src, &dst, DestEcoverse::NotLinked).await.unwrap();
        let first = core.migration_load_shared(&project.id).await.unwrap().unwrap();
        let again = core.migration_load_shared(&project.id).await.unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &again), "the second look reuses what was parsed");

        project.name = "Renamed".into();
        core.migration_save(&mut project).await.unwrap();
        let after_save = core.migration_load_shared(&project.id).await.unwrap().unwrap();
        assert_eq!(after_save.name, "Renamed");
        assert!(!Arc::ptr_eq(&first, &after_save));

        // Another window saving behind this one's back is seen, because the file's time and size changed.
        let mut other = (*after_save).clone();
        other.name = "Changed elsewhere, longer".into();
        core.store_set(&format!("migration-{}", project.id), serde_json::to_string(&other).unwrap()).await.unwrap();
        assert_eq!(core.migration_load_shared(&project.id).await.unwrap().unwrap().name, "Changed elsewhere, longer");

        core.migration_delete(&project.id).await.unwrap();
        assert!(core.migration_load_shared(&project.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn one_project_has_a_summary_and_a_missing_one_says_so() {
        let (core, src, dst) = core_with_servers().await;
        let project = core.migration_create("Acme move", &src, &dst, DestEcoverse::NotLinked).await.unwrap();
        let summary = core.migration_summary(&project.id).await.unwrap();
        assert_eq!((summary.name.as_str(), summary.devices), ("Acme move", 0));
        assert_eq!(core.migration_summary("mig-nothere").await.unwrap_err(), "That project no longer exists.");
    }

    #[tokio::test]
    async fn the_lease_goes_to_one_instance_and_is_released_on_exit() {
        let (core, src, dst) = core_with_servers().await;
        let project = core.migration_create("Acme move", &src, &dst, DestEcoverse::NotLinked).await.unwrap();
        assert!(core.migration_lease(&project.id, "app-1").await.unwrap());
        assert!(core.migration_lease(&project.id, "app-1").await.unwrap(), "renewed");
        assert!(!core.migration_lease(&project.id, "app-2").await.unwrap(), "the second app is read-only");
        core.migration_release_lease(&project.id, "app-2").await.unwrap();
        assert!(!core.migration_lease(&project.id, "app-2").await.unwrap(), "only the holder can release");
        core.migration_release_lease(&project.id, "app-1").await.unwrap();
        assert!(core.migration_lease(&project.id, "app-2").await.unwrap());
        assert_eq!(core.migration_list().await.unwrap().len(), 1, "a lease isn't mistaken for a project");
    }

    /// Not a check: prints how long saving and loading a large project take, for `cargo test scale -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn scale_of_the_project_store() {
        use crate::migration::planner::{DeliveryPath, PlannedDevice};
        for count in [1_000usize, 10_000] {
            let core = Core::for_tests("scale");
            let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
            let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
            let mut p = core.migration_create("Big", &src.id, &dst.id, DestEcoverse::NotLinked).await.unwrap();
            let now = Utc::now();
            let planned: Vec<PlannedDevice> = (0..count)
                .map(|i| PlannedDevice {
                    device_id: format!("{}", 100_000 + i),
                    name: format!("WORKSTATION-{i:05}"),
                    source_customer_id: Some("10".into()),
                    source_site_id: Some("11".into()),
                    dest_org_unit_id: Some("110".into()),
                    path: Some(DeliveryPath::Ecoverse),
                    asset_id: Some(format!("asset-{i}")),
                    blockers: vec![],
                    warnings: vec![],
                    excluded: false,
                })
                .collect();
            p.add_planned(&planned, now);
            for d in p.devices.iter_mut() {
                for e in 0..30 {
                    d.record(now, "poll", &format!("seen on the new server, check-in {e} minutes ago, Ecoverse reporting"));
                }
            }
            let t = std::time::Instant::now();
            core.migration_save(&mut p).await.unwrap();
            let save = t.elapsed();
            let size = std::fs::metadata(core.paths.store_dir().join(format!("migration-{}.enc", p.id))).map(|m| m.len()).unwrap_or(0);
            let t = std::time::Instant::now();
            let loaded = core.migration_load(&p.id).await.unwrap().unwrap();
            let load = t.elapsed();
            let t = std::time::Instant::now();
            let view = crate::migration::trackview::track_view(&loaded, &Default::default(), &Default::default(), &Default::default());
            let track = t.elapsed();
            let t = std::time::Instant::now();
            let _ = core.migration_load_shared(&p.id).await.unwrap().unwrap();
            let shared = t.elapsed();
            println!("{count} devices: asked again, from memory {shared:?}");
            println!("{count} devices: file {:.1} MB, save {save:?}, load {load:?}, track page {track:?} (rows {})", size as f64 / 1e6, view.rows.len());
        }
    }

    #[tokio::test]
    async fn a_project_is_sealed_under_its_own_key_and_a_deleted_one_cannot_be_read_back_from_a_copy() {
        let (core, src, dst) = core_with_servers().await;
        let mut project = core.migration_create("Acme move", &src, &dst, DestEcoverse::NotLinked).await.unwrap();
        core.migration_save(&mut project).await.unwrap();
        let path = core.paths.store_file(&format!("migration-{}", project.id)).unwrap();
        let sealed = std::fs::read(&path).unwrap();
        assert!(crate::vault::Vault::is_own_key(&sealed), "a project has its own key");
        assert!(!sealed.windows(9).any(|w| w == b"Acme move"), "and its name isn't readable in the file");
        assert!(core.secrets.get(&project_key_account(&project.id)).unwrap().is_some());

        let lease = core.paths.store_file(&format!("migration-lease-{}", project.id)).unwrap();
        assert!(core.migration_lease(&project.id, "app-1").await.unwrap() && lease.exists());
        // Leftovers of a crashed save and of an unreadable file sit beside it.
        let tmp = path.with_extension("123.4.tmp");
        let aside = path.with_extension("unreadable");
        std::fs::write(&tmp, b"partial").unwrap();
        std::fs::write(&aside, b"old").unwrap();
        let other = core.migration_create("Other", &src, &dst, DestEcoverse::NotLinked).await.unwrap();
        let other_path = core.paths.store_file(&format!("migration-{}", other.id)).unwrap();

        core.migration_delete(&project.id).await.unwrap();
        assert!(!path.exists() && !lease.exists() && !tmp.exists() && !aside.exists(), "the file, lease and leftovers are gone");
        assert!(core.secrets.get(&project_key_account(&project.id)).unwrap().is_none(), "the key is destroyed");
        assert!(other_path.exists() && core.migration_load(&other.id).await.unwrap().is_some(), "another project is untouched");
        assert!(core.migration_load(&project.id).await.unwrap().is_none());

        // A copy of the file, from a backup say, can no longer be opened.
        std::fs::write(&path, &sealed).unwrap();
        let err = core.migration_load(&project.id).await.unwrap_err();
        assert!(err.contains("key has been destroyed"), "{err}");

        // A window that still holds the project can't save it back into existence.
        std::fs::remove_file(&path).unwrap();
        let err = core.migration_save(&mut project).await.unwrap_err();
        assert!(err.contains("was deleted"), "{err}");
        assert!(core.secrets.get(&project_key_account(&project.id)).unwrap().is_none(), "and no new key is made");
    }

    #[tokio::test]
    async fn a_project_from_before_projects_had_their_own_key_still_opens_and_moves_to_one_on_its_next_save() {
        let (core, src, dst) = core_with_servers().await;
        let mut project = Project::new("mig-legacy1".into(), "Old format", &src, &dst, DestEcoverse::NotLinked, Utc::now());
        core.store_set("migration-mig-legacy1", serde_json::to_string(&project).unwrap()).await.unwrap();
        let path = core.paths.store_file("migration-mig-legacy1").unwrap();
        assert!(!crate::vault::Vault::is_own_key(&std::fs::read(&path).unwrap()));
        assert_eq!(core.migration_load("mig-legacy1").await.unwrap().unwrap().name, "Old format");
        core.migration_save(&mut project).await.unwrap();
        assert!(crate::vault::Vault::is_own_key(&std::fs::read(&path).unwrap()));
        core.migration_delete("mig-legacy1").await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn clearing_local_data_destroys_every_projects_key_and_removes_its_file() {
        let (core, src, dst) = core_with_servers().await;
        let a = core.migration_create("A", &src, &dst, DestEcoverse::NotLinked).await.unwrap();
        let b = core.migration_create("B", &src, &dst, DestEcoverse::NotLinked).await.unwrap();
        for p in [&a, &b] {
            assert!(core.secrets.get(&project_key_account(&p.id)).unwrap().is_some());
        }
        core.clear_local_data().await.unwrap();
        for p in [&a, &b] {
            assert!(core.secrets.get(&project_key_account(&p.id)).unwrap().is_none());
            assert!(core.migration_load(&p.id).await.unwrap().is_none());
        }
        assert!(!core.paths.store_dir().exists());
    }
}
