//! Turning selected devices into a plan: where each one goes, how it will be
//! reached, and what stops it. Pure logic over data the caller has already
//! read, so it's tested without a network. Rules are those of
//! docs/migration/04-delivery-paths.md.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::mapping::{MappingEntry, MappingStatus, destination_for};
use crate::ncentral::model::{Device, TokenStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DeliveryPath.ts"))]
#[serde(rename_all = "camelCase")]
pub enum DeliveryPath {
    /// Through the device's Unified Agent: preferred.
    Ecoverse,
    /// Through the N-central Windows Agent, as a direct scheduled task.
    Rest,
}

/// What Ecoverse knows about a source device, joined on its N-central device ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EcoAsset {
    pub asset_id: String,
    pub connected: bool,
}

/// Why a device can't be launched yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "Blocker.ts"))]
#[serde(rename_all = "camelCase")]
pub enum Blocker {
    NotWindows,
    /// Its customer or site has no destination.
    Unmapped,
    /// Its customer or site fits more than one destination unit.
    AmbiguousMapping,
    /// The destination has no registration token (generate one with `generate-tokens`, or `T` on the Migrate page).
    TokenMissing,
    TokenExpired,
    /// The destination's token wasn't read, so it can't be checked.
    TokenNotChecked,
    /// Needs the REST path, but the script hasn't been uploaded to the source server.
    RestScriptMissing,
    /// Asked for the Ecoverse path, which this device doesn't have.
    NoEcoversePath,
    /// Not seen by Ecoverse and not checked in for a long time: no path can reach it.
    Unreachable,
}

impl Blocker {
    /// Says what's wrong and what to do, in N-voke's style.
    pub fn message(self) -> &'static str {
        match self {
            Blocker::NotWindows => "Not a Windows device. Only Windows agents can be migrated; list it for manual follow-up.",
            Blocker::Unmapped => "Its customer or site has no destination. Map it with: nvoke migrate-project map <project> --source <id> --dest <id>, or leave this device out.",
            Blocker::AmbiguousMapping => "Its customer or site fits more than one destination. Choose one with: nvoke migrate-project map <project> --source <id> --dest <id>.",
            Blocker::TokenMissing => "The destination has no registration token. Generate it with T on the Migrate page, or: nvoke migrate-project generate-tokens <project>.",
            Blocker::TokenExpired => "The destination's registration token has expired. Generate a new one in N-central.",
            Blocker::TokenNotChecked => "The destination's registration token hasn't been read yet.",
            Blocker::RestScriptMissing => "This device can only be reached through N-central, and the script hasn't been uploaded to that server. Export and upload it in Prepare.",
            Blocker::NoEcoversePath => "Ecoverse can't reach this device (no asset on this server). Use the REST path.",
            Blocker::Unreachable => "No agent is reachable and it hasn't checked in for a long time. It will be picked up if it comes back; export the list for on-site follow-up.",
        }
    }
}

/// Things worth knowing that don't stop a launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "Warning.ts"))]
#[serde(rename_all = "camelCase")]
pub enum Warning {
    /// The destination's token ends soon: launches after that date fail.
    TokenExpiring,
    /// The destination's token is set but its expiry couldn't be read.
    TokenExpiryUnknown,
    /// Offline now: the launch is queued and runs when it reconnects.
    QueuedUntilOnline,
    /// The server gave no usable check-in time, so reachability is a guess.
    CheckinUnknown,
    /// Its site isn't mapped, so it goes to the customer instead of a site.
    SiteNotMapped,
    PathOverridden,
}

/// Which path a device's evidence points to, before overrides and blockers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathChoice {
    Ecoverse,
    Rest,
    Unreachable,
}

/// How recently a device checked in to the source server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckIn {
    /// Within the "recent" window: its Windows Agent is up now.
    Recent,
    /// Older, but not long ago: probably switched off.
    Offline,
    /// So long ago that waiting for it is pointless.
    Gone,
    Unknown,
}

pub struct PlanOptions {
    /// A check-in this recent means the device is online now.
    pub recent: Duration,
    /// A device unseen for longer than this, with no Ecoverse asset, is unreachable.
    pub gone_after: Duration,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self { recent: Duration::minutes(15), gone_after: Duration::days(3) }
    }
}

