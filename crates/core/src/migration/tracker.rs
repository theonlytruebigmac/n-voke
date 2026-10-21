//! The tracker's decisions: given what was observed about a device, which stage is it
//! in now? Pure logic with no network, following docs/migration/06-tracking.md. The
//! tracker that polls the APIs gathers an `Observation`, calls `advance`, and applies the
//! steps it returns. Because nothing here reads a clock or a server, every rule is tested
//! with a plain value, and a tracker that was closed for hours catches up the same way.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use super::project::{EcoState, ProjectDevice, Stage};
use crate::ncentral::model::DeviceIdentity;

/// How long each wait is before it becomes a problem. All configurable per project later.
#[derive(Clone, Debug)]
pub struct Thresholds {
    /// A launch on a connected device with no execution yet.
    pub not_picked_up: Duration,
    /// The same, when the device's Ecoverse asset is connected but hasn't sent any data yet: scripts are held until its
    /// first inventory, which can take much longer than a launch normally waits.
    pub data_pending_grace: Duration,
    /// Staged, but the source agent still checks in.
    pub stuck_staged: Duration,
    /// Agent removed, nothing on the destination.
    pub lost: Duration,
    /// Healthy in N-central but not in the destination's Ecoverse tenant.
    pub ecoverse_not_moved: Duration,
    /// A destination check-in this recent counts as checking in (twice the usual 5-minute interval).
    pub healthy_checkin: Duration,
    /// A source whose check-in is older than this has gone quiet.
    pub agent_quiet: Duration,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            not_picked_up: Duration::minutes(30),
            data_pending_grace: Duration::minutes(60),
            stuck_staged: Duration::minutes(90),
            lost: Duration::hours(4),
            ecoverse_not_moved: Duration::minutes(75),
            healthy_checkin: Duration::minutes(10),
            agent_quiet: Duration::minutes(10),
        }
    }
}

/// How the launch task ended up, whichever path it went through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// Accepted, but nothing has run on the device.
    NoExecution,
    Running,
    /// The launcher ran and registered the scheduled task (`TaskCreated`).
    Staged,
    /// Failed, was cancelled or expired, or ran without creating the task. Says why.
    Failed(String),
}

/// The destination device matched to this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestObservation {
    pub device_id: String,
    pub customer_id: Option<String>,
    pub site_id: Option<String>,
    /// From a zoned timestamp; None if it hasn't checked in yet.
    pub checkin_age: Option<Duration>,
}

#[derive(Clone, Debug, Default)]
pub struct Observation {
    /// The launch task, while there is one to look at.
    pub launch: Option<LaunchOutcome>,
    /// The device is reachable through its launch path right now (Ecoverse `CONNECTED`, or recent N-central check-in).
    /// None when unknown.
    pub source_connected: Option<bool>,
    /// The source's Ecoverse asset has gone `DISCONNECTED` since staging. The strongest sign the cleanup has begun.
    pub source_disconnected: bool,
    /// The source's Ecoverse asset is `CONNECTED` but has sent no asset data yet, so Ecoverse holds its scripts.
    pub source_data_pending: bool,
    /// Age of the device's last check-in to the source server (zoned), if known.
    pub source_checkin_age: Option<Duration>,
    /// A matching device on the destination. For a reinstall this is the same record as the source.
    pub dest: Option<DestObservation>,
    /// The device's asset in the destination's Ecoverse tenant is `CONNECTED`. None when the destination isn't linked.
    pub dest_ecoverse_connected: Option<bool>,
    /// What the destination's Ecoverse shows for it, when that was read.
    pub dest_ecoverse: Option<EcoState>,
    /// When it isn't shown there: where Ecoverse was looked and what was found, so the person can tell a wrong tenant from a slow agent.
    pub dest_ecoverse_note: Option<String>,
    /// `Cleanup` ran and returned `Cleaned = true`.
    pub cleaned: bool,
    /// The old source record was deleted (or tagged) as the last step.
    pub source_retired: bool,
}

