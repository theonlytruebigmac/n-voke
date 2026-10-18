//! The bookkeeping of watching tasks, as pure functions over the job list:
//! which tasks have run out of time, which to ask about, and what to do with
//! the answer. A port of the logic in `jobs.ts`; `JobStore` does the I/O around it.

use std::collections::HashSet;

use crate::{
    api::TaskFetch,
    jobs::Job,
    task_state::{JobTask, StopReason, apply_snapshot, max_wait_seconds},
};

pub const POLL_INTERVAL_MS: u64 = 5000;
pub const MAX_POLL_FAILURES: u32 = 5;
/// Tasks fetched per request (aliased into one GraphQL query).
pub const POLL_BATCH_SIZE: usize = 20;

/// Stops watching tasks whose watch window has passed. Returns whether anything changed.
pub fn expire(jobs: &mut [Job], now_ms: i64) -> bool {
    let mut changed = false;
    for job in jobs {
        let limit = max_wait_seconds(job.timeout_seconds) * 1000;
        for t in &mut job.tasks {
            if t.watching && now_ms - t.watch_started_at >= limit {
                t.watching = false;
                t.stop_reason = Some(StopReason::Timeout);
                changed = true;
            }
        }
    }
    changed
}

/// Task ids to ask about, in job order, split into request-sized batches (each id once per batch).
pub fn watched_batches(jobs: &[Job]) -> Vec<Vec<String>> {
    let ids: Vec<&str> = jobs.iter().flat_map(|j| j.tasks.iter().filter(|t| t.watching).map(|t| t.task_id.as_str())).collect();
    ids.chunks(POLL_BATCH_SIZE)
        .map(|chunk| {
            let mut seen = HashSet::new();
            chunk.iter().filter(|id| seen.insert(**id)).map(|id| id.to_string()).collect()
        })
        .collect()
}

fn miss(t: &mut JobTask, message: String) {
    // Can be transient right after creation; give up only after repeated misses.
    t.poll_failures += 1;
    let give_up = t.poll_failures >= MAX_POLL_FAILURES;
    t.last_error = Some(message);
    t.watching = !give_up;
    if give_up {
        t.stop_reason = Some(StopReason::Errors);
    }
}

