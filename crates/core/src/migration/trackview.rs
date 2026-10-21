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
    deviceufa::{DeviceUfa, UfaState, device_ufa},
    finish::CleanupFailure,
    packager::Package,
    planner::DeliveryPath,
    poller::PollReport,
    project::{DestEcoverse, EcoState, Event, MatchCandidate, Project, ProjectDevice, Stage},
    workflow::{DeviceOp, retryable},
};
use crate::{Core, ncentral::model::OrgUnit};

/// How long a server's customer and site names are kept: they change rarely and are only labels.
const NAMES_FOR: Duration = Duration::from_secs(600);
/// How long Probe and Cleanup wait for the device to answer.
const ACTION_WAIT: Duration = Duration::from_secs(120);
/// Devices probed per request, longest-waiting first: each one is a script run on a device, one after another.
pub(crate) const PROBE_BATCH: usize = 20;

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

/// The stage strip's groups: the stages read in nine.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "TrackGroup.ts"))]
#[serde(rename_all = "camelCase")]
pub enum TrackGroup {
    Planned,
    Launched,
    Staged,
    Moving,
    OnNew,
    /// Healthy in N-central, but Ecoverse isn't receiving its data yet.
    Waiting,
    Healthy,
    Done,
    NeedsYou,
}

impl TrackGroup {
    pub const ALL: [TrackGroup; 9] = [Self::Planned, Self::Launched, Self::Staged, Self::Moving, Self::OnNew, Self::Waiting, Self::Healthy, Self::Done, Self::NeedsYou];

    /// A device is in exactly one group; a device taken out of the project is in none.
    pub fn of(stage: Stage, waiting_for_ecoverse: bool) -> Option<Self> {
        Some(match stage {
            Stage::Planned => Self::Planned,
            Stage::Launched => Self::Launched,
            Stage::Staged => Self::Staged,
            Stage::AgentRemoved => Self::Moving,
            Stage::Registered => Self::OnNew,
            Stage::Healthy if waiting_for_ecoverse => Self::Waiting,
            Stage::Healthy => Self::Healthy,
            Stage::CleanedUp | Stage::Retired | Stage::Resolved => Self::Done,
            Stage::Unreachable | Stage::NotPickedUp | Stage::LaunchFailed | Stage::StuckStaged | Stage::Lost | Stage::WrongPlace => Self::NeedsYou,
            Stage::Excluded => return None,
        })
    }
}

/// Which rows of the device table to send: a filter and a page.
#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "TrackQuery.ts"))]
#[serde(rename_all = "camelCase", default)]
pub struct TrackQuery {
    pub group: Option<TrackGroup>,
    pub wave: Option<u32>,
    pub customer: Option<String>,
    pub text: String,
    pub offset: usize,
    /// 0 means the default page.
    pub limit: usize,
}

const DEFAULT_PAGE: usize = 200;
const MAX_PAGE: usize = 1000;
/// How many devices the "needs you" list shows.
/// How many devices of each reason are listed to open; "Show them all" has the rest.
const NEEDS_YOU_EXAMPLES: usize = 3;

impl TrackQuery {
    pub fn all() -> Self {
        Self::default()
    }

    fn keeps(&self, row: &TrackRow) -> bool {
        let in_group = match self.group {
            None => true,
            Some(TrackGroup::NeedsYou) => row_needs_you(row),
            Some(group) => TrackGroup::of(row.stage, row.waiting_for_ecoverse) == Some(group),
        };
        let term = self.text.trim().to_lowercase();
        in_group
            && self.wave.is_none_or(|w| row.wave == Some(w))
            && self.customer.as_deref().is_none_or(|c| row.customer == c)
            && (term.is_empty() || row.name.to_lowercase().contains(&term) || row.customer.to_lowercase().contains(&term))
    }
}

fn row_needs_you(row: &TrackRow) -> bool {
    row.stage.needs_attention() || row.needs_match
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "GroupCount.ts"))]
#[serde(rename_all = "camelCase")]
pub struct GroupCount {
    pub group: TrackGroup,
    pub count: usize,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "TrackView.ts"))]
