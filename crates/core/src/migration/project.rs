//! A migration project: one move of devices from a source server to a destination
//! (the same server for a reinstall). It holds the mapping decisions, the waves and
//! every device's stage and history, and is saved in its own encrypted store.
//! Stages and their allowed moves are those in docs/migration/06-tracking.md.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ncentral::model::DeviceIdentity;

use super::{
    mapping::Override,
    planner::{DeliveryPath, PlannedDevice},
};

/// What the destination's Ecoverse shows for a device that has moved. "Healthy" in N-central is not the same as
/// working in Ecoverse: a device is registered there long before its agent has sent any asset data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "EcoState.ts"))]
#[serde(rename_all = "camelCase")]
pub enum EcoState {
    /// The destination isn't linked to Ecoverse, so there is nothing to wait for.
    NotLinked,
    /// No asset in the destination's Ecoverse tenant yet.
    NotSeen,
    /// An asset exists (it shows in Ecoverse) but no asset data has arrived: scripts can't run on it yet.
    WaitingForData,
    /// Asset data is arriving.
    Reporting,
}

/// A device's place in the migration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "Stage.ts"))]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Planned,
    /// No path can reach it now.
    Unreachable,
    /// The API accepted the launch; there is a task ID.
    Launched,
    NotPickedUp,
    LaunchFailed,
    /// The launcher ran and registered the scheduled task.
    Staged,
    StuckStaged,
    /// The old agent went quiet: cleanup and install are running.
    AgentRemoved,
    Lost,
    /// A matching device exists on the destination.
    Registered,
    WrongPlace,
    Healthy,
    CleanedUp,
    Retired,
    /// Taken out of the project by the operator.
    Excluded,
    /// Fixed outside N-voke and marked resolved by the operator.
    Resolved,
}

impl Stage {
    /// The stages a device may move to from here, as in the state diagram. The tracker
    /// may pass through several in one step (recording each) when it catches up.
    pub fn next(self) -> &'static [Stage] {
        use Stage::*;
        match self {
            Planned => &[Launched, LaunchFailed, Unreachable, Excluded],
            Unreachable => &[Planned, Resolved, Excluded],
            Launched => &[NotPickedUp, Staged, LaunchFailed],
            NotPickedUp => &[Launched, Planned, Resolved],
            LaunchFailed => &[Planned, Resolved],
            Staged => &[AgentRemoved, StuckStaged],
            StuckStaged => &[AgentRemoved, Planned, Resolved],
            AgentRemoved => &[Registered, Lost],
            Lost => &[Registered, Resolved],
            Registered => &[Healthy, WrongPlace],
            WrongPlace => &[Resolved],
            Healthy => &[CleanedUp],
            CleanedUp => &[Retired],
            Excluded => &[Planned],
            Retired | Resolved => &[],
        }
    }

    pub fn can_move_to(self, to: Stage) -> bool {
        self.next().contains(&to)
    }

    /// Nothing more happens to the device.
    pub fn is_final(self) -> bool {
        matches!(self, Stage::Retired | Stage::Excluded | Stage::Resolved)
    }

    /// Needs the operator's attention.
    pub fn needs_attention(self) -> bool {
        matches!(self, Stage::Unreachable | Stage::NotPickedUp | Stage::LaunchFailed | Stage::StuckStaged | Stage::Lost | Stage::WrongPlace)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub at: String,
    /// `stage`, `launch`, `output`, `status`, `note`, ...
    pub kind: String,
    pub detail: String,
}

/// The longest detail kept per event: collected output is capped like run output.
pub const MAX_EVENT_DETAIL: usize = 64 * 1024;
/// The most events kept per device; the oldest go first.
pub const MAX_EVENTS: usize = 500;

