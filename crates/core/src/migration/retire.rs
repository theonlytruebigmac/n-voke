//! Retiring the old source records: after the review period, deleting a moved device's record from the source server.
//! Only the record goes (`removeAgents=false`): nothing is sent to the device, and a stale Ecoverse asset for it
//! disappears on its own. Off unless the project turns it on, and only for devices that have been cleaned up.
//! The review period is the default protection; the operator can knowingly retire before it ends (the page warns and
//! asks for a tick), which is recorded on each device and in the audit log.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use super::{
    finish::CleanupFailure,
    project::{Project, ProjectDevice, Stage},
};
use crate::{Core, DRY_RUN_ERROR, audit::AuditEntry};
use serde_json::json;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "RetireView.ts"))]
#[serde(rename_all = "camelCase")]
pub struct RetireView {
    /// The project's option to retire the old records is on.
    pub turned_on: bool,
    pub source_host: String,
    pub review_days: u32,
    /// Cleaned up, with the review period over: what retiring would delete, by name.
    pub ready: Vec<String>,
    /// Cleaned up, but still inside the review period.
    pub in_review: usize,
    /// The same devices by name, each with when its review period ends: what retiring before the end would add.
    pub waiting: Vec<RetireWaiting>,
    /// When the next device's review period ends, if any is still running.
    pub next_review_ends: Option<String>,
    /// Healthy devices aren't offered: they have to be cleaned up first.
    pub not_cleaned_up: usize,
}

/// A cleaned-up device whose review period is still running.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "RetireWaiting.ts"))]
#[serde(rename_all = "camelCase")]
pub struct RetireWaiting {
    pub name: String,
    pub review_ends: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "RetireReport.ts"))]
#[serde(rename_all = "camelCase")]
pub struct RetireReport {
    pub retired: usize,
    /// Records that were already gone from the server: the device counts as retired too.
    pub already_gone: usize,
    pub failed: Vec<CleanupFailure>,
    /// A dry run: the devices whose records would have been deleted. Nothing was sent.
    pub would_remove: Vec<String>,
}

/// When the device first became healthy, which is when its review period started.
fn review_started(device: &ProjectDevice) -> Option<DateTime<Utc>> {
    let at = device.events.iter().rev().find(|e| e.kind == "stage" && e.detail.starts_with("Healthy")).map(|e| e.at.as_str()).unwrap_or(&device.entered_stage_at);
    DateTime::parse_from_rfc3339(at).ok().map(|t| t.with_timezone(&Utc))
}

fn review_ends(device: &ProjectDevice, review_days: u32) -> Option<DateTime<Utc>> {
    review_started(device).map(|t| t + Duration::days(i64::from(review_days)))
}

fn is_ready(device: &ProjectDevice, review_days: u32, now: DateTime<Utc>) -> bool {
    device.stage == Stage::CleanedUp && review_ends(device, review_days).is_none_or(|end| end <= now)
}

pub fn retire_view(project: &Project, source_host: &str, now: DateTime<Utc>) -> RetireView {
    let days = project.options.review_days;
    let cleaned: Vec<&ProjectDevice> = project.devices.iter().filter(|d| d.stage == Stage::CleanedUp).collect();
    RetireView {
        turned_on: project.options.retire_source_records,
        source_host: source_host.into(),
        review_days: days,
        ready: cleaned.iter().filter(|d| is_ready(d, days, now)).map(|d| d.name.clone()).collect(),
        in_review: cleaned.iter().filter(|d| !is_ready(d, days, now)).count(),
        waiting: cleaned.iter().filter(|d| !is_ready(d, days, now)).map(|d| RetireWaiting { name: d.name.clone(), review_ends: review_ends(d, days).map(super::project::stamp) }).collect(),
        next_review_ends: cleaned.iter().filter(|d| !is_ready(d, days, now)).filter_map(|d| review_ends(d, days)).min().map(super::project::stamp),
        not_cleaned_up: project.devices.iter().filter(|d| d.stage == Stage::Healthy).count(),
    }
}

