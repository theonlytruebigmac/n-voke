//! Running `Status` and `Cleanup` on a device and reading what comes back. The script has to be
//! sent through whichever agent the device has *now*: the old one until the cleanup starts
//! (this is how a stuck device is diagnosed), nothing in between (the old agents are gone and
//! the new one hasn't reported), and the new one afterwards, through the destination's
//! Ecoverse tenant if it is linked, otherwise through its N-central REST path. A device on an
//! unlinked destination that was never uploaded to needs neither: it removes its own records
//! after the review period.

use std::time::Duration;

use chrono::Utc;
use serde_json::json;

use super::{
    ecoverse::index_by_ncentral_id,
    endpoint_result::{EndpointResult, result_from_ecoverse},
    launcher::launch_params,
    packager::{Operation, Package, SCRIPT_TIMEOUT_SECONDS},
    planner::DeliveryPath,
    project::{DestEcoverse, Project, ProjectDevice, Stage},
};
use crate::{
    Core,
    api::RunScriptInput,
    audit::AuditEntry,
    ncentral::rest::DirectTask,
};

/// How a request will reach the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// An Ecoverse asset, with the script published for that tenant.
    Ecoverse { asset_id: String, script_id: String },
    /// An N-central server's direct task, with the script's repository item and the customer ID the task is recorded under.
    Rest { server_id: String, item_id: String, device_id: String, customer_id: String },
}

#[derive(Clone, Debug)]
pub struct ActionOutcome {
    pub via: String,
    pub result: EndpointResult,
}

/// Which side of the move the device is on, from its stage alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Source,
    Destination,
}

pub fn side_for(device: &ProjectDevice, reinstall: bool) -> Result<Side, String> {
    match device.stage {
        Stage::Launched | Stage::NotPickedUp | Stage::Staged | Stage::StuckStaged => Ok(Side::Source),
        // A reinstall keeps the same record and asset, so the same route reaches it before and after.
        Stage::Registered | Stage::Healthy | Stage::CleanedUp | Stage::WrongPlace if reinstall => Ok(Side::Source),
        Stage::Registered | Stage::Healthy | Stage::CleanedUp | Stage::WrongPlace => Ok(Side::Destination),
        Stage::AgentRemoved | Stage::Lost => Err("The old agent has been removed and the new one hasn't reported yet, so nothing can reach this device. Try again once it has checked in.".into()),
        Stage::Planned | Stage::Unreachable | Stage::LaunchFailed => Err("This device hasn't been launched, so there is nothing on it to read.".into()),
        Stage::Retired | Stage::Excluded | Stage::Resolved => Err("This device is finished with.".into()),
    }
}

impl Core {
    /// Works out how to reach the device now. Reads from the servers where it has to (the destination's record and Ecoverse asset).
    pub async fn migration_route(&self, project: &Project, device: &ProjectDevice) -> Result<Route, String> {
        let side = side_for(device, project.is_reinstall())?;
        let rest_for = |server_id: &str, device_id: &str, customer_id: Option<String>| -> Result<Route, String> {
            let item = project.rest_scripts.get(server_id).ok_or_else(|| {
                let host = self.ncentral_server(server_id).map(|s| s.host).unwrap_or_default();
                format!("The endpoint script hasn't been uploaded to {host}, so N-central can't be used to reach this device.{}", if side == Side::Destination { " Its records will remove themselves after the review period." } else { "" })
            })?;
            Ok(Route::Rest { server_id: server_id.into(), item_id: item.item_id.clone(), device_id: device_id.into(), customer_id: customer_id.ok_or("The device's customer isn't known.")? })
        };
        match side {
            Side::Source => match (device.path, &device.asset_id, &project.ecoverse_script_id) {
                (Some(DeliveryPath::Ecoverse), Some(asset), Some(script)) => Ok(Route::Ecoverse { asset_id: asset.clone(), script_id: script.clone() }),
                _ => rest_for(&project.source_server_id, &device.source_device_id, device.source_customer_id.clone()),
            },
            Side::Destination => {
                let dest_id = device.dest_device_id.clone().ok_or("The device hasn't been found on the destination yet.")?;
                let dest_host = self.ncentral_server(&project.dest_server_id)?.host;
                if project.dest_ecoverse == DestEcoverse::SameTenant
                    && let (Some(script), false) = (&project.ecoverse_script_id, project.source_ecoverse_org_ids.is_empty())
                    && let Ok(assets) = self.eco_devices(&project.source_ecoverse_org_ids).await
                    && let Some(asset) = index_by_ncentral_id(&assets, &dest_host).remove(&dest_id)
                    && asset.connected
                {
                    return Ok(Route::Ecoverse { asset_id: asset.asset_id, script_id: script.clone() });
                }
                let record = self.ncentral_device(&project.dest_server_id, &dest_id).await.map_err(|e| e.message)?;
                rest_for(&project.dest_server_id, &dest_id, record.customer_id)
            }
        }
    }