/// A destination device that could be this one, kept so the operator can choose between them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchCandidate {
    pub device_id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDevice {
    /// The N-central device ID on the source server.
    pub source_device_id: String,
    pub name: String,
    pub stage: Stage,
    pub wave: Option<u32>,
    pub path: Option<DeliveryPath>,
    pub source_customer_id: Option<String>,
    pub source_site_id: Option<String>,
    pub dest_org_unit_id: Option<String>,
    /// Ecoverse asset on the source side, when launching through Ecoverse.
    pub asset_id: Option<String>,
    /// The device record on the destination, once it registers.
    pub dest_device_id: Option<String>,
    /// More than one destination device fits: the operator chooses (empty when there's no such question).
    #[serde(default)]
    pub match_candidates: Vec<MatchCandidate>,
    pub task_id: Option<String>,
    /// What identifies the device on the source, read before it is moved, to recognise it on the destination.
    #[serde(default)]
    pub identity: Option<DeviceIdentity>,
    pub attempts: u32,
    pub last_error: Option<String>,
    /// A launch was sent (or was about to be) and what came of it isn't recorded: it may be running. The device
    /// can't be launched again until the operator has checked and said it didn't go.
    #[serde(default)]
    pub launch_unsure: bool,
    /// What the destination's Ecoverse showed the last time it was read for this device. None until it has been.
    #[serde(default)]
    pub ecoverse: Option<EcoState>,
    /// The source customer's and the destination unit's names when the device was planned, so the screens that show them
    /// (Track, waves) need no read of the servers. A project from before this was kept has none and looks them up.
    #[serde(default)]
    pub customer_name: Option<String>,
    #[serde(default)]
    pub dest_name: Option<String>,
    /// Why Ecoverse doesn't show the device, when it doesn't: which tenant was looked in and what was found there.
    #[serde(default)]
    pub ecoverse_note: Option<String>,
    /// When `ecoverse` was last read, so a device Ecoverse is already receiving data from isn't looked at every pass.
    #[serde(default)]
    pub ecoverse_read_at: Option<String>,
    pub entered_stage_at: String,
    pub events: Vec<Event>,
}

impl ProjectDevice {
    pub fn from_plan(planned: &PlannedDevice, now: DateTime<Utc>) -> Self {
        let stage = if planned.excluded { Stage::Excluded } else { Stage::Planned };
        let at = stamp(now);
        Self {
            source_device_id: planned.device_id.clone(),
            name: planned.name.clone(),
            stage,
            wave: None,
            path: planned.path,
            source_customer_id: planned.source_customer_id.clone(),
            source_site_id: planned.source_site_id.clone(),
            dest_org_unit_id: planned.dest_org_unit_id.clone(),
            asset_id: planned.asset_id.clone(),
            dest_device_id: None,
            match_candidates: vec![],
            task_id: None,
            identity: None,
            attempts: 0,
            last_error: None,
            launch_unsure: false,
            ecoverse: None,
            customer_name: None,
            dest_name: None,
            ecoverse_note: None,
            ecoverse_read_at: None,
            entered_stage_at: at.clone(),
            events: vec![Event { at, kind: "stage".into(), detail: format!("{stage:?}") }],
        }
    }

    /// Appends to the timeline, shortening an oversized detail and dropping the oldest events past the cap.
    pub fn record(&mut self, now: DateTime<Utc>, kind: &str, detail: &str) {
        let detail: String = crate::ncentral::rest::scrub_secrets(detail).chars().take(MAX_EVENT_DETAIL).collect();
        self.events.push(Event { at: stamp(now), kind: kind.into(), detail });
        if self.events.len() > MAX_EVENTS {
            self.events.remove(0);
        }
    }

    /// Moves to `to` if the diagram allows it, recording why. A device already there is left alone.
    pub fn move_to(&mut self, to: Stage, now: DateTime<Utc>, why: &str) -> Result<(), String> {
        if self.stage == to {
            return Ok(());
        }
        if !self.stage.can_move_to(to) {
            return Err(format!("{} can't go from {:?} to {:?}.", self.name, self.stage, to));
        }
        self.stage = to;
        self.entered_stage_at = stamp(now);
        self.record(now, "stage", &if why.is_empty() { format!("{to:?}") } else { format!("{to:?}: {why}") });
        Ok(())
    }
}

/// How the destination relates to Ecoverse, which decides how devices are tracked after the move.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DestEcoverse.ts"))]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum DestEcoverse {
    /// The usual case for a new server: devices leave Ecoverse until it's linked.
    NotLinked,
    SameTenant,
    OtherTenant { tenant_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "CleanMode.ts"))]
#[serde(rename_all = "camelCase")]
pub enum CleanMode {
    /// Remove every N-central component before installing (the default).
    Full,
    /// Install over the top.
    None,
}

