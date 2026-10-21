//! One polling pass over a project: read what the APIs say about each device that is still
//! moving, hand it to the tracker's `advance`, and record the stage changes. It asks for
//! things by customer where it can and per device only for the few devices in flight, and a
//! failed read never changes a device's stage: it is reported and tried again next time.
//! Because the tracker decides from current state, a pass after the app was closed for hours
//! catches up in one go.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use super::{
    ecoverse::{EcoDevice, EcoSeen, dest_asset_states},
    endpoint_result::{launch_outcome_from_ecoverse, launch_outcome_from_rest},
    planner::DeliveryPath,
    project::{DestEcoverse, EcoState, MatchCandidate, Project, ProjectDevice, Stage},
    tracker::{Attention, Candidate, Context, DestObservation, LaunchOutcome, MatchResult, Observation, Thresholds, advance, attention, match_destination},
};
use crate::{
    Core,
    ncentral::model::{Device, DeviceIdentity},
};

/// At most this many tasks per Ecoverse request: the API allows 5 aliases and 130 fields, and a full task is about 30 fields.
const TASKS_PER_REQUEST: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollReport {
    pub checked: usize,
    /// (device name, from, to).
    pub moved: Vec<(String, Stage, Stage)>,
    pub attention: Vec<(String, Attention)>,
    /// Devices whose destination match needs the operator to choose, with the candidates' device IDs.
    pub ambiguous: Vec<(String, Vec<String>)>,
    /// Reads that failed; the devices involved were left as they were.
    pub errors: Vec<String>,
}

fn pollable(stage: Stage) -> bool {
    matches!(stage, Stage::Launched | Stage::NotPickedUp | Stage::Staged | Stage::StuckStaged | Stage::AgentRemoved | Stage::Lost | Stage::Registered | Stage::Healthy)
}

/// How long a healthy device that Ecoverse is receiving data from is left alone before it is looked at again.
const SETTLED_RECHECK: Duration = Duration::minutes(15);

/// Whether the pass has to read anything for this device. A healthy device with no Ecoverse to wait for has nothing left
/// to find out (its only change is the review period ending, which needs the clock and not a server); one that Ecoverse
/// is receiving data from is looked at again every quarter of an hour, in case that stops.
fn needs_reading(device: &ProjectDevice, now: DateTime<Utc>) -> bool {
    if device.stage != Stage::Healthy {
        return true;
    }
    match device.ecoverse {
        Some(EcoState::NotLinked) => false,
        Some(EcoState::Reporting) => device.ecoverse_read_at.as_deref().and_then(|at| DateTime::parse_from_rfc3339(at).ok()).is_none_or(|at| now - at.with_timezone(&Utc) >= SETTLED_RECHECK),
        _ => true,
    }
}

fn age(now: DateTime<Utc>, at: Option<DateTime<Utc>>) -> Option<Duration> {
    at.map(|t| (now - t).max(Duration::zero()))
}

/// What the pass found out about one device, besides the observation itself.
#[derive(Default)]
struct Found {
    dest_device_id: Option<String>,
    ambiguous: Vec<(String, String)>,
}

#[derive(Default)]
struct Shared {
    eco_source: Option<Result<HashMap<String, EcoDevice>, String>>,
    eco_dest: Option<Result<HashMap<String, EcoSeen>, String>>,
    dest_units: HashMap<String, Result<Vec<Device>, String>>,
    /// Destination devices a source device has been matched to in this pass, so two can't claim the same one.
    claimed: HashSet<String>,
}

impl Core {
    pub async fn migration_poll(&self, project: &mut Project, thresholds: &Thresholds) -> PollReport {
        self.migration_poll_at(project, thresholds, Utc::now()).await
    }

    pub async fn migration_poll_at(&self, project: &mut Project, thresholds: &Thresholds, now: DateTime<Utc>) -> PollReport {
        let mut report = PollReport::default();
        let work: Vec<usize> = project.devices.iter().enumerate().filter(|(_, d)| pollable(d.stage)).map(|(i, _)| i).collect();
        report.checked = work.len();
        if work.is_empty() {
            return report;
        }
        let (read, settled): (Vec<usize>, Vec<usize>) = work.iter().partition(|&&i| needs_reading(&project.devices[i], now));
        let mut observed = self.observe(project, &read, now, &mut report).await;
        observed.extend(settled.into_iter().map(|i| (i, Observation::default(), Found::default())));

        let ctx = Context {
            now,
            thresholds,
            reinstall: project.is_reinstall(),
            destination_in_ecoverse: project.dest_ecoverse != DestEcoverse::NotLinked,
            review_days: project.options.review_days,
            retire_source_records: project.options.retire_source_records,
        };
        for (index, observation, found) in observed {
            let device = &mut project.devices[index];
            if let Some(id) = found.dest_device_id {
                device.dest_device_id = Some(id);
            }
            // What the operator has to choose between is kept on the device; it's cleared once there's a match or no question.
            device.match_candidates = found.ambiguous.iter().map(|(device_id, name)| MatchCandidate { device_id: device_id.clone(), name: name.clone() }).collect();
            if !found.ambiguous.is_empty() {
                report.ambiguous.push((device.name.clone(), found.ambiguous.iter().map(|(id, _)| id.clone()).collect()));
            }
            if let Some(state) = observation.dest_ecoverse {
                device.ecoverse = Some(state);
                device.ecoverse_read_at = Some(super::project::stamp(now));
            }
            let from = device.stage;
            for step in advance(device, &observation, &ctx) {
                let before = device.stage;
                if device.move_to(step.to, now, &step.why).is_ok() {
                    report.moved.push((device.name.clone(), before, step.to));
                }
            }
            if device.stage == Stage::LaunchFailed && from != Stage::LaunchFailed {
                device.last_error = device.events.last().map(|e| e.detail.clone());
            }
            for flag in attention(device, &observation, &ctx) {
                report.attention.push((device.name.clone(), flag));
            }
        }
        project.updated_at = super::project::stamp(now);
        report
    }

