//! What the Track step shows and does: every device with where it is, one device in detail (its timeline and what
//! can be done for it), the commands an operator can give a device, and a short digest of each poll for the page.
//! Which stage a device is in, and what is allowed in it, is decided by the tracker and `migration_device_op`;
//! this only shapes and forwards.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{
    packager::Package,
    planner::DeliveryPath,
    poller::PollReport,
    project::{EcoState, Event, MatchCandidate, Project, ProjectDevice, Stage},
    tracker::Thresholds,
    workflow::{DeviceOp, retryable},
};
use crate::{Core, ncentral::model::OrgUnit};

/// How long a server's customer and site names are kept: they change rarely and are only labels.
const NAMES_FOR: Duration = Duration::from_secs(600);
/// How long Probe and Cleanup wait for the device to answer.
const ACTION_WAIT: Duration = Duration::from_secs(120);

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "TrackRow.ts"))]
#[serde(rename_all = "camelCase")]
pub struct TrackRow {
    /// Position in the project's device list: what the commands take.
    pub index: usize,
    pub name: String,
    pub customer: String,
    pub destination: Option<String>,
    pub wave: Option<u32>,
    pub stage: Stage,
    /// When it entered this stage.
    pub since: String,
    pub path: Option<DeliveryPath>,
    /// More than one destination device fits, so the operator chooses.
    pub needs_match: bool,
    /// Why its last launch failed, when it did.
    pub last_error: Option<String>,
    /// What the destination's Ecoverse shows for it: None until it has been read (or before the device has moved).
    pub ecoverse: Option<EcoState>,
    /// Healthy in N-central but not yet sending asset data to Ecoverse: registered, not fully working.
    pub waiting_for_ecoverse: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "TrackView.ts"))]
#[serde(rename_all = "camelCase")]
pub struct TrackView {
    pub rows: Vec<TrackRow>,
    /// (number, label) of each wave, for the filter.
    pub waves: Vec<WaveName>,
    pub customers: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "WaveName.ts"))]
#[serde(rename_all = "camelCase")]
pub struct WaveName {
    pub number: u32,
    pub label: String,
}

/// What an operator can do for a device from where it is now.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DeviceAction.ts"))]
#[serde(rename_all = "camelCase")]
pub enum DeviceAction {
    /// Run Status through whichever agent reaches it.
    Probe,
    /// Run Cleanup: remove the migration's leftovers.
    Cleanup,
    /// Back to Planned, then launch again.
    Retry,
    RetryOtherPath,
    /// Fixed outside N-voke: stop tracking it.
    MarkResolved,
    ChooseMatch,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DeviceEvent.ts"))]
#[serde(rename_all = "camelCase")]
pub struct DeviceEvent {
    pub at: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DeviceCandidate.ts"))]
#[serde(rename_all = "camelCase")]
pub struct DeviceCandidate {
    pub device_id: String,
    pub name: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DeviceDetail.ts"))]
#[serde(rename_all = "camelCase")]
pub struct DeviceDetail {
    pub index: usize,
    pub name: String,
    pub stage: Stage,
    pub since: String,
    pub wave: Option<u32>,
    pub path: Option<DeliveryPath>,
    pub customer: String,
    pub destination: Option<String>,
    pub source_device_id: String,
    pub dest_device_id: Option<String>,
    pub asset_id: Option<String>,
    pub attempts: u32,
    pub last_error: Option<String>,
    /// A launch was sent and its answer lost: it may be running.
    pub launch_unsure: bool,
    pub ecoverse: Option<EcoState>,
    pub waiting_for_ecoverse: bool,
    pub candidates: Vec<DeviceCandidate>,
    pub actions: Vec<DeviceAction>,
    /// Oldest first.
    pub events: Vec<DeviceEvent>,
}

/// What an operator asks of one device.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DeviceCommand.ts"))]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum DeviceCommand {
    Probe,
    Cleanup,
    #[serde(rename_all = "camelCase")]
    Retry { other_path: bool, checked: bool },
    Resolve,
    #[serde(rename_all = "camelCase")]
    ChooseMatch { dest_device_id: String },
}

/// One pass of the tracker, in a few words, for the page to show and refresh from.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "PollDigest.ts"))]
#[serde(rename_all = "camelCase")]
pub struct PollDigest {
    pub checked: usize,
    pub moved: Vec<Moved>,
    /// Reads that failed; the devices involved were left as they were.
    pub errors: Vec<String>,
    pub at: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "Moved.ts"))]