#[serde(rename_all = "camelCase")]
pub struct TrackView {
    /// One page of the devices that match the query.
    pub rows: Vec<TrackRow>,
    pub matching: usize,
    pub offset: usize,
    /// (number, label) of each wave, for the filter.
    pub waves: Vec<WaveName>,
    pub customers: Vec<String>,
    /// Devices per group over the whole project. A device with a match to choose also counts under needs you.
    pub counts: Vec<GroupCount>,
    /// How many devices need the person in all, and what for: one entry per reason (a stage, or a match to choose),
    /// the commonest first, each with its first few devices.
    pub needs_you_total: usize,
    pub needs_you: Vec<NeedKind>,
}

/// One reason devices need the person, with how many there are and a few of them to open.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "NeedKind.ts"))]
#[serde(rename_all = "camelCase")]
pub struct NeedKind {
    pub stage: Stage,
    /// Several devices on the new server fit, and the person chooses which: the reason, whatever the stage.
    pub needs_match: bool,
    pub count: usize,
    pub examples: Vec<TrackRow>,
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
    /// Why Ecoverse doesn't show it, when it doesn't: where it was looked and what was found there.
    pub ecoverse_note: Option<String>,
    pub waiting_for_ecoverse: bool,
    /// What the device's own record says about its Ecoverse agent, from the last Probe. Only for a project whose
    /// destination is linked to Ecoverse; none when the device hasn't been probed.
    pub device_ufa: Option<DeviceUfa>,
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

/// What a bulk Probe of the devices waiting for Ecoverse's data found.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ProbeReport.ts"))]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    /// Devices that answered.
    pub probed: usize,
    /// Of those, how many reported each state of their Ecoverse agent, most common first.
    pub states: Vec<UfaCount>,
    /// Answered, but with no record of the Ecoverse agent in it.
    pub no_record: usize,
    pub failed: Vec<CleanupFailure>,
    /// Still waiting for Ecoverse's data and not probed this time: only so many are probed at once.
    pub not_reached: usize,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "UfaCount.ts"))]
#[serde(rename_all = "camelCase")]
pub struct UfaCount {
    pub state: UfaState,
    pub count: usize,
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

/// The devices that need the person grouped by why, the commonest reason first (then by stage order).
fn need_kinds(needs: &[&TrackRow]) -> Vec<NeedKind> {
    let mut kinds: Vec<NeedKind> = vec![];
    for row in needs {
        let found = kinds.iter_mut().find(|k| k.needs_match == row.needs_match && (row.needs_match || k.stage == row.stage));
        let kind = match found {
            Some(k) => k,
            None => {
                kinds.push(NeedKind { stage: row.stage, needs_match: row.needs_match, count: 0, examples: vec![] });
                kinds.last_mut().unwrap()
            }
        };
        kind.count += 1;
        if kind.examples.len() < NEEDS_YOU_EXAMPLES {
            kind.examples.push((*row).clone());
        }
    }
    kinds.sort_by(|a, b| b.count.cmp(&a.count).then(a.stage.cmp(&b.stage)));
    kinds
}

pub fn track_view(project: &Project, source_names: &HashMap<String, String>, dest_names: &HashMap<String, String>, query: &TrackQuery) -> TrackView {
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

    let counts = TrackGroup::ALL
        .iter()
        .map(|&group| GroupCount {
            group,
            count: rows.iter().filter(|r| if group == TrackGroup::NeedsYou { row_needs_you(r) } else { TrackGroup::of(r.stage, r.waiting_for_ecoverse) == Some(group) }).count(),
        })
        .collect();
    let needs: Vec<&TrackRow> = rows.iter().filter(|r| row_needs_you(r)).collect();
    let limit = if query.limit == 0 { DEFAULT_PAGE } else { query.limit.min(MAX_PAGE) };
    let kept: Vec<&TrackRow> = rows.iter().filter(|r| query.keeps(r)).collect();
    TrackView {
        matching: kept.len(),
        offset: query.offset,
        rows: kept.into_iter().skip(query.offset).take(limit).cloned().collect(),
        waves: project.waves.iter().map(|w| WaveName { number: w.number, label: w.label.clone() }).collect(),
        customers,
        counts,
        needs_you_total: needs.len(),
        needs_you: need_kinds(&needs),
    }
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
        ecoverse_note: d.ecoverse_note.clone(),
        waiting_for_ecoverse: project.waiting_for_ecoverse(d),
        device_ufa: if project.dest_ecoverse == DestEcoverse::NotLinked { None } else { device_ufa(&d.events) },
        candidates: d.match_candidates.iter().map(|MatchCandidate { device_id, name }| DeviceCandidate { device_id: device_id.clone(), name: name.clone() }).collect(),
        actions: actions_for(d),
        events: d.events.iter().map(|Event { at, kind, detail }| DeviceEvent { at: at.clone(), kind: kind.clone(), detail: detail.clone() }).collect(),
    })
}