    /// Runs `Status` or `Cleanup` on one device and waits (up to `wait`) for its report. Records the report
    /// on the device's timeline, and moves a healthy device to *Cleaned up* when the cleanup says it is.
    pub async fn migration_run_action(&self, project: &mut Project, index: usize, operation: Operation, package: &Package, wait: Duration) -> Result<ActionOutcome, String> {
        if !matches!(operation, Operation::Status | Operation::Cleanup) {
            return Err("Only Status and Cleanup can be run this way.".into());
        }
        let stage = project.devices.get(index).ok_or("That device isn't in the project.")?.stage;
        if operation == Operation::Cleanup && !matches!(stage, Stage::Healthy | Stage::CleanedUp) {
            return Err(format!("{} is {stage:?}. Cleanup removes the leftovers of the old agent, and is only for a device that has moved and is healthy.", project.devices[index].name));
        }
        let route = self.migration_route(project, &project.devices[index]).await?;
        let device = &project.devices[index];
        let wave = device.wave.unwrap_or(0);
        let params = launch_params(project, wave, package, operation.clone());
        let label = if operation == Operation::Status { "Status" } else { "Cleanup" };
        let name = device.name.clone();

        let (via, result) = match &route {
            Route::Ecoverse { asset_id, script_id } => {
                let input = RunScriptInput {
                    script_id: script_id.clone(),
                    script_name: package.script_name(),
                    target_label: name.clone(),
                    asset_ids: vec![asset_id.clone()],
                    name: format!("N-voke migration: {label} {name}"),
                    timeout_seconds: i64::from(SCRIPT_TIMEOUT_SECONDS),
                    inputs: params.ecoverse_inputs()?,
                    run_when_online: true,
                };
                let run = self.run_script(&input).await?;
                let task_id = run.tasks.first().map(|(id, _)| id.clone()).ok_or("Ecoverse created no task.")?;
                ("Ecoverse".to_string(), self.wait_for_ecoverse_report(&task_id, wait).await?)
            }
            Route::Rest { server_id, item_id, device_id, customer_id } => {
                let command_line = params.command_line()?;
                let task = DirectTask { name: &format!("N-voke migration: {label}"), item_id, customer_id, device_id, device_label: &name, command_line: &command_line };
                let task_id = self.ncentral_run_direct_task(server_id, &task).await.map_err(|e| e.message)?;
                ("N-central".to_string(), self.wait_for_rest_report(server_id, &task_id, wait).await?)
            }
        };

        let now = Utc::now();
        let device = &mut project.devices[index];
        let mut detail = format!("{label} via {via}: {}", if result.message.is_empty() { &result.result } else { &result.message });
        if !result.status_json.is_empty() {
            detail.push_str(&format!("\nstatus.json: {}", result.status_json));
        }
        device.record(now, "status", &detail);
        if operation == Operation::Cleanup && result.cleaned && device.stage == Stage::Healthy {
            let _ = device.move_to(Stage::CleanedUp, now, "cleanup returned Cleaned = true");
        }
        if operation == Operation::Cleanup {
            self.record(&AuditEntry::new("migration.cleanup", &format!("Cleanup on {name}: {}", result.result), Ok(()), json!({ "project": project.id, "cleaned": result.cleaned })));
        }
        Ok(ActionOutcome { via, result })
    }

