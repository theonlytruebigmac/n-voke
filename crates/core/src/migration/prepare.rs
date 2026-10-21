//! What the Prepare step shows and changes: whether the endpoint script is in Ecoverse and in the source server's
//! repository at the version this build ships, and the project's options. The decisions are pure functions; the
//! `Core` methods only load, call them, save, and audit.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use chrono::Utc;
use serde_json::json;

use super::{
    packager::{Package, PublishAction},
    project::{CleanMode, Delivery, DestEcoverse, Project, ProjectOptions, ScriptOrg},
};
use crate::{Core, audit::AuditEntry};

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ScriptState.ts"))]
#[serde(rename_all = "camelCase")]
pub enum ScriptState {
    /// Nothing there yet.
    NotThere,
    /// There, at the version this build ships.
    Current,
    /// There, but an older version.
    Outdated,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "EcoverseCard.ts"))]
#[serde(rename_all = "camelCase")]
pub struct EcoverseCard {
    /// Why Ecoverse isn't used for this project, when it isn't.
    pub not_used: Option<String>,
    pub state: ScriptState,
    pub org: Option<ScriptOrg>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "RestCard.ts"))]
#[serde(rename_all = "camelCase")]
pub struct RestCard {
    /// The server whose repository gets the file.
    pub server_label: String,
    pub server_host: String,
    pub item_id: Option<String>,
    /// The version the operator said they uploaded.
    pub uploaded_version: Option<String>,
    pub state: ScriptState,
}

/// Which of the project's two servers a script is for.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ScriptSide.ts"))]
#[serde(rename_all = "camelCase")]
pub enum ScriptSide {
    Source,
    Destination,
}

/// The script on the destination, which reaches a device after it has moved (to probe it or clean it up).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DestinationCards.ts"))]
#[serde(rename_all = "camelCase")]
pub struct DestinationCards {
    /// Only when the destination is linked to another Ecoverse tenant: the same tenant uses the source's script.
    pub ecoverse: Option<EcoverseCard>,
    pub rest: RestCard,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "PrepareView.ts"))]
#[serde(rename_all = "camelCase")]
pub struct PrepareView {
    /// For example `0.1.0+fa1d1a0`.
    pub script_version: String,
    pub script_name: String,
    pub file_name: String,
    pub delivery: Delivery,
    /// A wave has launched, so the delivery can't be changed any more.
    pub delivery_locked: bool,
    pub ecoverse: EcoverseCard,
    pub rest: RestCard,
    pub options: ProjectOptions,
    /// This destination is the same server, so the agents are reinstalled in place.
    pub reinstall: bool,
    /// None for a reinstall, where the source's script reaches the device before and after.
    pub destination: Option<DestinationCards>,
}

/// What the page needs to know about the destination server.
pub struct DestinationInfo<'a> {
    pub label: &'a str,
    pub host: &'a str,
    /// The destination server's devices can be reached through Ecoverse (it has a key).
    pub ecoverse_used: bool,
}

