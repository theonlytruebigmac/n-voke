//! Launching a wave: one `assetsScriptRun` per destination customer for devices reached
//! through Ecoverse, one direct task per device for those reached through N-central. The
//! grouping and the pairing of tasks back to devices are pure and tested on their own;
//! `Core::migration_launch_wave` does the calls and records what happened on the project.
//!
//! Rules from docs/migration/04-delivery-paths.md: every Ecoverse launch is queued for an
//! offline device (`run_when_online`); a launch is never retried automatically, because a
//! request that looked like it failed may still have created tasks; a device Ecoverse can't
//! reach falls back to the REST path in the same wave; registration tokens are read from the
//! destination at launch and never stored.

use std::collections::BTreeMap;

use chrono::Utc;
use serde::Serialize;
use serde_json::json;

use super::{
    packager::{LaunchParams, Operation, Package, SCRIPT_TIMEOUT_SECONDS},
    planner::DeliveryPath,
    project::{CleanMode, DestEcoverse, Project, Stage},
};
use crate::{
    Core,
    api::{RunResult, RunScriptInput},
    audit::AuditEntry,
    ncentral::{model::TokenStatus, rest::DirectTask},
};

/// Devices to launch through Ecoverse together: same destination, so the same inputs. Indexes are into `Project::devices`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EcoGroup {
    pub dest_org_unit_id: String,
    pub devices: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WavePlan {
    pub ecoverse: Vec<EcoGroup>,
    pub rest: Vec<usize>,
    /// Devices that can't be launched as planned, with why.
    pub problems: Vec<(usize, String)>,
}

/// What is in a wave and how each device goes. Only devices still `Planned` are launched: that is what stops a double launch.
pub fn plan_wave(project: &Project, wave: u32) -> WavePlan {
    let mut plan = WavePlan::default();
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, device) in project.devices.iter().enumerate().filter(|(_, d)| d.wave == Some(wave) && d.stage == Stage::Planned) {
        if device.launch_unsure {
            plan.problems.push((i, "A launch for it was sent but never recorded (the app stopped, or the answer was lost), so it may be running. Check Activity or the task list, then retry it only if it didn't run.".into()));
            continue;
        }
        let Some(dest) = device.dest_org_unit_id.clone() else {
            plan.problems.push((i, "It has no destination customer or site.".into()));
            continue;
        };
        match device.path {
            Some(DeliveryPath::Ecoverse) if device.asset_id.is_some() => groups.entry(dest).or_default().push(i),
            Some(DeliveryPath::Ecoverse) => plan.problems.push((i, "It was planned on Ecoverse but has no Ecoverse asset.".into())),
            Some(DeliveryPath::Rest) => plan.rest.push(i),
            None => plan.problems.push((i, "It has no delivery path.".into())),
        }
    }
    plan.ecoverse = groups.into_iter().map(|(dest_org_unit_id, devices)| EcoGroup { dest_org_unit_id, devices }).collect();
    plan
}

/// What to do with a device an Ecoverse run didn't create a task for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    /// Ecoverse can't reach this device (no permission, no asset, no script): try N-central.
    FallBackToRest,
    Failed(String),
    /// The request may have been carried out even so; the device must not be launched again until checked.
    Unsure(String),
}

/// Whether an Ecoverse error leaves it open that the run was created: the answer never came, or the server
/// failed. A refusal (access denied, rate limit, the API saying it created no run) is not.
fn eco_outcome_unknown(error: &str) -> bool {
    error.starts_with("Request to") || error.starts_with("Could not read the response") || error.contains("HTTP 5") || error.contains("non-JSON") || error.contains("no data")
}

/// Text with the registration tokens in use removed, for anything an outside server said back.
fn scrub_tokens(text: &str, tokens: &BTreeMap<String, String>) -> String {
    let mut text = crate::ncentral::rest::scrub_secrets(text);
    for token in tokens.values().filter(|t| !t.is_empty()) {
        text = text.replace(token.as_str(), "[redacted]");
    }
    text
}

pub fn classify_missing(warnings: &[String]) -> Missing {
    let text = warnings.join("; ");
    let lower = text.to_ascii_lowercase();
    let fallback = ["notpermitted", "not permitted", "assetnotfound", "asset not found", "scriptnotfound", "script not found"];
    if fallback.iter().any(|w| lower.contains(w)) {
        Missing::FallBackToRest
    } else if text.is_empty() {
        Missing::Failed("Ecoverse created no task for this device.".into())
    } else {
        Missing::Failed(text)
    }
}

/// Pairs the tasks an Ecoverse run created with the devices that asked for them, by asset.
/// `requested` is (index into the project's devices, asset ID).
pub fn match_tasks(requested: &[(usize, String)], result: &RunResult) -> (Vec<(usize, String)>, Vec<usize>) {
    let mut launched = vec![];
    let mut missing = vec![];
    for (index, asset) in requested {
        match result.tasks.iter().find(|(_, a)| a.as_deref() == Some(asset.as_str())) {
            Some((task_id, _)) => launched.push((*index, task_id.clone())),
            None => missing.push(*index),
        }
    }
    (launched, missing)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "LaunchReport.ts"))]
#[serde(rename_all = "camelCase")]
pub struct LaunchReport {
    pub launched_ecoverse: usize,
    pub launched_rest: usize,
    /// Devices that went to the REST path because Ecoverse couldn't reach them (counted in `launched_rest` too).
    pub fell_back: usize,
    /// Held back by dry-run: nothing was sent.
    pub held_back: usize,
    /// (device name, why) for devices that didn't launch.
    pub failed: Vec<(String, String)>,
    /// Why the launch stopped part-way, when it did: the devices not reached are still waiting in the wave.
    pub stopped: Option<String>,
    /// The Ecoverse runs this launch started that Activity can watch (those read with the app's own Ecoverse key).
    pub activity: Vec<ActivityRun>,
}