type Names = Arc<HashMap<String, String>>;

/// The names of a server, when they were read, and whether the read worked (a failed one is kept briefly so a server
/// that is down isn't asked again on every keystroke in a filter).
fn name_cache() -> &'static Mutex<HashMap<String, (Instant, Names, bool)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Names, bool)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// How long a failed read of the names is kept.
const NAMES_FAILED_FOR: Duration = Duration::from_secs(30);

pub(crate) fn forget_all_names() {
    name_cache().lock().unwrap().clear();
}

pub(crate) fn forget_names(server_id: &str) {
    name_cache().lock().unwrap().remove(server_id);
}

fn names_of(units: &[OrgUnit]) -> HashMap<String, String> {
    units.iter().map(|u| (u.org_unit_id.clone(), u.org_unit_name.clone())).collect()
}

impl Core {
    /// Customer and site names of a server, kept for a few minutes. When the server can't be read the page shows IDs.
    pub(crate) async fn unit_names(&self, server_id: &str) -> Names {
        if let Some((at, names, worked)) = name_cache().lock().unwrap().get(server_id)
            && at.elapsed() < if *worked { NAMES_FOR } else { NAMES_FAILED_FOR }
        {
            return names.clone();
        }
        let (names, worked) = match self.org_units_cached(server_id).await {
            Ok(units) => (Arc::new(names_of(&units)), true),
            Err(_) => (Arc::new(HashMap::new()), false),
        };
        name_cache().lock().unwrap().insert(server_id.to_string(), (Instant::now(), names.clone(), worked));
        names
    }

    /// The names the screens need: the ones kept on the devices, and only for what they lack a read of the servers (a
    /// project from before names were kept, or a unit that was created after planning).
    pub(crate) async fn names_for(&self, project: &Project) -> (Names, Names) {
        let (mut source, mut dest) = project.stored_names();
        let lacks_source = project.devices.iter().any(|d| d.source_customer_id.as_ref().is_some_and(|id| !source.contains_key(id)));
        let lacks_dest = project.devices.iter().any(|d| d.dest_org_unit_id.as_ref().is_some_and(|id| !dest.contains_key(id)));
        if lacks_source {
            for (id, name) in self.unit_names(&project.source_server_id).await.iter() {
                source.entry(id.clone()).or_insert_with(|| name.clone());
            }
        }
        if lacks_dest {
            for (id, name) in self.unit_names(&project.dest_server_id).await.iter() {
                dest.entry(id.clone()).or_insert_with(|| name.clone());
            }
        }
        (Arc::new(source), Arc::new(dest))
    }

