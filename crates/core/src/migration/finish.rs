//! What the Finish step shows and does: how far the project is, the report, and cleaning up healthy devices ahead of
//! their review period. Deleting the old source records isn't built (see the design's core gaps), so this says so
//! rather than offering it.

use serde::{Deserialize, Serialize};

use super::{
    packager::{Operation, Package},
    project::{Project, Stage},
    report::{Filter, Format},
};
use crate::Core;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "FinishView.ts"))]
#[serde(rename_all = "camelCase")]
pub struct FinishView {
    /// Devices being moved: everything except those left out of the project.
    pub total: usize,
    pub healthy: usize,
    pub cleaned_up: usize,
    pub retired: usize,
    pub resolved: usize,
    /// Still on their way: launched, staged, moving or on the new server.
    pub moving: usize,
    /// Planned but not launched.
    pub waiting: usize,
    pub needs_you: usize,
    pub closed: bool,
    pub review_days: u32,
    pub retire_source_records: bool,
    /// Healthy devices a cleanup would run on, by name: the ones Ecoverse is receiving asset data from, or that have no
    /// Ecoverse to wait for.
    pub cleanup_candidates: Vec<String>,
    /// Healthy in N-central but not sending asset data to Ecoverse yet. Cleanup leaves them alone: it removes the
    /// migration's record on the device, which is what you'd want to look at if Ecoverse never comes up.
    pub waiting_for_ecoverse: Vec<String>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "CleanupReport.ts"))]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub cleaned: usize,
    pub failed: Vec<CleanupFailure>,
    /// The person stopped it: devices not reached are still healthy and untouched.
    pub stopped: bool,
    pub not_reached: usize,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "CleanupState.ts"))]
#[serde(rename_all = "camelCase")]
pub enum CleanupState {
    Running,
    Cleaned,
    Failed,
}

/// One step of a bulk cleanup, for the page to show as it happens.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "CleanupProgress.ts"))]
#[serde(rename_all = "camelCase")]
pub struct CleanupProgress {
    pub name: String,
    /// 1-based, of `total`.
    pub position: usize,
    pub total: usize,
    pub state: CleanupState,
    pub why: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "CleanupFailure.ts"))]
#[serde(rename_all = "camelCase")]
pub struct CleanupFailure {
    pub name: String,
    pub why: String,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ReportFormat.ts"))]
#[serde(rename_all = "camelCase")]
pub enum ReportFormat {
    Html,
    Csv,
}

fn healthy_ready(project: &Project) -> impl Iterator<Item = &super::project::ProjectDevice> {
    project.devices.iter().filter(|d| d.stage == Stage::Healthy && !project.waiting_for_ecoverse(d))
}

pub fn finish_view(project: &Project) -> FinishView {
    let count = |stages: &[Stage]| project.devices.iter().filter(|d| stages.contains(&d.stage)).count();
    FinishView {
        total: project.devices.iter().filter(|d| d.stage != Stage::Excluded).count(),
        healthy: count(&[Stage::Healthy]),
        cleaned_up: count(&[Stage::CleanedUp]),
        retired: count(&[Stage::Retired]),
        resolved: count(&[Stage::Resolved]),
        moving: count(&[Stage::Launched, Stage::Staged, Stage::AgentRemoved, Stage::Registered]),
        waiting: count(&[Stage::Planned]),
        needs_you: project.attention_count(),
        closed: project.is_closed(),
        review_days: project.options.review_days,
        retire_source_records: project.options.retire_source_records,
        cleanup_candidates: healthy_ready(project).map(|d| d.name.clone()).collect(),
        waiting_for_ecoverse: project.devices.iter().filter(|d| project.waiting_for_ecoverse(d)).map(|d| d.name.clone()).collect(),
    }
}

impl Core {
    pub async fn migration_finish_view(&self, id: &str) -> Result<FinishView, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        Ok(finish_view(&project))
    }

