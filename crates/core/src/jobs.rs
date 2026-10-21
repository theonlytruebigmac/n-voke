//! Jobs: one script started on one or more devices, plus how they are kept.
//! Serialized exactly as the desktop app saves them (the encrypted `jobs`
//! store), and retained by the same rules (`retention.ts`).

use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::{
    model::Script,
    task_state::{JobTask, is_done},
};

/// Stored output per device is capped so one chatty script can't bloat the history.
pub const MAX_STORED_OUTPUT: usize = 64 * 1024;
const TRUNCATED: &str = "\n… (output truncated in saved history)";
pub const MAX_JOBS: usize = 50;
/// Name of the history in the encrypted store.
pub const STORE_NAME: &str = "jobs";

/// One script started on one or more devices.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub name: String,
    pub script_id: Option<String>,
    /// Full script (with input declarations), so "Run again" can pre-fill the Run page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<Script>,
    pub script_name: String,
    pub org_id: Option<String>,
    pub org_name: Option<String>,
    /// Human summary of what was targeted, e.g. "PatchGroupTwo + 1 device".
    pub target_label: String,
    pub created_at: String,
    /// Script timeout used for the run; 0 when unknown (falls back to the API max).
    pub timeout_seconds: i64,
    /// Per-device problems reported when the run was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    /// The migration project that started it, so deleting the project removes its entries from Activity too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_project: Option<String>,
    pub tasks: Vec<JobTask>,
}

impl Job {
    /// Whether any of its tasks is still being watched.
    pub fn is_active(&self) -> bool {
        self.tasks.iter().any(|t| t.watching)
    }

    fn created_ms(&self) -> Option<i64> {
        DateTime::parse_from_rfc3339(&self.created_at).ok().map(|t| t.timestamp_millis())
    }
}

/// Reads saved history. Anything unreadable is treated as empty rather than an error.
pub fn parse_jobs(raw: Option<&str>) -> Vec<Job> {
    raw.and_then(|r| serde_json::from_str(r).ok()).unwrap_or_default()
}

/// Jobs to keep under a retention of `days` (0 = keep nothing on disk). Jobs
/// still being watched are always kept, whatever their age; `max` caps the
/// finished ones only, since dropping a watched job would lose it on restart.
pub fn retain(jobs: Vec<Job>, days: u32, now_ms: i64, max: usize) -> Vec<Job> {
    let cutoff = if days == 0 { None } else { Some(now_ms - days as i64 * 24 * 60 * 60 * 1000) };
    jobs.into_iter()
        .filter(|j| j.is_active() || cutoff.is_some_and(|c| j.created_ms().is_some_and(|t| t >= c)))
        .enumerate()
        .filter(|(i, j)| *i < max || j.is_active())
        .map(|(_, j)| j)
        .collect()
}

fn cap(text: &Option<String>) -> Option<String> {
    match text {
        Some(t) if t.len() > MAX_STORED_OUTPUT => {
            // Cut on a character boundary at or below the limit.
            let mut end = MAX_STORED_OUTPUT;
            while !t.is_char_boundary(end) {
                end -= 1;
            }
            Some(format!("{}{TRUNCATED}", &t[..end]))
        }
        other => other.clone(),
    }
}

/// A copy of the job with each device's output capped for storage.
pub fn cap_output(job: &Job) -> Job {
    let mut job = job.clone();
    for t in &mut job.tasks {
        if let Some(out) = t.execution.as_mut().and_then(|e| e.output.as_mut()) {
            out.std_output = cap(&out.std_output);
            out.std_error = cap(&out.std_error);
        }
    }
    job
}

