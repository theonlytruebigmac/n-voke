//! What the Launch step shows and does: the waves, what launching one would do, and launching it (or, in dry-run,
//! showing the requests without sending them). The rules for what goes where are `plan_wave` and
//! `migration_launch_wave`; this shapes them for a screen.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{
    launcher::{LaunchReport, plan_wave},
    packager::Package,
    project::{CleanMode, Project, Stage},
};
use crate::{Core, DryRunEntry};

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "WaveRow.ts"))]
#[serde(rename_all = "camelCase")]
pub struct WaveRow {
    pub number: u32,
    pub label: String,
    pub launched_at: Option<String>,
    pub devices: usize,
    /// Still `Planned`: these are what launching would send.
    pub waiting: usize,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "LaunchView.ts"))]
#[serde(rename_all = "camelCase")]
pub struct LaunchView {
    pub waves: Vec<WaveRow>,
    /// Planned devices that aren't in a wave yet.
    pub unassigned: usize,
    /// The project's wave size, the default for a new wave.
    pub wave_size: u32,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DeviceProblem.ts"))]
#[serde(rename_all = "camelCase")]
pub struct DeviceProblem {
    pub name: String,
    pub why: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DestinationCount.ts"))]
#[serde(rename_all = "camelCase")]
pub struct DestinationCount {
    pub name: String,
    pub devices: usize,
}

/// What launching a wave would do, for the confirmation.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "LaunchPreview.ts"))]
#[serde(rename_all = "camelCase")]
pub struct LaunchPreview {
    pub wave: u32,
    pub label: String,
    pub ecoverse: usize,
    pub rest: usize,
    pub by_destination: Vec<DestinationCount>,
    /// Devices that can't be launched as planned; launching reports them as failed.
    pub problems: Vec<DeviceProblem>,
    /// Things that stop the whole launch until fixed.
    pub stoppers: Vec<String>,
    pub destination_host: String,
    pub clean_full: bool,
    pub review_days: u32,
    pub reinstall: bool,
}

#[derive(Serialize, Clone, Debug)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "LaunchResult.ts"))]
#[serde(rename_all = "camelCase")]
pub struct LaunchResult {
    pub report: LaunchReport,
    /// In dry-run: what would have been sent, with passwords redacted. Empty for a real launch.
    pub dry_run: Vec<DryRunEntry>,
    pub was_dry_run: bool,
}

pub fn launch_view(project: &Project) -> LaunchView {
    let waves = project
        .waves
        .iter()
        .map(|w| {
            let members = project.devices.iter().filter(|d| d.wave == Some(w.number));
            WaveRow {
                number: w.number,
                label: w.label.clone(),
                launched_at: w.launched_at.clone(),
                devices: members.clone().count(),
                waiting: members.filter(|d| d.stage == Stage::Planned).count(),
            }
        })
        .collect();
    LaunchView { waves, unassigned: project.devices.iter().filter(|d| d.stage == Stage::Planned && d.wave.is_none()).count(), wave_size: project.options.wave_size }
}

/// What launching `wave` would do. `unit_name` turns a destination unit ID into a name.
pub fn launch_preview(project: &Project, wave: u32, package: &Package, dest_host: &str, unit_name: &dyn Fn(&str) -> String) -> Result<LaunchPreview, String> {
    let label = project.waves.iter().find(|w| w.number == wave).map(|w| w.label.clone()).ok_or_else(|| format!("There is no wave {wave} in this project."))?;
    let plan = plan_wave(project, wave);

    let mut by_destination: BTreeMap<String, usize> = BTreeMap::new();
    for group in &plan.ecoverse {
        *by_destination.entry(unit_name(&group.dest_org_unit_id)).or_default() += group.devices.len();
    }
    for &i in &plan.rest {
        if let Some(unit) = &project.devices[i].dest_org_unit_id {
            *by_destination.entry(unit_name(unit)).or_default() += 1;
        }
    }

    let mut stoppers = vec![];
    let ecoverse: usize = plan.ecoverse.iter().map(|g| g.devices.len()).sum();
    if ecoverse > 0 && project.ecoverse_script_version.as_deref() != Some(package.version.as_str()) {
        stoppers.push("The endpoint script in Ecoverse isn't this version. Publish it in Prepare first.".to_string());
    }
    if !plan.rest.is_empty() {
        match project.rest_scripts.get(&project.source_server_id) {
            None => stoppers.push("The endpoint script hasn't been uploaded to the source server for the REST path. Do that in Prepare first.".to_string()),
            Some(s) if s.version != package.version => stoppers.push(format!("The script uploaded to the source server is version {}, not {}. Upload the current one in Prepare first.", s.version, package.version)),
            Some(_) => {}
        }
    }
    if ecoverse + plan.rest.len() + plan.problems.len() == 0 {
        stoppers.push("No device in this wave is waiting to be launched.".to_string());
    }

    Ok(LaunchPreview {
        wave,
        label,
        ecoverse,
        rest: plan.rest.len(),
        by_destination: by_destination.into_iter().map(|(name, devices)| DestinationCount { name, devices }).collect(),
        problems: plan.problems.iter().map(|(i, why)| DeviceProblem { name: project.devices[*i].name.clone(), why: why.clone() }).collect(),
        stoppers,
        destination_host: dest_host.into(),
        clean_full: project.options.clean_mode == CleanMode::Full,
        review_days: project.options.review_days,
        reinstall: project.is_reinstall(),
    })
}

impl Core {
    pub async fn migration_launch_view(&self, id: &str) -> Result<LaunchView, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        Ok(launch_view(&project))
    }