/// What differs between projects.
pub struct Context<'a> {
    pub now: DateTime<Utc>,
    pub thresholds: &'a Thresholds,
    pub reinstall: bool,
    pub destination_in_ecoverse: bool,
    pub review_days: u32,
    pub retire_source_records: bool,
}

/// A stage change and the reason to record with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Step {
    pub to: Stage,
    pub why: String,
}

fn step(to: Stage, why: impl Into<String>) -> Option<Step> {
    Some(Step { to, why: why.into() })
}

fn entered(device: &ProjectDevice) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&device.entered_stage_at).ok().map(|t| t.with_timezone(&Utc))
}

/// How long the device has been in its current stage. Zero if the time can't be read.
fn time_in_stage(device: &ProjectDevice, now: DateTime<Utc>) -> Duration {
    entered(device).map_or(Duration::zero(), |t| (now - t).max(Duration::zero()))
}

/// Whether the destination device is under the customer or site the device was mapped to.
fn in_expected_place(device: &ProjectDevice, dest: &DestObservation) -> bool {
    match device.dest_org_unit_id.as_deref() {
        None => true,
        Some(expected) => dest.site_id.as_deref() == Some(expected) || dest.customer_id.as_deref() == Some(expected),
    }
}

/// Whether the old agent has stopped: the cleanup or install is under way (or done).
fn agent_gone(device: &ProjectDevice, obs: &Observation, ctx: &Context) -> bool {
    let quiet = obs.source_checkin_age.is_some_and(|age| age > ctx.thresholds.agent_quiet);
    // A reinstall keeps the same record, so the destination appearing proves nothing there.
    let arrived = !ctx.reinstall && obs.dest.is_some();
    // Check-in time alone is unreliable for an Ecoverse device (it drifts and jumps), so it only counts on the REST path
    // or in a reinstall, where there is no other signal.
    let quiet_counts = quiet && (ctx.reinstall || device.path == Some(super::planner::DeliveryPath::Rest));
    obs.source_disconnected || arrived || quiet_counts
}

/// How long a launch may wait before it counts as not picked up: longer while the device's Ecoverse asset is still
/// sending its first inventory, because Ecoverse holds scripts until then.
fn wait_for_pickup(t: &Thresholds, obs: &Observation) -> Duration {
    if obs.source_data_pending { t.not_picked_up.max(t.data_pending_grace) } else { t.not_picked_up }
}

fn pickup_reason(waited: Duration, obs: &Observation) -> String {
    let minutes = waited.num_minutes();
    if obs.source_data_pending {
        format!("connected, but Ecoverse had still not received its asset data and no run started {minutes} minutes after the launch")
    } else {
        format!("connected, but no run started {minutes} minutes after the launch")
    }
}

