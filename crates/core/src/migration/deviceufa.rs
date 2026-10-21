//! What a device's own record says about its Ecoverse agent (UFA): the installer's watcher writes the agent's state,
//! its restarts and the one-time repair into the device's `status.json`, and a Probe brings that back as a `status`
//! event on the device. This reads the latest one, so the Track step can say more than "waiting for Ecoverse data".
//! It is only as fresh as the last Probe; the device is not read on its own.

use serde::Serialize;
use serde_json::Value;

use super::project::Event;

/// Where the Probe's answer carries the device's `status.json`, after the message line.
const MARKER: &str = "\nstatus.json: ";
/// The history entries that are the watcher doing something about the agent.
const STEP_EVENTS: [&str; 3] = ["ufa-sync-restart", "ufa-repair", "ufa-repair-restart"];
const MAX_STEPS: usize = 6;

/// The Ecoverse agent's state on the device, as the installer's watcher classifies it from the agent's own log.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "UfaState.ts"))]
#[serde(rename_all = "camelCase")]
pub enum UfaState {
    /// Not on the device (yet, or removed by the repair and waiting to be redeployed).
    NotInstalled,
    /// Installed, not provisioned with the destination yet.
    NotProvisioned,
    /// Provisioned, no fault and no inventory component yet (normal for a while).
    Waiting,
    /// The inventory component arrived: the data is flowing.
    Synced,
    /// The configuration fetch failed, the cloud answers again, and nothing was delivered: a restart fixes it.
    Stalled,
    /// The cloud's login service doesn't answer the agent. Often clears by itself.
    Blocked,
    /// The cloud rejects the agent's login (400 SSL certificate error).
    Rejected,
    /// The login works but the component downloads are refused (403).
    Denied,
    /// No log to read, or a state this version doesn't know.
    Unknown,
}