pub fn check_in(last: Option<DateTime<Utc>>, now: DateTime<Utc>, options: &PlanOptions) -> CheckIn {
    match last.map(|t| now - t) {
        None => CheckIn::Unknown,
        Some(age) if age <= options.recent => CheckIn::Recent,
        Some(age) if age <= options.gone_after => CheckIn::Offline,
        Some(_) => CheckIn::Gone,
    }
}

/// The delivery decision from the flowchart in the design: Ecoverse when its
/// agent is connected; REST when the device is only known to N-central, or when
/// Ecoverse thinks it's offline but N-central saw it recently (Ecoverse's view
/// lags by minutes); otherwise Ecoverse with the launch queued.
pub fn choose_path(ecoverse_available: bool, eco: Option<&EcoAsset>, checkin: CheckIn) -> PathChoice {
    match eco.filter(|_| ecoverse_available) {
        Some(asset) if asset.connected => PathChoice::Ecoverse,
        Some(_) if checkin == CheckIn::Recent => PathChoice::Rest,
        Some(_) => PathChoice::Ecoverse,
        None if checkin == CheckIn::Gone => PathChoice::Unreachable,
        None => PathChoice::Rest,
    }
}

pub struct PlanInput<'a> {
    pub devices: &'a [Device],
    pub mapping: &'a [MappingEntry],
    /// Registration token status by destination org unit ID.
    pub tokens: &'a HashMap<String, TokenStatus>,
    /// Ecoverse assets by source N-central device ID.
    pub eco: &'a HashMap<String, EcoAsset>,
    /// A usable Ecoverse key, and the source server linked to it.
    pub ecoverse_available: bool,
    /// The script has been uploaded to the source server's repository.
    pub rest_script_ready: bool,
    /// Operator's choice of path, by device ID.
    pub path_overrides: &'a HashMap<String, DeliveryPath>,
    /// Devices the operator took out of the wave, by device ID.
    pub excluded: &'a BTreeSet<String>,
    pub now: DateTime<Utc>,
    pub options: PlanOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedDevice {
    pub device_id: String,
    pub name: String,
    pub source_customer_id: Option<String>,
    pub source_site_id: Option<String>,
    /// The destination customer or site that will register the device.
    pub dest_org_unit_id: Option<String>,
    /// None when blocked or excluded.
    pub path: Option<DeliveryPath>,
    pub asset_id: Option<String>,
    pub blockers: Vec<Blocker>,
    pub warnings: Vec<Warning>,
    pub excluded: bool,
}

impl PlannedDevice {
    pub fn is_ready(&self) -> bool {
        !self.excluded && self.blockers.is_empty() && self.path.is_some()
    }
}

/// The destination units whose registration tokens a plan needs, so the caller
/// can read them before planning.
pub fn token_targets(devices: &[Device], mapping: &[MappingEntry]) -> BTreeSet<String> {
    devices
        .iter()
        .filter_map(|d| destination_for(mapping, d.customer_id.as_deref(), d.site_id.as_deref()))
        .filter(|e| e.status() == MappingStatus::Mapped)
        .filter_map(|e| e.dest_id.clone())
        .collect()
}

pub fn plan(input: &PlanInput) -> Vec<PlannedDevice> {
    input.devices.iter().map(|device| plan_device(input, device)).collect()
}