/// The next single stage change for this device, or None if it stays where it is.
fn next_step(device: &ProjectDevice, obs: &Observation, ctx: &Context) -> Option<Step> {
    let t = ctx.thresholds;
    let waited = time_in_stage(device, ctx.now);
    match device.stage {
        Stage::Launched | Stage::NotPickedUp => match &obs.launch {
            Some(LaunchOutcome::Staged) if device.stage == Stage::NotPickedUp => step(Stage::Launched, "the launch was picked up late"),
            Some(LaunchOutcome::Staged) => step(Stage::Staged, "the launcher registered the scheduled task"),
            Some(LaunchOutcome::Failed(reason)) if device.stage == Stage::NotPickedUp => step(Stage::Launched, format!("the launch ended: {reason}")),
            Some(LaunchOutcome::Failed(reason)) => step(Stage::LaunchFailed, reason.clone()),
            Some(LaunchOutcome::Running) if device.stage == Stage::NotPickedUp => step(Stage::Launched, "the launch is running"),
            // A queued launch for a device that's offline looks the same as a stalled one, so only a connected device is flagged.
            Some(LaunchOutcome::NoExecution) | None if device.stage == Stage::Launched && obs.source_connected == Some(true) && waited >= wait_for_pickup(t, obs) => {
                step(Stage::NotPickedUp, pickup_reason(waited, obs))
            }
            _ => None,
        },
        Stage::Staged | Stage::StuckStaged => {
            if agent_gone(device, obs, ctx) {
                step(Stage::AgentRemoved, "the old agent has stopped")
            } else if device.stage == Stage::Staged && waited >= t.stuck_staged {
                step(Stage::StuckStaged, format!("still checking in to the old server {} minutes after staging", waited.num_minutes()))
            } else {
                None
            }
        }
        Stage::AgentRemoved | Stage::Lost => match &obs.dest {
            Some(dest) => step(Stage::Registered, format!("found on the destination as device {}", dest.device_id)),
            None if device.stage == Stage::AgentRemoved && waited >= t.lost => step(Stage::Lost, format!("nothing on the destination {} minutes after the old agent stopped", waited.num_minutes())),
            None => None,
        },
        Stage::Registered => {
            let dest = obs.dest.as_ref()?;
            if !in_expected_place(device, dest) {
                return step(Stage::WrongPlace, "registered under a different customer or site than mapped");
            }
            let checking_in = dest.checkin_age.is_some_and(|age| age <= t.healthy_checkin);
            let ecoverse_ok = !ctx.destination_in_ecoverse || obs.dest_ecoverse_connected == Some(true);
            (checking_in && ecoverse_ok).then(|| Step { to: Stage::Healthy, why: "checking in to the destination".into() })
        }
        Stage::Healthy => {
            let review_over = waited >= Duration::days(i64::from(ctx.review_days));
            if obs.cleaned {
                step(Stage::CleanedUp, "cleanup returned Cleaned = true")
            } else if review_over {
                step(Stage::CleanedUp, "the review period ended and the device removed its own records")
            } else {
                None
            }
        }
        Stage::CleanedUp if ctx.retire_source_records && obs.source_retired => step(Stage::Retired, "the old source record was removed"),
        _ => None,
    }
}

/// Every stage change that has happened since the device was last looked at, in order. A device that
/// was left for hours can pass through several stages in one look, and each is recorded.
pub fn advance(device: &ProjectDevice, obs: &Observation, ctx: &Context) -> Vec<Step> {
    let mut probe = device.clone();
    let mut steps = vec![];
    // Stages entered in this pass count from now, so a waiting stage can't be skipped by a stale timestamp.
    while let Some(step) = next_step(&probe, obs, ctx) {
        if steps.len() >= 8 || !probe.stage.can_move_to(step.to) {
            break;
        }
        probe.stage = step.to;
        probe.entered_stage_at = ctx.now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        steps.push(step);
    }
    steps
}

/// Something worth the operator's eye that isn't a stage of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Attention {
    /// Healthy in N-central, but its asset isn't connected in the destination's Ecoverse tenant after the grace period.
    EcoverseNotMoved,
}

pub fn attention(device: &ProjectDevice, obs: &Observation, ctx: &Context) -> Vec<Attention> {
    let mut flags = vec![];
    let past_registration = matches!(device.stage, Stage::Registered | Stage::Healthy);
    if ctx.destination_in_ecoverse && past_registration && obs.dest_ecoverse_connected != Some(true) && time_in_stage(device, ctx.now) >= ctx.thresholds.ecoverse_not_moved {
        flags.push(Attention::EcoverseNotMoved);
    }
    flags
}

// ---- matching the new device to the old one ----------------------------------------------

/// A device on the destination that might be the one that moved.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub device_id: String,
    pub identity: DeviceIdentity,
    /// It wasn't on the destination before the launch.
    pub new_since_launch: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strength {
    /// Serial and a MAC, or host name and a MAC.
    Strong,
    /// Host name alone, the only new device with it.
    Weak,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchResult {
    Found { device_id: String, strength: Strength },
    /// More than one fits equally: the operator chooses.
    Ambiguous(Vec<String>),
    None,
}

fn same(a: &Option<String>, b: &Option<String>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if x.eq_ignore_ascii_case(y))
}