/// How devices are reached to start the migration. Ecoverse is the usual way in; REST is the fallback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "Delivery.ts"))]
#[serde(rename_all = "camelCase")]
pub enum Delivery {
    /// Use Ecoverse for the devices it reaches, and N-central REST for the rest.
    #[default]
    EcoverseFirst,
    /// Everything goes over N-central REST; Ecoverse isn't read or used.
    RestOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ProjectOptions.ts"))]
#[serde(rename_all = "camelCase")]
pub struct ProjectOptions {
    pub review_days: u32,
    pub clean_mode: CleanMode,
    pub task_repeat_minutes: u32,
    pub task_expiry_days: u32,
    pub wave_size: u32,
    pub retire_source_records: bool,
    #[serde(default)]
    pub delivery: Delivery,
}

impl Default for ProjectOptions {
    fn default() -> Self {
        Self { review_days: 7, clean_mode: CleanMode::Full, task_repeat_minutes: 15, task_expiry_days: 14, wave_size: 50, retire_source_records: false, delivery: Delivery::EcoverseFirst }
    }
}

/// An Ecoverse organization the endpoint script is published in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ScriptOrg.ts"))]
#[serde(rename_all = "camelCase")]
pub struct ScriptOrg {
    pub id: String,
    pub name: String,
}

/// The endpoint script as uploaded by hand to a server's repository (REST path).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestScript {
    pub item_id: String,
    /// The version the operator uploaded, to spot a stale copy.
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Wave {
    pub number: u32,
    pub label: String,
    pub created_at: String,
    pub launched_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub source_server_id: String,
    /// The same as the source for a reinstall.
    pub dest_server_id: String,
    pub dest_ecoverse: DestEcoverse,
    pub options: ProjectOptions,
    /// The operator's mapping decisions, by source org unit ID. The automatic mapping is recomputed.
    pub mapping_overrides: HashMap<String, Override>,
    /// The operator's choice of delivery path for a device, by source device ID, over the suggested one.
    #[serde(default)]
    pub path_overrides: HashMap<String, DeliveryPath>,
    /// Devices the operator took out of the project, by source device ID, so planning leaves them out too.
    #[serde(default)]
    pub left_out: BTreeSet<String>,
    /// Uploaded endpoint script per server ID.
    pub rest_scripts: HashMap<String, RestScript>,
    /// Ecoverse organizations to search for the source's assets, and the destination's when it is linked.
    #[serde(default)]
    pub source_ecoverse_org_ids: Vec<String>,
    #[serde(default)]
    pub dest_ecoverse_org_ids: Vec<String>,
    /// The endpoint script as published in the source's Ecoverse tenant.
    #[serde(default)]
    pub ecoverse_script_id: Option<String>,
    /// The version of the script that was published there, so a launch can refuse an out-of-date copy.
    #[serde(default)]
    pub ecoverse_script_version: Option<String>,
    /// Where the script was published, so the page can say and publishing again goes to the same place.
    #[serde(default)]
    pub ecoverse_script_org: Option<ScriptOrg>,
    /// The same script published in the destination's own Ecoverse tenant, for a destination linked to another
    /// tenant: it is what reaches a device after it has moved there.
    #[serde(default)]
    pub dest_ecoverse_script_id: Option<String>,
    #[serde(default)]
    pub dest_ecoverse_script_version: Option<String>,
    #[serde(default)]
    pub dest_ecoverse_script_org: Option<ScriptOrg>,
    /// The device IDs on each destination org unit before launching, so a device that appears later is known to be new.
    #[serde(default)]
    pub dest_baseline: HashMap<String, Vec<String>>,
    pub devices: Vec<ProjectDevice>,
    pub waves: Vec<Wave>,
    /// Counts saves, so a save from a copy that is out of date is refused rather than overwriting newer work.
    #[serde(default)]
    pub revision: u64,
}

pub fn stamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

impl Project {
    pub fn new(id: String, name: &str, source_server_id: &str, dest_server_id: &str, dest_ecoverse: DestEcoverse, now: DateTime<Utc>) -> Self {
        Self {
            id,
            name: name.trim().to_string(),
            created_at: stamp(now),
            updated_at: stamp(now),
            source_server_id: source_server_id.into(),
            dest_server_id: dest_server_id.into(),
            dest_ecoverse,
            options: ProjectOptions::default(),
            mapping_overrides: HashMap::new(),
            path_overrides: HashMap::new(),
            left_out: BTreeSet::new(),
            rest_scripts: HashMap::new(),
            source_ecoverse_org_ids: vec![],
            dest_ecoverse_org_ids: vec![],
            ecoverse_script_id: None,
            ecoverse_script_version: None,
            ecoverse_script_org: None,
            dest_ecoverse_script_id: None,
            dest_ecoverse_script_version: None,
            dest_ecoverse_script_org: None,
            dest_baseline: HashMap::new(),
            devices: vec![],
            waves: vec![],
            revision: 0,
        }
    }