    pub async fn migration_track_view(&self, id: &str, query: &TrackQuery) -> Result<TrackView, String> {
        let project = self.migration_load_shared(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let (source, dest) = self.names_for(&project).await;
        Ok(track_view(&project, &source, &dest, query))
    }

    pub async fn migration_device_detail(&self, id: &str, index: usize) -> Result<DeviceDetail, String> {
        let project = self.migration_load_shared(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
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

    /// Probes the healthy devices that are waiting for Ecoverse's data, one after another, saving after each so a stop
    /// part-way loses nothing, and says what their Ecoverse agents reported. Longest-waiting first, at most
    /// `PROBE_BATCH` at a time; the rest are counted in `not_reached`. A device that can't be reached is reported and
    /// the rest go on.
    pub async fn migration_probe_waiting(&self, id: &str, instance: &str) -> Result<ProbeReport, String> {
        use crate::migration::packager::Operation;
        let package = Package::embedded()?;
        let mut project = self.migration_open_for_write(id, instance).await?;
        let mut targets: Vec<usize> = project.devices.iter().enumerate().filter(|(_, d)| d.stage == Stage::Healthy && project.waiting_for_ecoverse(d)).map(|(i, _)| i).collect();
        targets.sort_by(|a, b| project.devices[*a].entered_stage_at.cmp(&project.devices[*b].entered_stage_at));
        let mut report = ProbeReport { not_reached: targets.len().saturating_sub(PROBE_BATCH), ..Default::default() };
        targets.truncate(PROBE_BATCH);
        let mut counts: Vec<UfaCount> = vec![];
        for index in targets {
            let name = project.devices[index].name.clone();
            let outcome = self.migration_run_action(&mut project, index, Operation::Status, &package, ACTION_WAIT).await;
            self.migration_save(&mut project).await?;
            match outcome {
                Ok(_) => {
                    report.probed += 1;
                    // Only the answer just recorded counts: an older Probe's state would be stale.
                    let just_now = project.devices[index].events.last().map(std::slice::from_ref).and_then(device_ufa);
                    match just_now {
                        Some(ufa) => match counts.iter_mut().find(|c| c.state == ufa.state) {
                            Some(c) => c.count += 1,
                            None => counts.push(UfaCount { state: ufa.state, count: 1 }),
                        },
                        None => report.no_record += 1,
                    }
                }
                Err(why) => report.failed.push(CleanupFailure { name, why }),
            }
        }
        counts.sort_by(|a, b| b.count.cmp(&a.count));
        report.states = counts;
        Ok(report)
    }

    /// One tracker pass on the saved project.
    pub async fn migration_poll_digest(&self, id: &str, instance: &str) -> Result<PollDigest, String> {
        let (_, report) = self.migration_poll_saved(id, instance, &self.migration_settings().thresholds()).await?;
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
        let view = track_view(&p, &names(&[("10", "Zeta"), ("11", "Acme")]), &names(&[("110", "Zeta")]), &TrackQuery::all());
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
    fn the_detail_carries_the_agents_state_from_the_last_probe_only_when_the_destination_is_linked_to_ecoverse() {
        let now = chrono::Utc::now();
        let status = r#"{"ufa":{"present":true,"running":true,"provisioned":true,"sync":"blocked"},"ufaSyncRestarts":1}"#;
        for (link, expected) in [(DestEcoverse::SameTenant, true), (DestEcoverse::NotLinked, false)] {
            let mut p = Project::new("p".into(), "x", "a", "b", link, now);
            p.add_planned(&[planned("1", "10", "110")], now);
            assert!(device_detail(&p, 0, &HashMap::new(), &HashMap::new()).unwrap().device_ufa.is_none(), "not probed yet");
            p.devices[0].record(now, "status", &format!("Status via N-central: Last log: awaiting-ufa\nstatus.json: {status}"));
            let ufa = device_detail(&p, 0, &HashMap::new(), &HashMap::new()).unwrap().device_ufa;
            assert_eq!(ufa.is_some(), expected);
            if let Some(ufa) = ufa {
                assert_eq!((ufa.state, ufa.restarts), (crate::migration::deviceufa::UfaState::Blocked, 1));
            }
        }
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

    fn with_stages(stages: &[Stage]) -> Project {
        let now = chrono::Utc::now();
        let mut p = Project::new("p".into(), "x", "a", "b", DestEcoverse::NotLinked, now);
        let planned: Vec<PlannedDevice> = (0..stages.len()).map(|i| planned(&i.to_string(), if i % 2 == 0 { "10" } else { "11" }, "110")).collect();
        p.add_planned(&planned, now);
        for (d, stage) in p.devices.iter_mut().zip(stages) {
            d.stage = *stage;
        }
        p
    }

    #[test]
    fn every_stage_but_excluded_is_in_one_group() {
        let all = [
            Stage::Planned, Stage::Unreachable, Stage::Launched, Stage::NotPickedUp, Stage::LaunchFailed, Stage::Staged, Stage::StuckStaged, Stage::AgentRemoved, Stage::Lost, Stage::Registered, Stage::WrongPlace, Stage::Healthy,
            Stage::CleanedUp, Stage::Retired, Stage::Resolved,
        ];
        for stage in all {
            assert!(TrackGroup::of(stage, false).is_some(), "{stage:?}");
        }
        assert_eq!(TrackGroup::of(Stage::Excluded, false), None);
        assert_eq!((TrackGroup::of(Stage::Healthy, true), TrackGroup::of(Stage::Healthy, false), TrackGroup::of(Stage::Lost, true)), (Some(TrackGroup::Waiting), Some(TrackGroup::Healthy), Some(TrackGroup::NeedsYou)));
    }

    #[test]
    fn the_strip_counts_the_whole_project_and_a_query_filters_and_pages_the_table() {
        let mut stages = vec![Stage::Healthy; 5];
        stages.extend([Stage::Lost, Stage::NotPickedUp, Stage::Staged, Stage::Excluded]);
        let mut p = with_stages(&stages);
        p.dest_ecoverse = DestEcoverse::SameTenant; // linked: a healthy device that Ecoverse hasn't read is waiting
        p.devices[0].ecoverse = Some(EcoState::Reporting);
        let view = track_view(&p, &HashMap::new(), &HashMap::new(), &TrackQuery { limit: 2, ..TrackQuery::all() });
        let count = |g: TrackGroup| view.counts.iter().find(|c| c.group == g).unwrap().count;
        assert_eq!((count(TrackGroup::Healthy), count(TrackGroup::Waiting), count(TrackGroup::NeedsYou), count(TrackGroup::Staged)), (1, 4, 2, 1));
        assert_eq!((view.matching, view.rows.len()), (9, 2), "the excluded device is in the table but in no group; the page is two");
        assert_eq!(view.needs_you_total, 2);
        let mut kinds: Vec<_> = view.needs_you.iter().map(|k| (k.stage, k.count)).collect();
        kinds.sort();
        let mut expected = vec![(Stage::Lost, 1), (Stage::NotPickedUp, 1)];
        expected.sort();
        assert_eq!(kinds, expected);

        let lost = track_view(&p, &HashMap::new(), &HashMap::new(), &TrackQuery { group: Some(TrackGroup::NeedsYou), ..TrackQuery::all() });
        assert_eq!(lost.rows.iter().map(|r| r.stage).collect::<Vec<_>>(), [Stage::Lost, Stage::NotPickedUp]);
        let second_page = track_view(&p, &HashMap::new(), &HashMap::new(), &TrackQuery { offset: 2, limit: 2, ..TrackQuery::all() });
        assert_eq!(second_page.rows.iter().map(|r| r.index).collect::<Vec<_>>(), [2, 3]);
    }

    #[test]
    fn a_query_narrows_by_wave_customer_and_text_and_a_device_with_a_match_to_choose_needs_you() {
        let mut p = with_stages(&[Stage::Planned, Stage::Planned, Stage::Planned]);
        p.auto_wave("one", Some(2), chrono::Utc::now()).unwrap();
        p.devices.iter_mut().for_each(|d| d.stage = Stage::Healthy);
        p.devices[2].match_candidates = vec![MatchCandidate { device_id: "9".into(), name: "PC-A".into() }];
        let names = names(&[("10", "Zeta"), ("11", "Acme")]);
        let q = |query: TrackQuery| track_view(&p, &names, &HashMap::new(), &query).rows.iter().map(|r| r.index).collect::<Vec<_>>();
        assert_eq!(q(TrackQuery { wave: Some(1), ..TrackQuery::all() }), [0, 1]);
        assert_eq!(q(TrackQuery { customer: Some("Acme".into()), ..TrackQuery::all() }), [1]);
        assert_eq!(q(TrackQuery { text: "PC-2".into(), ..TrackQuery::all() }), [2]);
        assert_eq!(q(TrackQuery { group: Some(TrackGroup::NeedsYou), ..TrackQuery::all() }), [2]);
        let view = track_view(&p, &names, &HashMap::new(), &TrackQuery::all());
        assert_eq!(view.needs_you_total, 1);
    }

    #[test]
    fn needs_you_is_grouped_by_reason_commonest_first_with_a_few_examples_each() {
        let mut stages = vec![Stage::Lost; 5];
        stages.extend([Stage::NotPickedUp, Stage::NotPickedUp, Stage::Healthy]);
        let mut p = with_stages(&stages);
        p.devices[7].match_candidates = vec![MatchCandidate { device_id: "9".into(), name: "PC-A".into() }];
        let view = track_view(&p, &HashMap::new(), &HashMap::new(), &TrackQuery::all());
        let kinds: Vec<_> = view.needs_you.iter().map(|k| (k.stage, k.needs_match, k.count, k.examples.len())).collect();
        assert_eq!(kinds, [(Stage::Lost, false, 5, 3), (Stage::NotPickedUp, false, 2, 2), (Stage::Healthy, true, 1, 1)]);
        assert_eq!(view.needs_you_total, 8);
    }

    #[tokio::test]
    async fn names_kept_on_the_devices_need_no_read_of_the_servers_and_missing_ones_are_read_once() {
        use crate::ncentral::{
            model::OrgUnitType,
            rest::testing::{Fake, auth_ok, ok},
        };
        let fake = Fake::new(|req, _| {
            if req.url.contains("authenticate") {
                return auth_ok(3600);
            }
            let units = [(10, "Acme", "CUSTOMER"), (11, "Zeta", "CUSTOMER"), (110, "Acme HQ", "SITE")];
            let items: Vec<_> = units.iter().map(|(id, name, kind)| serde_json::json!({ "orgUnitId": id, "orgUnitName": name, "orgUnitType": kind, "parentId": null })).collect();
            ok(serde_json::json!({ "data": items, "totalPages": 1, "totalItems": 3, "page": 1, "pageSize": 1000 }))
        });
        let core = Core::for_tests("names-kept");
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        core.set_rest_transport(fake.clone());
        let mut p = with_stages(&[Stage::Healthy, Stage::Healthy]);
        p.source_server_id = src.id;
        p.dest_server_id = dst.id;
        let reads = || fake.paths().iter().filter(|p| p.starts_with("GET api/org-units")).count();

        let (source, dest) = core.names_for(&p).await;
        assert_eq!((reads(), source.get("10").cloned(), dest.get("110").cloned()), (2, Some("Acme".into()), Some("Acme HQ".into())), "a project with no names kept reads both servers");
        let units = |id: &str, name: &str| OrgUnit { org_unit_id: id.into(), org_unit_name: name.into(), org_unit_type: OrgUnitType::Customer, parent_id: None, external_id: None };
        p.fill_names(&[units("10", "Acme"), units("11", "Zeta")], &[units("110", "Acme HQ")]);
        let before = reads();
        let (source, dest) = core.names_for(&p).await;
        assert_eq!(reads(), before, "with the names kept, nothing is read");
        assert_eq!((source.get("11").cloned(), dest.get("110").cloned()), (Some("Zeta".into()), Some("Acme HQ".into())));
    }
}