    /// Reads everything for the devices in `work`. Takes the project by shared reference: nothing is changed until the reads are done.
    async fn observe(&self, project: &Project, work: &[usize], now: DateTime<Utc>, report: &mut PollReport) -> Vec<(usize, Observation, Found)> {
        let mut shared = Shared::default();
        let dest_host = self.ncentral_server(&project.dest_server_id).map(|s| s.host).unwrap_or_default();
        let linked = project.dest_ecoverse != DestEcoverse::NotLinked;

        // Launch tasks first: Ecoverse ones in batches, REST ones one at a time.
        let mut outcomes: HashMap<usize, LaunchOutcome> = HashMap::new();
        let eco_tasks: Vec<(usize, String)> = work
            .iter()
            .filter(|&&i| matches!(project.devices[i].stage, Stage::Launched | Stage::NotPickedUp) && project.devices[i].path == Some(DeliveryPath::Ecoverse))
            .filter_map(|&i| Some((i, project.devices[i].task_id.clone()?)))
            .collect();
        for chunk in eco_tasks.chunks(TASKS_PER_REQUEST) {
            let ids: Vec<String> = chunk.iter().map(|(_, id)| id.clone()).collect();
            match self.with_ecoverse(&project.source_server_id, self.get_tasks(&ids)).await {
                Ok(fetch) => {
                    for (i, id) in chunk {
                        if let Some(Some(snapshot)) = fetch.tasks.get(id) {
                            outcomes.insert(*i, launch_outcome_from_ecoverse(snapshot));
                        }
                    }
                }
                Err(e) => report.errors.push(format!("Could not read launch tasks from Ecoverse: {e}")),
            }
        }
        for &i in work {
            let device = &project.devices[i];
            if !matches!(device.stage, Stage::Launched | Stage::NotPickedUp) || device.path != Some(DeliveryPath::Rest) {
                continue;
            }
            let Some(task_id) = &device.task_id else { continue };
            match self.ncentral_task_details(&project.source_server_id, task_id).await {
                // Nothing yet: the task hasn't reported, or its history was deleted with the script item (the two look alike).
                Ok(details) if details.is_empty() => {
                    outcomes.insert(i, LaunchOutcome::NoExecution);
                }
                Ok(details) => {
                    let detail = &details[0];
                    let finished = matches!(detail.status.to_ascii_lowercase().as_str(), "success" | "failed");
                    let output = match (&detail.output_file_name, finished) {
                        (Some(file), true) => self.ncentral_task_output(&project.source_server_id, file).await.ok(),
                        _ => None,
                    };
                    outcomes.insert(i, launch_outcome_from_rest(&detail.status, detail.message.as_deref().unwrap_or(""), output.as_ref()));
                }
                Err(e) => report.errors.push(format!("{}: could not read the launch task: {e}", device.name)),
            }
        }

        let mut result = Vec::with_capacity(work.len());
        for &i in work {
            let device = &project.devices[i];
            let mut obs = Observation { launch: outcomes.remove(&i), ..Default::default() };
            let mut found = Found::default();

            // The source's Ecoverse connection, for devices launched through it.
            let wants_source_eco = device.path == Some(DeliveryPath::Ecoverse) && matches!(device.stage, Stage::Launched | Stage::NotPickedUp | Stage::Staged | Stage::StuckStaged) && !project.source_ecoverse_org_ids.is_empty();
            if wants_source_eco {
                if shared.eco_source.is_none() {
                    let fetched = self.with_ecoverse(&project.source_server_id, self.eco_devices(&project.source_ecoverse_org_ids)).await.map(|d| d.into_iter().map(|a| (a.id.clone(), a)).collect());
                    shared.eco_source = Some(fetched);
                }
                match shared.eco_source.as_ref() {
                    Some(Ok(map)) => {
                        if let Some(asset) = device.asset_id.as_ref().and_then(|a| map.get(a)) {
                            obs.source_connected = Some(asset.connected());
                            obs.source_data_pending = asset.connected() && !asset.has_data();
                            obs.source_disconnected = matches!(device.stage, Stage::Staged | Stage::StuckStaged) && !asset.connected();
                        }
                    }
                    Some(Err(e)) => report.errors.push(format!("{}: could not read Ecoverse: {e}", device.name)),
                    None => {}
                }
            }

            // The source's check-in, where it is the signal: the REST path, and a reinstall.
            let wants_checkin = matches!(device.stage, Stage::Staged | Stage::StuckStaged) && (device.path == Some(DeliveryPath::Rest) || project.is_reinstall());
            if wants_checkin {
                match self.ncentral_device(&project.source_server_id, &device.source_device_id).await {
                    Ok(source) => obs.source_checkin_age = age(now, source.last_checkin()),
                    Err(e) => report.errors.push(format!("{}: could not read its check-in: {e}", device.name)),
                }
            }

            // Where it is now.
            let looks_for_dest = match device.stage {
                Stage::Staged | Stage::StuckStaged => !project.is_reinstall(),
                Stage::AgentRemoved | Stage::Lost | Stage::Registered => true,
                _ => false,
            };
            if looks_for_dest {
                match self.find_on_destination(project, device, &mut shared, now).await {
                    Ok((dest, picked)) => {
                        obs.dest = dest;
                        found = picked;
                    }
                    Err(e) => report.errors.push(format!("{}: could not look for it on the destination: {e}", device.name)),
                }
            }
            if !linked && matches!(device.stage, Stage::Registered | Stage::Healthy) {
                obs.dest_ecoverse = Some(EcoState::NotLinked);
            }
            if linked && matches!(device.stage, Stage::Registered | Stage::Healthy) {
                let dest_id = found.dest_device_id.clone().or_else(|| device.dest_device_id.clone());
                if project.dest_ecoverse != DestEcoverse::NotLinked {
                    if shared.eco_dest.is_none() {
                        let fetched = self.with_ecoverse(project.dest_ecoverse_server(), self.eco_devices(&project.dest_ecoverse_org_ids)).await.map(|d| dest_asset_states(&d, &dest_host));
                        shared.eco_dest = Some(fetched);
                    }
                    let seen = match (shared.eco_dest.as_ref(), dest_id) {
                        (Some(Ok(map)), Some(id)) => Ok(map.get(&id).copied()),
                        (Some(Ok(_)), None) => Ok(None),
                        (Some(Err(e)), _) => {
                            report.errors.push(format!("{}: could not read the destination's Ecoverse: {e}", device.name));
                            Err(())
                        }
                        (None, _) => Err(()),
                    };
                    obs.dest_ecoverse_connected = seen.as_ref().ok().map(|s| s.is_some_and(|s| s.connected));
                    obs.dest_ecoverse = seen.ok().map(|s| match s {
                        None => EcoState::NotSeen,
                        Some(s) if s.has_data => EcoState::Reporting,
                        Some(_) => EcoState::WaitingForData,
                    });
                } else {
                    obs.dest_ecoverse_connected = Some(false);
                }
            }
            result.push((i, obs, found));
        }
        result
    }