    /// A reinstall on the same server rather than a move.
    /// The server whose Ecoverse key reads the destination's Ecoverse assets: the source's when the destination is in
    /// the same tenant, otherwise the destination's own.
    pub fn dest_ecoverse_server(&self) -> &str {
        match self.dest_ecoverse {
            DestEcoverse::SameTenant => &self.source_server_id,
            _ => &self.dest_server_id,
        }
    }

    pub fn is_reinstall(&self) -> bool {
        self.source_server_id == self.dest_server_id
    }

    pub fn device_mut(&mut self, source_device_id: &str) -> Option<&mut ProjectDevice> {
        self.devices.iter_mut().find(|d| d.source_device_id == source_device_id)
    }

    /// Adds devices from a plan. Devices already in the project are left alone, so planning again is safe.
    pub fn add_planned(&mut self, planned: &[PlannedDevice], now: DateTime<Utc>) -> usize {
        let mut added = 0;
        for device in planned {
            if self.devices.iter().all(|d| d.source_device_id != device.device_id) {
                self.devices.push(ProjectDevice::from_plan(device, now));
                added += 1;
            }
        }
        self.updated_at = stamp(now);
        added
    }

    /// Creates the next wave and puts the given planned devices in it. Only devices that
    /// are `Planned` and not already in a wave can join; the rest are reported.
    pub fn create_wave(&mut self, label: &str, device_ids: &[String], now: DateTime<Utc>) -> Result<u32, String> {
        let eligible = |d: &ProjectDevice| d.stage == Stage::Planned && d.wave.is_none();
        if let Some(bad) = device_ids.iter().find(|id| !self.devices.iter().any(|d| &d.source_device_id == *id && eligible(d))) {
            return Err(format!("Device {bad} isn't waiting to be planned into a wave."));
        }
        let number = self.waves.iter().map(|w| w.number).max().unwrap_or(0) + 1;
        for device in self.devices.iter_mut().filter(|d| device_ids.contains(&d.source_device_id)) {
            device.wave = Some(number);
        }
        let label = if label.trim().is_empty() { format!("Wave {number}") } else { label.trim().to_string() };
        self.waves.push(Wave { number, label, created_at: stamp(now), launched_at: None });
        self.updated_at = stamp(now);
        Ok(number)
    }