fn plan_device(input: &PlanInput, device: &Device) -> PlannedDevice {
    let mut planned = PlannedDevice {
        device_id: device.device_id.clone(),
        name: device.name().to_string(),
        source_customer_id: device.customer_id.clone(),
        source_site_id: device.site_id.clone(),
        dest_org_unit_id: None,
        path: None,
        asset_id: None,
        blockers: vec![],
        warnings: vec![],
        excluded: input.excluded.contains(&device.device_id),
    };
    if planned.excluded {
        return planned;
    }

    if !device.is_windows() {
        planned.blockers.push(Blocker::NotWindows);
    }

    // Where it goes.
    match destination_for(input.mapping, device.customer_id.as_deref(), device.site_id.as_deref()) {
        None => planned.blockers.push(Blocker::Unmapped),
        Some(entry) => match entry.status() {
            MappingStatus::Mapped => {
                planned.dest_org_unit_id = entry.dest_id.clone();
                if device.site_id.is_some() && entry.source_id != device.site_id.clone().unwrap_or_default() {
                    planned.warnings.push(Warning::SiteNotMapped);
                }
            }
            MappingStatus::Ambiguous => planned.blockers.push(Blocker::AmbiguousMapping),
            MappingStatus::Unmatched => planned.blockers.push(Blocker::Unmapped),
            MappingStatus::Excluded => planned.excluded = true,
        },
    }
    if planned.excluded {
        planned.blockers.clear();
        return planned;
    }

    // Its registration token.
    if let Some(target) = &planned.dest_org_unit_id {
        match input.tokens.get(target) {
            None => planned.blockers.push(Blocker::TokenNotChecked),
            Some(TokenStatus::Missing) => planned.blockers.push(Blocker::TokenMissing),
            Some(TokenStatus::Expired { .. }) => planned.blockers.push(Blocker::TokenExpired),
            Some(TokenStatus::Expiring { .. }) => planned.warnings.push(Warning::TokenExpiring),
            Some(TokenStatus::UnknownExpiry) => planned.warnings.push(Warning::TokenExpiryUnknown),
            Some(TokenStatus::Valid { .. }) => {}
        }
    }

    // How it will be reached.
    let eco = input.eco.get(&device.device_id);
    let checkin = check_in(device.last_checkin(), input.now, &input.options);
    let path = match input.path_overrides.get(&device.device_id) {
        Some(&chosen) => {
            planned.warnings.push(Warning::PathOverridden);
            if chosen == DeliveryPath::Ecoverse && !(input.ecoverse_available && eco.is_some()) {
                planned.blockers.push(Blocker::NoEcoversePath);
                None
            } else {
                Some(chosen)
            }
        }
        None => match choose_path(input.ecoverse_available, eco, checkin) {
            PathChoice::Ecoverse => Some(DeliveryPath::Ecoverse),
            PathChoice::Rest => Some(DeliveryPath::Rest),
            PathChoice::Unreachable => {
                planned.blockers.push(Blocker::Unreachable);
                None
            }
        },
    };
    if path == Some(DeliveryPath::Rest) && !input.rest_script_ready {
        planned.blockers.push(Blocker::RestScriptMissing);
    }
    if let Some(path) = path {
        let online_now = match path {
            DeliveryPath::Ecoverse => eco.is_some_and(|a| a.connected),
            DeliveryPath::Rest => checkin == CheckIn::Recent,
        };
        if checkin == CheckIn::Unknown && path == DeliveryPath::Rest {
            planned.warnings.push(Warning::CheckinUnknown);
        } else if !online_now {
            planned.warnings.push(Warning::QueuedUntilOnline);
        }
        planned.asset_id = eco.filter(|_| path == DeliveryPath::Ecoverse).map(|a| a.asset_id.clone());
    }
    planned.path = path;
    planned.blockers.sort();
    planned.blockers.dedup();
    planned.warnings.sort();
    planned.warnings.dedup();
    planned
}

/// Counts for the plan screen and the launch confirmation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummary {
    pub total: usize,
    pub ready: usize,
    pub ready_ecoverse: usize,
    pub ready_rest: usize,
    pub excluded: usize,
    /// Devices stopped by each reason (a device with two reasons counts under both).
    pub blocked_by: BTreeMap<Blocker, usize>,
    /// Ready devices per destination customer or site.
    pub ready_by_destination: BTreeMap<String, usize>,
}

