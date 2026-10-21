//! The steps an operator takes on a project, as calls the terminal commands and (later) the
//! Migrate screens share: take the project's lease, plan devices into it, make waves, publish
//! or register the endpoint script, launch, and poll. Each changes the project in memory and
//! leaves saving to the caller, except `migration_poll_saved`, which is the loop's one step.

use serde::Serialize;
use serde_json::json;

use super::{
    packager::{Package, Published},
    planner::DeliveryPath,
    poller::PollReport,
    project::{DestEcoverse, Project, RestScript, Stage},
    mapping::Override,
    survey::SurveyOptions,
    tracker::Thresholds,
};
use crate::ncentral::model::TokenStatus;
use crate::{Core, audit::AuditEntry};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanIntoReport {
    /// Devices that were ready and are now in the project.
    pub added: usize,
    /// Devices already in the project, so left alone.
    pub already_in: usize,
    /// Devices that can't be launched yet, with the reason, so the operator can fix and plan again.
    pub blocked: Vec<(String, Vec<String>)>,
    pub warnings: Vec<String>,
}

/// What generating missing registration tokens did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "TokenReport.ts"))]
#[serde(rename_all = "camelCase")]
pub struct TokenReport {
    /// Destination customers and sites that now have a token.
    pub generated: Vec<String>,
    /// Ones that already had a usable token and were left alone.
    pub already_fine: usize,
    /// Ones whose token has expired: N-central has to renew it.
    pub expired: Vec<String>,
    /// Held back by dry-run.
    pub held_back: usize,
    pub failed: Vec<(String, String)>,
}

/// Whether an operator can put a device back in line for a launch: its launch failed, wasn't picked up or is
/// stuck, or a launch was sent and never recorded (`unsure`).
pub fn retryable(stage: Stage, unsure: bool) -> bool {
    matches!(stage, Stage::LaunchFailed | Stage::NotPickedUp | Stage::StuckStaged | Stage::Unreachable) || (unsure && stage == Stage::Planned)
}

/// What an operator can do to one device that needs help.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceOp {
    /// Put it back in line for a launch, on the same path or the other one. `checked`: the operator looked, and an
    /// earlier launch whose answer was lost did not run (needed only for such a device).
    Retry { other_path: bool, checked: bool },
    /// It was fixed outside N-voke: stop tracking it.
    Resolve,
    /// Two destination devices fit; this is the one.
    ChooseMatch { dest_device_id: String },
}

impl Core {
    /// Creates a registration token on the destination for every customer or site the (selected) devices are going to
    /// that has none. A valid token is never touched, and an expired one is only reported: how N-central treats
    /// generating over it is untested. Writes to the destination server, not to the project, so it needs no lease.
    pub async fn migration_generate_tokens(&self, project: &Project, customer: Option<&str>) -> Result<TokenReport, String> {
        let options = SurveyOptions {
            source_server_id: project.source_server_id.clone(),
            dest_server_id: project.dest_server_id.clone(),
            ecoverse_org_ids: vec![],
            customer: customer.map(String::from),
            rest_script_ready: true,
        };
        super::planview::forget_plan(&project.id);
        // What is about to be decided is which tokens are missing, so that is read fresh, and again after creating any.
        super::readcache::forget_server(&project.dest_server_id);
        let survey = self.migration_survey(&options, Some(project)).await?;
        let name = |id: &str| survey.dest_units.iter().find(|u| u.org_unit_id == id).map_or_else(|| id.to_string(), |u| u.org_unit_name.clone());
        let mut report = TokenReport::default();
        let mut units: Vec<(&String, &TokenStatus)> = survey.tokens.iter().collect();
        units.sort_by(|a, b| a.0.cmp(b.0));
        for (unit, status) in units {
            match status {
                TokenStatus::Missing => match self.ncentral_generate_registration_token(&project.dest_server_id, unit, &name(unit)).await {
                    Ok(_) => report.generated.push(name(unit)),
                    Err(e) if e.is_dry_run() => report.held_back += 1,
                    Err(e) => report.failed.push((name(unit), e.message)),
                },
                TokenStatus::Expired { .. } => report.expired.push(name(unit)),
                _ => report.already_fine += 1,
            }
        }
        if !report.generated.is_empty() {
            super::readcache::forget_server(&project.dest_server_id);
        }
        Ok(report)
    }

