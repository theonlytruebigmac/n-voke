//! Settings that belong to migration and are kept per machine: how often the tracker looks, how long each wait is before
//! it becomes a problem, and the options a new project starts with. They live in `config.json`; nothing here is secret.

use chrono::Duration;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{prepare::validate_options, project::ProjectOptions, tracker::Thresholds};
use crate::{Core, audit::AuditEntry};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "MigrationSettings.ts"))]
#[serde(rename_all = "camelCase", default)]
pub struct MigrationSettings {
    /// How often the tracker reads the devices of an open project.
    pub poll_seconds: u32,
    /// Launched on a connected device and still not running.
    pub not_picked_up_minutes: u32,
    /// Staged, but the old agent still checks in.
    pub stuck_staged_minutes: u32,
    /// The old agent is gone and nothing has appeared on the destination.
    pub lost_minutes: u32,
    /// Healthy in N-central but not in the destination's Ecoverse.
    pub ecoverse_not_moved_minutes: u32,
    /// A check-in this recent counts as checking in.
    pub healthy_checkin_minutes: u32,
    /// A source whose check-in is older than this has gone quiet.
    pub agent_quiet_minutes: u32,
    /// What a new project starts with.
    pub defaults: ProjectOptions,
}

impl Default for MigrationSettings {
    fn default() -> Self {
        Self {
            poll_seconds: 60,
            not_picked_up_minutes: 30,
            stuck_staged_minutes: 90,
            lost_minutes: 240,
            ecoverse_not_moved_minutes: 75,
            healthy_checkin_minutes: 10,
            agent_quiet_minutes: 10,
            defaults: ProjectOptions::default(),
        }
    }
}

impl MigrationSettings {
    /// Refuses values that would make the tracker useless or hammer the servers, naming the limit.
    pub fn validate(&self) -> Result<(), String> {
        let within = |name: &str, value: u32, low: u32, high: u32, unit: &str| {
            if (low..=high).contains(&value) { Ok(()) } else { Err(format!("{name} must be between {low} and {high} {unit}.")) }
        };
        within("The poll interval", self.poll_seconds, 15, 900, "seconds")?;
        within("Not picked up", self.not_picked_up_minutes, 5, 1440, "minutes")?;
        within("Stuck staged", self.stuck_staged_minutes, 10, 2880, "minutes")?;
        within("Lost", self.lost_minutes, 30, 10_080, "minutes")?;
        within("Ecoverse not moved", self.ecoverse_not_moved_minutes, 15, 2880, "minutes")?;
        within("A device checking in", self.healthy_checkin_minutes, 2, 120, "minutes")?;
        within("A source gone quiet", self.agent_quiet_minutes, 2, 120, "minutes")?;
        validate_options(&self.defaults)
    }

    pub fn thresholds(&self) -> Thresholds {
        Thresholds {
            not_picked_up: Duration::minutes(i64::from(self.not_picked_up_minutes)),
            data_pending_grace: Thresholds::default().data_pending_grace,
            stuck_staged: Duration::minutes(i64::from(self.stuck_staged_minutes)),
            lost: Duration::minutes(i64::from(self.lost_minutes)),
            ecoverse_not_moved: Duration::minutes(i64::from(self.ecoverse_not_moved_minutes)),
            healthy_checkin: Duration::minutes(i64::from(self.healthy_checkin_minutes)),
            agent_quiet: Duration::minutes(i64::from(self.agent_quiet_minutes)),
        }
    }
}

impl Core {
    pub fn migration_settings(&self) -> MigrationSettings {
        self.load_config().migration
    }

    /// Saves them after checking the limits. Takes effect on the next pass and the next project.
    pub fn save_migration_settings(&self, settings: MigrationSettings) -> Result<MigrationSettings, String> {
        settings.validate()?;
        let mut config = self.load_config();
        if config.migration == settings {
            return Ok(settings);
        }
        config.migration = settings.clone();
        self.save_config(&config)?;
        self.record(&AuditEntry::new("settings.migration", "Changed the migration settings", Ok(()), json!({ "pollSeconds": settings.poll_seconds })));
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_tracker_defaults_and_pass_their_own_checks() {
        let s = MigrationSettings::default();
        assert!(s.validate().is_ok());
        let t = s.thresholds();
        let d = Thresholds::default();
        assert_eq!((t.not_picked_up, t.stuck_staged, t.lost, t.ecoverse_not_moved, t.healthy_checkin, t.agent_quiet), (d.not_picked_up, d.stuck_staged, d.lost, d.ecoverse_not_moved, d.healthy_checkin, d.agent_quiet));
    }

    #[test]
    fn limits_are_named_and_a_default_option_out_of_range_is_refused_too() {
        let with = |f: &dyn Fn(&mut MigrationSettings)| {
            let mut s = MigrationSettings::default();
            f(&mut s);
            s.validate()
        };
        assert!(with(&|s| s.poll_seconds = 5).unwrap_err().contains("15 and 900 seconds"));
        assert!(with(&|s| s.lost_minutes = 10).unwrap_err().contains("Lost must be between 30"));
        assert!(with(&|s| s.defaults.wave_size = 0).unwrap_err().contains("wave size"));
        assert!(with(&|s| s.not_picked_up_minutes = 60).is_ok());
    }

    #[test]
    fn settings_are_saved_audited_read_back_and_an_old_config_without_them_gets_the_defaults() {
        let core = Core::for_tests("migsettings");
        assert_eq!(core.migration_settings(), MigrationSettings::default(), "a config written before these settings existed");
        let mut s = MigrationSettings::default();
        s.poll_seconds = 30;
        s.defaults.review_days = 14;
        core.save_migration_settings(s.clone()).unwrap();
        assert_eq!(core.migration_settings(), s);
        s.poll_seconds = 1;
        assert!(core.save_migration_settings(s).is_err());
        assert_eq!(core.migration_settings().poll_seconds, 30, "a refused change isn't kept");
    }

    #[tokio::test]
    async fn a_new_project_starts_with_the_default_options_and_the_poll_uses_the_thresholds() {
        let core = Core::for_tests("migsettings2");
        let mut s = MigrationSettings::default();
        s.defaults.review_days = 21;
        s.defaults.wave_size = 10;
        core.save_migration_settings(s).unwrap();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let project = core.migration_create("Defaults", &src.id, &dst.id, crate::migration::project::DestEcoverse::NotLinked).await.unwrap();
        assert_eq!((project.options.review_days, project.options.wave_size), (21, 10));
    }
}