/// Where the script stands for a project. `ecoverse_used`: the project's delivery and the source server's key both
/// allow Ecoverse.
pub fn prepare_view(project: &Project, package: &Package, source_label: &str, source_host: &str, ecoverse_used: bool, dest: &DestinationInfo) -> PrepareView {
    let state_of = |there: bool, current: bool| match (there, current) {
        (false, _) => ScriptState::NotThere,
        (true, true) => ScriptState::Current,
        (true, false) => ScriptState::Outdated,
    };
    let rest = project.rest_scripts.get(&project.source_server_id);
    let not_used = if project.options.delivery == Delivery::RestOnly {
        Some("This project reaches every device through N-central.".to_string())
    } else if !ecoverse_used {
        Some(format!("{source_label} is set to not use Ecoverse, so its devices are reached through N-central."))
    } else {
        None
    };
    PrepareView {
        script_version: package.version.clone(),
        script_name: package.script_name(),
        file_name: package.file_name(),
        delivery: project.options.delivery,
        delivery_locked: project.has_launched(),
        ecoverse: EcoverseCard {
            not_used,
            state: state_of(project.ecoverse_script_id.is_some(), project.ecoverse_script_version.as_deref() == Some(package.version.as_str())),
            org: project.ecoverse_script_org.clone(),
        },
        rest: RestCard {
            server_label: source_label.into(),
            server_host: source_host.into(),
            item_id: rest.map(|r| r.item_id.clone()),
            uploaded_version: rest.map(|r| r.version.clone()),
            state: state_of(rest.is_some(), rest.is_some_and(|r| r.version == package.version)),
        },
        options: project.options.clone(),
        reinstall: project.is_reinstall(),
        destination: (!project.is_reinstall()).then(|| {
            let rest = project.rest_scripts.get(&project.dest_server_id);
            DestinationCards {
                ecoverse: matches!(project.dest_ecoverse, DestEcoverse::OtherTenant { .. }).then(|| EcoverseCard {
                    not_used: (!dest.ecoverse_used).then(|| format!("{} is set to not use Ecoverse, so devices that have moved are reached through N-central.", dest.label)),
                    state: state_of(project.dest_ecoverse_script_id.is_some(), project.dest_ecoverse_script_version.as_deref() == Some(package.version.as_str())),
                    org: project.dest_ecoverse_script_org.clone(),
                }),
                rest: RestCard {
                    server_label: dest.label.into(),
                    server_host: dest.host.into(),
                    item_id: rest.map(|r| r.item_id.clone()),
                    uploaded_version: rest.map(|r| r.version.clone()),
                    state: state_of(rest.is_some(), rest.is_some_and(|r| r.version == package.version)),
                },
            }
        }),
    }
}

/// Refuses values the script or a Windows scheduled task can't use.
pub fn validate_options(options: &ProjectOptions) -> Result<(), String> {
    let within = |name: &str, value: u32, low: u32, high: u32, unit: &str| {
        if (low..=high).contains(&value) { Ok(()) } else { Err(format!("{name} must be between {low} and {high} {unit}.")) }
    };
    within("The review period", options.review_days, 0, 999, "days")?;
    within("The wave size", options.wave_size, 1, 500, "devices")?;
    within("How often the device retries", options.task_repeat_minutes, 5, 1440, "minutes")?;
    within("How long the device keeps retrying", options.task_expiry_days, 1, 90, "days")?;
    Ok(())
}

fn describe(options: &ProjectOptions) -> String {
    let clean = match options.clean_mode {
        CleanMode::Full => "full cleanup",
        CleanMode::None => "install over the top",
    };
    format!(
        "clean mode {clean}, review {} days, wave size {}, retry every {} min for {} days, retire source records {}",
        options.review_days,
        options.wave_size,
        options.task_repeat_minutes,
        options.task_expiry_days,
        if options.retire_source_records { "yes" } else { "no" }
    ) + &format!(", delivery {}", if options.delivery == Delivery::RestOnly { "N-central only" } else { "Ecoverse first" })
}