    /// Puts the next waiting devices (up to `max`, name order) in a new wave.
    pub async fn migration_wave_create(&self, id: &str, instance: &str, label: &str, max: usize) -> Result<String, String> {
        if max == 0 {
            return Err("A wave needs at least one device.".into());
        }
        self.migration_wave_saved(id, instance, label, Some(max)).await.map(|(_, message)| message)
    }

    pub async fn migration_launch_preview(&self, id: &str, wave: u32) -> Result<LaunchPreview, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let dest = self.ncentral_server(&project.dest_server_id)?;
        // Names are a nicety: when the destination can't be read the ID is shown instead.
        let units = self.ncentral_org_units(&dest.id).await.unwrap_or_default();
        let name = |unit: &str| units.iter().find(|u| u.org_unit_id == unit).map_or_else(|| format!("#{unit}"), |u| u.org_unit_name.clone());
        launch_preview(&project, wave, &Package::embedded()?, &dest.host, &name)
    }

    /// Launches the wave. With `dry_run`, nothing is sent and the requests that would have been are returned.
    pub async fn migration_launch_saved(&self, id: &str, instance: &str, wave: u32, dry_run: bool) -> Result<LaunchResult, String> {
        let package = Package::embedded()?;
        let mut project = self.migration_open_for_write(id, instance).await?;
        let script_id = project.ecoverse_script_id.clone();

        let was_dry = self.is_dry_run();
        let held_before = self.dry_run_log().len();
        if dry_run {
            self.set_dry_run(true);
        }
        let report = self.migration_launch_wave(&mut project, wave, &package, script_id.as_deref()).await;
        if dry_run && !was_dry {
            self.set_dry_run(false);
        }
        let dry = self.is_dry_run() || dry_run;
        let held: Vec<DryRunEntry> = if dry { self.dry_run_log().into_iter().skip(held_before).collect() } else { vec![] };

        // What the launch changed is saved even when it stopped part-way: the devices it reached must not be sent again.
        self.migration_save(&mut project).await?;
        Ok(LaunchResult { report: report?, dry_run: held, was_dry_run: dry })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::{
        planner::{PlannedDevice, DeliveryPath},
        project::{DestEcoverse, RestScript},
    };

    fn planned(id: &str, dest: &str, path: DeliveryPath, asset: Option<&str>) -> PlannedDevice {
        PlannedDevice {
            device_id: id.into(),
            name: format!("PC-{id}"),
            source_customer_id: Some("10".into()),
            source_site_id: None,
            dest_org_unit_id: Some(dest.into()),
            path: Some(path),
            asset_id: asset.map(String::from),
            blockers: vec![],
            warnings: vec![],
            excluded: false,
        }
    }

    fn package(version: &str) -> Package {
        Package { version: version.into(), content: String::new() }
    }

    fn project() -> Project {
        let now = chrono::Utc::now();
        let mut p = Project::new("p".into(), "x", "src", "dst", DestEcoverse::NotLinked, now);
        p.add_planned(&[planned("1", "110", DeliveryPath::Ecoverse, Some("a1")), planned("2", "110", DeliveryPath::Rest, None), planned("3", "120", DeliveryPath::Ecoverse, Some("a3")), planned("4", "120", DeliveryPath::Rest, None)], now);
        p
    }

    #[test]
    fn waves_show_their_devices_and_how_many_still_wait_and_the_rest_are_unassigned() {
        let mut p = project();
        p.auto_wave("pilot", Some(2), chrono::Utc::now()).unwrap();
        p.devices[0].stage = Stage::Launched;
        let view = launch_view(&p);
        assert_eq!((view.unassigned, view.wave_size), (2, 50));
        assert_eq!(view.waves, [WaveRow { number: 1, label: "pilot".into(), launched_at: None, devices: 2, waiting: 1 }]);
    }

    #[test]
    fn a_preview_counts_by_path_and_destination_and_lists_what_stops_the_launch() {
        let mut p = project();
        p.auto_wave("", None, chrono::Utc::now()).unwrap();
        let name = |id: &str| format!("Unit {id}");
        let preview = launch_preview(&p, 1, &package("v1"), "new.example.com", &name).unwrap();
        assert_eq!((preview.ecoverse, preview.rest, preview.label.as_str()), (2, 2, "Wave 1"));
        assert_eq!(preview.by_destination, [DestinationCount { name: "Unit 110".into(), devices: 2 }, DestinationCount { name: "Unit 120".into(), devices: 2 }]);
        assert_eq!(preview.stoppers.len(), 2, "neither script is in place: {:?}", preview.stoppers);
        assert!(preview.clean_full && !preview.reinstall);

        p.ecoverse_script_version = Some("v1".into());
        p.rest_scripts.insert("src".into(), RestScript { item_id: "7".into(), version: "v0".into() });
        let stale = launch_preview(&p, 1, &package("v1"), "h", &name).unwrap();
        assert_eq!(stale.stoppers.len(), 1);
        assert!(stale.stoppers[0].contains("version v0, not v1"), "{:?}", stale.stoppers);

        p.rest_scripts.insert("src".into(), RestScript { item_id: "7".into(), version: "v1".into() });
        assert!(launch_preview(&p, 1, &package("v1"), "h", &name).unwrap().stoppers.is_empty());
        assert!(launch_preview(&p, 9, &package("v1"), "h", &name).is_err());
    }

    #[test]
    fn a_device_whose_launch_may_have_run_is_listed_as_a_problem_not_launched_again() {
        let mut p = project();
        p.auto_wave("", None, chrono::Utc::now()).unwrap();
        p.devices[1].launch_unsure = true;
        p.ecoverse_script_version = Some("v1".into());
        p.rest_scripts.insert("src".into(), RestScript { item_id: "7".into(), version: "v1".into() });
        let preview = launch_preview(&p, 1, &package("v1"), "h", &|id| id.to_string()).unwrap();
        assert_eq!((preview.rest, preview.problems.len()), (1, 1));
        assert_eq!(preview.problems[0].name, "PC-2");
    }

    #[tokio::test]
    async fn a_wave_needs_at_least_one_device_and_only_a_window_holding_the_lease_makes_it() {
        let core = Core::for_tests("waves");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let mut p = core.migration_create_full("W", &src.id, &dst.id, DestEcoverse::NotLinked, vec![], vec![]).await.unwrap();
        p.add_planned(&[planned("1", "110", DeliveryPath::Rest, None), planned("2", "110", DeliveryPath::Rest, None)], chrono::Utc::now());
        core.migration_save(&mut p).await.unwrap();

        assert!(core.migration_wave_create(&p.id, "me", "", 0).await.is_err());
        assert!(core.migration_wave_create(&p.id, "other", "", 1).await.is_ok());
        assert!(core.migration_wave_create(&p.id, "me", "", 1).await.unwrap_err().contains("Another N-voke window"));
        let view = core.migration_launch_view(&p.id).await.unwrap();
        assert_eq!((view.waves.len(), view.unassigned), (1, 1));
    }

    #[tokio::test]
    async fn a_dry_run_launch_sends_nothing_returns_the_held_back_requests_and_leaves_dry_run_as_it_was() {
        let core = Core::for_tests("drylaunch");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let mut p = core.migration_create_full("D", &src.id, &dst.id, DestEcoverse::NotLinked, vec![], vec![]).await.unwrap();
        p.add_planned(&[planned("1", "110", DeliveryPath::Ecoverse, Some("a1"))], chrono::Utc::now());
        p.auto_wave("", None, chrono::Utc::now()).unwrap();
        core.migration_save(&mut p).await.unwrap();
        // The script was never published, so the launch refuses before sending anything.
        let error = core.migration_launch_saved(&p.id, "me", 1, true).await.unwrap_err();
        assert!(error.contains("Publish the endpoint script"), "{error}");
        assert!(!core.is_dry_run(), "dry-run goes back to what it was");
    }
}