impl Core {
    pub async fn migration_retire_view(&self, id: &str) -> Result<RetireView, String> {
        let project = self.migration_load_shared(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let host = self.ncentral_server(&project.source_server_id).map(|s| s.host).unwrap_or_default();
        Ok(retire_view(&project, &host, Utc::now()))
    }

    /// Deletes the source record of every cleaned-up device whose review period is over, saving after each so that a
    /// stop part-way loses nothing. A failure is reported and the rest go on. With `before_review_ends` the cleaned-up
    /// devices still inside their review period go too: that is the operator's explicit choice to skip the wait, and
    /// it is written on each device and in the audit log.
    pub async fn migration_retire_sources(&self, id: &str, instance: &str, dry_run: bool, before_review_ends: bool) -> Result<RetireReport, String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        if !project.options.retire_source_records {
            return Err("Retiring the old records is off for this project. Turn it on in Prepare, under Options.".into());
        }
        let now = Utc::now();
        let days = project.options.review_days;
        let targets: Vec<usize> = project.devices.iter().enumerate().filter(|(_, d)| is_ready(d, days, now) || (before_review_ends && d.stage == Stage::CleanedUp)).map(|(i, _)| i).collect();
        let early: Vec<String> = targets.iter().filter(|&&i| !is_ready(&project.devices[i], days, now)).map(|&i| project.devices[i].name.clone()).collect();
        let mut report = RetireReport::default();
        let was_dry = self.is_dry_run();
        if dry_run {
            self.set_dry_run(true);
        }
        let host = self.ncentral_server(&project.source_server_id).map(|s| s.host).unwrap_or_default();
        for index in targets {
            let (device_id, name) = (project.devices[index].source_device_id.clone(), project.devices[index].name.clone());
            let before_review = early.contains(&name);
            match self.ncentral_delete_device_record(&project.source_server_id, &device_id, &name).await {
                Ok(existed) => {
                    let what = if existed { "the old source record was removed" } else { "the old source record was already gone" };
                    let why = if before_review { format!("{what}, before its review period ended (the operator chose to skip it)") } else { what.to_string() };
                    let _ = project.devices[index].move_to(Stage::Retired, Utc::now(), &why);
                    if existed {
                        report.retired += 1;
                    } else {
                        report.already_gone += 1;
                    }
                    self.migration_save(&mut project).await?;
                }
                Err(e) if e.message == DRY_RUN_ERROR => report.would_remove.push(name),
                Err(e) => report.failed.push(CleanupFailure { name, why: e.message }),
            }
        }
        if dry_run && !was_dry {
            self.set_dry_run(false);
        }
        if report.retired + report.already_gone > 0 {
            let before_review_done = early.iter().filter(|n| project.devices.iter().any(|d| &d.name == *n && d.stage == Stage::Retired)).count();
            self.record(&AuditEntry::new(
                "migration.retire.done",
                &if before_review_done > 0 {
                    format!("Retired the old source records of {} on {host}, {before_review_done} of them before their review period ended", project.name)
                } else {
                    format!("Retired the old source records of {} on {host}", project.name)
                },
                Ok(()),
                json!({ "project": project.id, "retired": report.retired, "alreadyGone": report.already_gone, "failed": report.failed.len(), "beforeReviewEnded": before_review_done }),
            ));
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        migration::project::DestEcoverse,
        ncentral::rest::testing::{Fake, auth_ok, status},
    };

    fn project(stages: &[Stage], healthy_days_ago: i64) -> Project {
        let now = Utc::now();
        let mut p = Project::new("p".into(), "Move", "a", "b", DestEcoverse::NotLinked, now);
        p.options.review_days = 7;
        p.options.retire_source_records = true;
        for (i, stage) in stages.iter().enumerate() {
            let planned = crate::migration::planner::PlannedDevice {
                device_id: format!("{}", 9000 + i),
                name: format!("PC-{i}"),
                source_customer_id: Some("10".into()),
                source_site_id: None,
                dest_org_unit_id: Some("110".into()),
                path: Some(crate::migration::planner::DeliveryPath::Rest),
                asset_id: None,
                blockers: vec![],
                warnings: vec![],
                excluded: false,
            };
            p.add_planned(&[planned], now);
            let device = p.devices.last_mut().unwrap();
            device.stage = *stage;
            let at = crate::migration::project::stamp(now - Duration::days(healthy_days_ago));
            device.entered_stage_at = at.clone();
            device.events.push(crate::migration::project::Event { at, kind: "stage".into(), detail: "Healthy: seen on the new server".into() });
        }
        p
    }

    #[test]
    fn only_cleaned_up_devices_past_their_review_period_are_ready_and_the_rest_are_counted() {
        let now = Utc::now();
        let over = project(&[Stage::CleanedUp, Stage::Healthy, Stage::Retired], 8);
        let view = retire_view(&over, "old.example.com", now);
        assert_eq!((view.ready.clone(), view.in_review, view.not_cleaned_up, view.turned_on), (vec!["PC-0".to_string()], 0, 1, true));

        let within = project(&[Stage::CleanedUp, Stage::CleanedUp], 2);
        let view = retire_view(&within, "old.example.com", now);
        assert_eq!((view.ready.len(), view.in_review), (0, 2));
        assert!(view.next_review_ends.is_some());

        let mut none = project(&[Stage::CleanedUp], 0);
        none.options.review_days = 0;
        assert_eq!(retire_view(&none, "h", now).ready.len(), 1, "no review period means ready at once");
    }

    async fn core_with(fake: &std::sync::Arc<Fake>, p: &mut Project) -> std::sync::Arc<Core> {
        let core = Core::for_tests("retire");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        core.set_rest_transport(fake.clone());
        p.source_server_id = src.id;
        p.dest_server_id = dst.id;
        core
    }

    fn lab(unlucky: &'static str, gone: &'static str) -> std::sync::Arc<Fake> {
        Fake::new(move |req, _| {
            let path = req.url.trim_start_matches("https://").split_once('/').unwrap().1.to_string();
            if path.starts_with("api/auth/authenticate") {
                return auth_ok(3600);
            }
            let id = path.strip_prefix("api/devices/").and_then(|p| p.split('?').next()).unwrap_or("").to_string();
            if id == unlucky {
                status(500, r#"{"message":"boom"}"#)
            } else if id == gone {
                status(404, r#"{"message":"No such device"}"#)
            } else {
                status(204, "")
            }
        })
    }

    #[tokio::test]
    async fn retiring_deletes_the_record_only_marks_the_devices_and_goes_on_after_a_failure_or_a_record_already_gone() {
        let fake = lab("9001", "9002");
        let mut p = project(&[Stage::CleanedUp, Stage::CleanedUp, Stage::CleanedUp, Stage::Healthy], 9);
        let core = core_with(&fake, &mut p).await;
        core.migration_save(&mut p).await.unwrap();

        let report = core.migration_retire_sources(&p.id, "me", false, false).await.unwrap();
        assert_eq!((report.retired, report.already_gone, report.failed.len()), (1, 1, 1));
        assert_eq!(report.failed[0].name, "PC-1");

        let deletes: Vec<String> = fake.paths().into_iter().filter(|p| p.starts_with("DELETE")).collect();
        assert_eq!(deletes, ["DELETE api/devices/9000?removeAgents=false", "DELETE api/devices/9001?removeAgents=false", "DELETE api/devices/9002?removeAgents=false"]);

        let saved = core.migration_load(&p.id).await.unwrap().unwrap();
        let stages: Vec<Stage> = saved.devices.iter().map(|d| d.stage).collect();
        assert_eq!(stages, [Stage::Retired, Stage::CleanedUp, Stage::Retired, Stage::Healthy], "a failed one stays, and a healthy one is never touched");
    }

    #[tokio::test]
    async fn retiring_is_refused_while_the_option_is_off_and_a_dry_run_sends_nothing() {
        let fake = lab("", "");
        let mut p = project(&[Stage::CleanedUp], 9);
        p.options.retire_source_records = false;
        let core = core_with(&fake, &mut p).await;
        core.migration_save(&mut p).await.unwrap();
        assert!(core.migration_retire_sources(&p.id, "me", false, false).await.unwrap_err().contains("Turn it on in Prepare"));

        p.options.retire_source_records = true;
        core.migration_save(&mut p).await.unwrap();
        let report = core.migration_retire_sources(&p.id, "me", true, false).await.unwrap();
        assert_eq!((report.retired, report.would_remove.clone()), (0, vec!["PC-0".to_string()]));
        assert!(!core.is_dry_run(), "the switch is put back");
        assert!(fake.paths().iter().all(|p| !p.starts_with("DELETE")));
        let saved = core.migration_load(&p.id).await.unwrap().unwrap();
        assert_eq!(saved.devices[0].stage, Stage::CleanedUp);
    }

    #[test]
    fn the_view_lists_the_devices_still_in_review_with_when_each_review_ends() {
        let p = project(&[Stage::CleanedUp, Stage::CleanedUp, Stage::Healthy], 2);
        let view = retire_view(&p, "old.example.com", Utc::now());
        assert!(view.ready.is_empty());
        assert_eq!(view.waiting.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["PC-0", "PC-1"]);
        assert!(view.waiting.iter().all(|w| w.review_ends.is_some()));
        assert_eq!(view.in_review, view.waiting.len());
    }

    #[tokio::test]
    async fn devices_in_their_review_period_are_left_alone_unless_the_operator_chooses_to_skip_it() {
        let fake = lab("", "");
        let mut p = project(&[Stage::CleanedUp, Stage::CleanedUp, Stage::Healthy], 2);
        let core = core_with(&fake, &mut p).await;
        core.migration_save(&mut p).await.unwrap();

        let report = core.migration_retire_sources(&p.id, "me", false, false).await.unwrap();
        assert_eq!((report.retired, report.already_gone, report.failed.len()), (0, 0, 0), "the default keeps the review period");
        assert!(fake.paths().iter().all(|p| !p.starts_with("DELETE")));

        let report = core.migration_retire_sources(&p.id, "me", false, true).await.unwrap();
        assert_eq!(report.retired, 2);
        let deletes: Vec<String> = fake.paths().into_iter().filter(|p| p.starts_with("DELETE")).collect();
        assert_eq!(deletes, ["DELETE api/devices/9000?removeAgents=false", "DELETE api/devices/9001?removeAgents=false"]);
        let saved = core.migration_load(&p.id).await.unwrap().unwrap();
        let stages: Vec<Stage> = saved.devices.iter().map(|d| d.stage).collect();
        assert_eq!(stages, [Stage::Retired, Stage::Retired, Stage::Healthy], "a healthy device is never retired, early or not");
        assert!(saved.devices[0].events.last().unwrap().detail.contains("before its review period ended"), "{:?}", saved.devices[0].events.last());
    }

    #[tokio::test]
    async fn skipping_the_review_period_only_adds_what_is_still_in_it_and_a_dry_run_of_it_sends_nothing() {
        let fake = lab("", "");
        let mut ready_and_waiting = project(&[Stage::CleanedUp], 9);
        let core = core_with(&fake, &mut ready_and_waiting).await;
        let in_review = project(&[Stage::CleanedUp], 1).devices.remove(0);
        let mut waiting = in_review.clone();
        waiting.name = "PC-waiting".into();
        waiting.source_device_id = "9100".into();
        ready_and_waiting.devices.push(waiting);
        core.migration_save(&mut ready_and_waiting).await.unwrap();

        let dry = core.migration_retire_sources(&ready_and_waiting.id, "me", true, true).await.unwrap();
        assert_eq!(dry.would_remove, ["PC-0", "PC-waiting"]);
        assert!(fake.paths().iter().all(|p| !p.starts_with("DELETE")));
        let saved = core.migration_load(&ready_and_waiting.id).await.unwrap().unwrap();
        assert!(saved.devices.iter().all(|d| d.stage == Stage::CleanedUp), "a dry run changes nothing");

        let report = core.migration_retire_sources(&ready_and_waiting.id, "me", false, true).await.unwrap();
        assert_eq!(report.retired, 2);
        let saved = core.migration_load(&ready_and_waiting.id).await.unwrap().unwrap();
        assert!(!saved.devices[0].events.last().unwrap().detail.contains("before its review period ended"), "the one whose period was over is not marked as early");
        assert!(saved.devices[1].events.last().unwrap().detail.contains("before its review period ended"));
    }
}
