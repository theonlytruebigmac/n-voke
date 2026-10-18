//! Following a run that was started elsewhere (for example by `Invoke-NableScriptRun.ps1`) by its
//! task ID. A port of the desktop app's "Watch a task" dialog.

use chrono::{DateTime, SecondsFormat, Utc};

use crate::{
    Core,
    api::asset_id_from_task_id,
    jobs::Job,
    model::TaskSnapshot,
    task_state::{JobTask, apply_snapshot},
};

/// A pasted task ID, as typed: trimmed, and without the quotes PowerShell prints around it.
pub fn clean_task_id(raw: &str) -> String {
    raw.trim().trim_matches(|c| c == '\'' || c == '"').trim().to_string()
}

/// The job for a task that already exists, built from what N-able says about it.
pub fn watched_job(task_id: &str, snapshot: &TaskSnapshot, device_name: &str, now: DateTime<Utc>) -> Job {
    let asset_id = asset_id_from_task_id(task_id);
    let task = apply_snapshot(&JobTask::new(task_id, asset_id.as_deref(), device_name, now.timestamp_millis()), snapshot);
    Job {
        id: task_id.to_string(),
        name: snapshot.name.clone().unwrap_or_else(|| "Untitled task".into()),
        script_id: snapshot.script.as_ref().map(|s| s.id.clone()),
        script: None,
        script_name: snapshot.script.as_ref().map_or_else(|| "Unknown script".into(), |s| s.name.clone()),
        org_id: None,
        org_name: None,
        target_label: device_name.to_string(),
        created_at: snapshot.created_at.clone().unwrap_or_else(|| now.to_rfc3339_opts(SecondsFormat::Millis, true)),
        timeout_seconds: 0,
        warnings: None,
        tasks: vec![task],
    }
}

/// Looks the task up and builds its job. The device's name comes from its ID (which the task ID contains).
pub async fn watch_task(core: &Core, raw_id: &str) -> Result<Job, String> {
    let id = clean_task_id(raw_id);
    if id.is_empty() {
        return Err("Paste a task ID first.".into());
    }
    let (snapshot, _partial) = core.get_task(&id).await?;
    let snapshot = snapshot.ok_or("No task found with that ID.")?;
    let device = match asset_id_from_task_id(&id) {
        Some(asset) => core.get_asset_name(&asset).await.ok().flatten(),
        None => None,
    };
    Ok(watched_job(&id, &snapshot, device.as_deref().unwrap_or("Unknown device"), Utc::now()))
}

#[cfg(test)]
mod tests {
    use base64::{Engine, engine::general_purpose::STANDARD as B64};
    use chrono::TimeZone;

    use super::*;
    use crate::model::{Execution, Executions, ScriptRef};

    fn snapshot(name: Option<&str>, status: &str, execution: Option<&str>) -> TaskSnapshot {
        let nodes = execution
            .map(|s| Some(Execution { id: "e".into(), status: s.into(), exit_code: None, error_message: None, started_at: None, updated_at: None, duration_milliseconds: None, output: None }))
            .into_iter()
            .collect();
        TaskSnapshot {
            id: "x".into(),
            name: name.map(String::from),
            status: status.into(),
            created_at: Some("2026-10-17T12:00:00Z".into()),
            script: Some(ScriptRef { id: "s1".into(), name: "Cleanup".into() }),
            executions: Some(Executions { nodes }),
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 18, 9, 0, 0).unwrap()
    }

    #[test]
    fn a_pasted_id_is_cleaned_of_whitespace_and_the_quotes_powershell_adds() {
        assert_eq!(clean_task_id("  abc123  "), "abc123");
        assert_eq!(clean_task_id("'abc123'"), "abc123");
        assert_eq!(clean_task_id("\"abc123\""), "abc123");
        assert_eq!(clean_task_id(" 'abc==' \n"), "abc==");
        assert_eq!(clean_task_id("a'b"), "a'b", "quotes inside are left alone");
        assert_eq!(clean_task_id("''"), "");
    }

    #[test]
    fn a_job_is_built_from_what_nable_says_about_the_task() {
        let id = B64.encode("task-uuid:asset-uuid");
        let job = watched_job(&id, &snapshot(Some("Weekly cleanup"), "IN_PROGRESS", Some("RUNNING")), "HP Laptop", now());
        assert_eq!((job.id.as_str(), job.name.as_str(), job.script_name.as_str()), (id.as_str(), "Weekly cleanup", "Cleanup"));
        assert_eq!((job.script_id.as_deref(), job.target_label.as_str(), job.created_at.as_str()), (Some("s1"), "HP Laptop", "2026-10-17T12:00:00Z"));
        assert!(job.org_id.is_none() && job.timeout_seconds == 0);
        let task = &job.tasks[0];
        assert_eq!((task.device_name.as_str(), task.asset_id.as_deref()), ("HP Laptop", Some("asset-uuid")), "the device comes from the id");
        assert!(task.watching, "it is still running, so it is followed");
    }

    #[test]
    fn a_task_that_already_finished_is_added_as_finished_and_not_watched() {
        let id = B64.encode("t:a");
        let job = watched_job(&id, &snapshot(None, "SUCCEEDED", Some("SUCCEEDED")), "PC", now());
        assert!(!job.tasks[0].watching);
        assert_eq!(job.tasks[0].execution.as_ref().unwrap().status, "SUCCEEDED");
        assert_eq!(job.name, "Untitled task");
    }

    #[test]
    fn unknown_script_and_time_get_sensible_stand_ins() {
        let mut bare = snapshot(None, "IN_PROGRESS", None);
        bare.script = None;
        bare.created_at = None;
        let job = watched_job("opaque-id", &bare, "Unknown device", now());
        assert_eq!((job.script_name.as_str(), job.script_id.as_deref()), ("Unknown script", None));
        assert_eq!(job.created_at, "2026-10-18T09:00:00.000Z", "now, when N-able gives no time");
        assert!(job.tasks[0].asset_id.is_none(), "an id that isn't base64 names no device");
    }

    #[tokio::test]
    async fn a_blank_id_is_refused_before_anything_is_sent() {
        let core = Core::for_tests("watch");
        assert_eq!(watch_task(&core, "  ''  ").await.unwrap_err(), "Paste a task ID first.");
    }
}