/// Applies the answer to one batch of task ids to every task still being watched among them.
pub fn apply_batch(jobs: &mut [Job], ids: &[String], result: Result<TaskFetch, String>) {
    let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
    for job in jobs {
        for t in &mut job.tasks {
            if !ids.contains(t.task_id.as_str()) || !t.watching {
                continue;
            }
            match &result {
                Ok(fetch) => match fetch.tasks.get(&t.task_id).and_then(Option::as_ref) {
                    None => miss(t, "The task wasn't found.".into()),
                    Some(snapshot) => {
                        // Partial errors (e.g. output not readable yet) are shown but don't stop polling.
                        let partial = (!fetch.errors.is_empty()).then(|| fetch.errors.join("; "));
                        *t = JobTask { last_error: partial, ..apply_snapshot(t, snapshot) };
                    }
                },
                Err(message) => miss(t, message.clone()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::model::{Execution, Executions, TaskSnapshot};

    fn task(id: &str, started: i64) -> JobTask {
        let mut t = JobTask::new(id, None, id, started);
        t.task_status = Some("IN_PROGRESS".into());
        t
    }

    fn job(id: &str, timeout: i64, tasks: Vec<JobTask>) -> Job {
        Job {
            id: id.into(),
            name: "run".into(),
            script_id: None,
            script: None,
            script_name: "S".into(),
            org_id: None,
            org_name: None,
            target_label: "x".into(),
            created_at: "2026-10-17T12:00:00Z".into(),
            timeout_seconds: timeout,
            warnings: None,
            tasks,
        }
    }

    fn snapshot(id: &str, task_status: &str, exec_status: Option<&str>) -> TaskSnapshot {
        let nodes = exec_status
            .map(|s| Some(Execution { id: "e".into(), status: s.into(), exit_code: None, error_message: None, started_at: None, updated_at: None, duration_milliseconds: None, output: None }))
            .into_iter()
            .collect();
        TaskSnapshot { id: id.into(), name: None, status: task_status.into(), created_at: None, script: None, executions: Some(Executions { nodes }) }
    }

    fn fetch(tasks: Vec<(&str, Option<TaskSnapshot>)>, errors: Vec<&str>) -> TaskFetch {
        TaskFetch { tasks: tasks.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<HashMap<_, _>>(), errors: errors.into_iter().map(String::from).collect() }
    }

    #[test]
    fn tasks_expire_after_the_scripts_timeout_plus_pickup_time() {
        // timeout 60s -> 360s window
        let mut jobs = vec![job("j", 60, vec![task("a", 0)])];
        assert!(!expire(&mut jobs, 359_000));
        assert!(jobs[0].tasks[0].watching);
        assert!(expire(&mut jobs, 360_000));
        assert!(!jobs[0].tasks[0].watching);
        assert_eq!(jobs[0].tasks[0].stop_reason, Some(StopReason::Timeout));
    }

    #[test]
    fn an_unknown_timeout_falls_back_to_the_apis_maximum() {
        let mut jobs = vec![job("j", 0, vec![task("a", 0)])];
        assert!(!expire(&mut jobs, 3_899_000));
        assert!(expire(&mut jobs, 3_900_000));
    }

    #[test]
    fn only_watched_tasks_are_asked_about_and_requests_are_batched() {
        let mut stopped = task("stopped", 0);
        stopped.watching = false;
        let many: Vec<JobTask> = (0..45).map(|i| task(&format!("t{i}"), 0)).collect();
        let batches = watched_batches(&[job("j1", 0, vec![stopped]), job("j2", 0, many)]);
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [20, 20, 5]);
        assert!(batches.iter().flatten().all(|id| id != "stopped"));
        assert_eq!(batches[0][0], "t0");
    }

    #[test]
    fn a_finished_snapshot_stops_watching_and_clears_old_errors() {
        let mut t = task("a", 0);
        t.last_error = Some("old".into());
        t.poll_failures = 3;
        let mut jobs = vec![job("j", 0, vec![t])];
        apply_batch(&mut jobs, &["a".into()], Ok(fetch(vec![("a", Some(snapshot("a", "SUCCEEDED", Some("SUCCEEDED"))))], vec![])));
        let t = &jobs[0].tasks[0];
        assert!(!t.watching && t.poll_failures == 0 && t.last_error.is_none());
        assert_eq!(t.execution.as_ref().unwrap().status, "SUCCEEDED");
    }

    #[test]
    fn partial_errors_are_shown_but_dont_stop_polling() {
        let mut jobs = vec![job("j", 0, vec![task("a", 0)])];
        apply_batch(&mut jobs, &["a".into()], Ok(fetch(vec![("a", Some(snapshot("a", "IN_PROGRESS", Some("RUNNING"))))], vec!["output not readable yet"])));
        assert!(jobs[0].tasks[0].watching);
        assert_eq!(jobs[0].tasks[0].last_error.as_deref(), Some("output not readable yet"));
    }

    #[test]
    fn a_missing_task_is_retried_and_given_up_on_after_repeated_misses() {
        let mut jobs = vec![job("j", 0, vec![task("a", 0)])];
        for attempt in 1..MAX_POLL_FAILURES {
            apply_batch(&mut jobs, &["a".into()], Ok(fetch(vec![("a", None)], vec![])));
            assert!(jobs[0].tasks[0].watching, "still watching after {attempt} misses");
        }
        apply_batch(&mut jobs, &["a".into()], Ok(fetch(vec![], vec![])));
        let t = &jobs[0].tasks[0];
        assert!(!t.watching);
        assert_eq!(t.stop_reason, Some(StopReason::Errors));
        assert_eq!(t.last_error.as_deref(), Some("The task wasn't found."));
    }

    #[test]
    fn request_failures_count_the_same_way_and_a_success_resets_the_count() {
        let mut jobs = vec![job("j", 0, vec![task("a", 0)])];
        apply_batch(&mut jobs, &["a".into()], Err("network down".into()));
        apply_batch(&mut jobs, &["a".into()], Err("network down".into()));
        assert_eq!((jobs[0].tasks[0].poll_failures, jobs[0].tasks[0].last_error.as_deref()), (2, Some("network down")));
        apply_batch(&mut jobs, &["a".into()], Ok(fetch(vec![("a", Some(snapshot("a", "IN_PROGRESS", None)))], vec![])));
        assert_eq!(jobs[0].tasks[0].poll_failures, 0);
        assert!(jobs[0].tasks[0].last_error.is_none());
    }

    #[test]
    fn tasks_outside_the_batch_or_no_longer_watched_are_left_alone() {
        let mut other = task("b", 0);
        other.watching = false;
        let mut jobs = vec![job("j", 0, vec![task("a", 0), task("c", 0), other])];
        apply_batch(&mut jobs, &["a".into(), "b".into()], Err("boom".into()));
        assert_eq!(jobs[0].tasks[0].poll_failures, 1);
        assert_eq!(jobs[0].tasks[1].poll_failures, 0, "not in the batch");
        assert_eq!(jobs[0].tasks[2].poll_failures, 0, "already stopped");
    }
}