impl Core {
    /// Where the endpoint script stands for a project. Reads only the saved project, so it is quick.
    pub async fn migration_prepare_view(&self, id: &str) -> Result<PrepareView, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let source = self.ncentral_server(&project.source_server_id)?;
        let dest = self.ncentral_server(&project.dest_server_id)?;
        let dest_info = DestinationInfo { label: &dest.label, host: &dest.host, ecoverse_used: self.server_uses_ecoverse(&dest.id) };
        Ok(prepare_view(&project, &Package::embedded()?, &source.label, &source.host, self.server_uses_ecoverse(&source.id), &dest_info))
    }

    /// Ecoverse organizations, on the key of the server the script is for, to choose where it is published.
    pub async fn migration_search_script_orgs(&self, id: &str, side: ScriptSide, term: &str) -> Result<Vec<ScriptOrg>, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let server = if side == ScriptSide::Source { &project.source_server_id } else { &project.dest_server_id };
        let found = self.with_ecoverse(server, self.search_organizations(term)).await?;
        Ok(found.into_iter().map(|o| ScriptOrg { id: o.id, name: o.name }).collect())
    }

    /// The organization to publish the script in when nobody has chosen one: the tenant's single service
    /// organization (the top of its tree, so the script reaches every customer under it). None when the key sees none
    /// or several, in which case the person chooses.
    pub async fn migration_default_script_org(&self, server_id: &str) -> Result<Option<ScriptOrg>, String> {
        let all = self.with_ecoverse(server_id, self.fetch_all_organizations()).await?;
        let mut roots = all.into_iter().filter(|o| o.typename == "ServiceOrganization");
        Ok(match (roots.next(), roots.next()) {
            (Some(only), None) => Some(ScriptOrg { id: only.id, name: only.name }),
            _ => None,
        })
    }

    /// The default for one side of a project, for the page to preselect.
    pub async fn migration_default_script_org_for(&self, id: &str, side: ScriptSide) -> Result<Option<ScriptOrg>, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let server = if side == ScriptSide::Source { &project.source_server_id } else { &project.dest_server_id };
        self.migration_default_script_org(server).await
    }

    /// Publishes the endpoint script to the chosen organization (or leaves it, when it is already there) and
    /// remembers where. Returns what happened, in a sentence.
    pub async fn migration_prepare_publish(&self, id: &str, instance: &str, side: ScriptSide, org: ScriptOrg) -> Result<String, String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        let package = Package::embedded()?;
        let published = match side {
            ScriptSide::Source => {
                let published = self.migration_publish(&mut project, &package, Some(&org.id)).await?;
                project.ecoverse_script_org = Some(org.clone());
                published
            }
            ScriptSide::Destination => {
                if !matches!(project.dest_ecoverse, DestEcoverse::OtherTenant { .. }) {
                    return Err("The destination isn't linked to another Ecoverse tenant, so it has no script of its own.".into());
                }
                let published = self.with_ecoverse(&project.dest_server_id, self.publish_migration_script(&package, &org.id)).await?;
                project.dest_ecoverse_script_id = Some(published.script_id.clone());
                project.dest_ecoverse_script_version = Some(package.version.clone());
                project.dest_ecoverse_script_org = Some(org.clone());
                published
            }
        };
        self.migration_save(&mut project).await?;
        let what = match published.action {
            PublishAction::Create => "Published",
            PublishAction::Update { .. } => "Replaced the old version with",
            PublishAction::UpToDate { .. } => "Already up to date:",
        };
        Ok(format!("{what} {} in {}.", package.script_name(), org.name))
    }

    /// Saves the script as a file in the Downloads folder, for uploading to the source server's repository.
    pub fn migration_prepare_export(&self) -> Result<PathBuf, String> {
        let dir = dirs::download_dir().or_else(dirs::data_dir).ok_or("There is no Downloads folder to save the script in.")?;
        self.migration_export_script(&Package::embedded()?, &dir)
    }

    /// Records the repository item the operator uploaded the script as, and which version it was.
    pub async fn migration_prepare_rest_script(&self, id: &str, instance: &str, side: ScriptSide, item_id: &str, version: &str) -> Result<(), String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        let version = version.trim();
        if version.is_empty() || version.len() > 60 || !version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-')) {
            return Err("The script version looks like 0.1.0+fa1d1a0.".into());
        }
        let server = if side == ScriptSide::Source { project.source_server_id.clone() } else { project.dest_server_id.clone() };
        self.migration_set_rest_script_on(&mut project, &server, item_id.trim(), version)?;
        self.migration_save(&mut project).await
    }

    /// Changes how the project behaves from now on. Devices already launched keep what they were sent.
    pub async fn migration_prepare_options(&self, id: &str, instance: &str, options: ProjectOptions) -> Result<(), String> {
        validate_options(&options)?;
        let mut project = self.migration_open_for_write(id, instance).await?;
        if project.options == options {
            return Ok(());
        }
        let delivery_changed = project.options.delivery != options.delivery;
        if delivery_changed && project.has_launched() {
            return Err("The delivery can't change once a wave has launched: devices have already been sent the migration. Choose a path for a single device in Plan instead.".into());
        }
        self.record(&AuditEntry::new("migration.options", &format!("Changed the options of {}: {}", project.name, describe(&options)), Ok(()), json!({ "project": project.id })));
        project.options = options;
        if delivery_changed {
            project.repath_planned(Utc::now());
            super::planview::forget_plan(id);
        }
        self.migration_save(&mut project).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::project::{DestEcoverse, RestScript};

    fn package(version: &str) -> Package {
        Package { version: version.into(), content: String::new() }
    }

    fn dest() -> DestinationInfo<'static> {
        DestinationInfo { label: "New", host: "new.example.com", ecoverse_used: true }
    }

    fn project() -> Project {
        Project::new("p".into(), "Acme", "src", "dst", DestEcoverse::NotLinked, chrono::Utc::now())
    }

    #[test]
    fn a_new_project_has_the_script_nowhere_and_ecoverse_in_use() {
        let view = prepare_view(&project(), &package("0.1.0+abc"), "Old", "old.example.com", true, &dest());
        assert_eq!((view.ecoverse.state, view.rest.state, view.ecoverse.not_used.is_none()), (ScriptState::NotThere, ScriptState::NotThere, true));
        assert_eq!((view.script_version.as_str(), view.file_name.as_str()), ("0.1.0+abc", "nvoke-migration-0.1.0-abc.ps1"));
    }

    #[test]
    fn a_script_from_another_build_is_outdated_in_both_places_and_current_when_the_versions_match() {
        let mut p = project();
        p.ecoverse_script_id = Some("s1".into());
        p.ecoverse_script_version = Some("0.1.0+old".into());
        p.rest_scripts.insert("src".into(), RestScript { item_id: "77".into(), version: "0.1.0+old".into() });
        let view = prepare_view(&p, &package("0.1.0+new"), "Old", "old.example.com", true, &dest());
        assert_eq!((view.ecoverse.state, view.rest.state, view.rest.item_id.as_deref()), (ScriptState::Outdated, ScriptState::Outdated, Some("77")));
        let same = prepare_view(&p, &package("0.1.0+old"), "Old", "old.example.com", true, &dest());
        assert_eq!((same.ecoverse.state, same.rest.state), (ScriptState::Current, ScriptState::Current));
    }

    #[test]
    fn ecoverse_says_why_it_isnt_used() {
        let mut p = project();
        p.options.delivery = Delivery::RestOnly;
        assert!(prepare_view(&p, &package("v"), "Old", "h", true, &dest()).ecoverse.not_used.unwrap().contains("every device through N-central"));
        p.options.delivery = Delivery::EcoverseFirst;
        assert!(prepare_view(&p, &package("v"), "Old", "h", false, &dest()).ecoverse.not_used.unwrap().contains("Old is set to not use Ecoverse"));
    }

    #[test]
    fn options_outside_what_the_script_and_the_task_can_use_are_refused_with_the_range() {
        let ok = ProjectOptions::default();
        assert!(validate_options(&ok).is_ok());
        let with = |f: &dyn Fn(&mut ProjectOptions)| {
            let mut o = ok.clone();
            f(&mut o);
            validate_options(&o)
        };
        assert!(with(&|o| o.review_days = 0).is_ok(), "no review period is allowed");
        assert!(with(&|o| o.review_days = 1000).unwrap_err().contains("0 and 999"));
        assert!(with(&|o| o.wave_size = 0).unwrap_err().contains("1 and 500"));
        assert!(with(&|o| o.task_repeat_minutes = 1).unwrap_err().contains("5 and 1440"));
        assert!(with(&|o| o.task_expiry_days = 91).unwrap_err().contains("1 and 90"));
    }

    async fn saved() -> (std::sync::Arc<Core>, Project) {
        let core = Core::for_tests("prepare");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let project = core.migration_create_full("Prep", &src.id, &dst.id, DestEcoverse::NotLinked, vec![], vec![]).await.unwrap();
        (core, project)
    }

    #[tokio::test]
    async fn options_are_checked_saved_audited_and_a_second_identical_save_changes_nothing() {
        let (core, project) = saved().await;
        let mut options = project.options.clone();
        options.review_days = 14;
        options.clean_mode = CleanMode::None;
        core.migration_prepare_options(&project.id, "me", options.clone()).await.unwrap();
        let reloaded = core.migration_load(&project.id).await.unwrap().unwrap();
        assert_eq!(reloaded.options, options);
        let revision = reloaded.revision;
        core.migration_prepare_options(&project.id, "me", options.clone()).await.unwrap();
        assert_eq!(core.migration_load(&project.id).await.unwrap().unwrap().revision, revision, "nothing was saved");

        options.wave_size = 0;
        assert!(core.migration_prepare_options(&project.id, "me", options).await.is_err());
        assert!(core.migration_prepare_options(&project.id, "other", ProjectOptions::default()).await.unwrap_err().contains("Another N-voke window"));
    }

    #[tokio::test]
    async fn the_rest_item_is_a_number_the_version_is_plain_and_the_view_follows() {
        let (core, project) = saved().await;
        assert!(core.migration_prepare_rest_script(&project.id, "me", ScriptSide::Source, "12ab", "0.1.0+abc").await.is_err());
        assert!(core.migration_prepare_rest_script(&project.id, "me", ScriptSide::Source, "1271448998", "bad version!").await.is_err());
        let version = core.migration_prepare_view(&project.id).await.unwrap().script_version;
        core.migration_prepare_rest_script(&project.id, "me", ScriptSide::Source, " 1271448998 ", &version).await.unwrap();
        let view = core.migration_prepare_view(&project.id).await.unwrap();
        assert_eq!((view.rest.item_id.as_deref(), view.rest.state), (Some("1271448998"), ScriptState::Current));
    }

    #[tokio::test]
    async fn publishing_remembers_the_organization_and_where_the_script_is() {
        let (core, project) = saved().await;
        core.set_graphql_fake(|query, _| {
            let body = if query.contains("scriptSearch") {
                json!({ "data": { "byName": { "nodes": [] } } })
            } else {
                json!({ "data": { "scriptCreate": { "item": { "id": "script-9" }, "errors": [] } } })
            };
            (200, body.to_string())
        });
        let message = core.migration_prepare_publish(&project.id, "me", ScriptSide::Source, ScriptOrg { id: "org-1".into(), name: "Contoso".into() }).await.unwrap();
        assert!(message.starts_with("Published N-voke migration") && message.ends_with("in Contoso."), "{message}");
        let saved = core.migration_load(&project.id).await.unwrap().unwrap();
        assert_eq!((saved.ecoverse_script_id.as_deref(), saved.ecoverse_script_org.map(|o| o.name)), (Some("script-9"), Some("Contoso".to_string())));
        let view = core.migration_prepare_view(&project.id).await.unwrap();
        assert_eq!((view.ecoverse.state, view.ecoverse.org.map(|o| o.id)), (ScriptState::Current, Some("org-1".to_string())));
    }

    #[test]
    fn a_move_to_another_server_also_asks_for_the_destinations_script_and_a_reinstall_does_not() {
        let mut p = project();
        let none = prepare_view(&p, &package("v"), "Old", "o", true, &dest());
        let cards = none.destination.expect("a move has destination cards");
        assert!(cards.ecoverse.is_none(), "not linked: no Ecoverse script on the destination");
        assert_eq!((cards.rest.server_host.as_str(), cards.rest.state), ("new.example.com", ScriptState::NotThere));

        p.dest_ecoverse = DestEcoverse::SameTenant;
        assert!(prepare_view(&p, &package("v"), "Old", "o", true, &dest()).destination.unwrap().ecoverse.is_none(), "the same tenant uses the source's script");

        p.dest_ecoverse = DestEcoverse::OtherTenant { tenant_id: "t".into() };
        p.dest_ecoverse_script_id = Some("s".into());
        p.dest_ecoverse_script_version = Some("v".into());
        p.rest_scripts.insert("dst".into(), RestScript { item_id: "9".into(), version: "old".into() });
        let cards = prepare_view(&p, &package("v"), "Old", "o", true, &dest()).destination.unwrap();
        assert_eq!(cards.ecoverse.unwrap().state, ScriptState::Current);
        assert_eq!((cards.rest.item_id.as_deref(), cards.rest.state), (Some("9"), ScriptState::Outdated));

        let mut same = project();
        same.dest_server_id = "src".into();
        assert!(prepare_view(&same, &package("v"), "Old", "o", true, &dest()).destination.is_none(), "a reinstall has one server");
    }

    #[tokio::test]
    async fn the_destinations_script_is_published_with_its_own_key_and_kept_apart_from_the_sources() {
        let (core, project) = saved().await;
        let org = || ScriptOrg { id: "org-9".into(), name: "Destination".into() };
        // Not linked to another tenant: there is nothing to publish there.
        assert!(core.migration_prepare_publish(&project.id, "me", ScriptSide::Destination, org()).await.unwrap_err().contains("isn't linked to another Ecoverse tenant"));

        let mut p = core.migration_load(&project.id).await.unwrap().unwrap();
        p.dest_ecoverse = DestEcoverse::OtherTenant { tenant_id: "t".into() };
        core.migration_save(&mut p).await.unwrap();
        core.set_graphql_fake(|query, _| {
            let body = if query.contains("scriptSearch") { json!({ "data": { "byName": { "nodes": [] } } }) } else { json!({ "data": { "scriptCreate": { "item": { "id": "dest-script" }, "errors": [] } } }) };
            (200, body.to_string())
        });
        core.migration_prepare_publish(&project.id, "me", ScriptSide::Destination, org()).await.unwrap();
        let saved = core.migration_load(&project.id).await.unwrap().unwrap();
        assert_eq!((saved.dest_ecoverse_script_id.as_deref(), saved.dest_ecoverse_script_org.map(|o| o.name)), (Some("dest-script"), Some("Destination".to_string())));
        assert!(saved.ecoverse_script_id.is_none(), "the source's script is untouched");

        // The repository item for the destination is recorded on the destination's server.
        core.migration_prepare_rest_script(&project.id, "me", ScriptSide::Destination, "555", "v1").await.unwrap();
        let saved = core.migration_load(&project.id).await.unwrap().unwrap();
        assert_eq!(saved.rest_scripts.get(&saved.dest_server_id).map(|r| r.item_id.as_str()), Some("555"));
        assert!(!saved.rest_scripts.contains_key(&saved.source_server_id));
    }

    #[tokio::test]
    async fn the_delivery_can_change_until_a_wave_launches_and_re_paths_the_planned_devices() {
        use crate::migration::planner::{DeliveryPath, PlannedDevice};
        let core = Core::for_tests("delivery");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let mut p = core.migration_create_full("Del", &src.id, &dst.id, DestEcoverse::NotLinked, vec![], vec![]).await.unwrap();
        let planned = |id: &str, asset: Option<&str>| PlannedDevice {
            device_id: id.into(),
            name: format!("PC-{id}"),
            source_customer_id: Some("10".into()),
            source_site_id: None,
            dest_org_unit_id: Some("110".into()),
            path: Some(if asset.is_some() { DeliveryPath::Ecoverse } else { DeliveryPath::Rest }),
            asset_id: asset.map(String::from),
            blockers: vec![],
            warnings: vec![],
            excluded: false,
        };
        p.add_planned(&[planned("1", Some("a1")), planned("2", Some("a2")), planned("3", None)], chrono::Utc::now());
        p.path_overrides.insert("2".into(), DeliveryPath::Ecoverse);
        core.migration_save(&mut p).await.unwrap();

        let mut rest_only = p.options.clone();
        rest_only.delivery = Delivery::RestOnly;
        core.migration_prepare_options(&p.id, "me", rest_only.clone()).await.unwrap();
        let saved = core.migration_load(&p.id).await.unwrap().unwrap();
        let paths: Vec<_> = saved.devices.iter().map(|d| d.path).collect();
        assert_eq!(paths, [Some(DeliveryPath::Rest), Some(DeliveryPath::Ecoverse), Some(DeliveryPath::Rest)], "a path the operator chose is kept");

        let mut back = rest_only.clone();
        back.delivery = Delivery::EcoverseFirst;
        core.migration_prepare_options(&p.id, "me", back.clone()).await.unwrap();
        let saved = core.migration_load(&p.id).await.unwrap().unwrap();
        assert_eq!(saved.devices[0].path, Some(DeliveryPath::Ecoverse));
        assert_eq!(saved.devices[2].path, Some(DeliveryPath::Rest), "no Ecoverse asset, so N-central");
        assert!(!prepare_view(&saved, &package("v"), "Old", "h", true, &dest()).delivery_locked);

        let mut saved = saved;
        saved.create_wave("Pilot", &["1".to_string()], chrono::Utc::now()).unwrap();
        saved.waves[0].launched_at = Some("2026-10-21T00:00:00Z".into());
        core.migration_save(&mut saved).await.unwrap();
        assert!(prepare_view(&saved, &package("v"), "Old", "h", true, &dest()).delivery_locked);
        let err = core.migration_prepare_options(&p.id, "me", rest_only).await.unwrap_err();
        assert!(err.contains("once a wave has launched"), "{err}");
        let mut same_delivery = back;
        same_delivery.wave_size = 10;
        core.migration_prepare_options(&p.id, "me", same_delivery).await.unwrap();
    }
}