    /// The device on the destination: the same record for a reinstall, otherwise the match among what's under its customer or site.
    async fn find_on_destination(&self, project: &Project, device: &ProjectDevice, shared: &mut Shared, now: DateTime<Utc>) -> Result<(Option<DestObservation>, Found), String> {
        let observation = |d: &Device| DestObservation { device_id: d.device_id.clone(), customer_id: d.customer_id.clone(), site_id: d.site_id.clone(), checkin_age: age(now, d.last_checkin()) };

        if project.is_reinstall() {
            let same = self.ncentral_device(&project.dest_server_id, &device.source_device_id).await.map_err(|e| e.message)?;
            return Ok((Some(observation(&same)), Found { dest_device_id: Some(same.device_id), ambiguous: vec![] }));
        }
        if let Some(known) = &device.dest_device_id {
            let found = self.ncentral_device(&project.dest_server_id, known).await.map_err(|e| e.message)?;
            return Ok((Some(observation(&found)), Found { dest_device_id: Some(found.device_id), ambiguous: vec![] }));
        }
        let Some(unit) = device.dest_org_unit_id.clone() else { return Ok((None, Found::default())) };
        if !shared.dest_units.contains_key(&unit) {
            let listed = self.ncentral_org_unit_devices(&project.dest_server_id, &unit).await.map_err(|e| e.message);
            shared.dest_units.insert(unit.clone(), listed);
        }
        let listed = shared.dest_units[&unit].clone()?;

        let old = device.identity.clone().unwrap_or_else(|| DeviceIdentity { hostname: Some(device.name.clone()), ..Default::default() });
        let old_host = old.hostname.clone().unwrap_or_else(|| device.name.clone());
        let baseline: HashSet<&String> = project.dest_baseline.get(&unit).map(|v| v.iter().collect()).unwrap_or_default();
        // Without a baseline nothing can be called new, so a host name alone is never enough.
        let has_baseline = project.dest_baseline.contains_key(&unit);
        // A destination device another source device already has is not offered again.
        let taken: HashSet<&str> = project.devices.iter().filter(|o| o.source_device_id != device.source_device_id).filter_map(|o| o.dest_device_id.as_deref()).chain(shared.claimed.iter().map(String::as_str)).collect();
        // Only devices that could be it are read in full: new ones, and any with the same name.
        let mut candidates = vec![];
        for listed_device in &listed {
            if taken.contains(listed_device.device_id.as_str()) {
                continue;
            }
            let is_new = has_baseline && !baseline.contains(&listed_device.device_id);
            let same_name = listed_device.name().eq_ignore_ascii_case(&old_host);
            if !is_new && !same_name {
                continue;
            }
            let mut identity = self.device_identity_cached(&project.dest_server_id, &listed_device.device_id).await.unwrap_or_default();
            identity.hostname.get_or_insert_with(|| listed_device.name().to_string());
            candidates.push(Candidate { device_id: listed_device.device_id.clone(), identity, new_since_launch: is_new });
        }
        match match_destination(&old, &candidates) {
            MatchResult::Found { device_id, .. } => {
                let detail = self.ncentral_device(&project.dest_server_id, &device_id).await.map_err(|e| e.message)?;
                shared.claimed.insert(device_id.clone());
                Ok((Some(observation(&detail)), Found { dest_device_id: Some(device_id), ambiguous: vec![] }))
            }
            MatchResult::Ambiguous(ids) => {
                let named = ids.into_iter().map(|id| (listed.iter().find(|d| d.device_id == id).map_or_else(|| id.clone(), |d| d.name().to_string()), id)).map(|(name, id)| (id, name)).collect();
                Ok((None, Found { dest_device_id: None, ambiguous: named }))
            }
            MatchResult::None => Ok((None, Found::default())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};

    use super::*;
    use crate::{
        migration::{
            endpoint_result::tests::zip_of,
            planner::PlannedDevice,
            project::{DestEcoverse, RestScript},
        },
        ncentral::rest::{
            Method, RestRequest, RestResponse,
            testing::{Fake, auth_ok, ok, status},
        },
    };

    fn now() -> DateTime<Utc> {
        "2026-10-20T12:00:00Z".parse().unwrap()
    }

    fn ago(minutes: i64) -> String {
        (now() - Duration::minutes(minutes)).to_rfc3339()
    }

    /// What the two N-central servers currently say. Tests change it between polls.
    #[derive(Default)]
    struct World {
        task: Option<(String, Option<Vec<u8>>)>,
        task_error: bool,
        /// Check-in age in minutes by "<host>/<device id>".
        checkin: HashMap<String, i64>,
        /// Devices listed under the destination unit.
        dest_list: Vec<Value>,
        /// Identity (serial, host name, mac) by destination device id.
        identities: HashMap<String, (String, String, String)>,
        /// Source device's customer and site, and the destination device's.
        placement: HashMap<String, (String, String)>,
    }

    const STAGED_OUTPUT: &str = r#"NVOKE-RESULT {"action":"Launch","result":"staged","errorCode":0,"taskCreated":true,"message":"Staged"}"#;

    fn rest_fake(world: &Arc<Mutex<World>>) -> Arc<Fake> {
        let world = world.clone();
        Fake::new(move |req: &RestRequest, _| {
            let w = world.lock().unwrap();
            let url = req.url.trim_start_matches("https://");
            let (host, path) = url.split_once('/').unwrap();
            let path = path.split('?').next().unwrap();
            let device = |host: &str, id: &str| {
                let (customer, site) = w.placement.get(&format!("{host}/{id}")).cloned().unwrap_or(("110".into(), "111".into()));
                let mut d = json!({ "deviceId": id, "longName": format!("PC-{id}"), "osId": "winnt", "customerId": customer, "siteId": site });
                if let Some(m) = w.checkin.get(&format!("{host}/{id}")) {
                    d["lastApplianceCheckinTime"] = json!(ago(*m));
                }
                d
            };
            match (req.method, path) {
                (_, "api/auth/authenticate") => auth_ok(3600),
                (Method::Get, p) if p.starts_with("api/scheduled-tasks/") && p.ends_with("/status/details") => {
                    if w.task_error {
                        return status(500, r#"{"message":"boom"}"#);
                    }
                    match &w.task {
                        None => ok(json!({ "data": [], "totalItems": 0 })),
                        Some((state, zip)) => ok(json!({ "data": [{ "status": state, "message": "", "taskId": 8, "outputFileName": zip.as_ref().map(|_| "/download/scheduled_tasks/output_files/a/b/c.zip") }] })),
                    }
                }
                (Method::Get, p) if p.starts_with("download/scheduled_tasks/") => RestResponse { status: 200, body: w.task.as_ref().and_then(|t| t.1.clone()).unwrap_or_default(), retry_after: None },
                (Method::Get, p) if p.ends_with("/assets") => {
                    let id = p.split('/').nth(2).unwrap();
                    match w.identities.get(id) {
                        Some((serial, host_name, mac)) => ok(json!({ "data": { "computersystem": { "serialnumber": serial, "netbiosname": host_name }, "networkadapter": { "list": [{ "macaddress": mac }] } } })),
                        None => status(404, ""),
                    }
                }
                (Method::Get, p) if p.starts_with("api/org-units/") && p.ends_with("/devices") => ok(json!({ "data": w.dest_list, "totalPages": 1 })),
                (Method::Get, p) if p.starts_with("api/devices/") => {
                    let id = p.trim_start_matches("api/devices/");
                    ok(json!({ "data": device(host, id) }))
                }
                other => panic!("unexpected {other:?} on {host}"),
            }
        })
    }

    fn planned(id: &str, path: DeliveryPath, asset: Option<&str>) -> PlannedDevice {
        PlannedDevice {
            device_id: id.into(),
            name: format!("PC-{id}"),
            source_customer_id: Some("10".into()),
            source_site_id: Some("11".into()),
            dest_org_unit_id: Some("111".into()),
            path: Some(path),
            asset_id: asset.map(String::from),
            blockers: vec![],
            warnings: vec![],
            excluded: false,
        }
    }

    async fn setup(devices: &[PlannedDevice], reinstall: bool) -> (Arc<Core>, Project) {
        let core = Core::for_tests("poll");
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = if reinstall { src.clone() } else { core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap() };
        let mut p = Project::new("mig-1".into(), "Acme", &src.id, &dst.id, DestEcoverse::NotLinked, now() - Duration::hours(2));
        p.add_planned(devices, now() - Duration::hours(2));
        let ids: Vec<String> = devices.iter().map(|d| d.device_id.clone()).collect();
        p.create_wave("Pilot", &ids, now()).unwrap();
        p.rest_scripts.insert(src.id.clone(), RestScript { item_id: "77".into(), version: "v".into() });
        (core, p)
    }

    fn launch(p: &mut Project, index: usize, minutes_ago: i64, task: &str) {
        let d = &mut p.devices[index];
        d.stage = Stage::Launched;
        d.task_id = Some(task.into());
        d.entered_stage_at = super::super::project::stamp(now() - Duration::minutes(minutes_ago));
    }

    fn stages(p: &Project) -> Vec<Stage> {
        p.devices.iter().map(|d| d.stage).collect()
    }

    fn eco_task(id: &str, outputs: &[(&str, &str)], status_word: &str) -> Value {
        json!({ "id": id, "status": "COMPLETED", "createdAt": "2026-10-20T10:00:00Z", "executions": { "nodes": [{
            "id": "e", "status": status_word, "exitCode": null, "errorMessage": null,
            "output": { "runAs": "SYSTEM", "stdOutput": "", "stdError": "", "outputs": outputs.iter().map(|(v, s)| json!({ "name": v, "variable": v, "type": "STRING", "value": { "stringValue": s } })).collect::<Vec<_>>() }
        }] } })
    }

    #[tokio::test]
    async fn a_rest_device_moves_through_the_whole_flow_across_polls() {
        let world = Arc::new(Mutex::new(World::default()));
        let fake = rest_fake(&world);
        let (core, mut p) = setup(&[planned("9001", DeliveryPath::Rest, None)], false).await;
        core.set_rest_transport(fake.clone());
        p.devices[0].identity = Some(DeviceIdentity { serial: Some("VMW-1".into()), hostname: Some("PC-9001".into()), macs: vec!["aabbccddee01".into()], smbios_uuid: None });
        p.dest_baseline.insert("111".into(), vec!["4001".into()]);
        launch(&mut p, 0, 1, "5550");
        let t = Thresholds::default();

        // The device is off: the task waits.
        world.lock().unwrap().task = Some(("Pending".into(), None));
        let report = core.migration_poll_at(&mut p, &t, now()).await;
        assert_eq!((report.checked, report.moved.len(), report.errors.len()), (1, 0, 0));
        assert_eq!(stages(&p), [Stage::Launched]);

        // The launcher ran, and N-central kept its output.
        let zip = zip_of("Output-Formatted.txt", format!("Exit Code: 0\r\n{STAGED_OUTPUT}\r\n").as_bytes(), true);
        world.lock().unwrap().task = Some(("Success".into(), Some(zip)));
        world.lock().unwrap().checkin.insert("src.example.com/9001".into(), 2);
        let report = core.migration_poll_at(&mut p, &t, now()).await;
        assert_eq!(report.moved, [("PC-9001".to_string(), Stage::Launched, Stage::Staged)]);
        assert_eq!(stages(&p), [Stage::Staged]);

        // The old agent is still checking in: nothing more.
        assert!(core.migration_poll_at(&mut p, &t, now()).await.moved.is_empty());

        // Later: the old agent has gone quiet, and a new device with the same serial and MAC has appeared and checks in.
        {
            let mut w = world.lock().unwrap();
            w.checkin.insert("src.example.com/9001".into(), 40);
            w.dest_list = vec![json!({ "deviceId": 4001, "longName": "ALREADY-THERE" }), json!({ "deviceId": 5001, "longName": "PC-9001" })];
            w.identities.insert("5001".into(), ("vmw-1".into(), "PC-9001".into(), "AA-BB-CC-DD-EE-01".into()));
            w.checkin.insert("dst.example.com/5001".into(), 3);
        }
        let report = core.migration_poll_at(&mut p, &t, now()).await;
        let path: Vec<Stage> = report.moved.iter().map(|m| m.2).collect();
        assert_eq!(path, [Stage::AgentRemoved, Stage::Registered, Stage::Healthy]);
        assert_eq!(p.devices[0].dest_device_id.as_deref(), Some("5001"));
        assert_eq!(p.devices[0].stage, Stage::Healthy);
        assert!(p.devices[0].events.iter().any(|e| e.detail.contains("found on the destination as device 5001")));
        assert!(p.devices[0].last_error.is_none());
        let after_review = core.migration_poll_at(&mut p, &t, now() + Duration::days(8)).await;
        assert_eq!(after_review.moved.last().map(|m| m.2), Some(Stage::CleanedUp));
    }

    #[tokio::test]
    async fn a_failed_launch_is_recorded_with_the_scripts_message() {
        let world = Arc::new(Mutex::new(World::default()));
        let (core, mut p) = setup(&[planned("9001", DeliveryPath::Rest, None)], false).await;
        core.set_rest_transport(rest_fake(&world));
        launch(&mut p, 0, 1, "5550");
        let zip = zip_of("Output-Formatted.txt", "Exit Code: 1004\r\nNVOKE-RESULT {\"result\":\"failed\",\"errorCode\":1004,\"message\":\"RegistrationToken is missing or malformed\"}\r\n".as_bytes(), false);
        world.lock().unwrap().task = Some(("Failed".into(), Some(zip)));
        let report = core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        assert_eq!(stages(&p), [Stage::LaunchFailed]);
        assert!(p.devices[0].last_error.as_deref().unwrap().contains("RegistrationToken is missing"), "{:?}", p.devices[0].last_error);
        assert_eq!(report.moved.len(), 1);
    }

    #[tokio::test]
    async fn a_read_that_fails_leaves_the_device_where_it_was_and_says_so() {
        let world = Arc::new(Mutex::new(World::default()));
        world.lock().unwrap().task_error = true;
        let (core, mut p) = setup(&[planned("9001", DeliveryPath::Rest, None)], false).await;
        core.set_rest_transport(rest_fake(&world));
        launch(&mut p, 0, 500, "5550");
        let report = core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        assert_eq!(stages(&p), [Stage::Launched], "an API error is never a launch failure");
        assert!(report.errors[0].contains("PC-9001") && report.errors[0].contains("boom"), "{:?}", report.errors);
    }

    #[tokio::test]
    async fn nothing_is_read_when_no_device_is_moving() {
        let world = Arc::new(Mutex::new(World::default()));
        let fake = rest_fake(&world);
        let (core, mut p) = setup(&[planned("9001", DeliveryPath::Rest, None)], false).await;
        core.set_rest_transport(fake.clone());
        let report = core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        assert_eq!((report.checked, fake.seen().len()), (0, 0), "a planned device isn't polled");
    }

    #[tokio::test]
    async fn ecoverse_launches_are_polled_four_to_a_request_and_a_dropped_asset_means_the_agent_is_gone() {
        let (core, mut p) = setup(&(1..=5).map(|i| planned(&i.to_string(), DeliveryPath::Ecoverse, Some(&format!("a{i}")))).collect::<Vec<_>>(), false).await;
        p.source_ecoverse_org_ids = vec!["org-1".into()];
        for i in 0..5 {
            launch(&mut p, i, 2, &format!("task-{}", i + 1));
        }
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = requests.clone();
        core.set_graphql_fake(move |query, variables| {
            log.lock().unwrap().push(if query.contains("GetTasks(") { "GetTasks".to_string() } else { "other".to_string() });
            if query.contains("GetTasks") {
                let mut data = serde_json::Map::new();
                for (key, id) in variables.as_object().unwrap() {
                    let n = key.trim_start_matches("id");
                    let id = id.as_str().unwrap();
                    // Task 5 has not started; the rest staged.
                    let task = if id == "task-5" { json!({ "id": id, "status": "IN_PROGRESS", "executions": { "nodes": [] } }) } else { eco_task(id, &[("Result", "staged"), ("TaskCreated", "true")], "SUCCEEDED") };
                    data.insert(format!("t{n}"), task);
                }
                return (200, json!({ "data": data }).to_string());
            }
            let nodes: Vec<Value> = (1..=5).map(|i| json!({ "id": format!("a{i}"), "name": format!("PC-{i}"), "agentConnection": { "status": if i <= 2 { "DISCONNECTED" } else { "CONNECTED" } } })).collect();
            (200, json!({ "data": { "page": { "totalCount": 5, "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": nodes } } }).to_string())
        });
        let report = core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        assert_eq!(requests.lock().unwrap().iter().filter(|q| *q == "GetTasks").count(), 2, "five tasks, four per request");
        assert_eq!(requests.lock().unwrap().iter().filter(|q| *q == "other").count(), 1, "Ecoverse's device list is read once for the whole pass, not once per device");
        assert_eq!(stages(&p), [Stage::Staged, Stage::Staged, Stage::Staged, Stage::Staged, Stage::Launched]);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        // Next pass: two assets dropped, so those agents are gone.
        let report = core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        assert_eq!(stages(&p), [Stage::AgentRemoved, Stage::AgentRemoved, Stage::Staged, Stage::Staged, Stage::Launched]);
        assert_eq!(report.moved.len(), 2);
    }

    #[tokio::test]
    async fn a_queued_launch_is_not_flagged_but_a_connected_device_with_no_run_is() {
        let (core, mut p) = setup(&[planned("1", DeliveryPath::Ecoverse, Some("a1")), planned("2", DeliveryPath::Ecoverse, Some("a2"))], false).await;
        p.source_ecoverse_org_ids = vec!["org-1".into()];
        launch(&mut p, 0, 120, "task-1");
        launch(&mut p, 1, 120, "task-2");
        core.set_graphql_fake(|query, variables| {
            if query.contains("GetTasks") {
                let data: serde_json::Map<String, Value> = variables.as_object().unwrap().iter().map(|(k, id)| (format!("t{}", k.trim_start_matches("id")), json!({ "id": id, "status": "IN_PROGRESS", "executions": { "nodes": [] } }))).collect();
                return (200, json!({ "data": data }).to_string());
            }
            let nodes = vec![json!({ "id": "a1", "agentConnection": { "status": "DISCONNECTED" } }), json!({ "id": "a2", "agentConnection": { "status": "CONNECTED" } })];
            (200, json!({ "data": { "page": { "totalCount": 2, "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": nodes } } }).to_string())
        });
        core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        assert_eq!(stages(&p), [Stage::Launched, Stage::NotPickedUp]);
    }

    #[tokio::test]
    async fn a_reinstall_is_tracked_on_the_same_record_by_its_check_in() {
        let world = Arc::new(Mutex::new(World::default()));
        let (core, mut p) = setup(&[planned("9001", DeliveryPath::Rest, None)], true).await;
        core.set_rest_transport(rest_fake(&world));
        p.dest_ecoverse = DestEcoverse::NotLinked;
        p.devices[0].stage = Stage::Staged;
        p.devices[0].entered_stage_at = super::super::project::stamp(now() - Duration::minutes(10));
        let t = Thresholds::default();
        world.lock().unwrap().placement.insert("src.example.com/9001".into(), ("110".into(), "111".into()));

        world.lock().unwrap().checkin.insert("src.example.com/9001".into(), 2);
        assert!(core.migration_poll_at(&mut p, &t, now()).await.moved.is_empty(), "still checking in");

        // Paused: the old agent is gone. The record is the same, so the next look finds it, not yet checking in.
        world.lock().unwrap().checkin.insert("src.example.com/9001".into(), 15);
        let report = core.migration_poll_at(&mut p, &t, now()).await;
        assert_eq!(report.moved.iter().map(|m| m.2).collect::<Vec<_>>(), [Stage::AgentRemoved]);
        let report = core.migration_poll_at(&mut p, &t, now()).await;
        assert_eq!(report.moved.iter().map(|m| m.2).collect::<Vec<_>>(), [Stage::Registered], "registered, but not checking in yet");
        assert!(core.migration_poll_at(&mut p, &t, now()).await.moved.is_empty());

        // The new agent checks in on the same record.
        world.lock().unwrap().checkin.insert("src.example.com/9001".into(), 1);
        core.migration_poll_at(&mut p, &t, now()).await;
        assert_eq!(stages(&p), [Stage::Healthy]);
        assert_eq!(p.devices[0].dest_device_id.as_deref(), Some("9001"));
    }

    #[tokio::test]
    async fn a_healthy_device_is_only_fully_working_in_ecoverse_once_its_asset_has_sent_data() {
        let world = Arc::new(Mutex::new(World::default()));
        let (core, mut p) = setup(&[planned("9001", DeliveryPath::Rest, None)], true).await;
        core.set_rest_transport(rest_fake(&world));
        p.dest_ecoverse = DestEcoverse::SameTenant;
        p.devices[0].stage = Stage::Healthy;
        p.devices[0].dest_device_id = Some("9001".into());
        p.devices[0].entered_stage_at = super::super::project::stamp(now() - Duration::minutes(10));
        world.lock().unwrap().placement.insert("src.example.com/9001".into(), ("110".into(), "111".into()));
        world.lock().unwrap().checkin.insert("src.example.com/9001".into(), 1);
        let t = Thresholds::default();
        assert_eq!(p.devices[0].ecoverse, None, "not read yet");
        assert_eq!(p.ecoverse_waiting(), 1, "healthy, but Ecoverse hasn't been read, so not counted as working");

        let asset = |extra: Value| {
            let mut a = json!({ "id": "asset-1", "agentConnection": { "status": "CONNECTED" }, "ncentralDevice": { "deviceId": "9001", "server": { "id": "s", "origin": "https://src.example.com" } } });
            a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            a
        };
        let serve = |core: &Core, nodes: Vec<Value>| {
            core.set_graphql_fake(move |_, _| (200, json!({ "data": { "page": { "totalCount": nodes.len(), "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": nodes } } }).to_string()));
        };

        // Registered and connected, but no asset data: it shows in Ecoverse, and scripts are still held.
        serve(&core, vec![asset(json!({}))]);
        core.migration_poll_at(&mut p, &t, now()).await;
        assert_eq!((p.devices[0].stage, p.devices[0].ecoverse), (Stage::Healthy, Some(EcoState::WaitingForData)));
        assert_eq!(p.ecoverse_waiting(), 1);

        // Empty values are not data either.
        serve(&core, vec![asset(json!({ "systemInfo": { "hostname": " ", "serialNumber": null }, "networkInterfaces": [{ "macAddress": "" }] }))]);
        core.migration_poll_at(&mut p, &t, now()).await;
        assert_eq!(p.devices[0].ecoverse, Some(EcoState::WaitingForData));

        // The first asset data arrives.
        serve(&core, vec![asset(json!({ "systemInfo": { "hostname": "PC-9001", "serialNumber": "SN1" } }))]);
        core.migration_poll_at(&mut p, &t, now()).await;
        assert_eq!((p.devices[0].ecoverse, p.ecoverse_waiting()), (Some(EcoState::Reporting), 0));

        // A device Ecoverse is receiving data from isn't looked at again for a while, whatever Ecoverse now says.
        serve(&core, vec![]);
        core.migration_poll_at(&mut p, &t, now() + Duration::minutes(5)).await;
        assert_eq!(p.devices[0].ecoverse, Some(EcoState::Reporting), "left alone inside the recheck time");

        // No asset for it at all, once the recheck time has passed.
        core.migration_poll_at(&mut p, &t, now() + Duration::minutes(16)).await;
        assert_eq!(p.devices[0].ecoverse, Some(EcoState::NotSeen));

        // A destination that isn't linked has nothing to wait for.
        p.dest_ecoverse = DestEcoverse::NotLinked;
        core.migration_poll_at(&mut p, &t, now()).await;
        assert_eq!((p.devices[0].ecoverse, p.ecoverse_waiting()), (Some(EcoState::NotLinked), 0));
    }

    #[tokio::test]
    async fn two_equally_good_matches_are_reported_for_the_operator_not_guessed() {
        let world = Arc::new(Mutex::new(World::default()));
        let (core, mut p) = setup(&[planned("9001", DeliveryPath::Rest, None)], false).await;
        core.set_rest_transport(rest_fake(&world));
        p.devices[0].identity = Some(DeviceIdentity { serial: None, hostname: Some("PC-9001".into()), macs: vec![], smbios_uuid: None });
        p.devices[0].stage = Stage::AgentRemoved;
        p.dest_baseline.insert("111".into(), vec![]);
        {
            let mut w = world.lock().unwrap();
            w.dest_list = vec![json!({ "deviceId": 5001, "longName": "PC-9001" }), json!({ "deviceId": 5002, "longName": "pc-9001" })];
            w.identities.insert("5001".into(), ("a".into(), "PC-9001".into(), "".into()));
            w.identities.insert("5002".into(), ("b".into(), "PC-9001".into(), "".into()));
        }
        let report = core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        assert_eq!(stages(&p), [Stage::AgentRemoved]);
        assert_eq!(report.ambiguous, [("PC-9001".to_string(), vec!["5001".to_string(), "5002".to_string()])]);
        let kept: Vec<(&str, &str)> = p.devices[0].match_candidates.iter().map(|c| (c.device_id.as_str(), c.name.as_str())).collect();
        assert_eq!(kept, [("5001", "PC-9001"), ("5002", "pc-9001")], "the operator can see what to choose between");
        // Once only one fits, the question goes away.
        world.lock().unwrap().dest_list = vec![json!({ "deviceId": 5001, "longName": "PC-9001" })];
        core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        assert!(p.devices[0].match_candidates.is_empty());
        assert_eq!(p.devices[0].dest_device_id.as_deref(), Some("5001"));
    }

    #[tokio::test]
    async fn a_host_name_alone_is_never_enough_when_what_was_there_before_is_unknown() {
        let world = Arc::new(Mutex::new(World::default()));
        let (core, mut p) = setup(&[planned("9001", DeliveryPath::Rest, None)], false).await;
        core.set_rest_transport(rest_fake(&world));
        p.devices[0].identity = Some(DeviceIdentity { serial: None, hostname: Some("PC-9001".into()), macs: vec![], smbios_uuid: None });
        p.devices[0].stage = Stage::AgentRemoved;
        {
            let mut w = world.lock().unwrap();
            w.dest_list = vec![json!({ "deviceId": 5001, "longName": "PC-9001" })];
            w.identities.insert("5001".into(), ("a".into(), "PC-9001".into(), "".into()));
        }
        core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        assert!(p.devices[0].dest_device_id.is_none(), "without a baseline, a device with the same name could have been there all along");

        p.dest_baseline.insert("111".into(), vec![]);
        core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        assert_eq!(p.devices[0].dest_device_id.as_deref(), Some("5001"), "with one, a new device with the name is the match");
    }

    #[tokio::test]
    async fn a_destination_device_is_claimed_by_one_source_device_only() {
        let world = Arc::new(Mutex::new(World::default()));
        let mut a = planned("9001", DeliveryPath::Rest, None);
        a.name = "PC-SAME".into();
        let mut b = planned("9002", DeliveryPath::Rest, None);
        b.name = "PC-SAME".into();
        let (core, mut p) = setup(&[a, b], false).await;
        core.set_rest_transport(rest_fake(&world));
        for d in &mut p.devices {
            d.identity = Some(DeviceIdentity { serial: None, hostname: Some("PC-SAME".into()), macs: vec![], smbios_uuid: None });
            d.stage = Stage::AgentRemoved;
        }
        p.dest_baseline.insert("111".into(), vec![]);
        {
            let mut w = world.lock().unwrap();
            w.dest_list = vec![json!({ "deviceId": 5001, "longName": "PC-SAME" })];
            w.identities.insert("5001".into(), ("a".into(), "PC-SAME".into(), "".into()));
        }
        core.migration_poll_at(&mut p, &Thresholds::default(), now()).await;
        let claimed: Vec<Option<&str>> = p.devices.iter().map(|d| d.dest_device_id.as_deref()).collect();
        assert_eq!(claimed, [Some("5001"), None], "the second one doesn't take the same record");
    }
}