pub fn summarize(planned: &[PlannedDevice]) -> PlanSummary {
    let mut summary = PlanSummary { total: planned.len(), ..Default::default() };
    for device in planned {
        if device.excluded {
            summary.excluded += 1;
        }
        for blocker in &device.blockers {
            *summary.blocked_by.entry(*blocker).or_default() += 1;
        }
        if device.is_ready() {
            summary.ready += 1;
            match device.path {
                Some(DeliveryPath::Ecoverse) => summary.ready_ecoverse += 1,
                Some(DeliveryPath::Rest) => summary.ready_rest += 1,
                None => {}
            }
            if let Some(dest) = &device.dest_org_unit_id {
                *summary.ready_by_destination.entry(dest.clone()).or_default() += 1;
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{migration::mapping::auto_map, ncentral::model::{OrgUnit, OrgUnitType::{Customer, ServiceOrg, Site}}};
    use chrono::FixedOffset;

    fn org(id: &str, name: &str, kind: crate::ncentral::model::OrgUnitType, parent: Option<&str>) -> OrgUnit {
        OrgUnit { org_unit_id: id.into(), org_unit_name: name.into(), org_unit_type: kind, parent_id: parent.map(String::from), external_id: None }
    }

    fn now() -> DateTime<Utc> {
        "2026-10-20T12:00:00Z".parse().unwrap()
    }

    /// A Windows device that checked in `minutes_ago` minutes ago (None: no zoned check-in).
    fn device(id: &str, customer: &str, site: Option<&str>, minutes_ago: Option<i64>) -> Device {
        Device {
            device_id: id.into(),
            long_name: Some(format!("PC-{id}")),
            discovered_name: None,
            device_class: None,
            supported_os: None,
            os_id: Some("winnt".into()),
            customer_id: Some(customer.into()),
            site_id: site.map(String::from),
            org_unit_id: None,
            last_appliance_checkin_time: minutes_ago.map(|m| (now() - Duration::minutes(m)).to_rfc3339()),
            last_appliance_checkin_time_local: None,
        }
    }

    struct World {
        mapping: Vec<MappingEntry>,
        tokens: HashMap<String, TokenStatus>,
        eco: HashMap<String, EcoAsset>,
        overrides: HashMap<String, DeliveryPath>,
        excluded: BTreeSet<String>,
        ecoverse_available: bool,
        rest_script_ready: bool,
    }

    fn world() -> World {
        let source = vec![org("1", "SO", ServiceOrg, None), org("10", "Acme", Customer, Some("1")), org("11", "HQ", Site, Some("10")), org("20", "Globex", Customer, Some("1"))];
        let dest = vec![org("100", "SO", ServiceOrg, None), org("110", "Acme", Customer, Some("100")), org("111", "HQ", Site, Some("110"))];
        let valid = TokenStatus::Valid { expires: DateTime::<FixedOffset>::parse_from_rfc3339("2026-12-01T00:00:00-05:00").unwrap() };
        World {
            mapping: auto_map(&source, &dest),
            tokens: HashMap::from([("110".to_string(), valid), ("111".to_string(), valid)]),
            eco: HashMap::new(),
            overrides: HashMap::new(),
            excluded: BTreeSet::new(),
            ecoverse_available: true,
            rest_script_ready: true,
        }
    }

    fn run(w: &World, devices: &[Device]) -> Vec<PlannedDevice> {
        plan(&PlanInput {
            devices,
            mapping: &w.mapping,
            tokens: &w.tokens,
            eco: &w.eco,
            ecoverse_available: w.ecoverse_available,
            rest_script_ready: w.rest_script_ready,
            path_overrides: &w.overrides,
            excluded: &w.excluded,
            now: now(),
            options: PlanOptions::default(),
        })
    }

    fn asset(connected: bool) -> EcoAsset {
        EcoAsset { asset_id: "asset-1".into(), connected }
    }

    #[test]
    fn the_path_follows_the_design_flowchart() {
        use CheckIn::*;
        let (on, off) = (asset(true), asset(false));
        assert_eq!(choose_path(true, Some(&on), Offline), PathChoice::Ecoverse, "connected: Ecoverse, whatever N-central says");
        assert_eq!(choose_path(true, Some(&off), Recent), PathChoice::Rest, "Ecoverse lags; N-central saw it just now");
        assert_eq!(choose_path(true, Some(&off), Offline), PathChoice::Ecoverse, "offline everywhere: queue on Ecoverse");
        assert_eq!(choose_path(true, Some(&off), Unknown), PathChoice::Ecoverse);
        assert_eq!(choose_path(true, None, Recent), PathChoice::Rest, "not in Ecoverse");
        assert_eq!(choose_path(false, Some(&on), Recent), PathChoice::Rest, "no usable Ecoverse key");
        assert_eq!(choose_path(true, None, Offline), PathChoice::Rest);
        assert_eq!(choose_path(true, None, Unknown), PathChoice::Rest);
        assert_eq!(choose_path(true, None, Gone), PathChoice::Unreachable);
        assert_eq!(choose_path(true, Some(&off), Gone), PathChoice::Ecoverse, "an Ecoverse asset can still queue the launch");
    }

    #[test]
    fn check_ins_are_judged_against_the_windows() {
        let options = PlanOptions::default();
        let at = |m: i64| check_in(Some(now() - Duration::minutes(m)), now(), &options);
        assert_eq!((at(5), at(60), at(60 * 24), at(60 * 24 * 10)), (CheckIn::Recent, CheckIn::Offline, CheckIn::Offline, CheckIn::Gone));
        assert_eq!(check_in(None, now(), &options), CheckIn::Unknown);
    }

    #[test]
    fn an_online_ecoverse_device_is_ready_with_its_asset_and_destination_site() {
        let mut w = world();
        w.eco.insert("1".into(), asset(true));
        let planned = run(&w, &[device("1", "10", Some("11"), Some(3))]);
        let d = &planned[0];
        assert!(d.is_ready());
        assert_eq!((d.path, d.asset_id.as_deref(), d.dest_org_unit_id.as_deref()), (Some(DeliveryPath::Ecoverse), Some("asset-1"), Some("111")));
        assert!(d.warnings.is_empty() && d.blockers.is_empty());
    }

    #[test]
    fn an_offline_device_is_queued_and_says_so() {
        let mut w = world();
        w.eco.insert("1".into(), asset(false));
        let d = &run(&w, &[device("1", "10", None, Some(600))])[0];
        assert_eq!(d.path, Some(DeliveryPath::Ecoverse));
        assert_eq!(d.warnings, [Warning::QueuedUntilOnline]);
    }

    #[test]
    fn a_device_without_ecoverse_uses_rest_and_needs_the_uploaded_script() {
        let mut w = world();
        let devices = [device("1", "10", None, Some(2))];
        assert_eq!(run(&w, &devices)[0].path, Some(DeliveryPath::Rest));
        assert!(run(&w, &devices)[0].is_ready());
        w.rest_script_ready = false;
        let d = &run(&w, &devices)[0];
        assert_eq!(d.blockers, [Blocker::RestScriptMissing]);
        assert!(!d.is_ready());
    }

    #[test]
    fn a_device_nobody_can_reach_is_unreachable_not_launched() {
        let w = world();
        let d = &run(&w, &[device("1", "10", None, Some(60 * 24 * 30))])[0];
        assert_eq!((d.path, d.blockers.as_slice()), (None, [Blocker::Unreachable].as_slice()));
    }

    #[test]
    fn an_unzoned_check_in_is_a_guess_and_says_so() {
        let w = world();
        let d = &run(&w, &[device("1", "10", None, None)])[0];
        assert_eq!(d.path, Some(DeliveryPath::Rest));
        assert_eq!(d.warnings, [Warning::CheckinUnknown]);
        assert!(d.is_ready());
    }

    #[test]
    fn tokens_gate_the_launch() {
        let mut w = world();
        let devices = [device("1", "10", None, Some(2))];
        w.tokens.insert("110".into(), TokenStatus::Missing);
        assert_eq!(run(&w, &devices)[0].blockers, [Blocker::TokenMissing]);
        let past = DateTime::<FixedOffset>::parse_from_rfc3339("2026-10-01T00:00:00Z").unwrap();
        w.tokens.insert("110".into(), TokenStatus::Expired { expires: past });
        assert_eq!(run(&w, &devices)[0].blockers, [Blocker::TokenExpired]);
        w.tokens.insert("110".into(), TokenStatus::Expiring { expires: past });
        let d = &run(&w, &devices)[0];
        assert!(d.is_ready());
        assert_eq!(d.warnings, [Warning::TokenExpiring]);
        w.tokens.remove("110");
        assert_eq!(run(&w, &devices)[0].blockers, [Blocker::TokenNotChecked]);
    }

    #[test]
    fn mapping_problems_and_non_windows_devices_are_blocked_with_reasons() {
        let w = world();
        let mut linux = device("2", "10", None, Some(2));
        linux.os_id = Some("linux".into());
        let planned = run(&w, &[device("1", "20", None, Some(2)), linux, device("3", "999", None, Some(2))]);
        assert_eq!(planned[0].blockers, [Blocker::Unmapped], "Globex has no destination");
        assert_eq!(planned[1].blockers, [Blocker::NotWindows]);
        assert_eq!(planned[2].blockers, [Blocker::Unmapped]);
        assert!(planned.iter().all(|d| d.path.is_some() || d.blockers.contains(&Blocker::Unmapped)));
    }

    #[test]
    fn an_unmapped_site_falls_back_to_the_customer_with_a_warning() {
        let source = vec![org("1", "SO", ServiceOrg, None), org("10", "Acme", Customer, Some("1")), org("12", "Branch", Site, Some("10"))];
        let dest = vec![org("100", "SO", ServiceOrg, None), org("110", "Acme", Customer, Some("100"))];
        let mut w = world();
        w.mapping = auto_map(&source, &dest);
        let d = &run(&w, &[device("1", "10", Some("12"), Some(2))])[0];
        assert_eq!(d.dest_org_unit_id.as_deref(), Some("110"));
        assert_eq!(d.warnings, [Warning::SiteNotMapped]);
    }

    #[test]
    fn excluded_devices_and_excluded_units_are_left_out_without_blockers() {
        let mut w = world();
        w.excluded.insert("1".into());
        crate::migration::mapping::apply_overrides(&mut w.mapping, &HashMap::from([("11".to_string(), crate::migration::mapping::Override::Exclude)]), &[]);
        let planned = run(&w, &[device("1", "10", None, Some(2)), device("2", "10", Some("11"), Some(2))]);
        assert!(planned.iter().all(|d| d.excluded && d.blockers.is_empty() && !d.is_ready()));
        assert_eq!(summarize(&planned).excluded, 2);
    }

    #[test]
    fn an_override_is_honoured_but_ecoverse_needs_an_asset() {
        let mut w = world();
        w.eco.insert("1".into(), asset(true));
        w.overrides.insert("1".into(), DeliveryPath::Rest);
        w.overrides.insert("2".into(), DeliveryPath::Ecoverse);
        let planned = run(&w, &[device("1", "10", None, Some(2)), device("2", "10", None, Some(2))]);
        assert_eq!(planned[0].path, Some(DeliveryPath::Rest));
        assert!(planned[0].warnings.contains(&Warning::PathOverridden));
        assert_eq!(planned[0].asset_id, None, "no asset is needed on the REST path");
        assert_eq!(planned[1].blockers, [Blocker::NoEcoversePath]);
    }

    #[test]
    fn without_an_ecoverse_key_everything_goes_through_rest() {
        let mut w = world();
        w.ecoverse_available = false;
        w.eco.insert("1".into(), asset(true));
        assert_eq!(run(&w, &[device("1", "10", None, Some(2))])[0].path, Some(DeliveryPath::Rest));
    }

    #[test]
    fn the_token_targets_are_the_mapped_destinations_of_the_selection() {
        let w = world();
        let devices = [device("1", "10", Some("11"), None), device("2", "10", None, None), device("3", "20", None, None)];
        assert_eq!(token_targets(&devices, &w.mapping).into_iter().collect::<Vec<_>>(), ["110", "111"]);
    }

    #[test]
    fn the_summary_counts_by_path_reason_and_destination() {
        let mut w = world();
        w.eco.insert("1".into(), asset(true));
        let planned = run(&w, &[device("1", "10", Some("11"), Some(2)), device("2", "10", None, Some(2)), device("3", "20", None, Some(2))]);
        let summary = summarize(&planned);
        assert_eq!((summary.total, summary.ready, summary.ready_ecoverse, summary.ready_rest), (3, 2, 1, 1));
        assert_eq!(summary.blocked_by, BTreeMap::from([(Blocker::Unmapped, 1)]));
        assert_eq!(summary.ready_by_destination, BTreeMap::from([("110".to_string(), 1), ("111".to_string(), 1)]));
    }

    #[test]
    fn every_blocker_has_an_actionable_message() {
        for blocker in [Blocker::NotWindows, Blocker::Unmapped, Blocker::AmbiguousMapping, Blocker::TokenMissing, Blocker::TokenExpired, Blocker::TokenNotChecked, Blocker::RestScriptMissing, Blocker::NoEcoversePath, Blocker::Unreachable] {
            assert!(blocker.message().len() > 30, "{blocker:?}");
        }
    }
}
