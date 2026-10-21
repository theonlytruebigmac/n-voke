//! Saving and listing migration projects. Each project is its own encrypted store
//! (`migration-<id>`), kept until the user deletes it: the run-history retention
//! setting doesn't apply, because a migration can span weeks.
//!
//! Only one app should drive a project at a time. A small lease (instance and
//! heartbeat) in its own store says who; the other app opens the project
//! read-only. The lease is advisory: it stops the common case of two windows
//! open, not two writers racing within the same second.

use std::{collections::BTreeMap, fs};

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

fn project_store(id: &str) -> String {
    format!("{PROJECT_PREFIX}{id}")
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
        #[derive(serde::Deserialize)]
        struct Stored {
            #[serde(default)]
            revision: u64,
        }
        let name = project_store(&project.id);
        if let Some(stored) = self.store_get(&name).await?.and_then(|t| serde_json::from_str::<Stored>(&t).ok())
            && stored.revision != project.revision
        {
            return Err(format!("{} was changed in another N-voke window since this one loaded it, so this save was refused. Close and reopen the project to see what changed.", project.name));
        }
        project.revision += 1;
        let json = serde_json::to_string(&*project).map_err(|e| e.to_string())?;
        self.store_set(&name, json).await.map_err(|e| {
            project.revision -= 1;
            format!("Could not save the project: {e}")
        })
    }

    pub async fn migration_load(&self, id: &str) -> Result<Option<Project>, String> {
        match self.store_get(&project_store(id)).await? {
            None => Ok(None),
            Some(text) => serde_json::from_str(&text).map(Some).map_err(|e| format!("The project {id} couldn't be read: {e}")),
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
            if let Some(project) = self.migration_load(&name[PROJECT_PREFIX.len()..]).await.ok().flatten() {
                summaries.push(ProjectSummary::of(&project));
            }
        }
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.name.cmp(&b.name)));
        Ok(summaries)
    }

    /// One project's summary, for a header.
    pub async fn migration_summary(&self, id: &str) -> Result<ProjectSummary, String> {
        self.migration_load(id).await?.map(|p| ProjectSummary::of(&p)).ok_or_else(|| "That project no longer exists.".to_string())
    }

    /// The name of a project that uses this server as its source or destination, if any. Such a server can't be
    /// removed: the project would lose it.
    pub async fn migration_project_using_server(&self, server_id: &str) -> Result<Option<String>, String> {
        Ok(self.migration_list().await?.into_iter().find(|p| p.source_server_id == server_id || p.dest_server_id == server_id).map(|p| p.name))
    }

    /// Deletes a project's local history. The audit log keeps its entries.
    pub async fn migration_delete(&self, id: &str) -> Result<(), String> {
        let name = self.migration_load(id).await?.map(|p| p.name).ok_or("That project no longer exists.")?;
        let result = self.store_remove(&project_store(id)).await.and(self.store_remove(&lease_store(id)).await);
        self.record(&AuditEntry::new("migration.project.delete", &format!("Deleted migration project {name}"), result.clone(), json!({ "project": id })));
        result
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
        assert!(log.iter().any(|e| e.action == "migration.project.delete" && e.summary == "Deleted migration project Globex move"));
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
}