    /// Records a mapping decision for a source customer or site, or leaves it out of the migration. The automatic
    /// mapping is recomputed on every plan, so only decisions are stored.
    pub fn migration_set_mapping(&self, project: &mut Project, source_id: &str, decision: Override) -> Result<(), String> {
        let valid_id = |id: &str| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit());
        if !valid_id(source_id) || matches!(&decision, Override::MapTo(dest) if !valid_id(dest)) {
            return Err("Customer and site IDs are numbers (the plan shows them in square brackets).".into());
        }
        let summary = match &decision {
            Override::MapTo(dest) => format!("Mapped source {source_id} to destination {dest}"),
            Override::Exclude => format!("Left source {source_id} out of the migration"),
        };
        project.mapping_overrides.insert(source_id.to_string(), decision);
        self.record(&AuditEntry::new("migration.mapping.set", &summary, Ok(()), json!({ "project": project.id, "source": source_id })));
        Ok(())
    }

    /// Loads a project for changing, holding its lease. Fails if another app instance holds it.
    pub async fn migration_open_for_write(&self, id: &str, instance: &str) -> Result<Project, String> {
        // Lease first, then load, so what's read can't be older than the lease.
        let held = self.migration_lease(id, instance).await?;
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        if !held {
            return Err(format!("Another N-voke window is working on {}. Close it, or wait two minutes, then try again.", project.name));
        }
        Ok(project)
    }

    /// Creates a project and records which Ecoverse organizations to search on each side.
    pub async fn migration_create_full(&self, name: &str, source_server_id: &str, dest_server_id: &str, dest_ecoverse: DestEcoverse, source_orgs: Vec<String>, dest_orgs: Vec<String>) -> Result<Project, String> {
        let mut project = self.migration_create(name, source_server_id, dest_server_id, dest_ecoverse).await?;
        project.source_ecoverse_org_ids = source_orgs;
        project.dest_ecoverse_org_ids = dest_orgs;
        self.migration_save(&mut project).await?;
        Ok(project)
    }

    /// `migration_plan_into` on a saved project: lease, plan, save.
    pub async fn migration_plan_saved(&self, id: &str, instance: &str, customer: Option<&str>) -> Result<(Project, PlanIntoReport), String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        let (report, _) = self.migration_plan_into(&mut project, customer).await?;
        self.migration_save(&mut project).await?;
        Ok((project, report))
    }

    /// Puts the next waiting devices (up to `max`) in a new wave, on a saved project.
    pub async fn migration_wave_saved(&self, id: &str, instance: &str, label: &str, max: Option<usize>) -> Result<(Project, String), String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        let number = project.auto_wave(label, max, chrono::Utc::now())?;
        let size = project.devices.iter().filter(|d| d.wave == Some(number)).count();
        self.migration_save(&mut project).await?;
        Ok((project, format!("Made wave {number} with {size} device{}. Launch it with L.", if size == 1 { "" } else { "s" })))
    }

    /// Publishes the endpoint script built into this app to the project's Ecoverse organization, on a saved project.
    pub async fn migration_publish_saved(&self, id: &str, instance: &str) -> Result<(Project, String), String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        let package = Package::embedded()?;
        let published = self.migration_publish(&mut project, &package, None).await?;
        self.migration_save(&mut project).await?;
        let what = match published.action {
            super::packager::PublishAction::Create => "Published",
            super::packager::PublishAction::Update { .. } => "Replaced the old version with",
            super::packager::PublishAction::UpToDate { .. } => "Already up to date:",
        };
        Ok((project, format!("{what} {} in Ecoverse.", package.script_name())))
    }

    /// Reads both servers (and Ecoverse, if the project searches it), plans the source's devices (all, or one
    /// customer's), and adds the ones that are ready to the project. Returns the full plan for display.
    pub async fn migration_plan_into(&self, project: &mut Project, customer: Option<&str>) -> Result<(PlanIntoReport, super::survey::Survey), String> {
        let options = SurveyOptions {
            source_server_id: project.source_server_id.clone(),
            dest_server_id: project.dest_server_id.clone(),
            ecoverse_org_ids: project.source_ecoverse_org_ids.clone(),
            customer: customer.map(String::from),
            rest_script_ready: project.rest_scripts.contains_key(&project.source_server_id),
        };
        let survey = self.migration_survey(&options, Some(project)).await?;
        let ready: Vec<_> = survey.planned.iter().filter(|d| d.is_ready()).cloned().collect();
        let before = project.devices.len();
        let added = project.add_planned(&ready, chrono::Utc::now());
        project.fill_names(&survey.source_units, &survey.dest_units);
        let mut report = PlanIntoReport { added, already_in: ready.len() - (project.devices.len() - before), warnings: survey.warnings.clone(), ..Default::default() };
        report.blocked = survey.planned.iter().filter(|d| !d.is_ready() && !d.excluded).map(|d| (d.name.clone(), d.blockers.iter().map(|b| b.message().to_string()).collect())).collect();
        Ok((report, survey))
    }

    /// Records the repository item the operator uploaded the endpoint script as, on the project's source server.
    pub fn migration_set_rest_script(&self, project: &mut Project, item_id: &str, version: &str) -> Result<(), String> {
        let server_id = project.source_server_id.clone();
        self.migration_set_rest_script_on(project, &server_id, item_id, version)
    }

    /// The same for either server of the project: the destination's repository holds the copy that reaches a device
    /// after it has moved.
    pub fn migration_set_rest_script_on(&self, project: &mut Project, server_id: &str, item_id: &str, version: &str) -> Result<(), String> {
        if item_id.is_empty() || !item_id.chars().all(|c| c.is_ascii_digit()) {
            return Err("The script item ID is the number N-central shows for the uploaded script.".into());
        }
        if server_id != project.source_server_id && server_id != project.dest_server_id {
            return Err("That server isn't part of this project.".into());
        }
        let host = self.ncentral_server(server_id)?.host;
        project.rest_scripts.insert(server_id.to_string(), RestScript { item_id: item_id.into(), version: version.into() });
        self.record(&AuditEntry::new("migration.restScript.set", &format!("Set the endpoint script item for {host} to {item_id}"), Ok(()), json!({ "server": host, "itemId": item_id, "version": version })));
        Ok(())
    }

    /// Publishes the endpoint script to Ecoverse (unless it is already there at this version) and remembers its ID.
    pub async fn migration_publish(&self, project: &mut Project, package: &Package, organization_id: Option<&str>) -> Result<Published, String> {
        let org = organization_id
            .map(String::from)
            .or_else(|| project.ecoverse_script_org.as_ref().map(|o| o.id.clone()))
            .or_else(|| project.source_ecoverse_org_ids.first().cloned())
            .ok_or("Name the Ecoverse organization to publish the script in.")?;
        let published = self.with_ecoverse(&project.source_server_id, self.publish_migration_script(package, &org)).await?;
        project.ecoverse_script_id = Some(published.script_id.clone());
        project.ecoverse_script_version = Some(package.version.clone());
        Ok(published)
    }

    /// Applies an operator's decision to one device, and returns what happened in a sentence. Changes the project
    /// in memory; the caller saves. A retry on Ecoverse first cancels a launch that was never picked up, because a
    /// device must never have two launches alive; if that can't be done, nothing changes.
    pub async fn migration_device_op(&self, project: &mut Project, index: usize, op: DeviceOp) -> Result<String, String> {
        let now = chrono::Utc::now();
        let device = project.devices.get(index).ok_or("That device isn't in the project.")?;
        let (name, stage) = (device.name.clone(), device.stage);
        match op {
            DeviceOp::Retry { other_path, checked } => {
                let unsure = device.launch_unsure;
                if !retryable(stage, unsure) {
                    return Err(format!("{name} is {stage:?}: only a device whose launch failed, wasn't picked up or is stuck can be retried."));
                }
                if unsure && !checked {
                    return Err(format!("{name}'s last launch may have gone through: the answer was lost. Look in Activity (Ecoverse) or the task list (N-central) first. If it didn't run, retry again and say you checked."));
                }
                let current = device.path.ok_or_else(|| format!("{name} has no delivery path: plan it again."))?;
                let path = match (other_path, current) {
                    (false, p) => p,
                    (true, DeliveryPath::Ecoverse) => DeliveryPath::Rest,
                    (true, DeliveryPath::Rest) => DeliveryPath::Ecoverse,
                };
                match path {
                    DeliveryPath::Ecoverse if device.asset_id.is_none() || project.ecoverse_script_id.is_none() => {
                        return Err(format!("{name} can't go through Ecoverse: {}.", if device.asset_id.is_none() { "Ecoverse has no asset for it" } else { "the script isn't published to Ecoverse yet" }));
                    }
                    DeliveryPath::Rest if !project.rest_scripts.contains_key(&project.source_server_id) => {
                        return Err(format!("{name} can't go through N-central: the endpoint script hasn't been uploaded to the source server."));
                    }
                    _ => {}
                }
                // A launch still waiting in Ecoverse is cancelled first, so the retry can't run twice.
                if let (Stage::NotPickedUp, DeliveryPath::Ecoverse, Some(task)) = (stage, current, device.task_id.clone()) {
                    self.with_ecoverse(&project.source_server_id, self.cancel_script_tasks(&[task])).await?;
                }
                let device = &mut project.devices[index];
                device.path = Some(path);
                device.task_id = None;
                device.last_error = None;
                if device.launch_unsure {
                    device.launch_unsure = false;
                    device.record(now, "note", "The operator checked that the earlier launch did not run.");
                }
                if path != current {
                    device.record(now, "path", &format!("switched from {current:?} to {path:?} by the operator"));
                }
                device.move_to(Stage::Planned, now, "retry requested by the operator")?;
                self.record(&AuditEntry::new("migration.retry", &format!("Retry {name} on {path:?}"), Ok(()), json!({ "project": project.id, "device": name })));
                Ok(format!("{name} is back in its wave, to go through {}. Launch the wave to send it again.", if path == DeliveryPath::Rest { "N-central" } else { "Ecoverse" }))
            }
            DeviceOp::Resolve => {
                project.devices[index].move_to(Stage::Resolved, now, "marked resolved by the operator").map_err(|_| format!("{name} is {stage:?}, and can't be marked resolved from there."))?;
                self.record(&AuditEntry::new("migration.resolve", &format!("Marked {name} resolved"), Ok(()), json!({ "project": project.id, "device": name, "from": format!("{stage:?}") })));
                Ok(format!("{name} is marked resolved. N-voke won't track it any more."))
            }
            DeviceOp::ChooseMatch { dest_device_id } => {
                if !dest_device_id.chars().all(|c| c.is_ascii_digit()) || dest_device_id.is_empty() {
                    return Err("A destination device ID is a number.".into());
                }
                if !matches!(stage, Stage::Staged | Stage::StuckStaged | Stage::AgentRemoved | Stage::Lost) {
                    return Err(format!("{name} is {stage:?}: a match is only needed while it is moving."));
                }
                let device = &mut project.devices[index];
                if !device.match_candidates.is_empty() && !device.match_candidates.iter().any(|c| c.device_id == dest_device_id) {
                    return Err(format!("{dest_device_id} isn't one of the devices that fit {name}."));
                }
                device.dest_device_id = Some(dest_device_id.clone());
                device.match_candidates.clear();
                device.record(now, "match", &format!("the operator chose destination device {dest_device_id}"));
                self.record(&AuditEntry::new("migration.match", &format!("Matched {name} to destination device {dest_device_id}"), Ok(()), json!({ "project": project.id, "device": name })));
                Ok(format!("{name} is matched to destination device {dest_device_id}. The next poll picks it up."))
            }
        }
    }

    /// `migration_device_op` on a saved project: lease, apply, save.
    pub async fn migration_device_op_saved(&self, id: &str, instance: &str, index: usize, op: DeviceOp) -> Result<(Project, String), String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        let message = self.migration_device_op(&mut project, index, op).await?;
        self.migration_save(&mut project).await?;
        Ok((project, message))
    }

    /// One polling pass on a saved project: lease, poll, save. The step a background loop repeats.
    pub async fn migration_poll_saved(&self, id: &str, instance: &str, thresholds: &Thresholds) -> Result<(Project, PollReport), String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        let report = self.migration_poll(&mut project, thresholds).await;
        self.migration_save(&mut project).await?;
        Ok((project, report))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        migration::project::{DestEcoverse, Stage},
        ncentral::rest::testing::{Fake, auth_ok, ok},
    };

    async fn setup() -> (std::sync::Arc<Core>, Project) {
        let core = Core::for_tests("workflow");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let fake = Fake::new(|req, _| {
            let url = req.url.trim_start_matches("https://");
            let (host, path) = url.split_once('/').unwrap();
            let path = path.split('?').next().unwrap();
            let units = |p: u32| {
                json!({ "data": [
                    { "orgUnitId": p.to_string(), "orgUnitName": "SO", "orgUnitType": "SO" },
                    { "orgUnitId": (p + 10).to_string(), "orgUnitName": "Acme", "orgUnitType": "CUSTOMER", "parentId": p.to_string() },
                ], "totalPages": 1 })
            };
            match (host, path) {
                (_, "api/auth/authenticate") => auth_ok(3600),
                (_, "api/server-info") => ok(json!({ "ncentral": "2026.4.1.1", "version": "1.3.23" })),
                ("src.example.com", "api/org-units") => ok(units(0)),
                ("dst.example.com", "api/org-units") => ok(units(100)),
                ("src.example.com", "api/org-units/10/devices") => ok(json!({ "data": [
                    { "deviceId": 2, "longName": "B-PC", "osId": "winnt", "customerId": 10 },
                    { "deviceId": 1, "longName": "A-PC", "osId": "winnt", "customerId": 10 },
                    { "deviceId": 3, "longName": "MAC", "osId": "macos", "customerId": 10 },
                ], "totalPages": 1 })),
                ("dst.example.com", "api/org-units/110/registration-token") => ok(json!({ "data": { "registrationToken": "abcd-1234-efgh", "registrationTokenExpiryDate": "2999-01-01T00:00:00-05:00" } })),
                other => panic!("unexpected {other:?}"),
            }
        });
        core.set_rest_transport(fake);
        let project = core.migration_create("Acme", &src.id, &dst.id, DestEcoverse::NotLinked).await.unwrap();
        (core, project)
    }

    #[tokio::test]
    async fn planning_adds_only_ready_devices_reports_the_rest_and_is_safe_to_repeat() {
        let (core, mut project) = setup().await;
        core.migration_set_rest_script(&mut project, "77", "0.1.0+abc1234").unwrap();
        let (report, survey) = core.migration_plan_into(&mut project, None).await.unwrap();
        assert_eq!((report.added, report.already_in), (2, 0));
        assert_eq!(report.blocked.len(), 1);
        assert_eq!(report.blocked[0].0, "MAC");
        assert!(report.blocked[0].1[0].contains("Not a Windows device"));
        assert_eq!(survey.planned.len(), 3);
        assert!(project.devices.iter().all(|d| d.stage == Stage::Planned && d.path.is_some()));

        let (again, _) = core.migration_plan_into(&mut project, None).await.unwrap();
        assert_eq!((again.added, again.already_in), (0, 2));
        assert_eq!(project.devices.len(), 2);
    }

    #[tokio::test]
    async fn without_the_uploaded_script_rest_devices_are_blocked_not_added() {
        let (core, mut project) = setup().await;
        let (report, _) = core.migration_plan_into(&mut project, None).await.unwrap();
        assert_eq!(report.added, 0);
        assert!(report.blocked.iter().any(|(_, reasons)| reasons.iter().any(|r| r.contains("Export and upload it in Prepare"))));
    }

    #[test]
    fn the_script_item_must_be_a_number_and_setting_it_is_audited() {
        let core = Core::for_tests("restscript");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut project = rt.block_on(async {
            let s = core.save_ncentral_server(None, "Old", "src.example.com", Some("j".into()), None).await.unwrap();
            Project::new("m".into(), "x", &s.id, &s.id, DestEcoverse::SameTenant, chrono::Utc::now())
        });
        for bad in ["", "7a", "../1", "7 7"] {
            assert!(core.migration_set_rest_script(&mut project, bad, "v").is_err(), "{bad:?}");
        }
        core.migration_set_rest_script(&mut project, "77", "0.1.0+abc1234").unwrap();
        assert_eq!(project.rest_scripts[&project.source_server_id].item_id, "77");
        assert!(core.audit_log(None).unwrap().entries.iter().any(|e| e.action == "migration.restScript.set"));
    }

    #[tokio::test]
    async fn a_project_is_opened_for_writing_by_one_instance_at_a_time() {
        let (core, project) = setup().await;
        assert!(core.migration_open_for_write(&project.id, "a").await.is_ok());
        let error = core.migration_open_for_write(&project.id, "b").await.unwrap_err();
        assert!(error.contains("Another N-voke window"), "{error}");
        assert!(core.migration_open_for_write("mig-nope", "a").await.unwrap_err().contains("no migration project"));
    }

    #[tokio::test]
    async fn the_poll_step_saves_what_it_changed() {
        let (core, mut project) = setup().await;
        core.migration_set_rest_script(&mut project, "77", "v").unwrap();
        core.migration_plan_into(&mut project, None).await.unwrap();
        project.auto_wave("Pilot", None, chrono::Utc::now()).unwrap();
        core.migration_save(&mut project).await.unwrap();
        let (polled, report) = core.migration_poll_saved(&project.id, "a", &Thresholds::default()).await.unwrap();
        assert_eq!(report.checked, 0, "nothing has been launched");
        assert_eq!(core.migration_load(&project.id).await.unwrap().unwrap(), polled);
        assert!(core.migration_poll_saved(&project.id, "b", &Thresholds::default()).await.is_err(), "the second instance is locked out");
    }

    fn device_project() -> Project {
        use crate::migration::planner::PlannedDevice;
        let now = chrono::Utc::now();
        let mut p = Project::new("m".into(), "Acme", "src", "dst", DestEcoverse::NotLinked, now);
        let planned = |id: &str, path: DeliveryPath, asset: Option<&str>| PlannedDevice {
            device_id: id.into(), name: format!("PC-{id}"), source_customer_id: Some("10".into()), source_site_id: None, dest_org_unit_id: Some("110".into()),
            path: Some(path), asset_id: asset.map(String::from), blockers: vec![], warnings: vec![], excluded: false,
        };
        p.add_planned(&[planned("1", DeliveryPath::Ecoverse, Some("a1")), planned("2", DeliveryPath::Rest, None)], now);
        p.rest_scripts.insert("src".into(), RestScript { item_id: "77".into(), version: "v".into() });
        p.ecoverse_script_id = Some("script-1".into());
        p
    }

    #[tokio::test]
    async fn a_failed_launch_can_be_retried_on_the_same_path_or_the_other_and_the_switch_is_recorded() {
        let core = Core::for_tests("retry");
        let mut p = device_project();
        p.devices[0].stage = Stage::LaunchFailed;
        p.devices[0].last_error = Some("boom".into());
        p.devices[0].task_id = Some("t".into());
        let message = core.migration_device_op(&mut p, 0, DeviceOp::Retry { other_path: true, checked: false }).await.unwrap();
        assert!(message.contains("N-central"), "{message}");
        let d = &p.devices[0];
        assert_eq!((d.stage, d.path, d.task_id.clone(), d.last_error.clone()), (Stage::Planned, Some(DeliveryPath::Rest), None, None));
        assert!(d.events.iter().any(|e| e.kind == "path" && e.detail.contains("switched from Ecoverse to Rest")));
        assert!(core.audit_log(None).unwrap().entries.iter().any(|e| e.action == "migration.retry"));

        // On the same path nothing is switched.
        p.devices[1].stage = Stage::StuckStaged;
        core.migration_device_op(&mut p, 1, DeviceOp::Retry { other_path: false, checked: false }).await.unwrap();
        assert_eq!((p.devices[1].stage, p.devices[1].path), (Stage::Planned, Some(DeliveryPath::Rest)));
        assert!(p.devices[1].events.iter().all(|e| e.kind != "path"));
    }

    #[tokio::test]
    async fn the_other_path_must_be_usable_and_only_a_stuck_failed_or_unpicked_device_can_be_retried() {
        let core = Core::for_tests("retry2");
        let mut p = device_project();
        p.devices[0].stage = Stage::LaunchFailed;
        p.devices[1].stage = Stage::Staged;
        assert!(core.migration_device_op(&mut p, 1, DeviceOp::Retry { other_path: false, checked: false }).await.unwrap_err().contains("only a device whose launch failed"));
        // PC-2 has no Ecoverse asset; PC-1 (Ecoverse) can't go to N-central without the uploaded script.
        p.devices[1].stage = Stage::LaunchFailed;
        assert!(core.migration_device_op(&mut p, 1, DeviceOp::Retry { other_path: true, checked: false }).await.unwrap_err().contains("Ecoverse has no asset"));
        p.rest_scripts.clear();
        assert!(core.migration_device_op(&mut p, 0, DeviceOp::Retry { other_path: true, checked: false }).await.unwrap_err().contains("hasn't been uploaded"));
        assert_eq!(p.devices[0].stage, Stage::LaunchFailed, "a refused retry changes nothing");
        p.ecoverse_script_id = None;
        assert!(core.migration_device_op(&mut p, 0, DeviceOp::Retry { other_path: false, checked: false }).await.unwrap_err().contains("isn't published to Ecoverse"));
        assert!(core.migration_device_op(&mut p, 9, DeviceOp::Resolve).await.is_err());
    }

    async fn use_a_saved_source(core: &Core, p: &mut Project) {
        let server = core.save_ncentral_server(None, "Old", "src.example.com", Some("jwt".into()), None).await.unwrap();
        let script = p.rest_scripts.remove("src").unwrap();
        p.rest_scripts.insert(server.id.clone(), script);
        p.source_server_id = server.id;
    }

    #[tokio::test]
    async fn retrying_an_unpicked_ecoverse_launch_cancels_it_first_and_changes_nothing_if_it_cant() {
        let core = Core::for_tests("retry3");
        let mut p = device_project();
        use_a_saved_source(&core, &mut p).await;
        p.devices[0].stage = Stage::NotPickedUp;
        p.devices[0].task_id = Some("task-1".into());
        // The cancel can't be sent (no API reachable): the device must stay as it is, still owning its task.
        assert!(core.migration_device_op(&mut p, 0, DeviceOp::Retry { other_path: false, checked: false }).await.is_err());
        assert_eq!((p.devices[0].stage, p.devices[0].task_id.as_deref()), (Stage::NotPickedUp, Some("task-1")));
        let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let log = seen.clone();
        core.set_graphql_fake(move |query, variables| {
            assert!(query.contains("scriptTasksCancel") && !query.contains("items"));
            log.lock().unwrap().push(variables.clone());
            (200, json!({ "data": { "scriptTasksCancel": { "errors": [] } } }).to_string())
        });
        core.migration_device_op(&mut p, 0, DeviceOp::Retry { other_path: true, checked: false }).await.unwrap();
        assert_eq!(seen.lock().unwrap()[0]["ids"], json!(["task-1"]));
        assert_eq!((p.devices[0].stage, p.devices[0].path), (Stage::Planned, Some(DeliveryPath::Rest)));
    }

    #[tokio::test]
    async fn dry_run_holds_the_cancel_back_so_the_retry_changes_nothing() {
        let core = Core::for_tests("retry4");
        core.set_dry_run(true);
        let mut p = device_project();
        use_a_saved_source(&core, &mut p).await;
        p.devices[0].stage = Stage::NotPickedUp;
        p.devices[0].task_id = Some("task-1".into());
        let error = core.migration_device_op(&mut p, 0, DeviceOp::Retry { other_path: false, checked: false }).await.unwrap_err();
        assert!(crate::is_dry_run_error(&error));
        assert_eq!(p.devices[0].stage, Stage::NotPickedUp);
        assert_eq!(core.dry_run_log()[0].action, "script.cancel");
    }

    #[tokio::test]
    async fn a_device_fixed_by_hand_is_marked_resolved_from_where_the_diagram_allows() {
        let core = Core::for_tests("resolve");
        let mut p = device_project();
        for stage in [Stage::Lost, Stage::WrongPlace, Stage::StuckStaged, Stage::LaunchFailed, Stage::NotPickedUp, Stage::Unreachable] {
            p.devices[0].stage = stage;
            assert!(core.migration_device_op(&mut p, 0, DeviceOp::Resolve).await.is_ok(), "{stage:?}");
            assert_eq!(p.devices[0].stage, Stage::Resolved);
        }
        p.devices[1].stage = Stage::Healthy;
        assert!(core.migration_device_op(&mut p, 1, DeviceOp::Resolve).await.unwrap_err().contains("can't be marked resolved"));
        assert_eq!(p.devices[1].stage, Stage::Healthy);
    }

    #[tokio::test]
    async fn choosing_a_match_sets_the_destination_device_and_only_from_the_offered_ones() {
        use crate::migration::project::MatchCandidate;
        let core = Core::for_tests("match");
        let mut p = device_project();
        p.devices[0].stage = Stage::AgentRemoved;
        p.devices[0].match_candidates = vec![MatchCandidate { device_id: "5001".into(), name: "PC-1".into() }, MatchCandidate { device_id: "5002".into(), name: "pc-1".into() }];
        assert!(core.migration_device_op(&mut p, 0, DeviceOp::ChooseMatch { dest_device_id: "9999".into() }).await.unwrap_err().contains("isn't one of the devices that fit"));
        assert!(core.migration_device_op(&mut p, 0, DeviceOp::ChooseMatch { dest_device_id: "5001; drop".into() }).await.is_err());
        core.migration_device_op(&mut p, 0, DeviceOp::ChooseMatch { dest_device_id: "5002".into() }).await.unwrap();
        assert_eq!((p.devices[0].dest_device_id.as_deref(), p.devices[0].match_candidates.len()), (Some("5002"), 0));
        assert!(p.devices[0].events.iter().any(|e| e.kind == "match"));
        p.devices[1].stage = Stage::Healthy;
        assert!(core.migration_device_op(&mut p, 1, DeviceOp::ChooseMatch { dest_device_id: "1".into() }).await.unwrap_err().contains("only needed while it is moving"));
        // Without offered candidates (chosen by ID on the command line) any destination device ID is accepted.
        p.devices[1].stage = Stage::Lost;
        assert!(core.migration_device_op(&mut p, 1, DeviceOp::ChooseMatch { dest_device_id: "7".into() }).await.is_ok());
    }

    #[tokio::test]
    async fn the_saved_form_leases_applies_and_saves() {
        let (core, mut project) = setup().await;
        core.migration_set_rest_script(&mut project, "77", "v").unwrap();
        core.migration_plan_into(&mut project, None).await.unwrap();
        project.devices[0].stage = Stage::Lost;
        core.migration_save(&mut project).await.unwrap();
        let (saved, message) = core.migration_device_op_saved(&project.id, "a", 0, DeviceOp::Resolve).await.unwrap();
        assert!(message.contains("marked resolved"));
        assert_eq!(core.migration_load(&project.id).await.unwrap().unwrap().devices[0].stage, Stage::Resolved);
        assert_eq!(saved.devices[0].stage, Stage::Resolved);
        assert!(core.migration_device_op_saved(&project.id, "b", 0, DeviceOp::Resolve).await.is_err(), "the lease is held by the first");
    }

    #[tokio::test]
    async fn a_project_is_created_with_its_ecoverse_organizations_and_planned_waved_and_saved() {
        let (core, _) = setup().await;
        let servers = core.ncentral_servers().await.unwrap();
        let (src, dst) = (servers[0].id.clone(), servers[1].id.clone());
        let project = core.migration_create_full("Full", &src, &dst, DestEcoverse::SameTenant, vec!["org-a".into()], vec!["org-b".into()]).await.unwrap();
        let saved = core.migration_load(&project.id).await.unwrap().unwrap();
        assert_eq!((saved.source_ecoverse_org_ids.clone(), saved.dest_ecoverse_org_ids.clone()), (vec!["org-a".to_string()], vec!["org-b".to_string()]));

        core.set_graphql_fake(|_, _| (200, json!({ "data": { "page": { "totalCount": 0, "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": [] } } }).to_string()));
        // Without a REST script the devices are blocked, so nothing is added and no wave can be made.
        let (planned, report) = core.migration_plan_saved(&project.id, "a", None).await.unwrap();
        assert_eq!((planned.devices.len(), report.added), (0, 0));
        assert!(core.migration_wave_saved(&project.id, "a", "Pilot", None).await.unwrap_err().contains("No device is waiting"));
    }

    #[tokio::test]
    async fn a_wave_of_the_next_waiting_devices_is_made_on_the_saved_project() {
        let (core, mut project) = setup().await;
        core.migration_set_rest_script(&mut project, "77", "v").unwrap();
        core.migration_plan_into(&mut project, None).await.unwrap();
        core.migration_save(&mut project).await.unwrap();
        let (saved, message) = core.migration_wave_saved(&project.id, "a", "Pilot", Some(1)).await.unwrap();
        assert_eq!(message, "Made wave 1 with 1 device. Launch it with L.");
        assert_eq!(saved.devices.iter().filter(|d| d.wave == Some(1)).count(), 1);
        assert_eq!(core.migration_load(&project.id).await.unwrap().unwrap().waves.len(), 1);
    }

    #[tokio::test]
    async fn publishing_needs_an_ecoverse_organization_and_goes_through_the_api() {
        let (core, project) = setup().await;
        assert!(core.migration_publish_saved(&project.id, "a").await.unwrap_err().contains("Name the Ecoverse organization"));
        let mut p = core.migration_load(&project.id).await.unwrap().unwrap();
        p.source_ecoverse_org_ids = vec!["org-1".into()];
        core.migration_save(&mut p).await.unwrap();
        core.set_graphql_fake(|query, _| {
            if query.contains("FindScripts") {
                return (200, json!({ "data": { "byName": { "nodes": [] }, "byText": { "nodes": [] } } }).to_string());
            }
            assert!(query.contains("CreateScript"));
            (200, json!({ "data": { "scriptCreate": { "item": { "id": "script-9" }, "errors": [] } } }).to_string())
        });
        let (saved, message) = core.migration_publish_saved(&project.id, "a").await.unwrap();
        assert_eq!(saved.ecoverse_script_id.as_deref(), Some("script-9"));
        assert!(message.starts_with("Published N-voke migration 0.1.0 ("), "{message}");
        assert_eq!(core.migration_load(&project.id).await.unwrap().unwrap().ecoverse_script_id.as_deref(), Some("script-9"));
    }

    #[tokio::test]
    async fn missing_tokens_are_generated_valid_ones_left_alone_and_expired_ones_reported() {
        use crate::ncentral::rest::{Method, RestRequest, testing::{Fake, auth_ok, ok, status}};
        let core = Core::for_tests("tokens");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let generated = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let seen = generated.clone();
        let fake = Fake::new(move |req: &RestRequest, _| {
            let url = req.url.trim_start_matches("https://");
            let (host, path) = url.split_once('/').unwrap();
            let path = path.split('?').next().unwrap();
            let units = |p: u32| json!({ "data": [
                { "orgUnitId": p.to_string(), "orgUnitName": "SO", "orgUnitType": "SO" },
                { "orgUnitId": (p + 10).to_string(), "orgUnitName": "Acme", "orgUnitType": "CUSTOMER", "parentId": p.to_string() },
                { "orgUnitId": (p + 20).to_string(), "orgUnitName": "Globex", "orgUnitType": "CUSTOMER", "parentId": p.to_string() },
                { "orgUnitId": (p + 30).to_string(), "orgUnitName": "Initech", "orgUnitType": "CUSTOMER", "parentId": p.to_string() },
                { "orgUnitId": (p + 40).to_string(), "orgUnitName": "Umbrella", "orgUnitType": "CUSTOMER", "parentId": p.to_string() },
            ], "totalPages": 1 });
            let dev = |id: u32, c: u32| json!({ "deviceId": id, "longName": format!("PC-{id}"), "osId": "winnt", "customerId": c });
            match (req.method, host, path) {
                (_, _, "api/auth/authenticate") => auth_ok(3600),
                (_, _, "api/server-info") => ok(json!({ "ncentral": "2026.4.1.1", "version": "1.3.23" })),
                (_, "src.example.com", "api/org-units") => ok(units(0)),
                (_, "dst.example.com", "api/org-units") => ok(units(100)),
                (_, "src.example.com", p) if p.starts_with("api/org-units/") && p.ends_with("/devices") => {
                    let c: u32 = p.split('/').nth(2).unwrap().parse().unwrap();
                    ok(json!({ "data": [dev(c + 1, c)], "totalPages": 1 }))
                }
                (Method::Get, "dst.example.com", "api/org-units/110/registration-token") => ok(json!({ "data": { "registrationToken": null, "registrationTokenExpiryDate": null } })),
                (Method::Get, "dst.example.com", "api/org-units/120/registration-token") => ok(json!({ "data": { "registrationToken": "valid-token-1234", "registrationTokenExpiryDate": "2999-01-01T00:00:00-05:00" } })),
                (Method::Get, "dst.example.com", "api/org-units/130/registration-token") => ok(json!({ "data": { "registrationToken": "old-token-12345", "registrationTokenExpiryDate": "2020-01-01T00:00:00-05:00" } })),
                (Method::Get, "dst.example.com", "api/org-units/140/registration-token") => ok(json!({ "data": { "registrationToken": null, "registrationTokenExpiryDate": null } })),
                (Method::Get, "dst.example.com", p) if p.ends_with("/software/installers") => ok(json!({ "data": [{ "softwareId": "101", "operatingSystem": "winnt" }] })),
                (Method::Post, "dst.example.com", p) if p.ends_with("/software/installers") => {
                    seen.lock().unwrap().push(p.to_string());
                    // Initech's generation is refused, to see a failure reported without stopping the rest.
                    if p.contains("/140/") { status(403, r#"{"message":"not allowed"}"#) } else { status(201, "{}") }
                }
                other => panic!("unexpected {other:?}"),
            }
        });
        core.set_rest_transport(fake);
        let mut project = core.migration_create("T", &src.id, &dst.id, DestEcoverse::NotLinked).await.unwrap();
        project.rest_scripts.insert(src.id.clone(), RestScript { item_id: "1".into(), version: "v".into() });
        let report = core.migration_generate_tokens(&project, None).await.unwrap();
        assert_eq!(report.generated, ["Acme"]);
        assert_eq!((report.already_fine, report.expired.as_slice()), (1, ["Initech".to_string()].as_slice()));
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "Umbrella");
        assert_eq!(generated.lock().unwrap().iter().filter(|p| p.contains("/110/")).count(), 1, "one call, for the unit that had none");
        let log = serde_json::to_string(&core.audit_log(None).unwrap().entries).unwrap();
        assert!(log.contains("Generated a registration token for Acme") && !log.contains("valid-token"), "{log}");

        // Dry-run lists the change and sends nothing.
        core.set_dry_run(true);
        let held = core.migration_generate_tokens(&project, None).await.unwrap();
        assert_eq!((held.held_back, held.generated.len()), (2, 0));
    }

    #[tokio::test]
    async fn mapping_decisions_are_stored_validated_and_audited() {
        let core = Core::for_tests("mapping");
        let mut p = Project::new("m".into(), "x", "a", "b", DestEcoverse::NotLinked, chrono::Utc::now());
        core.migration_set_mapping(&mut p, "10", Override::MapTo("110".into())).unwrap();
        core.migration_set_mapping(&mut p, "20", Override::Exclude).unwrap();
        assert_eq!(p.mapping_overrides["10"], Override::MapTo("110".into()));
        assert_eq!(p.mapping_overrides["20"], Override::Exclude);
        for bad in [("", "1"), ("1/2", "1"), ("10", "x")] {
            assert!(core.migration_set_mapping(&mut p, bad.0, Override::MapTo(bad.1.into())).is_err(), "{bad:?}");
        }
        let log = core.audit_log(None).unwrap().entries;
        assert!(log.iter().any(|e| e.action == "migration.mapping.set" && e.summary == "Mapped source 10 to destination 110"));
        assert!(log.iter().any(|e| e.summary == "Left source 20 out of the migration"));
    }
}
