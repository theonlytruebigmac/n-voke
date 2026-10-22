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
    /// An Ecoverse asset, with the script published for that tenant. `server_id` is the N-central server whose Ecoverse
    /// key reaches that tenant.
    Ecoverse { asset_id: String, script_id: String, server_id: String },
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
    /// A device that moved into another Ecoverse tenant can only be reached through a copy of the script published in
    /// *that* tenant. When it isn't there (or is an older version), publish it now in the tenant's service
    /// organization, so Probe and Cleanup just work. Says what it did, or why it couldn't; the device is then reached
    /// over N-central if that is set up.
    pub(crate) async fn ensure_destination_script(&self, project: &mut Project, package: &Package) -> Result<Option<String>, String> {
        if !matches!(project.dest_ecoverse, DestEcoverse::OtherTenant { .. }) || !self.server_uses_ecoverse(&project.dest_server_id) {
            return Ok(None);
        }
        let current = project.dest_ecoverse_script_id.is_some() && project.dest_ecoverse_script_version.as_deref() == Some(package.version.as_str());
        if current {
            return Ok(None);
        }
        let org = match project.dest_ecoverse_script_org.clone() {
            Some(org) => org,
            None => self
                .migration_default_script_org(&project.dest_server_id)
                .await?
                .ok_or("The destination's Ecoverse has more than one top-level organization (or none), so the script wasn't published there. Choose one in Prepare.")?,
        };
        let published = self.with_ecoverse(&project.dest_server_id, self.publish_migration_script(package, &org.id)).await?;
        project.dest_ecoverse_script_id = Some(published.script_id);
        project.dest_ecoverse_script_version = Some(package.version.clone());
        project.dest_ecoverse_script_org = Some(org.clone());
        Ok(Some(format!("Published the endpoint script in the destination's Ecoverse ({}) so it can reach this device.", org.name)))
    }

    /// Works out how to reach the device now. Reads from the servers where it has to (the destination's record and Ecoverse asset).
    pub async fn migration_route(&self, project: &Project, device: &ProjectDevice) -> Result<Route, String> {
        let side = side_for(device, project.is_reinstall())?;
        let rest_for = |server_id: &str, device_id: &str, customer_id: Option<String>| -> Result<Route, String> {
            let item = project.rest_scripts.get(server_id).ok_or_else(|| {
                let host = self.ncentral_server(server_id).map(|s| s.host).unwrap_or_default();
                format!(
                    "The endpoint script hasn't been uploaded to {host}, so N-central can't be used to reach this device. Upload it and record it in Prepare.{}",
                    if side == Side::Destination { " Until then its records will remove themselves after the review period." } else { "" }
                )
            })?;
            Ok(Route::Rest { server_id: server_id.into(), item_id: item.item_id.clone(), device_id: device_id.into(), customer_id: customer_id.ok_or("The device's customer isn't known.")? })
        };
        match side {
            Side::Source => match (device.path, &device.asset_id, &project.ecoverse_script_id) {
                (Some(DeliveryPath::Ecoverse), Some(asset), Some(script)) => Ok(Route::Ecoverse { asset_id: asset.clone(), script_id: script.clone(), server_id: project.source_server_id.clone() }),
                _ => rest_for(&project.source_server_id, &device.source_device_id, device.source_customer_id.clone()),
            },
            Side::Destination => {
                let dest_id = device.dest_device_id.clone().ok_or("The device hasn't been found on the destination yet.")?;
                let dest_host = self.ncentral_server(&project.dest_server_id)?.host;
                // Where the device shows up in Ecoverse once it has moved: the source's tenant (same tenant), or the
                // destination's own (another tenant), each with its own key and its own copy of the script.
                let tenant = match project.dest_ecoverse {
                    DestEcoverse::SameTenant => project.ecoverse_script_id.as_ref().map(|s| (project.source_server_id.clone(), s.clone(), &project.source_ecoverse_org_ids)),
                    DestEcoverse::OtherTenant { .. } => project.dest_ecoverse_script_id.as_ref().map(|s| (project.dest_server_id.clone(), s.clone(), &project.dest_ecoverse_org_ids)),
                    DestEcoverse::NotLinked => None,
                };
                // Why Ecoverse isn't used, so that when N-central can't help either the message says what is actually missing.
                let mut why_not_ecoverse = match (&project.dest_ecoverse, &tenant) {
                    (DestEcoverse::NotLinked, _) => String::new(),
                    (DestEcoverse::OtherTenant { .. }, None) => "The endpoint script isn't published in the destination's Ecoverse.".to_string(),
                    (DestEcoverse::SameTenant, None) => "The endpoint script isn't published in Ecoverse.".to_string(),
                    _ => String::new(),
                };
                // Ecoverse can't run a script on an agent that hasn't had its data yet: the task would sit queued while the Windows
                // Agent, which is checking in, could run it at once. So N-central goes first for such a device, if it can be used.
                let ecoverse_not_ready = project.waiting_for_ecoverse(device) && project.rest_scripts.contains_key(&project.dest_server_id);
                if ecoverse_not_ready && tenant.is_some() {
                    why_not_ecoverse = "Ecoverse can't run scripts on it until its data arrives, so N-central is used.".into();
                }
                if let Some((server_id, script, org_ids)) = tenant.filter(|_| !ecoverse_not_ready) {
                    match self.with_ecoverse(&server_id, self.eco_devices(org_ids)).await {
                        Ok(assets) => match index_by_ncentral_id(&assets, &dest_host).remove(&dest_id) {
                            Some(asset) if asset.connected => return Ok(Route::Ecoverse { asset_id: asset.asset_id, script_id: script, server_id }),
                            Some(_) => why_not_ecoverse = "Ecoverse shows this device, but it isn't connected there right now.".into(),
                            None => why_not_ecoverse = "Ecoverse doesn't show this device yet.".into(),
                        },
                        Err(error) => why_not_ecoverse = format!("Ecoverse couldn't be read ({error})."),
                    }
                }
                let record = self.ncentral_device(&project.dest_server_id, &dest_id).await.map_err(|e| e.message)?;
                rest_for(&project.dest_server_id, &dest_id, record.customer_id).map_err(|e| if why_not_ecoverse.is_empty() { e } else { format!("{why_not_ecoverse} {e}") })
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
        // After a move into another Ecoverse tenant the script has to be in that tenant: put it there first if it isn't.
        let published = match side_for(&project.devices[index], project.is_reinstall()) {
            Ok(Side::Destination) => self.ensure_destination_script(project, package).await,
            _ => Ok(None),
        };
        if let Ok(Some(note)) = &published {
            project.devices[index].record(chrono::Utc::now(), "note", note);
        }
        let route = match self.migration_route(project, &project.devices[index]).await {
            Ok(route) => route,
            Err(error) => return Err(match published {
                Err(why) => format!("{error} ({why})"),
                _ => error,
            }),
        };
        let device = &project.devices[index];
        let wave = device.wave.unwrap_or(0);
        let params = launch_params(project, wave, package, operation.clone());
        let label = if operation == Operation::Status { "Status" } else { "Cleanup" };
        let name = device.name.clone();

        let (via, result) = match &route {
            Route::Ecoverse { asset_id, script_id, server_id } => {
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
                let report = self
                    .with_ecoverse(server_id, async {
                        let run = self.run_script(&input).await?;
                        let task_id = run.tasks.first().map(|(id, _)| id.clone()).ok_or("Ecoverse created no task.")?;
                        self.wait_for_ecoverse_report(&task_id, wait).await
                    })
                    .await?;
                ("Ecoverse".to_string(), report)
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
            project::{EcoState, RestScript, stamp},
            trackview::PROBE_BATCH,
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
        assert_eq!(core.migration_route(&p, &p.devices[0]).await.unwrap(), Route::Ecoverse { asset_id: "a1".into(), script_id: "script-1".into(), server_id: p.source_server_id.clone() });
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

    #[tokio::test]
    async fn a_device_that_moved_to_another_tenant_is_reached_through_the_destinations_own_ecoverse_script_and_key() {
        let fake = Fake::new(|req: &RestRequest, _| match req.url.trim_start_matches("https://") {
            u if u.ends_with("/authenticate") => auth_ok(3600),
            "dst.example.com/api/devices/5001" => ok(json!({ "data": { "deviceId": 5001, "customerId": 110, "siteId": 111 } })),
            other => panic!("unexpected {other}"),
        });
        let (core, mut p) = setup(Stage::Healthy, DeliveryPath::Rest, false).await;
        core.set_rest_transport(fake);
        p.dest_ecoverse = DestEcoverse::OtherTenant { tenant_id: "t".into() };
        p.ecoverse_script_id = Some("source-script".into());
        p.rest_scripts.insert(p.dest_server_id.clone(), RestScript { item_id: "88".into(), version: "v".into() });
        core.set_graphql_fake(|_, _| {
            let asset = json!({ "id": "asset-new", "name": "PC", "agentConnection": { "status": "CONNECTED" }, "ncentralDevice": { "deviceId": "5001", "server": { "id": "s", "origin": "https://dst.example.com" } } });
            (200, json!({ "data": { "page": { "totalCount": 1, "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": [asset] } } }).to_string())
        });

        // The destination's tenant has no script of its own yet, so the device is reached through N-central.
        assert!(matches!(core.migration_route(&p, &p.devices[0]).await.unwrap(), Route::Rest { .. }));

        // With it, the source tenant's script is not used for a device that is now in the other tenant. A device whose Ecoverse
        // agent is reporting is reached there; one still waiting for its data goes through N-central instead (a queued Ecoverse
        // task would never run).
        p.dest_ecoverse_script_id = Some("dest-script".into());
        assert!(matches!(core.migration_route(&p, &p.devices[0]).await.unwrap(), Route::Rest { .. }), "waiting for Ecoverse data");
        p.devices[0].ecoverse = Some(EcoState::Reporting);
        assert_eq!(core.migration_route(&p, &p.devices[0]).await.unwrap(), Route::Ecoverse { asset_id: "asset-new".into(), script_id: "dest-script".into(), server_id: p.dest_server_id.clone() });
        p.devices[0].ecoverse = None;
        p.rest_scripts.clear();
        assert!(matches!(core.migration_route(&p, &p.devices[0]).await.unwrap(), Route::Ecoverse { .. }), "without N-central's script, Ecoverse is still tried");
    }

    fn orgs_fake(core: &Core, orgs: &'static [(&'static str, &'static str)]) {
        core.set_graphql_fake(move |query, _| {
            let body = if query.contains("organizationSearch") {
                let nodes: Vec<Value> = orgs.iter().map(|(id, kind)| json!({ "id": id, "name": format!("Org {id}"), "__typename": kind })).collect();
                json!({ "data": { "page": { "totalCount": nodes.len(), "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": nodes } } })
            } else if query.contains("scriptSearch") {
                json!({ "data": { "byName": { "nodes": [] } } })
            } else {
                json!({ "data": { "scriptCreate": { "item": { "id": "dest-script" }, "errors": [] } } })
            };
            (200, body.to_string())
        });
    }

    #[tokio::test]
    async fn the_destinations_script_is_published_on_demand_in_the_tenants_one_service_organization() {
        let (core, mut p) = setup(Stage::Healthy, DeliveryPath::Rest, false).await;
        p.dest_ecoverse = DestEcoverse::OtherTenant { tenant_id: "t".into() };
        orgs_fake(&core, &[("c1", "Customer"), ("so", "ServiceOrganization"), ("s1", "Site")]);

        let note = core.ensure_destination_script(&mut p, &package()).await.unwrap().expect("it published");
        assert!(note.contains("Org so"), "{note}");
        assert_eq!((p.dest_ecoverse_script_id.as_deref(), p.dest_ecoverse_script_org.as_ref().map(|o| o.id.as_str())), (Some("dest-script"), Some("so")));
        assert_eq!(p.dest_ecoverse_script_version.as_deref(), Some(package().version.as_str()));
        assert!(core.ensure_destination_script(&mut p, &package()).await.unwrap().is_none(), "already there at this version: nothing to do");

        // A destination that isn't in another tenant has nothing to publish.
        let (core2, mut same) = setup(Stage::Healthy, DeliveryPath::Rest, false).await;
        assert!(core2.ensure_destination_script(&mut same, &package()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn with_no_single_top_level_organization_it_asks_the_person_instead_of_guessing() {
        for orgs in [&[("c1", "Customer")][..], &[("a", "ServiceOrganization"), ("b", "ServiceOrganization")][..]] {
            let (core, mut p) = setup(Stage::Healthy, DeliveryPath::Rest, false).await;
            p.dest_ecoverse = DestEcoverse::OtherTenant { tenant_id: "t".into() };
            orgs_fake(&core, Box::leak(orgs.to_vec().into_boxed_slice()));
            let error = core.ensure_destination_script(&mut p, &package()).await.unwrap_err();
            assert!(error.contains("Choose one in Prepare"), "{error}");
            assert!(p.dest_ecoverse_script_id.is_none());
        }
    }

    #[tokio::test]
    async fn when_neither_path_works_the_message_says_why_ecoverse_wasnt_used_before_asking_for_the_upload() {
        let fake = Fake::new(|req: &RestRequest, _| match req.url.trim_start_matches("https://") {
            u if u.ends_with("/authenticate") => auth_ok(3600),
            "dst.example.com/api/devices/5001" => ok(json!({ "data": { "deviceId": 5001, "customerId": 110, "siteId": 111 } })),
            other => panic!("unexpected {other}"),
        });
        let (core, mut p) = setup(Stage::Healthy, DeliveryPath::Rest, false).await;
        core.set_rest_transport(fake);
        p.dest_ecoverse = DestEcoverse::OtherTenant { tenant_id: "t".into() };
        let error = core.migration_route(&p, &p.devices[0]).await.unwrap_err();
        assert!(error.starts_with("The endpoint script isn't published in the destination's Ecoverse."), "{error}");
        assert!(error.contains("hasn't been uploaded to dst.example.com"), "{error}");

        // With the script there but no asset for the device yet, that is the reason given.
        p.dest_ecoverse_script_id = Some("dest-script".into());
        core.set_graphql_fake(|_, _| (200, json!({ "data": { "page": { "totalCount": 0, "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": [] } } }).to_string()));
        let error = core.migration_route(&p, &p.devices[0]).await.unwrap_err();
        assert!(error.starts_with("Ecoverse doesn't show this device yet."), "{error}");
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

    #[tokio::test]
    async fn probing_a_device_that_moved_to_another_tenant_publishes_the_script_there_first_then_runs_it() {
        let fake = Fake::new(|req: &RestRequest, _| match req.url.trim_start_matches("https://") {
            u if u.ends_with("/authenticate") => auth_ok(3600),
            "dst.example.com/api/devices/5001" => ok(json!({ "data": { "deviceId": 5001, "customerId": 110, "siteId": 111 } })),
            other => panic!("unexpected {other}"),
        });
        let (core, mut p) = setup(Stage::Healthy, DeliveryPath::Rest, false).await;
        core.set_rest_transport(fake);
        p.dest_ecoverse = DestEcoverse::OtherTenant { tenant_id: "t".into() };
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        core.set_graphql_fake(move |query, variables| {
            let body = if query.contains("organizationSearch") && !query.contains("assetSearch") {
                log.lock().unwrap().push("orgs".into());
                json!({ "data": { "page": { "totalCount": 1, "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": [{ "id": "so", "name": "Dest SO", "__typename": "ServiceOrganization" }] } } })
            } else if query.contains("scriptSearch") {
                json!({ "data": { "byName": { "nodes": [] } } })
            } else if query.contains("scriptCreate") {
                log.lock().unwrap().push("publish".into());
                json!({ "data": { "scriptCreate": { "item": { "id": "dest-script" }, "errors": [] } } })
            } else if query.contains("assetSearch") {
                let asset = json!({ "id": "asset-new", "agentConnection": { "status": "CONNECTED" }, "ncentralDevice": { "deviceId": "5001", "server": { "id": "s", "origin": "https://dst.example.com" } } });
                json!({ "data": { "page": { "totalCount": 1, "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": [asset] } } })
            } else if query.contains("assetsScriptRun") {
                log.lock().unwrap().push(format!("run {}", variables["input"]["configuration"]["scriptId"]));
                json!({ "data": { "assetsScriptRun": { "items": [{ "id": B64.encode("t:asset-new") }], "errors": [] } } })
            } else {
                let outputs: Vec<Value> = [("Result", "ok"), ("Message", "Last log: fine"), ("StatusJson", "{}"), ("TaskState", "Ready")]
                    .iter()
                    .map(|(v, s)| json!({ "name": v, "variable": v, "type": "STRING", "value": { "stringValue": s } }))
                    .collect();
                json!({ "data": { "task": { "id": "x", "status": "COMPLETED", "executions": { "nodes": [{ "id": "e", "status": "SUCCEEDED", "output": { "outputs": outputs } }] } } } })
            };
            (200, body.to_string())
        });

        let outcome = core.migration_run_action(&mut p, 0, Operation::Status, &package(), Duration::from_secs(60)).await.unwrap();
        assert_eq!(outcome.via, "Ecoverse");
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen, ["orgs", "publish", "run \"dest-script\""], "published in the destination's service organization, then run with that copy");
        assert!(p.devices[0].events.iter().any(|e| e.kind == "note" && e.detail.contains("Published the endpoint script in the destination's Ecoverse (Dest SO)")));
    }

    const BLOCKED_STATUS_LINE: &str = r#"NVOKE-RESULT {"action":"Status","result":"ok","errorCode":0,"message":"Last log: awaiting-ufa","taskState":"Ready","statusJson":"{\"ufa\":{\"present\":true,\"provisioned\":true,\"sync\":\"blocked\"},\"ufaSyncRestarts\":1}"}"#;

    /// A reinstall project (so every device is reached through its one N-central) linked to Ecoverse, with healthy devices
    /// named PC-1... : the first `waiting` still have no data in Ecoverse, the rest are reporting.
    async fn waiting_project(waiting: usize, reporting: usize) -> (Arc<Core>, Project) {
        let (core, mut p) = setup(Stage::Healthy, DeliveryPath::Rest, true).await;
        p.dest_ecoverse = DestEcoverse::SameTenant;
        let template = p.devices.remove(0);
        for i in 0..waiting + reporting {
            let mut d = template.clone();
            d.name = format!("PC-{}", i + 1);
            d.source_device_id = format!("{}", 9100 + i);
            d.dest_device_id = Some(format!("{}", 5100 + i));
            // Later numbers have been healthy for less time.
            d.entered_stage_at = stamp(now() + chrono::Duration::minutes(i as i64));
            d.ecoverse = Some(if i < waiting { EcoState::WaitingForData } else { EcoState::Reporting });
            p.devices.push(d);
        }
        core.migration_save(&mut p).await.unwrap();
        (core, p)
    }

    #[tokio::test]
    async fn a_bulk_probe_reaches_only_the_devices_waiting_for_ecoverse_and_tallies_what_their_agents_say() {
        use crate::migration::deviceufa::UfaState;
        let seen = Arc::new(Mutex::new(vec![]));
        let (core, p) = waiting_project(2, 1).await;
        core.set_rest_transport(rest_world(BLOCKED_STATUS_LINE, true, seen.clone()));

        let report = core.migration_probe_waiting(&p.id, "me").await.unwrap();
        assert_eq!((report.probed, report.no_record, report.not_reached, report.failed.len()), (2, 0, 0, 0));
        assert_eq!(report.states.iter().map(|s| (s.state, s.count)).collect::<Vec<_>>(), [(UfaState::Blocked, 2)]);
        assert_eq!(seen.lock().unwrap().len(), 2, "the device that is reporting was not probed");

        let saved = core.migration_load(&p.id).await.unwrap().unwrap();
        let probed: Vec<&str> = saved.devices.iter().filter(|d| d.events.last().is_some_and(|e| e.kind == "status")).map(|d| d.name.as_str()).collect();
        assert_eq!(probed, ["PC-1", "PC-2"]);
        assert!(saved.devices.iter().all(|d| d.stage == Stage::Healthy), "a probe changes nothing about the stage");
    }

    #[tokio::test]
    async fn a_bulk_probe_counts_an_answer_with_no_agent_record_and_goes_on_after_a_device_that_cannot_be_reached() {
        let line: &'static str = r#"NVOKE-RESULT {"action":"Status","result":"ok","errorCode":0,"message":"no record","taskState":"Ready","statusJson":"-"}"#;
        let (core, p) = waiting_project(2, 0).await;
        core.set_rest_transport(rest_world(line, true, Arc::new(Mutex::new(vec![]))));
        let report = core.migration_probe_waiting(&p.id, "me").await.unwrap();
        assert_eq!((report.probed, report.no_record, report.states.len()), (2, 2, 0));

        // A project with no script uploaded can't reach any device: each is reported, nothing stops.
        let (core, mut p) = waiting_project(2, 0).await;
        p.rest_scripts.clear();
        core.migration_save(&mut p).await.unwrap();
        let report = core.migration_probe_waiting(&p.id, "me").await.unwrap();
        assert_eq!((report.probed, report.failed.len()), (0, 2));
        assert!(report.failed[0].why.contains("hasn't been uploaded"), "{}", report.failed[0].why);
    }

    #[tokio::test]
    async fn a_bulk_probe_takes_the_longest_waiting_first_and_leaves_the_rest_for_another_run() {
        let (core, p) = waiting_project(PROBE_BATCH + 2, 0).await;
        core.set_rest_transport(rest_world(BLOCKED_STATUS_LINE, true, Arc::new(Mutex::new(vec![]))));
        let report = core.migration_probe_waiting(&p.id, "me").await.unwrap();
        assert_eq!((report.probed, report.not_reached), (PROBE_BATCH, 2));
        let saved = core.migration_load(&p.id).await.unwrap().unwrap();
        let left: Vec<&str> = saved.devices.iter().filter(|d| !d.events.last().is_some_and(|e| e.kind == "status")).map(|d| d.name.as_str()).collect();
        assert_eq!(left, [format!("PC-{}", PROBE_BATCH + 1), format!("PC-{}", PROBE_BATCH + 2)], "the ones that became healthy last wait");
    }

    #[tokio::test]
    async fn the_tracker_probes_a_waiting_device_on_its_own_not_again_within_the_interval_and_a_failure_is_retried_after_it() {
        let seen = Arc::new(Mutex::new(vec![]));
        let (core, p) = waiting_project(2, 1).await;
        core.set_rest_transport(rest_world(BLOCKED_STATUS_LINE, true, seen.clone()));
        let half_hour = chrono::Duration::minutes(30);

        assert_eq!(core.migration_probe_due(&p.id, "me", half_hour).await.unwrap(), 2, "the two waiting devices, not the one that is reporting");
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(core.migration_probe_due(&p.id, "me", half_hour).await.unwrap(), 0, "they were just probed");
        assert_eq!(seen.lock().unwrap().len(), 2, "and nothing was sent");
        assert_eq!(core.migration_probe_due(&p.id, "me", chrono::Duration::milliseconds(-1)).await.unwrap(), 2, "after the interval they are due again");
        let saved = core.migration_load(&p.id).await.unwrap().unwrap();
        assert!(saved.devices.iter().all(|d| d.stage == Stage::Healthy), "a probe changes nothing about the stage");

        // Unreachable devices: a note on the timeline, and no retry until the interval has passed.
        let (core, mut p) = waiting_project(2, 0).await;
        p.rest_scripts.clear();
        core.migration_save(&mut p).await.unwrap();
        assert_eq!(core.migration_probe_due(&p.id, "me", half_hour).await.unwrap(), 2);
        let saved = core.migration_load(&p.id).await.unwrap().unwrap();
        let last = saved.devices[0].events.last().unwrap();
        assert!(last.kind == "note" && last.detail.starts_with("Automatic Probe failed") && last.detail.contains("hasn't been uploaded"), "{last:?}");
        assert_eq!(core.migration_probe_due(&p.id, "me", half_hour).await.unwrap(), 0, "not on every pass");
        assert_eq!(core.migration_probe_due(&p.id, "me", chrono::Duration::milliseconds(-1)).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn the_tracker_probes_only_a_few_at_a_time_leaves_a_device_that_just_became_healthy_and_does_nothing_when_not_linked() {
        use super::super::trackview::AUTO_PROBE_BATCH;
        let seen = Arc::new(Mutex::new(vec![]));
        let (core, mut p) = waiting_project(AUTO_PROBE_BATCH + 2, 0).await;
        // The last one became healthy a moment ago: its agent is still provisioning.
        let last = p.devices.len() - 1;
        p.devices[last].entered_stage_at = stamp(chrono::Utc::now());
        core.migration_save(&mut p).await.unwrap();
        core.set_rest_transport(rest_world(BLOCKED_STATUS_LINE, true, seen.clone()));
        let every = chrono::Duration::minutes(30);
        assert_eq!(core.migration_probe_due(&p.id, "me", every).await.unwrap(), AUTO_PROBE_BATCH, "a few per pass");
        assert_eq!(core.migration_probe_due(&p.id, "me", every).await.unwrap(), 1, "the rest on the next pass; the newest healthy one still waits");
        assert_eq!(core.migration_probe_due(&p.id, "me", every).await.unwrap(), 0);

        let (core, mut p) = waiting_project(2, 0).await;
        p.dest_ecoverse = DestEcoverse::NotLinked;
        core.migration_save(&mut p).await.unwrap();
        assert_eq!(core.migration_probe_due(&p.id, "me", every).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_bulk_probe_has_nothing_to_do_when_the_destination_is_not_linked_to_ecoverse() {
        let (core, mut p) = waiting_project(2, 0).await;
        p.dest_ecoverse = DestEcoverse::NotLinked;
        core.migration_save(&mut p).await.unwrap();
        core.set_rest_transport(rest_world(BLOCKED_STATUS_LINE, true, Arc::new(Mutex::new(vec![]))));
        let report = core.migration_probe_waiting(&p.id, "me").await.unwrap();
        assert_eq!((report.probed, report.not_reached, report.failed.len()), (0, 0, 0));
    }
}