/// One thing the watcher did about the agent.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "UfaStep.ts"))]
#[serde(rename_all = "camelCase")]
pub struct UfaStep {
    pub at: String,
    /// `ufa-sync-restart`, `ufa-repair` or `ufa-repair-restart`.
    pub event: String,
    pub detail: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DeviceUfa.ts"))]
#[serde(rename_all = "camelCase")]
pub struct DeviceUfa {
    pub state: UfaState,
    /// When the Probe that brought this back was recorded.
    pub probed_at: String,
    /// How many times the watcher restarted the agent's service.
    pub restarts: u32,
    /// When the one-time repair was done, if it was.
    pub repaired_at: Option<String>,
    /// When the watcher first saw a fault, if it did.
    pub fault_seen_at: Option<String>,
    /// After a repair: files written to the agent's folder since the last restart, the sign that it is being redeployed.
    pub redeploy_files: Option<u32>,
    /// The installer's own warning, if it gave up waiting.
    pub warning: Option<String>,
    /// The latest restarts and repair steps, oldest first.
    pub steps: Vec<UfaStep>,
}

/// The agent's state from the latest Probe that brought back a readable `status.json`, or none when the device
/// hasn't been probed, the record says nothing about the agent, or it isn't tracked there.
pub fn device_ufa(events: &[Event]) -> Option<DeviceUfa> {
    for event in events.iter().rev().filter(|e| e.kind == "status") {
        let Some((_, json)) = event.detail.split_once(MARKER) else { continue };
        let Ok(value) = serde_json::from_str::<Value>(json.trim()) else { continue };
        return from_status(&event.at, &value);
    }
    None
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

fn from_status(probed_at: &str, status: &Value) -> Option<DeviceUfa> {
    let ufa = status.get("ufa")?.as_object()?;
    let flag = |key: &str| ufa.get(key).and_then(Value::as_bool).unwrap_or(false);
    let state = match ufa.get("sync").and_then(Value::as_str) {
        Some("synced") => UfaState::Synced,
        Some("stalled") => UfaState::Stalled,
        Some("blocked") => UfaState::Blocked,
        Some("rejected") => UfaState::Rejected,
        Some("denied") => UfaState::Denied,
        Some("waiting") => UfaState::Waiting,
        Some(_) => UfaState::Unknown,
        None if !flag("present") => UfaState::NotInstalled,
        None if !flag("provisioned") => UfaState::NotProvisioned,
        None => UfaState::Waiting,
    };
    let steps: Vec<UfaStep> = status
        .get("history")
        .and_then(Value::as_array)
        .map(|history| {
            history
                .iter()
                .filter_map(|entry| {
                    let event = text(entry, "event").filter(|e| STEP_EVENTS.contains(&e.as_str()))?;
                    Some(UfaStep { at: text(entry, "at")?, event, detail: text(entry, "detail").unwrap_or_default() })
                })
                .collect()
        })
        .unwrap_or_default();
    let skip = steps.len().saturating_sub(MAX_STEPS);
    Some(DeviceUfa {
        state,
        probed_at: probed_at.to_string(),
        restarts: status.get("ufaSyncRestarts").and_then(Value::as_u64).unwrap_or(0) as u32,
        repaired_at: text(status, "ufaRepairedAt"),
        fault_seen_at: text(status, "ufaFaultSeenAt"),
        redeploy_files: ufa.get("redeployFiles").and_then(Value::as_u64).map(|n| n as u32),
        warning: text(status, "warning"),
        steps: steps.into_iter().skip(skip).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(at: &str, status: &str) -> Event {
        Event { at: at.into(), kind: "status".into(), detail: format!("Status via N-central: Last log: awaiting-ufa\nstatus.json: {status}") }
    }

    fn state_of(ufa: &str) -> UfaState {
        let status = format!(r#"{{"stage":"awaiting-ufa","ufa":{ufa}}}"#);
        device_ufa(&[probe("2026-10-21T15:00:00Z", &status)]).unwrap().state
    }

    #[test]
    fn each_sync_word_from_the_installer_is_a_state() {
        for (word, expected) in [
            ("synced", UfaState::Synced),
            ("stalled", UfaState::Stalled),
            ("blocked", UfaState::Blocked),
            ("rejected", UfaState::Rejected),
            ("denied", UfaState::Denied),
            ("waiting", UfaState::Waiting),
            ("unknown", UfaState::Unknown),
            ("something new", UfaState::Unknown),
        ] {
            assert_eq!(state_of(&format!(r#"{{"present":true,"running":true,"provisioned":true,"sync":"{word}"}}"#)), expected, "{word}");
        }
    }

    #[test]
    fn without_a_sync_word_the_state_is_how_far_the_agent_got() {
        assert_eq!(state_of(r#"{"present":false,"running":false,"provisioned":false}"#), UfaState::NotInstalled);
        assert_eq!(state_of(r#"{"present":true,"running":true,"provisioned":false}"#), UfaState::NotProvisioned);
        assert_eq!(state_of(r#"{"present":true,"running":true,"provisioned":true}"#), UfaState::Waiting);
    }

    #[test]
    fn restarts_the_repair_and_the_watchers_steps_come_through_and_other_history_does_not() {
        let status = r#"{"stage":"awaiting-ufa","ufaSyncRestarts":1,"ufaRepairedAt":"2026-10-21T16:10:00Z","ufaFaultSeenAt":"2026-10-21T14:55:00Z","warning":"gave up",
            "ufa":{"present":false,"running":false,"provisioned":false,"redeployFiles":4,"redeployNewestAt":"2026-10-21T16:20:00Z"},
            "history":[
              {"at":"2026-10-21T14:55:00Z","event":"awaiting-ufa","detail":"blocked"},
              {"at":"2026-10-21T15:25:00Z","event":"ufa-sync-restart","detail":"restarted MspAgent"},
              {"at":"2026-10-21T16:10:00Z","event":"ufa-repair","detail":"removed the UFA core"},
              {"at":"2026-10-21T16:20:00Z","event":"awaiting-ufa","detail":"4 file(s) written"}]}"#;
        let ufa = device_ufa(&[probe("2026-10-21T16:25:00Z", status)]).unwrap();
        assert_eq!((ufa.state, ufa.restarts, ufa.redeploy_files), (UfaState::NotInstalled, 1, Some(4)));
        assert_eq!((ufa.repaired_at.as_deref(), ufa.fault_seen_at.as_deref(), ufa.warning.as_deref()), (Some("2026-10-21T16:10:00Z"), Some("2026-10-21T14:55:00Z"), Some("gave up")));
        assert_eq!(ufa.probed_at, "2026-10-21T16:25:00Z");
        assert_eq!(ufa.steps.iter().map(|s| s.event.as_str()).collect::<Vec<_>>(), ["ufa-sync-restart", "ufa-repair"]);
        assert_eq!(ufa.steps[1].detail, "removed the UFA core");
    }

    #[test]
    fn only_the_latest_few_steps_are_kept_oldest_first() {
        let steps: Vec<String> = (0..9).map(|i| format!(r#"{{"at":"2026-10-21T15:{i:02}:00Z","event":"ufa-sync-restart","detail":"restart {i}"}}"#)).collect();
        let status = format!(r#"{{"ufa":{{"present":true,"provisioned":true,"sync":"blocked"}},"history":[{}]}}"#, steps.join(","));
        let ufa = device_ufa(&[probe("2026-10-21T16:00:00Z", &status)]).unwrap();
        assert_eq!(ufa.steps.len(), MAX_STEPS);
        assert_eq!((ufa.steps[0].detail.as_str(), ufa.steps[MAX_STEPS - 1].detail.as_str()), ("restart 3", "restart 8"));
    }

    #[test]
    fn the_latest_readable_probe_wins() {
        let events = [
            probe("2026-10-21T15:00:00Z", r#"{"ufa":{"present":true,"provisioned":true,"sync":"blocked"}}"#),
            probe("2026-10-21T16:00:00Z", r#"{"ufa":{"present":true,"provisioned":true,"sync":"synced"}}"#),
            Event { at: "2026-10-21T16:30:00Z".into(), kind: "note".into(), detail: "something else".into() },
        ];
        let ufa = device_ufa(&events).unwrap();
        assert_eq!((ufa.state, ufa.probed_at.as_str()), (UfaState::Synced, "2026-10-21T16:00:00Z"));
    }

    #[test]
    fn a_probe_with_no_record_or_a_cut_off_one_falls_back_to_an_earlier_probe_and_nothing_gives_none() {
        let earlier = probe("2026-10-21T15:00:00Z", r#"{"ufa":{"present":true,"provisioned":true,"sync":"blocked"}}"#);
        let no_record = Event { at: "2026-10-21T16:00:00Z".into(), kind: "status".into(), detail: "Status via N-central: no record\nstatus.json: -".into() };
        let cut_off = probe("2026-10-21T17:00:00Z", r#"{"ufa":{"present":true,"provi..."#);
        assert_eq!(device_ufa(&[earlier, no_record, cut_off]).unwrap().state, UfaState::Blocked);
        assert!(device_ufa(&[]).is_none());
        assert!(device_ufa(&[Event { at: "t".into(), kind: "poll".into(), detail: "seen".into() }]).is_none());
    }

    #[test]
    fn a_record_that_says_nothing_about_the_agent_gives_none() {
        assert!(device_ufa(&[probe("2026-10-21T15:00:00Z", r#"{"stage":"staged","attempts":1}"#)]).is_none());
    }
}