    /// Saves a report of the project into the Downloads folder and returns where it went.
    pub async fn migration_finish_report(&self, id: &str, format: ReportFormat) -> Result<String, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let dir = dirs::download_dir().or_else(dirs::data_dir).ok_or("There is no Downloads folder to save the report in.")?;
        let format = match format {
            ReportFormat::Html => Format::Html,
            ReportFormat::Csv => Format::Csv,
        };
        self.migration_write_report(&project, &Filter::default(), format, &dir).map(|p| p.display().to_string())
    }

    /// Runs Cleanup on every healthy device that is fully working, one after another, saving after each so a stop
    /// part-way loses nothing. A device that fails is reported and the rest go on.
    pub async fn migration_cleanup_healthy(&self, id: &str, instance: &str) -> Result<CleanupReport, String> {
        self.migration_cleanup_healthy_with(id, instance, &|_| {}, &|| false).await
    }

    /// The same, telling `progress` as each device starts and finishes, and checking `cancelled` before each one.
    pub async fn migration_cleanup_healthy_with(&self, id: &str, instance: &str, progress: &(dyn Fn(CleanupProgress) + Sync), cancelled: &(dyn Fn() -> bool + Sync)) -> Result<CleanupReport, String> {
        let package = Package::embedded()?;
        let mut report = CleanupReport::default();
        let mut project = self.migration_open_for_write(id, instance).await?;
        let targets: Vec<usize> = project.devices.iter().enumerate().filter(|(_, d)| d.stage == Stage::Healthy && !project.waiting_for_ecoverse(d)).map(|(i, _)| i).collect();
        let total = targets.len();
        for (n, index) in targets.into_iter().enumerate() {
            if cancelled() {
                report.stopped = true;
                report.not_reached = total - n;
                break;
            }
            let name = project.devices[index].name.clone();
            let step = |state, why| CleanupProgress { name: name.clone(), position: n + 1, total, state, why };
            progress(step(CleanupState::Running, None));
            let outcome = self.migration_run_action(&mut project, index, Operation::Cleanup, &package, std::time::Duration::from_secs(120)).await;
            self.migration_save(&mut project).await?;
            match outcome {
                Ok(done) if done.result.result == "cleaned" || done.result.cleaned => {
                    report.cleaned += 1;
                    progress(step(CleanupState::Cleaned, None));
                }
                Ok(done) => {
                    let why = if done.result.message.is_empty() { format!("The device said {}.", done.result.result) } else { done.result.message };
                    progress(step(CleanupState::Failed, Some(why.clone())));
                    report.failed.push(CleanupFailure { name, why });
                }
                Err(why) => {
                    progress(step(CleanupState::Failed, Some(why.clone())));
                    report.failed.push(CleanupFailure { name, why });
                }
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::{
        planner::{DeliveryPath, PlannedDevice},
        project::{DestEcoverse, EcoState},
    };

    fn project(stages: &[Stage]) -> Project {
        let now = chrono::Utc::now();
        let mut p = Project::new("p".into(), "x", "a", "b", DestEcoverse::NotLinked, now);
        let planned: Vec<PlannedDevice> = (0..stages.len())
            .map(|i| PlannedDevice {
                device_id: i.to_string(),
                name: format!("PC-{i}"),
                source_customer_id: Some("1".into()),
                source_site_id: None,
                dest_org_unit_id: Some("2".into()),
                path: Some(DeliveryPath::Rest),
                asset_id: None,
                blockers: vec![],
                warnings: vec![],
                excluded: false,
            })
            .collect();
        p.add_planned(&planned, now);
        for (device, stage) in p.devices.iter_mut().zip(stages) {
            device.stage = *stage;
        }
        p
    }

    #[test]
    fn the_view_counts_where_everything_is_and_lists_what_a_cleanup_would_touch() {
        let p = project(&[Stage::Healthy, Stage::Healthy, Stage::CleanedUp, Stage::Retired, Stage::Staged, Stage::Planned, Stage::Lost, Stage::Excluded, Stage::Resolved]);
        let view = finish_view(&p);
        assert_eq!((view.total, view.healthy, view.cleaned_up, view.retired, view.resolved), (8, 2, 1, 1, 1));
        assert_eq!((view.moving, view.waiting, view.needs_you, view.closed), (1, 1, 1, false));
        assert_eq!(view.cleanup_candidates, ["PC-0", "PC-1"]);
    }

    #[test]
    fn a_project_where_everything_is_final_is_closed() {
        assert!(finish_view(&project(&[Stage::Retired, Stage::Excluded, Stage::Resolved])).closed);
        assert!(!finish_view(&project(&[])).closed, "an empty project isn't finished");
    }

    #[tokio::test]
    async fn cleanup_with_nothing_healthy_does_nothing_and_a_report_is_written_to_a_file() {
        let core = Core::for_tests("finish");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let p = core.migration_create_full("Fin", &src.id, &dst.id, DestEcoverse::NotLinked, vec![], vec![]).await.unwrap();
        let report = core.migration_cleanup_healthy(&p.id, "me").await.unwrap();
        assert_eq!((report.cleaned, report.failed.len()), (0, 0));
        assert!(core.migration_cleanup_healthy(&p.id, "other").await.unwrap_err().contains("Another N-voke window"));
    }

    #[test]
    fn a_device_waiting_for_ecoverse_data_is_not_offered_for_cleanup_and_is_listed_apart() {
        let mut p = project(&[Stage::Healthy, Stage::Healthy, Stage::Healthy, Stage::Healthy]);
        p.dest_ecoverse = DestEcoverse::SameTenant;
        p.devices[0].ecoverse = Some(EcoState::Reporting);
        p.devices[1].ecoverse = Some(EcoState::WaitingForData);
        p.devices[2].ecoverse = Some(EcoState::NotSeen);
        // The fourth has not been read yet.
        let view = finish_view(&p);
        assert_eq!((view.healthy, view.cleanup_candidates.clone()), (4, vec!["PC-0".to_string()]));
        assert_eq!(view.waiting_for_ecoverse, ["PC-1", "PC-2", "PC-3"]);
        assert_eq!(p.ecoverse_waiting(), 3);

        // With nothing in Ecoverse to wait for, every healthy device is ready.
        p.dest_ecoverse = DestEcoverse::NotLinked;
        let view = finish_view(&p);
        assert_eq!((view.cleanup_candidates.len(), view.waiting_for_ecoverse.len(), p.ecoverse_waiting()), (4, 0, 0));
    }

    async fn two_healthy_devices_without_a_destination_record() -> (std::sync::Arc<Core>, String) {
        let core = Core::for_tests("cleanup-progress");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let mut p = core.migration_create_full("Prog", &src.id, &dst.id, DestEcoverse::NotLinked, vec![], vec![]).await.unwrap();
        let mut healthy = project(&[Stage::Healthy, Stage::Healthy]);
        p.devices = std::mem::take(&mut healthy.devices);
        core.migration_save(&mut p).await.unwrap();
        (core, p.id)
    }

    #[tokio::test]
    async fn a_bulk_cleanup_tells_the_page_as_each_device_starts_and_finishes_and_goes_on_after_a_failure() {
        let (core, id) = two_healthy_devices_without_a_destination_record().await;
        let seen = std::sync::Mutex::new(vec![]);
        let report = core.migration_cleanup_healthy_with(&id, "me", &|p| seen.lock().unwrap().push((p.name.clone(), p.position, p.total, p.state)), &|| false).await.unwrap();
        assert_eq!((report.cleaned, report.failed.len(), report.stopped), (0, 2, false));
        assert_eq!(
            *seen.lock().unwrap(),
            [("PC-0".to_string(), 1, 2, CleanupState::Running), ("PC-0".to_string(), 1, 2, CleanupState::Failed), ("PC-1".to_string(), 2, 2, CleanupState::Running), ("PC-1".to_string(), 2, 2, CleanupState::Failed)]
        );
        assert!(report.failed[0].why.contains("hasn't been found on the destination"), "{}", report.failed[0].why);
    }

    #[tokio::test]
    async fn stopping_a_bulk_cleanup_finishes_the_device_in_hand_and_leaves_the_rest_untouched() {
        let (core, id) = two_healthy_devices_without_a_destination_record().await;
        let stop = std::sync::atomic::AtomicBool::new(false);
        let report = core
            .migration_cleanup_healthy_with(
                &id,
                "me",
                &|p| {
                    if p.state != CleanupState::Running {
                        stop.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                },
                &|| stop.load(std::sync::atomic::Ordering::SeqCst),
            )
            .await
            .unwrap();
        assert_eq!((report.failed.len(), report.stopped, report.not_reached), (1, true, 1));
        let saved = core.migration_load(&id).await.unwrap().unwrap();
        assert!(saved.devices.iter().all(|d| d.stage == Stage::Healthy), "the one not reached is still healthy");
    }
}