/// One Ecoverse run as the Activity page shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ActivityRun.ts"))]
#[serde(rename_all = "camelCase")]
pub struct ActivityRun {
    /// The project that launched it.
    pub project_id: String,
    pub name: String,
    pub script_id: String,
    pub script_name: String,
    pub target_label: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub timeout_seconds: i64,
    /// (task ID, asset ID, device name)
    pub tasks: Vec<(String, String, String)>,
}

pub(super) fn launch_params(project: &Project, wave: u32, package: &Package, operation: Operation) -> LaunchParams {
    LaunchParams {
        operation,
        migration_id: format!("{}/wave-{wave}", project.id),
        launcher_version: package.version.clone(),
        task_repeat_minutes: project.options.task_repeat_minutes,
        task_expiry_days: project.options.task_expiry_days,
        clean_full: project.options.clean_mode == CleanMode::Full,
        expect_ufa: project.dest_ecoverse != DestEcoverse::NotLinked,
        review_days: project.options.review_days,
    }
}

impl Core {
    /// Launches every `Planned` device in the wave. Changes `project` (stages, task IDs, events) for the caller to save.
    /// Per-device problems are in the report, not an error; an error means nothing was attempted.
    pub async fn migration_launch_wave(&self, project: &mut Project, wave: u32, package: &Package, ecoverse_script_id: Option<&str>) -> Result<LaunchReport, String> {
        if !project.waves.iter().any(|w| w.number == wave) {
            return Err(format!("There is no wave {wave} in this project."));
        }
        let plan = plan_wave(project, wave);
        if plan.ecoverse.is_empty() && plan.rest.is_empty() && plan.problems.is_empty() {
            return Err("No device in this wave is waiting to be launched.".into());
        }
        if !plan.ecoverse.is_empty() && ecoverse_script_id.is_none() {
            return Err("Publish the endpoint script to Ecoverse first (Prepare).".into());
        }
        let rest_script = project.rest_scripts.get(&project.source_server_id);
        let source_item = rest_script.filter(|s| s.version == package.version).map(|s| s.item_id.clone());
        let no_rest_script = match rest_script {
            Some(s) if source_item.is_none() => format!("The endpoint script uploaded to the source server is version {}, not {}. Export and upload the current one in Prepare, then record it.", s.version, package.version),
            _ => "This device can only be reached through N-central, and the endpoint script hasn't been uploaded to that server. Export and upload it in Prepare.".to_string(),
        };
        if !plan.ecoverse.is_empty() && project.ecoverse_script_version.as_deref() != Some(package.version.as_str()) {
            return Err("The script published to Ecoverse isn't this version. Publish it again (Prepare) before launching.".into());
        }
        let live = !self.is_dry_run();
        let dest = self.ncentral_server(&project.dest_server_id)?;
        let now = Utc::now();
        let mut report = LaunchReport::default();

        for (i, why) in &plan.problems {
            let unsure = project.devices[*i].launch_unsure;
            self.fail_launch(project, *i, why, unsure, &mut report);
        }

        if live {
            let indexes: Vec<usize> = plan.ecoverse.iter().flat_map(|g| g.devices.clone()).chain(plan.rest.iter().copied()).collect();
            self.capture_before_launch(project, &indexes).await;
        }

        // Tokens are read now, one per destination, and only kept for this call.
        let mut tokens: BTreeMap<String, String> = BTreeMap::new();
        let mut unusable: BTreeMap<String, String> = BTreeMap::new();
        let destinations: std::collections::BTreeSet<String> =
            plan.ecoverse.iter().map(|g| g.dest_org_unit_id.clone()).chain(plan.rest.iter().filter_map(|&i| project.devices[i].dest_org_unit_id.clone())).collect();
        for unit in destinations {
            match self.ncentral_registration_token(&project.dest_server_id, &unit).await {
                Ok(token) => match (token.status(now, chrono::Duration::zero()), token.registration_token) {
                    (TokenStatus::Valid { .. } | TokenStatus::Expiring { .. }, Some(value)) => {
                        tokens.insert(unit, value);
                    }
                    (TokenStatus::Missing, _) => {
                        unusable.insert(unit, format!("{} has no registration token for this destination. Generate it with T on the Migrate page, or: nvoke migrate-project generate-tokens <project>.", dest.label));
                    }
                    (TokenStatus::Expired { .. }, _) => {
                        unusable.insert(unit, "The destination's registration token has expired. Generate a new one in N-central.".into());
                    }
                    _ => {
                        unusable.insert(unit, "The destination's registration token has no readable expiry, so it isn't used.".into());
                    }
                },
                Err(e) => {
                    unusable.insert(unit, format!("Could not read the destination's registration token: {e}"));
                }
            }
        }

        let mut rest_queue: Vec<(usize, Option<String>)> = plan.rest.iter().map(|&i| (i, None)).collect();

        'groups: for group in &plan.ecoverse {
            if let Some(why) = unusable.get(&group.dest_org_unit_id) {
                for &i in &group.devices {
                    self.fail_launch(project, i, why, false, &mut report);
                }
                continue;
            }
            let operation = Operation::Launch { dest_server: dest.host.clone(), dest_customer_id: group.dest_org_unit_id.clone(), registration_token: tokens[&group.dest_org_unit_id].clone() };
            let params = launch_params(project, wave, package, operation);
            let inputs = match params.ecoverse_inputs() {
                Ok(inputs) => inputs,
                Err(e) => {
                    for &i in &group.devices {
                        self.fail_launch(project, i, &e, false, &mut report);
                    }
                    continue;
                }
            };
            if live && !self.mark_in_flight(project, &group.devices, &mut report).await {
                break 'groups;
            }
            let requested: Vec<(usize, String)> = group.devices.iter().map(|&i| (i, project.devices[i].asset_id.clone().unwrap_or_default())).collect();
            let input = RunScriptInput {
                script_id: ecoverse_script_id.unwrap_or_default().to_string(),
                script_name: package.script_name(),
                target_label: format!("wave {wave}, destination {}", group.dest_org_unit_id),
                asset_ids: requested.iter().map(|(_, a)| a.clone()).collect(),
                name: format!("N-voke migration: launch wave {wave}"),
                timeout_seconds: i64::from(SCRIPT_TIMEOUT_SECONDS),
                inputs,
                run_when_online: true,
            };
            let result = self.with_ecoverse(&project.source_server_id, self.run_script(&input)).await;
            let missing: Vec<(usize, Missing)> = match result {
                Ok(run) => {
                    let (launched, missing) = match_tasks(&requested, &run);
                    let watchable = self.ncentral_server(&project.source_server_id).is_ok_and(|s| s.ecoverse == crate::ncentral::EcoCredential::AppKey);
                    if watchable && !launched.is_empty() {
                        report.activity.push(ActivityRun {
                            project_id: project.id.clone(),
                            name: input.name.clone(),
                            script_id: input.script_id.clone(),
                            script_name: input.script_name.clone(),
                            target_label: input.target_label.clone(),
                            timeout_seconds: input.timeout_seconds,
                            tasks: launched.iter().map(|(i, task_id)| (task_id.clone(), project.devices[*i].asset_id.clone().unwrap_or_default(), project.devices[*i].name.clone())).collect(),
                        });
                    }
                    for (i, task_id) in launched {
                        self.mark_launched(project, i, DeliveryPath::Ecoverse, &task_id, "Ecoverse");
                        report.launched_ecoverse += 1;
                    }
                    let kind = classify_missing(&run.warnings);
                    missing.into_iter().map(|i| (i, kind.clone())).collect()
                }
                Err(e) if crate::is_dry_run_error(&e) => {
                    report.held_back += requested.len();
                    vec![]
                }
                // The request may have been accepted even though the answer was lost: don't send it again.
                Err(e) => {
                    let e = scrub_tokens(&e, &tokens);
                    if eco_outcome_unknown(&e) {
                        requested.iter().map(|(i, _)| (*i, Missing::Unsure(format!("{e} Check Activity before launching these again: the request may have gone through.")))).collect()
                    } else {
                        requested.iter().map(|(i, _)| (*i, classify_missing(std::slice::from_ref(&e)))).collect()
                    }
                }
            };
            for (i, what) in missing {
                project.devices[i].launch_unsure = false;
                match what {
                    Missing::FallBackToRest => rest_queue.push((i, Some("Ecoverse can't reach it".into()))),
                    Missing::Failed(why) => self.fail_launch(project, i, &scrub_tokens(&why, &tokens), false, &mut report),
                    Missing::Unsure(why) => self.fail_launch(project, i, &why, true, &mut report),
                }
            }
        }