/// Tasks still being watched when the app closed get a fresh watch window, and
/// unfinished tasks that stopped because of errors (not by choice) are picked up again.
pub fn rewatch(jobs: Vec<Job>, now_ms: i64) -> Vec<Job> {
    use crate::task_state::StopReason;
    jobs.into_iter()
        .map(|mut j| {
            for t in &mut j.tasks {
                if t.watching || (t.stop_reason == Some(StopReason::Errors) && !is_done(t)) {
                    t.watching = true;
                    t.stop_reason = None;
                    t.last_error = None;
                    t.watch_started_at = now_ms;
                    t.poll_failures = 0;
                }
            }
            j
        })
        .collect()
}

/// Newest first, as the history is kept.
pub fn sort_newest_first(jobs: &mut [Job]) {
    jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{Execution, ExecutionOutput},
        task_state::StopReason,
    };

    const DAY: i64 = 24 * 60 * 60 * 1000;
    const NOW: i64 = 1_792_238_400_000; // 2026-10-17T12:00:00Z

    fn task(watching: bool, stdout: Option<&str>) -> JobTask {
        JobTask {
            task_id: "t".into(),
            asset_id: Some("a".into()),
            device_name: "PC".into(),
            task_status: Some(if watching { "IN_PROGRESS" } else { "SUCCEEDED" }.into()),
            execution: stdout.map(|s| Execution {
                id: "e".into(),
                status: "SUCCEEDED".into(),
                exit_code: None,
                error_message: None,
                started_at: None,
                updated_at: None,
                duration_milliseconds: Some(1.0),
                output: Some(ExecutionOutput { run_as: None, std_output: Some(s.into()), std_error: None, outputs: None }),
            }),
            watching,
            watch_started_at: 0,
            poll_failures: 0,
            last_error: None,
            stop_reason: None,
        }
    }

    fn job(id: &str, age_days: f64, watching: bool) -> Job {
        let at = DateTime::from_timestamp_millis(NOW - (age_days * DAY as f64) as i64).unwrap();
        Job {
            id: id.into(),
            name: "run".into(),
            script_id: None,
            script: None,
            script_name: "S".into(),
            org_id: None,
            org_name: None,
            target_label: "PC".into(),
            created_at: at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            timeout_seconds: 0,
            warnings: None,
            migration_project: None,
            tasks: vec![task(watching, None)],
        }
    }

    fn ids(jobs: &[Job]) -> Vec<&str> {
        jobs.iter().map(|j| j.id.as_str()).collect()
    }

    #[test]
    fn drops_finished_jobs_older_than_the_retention_window() {
        let kept = retain(vec![job("new", 1.0, false), job("old", 40.0, false), job("edge", 29.9, false)], 30, NOW, 50);
        assert_eq!(ids(&kept), ["new", "edge"]);
    }

    #[test]
    fn always_keeps_jobs_that_are_still_being_watched() {
        assert_eq!(ids(&retain(vec![job("old-but-running", 400.0, true)], 7, NOW, 50)), ["old-but-running"]);
    }

    #[test]
    fn caps_the_number_of_jobs_without_dropping_ones_still_being_watched() {
        let jobs = vec![job("a", 0.0, false), job("b", 0.0, false), job("c", 0.0, true), job("d", 0.0, true), job("e", 0.0, false)];
        assert_eq!(ids(&retain(jobs, 30, NOW, 2)), ["a", "b", "c", "d"]);
    }

    #[test]
    fn keeps_nothing_finished_when_history_isnt_saved() {
        assert_eq!(ids(&retain(vec![job("a", 0.0, false), job("b", 0.0, true)], 0, NOW, 50)), ["b"]);
    }

    #[test]
    fn truncates_very_long_output_and_leaves_short_output_alone() {
        let long = "x".repeat(MAX_STORED_OUTPUT + 500);
        let mut j = job("a", 0.0, false);
        j.tasks = vec![task(false, Some(&long)), task(false, Some("short"))];
        let capped = cap_output(&j);
        let out = |i: usize| capped.tasks[i].execution.as_ref().unwrap().output.as_ref().unwrap().std_output.clone().unwrap();
        assert!(out(0).len() < long.len() && out(0).contains("truncated"));
        assert_eq!(out(1), "short");
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        let long = "é".repeat(MAX_STORED_OUTPUT); // 2 bytes each
        let mut j = job("a", 0.0, false);
        j.tasks = vec![task(false, Some(&long))];
        let capped = cap_output(&j);
        assert!(capped.tasks[0].execution.as_ref().unwrap().output.as_ref().unwrap().std_output.as_ref().unwrap().contains("truncated"));
    }

    #[test]
    fn rewatch_restarts_the_watch_window_and_picks_up_tasks_that_gave_up_on_errors() {
        let mut j = job("a", 0.0, false);
        let mut errored = task(false, None);
        errored.task_status = Some("IN_PROGRESS".into());
        errored.stop_reason = Some(StopReason::Errors);
        errored.last_error = Some("boom".into());
        let mut manual = errored.clone();
        manual.stop_reason = Some(StopReason::Manual);
        j.tasks = vec![errored, manual];
        let out = rewatch(vec![j], NOW);
        assert!(out[0].tasks[0].watching && out[0].tasks[0].stop_reason.is_none() && out[0].tasks[0].last_error.is_none());
        assert_eq!(out[0].tasks[0].watch_started_at, NOW);
        assert!(!out[0].tasks[1].watching, "a task stopped on purpose stays stopped");
    }

    /// History written by the desktop app (this is its JSON), read and written back unchanged in meaning.
    #[test]
    fn reads_and_rewrites_history_saved_by_the_desktop_app() {
        let saved = serde_json::json!([{
            "id": "abc", "name": "Run Cleanup", "scriptId": "s1",
            "script": { "id": "s1", "name": "Cleanup", "language": "POWERSHELL", "description": null, "timeoutSeconds": 120,
                        "inputs": [{ "name": "Pass", "variable": "pw", "type": "PASSWORD" }] },
            "scriptName": "Cleanup", "orgId": "o1", "orgName": "Acme", "targetLabel": "PatchGroupTwo + 1 device",
            "createdAt": "2026-10-17T12:00:00.000Z", "timeoutSeconds": 120, "warnings": ["PC-9 is offline"],
            "tasks": [{
                "taskId": "dDpB", "assetId": "a1", "deviceName": "PC-1", "taskStatus": "SUCCEEDED",
                "execution": { "id": "e1", "status": "SUCCEEDED", "exitCode": null, "errorMessage": null, "startedAt": "2026-10-17T12:00:05Z",
                               "updatedAt": "2026-10-17T12:00:09Z", "durationMilliseconds": 4200,
                               "output": { "runAs": "SYSTEM", "stdOutput": "all good", "stdError": null,
                                           "outputs": [{ "name": "Count", "variable": "n", "type": "INT", "value": { "intValue": 3 } }] } },
                "watching": false, "watchStartedAt": 1792238400000i64, "pollFailures": 0, "lastError": null, "stopReason": null
            }]
        }]);
        let text = saved.to_string();
        let jobs = parse_jobs(Some(&text));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].tasks[0].execution.as_ref().unwrap().duration_milliseconds, Some(4200.0));
        assert_eq!(jobs[0].script.as_ref().unwrap().inputs.as_ref().unwrap()[0].kind, "PASSWORD");
        // What we write back must mean the same thing to the desktop app.
        let again: serde_json::Value = serde_json::to_value(&jobs).unwrap();
        assert_eq!(again[0]["script"]["inputs"][0]["type"], "PASSWORD");
        assert_eq!(again[0]["tasks"][0]["execution"]["output"]["outputs"][0]["value"]["intValue"], 3);
        assert_eq!(again[0]["warnings"][0], "PC-9 is offline");
        assert_eq!(parse_jobs(Some(&again.to_string())), jobs);
    }

    #[test]
    fn unreadable_history_is_treated_as_empty() {
        assert!(parse_jobs(None).is_empty());
        assert!(parse_jobs(Some("not json")).is_empty());
        assert!(parse_jobs(Some("{}")).is_empty());
    }
}