fn shares_mac(a: &DeviceIdentity, b: &DeviceIdentity) -> bool {
    a.macs.iter().any(|m| b.macs.contains(m))
}

/// Matches by, in order: serial plus a MAC, host name plus a MAC, then host name alone when exactly one
/// candidate that appeared since the launch has it. Candidates are those under the mapped destination
/// customer, which the caller selects. A new asset's inventory is empty at first, so early on only the
/// host name is known and the match is weak or absent.
pub fn match_destination(old: &DeviceIdentity, candidates: &[Candidate]) -> MatchResult {
    let strong: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| shares_mac(old, &c.identity) && (same(&old.serial, &c.identity.serial) || same(&old.hostname, &c.identity.hostname)))
        .collect();
    match strong.as_slice() {
        [one] => return MatchResult::Found { device_id: one.device_id.clone(), strength: Strength::Strong },
        [] => {}
        several => return MatchResult::Ambiguous(several.iter().map(|c| c.device_id.clone()).collect()),
    }
    let by_name: Vec<&Candidate> = candidates.iter().filter(|c| c.new_since_launch && same(&old.hostname, &c.identity.hostname)).collect();
    match by_name.as_slice() {
        [] => MatchResult::None,
        [one] => MatchResult::Found { device_id: one.device_id.clone(), strength: Strength::Weak },
        several => MatchResult::Ambiguous(several.iter().map(|c| c.device_id.clone()).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::{
        planner::{DeliveryPath, PlannedDevice},
        project::stamp,
    };

    fn now() -> DateTime<Utc> {
        "2026-10-20T12:00:00Z".parse().unwrap()
    }

    /// A device that entered `stage` `minutes` minutes before `now()`.
    fn device(stage: Stage, minutes: i64, path: DeliveryPath) -> ProjectDevice {
        let planned = PlannedDevice {
            device_id: "9001".into(),
            name: "PC".into(),
            source_customer_id: Some("10".into()),
            source_site_id: Some("11".into()),
            dest_org_unit_id: Some("111".into()),
            path: Some(path),
            asset_id: Some("a1".into()),
            blockers: vec![],
            warnings: vec![],
            excluded: false,
        };
        let mut d = ProjectDevice::from_plan(&planned, now());
        d.stage = stage;
        d.entered_stage_at = stamp(now() - Duration::minutes(minutes));
        d
    }

    fn ctx(t: &Thresholds) -> Context<'_> {
        Context { now: now(), thresholds: t, reinstall: false, destination_in_ecoverse: false, review_days: 7, retire_source_records: false }
    }

    fn dest(site: &str, checkin_minutes: Option<i64>) -> DestObservation {
        DestObservation { device_id: "5001".into(), customer_id: Some("110".into()), site_id: Some(site.into()), checkin_age: checkin_minutes.map(Duration::minutes) }
    }

    fn stages(steps: Vec<Step>) -> Vec<Stage> {
        steps.into_iter().map(|s| s.to).collect()
    }

    #[test]
    fn a_launch_that_ran_is_staged_one_that_failed_says_why() {
        let t = Thresholds::default();
        let d = device(Stage::Launched, 1, DeliveryPath::Ecoverse);
        let obs = Observation { launch: Some(LaunchOutcome::Staged), ..Default::default() };
        assert_eq!(stages(advance(&d, &obs, &ctx(&t))), [Stage::Staged]);
        let obs = Observation { launch: Some(LaunchOutcome::Failed("exit status 1004".into())), ..Default::default() };
        let steps = advance(&d, &obs, &ctx(&t));
        assert_eq!((steps[0].to, steps[0].why.as_str()), (Stage::LaunchFailed, "exit status 1004"));
        let running = Observation { launch: Some(LaunchOutcome::Running), ..Default::default() };
        assert!(advance(&d, &running, &ctx(&t)).is_empty());
    }

    #[test]
    fn only_a_connected_device_with_no_run_after_30_minutes_is_not_picked_up() {
        let t = Thresholds::default();
        let queued = |minutes, connected| {
            let obs = Observation { launch: Some(LaunchOutcome::NoExecution), source_connected: connected, ..Default::default() };
            advance(&device(Stage::Launched, minutes, DeliveryPath::Ecoverse), &obs, &ctx(&t))
        };
        assert!(queued(29, Some(true)).is_empty(), "not yet");
        assert_eq!(stages(queued(30, Some(true))), [Stage::NotPickedUp]);
        assert!(queued(600, Some(false)).is_empty(), "a queued launch for an offline device waits, however long");
        assert!(queued(600, None).is_empty(), "unknown connectivity isn't flagged");
    }

    #[test]
    fn a_launch_waits_an_hour_while_the_assets_data_is_still_pending_and_says_why() {
        let t = Thresholds::default();
        let queued = |minutes, pending| {
            let obs = Observation { launch: Some(LaunchOutcome::NoExecution), source_connected: Some(true), source_data_pending: pending, ..Default::default() };
            advance(&device(Stage::Launched, minutes, DeliveryPath::Ecoverse), &obs, &ctx(&t))
        };
        assert!(queued(45, true).is_empty(), "Ecoverse holds scripts until the first inventory, so 45 minutes isn't long yet");
        assert_eq!(stages(queued(45, false)), [Stage::NotPickedUp], "with its data in, 45 minutes is");
        let late = queued(60, true);
        assert_eq!(stages(late.clone()), [Stage::NotPickedUp]);
        assert!(late[0].why.contains("asset data"), "{}", late[0].why);
    }

    #[test]
    fn a_late_pickup_goes_back_to_launched_and_on_to_staged_in_one_look() {
        let t = Thresholds::default();
        let d = device(Stage::NotPickedUp, 50, DeliveryPath::Ecoverse);
        let obs = Observation { launch: Some(LaunchOutcome::Staged), ..Default::default() };
        assert_eq!(stages(advance(&d, &obs, &ctx(&t))), [Stage::Launched, Stage::Staged]);
    }

    #[test]
    fn staged_becomes_agent_removed_when_ecoverse_drops_or_the_destination_appears() {
        let t = Thresholds::default();
        let d = device(Stage::Staged, 5, DeliveryPath::Ecoverse);
        let dropped = Observation { source_disconnected: true, ..Default::default() };
        assert_eq!(stages(advance(&d, &dropped, &ctx(&t))), [Stage::AgentRemoved]);
        let arrived = Observation { dest: Some(dest("111", None)), ..Default::default() };
        assert_eq!(stages(advance(&d, &arrived, &ctx(&t))), [Stage::AgentRemoved, Stage::Registered]);
    }

    #[test]
    fn an_old_check_in_alone_moves_a_rest_device_but_not_an_ecoverse_one() {
        let t = Thresholds::default();
        let obs = Observation { source_checkin_age: Some(Duration::minutes(30)), ..Default::default() };
        assert_eq!(stages(advance(&device(Stage::Staged, 5, DeliveryPath::Rest), &obs, &ctx(&t))), [Stage::AgentRemoved]);
        assert!(advance(&device(Stage::Staged, 5, DeliveryPath::Ecoverse), &obs, &ctx(&t)).is_empty(), "the check-in time drifts and jumps; Ecoverse's connection is the signal");
        let recent = Observation { source_checkin_age: Some(Duration::minutes(3)), ..Default::default() };
        assert!(advance(&device(Stage::Staged, 5, DeliveryPath::Rest), &recent, &ctx(&t)).is_empty());
    }

    #[test]
    fn staged_for_90_minutes_with_the_agent_still_there_is_stuck_and_can_still_recover() {
        let t = Thresholds::default();
        let still = Observation { source_checkin_age: Some(Duration::minutes(2)), ..Default::default() };
        assert!(advance(&device(Stage::Staged, 89, DeliveryPath::Rest), &still, &ctx(&t)).is_empty());
        assert_eq!(stages(advance(&device(Stage::Staged, 90, DeliveryPath::Rest), &still, &ctx(&t))), [Stage::StuckStaged]);
        let gone = Observation { source_disconnected: true, ..Default::default() };
        assert_eq!(stages(advance(&device(Stage::StuckStaged, 200, DeliveryPath::Rest), &gone, &ctx(&t))), [Stage::AgentRemoved]);
    }

    #[test]
    fn nothing_on_the_destination_after_four_hours_is_lost_and_a_late_arrival_is_found() {
        let t = Thresholds::default();
        let none = Observation::default();
        assert!(advance(&device(Stage::AgentRemoved, 239, DeliveryPath::Rest), &none, &ctx(&t)).is_empty());
        assert_eq!(stages(advance(&device(Stage::AgentRemoved, 240, DeliveryPath::Rest), &none, &ctx(&t))), [Stage::Lost]);
        assert!(advance(&device(Stage::Lost, 9000, DeliveryPath::Rest), &none, &ctx(&t)).is_empty(), "lost devices stay lost until they show up");
        let back = Observation { dest: Some(dest("111", Some(2))), ..Default::default() };
        assert_eq!(stages(advance(&device(Stage::Lost, 9000, DeliveryPath::Rest), &back, &ctx(&t))), [Stage::Registered, Stage::Healthy]);
    }

    #[test]
    fn registered_is_healthy_once_it_checks_in_and_wrong_place_when_under_another_unit() {
        let t = Thresholds::default();
        let d = device(Stage::Registered, 3, DeliveryPath::Rest);
        let first_seen = Observation { dest: Some(dest("111", None)), ..Default::default() };
        assert!(advance(&d, &first_seen, &ctx(&t)).is_empty(), "registered but not checking in yet");
        let old_checkin = Observation { dest: Some(dest("111", Some(45))), ..Default::default() };
        assert!(advance(&d, &old_checkin, &ctx(&t)).is_empty());
        let checking_in = Observation { dest: Some(dest("111", Some(4))), ..Default::default() };
        assert_eq!(stages(advance(&d, &checking_in, &ctx(&t))), [Stage::Healthy]);
        let elsewhere = Observation { dest: Some(dest("999", Some(4))), ..Default::default() };
        let steps = advance(&d, &elsewhere, &ctx(&t));
        assert_eq!(stages(steps), [Stage::WrongPlace], "a wrong place is never called healthy");
        // Mapped to the customer rather than a site: the customer matches.
        let mut to_customer = device(Stage::Registered, 3, DeliveryPath::Rest);
        to_customer.dest_org_unit_id = Some("110".into());
        assert_eq!(stages(advance(&to_customer, &checking_in, &ctx(&t))), [Stage::Healthy]);
    }

    #[test]
    fn with_a_linked_destination_healthy_also_needs_the_new_ecoverse_asset_connected() {
        let t = Thresholds::default();
        let mut c = ctx(&t);
        c.destination_in_ecoverse = true;
        let d = device(Stage::Registered, 3, DeliveryPath::Rest);
        let mut obs = Observation { dest: Some(dest("111", Some(4))), dest_ecoverse_connected: Some(false), ..Default::default() };
        assert!(advance(&d, &obs, &c).is_empty());
        obs.dest_ecoverse_connected = None;
        assert!(advance(&d, &obs, &c).is_empty());
        obs.dest_ecoverse_connected = Some(true);
        assert_eq!(stages(advance(&d, &obs, &c)), [Stage::Healthy]);
    }

    #[test]
    fn ecoverse_not_moved_is_flagged_after_75_minutes_only_for_a_linked_destination() {
        let t = Thresholds::default();
        let obs = Observation { dest: Some(dest("111", Some(2))), dest_ecoverse_connected: Some(false), ..Default::default() };
        let mut c = ctx(&t);
        assert!(attention(&device(Stage::Healthy, 200, DeliveryPath::Rest), &obs, &c).is_empty(), "leaving Ecoverse is expected when the destination isn't linked");
        c.destination_in_ecoverse = true;
        assert!(attention(&device(Stage::Healthy, 74, DeliveryPath::Rest), &obs, &c).is_empty());
        assert_eq!(attention(&device(Stage::Healthy, 75, DeliveryPath::Rest), &obs, &c), [Attention::EcoverseNotMoved]);
        let connected = Observation { dest_ecoverse_connected: Some(true), ..obs };
        assert!(attention(&device(Stage::Healthy, 500, DeliveryPath::Rest), &connected, &c).is_empty());
    }

    #[test]
    fn a_reinstall_is_tracked_by_check_in_age_on_the_same_record() {
        let t = Thresholds::default();
        let mut c = ctx(&t);
        c.reinstall = true;
        // The destination is the same record, so seeing it proves nothing while the old agent is still there.
        let same_record = Observation { dest: Some(dest("111", Some(2))), source_checkin_age: Some(Duration::minutes(2)), ..Default::default() };
        assert!(advance(&device(Stage::Staged, 5, DeliveryPath::Ecoverse), &same_record, &c).is_empty());
        let paused = Observation { dest: Some(dest("111", Some(12))), source_checkin_age: Some(Duration::minutes(12)), ..Default::default() };
        assert_eq!(stages(advance(&device(Stage::Staged, 5, DeliveryPath::Ecoverse), &paused, &c)), [Stage::AgentRemoved, Stage::Registered]);
    }

    #[test]
    fn healthy_is_cleaned_up_when_cleanup_ran_or_the_review_period_ended() {
        let t = Thresholds::default();
        let none = Observation::default();
        let day = 24 * 60;
        assert!(advance(&device(Stage::Healthy, 6 * day, DeliveryPath::Rest), &none, &ctx(&t)).is_empty(), "still under review");
        assert_eq!(stages(advance(&device(Stage::Healthy, 7 * day, DeliveryPath::Rest), &none, &ctx(&t))), [Stage::CleanedUp]);
        let cleaned = Observation { cleaned: true, ..Default::default() };
        assert_eq!(stages(advance(&device(Stage::Healthy, 5, DeliveryPath::Rest), &cleaned, &ctx(&t))), [Stage::CleanedUp]);
    }

    #[test]
    fn retiring_the_source_record_is_optional_and_needs_its_own_confirmation() {
        let t = Thresholds::default();
        let d = device(Stage::CleanedUp, 5, DeliveryPath::Rest);
        let done = Observation { source_retired: true, ..Default::default() };
        assert!(advance(&d, &done, &ctx(&t)).is_empty(), "off by default");
        let mut c = ctx(&t);
        c.retire_source_records = true;
        assert!(advance(&d, &Observation::default(), &c).is_empty(), "not until it has happened");
        assert_eq!(stages(advance(&d, &done, &c)), [Stage::Retired]);
    }

    #[test]
    fn final_and_attention_stages_wait_for_the_operator() {
        let t = Thresholds::default();
        let any = Observation { launch: Some(LaunchOutcome::Staged), source_disconnected: true, dest: Some(dest("111", Some(1))), cleaned: true, ..Default::default() };
        for stage in [Stage::Planned, Stage::LaunchFailed, Stage::WrongPlace, Stage::Retired, Stage::Excluded, Stage::Resolved, Stage::Unreachable] {
            assert!(advance(&device(stage, 9999, DeliveryPath::Rest), &any, &ctx(&t)).is_empty(), "{stage:?}");
        }
    }

    #[test]
    fn every_step_follows_the_state_diagram_even_after_a_long_gap() {
        let t = Thresholds::default();
        let obs = Observation { launch: Some(LaunchOutcome::Staged), source_disconnected: true, dest: Some(dest("111", Some(3))), cleaned: true, ..Default::default() };
        let mut d = device(Stage::Launched, 600, DeliveryPath::Ecoverse);
        let steps = advance(&d, &obs, &ctx(&t));
        assert_eq!(stages(steps.clone()), [Stage::Staged, Stage::AgentRemoved, Stage::Registered, Stage::Healthy, Stage::CleanedUp]);
        for s in steps {
            d.move_to(s.to, now(), &s.why).expect("every step is a legal move");
        }
    }

    // ---- matching ----

    fn identity(serial: Option<&str>, host: Option<&str>, macs: &[&str]) -> DeviceIdentity {
        DeviceIdentity { serial: serial.map(String::from), hostname: host.map(String::from), macs: macs.iter().map(|m| m.to_string()).collect(), smbios_uuid: None }
    }

    fn candidate(id: &str, identity: DeviceIdentity, new: bool) -> Candidate {
        Candidate { device_id: id.into(), identity, new_since_launch: new }
    }

    #[test]
    fn serial_plus_a_mac_or_host_plus_a_mac_is_a_strong_match() {
        let old = identity(Some("VMW-1"), Some("DESKTOP-42"), &["aabbccddee01"]);
        let by_serial = candidate("1", identity(Some("vmw-1"), None, &["aabbccddee01", "112233445566"]), false);
        let other = candidate("2", identity(Some("VMW-9"), Some("OTHER"), &["ffffffffffff"]), false);
        assert_eq!(match_destination(&old, &[other.clone(), by_serial]), MatchResult::Found { device_id: "1".into(), strength: Strength::Strong });
        let by_host = candidate("3", identity(None, Some("desktop-42"), &["aabbccddee01"]), false);
        assert_eq!(match_destination(&old, &[other, by_host]), MatchResult::Found { device_id: "3".into(), strength: Strength::Strong });
    }

    #[test]
    fn a_host_name_alone_is_weak_and_only_accepted_when_unique_and_new() {
        let old = identity(Some("VMW-1"), Some("DESKTOP-42"), &["aabbccddee01"]);
        // A new asset has no inventory yet: just a host name.
        let fresh = candidate("7", identity(None, Some("DESKTOP-42"), &[]), true);
        assert_eq!(match_destination(&old, std::slice::from_ref(&fresh)), MatchResult::Found { device_id: "7".into(), strength: Strength::Weak });
        let pre_existing = candidate("8", identity(None, Some("DESKTOP-42"), &[]), false);
        assert_eq!(match_destination(&old, &[pre_existing]), MatchResult::None, "a device that was already there isn't it");
        let twin = candidate("9", identity(None, Some("desktop-42"), &[]), true);
        assert_eq!(match_destination(&old, &[fresh, twin]), MatchResult::Ambiguous(vec!["7".into(), "9".into()]));
    }

    #[test]
    fn several_strong_matches_or_no_candidates_are_reported_not_guessed() {
        let old = identity(Some("VMW-1"), Some("PC"), &["aabbccddee01"]);
        let a = candidate("1", identity(Some("VMW-1"), None, &["aabbccddee01"]), false);
        let b = candidate("2", identity(Some("VMW-1"), None, &["aabbccddee01"]), false);
        assert_eq!(match_destination(&old, &[a, b]), MatchResult::Ambiguous(vec!["1".into(), "2".into()]));
        assert_eq!(match_destination(&old, &[]), MatchResult::None);
        let unrelated = candidate("5", identity(Some("X"), Some("OTHER"), &["010203040506"]), true);
        assert_eq!(match_destination(&old, &[unrelated]), MatchResult::None);
    }

    #[test]
    fn a_shared_mac_without_a_serial_or_name_match_is_not_enough() {
        // Virtual adapters and docks can share MACs across machines.
        let old = identity(Some("VMW-1"), Some("PC-A"), &["aabbccddee01"]);
        let other_machine = candidate("4", identity(Some("VMW-2"), Some("PC-B"), &["aabbccddee01"]), true);
        assert_eq!(match_destination(&old, &[other_machine]), MatchResult::None);
    }
}