        for (i, fallback) in rest_queue {
            if report.stopped.is_some() {
                break;
            }
            let dest_unit = project.devices[i].dest_org_unit_id.clone().unwrap_or_default();
            if let Some(why) = unusable.get(&dest_unit) {
                self.fail_launch(project, i, why, false, &mut report);
                continue;
            }
            let Some(item_id) = source_item.as_deref() else {
                self.fail_launch(project, i, &no_rest_script, false, &mut report);
                continue;
            };
            let Some(customer_id) = project.devices[i].source_customer_id.clone() else {
                self.fail_launch(project, i, "Its source customer isn't known, which the N-central launch needs.", false, &mut report);
                continue;
            };
            let operation = Operation::Launch { dest_server: dest.host.clone(), dest_customer_id: dest_unit.clone(), registration_token: tokens.get(&dest_unit).cloned().unwrap_or_default() };
            let command_line = match launch_params(project, wave, package, operation).command_line() {
                Ok(line) => line,
                Err(e) => {
                    self.fail_launch(project, i, &e, false, &mut report);
                    continue;
                }
            };
            if live && !self.mark_in_flight(project, &[i], &mut report).await {
                break;
            }
            let device = &project.devices[i];
            let (name, device_id) = (device.name.clone(), device.source_device_id.clone());
            let task = DirectTask { name: &format!("N-voke migration: launch wave {wave}"), item_id, customer_id: &customer_id, device_id: &device_id, device_label: &name, command_line: &command_line };
            match self.ncentral_run_direct_task(&project.source_server_id, &task).await {
                Ok(task_id) => {
                    let how = fallback.as_deref().map_or("N-central".to_string(), |why| format!("N-central ({why})"));
                    self.mark_launched(project, i, DeliveryPath::Rest, &task_id, &how);
                    report.launched_rest += 1;
                    if fallback.is_some() {
                        report.fell_back += 1;
                    }
                }
                Err(e) if e.is_dry_run() => report.held_back += 1,
                Err(e) => {
                    let why = scrub_tokens(&e.message, &tokens);
                    let why = if e.outcome_unknown { format!("{why} Check the task list on the source server before launching it again: the request may have gone through.") } else { why };
                    self.fail_launch(project, i, &why, e.outcome_unknown, &mut report);
                }
            }
        }