    async fn wait_for_ecoverse_report(&self, task_id: &str, wait: Duration) -> Result<EndpointResult, String> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let (Some(snapshot), _) = self.get_task(task_id).await? {
                match result_from_ecoverse(&snapshot) {
                    Some(Ok(result)) => return Ok(result),
                    Some(Err(e)) => return Err(e),
                    None => {}
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("No report yet after {} seconds. The task is still queued or running; look at it in Activity.", wait.as_secs()));
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn wait_for_rest_report(&self, server_id: &str, task_id: &str, wait: Duration) -> Result<EndpointResult, String> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(detail) = self.ncentral_task_details(server_id, task_id).await.map_err(|e| e.message)?.into_iter().next() {
                match detail.status.to_ascii_lowercase().as_str() {
                    "success" | "failed" => {
                        let output = match &detail.output_file_name {
                            Some(file) => Some(self.ncentral_task_output(server_id, file).await.map_err(|e| e.message)?),
                            None => None,
                        };
                        return match output.and_then(|o| o.result) {
                            Some(result) => Ok(result),
                            None => Err(format!("The task {} but its output has no report. {}", detail.status.to_lowercase(), detail.message.unwrap_or_default()).trim().to_string()),
                        };
                    }
                    _ => {}
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("No report yet after {} seconds. The task is waiting for the device or still running.", wait.as_secs()));
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use base64::{Engine, engine::general_purpose::STANDARD as B64};
    use chrono::DateTime;
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        migration::{
            endpoint_result::tests::zip_of,
            planner::PlannedDevice,
            project::{RestScript, stamp},
        },
        ncentral::rest::{
            Method, RestRequest, RestResponse,
            testing::{Fake, auth_ok, ok, status},
        },
    };

    fn now() -> DateTime<Utc> {
        "2026-10-20T12:00:00Z".parse().unwrap()
    }

    fn package() -> Package {
        let template = include_str!("../../../../scripts/migration/endpoint/nvoke-migration.template.ps1");
        Package::from_built(&template.replace("__SCRIPT_VERSION__", "0.1.0+3f9a2c1").replace("__INSTALLER_B64__", "QUJD").replace("__INSTALLER_SHA256__", &"A".repeat(64))).unwrap()
    }

    fn device(stage: Stage, path: DeliveryPath) -> ProjectDevice {
        let planned = PlannedDevice {
            device_id: "9001".into(),
            name: "PC-9001".into(),
            source_customer_id: Some("10".into()),
            source_site_id: None,
            dest_org_unit_id: Some("111".into()),
            path: Some(path),
            asset_id: Some("a1".into()),
            blockers: vec![],
            warnings: vec![],
            excluded: false,
        };
        let mut d = ProjectDevice::from_plan(&planned, now());
        d.stage = stage;
        d.entered_stage_at = stamp(now());
        d
    }

    async fn setup(stage: Stage, path: DeliveryPath, reinstall: bool) -> (Arc<Core>, Project) {
        let core = Core::for_tests("actions");
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = if reinstall { src.clone() } else { core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap() };
        let mut p = Project::new("mig-1".into(), "Acme", &src.id, &dst.id, DestEcoverse::NotLinked, now());
        p.devices.push(device(stage, path));
        p.devices[0].dest_device_id = Some("5001".into());
        p.rest_scripts.insert(src.id.clone(), RestScript { item_id: "77".into(), version: "v".into() });
        (core, p)
    }

    #[test]
    fn the_side_follows_the_stage_and_nothing_can_reach_a_device_mid_move() {
        let side = |stage, reinstall| side_for(&device(stage, DeliveryPath::Rest), reinstall);
        for stage in [Stage::Launched, Stage::NotPickedUp, Stage::Staged, Stage::StuckStaged] {
            assert_eq!(side(stage, false), Ok(Side::Source), "{stage:?}");
        }
        for stage in [Stage::Registered, Stage::Healthy, Stage::CleanedUp, Stage::WrongPlace] {
            assert_eq!(side(stage, false), Ok(Side::Destination), "{stage:?}");
            assert_eq!(side(stage, true), Ok(Side::Source), "a reinstall stays on the same server");
        }
        assert!(side(Stage::AgentRemoved, false).unwrap_err().contains("nothing can reach"));
        assert!(side(Stage::Lost, false).is_err());
        assert!(side(Stage::Planned, false).unwrap_err().contains("hasn't been launched"));
        assert!(side(Stage::Retired, false).unwrap_err().contains("finished"));
    }

    #[tokio::test]
    async fn the_source_route_uses_ecoverse_when_the_device_went_that_way_and_n_central_otherwise() {
        let (core, mut p) = setup(Stage::Staged, DeliveryPath::Ecoverse, false).await;
        let route = core.migration_route(&p, &p.devices[0]).await.unwrap();
        assert!(matches!(route, Route::Rest { .. }), "no Ecoverse script is known, so N-central is used");
        p.ecoverse_script_id = Some("script-1".into());
        assert_eq!(core.migration_route(&p, &p.devices[0]).await.unwrap(), Route::Ecoverse { asset_id: "a1".into(), script_id: "script-1".into() });
        p.devices[0].path = Some(DeliveryPath::Rest);
        match core.migration_route(&p, &p.devices[0]).await.unwrap() {
            Route::Rest { item_id, device_id, customer_id, .. } => assert_eq!((item_id.as_str(), device_id.as_str(), customer_id.as_str()), ("77", "9001", "10")),
            other => panic!("{other:?}"),
        }
        p.rest_scripts.clear();
        assert!(core.migration_route(&p, &p.devices[0]).await.unwrap_err().contains("hasn't been uploaded to src.example.com"));
    }

    #[tokio::test]
    async fn on_the_destination_n_central_is_used_with_the_destination_script_and_its_own_customer() {
        let fake = Fake::new(|req: &RestRequest, _| match req.url.trim_start_matches("https://") {
            u if u.ends_with("/authenticate") => auth_ok(3600),
            "dst.example.com/api/devices/5001" => ok(json!({ "data": { "deviceId": 5001, "customerId": 110, "siteId": 111 } })),
            other => panic!("unexpected {other}"),
        });
        let (core, mut p) = setup(Stage::Healthy, DeliveryPath::Rest, false).await;
        core.set_rest_transport(fake);
        let error = core.migration_route(&p, &p.devices[0]).await.unwrap_err();
        assert!(error.contains("hasn't been uploaded to dst.example.com") && error.contains("remove themselves after the review period"), "{error}");
        p.rest_scripts.insert(p.dest_server_id.clone(), RestScript { item_id: "88".into(), version: "v".into() });
        match core.migration_route(&p, &p.devices[0]).await.unwrap() {
            Route::Rest { server_id, item_id, device_id, customer_id } => assert_eq!((server_id, item_id.as_str(), device_id.as_str(), customer_id.as_str()), (p.dest_server_id.clone(), "88", "5001", "110")),
            other => panic!("{other:?}"),
        }
        p.devices[0].dest_device_id = None;
        assert!(core.migration_route(&p, &p.devices[0]).await.unwrap_err().contains("hasn't been found on the destination"));
    }

    const STATUS_LINE: &str = r#"NVOKE-RESULT {"action":"Status","result":"ok","errorCode":0,"message":"Last log: installing","taskState":"Ready","statusJson":"{\"stage\":\"installing\",\"attempts\":2}"}"#;

    /// An N-central server that accepts a direct task and reports `line` for it.
    fn rest_world(line: &'static str, finished: bool, seen: Arc<Mutex<Vec<Value>>>) -> Arc<Fake> {
        Fake::new(move |req: &RestRequest, _| {
            let url = req.url.trim_start_matches("https://");
            let (_, path) = url.split_once('/').unwrap();
            match (req.method, path.split('?').next().unwrap()) {
                (_, "api/auth/authenticate") => auth_ok(3600),
                (Method::Post, "api/scheduled-tasks/direct") => {
                    seen.lock().unwrap().push(req.body.clone().unwrap());
                    status(201, r#"{"data":{"taskId":"5550"}}"#)
                }
                (Method::Get, p) if p.ends_with("/status/details") => {
                    if finished { ok(json!({ "data": [{ "status": "Success", "outputFileName": "/download/scheduled_tasks/output_files/a/b.zip" }] })) } else { ok(json!({ "data": [{ "status": "Pending" }] })) }
                }
                (Method::Get, p) if p.starts_with("download/") => RestResponse { status: 200, body: zip_of("Output-Formatted.txt", format!("Exit Code: 0\r\n{line}\r\n").as_bytes(), true), retry_after: None },
                other => panic!("unexpected {other:?}"),
            }
        })
    }

    #[tokio::test]
    async fn a_status_probe_through_n_central_records_the_devices_own_status_file() {
        let seen = Arc::new(Mutex::new(vec![]));
        let (core, mut p) = setup(Stage::StuckStaged, DeliveryPath::Rest, false).await;
        core.set_rest_transport(rest_world(STATUS_LINE, true, seen.clone()));
        let outcome = core.migration_run_action(&mut p, 0, Operation::Status, &package(), Duration::from_secs(60)).await.unwrap();
        assert_eq!((outcome.via.as_str(), outcome.result.result.as_str(), outcome.result.task_state.as_str()), ("N-central", "ok", "Ready"));
        let line = seen.lock().unwrap()[0]["parameters"][0]["value"].as_str().unwrap().to_string();
        assert!(line.starts_with("-Action Status -DestServer - -DestCustomerId - -RegistrationToken - "), "{line}");
        let event = p.devices[0].events.last().unwrap();
        assert_eq!(event.kind, "status");
        assert!(event.detail.contains("Status via N-central: Last log: installing") && event.detail.contains("\"attempts\":2"), "{}", event.detail);
        assert_eq!(p.devices[0].stage, Stage::StuckStaged, "a probe changes nothing about the stage");
    }

    #[tokio::test]
    async fn a_cleanup_that_reports_cleaned_moves_a_healthy_device_on_and_is_audited() {
        let seen = Arc::new(Mutex::new(vec![]));
        let line: &'static str = r#"NVOKE-RESULT {"action":"Cleanup","result":"cleaned","errorCode":0,"message":"Scheduled task and folder removed","cleaned":true,"statusJson":"-"}"#;
        let (core, mut p) = setup(Stage::Healthy, DeliveryPath::Rest, true).await;
        core.set_rest_transport(rest_world(line, true, seen.clone()));
        let outcome = core.migration_run_action(&mut p, 0, Operation::Cleanup, &package(), Duration::from_secs(60)).await.unwrap();
        assert!(outcome.result.cleaned);
        assert_eq!(p.devices[0].stage, Stage::CleanedUp);
        assert!(core.audit_log(None).unwrap().entries.iter().any(|e| e.action == "migration.cleanup" && e.summary == "Cleanup on PC-9001: cleaned"));
        assert!(seen.lock().unwrap()[0]["parameters"][0]["value"].as_str().unwrap().starts_with("-Action Cleanup"));
    }

    #[tokio::test]
    async fn cleanup_is_only_for_a_device_that_has_moved_and_is_healthy() {
        let line: &'static str = r#"NVOKE-RESULT {"action":"Cleanup","result":"cleaned","errorCode":0,"message":"x","cleaned":true}"#;
        for stage in [Stage::Planned, Stage::Launched, Stage::Staged, Stage::StuckStaged, Stage::Registered, Stage::WrongPlace] {
            let seen = Arc::new(Mutex::new(vec![]));
            let (core, mut p) = setup(stage, DeliveryPath::Rest, false).await;
            core.set_rest_transport(rest_world(line, true, seen.clone()));
            let error = core.migration_run_action(&mut p, 0, Operation::Cleanup, &package(), Duration::from_secs(60)).await.unwrap_err();
            assert!(error.contains("only for a device that has moved and is healthy"), "{stage:?}: {error}");
            assert!(seen.lock().unwrap().is_empty(), "{stage:?}: nothing was sent");
            assert_eq!(p.devices[0].stage, stage);
        }
    }

    #[tokio::test]
    async fn a_cleanup_that_left_something_behind_doesnt_move_the_device() {
        let line: &'static str = r#"NVOKE-RESULT {"action":"Cleanup","result":"failed","errorCode":0,"message":"Left behind: folder exists: True","cleaned":false}"#;
        let (core, mut p) = setup(Stage::Healthy, DeliveryPath::Rest, true).await;
        core.set_rest_transport(rest_world(line, true, Arc::new(Mutex::new(vec![]))));
        core.migration_run_action(&mut p, 0, Operation::Cleanup, &package(), Duration::from_secs(60)).await.unwrap();
        assert_eq!(p.devices[0].stage, Stage::Healthy);
    }

    #[tokio::test(start_paused = true)]
    async fn a_device_that_never_reports_gives_a_clear_timeout_not_a_hang() {
        let (core, mut p) = setup(Stage::Staged, DeliveryPath::Rest, false).await;
        core.set_rest_transport(rest_world(STATUS_LINE, false, Arc::new(Mutex::new(vec![]))));
        let error = core.migration_run_action(&mut p, 0, Operation::Status, &package(), Duration::from_secs(30)).await.unwrap_err();
        assert!(error.contains("No report yet after 30 seconds"), "{error}");
        assert!(p.devices[0].events.iter().all(|e| e.kind != "status"), "nothing is recorded without a report");
    }

    #[tokio::test]
    async fn a_launch_cant_be_run_as_an_action_and_nothing_mid_move_is_attempted() {
        let (core, mut p) = setup(Stage::AgentRemoved, DeliveryPath::Rest, false).await;
        let launch = Operation::Launch { dest_server: "x".into(), dest_customer_id: "1".into(), registration_token: "abcd-1234".into() };
        assert!(core.migration_run_action(&mut p, 0, launch, &package(), Duration::from_secs(1)).await.unwrap_err().contains("Only Status and Cleanup"));
        assert!(core.migration_run_action(&mut p, 0, Operation::Status, &package(), Duration::from_secs(1)).await.unwrap_err().contains("nothing can reach"));
    }

    #[tokio::test(start_paused = true)]
    async fn an_ecoverse_status_probe_waits_for_the_task_and_reads_its_outputs() {
        let (core, mut p) = setup(Stage::Staged, DeliveryPath::Ecoverse, false).await;
        p.ecoverse_script_id = Some("script-1".into());
        let polls = Arc::new(Mutex::new(0));
        let counted = polls.clone();
        core.set_graphql_fake(move |query, variables| {
            if query.contains("assetsScriptRun") {
                assert_eq!(variables["input"]["configuration"]["inputs"].as_array().unwrap().len(), 11);
                return (200, json!({ "data": { "assetsScriptRun": { "items": [{ "id": B64.encode("t:a1") }], "errors": [] } } }).to_string());
            }
            let n = { let mut c = counted.lock().unwrap(); *c += 1; *c };
            // First look: queued. Then: done.
            let task = if n == 1 {
                json!({ "id": "x", "status": "IN_PROGRESS", "executions": { "nodes": [] } })
            } else {
                let outputs: Vec<Value> = [("Result", "ok"), ("Message", "Last log: downloading"), ("StatusJson", "{\"stage\":\"downloading\"}"), ("TaskState", "Ready")]
                    .iter()
                    .map(|(v, s)| json!({ "name": v, "variable": v, "type": "STRING", "value": { "stringValue": s } }))
                    .collect();
                json!({ "id": "x", "status": "COMPLETED", "executions": { "nodes": [{ "id": "e", "status": "SUCCEEDED", "output": { "outputs": outputs } }] } })
            };
            (200, json!({ "data": { "task": task } }).to_string())
        });
        let outcome = core.migration_run_action(&mut p, 0, Operation::Status, &package(), Duration::from_secs(60)).await.unwrap();
        assert_eq!((outcome.via.as_str(), outcome.result.message.as_str()), ("Ecoverse", "Last log: downloading"));
        assert!(*polls.lock().unwrap() >= 2);
        assert!(p.devices[0].events.last().unwrap().detail.contains("\"stage\":\"downloading\""));
    }
}