#[serde(rename_all = "camelCase")]
pub struct Moved {
    pub name: String,
    pub from: Stage,
    pub to: Stage,
}

pub fn digest(report: &PollReport, at: &str) -> PollDigest {
    PollDigest {
        checked: report.checked,
        moved: report.moved.iter().map(|(name, from, to)| Moved { name: name.clone(), from: *from, to: *to }).collect(),
        errors: report.errors.clone(),
        at: at.into(),
    }
}

/// What the operator can do for a device in this stage. The core refuses an action that doesn't fit when it is asked
/// for; this only decides which buttons the page shows.
pub fn actions_for(device: &ProjectDevice) -> Vec<DeviceAction> {
    let mut actions = vec![];
    if !device.match_candidates.is_empty() {
        actions.push(DeviceAction::ChooseMatch);
    }
    if matches!(device.stage, Stage::StuckStaged | Stage::Healthy | Stage::Registered | Stage::Lost | Stage::AgentRemoved) {
        actions.push(DeviceAction::Probe);
    }
    if device.stage == Stage::Healthy {
        actions.push(DeviceAction::Cleanup);
    }
    if retryable(device.stage, device.launch_unsure) {
        actions.push(DeviceAction::Retry);
        if matches!(device.stage, Stage::LaunchFailed | Stage::StuckStaged | Stage::NotPickedUp) {
            actions.push(DeviceAction::RetryOtherPath);
        }
    }
    if matches!(device.stage, Stage::Lost | Stage::WrongPlace | Stage::StuckStaged | Stage::LaunchFailed | Stage::NotPickedUp | Stage::Unreachable) {
        actions.push(DeviceAction::MarkResolved);
    }
    actions
}

fn name_of(names: &HashMap<String, String>, id: Option<&str>) -> Option<String> {
    let id = id?;
    Some(names.get(id).cloned().unwrap_or_else(|| format!("#{id}")))
}

pub fn track_view(project: &Project, source_names: &HashMap<String, String>, dest_names: &HashMap<String, String>) -> TrackView {
    let customer = |d: &ProjectDevice| name_of(source_names, d.source_customer_id.as_deref()).unwrap_or_else(|| "(no customer)".into());
    let rows: Vec<TrackRow> = project
        .devices
        .iter()
        .enumerate()
        .map(|(index, d)| TrackRow {
            index,
            name: d.name.clone(),
            customer: customer(d),
            destination: name_of(dest_names, d.dest_org_unit_id.as_deref()),
            wave: d.wave,
            stage: d.stage,
            since: d.entered_stage_at.clone(),
            path: d.path,
            needs_match: !d.match_candidates.is_empty(),
            last_error: d.last_error.clone(),
            ecoverse: d.ecoverse,
            waiting_for_ecoverse: project.waiting_for_ecoverse(d),
        })
        .collect();
    let mut customers: Vec<String> = rows.iter().map(|r| r.customer.clone()).collect();
    customers.sort_by_key(|c| c.to_lowercase());
    customers.dedup();
    TrackView { rows, waves: project.waves.iter().map(|w| WaveName { number: w.number, label: w.label.clone() }).collect(), customers }
}

pub fn device_detail(project: &Project, index: usize, source_names: &HashMap<String, String>, dest_names: &HashMap<String, String>) -> Result<DeviceDetail, String> {
    let d = project.devices.get(index).ok_or("That device isn't in the project.")?;
    Ok(DeviceDetail {
        index,
        name: d.name.clone(),
        stage: d.stage,
        since: d.entered_stage_at.clone(),
        wave: d.wave,
        path: d.path,
        customer: name_of(source_names, d.source_customer_id.as_deref()).unwrap_or_else(|| "(no customer)".into()),
        destination: name_of(dest_names, d.dest_org_unit_id.as_deref()),
        source_device_id: d.source_device_id.clone(),
        dest_device_id: d.dest_device_id.clone(),
        asset_id: d.asset_id.clone(),
        attempts: d.attempts,
        last_error: d.last_error.clone(),
        launch_unsure: d.launch_unsure,
        ecoverse: d.ecoverse,
        waiting_for_ecoverse: project.waiting_for_ecoverse(d),
        candidates: d.match_candidates.iter().map(|MatchCandidate { device_id, name }| DeviceCandidate { device_id: device_id.clone(), name: name.clone() }).collect(),
        actions: actions_for(d),
        events: d.events.iter().map(|Event { at, kind, detail }| DeviceEvent { at: at.clone(), kind: kind.clone(), detail: detail.clone() }).collect(),
    })
}