        if report.launched_ecoverse + report.launched_rest > 0 {
            if let Some(w) = project.waves.iter_mut().find(|w| w.number == wave) {
                w.launched_at.get_or_insert_with(|| super::project::stamp(now));
            }
            project.updated_at = super::project::stamp(now);
            self.record(&AuditEntry::new(
                "migration.launch",
                &format!("Launched wave {wave} of {}: {} through Ecoverse, {} through N-central", project.name, report.launched_ecoverse, report.launched_rest),
                Ok(()),
                json!({ "project": project.id, "wave": wave, "ecoverse": report.launched_ecoverse, "rest": report.launched_rest, "fellBack": report.fell_back, "failed": report.failed.len() }),
            ));
        }
        Ok(report)
    }

    /// Reads what the tracker will need to recognise each device once it has moved: its identity on the source,
    /// and what is already on each destination customer or site. Best effort: a failure is noted on the device
    /// and tracking falls back to the host name alone.
    async fn capture_before_launch(&self, project: &mut Project, indexes: &[usize]) {
        let now = Utc::now();
        for &i in indexes {
            if project.devices[i].identity.is_some() {
                continue;
            }
            match self.ncentral_device_identity(&project.source_server_id, &project.devices[i].source_device_id).await {
                Ok(identity) => project.devices[i].identity = Some(identity),
                Err(e) => project.devices[i].record(now, "note", &format!("Could not read the device's identity before the move: {e}")),
            }
        }
        if project.is_reinstall() {
            return;
        }
        let units: std::collections::BTreeSet<String> = indexes.iter().filter_map(|&i| project.devices[i].dest_org_unit_id.clone()).collect();
        let missing: Vec<String> = units.into_iter().filter(|u| !project.dest_baseline.contains_key(u)).collect();
        for unit in missing {
            if let Ok(devices) = self.ncentral_org_unit_devices(&project.dest_server_id, &unit).await {
                project.dest_baseline.insert(unit, devices.into_iter().map(|d| d.device_id).collect());
            }
        }
    }

    /// Saves the devices as "launch sent, outcome not recorded" before anything is sent, so a crash or a refused
    /// save can never leave a launched device looking like it is waiting. False means the save failed and nothing
    /// was sent: the marks are taken back and the launch stops.
    async fn mark_in_flight(&self, project: &mut Project, devices: &[usize], report: &mut LaunchReport) -> bool {
        for &i in devices {
            project.devices[i].launch_unsure = true;
        }
        match self.migration_save(project).await {
            Ok(()) => true,
            Err(e) => {
                for &i in devices {
                    project.devices[i].launch_unsure = false;
                }
                report.stopped = Some(format!("Stopped before sending anything more: {e}"));
                false
            }
        }
    }

    fn mark_launched(&self, project: &mut Project, index: usize, path: DeliveryPath, task_id: &str, how: &str) {
        let now = Utc::now();
        let device = &mut project.devices[index];
        if device.path != Some(path) {
            device.record(now, "path", &format!("{how}: launched through {path:?}"));
            device.path = Some(path);
        }
        device.task_id = Some(task_id.into());
        device.attempts += 1;
        device.last_error = None;
        device.launch_unsure = false;
        let _ = device.move_to(Stage::Launched, now, &format!("{how}, task {task_id}"));
    }

    fn fail_launch(&self, project: &mut Project, index: usize, why: &str, unsure: bool, report: &mut LaunchReport) {
        let now = Utc::now();
        let device = &mut project.devices[index];
        let why = &crate::ncentral::rest::scrub_secrets(why);
        device.launch_unsure = unsure;
        device.last_error = Some(why.into());
        device.attempts += 1;
        let _ = device.move_to(Stage::LaunchFailed, now, why);
        report.failed.push((device.name.clone(), why.into()));
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        DRY_RUN_ERROR,
        migration::{
            planner::PlannedDevice,
            project::{DestEcoverse, RestScript},
        },
        ncentral::rest::{
            Method, RestRequest,
            testing::{Fake, auth_ok, ok, status},
        },
    };

    fn now() -> DateTime<Utc> {
        "2026-10-20T12:00:00Z".parse().unwrap()
    }

    fn planned(id: &str, dest: &str, path: DeliveryPath, asset: Option<&str>) -> PlannedDevice {
        PlannedDevice {
            device_id: id.into(),
            name: format!("PC-{id}"),
            source_customer_id: Some("10".into()),
            source_site_id: None,
            dest_org_unit_id: Some(dest.into()),
            path: Some(path),
            asset_id: asset.map(String::from),
            blockers: vec![],
            warnings: vec![],
            excluded: false,
        }
    }

    fn project(devices: &[PlannedDevice]) -> Project {
        let mut p = Project::new("mig-1".into(), "Acme move", "srv-src", "srv-dst", DestEcoverse::NotLinked, now());
        p.add_planned(devices, now());
        let ids: Vec<String> = devices.iter().map(|d| d.device_id.clone()).collect();
        p.create_wave("Pilot", &ids, now()).unwrap();
        p
    }

    fn package() -> Package {
        let template = include_str!("../../../../scripts/migration/endpoint/nvoke-migration.template.ps1");
        Package::from_built(&template.replace("__SCRIPT_VERSION__", "0.1.0+3f9a2c1").replace("__INSTALLER_B64__", "QUJD").replace("__INSTALLER_SHA256__", &"A".repeat(64))).unwrap()
    }

    #[test]
    fn a_wave_is_grouped_by_destination_and_path_and_only_planned_devices_are_taken() {
        let mut p = project(&[
            planned("1", "110", DeliveryPath::Ecoverse, Some("a1")),
            planned("2", "110", DeliveryPath::Ecoverse, Some("a2")),
            planned("3", "120", DeliveryPath::Ecoverse, Some("a3")),
            planned("4", "110", DeliveryPath::Rest, None),
            planned("5", "110", DeliveryPath::Ecoverse, None),
            planned("6", "110", DeliveryPath::Rest, None),
        ]);
        p.devices[5].stage = Stage::Launched;
        let plan = plan_wave(&p, 1);
        assert_eq!(plan.ecoverse, [EcoGroup { dest_org_unit_id: "110".into(), devices: vec![0, 1] }, EcoGroup { dest_org_unit_id: "120".into(), devices: vec![2] }]);
        assert_eq!(plan.rest, [3], "the device already launched is not launched again");
        assert_eq!(plan.problems.len(), 1);
        assert!(plan.problems[0].1.contains("no Ecoverse asset"));
        assert_eq!(plan_wave(&p, 9), WavePlan::default());
    }

    #[test]
    fn a_missing_task_falls_back_to_rest_only_for_reasons_ecoverse_cant_fix() {
        assert_eq!(classify_missing(&["ScriptRunNotPermittedError".into()]), Missing::FallBackToRest);
        assert_eq!(classify_missing(&["Asset not found.".into()]), Missing::FallBackToRest);
        assert_eq!(classify_missing(&["ScriptRunScriptNotFoundError".into()]), Missing::FallBackToRest);
        assert_eq!(classify_missing(&["Device is offline.".into()]), Missing::Failed("Device is offline.".into()));
        assert!(matches!(classify_missing(&[]), Missing::Failed(m) if m.contains("no task")));
    }

    #[test]
    fn tasks_are_paired_with_devices_by_asset() {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        let id = |asset: &str| B64.encode(format!("3f2c9a1e-0000:{asset}"));
        let run = RunResult { tasks: vec![(id("a2"), Some("a2".into())), (id("a1"), Some("a1".into()))], warnings: vec![] };
        let (launched, missing) = match_tasks(&[(0, "a1".into()), (1, "a2".into()), (2, "a3".into())], &run);
        assert_eq!(launched, [(0, id("a1")), (1, id("a2"))]);
        assert_eq!(missing, [2]);
    }

    /// A destination whose tokens are `tokens`, and a source that accepts direct tasks.
    fn lab(tokens: &'static [(&'static str, Option<&'static str>)]) -> std::sync::Arc<Fake> {
        lab_with(tokens, |n| status(201, &format!(r#"{{"data":{{"taskId":"{}"}}}}"#, 9000 + n)))
    }

    /// Like `lab`, with `direct` answering each direct-task request.
    fn lab_with(tokens: &'static [(&'static str, Option<&'static str>)], direct: impl Fn(usize) -> crate::ncentral::rest::RestResponse + Send + Sync + 'static) -> std::sync::Arc<Fake> {
        Fake::new(move |req: &RestRequest, n| {
            let url = req.url.trim_start_matches("https://");
            let (host, path) = url.split_once('/').unwrap();
            let path = path.split('?').next().unwrap();
            if path == "api/auth/authenticate" {
                return auth_ok(3600);
            }
            if let Some(unit) = path.strip_prefix("api/org-units/").and_then(|p| p.strip_suffix("/registration-token")) {
                assert_eq!(host, "dst.example.com");
                let token = tokens.iter().find(|(u, _)| *u == unit).and_then(|(_, t)| *t);
                let expiry = if token == Some("expired-token-0000") { "2020-01-01T00:00:00-05:00" } else { "2999-01-01T00:00:00-05:00" };
                return ok(json!({ "data": { "registrationToken": token, "registrationTokenExpiryDate": token.map(|_| expiry) } }));
            }
            if path.ends_with("/assets") {
                return ok(json!({ "data": { "computersystem": { "serialnumber": "VMW-1", "netbiosname": "PC" }, "networkadapter": { "list": [{ "macaddress": "AA-BB-CC-DD-EE-01" }] } } }));
            }
            if path.ends_with("/devices") {
                assert_eq!(host, "dst.example.com");
                return ok(json!({ "data": [{ "deviceId": 4001, "longName": "ALREADY-THERE" }], "totalPages": 1 }));
            }
            if (req.method, path) == (Method::Post, "api/scheduled-tasks/direct") {
                assert_eq!(host, "src.example.com", "a device is launched through the server it is on now");
                return direct(n);
            }
            panic!("unexpected {host}/{path}");
        })
    }

    async fn core_with(fake: &std::sync::Arc<Fake>) -> std::sync::Arc<Core> {
        let core = Core::for_tests("launch");
        core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        core.set_rest_transport(fake.clone());
        core
    }

    /// The saved servers get random IDs, so the project is pointed at them.
    async fn point_at_servers(core: &Core, p: &mut Project) {
        let servers = core.ncentral_servers().await.unwrap();
        p.source_server_id = servers.iter().find(|s| s.host == "src.example.com").unwrap().id.clone();
        p.dest_server_id = servers.iter().find(|s| s.host == "dst.example.com").unwrap().id.clone();
        p.ecoverse_script_version = Some("0.1.0+3f9a2c1".into());
    }

    #[tokio::test]
    async fn rest_devices_are_launched_with_a_fresh_token_recorded_and_audited_without_it() {
        let fake = lab(&[("110", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        let mut p = project(&[planned("9001", "110", DeliveryPath::Rest, None), planned("9002", "110", DeliveryPath::Rest, None)]);
        point_at_servers(&core, &mut p).await;
                p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });
        let report = core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap();
        assert_eq!((report.launched_rest, report.launched_ecoverse, report.fell_back), (2, 0, 0));
        assert!(report.failed.is_empty() && report.held_back == 0);
        assert!(p.devices.iter().all(|d| d.stage == Stage::Launched && d.attempts == 1 && d.task_id.is_some()));
        assert_ne!(p.devices[0].task_id, p.devices[1].task_id);
        assert!(p.waves[0].launched_at.is_some());
        assert_eq!(p.devices[0].identity.as_ref().unwrap().serial.as_deref(), Some("VMW-1"), "what identifies the device is read before it moves");
        assert_eq!(p.dest_baseline["110"], ["4001"], "and so is what is already on the destination");

        let posts: Vec<Value> = fake.seen().into_iter().filter(|r| r.method == Method::Post && r.url.ends_with("/direct")).map(|r| r.body.unwrap()).collect();
        assert_eq!(posts.len(), 2);
        let line = posts[0]["parameters"][0]["value"].as_str().unwrap();
        assert!(line.contains("-RegistrationToken abcd-1234-efgh") && line.contains("-DestServer dst.example.com -DestCustomerId 110") && line.contains("-MigrationId mig-1/wave-1"), "{line}");
        assert_eq!(posts[0]["customerId"], 10);

        let log = serde_json::to_string(&core.audit_log(None).unwrap().entries).unwrap();
        assert!(log.contains("migration.launch") && log.contains("2 through N-central"));
        assert!(!log.contains("abcd-1234-efgh"), "the token never reaches the audit log");
        assert!(!serde_json::to_string(&p).unwrap().contains("abcd-1234-efgh"), "nor the project");
    }

    #[tokio::test]
    async fn a_second_launch_of_the_same_wave_does_nothing() {
        let fake = lab(&[("110", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        let mut p = project(&[planned("9001", "110", DeliveryPath::Rest, None)]);
        point_at_servers(&core, &mut p).await;
        p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });
        core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap();
        let before = fake.seen().len();
        assert!(core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap_err().contains("No device in this wave is waiting"));
        assert_eq!(fake.seen().len(), before, "no request, so no duplicate task");
        assert!(core.migration_launch_wave(&mut p, 7, &package(), None).await.unwrap_err().contains("no wave 7"));
    }

    #[tokio::test]
    async fn a_missing_expired_or_unreadable_token_fails_only_the_devices_that_need_it() {
        let fake = lab(&[("110", None), ("120", Some("expired-token-0000")), ("130", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        let mut p = project(&[planned("1", "110", DeliveryPath::Rest, None), planned("2", "120", DeliveryPath::Rest, None), planned("3", "130", DeliveryPath::Rest, None)]);
        point_at_servers(&core, &mut p).await;
        p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });
        let report = core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap();
        assert_eq!(report.launched_rest, 1);
        assert_eq!(p.devices.iter().map(|d| d.stage).collect::<Vec<_>>(), [Stage::LaunchFailed, Stage::LaunchFailed, Stage::Launched]);
        assert!(report.failed[0].1.contains("generate-tokens") && report.failed[1].1.contains("expired"), "{:?}", report.failed);
        assert!(p.devices[0].last_error.is_some() && p.devices[2].last_error.is_none());
    }

    #[tokio::test]
    async fn without_the_uploaded_script_rest_devices_fail_with_what_to_do_and_nothing_is_sent() {
        let fake = lab(&[("110", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        let mut p = project(&[planned("1", "110", DeliveryPath::Rest, None)]);
        point_at_servers(&core, &mut p).await;
        let report = core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap();
        assert!(report.failed[0].1.contains("Export and upload it in Prepare"));
        assert!(!fake.seen().iter().any(|r| r.url.ends_with("/direct")));
    }

    #[tokio::test]
    async fn an_ecoverse_wave_needs_a_published_script() {
        let fake = lab(&[("110", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        let mut p = project(&[planned("1", "110", DeliveryPath::Ecoverse, Some("a1"))]);
        point_at_servers(&core, &mut p).await;
        assert!(core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap_err().contains("Publish the endpoint script"));
        assert_eq!(p.devices[0].stage, Stage::Planned);
    }

    #[tokio::test]
    async fn dry_run_holds_back_both_paths_changes_nothing_and_shows_the_token_blanked() {
        let fake = lab(&[("110", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        core.set_dry_run(true);
        let mut p = project(&[planned("1", "110", DeliveryPath::Ecoverse, Some("a1")), planned("9002", "110", DeliveryPath::Rest, None)]);
        point_at_servers(&core, &mut p).await;
        p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });
        let report = core.migration_launch_wave(&mut p, 1, &package(), Some("script-1")).await.unwrap();
        assert_eq!((report.held_back, report.launched_ecoverse + report.launched_rest, report.failed.len()), (2, 0, 0));
        assert!(p.devices.iter().all(|d| d.stage == Stage::Planned && d.attempts == 0), "a held-back launch leaves the wave as it was");
        assert!(p.waves[0].launched_at.is_none());
        assert!(p.devices.iter().all(|d| d.identity.is_none()) && p.dest_baseline.is_empty(), "dry-run reads and records nothing");
        let log = core.dry_run_log();
        assert_eq!(log.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(), ["script.run", "migration.launch.rest"]);
        let shown = serde_json::to_string(&log).unwrap();
        assert!(!shown.contains("abcd-1234-efgh") && shown.contains("[redacted]"), "{shown}");
        assert!(!fake.seen().iter().any(|r| r.url.ends_with("/direct")), "nothing was sent");
        assert_eq!(DRY_RUN_ERROR, "Dry run: nothing was sent to N-able.");
    }

    /// Ecoverse answers a run with one task per asset except `unreachable`, which it reports as not found.
    fn eco_answering(unreachable: &'static str, seen: std::sync::Arc<std::sync::Mutex<Vec<Value>>>) -> impl Fn(&str, &Value) -> (u16, String) + Send + Sync + 'static {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        move |query, variables| {
            assert!(query.contains("assetsScriptRun"), "{query}");
            seen.lock().unwrap().push(variables.clone());
            let assets: Vec<String> = variables["input"]["assetIds"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect();
            let items: Vec<Value> = assets.iter().filter(|a| a.as_str() != unreachable).map(|a| json!({ "id": B64.encode(format!("task-uuid:{a}")) })).collect();
            let errors = if assets.iter().any(|a| a == unreachable) { json!([{ "__typename": "ScriptRunAssetNotFoundError", "message": "Asset not found." }]) } else { json!([]) };
            (200, json!({ "data": { "assetsScriptRun": { "items": items, "errors": errors } } }).to_string())
        }
    }

    #[tokio::test]
    async fn an_ecoverse_wave_is_one_queued_run_per_destination_with_every_input_and_the_token_as_a_password() {
        let fake = lab(&[("110", Some("abcd-1234-efgh")), ("120", Some("wxyz-5678-ijkl"))]);
        let core = core_with(&fake).await;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        core.set_graphql_fake(eco_answering("none", seen.clone()));
        let mut p = project(&[planned("1", "110", DeliveryPath::Ecoverse, Some("a1")), planned("2", "110", DeliveryPath::Ecoverse, Some("a2")), planned("3", "120", DeliveryPath::Ecoverse, Some("a3"))]);
        point_at_servers(&core, &mut p).await;

        let report = core.migration_launch_wave(&mut p, 1, &package(), Some("script-1")).await.unwrap();
        assert_eq!((report.launched_ecoverse, report.launched_rest, report.failed.len()), (3, 0, 0));
        assert!(p.devices.iter().all(|d| d.stage == Stage::Launched && d.path == Some(DeliveryPath::Ecoverse)));
        let watched: Vec<usize> = report.activity.iter().map(|r| r.tasks.len()).collect();
        assert_eq!(watched, [2, 1], "each run is offered to Activity with a task per device, when the app's own key read it");
        assert_eq!(report.activity[0].tasks[0].2, p.devices[0].name);
        assert!(p.devices[0].task_id.as_deref().is_some_and(|t| t != p.devices[2].task_id.as_deref().unwrap()));

        let runs = seen.lock().unwrap().clone();
        assert_eq!(runs.len(), 2, "one call per destination customer");
        let configuration = &runs[0]["input"]["configuration"];
        assert_eq!((configuration["scriptId"].as_str(), configuration["shouldRunWhenOnline"].as_bool()), (Some("script-1"), Some(true)));
        let inputs = configuration["inputs"].as_array().unwrap();
        assert_eq!(inputs.len(), 11, "Ecoverse refuses a run that omits an input");
        let token = inputs.iter().find(|i| i["variable"] == "RegistrationToken").unwrap();
        assert_eq!((token["type"].as_str(), token["value"]["stringValue"].as_str()), (Some("PASSWORD"), Some("abcd-1234-efgh")));
        assert_eq!(runs[1]["input"]["configuration"]["inputs"].as_array().unwrap().iter().find(|i| i["variable"] == "RegistrationToken").unwrap()["value"]["stringValue"], "wxyz-5678-ijkl", "each destination gets its own token");
        let log = serde_json::to_string(&core.audit_log(None).unwrap().entries).unwrap();
        assert!(log.contains("script.run") && !log.contains("abcd-1234-efgh") && !log.contains("wxyz-5678-ijkl"), "{log}");
    }

    #[tokio::test]
    async fn a_device_ecoverse_cant_find_is_launched_through_n_central_in_the_same_wave() {
        let fake = lab(&[("110", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        core.set_graphql_fake(eco_answering("a2", std::sync::Arc::new(std::sync::Mutex::new(vec![]))));
        let mut p = project(&[planned("1", "110", DeliveryPath::Ecoverse, Some("a1")), planned("9002", "110", DeliveryPath::Ecoverse, Some("a2"))]);
        point_at_servers(&core, &mut p).await;
        p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });

        let report = core.migration_launch_wave(&mut p, 1, &package(), Some("script-1")).await.unwrap();
        assert_eq!((report.launched_ecoverse, report.launched_rest, report.fell_back), (1, 1, 1));
        assert_eq!((p.devices[0].path, p.devices[1].path), (Some(DeliveryPath::Ecoverse), Some(DeliveryPath::Rest)));
        assert!(p.devices[1].events.iter().any(|e| e.kind == "path" && e.detail.contains("Ecoverse can't reach it")), "the switch is on its timeline");
        assert!(fake.seen().iter().filter(|r| r.url.ends_with("/direct")).count() == 1);
    }

    #[tokio::test]
    async fn a_lost_answer_is_never_resent_and_says_to_check_activity() {
        let fake = lab(&[("110", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        // No fake for GraphQL: the unreachable test URL fails, as a dropped connection would.
        let mut p = project(&[planned("1", "110", DeliveryPath::Ecoverse, Some("a1"))]);
        point_at_servers(&core, &mut p).await;
        p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });
        let report = core.migration_launch_wave(&mut p, 1, &package(), Some("script-1")).await.unwrap();
        assert_eq!((report.launched_ecoverse, report.launched_rest, report.failed.len()), (0, 0, 1));
        assert!(report.failed[0].1.contains("Check Activity before launching these again"), "{:?}", report.failed);
        assert!(!fake.seen().iter().any(|r| r.url.ends_with("/direct")), "and it didn't fall back to launching a second way");
        assert_eq!(p.devices[0].stage, Stage::LaunchFailed);
        assert!(p.devices[0].launch_unsure, "a lost answer leaves it unsure, not simply failed");
    }

    #[tokio::test]
    async fn a_server_error_or_an_answer_without_a_task_leaves_the_launch_unsure_but_a_refusal_does_not() {
        for (answer, unsure) in [(500u16, true), (201, true), (400, false)] {
            let fake = lab_with(&[("110", Some("abcd-1234-efgh"))], move |_| status(answer, if answer == 201 { r#"{"data":{}}"# } else { r#"{"message":"nope"}"# }));
            let core = core_with(&fake).await;
            let mut p = project(&[planned("1", "110", DeliveryPath::Rest, None)]);
            point_at_servers(&core, &mut p).await;
            p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });
            let report = core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap();
            assert_eq!((p.devices[0].stage, p.devices[0].launch_unsure), (Stage::LaunchFailed, unsure), "HTTP {answer}: {:?}", report.failed);
            assert_eq!(report.failed[0].1.contains("may have gone through"), unsure);
        }
    }

    #[tokio::test]
    async fn an_unsure_device_is_not_launched_again_until_the_operator_says_they_checked() {
        use crate::migration::workflow::DeviceOp;
        let fake = lab_with(&[("110", Some("abcd-1234-efgh"))], |_| status(500, "{}"));
        let core = core_with(&fake).await;
        let mut p = project(&[planned("1", "110", DeliveryPath::Rest, None)]);
        point_at_servers(&core, &mut p).await;
        p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });
        core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap();
        let sent = || fake.seen().iter().filter(|r| r.url.ends_with("/direct")).count();
        assert_eq!(sent(), 1);

        let refused = core.migration_device_op(&mut p, 0, DeviceOp::Retry { other_path: false, checked: false }).await.unwrap_err();
        assert!(refused.contains("may have gone through"), "{refused}");
        assert_eq!(p.devices[0].stage, Stage::LaunchFailed);

        core.migration_device_op(&mut p, 0, DeviceOp::Retry { other_path: false, checked: true }).await.unwrap();
        assert!(!p.devices[0].launch_unsure && p.devices[0].stage == Stage::Planned);
        assert!(p.devices[0].events.iter().any(|e| e.detail.contains("checked that the earlier launch did not run")));
        assert_eq!(sent(), 1, "retrying only puts it back in the wave");
    }

    #[tokio::test]
    async fn what_is_sent_is_saved_first_so_a_crash_can_never_leave_a_launched_device_looking_untouched() {
        let fake = lab(&[("110", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        let mut p = project(&[planned("1", "110", DeliveryPath::Rest, None)]);
        point_at_servers(&core, &mut p).await;
        p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });
        core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap();
        assert_eq!(p.devices[0].stage, Stage::Launched);

        // What is on disk is what the last checkpoint saw: the app stopped before the final save.
        let mut stored = core.migration_load(&p.id).await.unwrap().unwrap();
        assert_eq!((stored.devices[0].stage, stored.devices[0].launch_unsure), (Stage::Planned, true));
        let report = core.migration_launch_wave(&mut stored, 1, &package(), None).await.unwrap();
        assert_eq!(report.launched_rest, 0, "so a relaunch doesn't send it a second time");
        assert!(report.failed[0].1.contains("never recorded"), "{:?}", report.failed);
        assert_eq!(fake.seen().iter().filter(|r| r.url.ends_with("/direct")).count(), 1);
    }

    #[tokio::test]
    async fn a_launch_from_an_out_of_date_copy_sends_nothing() {
        let fake = lab(&[("110", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        let mut p = project(&[planned("1", "110", DeliveryPath::Rest, None), planned("2", "110", DeliveryPath::Rest, None)]);
        point_at_servers(&core, &mut p).await;
        p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });
        core.migration_save(&mut p).await.unwrap();
        let mut other_window = core.migration_load(&p.id).await.unwrap().unwrap();
        core.migration_save(&mut other_window).await.unwrap();

        let report = core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap();
        assert!(report.stopped.as_deref().is_some_and(|s| s.contains("changed in another N-voke window")), "{:?}", report.stopped);
        assert!(!fake.seen().iter().any(|r| r.url.ends_with("/direct")));
        assert!(p.devices.iter().all(|d| d.stage == Stage::Planned && !d.launch_unsure));
    }

    #[tokio::test]
    async fn a_stale_uploaded_or_published_script_is_not_launched() {
        let fake = lab(&[("110", Some("abcd-1234-efgh"))]);
        let core = core_with(&fake).await;
        let mut p = project(&[planned("1", "110", DeliveryPath::Rest, None)]);
        point_at_servers(&core, &mut p).await;
        p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.0.9+old".into() });
        let report = core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap();
        assert!(report.failed[0].1.contains("0.0.9+old") && report.failed[0].1.contains("upload the current one"), "{:?}", report.failed);
        assert!(!fake.seen().iter().any(|r| r.url.ends_with("/direct")));

        let mut q = project(&[planned("1", "110", DeliveryPath::Ecoverse, Some("a1"))]);
        point_at_servers(&core, &mut q).await;
        q.ecoverse_script_version = Some("0.0.9+old".into());
        assert!(core.migration_launch_wave(&mut q, 1, &package(), Some("script-1")).await.unwrap_err().contains("Publish it again"));
        q.ecoverse_script_version = None;
        assert!(core.migration_launch_wave(&mut q, 1, &package(), Some("script-1")).await.is_err(), "a project that never recorded a version publishes again too");
    }

    #[tokio::test]
    async fn a_token_in_what_a_server_says_back_is_blanked_before_it_is_stored() {
        let fake = lab_with(&[("110", Some("abcd-1234-efgh"))], |_| status(400, r#"{"message":"Bad CommandLine: -RegistrationToken abcd-1234-efgh -Foo"}"#));
        let core = core_with(&fake).await;
        let mut p = project(&[planned("1", "110", DeliveryPath::Rest, None)]);
        point_at_servers(&core, &mut p).await;
        p.rest_scripts.insert(p.source_server_id.clone(), RestScript { item_id: "77".into(), version: "0.1.0+3f9a2c1".into() });
        core.migration_launch_wave(&mut p, 1, &package(), None).await.unwrap();
        let everything = serde_json::to_string(&p).unwrap();
        assert!(!everything.contains("abcd-1234-efgh"), "{everything}");
        assert!(p.devices[0].last_error.as_deref().is_some_and(|e| e.contains("[redacted]")));
    }
}