    /// Creates the next wave from devices still waiting for one, in name order, up to `max` (all if None).
    pub fn auto_wave(&mut self, label: &str, max: Option<usize>, now: DateTime<Utc>) -> Result<u32, String> {
        let mut waiting: Vec<&ProjectDevice> = self.devices.iter().filter(|d| d.stage == Stage::Planned && d.wave.is_none()).collect();
        waiting.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.source_device_id.cmp(&b.source_device_id)));
        let ids: Vec<String> = waiting.into_iter().take(max.unwrap_or(usize::MAX)).map(|d| d.source_device_id.clone()).collect();
        if ids.is_empty() {
            return Err("No device is waiting to be put in a wave.".into());
        }
        self.create_wave(label, &ids, now)
    }

    /// Devices per stage.
    pub fn stage_counts(&self) -> BTreeMap<Stage, usize> {
        let mut counts = BTreeMap::new();
        for device in &self.devices {
            *counts.entry(device.stage).or_default() += 1;
        }
        counts
    }

    /// Devices that are healthy in N-central but whose Ecoverse asset hasn't sent data (or hasn't been read yet), when the
    /// destination is linked to Ecoverse. They are registered, but not fully working.
    pub fn ecoverse_waiting(&self) -> usize {
        self.devices.iter().filter(|d| self.waiting_for_ecoverse(d)).count()
    }

    /// After the delivery changes: planned devices the operator hasn't chosen a path for take the one the delivery
    /// gives (Ecoverse when delivery allows it and the device has an Ecoverse asset, otherwise N-central).
    pub fn repath_planned(&mut self, now: DateTime<Utc>) {
        let rest_only = self.options.delivery == Delivery::RestOnly;
        let chosen = self.path_overrides.clone();
        for device in self.devices.iter_mut().filter(|d| d.stage == Stage::Planned && !chosen.contains_key(&d.source_device_id)) {
            let path = if !rest_only && device.asset_id.is_some() { DeliveryPath::Ecoverse } else { DeliveryPath::Rest };
            if device.path != Some(path) {
                device.path = Some(path);
                device.record(now, "path", &format!("Delivery changed: now planned through {}", if path == DeliveryPath::Ecoverse { "Ecoverse" } else { "N-central" }));
            }
        }
        self.updated_at = stamp(now);
    }

    /// Remembers the names of the customer and destination unit each device has, from what was just read.
    pub fn fill_names(&mut self, source: &[crate::ncentral::model::OrgUnit], dest: &[crate::ncentral::model::OrgUnit]) {
        let find = |units: &[crate::ncentral::model::OrgUnit], id: &str| units.iter().find(|u| u.org_unit_id == id).map(|u| u.org_unit_name.clone());
        for d in &mut self.devices {
            if d.customer_name.is_none() {
                d.customer_name = d.source_customer_id.as_deref().and_then(|id| find(source, id));
            }
            if d.dest_name.is_none() {
                d.dest_name = d.dest_org_unit_id.as_deref().and_then(|id| find(dest, id));
            }
        }
    }

    /// The names kept on the devices, by unit ID: source customers and destination units.
    pub fn stored_names(&self) -> (HashMap<String, String>, HashMap<String, String>) {
        let mut source = HashMap::new();
        let mut dest = HashMap::new();
        for d in &self.devices {
            if let (Some(id), Some(name)) = (&d.source_customer_id, &d.customer_name) {
                source.entry(id.clone()).or_insert_with(|| name.clone());
            }
            if let (Some(id), Some(name)) = (&d.dest_org_unit_id, &d.dest_name) {
                dest.entry(id.clone()).or_insert_with(|| name.clone());
            }
        }
        (source, dest)
    }

    /// A wave has been launched, so devices have been sent the migration and the delivery can no longer change.
    pub fn has_launched(&self) -> bool {
        self.waves.iter().any(|w| w.launched_at.is_some())
    }

    /// Healthy in N-central, but its Ecoverse asset hasn't sent data (or hasn't been read yet) and the destination is
    /// linked to Ecoverse: registered there, not fully working. It stays in the Healthy stage; this is how it is told apart.
    pub fn waiting_for_ecoverse(&self, device: &ProjectDevice) -> bool {
        device.stage == Stage::Healthy && self.dest_ecoverse != DestEcoverse::NotLinked && device.ecoverse != Some(EcoState::Reporting)
    }

    pub fn attention_count(&self) -> usize {
        self.devices.iter().filter(|d| d.stage.needs_attention() || !d.match_candidates.is_empty()).count()
    }

    /// Every device is finished with (and there is at least one). A device that has cleaned up is the end of the road
    /// unless the project retires the old source records, which is the one more step after it.
    pub fn is_closed(&self) -> bool {
        !self.devices.is_empty() && self.devices.iter().all(|d| d.stage.is_final() || (d.stage == Stage::CleanedUp && !self.options.retire_source_records))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-10-20T12:00:00Z".parse().unwrap()
    }

    fn planned(id: &str, excluded: bool) -> PlannedDevice {
        PlannedDevice {
            device_id: id.into(),
            name: format!("PC-{id}"),
            source_customer_id: Some("10".into()),
            source_site_id: None,
            dest_org_unit_id: Some("110".into()),
            path: Some(DeliveryPath::Ecoverse),
            asset_id: Some("asset".into()),
            blockers: vec![],
            warnings: vec![],
            excluded,
        }
    }

    fn project() -> Project {
        Project::new("mig-1".into(), "  Acme move ", "src", "dst", DestEcoverse::NotLinked, now())
    }

    #[test]
    fn the_happy_path_is_allowed_end_to_end() {
        use Stage::*;
        let path = [Planned, Launched, Staged, AgentRemoved, Registered, Healthy, CleanedUp, Retired];
        for pair in path.windows(2) {
            assert!(pair[0].can_move_to(pair[1]), "{:?} -> {:?}", pair[0], pair[1]);
        }
        assert!(Retired.is_final() && Retired.next().is_empty());
    }

    #[test]
    fn stalls_and_failures_can_recover_or_be_resolved_but_not_skip_ahead() {
        use Stage::*;
        assert!(NotPickedUp.can_move_to(Launched) && NotPickedUp.can_move_to(Planned), "picked up late, or cancel and retry");
        assert!(LaunchFailed.can_move_to(Planned));
        assert!(StuckStaged.can_move_to(AgentRemoved) && StuckStaged.can_move_to(Planned));
        assert!(Lost.can_move_to(Registered), "a lost device can still turn up");
        assert!(WrongPlace.can_move_to(Resolved));
        assert!(Planned.can_move_to(LaunchFailed), "a launch the API refuses never got a task");
        assert!(!Planned.can_move_to(Healthy), "no skipping the launch");
        assert!(!Healthy.can_move_to(Planned), "healthy devices don't go back");
        assert!(!Registered.can_move_to(CleanedUp), "healthy comes first");
    }

    #[test]
    fn every_stage_that_isnt_final_leads_somewhere_and_attention_stages_are_not_final() {
        use Stage::*;
        for stage in [Planned, Unreachable, Launched, NotPickedUp, LaunchFailed, Staged, StuckStaged, AgentRemoved, Lost, Registered, WrongPlace, Healthy, CleanedUp, Excluded, Retired, Resolved] {
            assert_eq!(stage.is_final() && stage != Excluded, stage.next().is_empty(), "{stage:?}");
            assert!(!(stage.is_final() && stage.needs_attention()), "{stage:?}");
        }
    }

    #[test]
    fn planned_devices_join_once_and_excluded_ones_start_excluded() {
        let mut p = project();
        assert_eq!(p.name, "Acme move");
        assert_eq!(p.add_planned(&[planned("1", false), planned("2", true)], now()), 2);
        assert_eq!(p.add_planned(&[planned("1", false), planned("3", false)], now()), 1, "device 1 is already in");
        assert_eq!((p.devices[0].stage, p.devices[1].stage), (Stage::Planned, Stage::Excluded));
        assert_eq!(p.devices[0].events[0].detail, "Planned");
    }

    #[test]
    fn a_move_is_recorded_and_an_illegal_one_is_refused() {
        let mut p = project();
        p.add_planned(&[planned("1", false)], now());
        let device = p.device_mut("1").unwrap();
        let later: DateTime<Utc> = "2026-10-20T12:05:00Z".parse().unwrap();
        device.move_to(Stage::Launched, later, "Ecoverse task t1").unwrap();
        assert_eq!((device.stage, device.entered_stage_at.as_str()), (Stage::Launched, "2026-10-20T12:05:00Z"));
        assert_eq!(device.events.last().unwrap().detail, "Launched: Ecoverse task t1");
        assert!(device.move_to(Stage::Healthy, later, "").unwrap_err().contains("can't go from Launched to Healthy"));
        device.move_to(Stage::Launched, later, "").unwrap();
        assert_eq!(device.events.len(), 2, "staying put adds nothing");
    }

    #[test]
    fn the_timeline_is_capped_in_length_and_count() {
        let mut p = project();
        p.add_planned(&[planned("1", false)], now());
        let device = p.device_mut("1").unwrap();
        device.record(now(), "output", &"x".repeat(MAX_EVENT_DETAIL + 100));
        assert_eq!(device.events.last().unwrap().detail.len(), MAX_EVENT_DETAIL);
        for i in 0..MAX_EVENTS + 10 {
            device.record(now(), "note", &i.to_string());
        }
        assert_eq!(device.events.len(), MAX_EVENTS);
        assert_eq!(device.events.last().unwrap().detail, (MAX_EVENTS + 9).to_string());
    }

    #[test]
    fn waves_take_planned_devices_once_and_number_themselves() {
        let mut p = project();
        p.add_planned(&[planned("1", false), planned("2", false), planned("3", false)], now());
        assert_eq!(p.create_wave("Pilot", &["1".into()], now()), Ok(1));
        assert_eq!(p.create_wave("", &["2".into(), "3".into()], now()), Ok(2));
        assert_eq!(p.waves[1].label, "Wave 2");
        assert!(p.create_wave("again", &["1".into()], now()).is_err(), "already in a wave");
        assert!(p.create_wave("ghost", &["nope".into()], now()).is_err());
        assert_eq!(p.devices.iter().map(|d| d.wave).collect::<Vec<_>>(), [Some(1), Some(2), Some(2)]);
        assert_eq!(p.waves.len(), 2, "a refused wave isn't created");
    }

    #[test]
    fn an_automatic_wave_takes_the_first_waiting_devices_by_name() {
        let mut p = project();
        p.add_planned(&[planned("3", false), planned("1", false), planned("2", false)], now());
        assert_eq!(p.auto_wave("Pilot", Some(2), now()), Ok(1));
        assert_eq!(p.devices.iter().map(|d| d.wave).collect::<Vec<_>>(), [None, Some(1), Some(1)], "PC-1 and PC-2, not PC-3");
        assert_eq!(p.auto_wave("", None, now()), Ok(2));
        assert!(p.auto_wave("", None, now()).unwrap_err().contains("No device is waiting"));
    }

    #[test]
    fn counts_attention_and_closing_follow_the_stages() {
        let mut p = project();
        assert!(!p.is_closed(), "an empty project isn't finished");
        p.add_planned(&[planned("1", false), planned("2", false), planned("3", true)], now());
        p.device_mut("1").unwrap().stage = Stage::Lost;
        assert_eq!(p.attention_count(), 1);
        assert_eq!(p.stage_counts(), BTreeMap::from([(Stage::Planned, 1), (Stage::Lost, 1), (Stage::Excluded, 1)]));
        p.device_mut("1").unwrap().stage = Stage::Resolved;
        p.device_mut("2").unwrap().stage = Stage::Retired;
        assert!(p.is_closed());
    }

    #[test]
    fn a_project_round_trips_through_json_and_never_holds_a_secret() {
        let mut p = project();
        p.dest_ecoverse = DestEcoverse::OtherTenant { tenant_id: "t-9".into() };
        p.mapping_overrides.insert("10".into(), Override::MapTo("110".into()));
        p.mapping_overrides.insert("20".into(), Override::Exclude);
        p.rest_scripts.insert("src".into(), RestScript { item_id: "77".into(), version: "0.1.0+abc".into() });
        p.add_planned(&[planned("1", false)], now());
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Project>(&json).unwrap(), p);
        assert!(json.contains("\"kind\":\"otherTenant\"") || json.contains("OtherTenant"), "{json}");
        assert!(!json.to_lowercase().contains("registrationtoken"), "tokens are read again at launch, not stored in the project");
    }

    #[test]
    fn a_same_server_project_is_a_reinstall() {
        assert!(!project().is_reinstall());
        assert!(Project::new("m".into(), "r", "a", "a", DestEcoverse::SameTenant, now()).is_reinstall());
    }

    #[test]
    fn a_project_closes_when_every_device_has_cleaned_up_unless_it_also_retires_the_source_records() {
        let now = chrono::Utc::now();
        let planned = |id: &str| PlannedDevice {
            device_id: id.into(),
            name: format!("PC-{id}"),
            source_customer_id: Some("1".into()),
            source_site_id: None,
            dest_org_unit_id: Some("2".into()),
            path: Some(DeliveryPath::Rest),
            asset_id: None,
            blockers: vec![],
            warnings: vec![],
            excluded: false,
        };
        let mut p = Project::new("p".into(), "x", "a", "b", DestEcoverse::NotLinked, now);
        p.add_planned(&[planned("1"), planned("2")], now);
        assert!(!p.is_closed());
        p.devices[0].stage = Stage::CleanedUp;
        p.devices[1].stage = Stage::Healthy;
        assert!(!p.is_closed(), "one is still in its review period");
        p.devices[1].stage = Stage::Resolved;
        assert!(p.is_closed(), "cleaned up and resolved is everything done");
        p.options.retire_source_records = true;
        assert!(!p.is_closed(), "retiring the old records is one more step");
        p.devices[0].stage = Stage::Retired;
        assert!(p.is_closed());
        p.devices.clear();
        assert!(!p.is_closed(), "an empty project isn't finished");
    }
}