type Names = Arc<HashMap<String, String>>;

fn name_cache() -> &'static Mutex<HashMap<String, (Instant, Names)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Names)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn names_of(units: &[OrgUnit]) -> HashMap<String, String> {
    units.iter().map(|u| (u.org_unit_id.clone(), u.org_unit_name.clone())).collect()
}

impl Core {
    /// Customer and site names of a server, kept for a few minutes. When the server can't be read the page shows IDs.
    async fn unit_names(&self, server_id: &str) -> Names {
        if let Some((at, names)) = name_cache().lock().unwrap().get(server_id)
            && at.elapsed() < NAMES_FOR
        {
            return names.clone();
        }
        match self.ncentral_org_units(server_id).await {
            Ok(units) => {
                let names = Arc::new(names_of(&units));
                name_cache().lock().unwrap().insert(server_id.to_string(), (Instant::now(), names.clone()));
                names
            }
            Err(_) => Arc::new(HashMap::new()),
        }
    }

    async fn names_for(&self, project: &Project) -> (Names, Names) {
        let source = self.unit_names(&project.source_server_id).await;
        let dest = if project.dest_server_id == project.source_server_id { source.clone() } else { self.unit_names(&project.dest_server_id).await };
        (source, dest)
    }

    pub async fn migration_track_view(&self, id: &str) -> Result<TrackView, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let (source, dest) = self.names_for(&project).await;
        Ok(track_view(&project, &source, &dest))
    }

    pub async fn migration_device_detail(&self, id: &str, index: usize) -> Result<DeviceDetail, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let (source, dest) = self.names_for(&project).await;
        device_detail(&project, index, &source, &dest)
    }

    /// Does what the operator asked for one device and says what happened.
    pub async fn migration_device_command(&self, id: &str, instance: &str, index: usize, command: DeviceCommand) -> Result<String, String> {
        use crate::migration::packager::Operation;
        match command {
            DeviceCommand::Retry { other_path, checked } => self.migration_device_op_saved(id, instance, index, DeviceOp::Retry { other_path, checked }).await.map(|(_, message)| message),
            DeviceCommand::Resolve => self.migration_device_op_saved(id, instance, index, DeviceOp::Resolve).await.map(|(_, message)| message),
            DeviceCommand::ChooseMatch { dest_device_id } => self.migration_device_op_saved(id, instance, index, DeviceOp::ChooseMatch { dest_device_id }).await.map(|(_, message)| message),
            DeviceCommand::Probe | DeviceCommand::Cleanup => {
                let operation = if command == DeviceCommand::Probe { Operation::Status } else { Operation::Cleanup };
                let package = Package::embedded()?;
                let mut project = self.migration_open_for_write(id, instance).await?;
                let outcome = self.migration_run_action(&mut project, index, operation, &package, ACTION_WAIT).await;
                // The answer is on the device's timeline even when the action failed part-way.
                self.migration_save(&mut project).await?;
                let outcome = outcome?;
                let said = if outcome.result.message.is_empty() { outcome.result.result.clone() } else { outcome.result.message.clone() };
                Ok(format!("{} via {}: {said}", project.devices[index].name, outcome.via))
            }
        }
    }

    /// One tracker pass on the saved project.
    pub async fn migration_poll_digest(&self, id: &str, instance: &str) -> Result<PollDigest, String> {
        let (_, report) = self.migration_poll_saved(id, instance, &Thresholds::default()).await?;
        Ok(digest(&report, &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::{
        planner::PlannedDevice,
        project::{DestEcoverse, ProjectDevice},
    };

    fn planned(id: &str, customer: &str, dest: &str) -> PlannedDevice {
        PlannedDevice {
            device_id: id.into(),
            name: format!("PC-{id}"),
            source_customer_id: Some(customer.into()),
            source_site_id: None,
            dest_org_unit_id: Some(dest.into()),
            path: Some(DeliveryPath::Rest),
            asset_id: None,
            blockers: vec![],
            warnings: vec![],
            excluded: false,
        }
    }

    fn project() -> Project {
        let now = chrono::Utc::now();
        let mut p = Project::new("p".into(), "x", "a", "b", DestEcoverse::NotLinked, now);
        p.add_planned(&[planned("1", "10", "110"), planned("2", "11", "111")], now);
        p
    }

    fn names(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn device(stage: Stage) -> ProjectDevice {
        let mut p = project();
        p.devices[0].stage = stage;
        p.devices.remove(0)
    }

    #[test]
    fn rows_carry_names_where_known_ids_where_not_and_the_filters_lists() {
        let mut p = project();
        p.auto_wave("pilot", None, chrono::Utc::now()).unwrap();
        let view = track_view(&p, &names(&[("10", "Zeta"), ("11", "Acme")]), &names(&[("110", "Zeta")]));
        assert_eq!((view.rows[0].customer.as_str(), view.rows[0].destination.as_deref()), ("Zeta", Some("Zeta")));
        assert_eq!(view.rows[1].destination.as_deref(), Some("#111"), "an unknown unit shows its number");
        assert_eq!(view.customers, ["Acme", "Zeta"]);
        assert_eq!(view.waves, [WaveName { number: 1, label: "pilot".into() }]);
        assert_eq!((view.rows[0].index, view.rows[1].index), (0, 1));
    }

    #[test]
    fn what_can_be_done_follows_the_stage() {
        use DeviceAction::*;
        assert_eq!(actions_for(&device(Stage::Healthy)), [Probe, Cleanup]);
        assert_eq!(actions_for(&device(Stage::StuckStaged)), [Probe, Retry, RetryOtherPath, MarkResolved]);
        assert_eq!(actions_for(&device(Stage::LaunchFailed)), [Retry, RetryOtherPath, MarkResolved]);
        assert_eq!(actions_for(&device(Stage::Lost)), [Probe, MarkResolved]);
        assert_eq!(actions_for(&device(Stage::Unreachable)), [Retry, MarkResolved]);
        assert!(actions_for(&device(Stage::Planned)).is_empty());
        assert!(actions_for(&device(Stage::Retired)).is_empty());
    }

    #[test]
    fn a_device_with_candidates_can_be_matched_and_one_with_an_unrecorded_launch_can_be_retried() {
        let mut d = device(Stage::Healthy);
        d.match_candidates = vec![MatchCandidate { device_id: "5".into(), name: "PC-A".into() }];
        assert_eq!(actions_for(&d)[0], DeviceAction::ChooseMatch);
        let mut unsure = device(Stage::Planned);
        unsure.launch_unsure = true;
        assert_eq!(actions_for(&unsure), [DeviceAction::Retry]);
    }

    #[test]
    fn the_detail_has_the_timeline_oldest_first_and_says_where_the_device_is() {
        let mut p = project();
        p.devices[0].record(chrono::Utc::now(), "launch", "task accepted");
        let detail = device_detail(&p, 0, &names(&[("10", "Zeta")]), &names(&[("110", "Zeta")])).unwrap();
        assert_eq!((detail.name.as_str(), detail.customer.as_str(), detail.stage), ("PC-1", "Zeta", Stage::Planned));
        assert_eq!(detail.events.last().map(|e| e.kind.as_str()), Some("launch"));
        assert_eq!(detail.events.first().map(|e| e.kind.as_str()), Some("stage"));
        assert!(device_detail(&p, 9, &HashMap::new(), &HashMap::new()).is_err());
    }

    #[test]
    fn a_poll_digest_names_what_moved_and_what_could_not_be_read() {
        let report = PollReport { checked: 3, moved: vec![("PC-1".into(), Stage::Launched, Stage::Staged)], errors: vec!["Ecoverse timed out.".into()], ..Default::default() };
        let digest = digest(&report, "2026-10-20T20:00:00Z");
        assert_eq!((digest.checked, digest.moved[0].to, digest.errors.len()), (3, Stage::Staged, 1));
    }

    #[test]
    fn commands_come_from_the_page_as_tagged_objects() {
        let retry: DeviceCommand = serde_json::from_str(r#"{"kind":"retry","otherPath":true,"checked":false}"#).unwrap();
        assert_eq!(retry, DeviceCommand::Retry { other_path: true, checked: false });
        let pick: DeviceCommand = serde_json::from_str(r#"{"kind":"chooseMatch","destDeviceId":"9"}"#).unwrap();
        assert_eq!(pick, DeviceCommand::ChooseMatch { dest_device_id: "9".into() });
        assert_eq!(serde_json::from_str::<DeviceCommand>(r#"{"kind":"probe"}"#).unwrap(), DeviceCommand::Probe);
    }
}
