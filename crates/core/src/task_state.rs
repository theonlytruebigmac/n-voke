//! Per-device task state: what phase a task is in, how it ended, and how many
//! devices with identical results collapse into one group. A port of the
//! desktop app's `taskState.ts`; the rules (and the N-able quirks behind them)
//! are the same, and so are the tests.

use std::{collections::HashMap, sync::LazyLock};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::model::{Execution, TaskSnapshot};

const PICKUP_GRACE_SECONDS: i64 = 300;
const API_MAX_TIMEOUT_SECONDS: i64 = 3600;

/// Statuses that mean "not finished yet" for both tasks and executions.
fn is_pending(status: &str) -> bool {
    matches!(status, "IN_PROGRESS" | "PENDING" | "QUEUED" | "SCHEDULED" | "CREATED" | "RUNNING" | "STARTED")
}

/// Execution statuses that mean the device hasn't started the script yet.
fn not_started(status: &str) -> bool {
    matches!(status, "PENDING" | "QUEUED" | "SCHEDULED" | "CREATED")
}

static CANCELLED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)CANCEL").unwrap());
static TIMED_OUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)timed out").unwrap());
/// N-able's message when a PowerShell script exits non-zero and its output was captured.
static PARSE_ERROR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)parsing the powershell output").unwrap());
static EXIT_STATUS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)exit (?:status|code)[:\s]+(-?\d+)").unwrap());

fn ended(status: Option<&str>) -> bool {
    status.is_some_and(|s| !s.is_empty() && !is_pending(s))
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StopReason {
    Timeout,
    Errors,
    Manual,
}

/// One device's task within a job. Serialized as the desktop app saves it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobTask {
    pub task_id: String,
    pub asset_id: Option<String>,
    pub device_name: String,
    pub task_status: Option<String>,
    pub execution: Option<Execution>,
    pub watching: bool,
    /// When the current watch window began (ms since the epoch); reset on resume.
    #[serde(default)]
    pub watch_started_at: i64,
    #[serde(default)]
    pub poll_failures: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Why watching stopped without a final result, if it did.
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
}

impl JobTask {
    pub fn new(task_id: &str, asset_id: Option<&str>, device_name: &str, now_ms: i64) -> Self {
        let mut t = Self {
            task_id: task_id.into(),
            asset_id: asset_id.map(String::from),
            device_name: device_name.into(),
            task_status: Some("IN_PROGRESS".into()),
            execution: None,
            watching: true,
            watch_started_at: now_ms,
            poll_failures: 0,
            last_error: None,
            stop_reason: None,
        };
        if is_done(&t) {
            t.watching = false;
        }
        t
    }

    fn execution_status(&self) -> Option<&str> {
        self.execution.as_ref().map(|e| e.status.as_str())
    }
}

/// "NonZero": the script ran but exited with a non-zero code, which N-able
/// records as Failed. "Failed": it didn't run or didn't finish (error, timeout).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskPhase {
    Queued,
    Running,
    Succeeded,
    NonZero,
    Failed,
    Stopped,
}

impl TaskPhase {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::NonZero | Self::Failed)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Waiting for device",
            Self::Running => "Running",
            Self::Succeeded => "Succeeded",
            Self::NonZero => "Non-zero exit",
            Self::Failed => "Didn't run",
            Self::Stopped => "Not watching",
        }
    }
}

/// Finished when the execution finishes, or when the task itself ends (e.g.
/// cancelled or expired in N-able) while its execution never started, so no
/// output is coming. A task that ends while its script is still running is
/// watched until the execution catches up, unless it was cancelled.
pub fn is_done(t: &JobTask) -> bool {
    if ended(t.execution_status()) {
        return true;
    }
    if !ended(t.task_status.as_deref()) {
        return false;
    }
    match &t.execution {
        None => true,
        Some(e) => not_started(&e.status) || CANCELLED.is_match(t.task_status.as_deref().unwrap_or("")),
    }
}

/// The status a finished task ended with: the execution's, unless the task ended first.
pub fn final_status(t: &JobTask) -> Option<&str> {
    if ended(t.execution_status()) {
        return t.execution_status();
    }
    t.task_status.as_deref().or(t.execution_status())
}

pub fn task_phase(t: &JobTask) -> TaskPhase {
    if is_done(t) {
        if final_status(t) == Some("SUCCEEDED") {
            return TaskPhase::Succeeded;
        }
        return if task_outcome(t).is_some_and(|o| o.kind == OutcomeKind::NonZeroExit) { TaskPhase::NonZero } else { TaskPhase::Failed };
    }
    if !t.watching {
        return TaskPhase::Stopped;
    }
    if t.execution.is_some() { TaskPhase::Running } else { TaskPhase::Queued }
}

/// How long to keep watching a task: its script timeout (or the API's maximum) plus time for the device to pick it up.
pub fn max_wait_seconds(timeout_seconds: i64) -> i64 {
    (if timeout_seconds > 0 { timeout_seconds } else { API_MAX_TIMEOUT_SECONDS }) + PICKUP_GRACE_SECONDS
}

/// Applies a fresh task snapshot from the API to a stored task.
pub fn apply_snapshot(t: &JobTask, snapshot: &TaskSnapshot) -> JobTask {
    let execution = snapshot.executions.as_ref().and_then(|e| e.nodes.iter().flatten().next().cloned());
    let mut next = JobTask { task_status: Some(snapshot.status.clone()), execution, poll_failures: 0, ..t.clone() };
    next.watching = t.watching && !is_done(&next);
    next
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    Passed,
    NonZeroExit,
    TimedOut,
    Cancelled,
    Error,
}

/// How a finished task ended, from the script's point of view. N-able marks a
/// script Failed whenever it exits with a non-zero code, even when it ran
/// normally and produced its report, and it never fills in `exitCode` for
/// script runs; the code only sometimes appears in the error text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub kind: OutcomeKind,
    /// The script started and ran (as opposed to never running).
    pub ran: bool,
    /// Exit code if N-able reported it anywhere.
    pub exit_code: Option<i64>,
    pub has_output: bool,
}

pub fn task_outcome(t: &JobTask) -> Option<Outcome> {
    if !is_done(t) {
        return None;
    }
    let exec = t.execution.as_ref();
    let message = exec.and_then(|e| e.error_message.as_deref()).unwrap_or("");
    let out = exec.and_then(|e| e.output.as_ref());
    let has_output = out.is_some_and(|o| {
        o.std_output.as_deref().is_some_and(|s| !s.trim().is_empty())
            || o.std_error.as_deref().is_some_and(|s| !s.trim().is_empty())
            || o.outputs.as_ref().is_some_and(|v| !v.is_empty())
    });
    let reported = exec.and_then(|e| e.exit_code).or_else(|| EXIT_STATUS.captures(message).and_then(|c| c[1].parse().ok()));
    let status = final_status(t).unwrap_or("");

    let outcome = |kind, ran, exit_code| Some(Outcome { kind, ran, exit_code, has_output });
    if status == "SUCCEEDED" {
        return outcome(OutcomeKind::Passed, true, Some(reported.unwrap_or(0)));
    }
    if CANCELLED.is_match(status) {
        return outcome(OutcomeKind::Cancelled, has_output, reported);
    }
    if TIMED_OUT.is_match(message) || status == "TIMED_OUT" {
        return outcome(OutcomeKind::TimedOut, exec.is_some(), reported);
    }
    if reported.is_some_and(|c| c != 0) || PARSE_ERROR.is_match(message) {
        return outcome(OutcomeKind::NonZeroExit, true, reported);
    }
    // Failed without a recognisable reason: if it produced output it still ran.
    outcome(if has_output { OutcomeKind::NonZeroExit } else { OutcomeKind::Error }, has_output, reported)
}

/// Whether N-able's error text is its generic wording for a script that ran and exited non-zero:
/// it says "something went wrong while parsing the PowerShell output" whatever the script did.
pub fn is_generic_exit_message(message: &str) -> bool {
    PARSE_ERROR.is_match(message)
}

/// What a finished task's ending means in plain words, for the endings that confuse. N-able calls
/// a script "failed" whenever it exits non-zero, even when it ran normally and printed its report,
/// and its message then talks about "parsing the output", which makes a normal run look broken.
pub fn outcome_explanation(o: &Outcome) -> Option<String> {
    match o.kind {
        OutcomeKind::NonZeroExit => Some(match o.exit_code {
            Some(code) => format!("The script ran and finished with exit code {code}. N-able counts any non-zero exit code as a failure; the output above is the script's own."),
            None => "The script ran and finished with a non-zero exit code. N-able counts that as a failure; the output above is the script's own.".into(),
        }),
        OutcomeKind::Cancelled => Some("The task was cancelled in N-able.".into()),
        OutcomeKind::TimedOut => Some("The script didn't finish in time and was stopped.".into()),
        OutcomeKind::Passed | OutcomeKind::Error => None,
    }
}

/// Short label for a finished task, e.g. "Exit 1001".
pub fn outcome_label(o: &Outcome) -> String {
    match o.kind {
        OutcomeKind::Passed => "Succeeded".into(),
        OutcomeKind::NonZeroExit => o.exit_code.map_or("Non-zero exit".into(), |c| format!("Exit {c}")),
        OutcomeKind::TimedOut => "Timed out".into(),
        OutcomeKind::Cancelled => "Cancelled".into(),
        OutcomeKind::Error => "Didn't run".into(),
    }
}

/// Short status for one device: "Passed", "Exit 1001", "Waiting"...
pub fn task_status_word(t: &JobTask) -> String {
    let phase = task_phase(t);
    match (phase, task_outcome(t)) {
        (TaskPhase::NonZero | TaskPhase::Failed, Some(o)) => outcome_label(&o),
        (TaskPhase::Queued, _) => "Waiting".into(),
        (TaskPhase::Running, _) => "Running".into(),
        (TaskPhase::Succeeded, _) => "Passed".into(),
        (TaskPhase::NonZero, _) => "Non-zero exit".into(),
        (TaskPhase::Failed, _) => "Didn't run".into(),
        (TaskPhase::Stopped, _) => "Not watching".into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PhaseCounts {
    pub queued: usize,
    pub running: usize,
    pub succeeded: usize,
    pub non_zero: usize,
    pub failed: usize,
    pub stopped: usize,
}

impl PhaseCounts {
    pub fn total(&self) -> usize {
        self.queued + self.running + self.succeeded + self.non_zero + self.failed + self.stopped
    }
}

pub fn count_phases(tasks: &[JobTask]) -> PhaseCounts {
    let mut c = PhaseCounts::default();
    for t in tasks {
        match task_phase(t) {
            TaskPhase::Queued => c.queued += 1,
            TaskPhase::Running => c.running += 1,
            TaskPhase::Succeeded => c.succeeded += 1,
            TaskPhase::NonZero => c.non_zero += 1,
            TaskPhase::Failed => c.failed += 1,
            TaskPhase::Stopped => c.stopped += 1,
        }
    }
    c
}

/// Devices with the same outcome. `tasks` are indexes into the slice that was grouped.
#[derive(Clone, Debug)]
pub struct ResultGroup {
    pub key: String,
    pub phase: TaskPhase,
    /// Final status for finished groups (e.g. FAILED, TIMED_OUT).
    pub status: Option<String>,
    /// Representative output, with the device's own name replaced by {device}.
    pub stdout: String,
    pub stderr: String,
    pub error_message: String,
    pub tasks: Vec<usize>,
}

fn normalize(text: Option<&str>, device_name: &str) -> String {
    let Some(text) = text else { return String::new() };
    static TRAILING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)[ \t]+$").unwrap());
    static CRLF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r\n?").unwrap());
    let unified = CRLF.replace_all(text, "\n");
    let mut out = TRAILING.replace_all(&unified, "").trim().to_string();
    if !device_name.is_empty() {
        out = out.replace(device_name, "{device}");
    }
    out
}

/// Groups tasks by outcome so many devices with identical results read as one
/// line. Output is compared after normalizing line endings, trailing spaces and
/// the device's own name (scripts often echo the hostname). Unfinished groups
/// come first, then finished groups by size.
pub fn group_results(tasks: &[JobTask]) -> Vec<ResultGroup> {
    let mut groups: Vec<ResultGroup> = vec![];
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for (i, t) in tasks.iter().enumerate() {
        let phase = task_phase(t);
        let done = phase.is_finished();
        let out = t.execution.as_ref().and_then(|e| e.output.as_ref());
        let (stdout, stderr, error_message) = if done {
            (
                normalize(out.and_then(|o| o.std_output.as_deref()), &t.device_name),
                normalize(out.and_then(|o| o.std_error.as_deref()), &t.device_name),
                normalize(t.execution.as_ref().and_then(|e| e.error_message.as_deref()), &t.device_name),
            )
        } else {
            Default::default()
        };
        let status = if done { final_status(t).map(String::from) } else { None };
        let key = if done { [status.as_deref().unwrap_or("null"), &error_message, &stdout, &stderr].join("\u{0}") } else { format!("{phase:?}") };

        match by_key.get(&key) {
            Some(&g) => groups[g].tasks.push(i),
            None => {
                by_key.insert(key.clone(), groups.len());
                groups.push(ResultGroup { key, phase, status, stdout, stderr, error_message, tasks: vec![i] });
            }
        }
    }
    // Still-running things first (running, queued, stopped), then finished groups by size. The sort is stable.
    let rank = |p: TaskPhase| match p {
        TaskPhase::Running => Some(0),
        TaskPhase::Queued => Some(1),
        TaskPhase::Stopped => Some(2),
        _ => None,
    };
    groups.sort_by(|a, b| match (rank(a.phase), rank(b.phase)) {
        (None, None) => b.tasks.len().cmp(&a.tasks.len()),
        (x, y) => x.unwrap_or(99).cmp(&y.unwrap_or(99)),
    });
    groups
}

/// First meaningful line of a group's output, for its heading. When the script
/// ran and only its exit code failed it, the script's own output says more
/// than N-able's error message.
pub fn group_headline(g: &ResultGroup, tasks: &[JobTask]) -> String {
    static SEPARATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-=_*\s]+$").unwrap());
    let non_zero = g.tasks.first().and_then(|&i| task_outcome(&tasks[i])).is_some_and(|o| o.kind == OutcomeKind::NonZeroExit);
    let text = if non_zero && !g.stdout.is_empty() {
        &g.stdout
    } else if !g.error_message.is_empty() {
        &g.error_message
    } else if !g.stderr.is_empty() {
        &g.stderr
    } else {
        &g.stdout
    };
    text.split('\n').find(|l| !l.trim().is_empty() && !SEPARATOR.is_match(l)).map_or("No output".into(), |l| l.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExecutionOutput, Executions};

    fn task(device: &str) -> JobTask {
        JobTask {
            task_id: format!("t-{device}"),
            asset_id: Some(format!("a-{device}")),
            device_name: device.into(),
            task_status: Some("IN_PROGRESS".into()),
            execution: None,
            watching: true,
            watch_started_at: 0,
            poll_failures: 0,
            last_error: None,
            stop_reason: None,
        }
    }

    fn execution(device: &str, status: &str, stdout: &str, error: Option<&str>) -> Execution {
        Execution {
            id: format!("e-{device}"),
            status: status.into(),
            exit_code: None, // N-able never fills this in for script runs
            error_message: error.map(String::from),
            started_at: None,
            updated_at: None,
            duration_milliseconds: Some(1000.0),
            output: Some(ExecutionOutput { run_as: None, std_output: Some(stdout.into()), std_error: None, outputs: None }),
        }
    }

    fn finished(device: &str, status: &str, stdout: &str, error: Option<&str>) -> JobTask {
        JobTask { task_status: Some(status.into()), execution: Some(execution(device, status, stdout, error)), watching: false, ..task(device) }
    }

    fn snapshot(task_status: &str, execution_status: &str) -> TaskSnapshot {
        let mut e = execution("a", execution_status, "", None);
        e.output = None;
        TaskSnapshot { id: "t-a".into(), name: None, status: task_status.into(), created_at: None, script: None, executions: Some(Executions { nodes: vec![Some(e)] }) }
    }

    const PARSE: &str = "something went wrong while parsing the PowerShell output";

    #[test]
    fn phase_distinguishes_waiting_running_finished_and_unwatched() {
        assert_eq!(task_phase(&task("a")), TaskPhase::Queued);
        let running = JobTask { execution: Some(execution("a", "RUNNING", "", None)), ..task("a") };
        assert_eq!(task_phase(&running), TaskPhase::Running);
        assert_eq!(task_phase(&finished("a", "SUCCEEDED", "ok", None)), TaskPhase::Succeeded);
        assert_eq!(task_phase(&finished("a", "FAILED", "", None)), TaskPhase::Failed);
        assert_eq!(task_phase(&finished("a", "FAILED", "report", Some(PARSE))), TaskPhase::NonZero);
        assert_eq!(task_phase(&JobTask { watching: false, ..task("a") }), TaskPhase::Stopped);
    }

    #[test]
    fn a_task_that_ended_without_an_execution_is_finished() {
        assert_eq!(task_phase(&JobTask { task_status: Some("EXPIRED".into()), ..task("a") }), TaskPhase::Failed);
    }

    #[test]
    fn snapshot_stops_watching_once_the_execution_finishes() {
        let done = finished("a", "SUCCEEDED", "ok", None);
        let snap = TaskSnapshot {
            id: "t-a".into(),
            name: Some("run".into()),
            status: "SUCCEEDED".into(),
            created_at: None,
            script: None,
            executions: Some(Executions { nodes: vec![done.execution.clone()] }),
        };
        let next = apply_snapshot(&task("a"), &snap);
        assert!(!next.watching);
        assert_eq!(next.execution.unwrap().status, "SUCCEEDED");
    }

    #[test]
    fn snapshot_stops_watching_a_task_cancelled_before_the_device_started_it() {
        let next = apply_snapshot(&task("a"), &snapshot("CANCELLED", "PENDING"));
        assert!(!next.watching);
        assert_eq!(task_phase(&next), TaskPhase::Failed);
        assert_eq!(final_status(&next), Some("CANCELLED"));
        assert_eq!(outcome_label(&task_outcome(&next).unwrap()), "Cancelled");
    }

    #[test]
    fn snapshot_stops_watching_a_task_cancelled_while_its_script_was_running() {
        assert!(!apply_snapshot(&task("a"), &snapshot("CANCELED", "RUNNING")).watching);
    }

    #[test]
    fn snapshot_keeps_watching_when_the_task_ends_before_its_running_execution_reports_back() {
        let next = apply_snapshot(&task("a"), &snapshot("COMPLETED", "RUNNING"));
        assert!(next.watching);
        assert_eq!(task_phase(&next), TaskPhase::Running);
    }

    #[test]
    fn snapshot_uses_the_executions_result_once_it_has_one() {
        let next = apply_snapshot(&task("a"), &snapshot("COMPLETED", "SUCCEEDED"));
        assert_eq!(final_status(&next), Some("SUCCEEDED"));
        assert_eq!(task_phase(&next), TaskPhase::Succeeded);
    }

    #[test]
    fn groups_identical_outputs_ignoring_line_endings_and_the_devices_own_name() {
        let tasks = [
            finished("PC-1", "SUCCEEDED", "PC-1: all good\r\n", None),
            finished("PC-2", "SUCCEEDED", "PC-2: all good", None),
            finished("PC-3", "FAILED", "TPM missing", Some("script failed")),
            finished("PC-4", "SUCCEEDED", "PC-4: all good  ", None),
        ];
        let groups = group_results(&tasks);
        assert_eq!(groups.len(), 2);
        let names: Vec<_> = groups[0].tasks.iter().map(|&i| tasks[i].device_name.as_str()).collect();
        assert_eq!(names, ["PC-1", "PC-2", "PC-4"]);
        assert_eq!(groups[0].stdout, "{device}: all good");
        assert_eq!(groups[1].status.as_deref(), Some("FAILED"));
    }

    #[test]
    fn unfinished_groups_come_before_finished_ones() {
        let tasks = [finished("PC-1", "SUCCEEDED", "ok", None), task("PC-2"), finished("PC-3", "SUCCEEDED", "ok", None)];
        let phases: Vec<_> = group_results(&tasks).iter().map(|g| g.phase).collect();
        assert_eq!(phases, [TaskPhase::Queued, TaskPhase::Succeeded]);
    }

    #[test]
    fn headline_skips_separator_lines_and_prefers_the_error_message() {
        let tasks = [finished("PC-1", "FAILED", "-----\nFail - TPM not ready", None)];
        assert_eq!(group_headline(&group_results(&tasks)[0], &tasks), "Fail - TPM not ready");
        let tasks = [finished("PC-1", "FAILED", "", Some("could not start the script"))];
        assert_eq!(group_headline(&group_results(&tasks)[0], &tasks), "could not start the script");
    }

    #[test]
    fn counts_each_phase() {
        let c = count_phases(&[task("a"), finished("b", "SUCCEEDED", "", None), finished("c", "FAILED", "", None)]);
        assert_eq!(c, PhaseCounts { queued: 1, running: 0, succeeded: 1, non_zero: 0, failed: 1, stopped: 0 });
    }

    #[test]
    fn powershell_parse_error_with_output_means_the_script_ran_and_exited_non_zero() {
        let o = task_outcome(&finished("PC", "FAILED", "Fail - Bitlocker Key Dependancys Not Met", Some(PARSE))).unwrap();
        assert_eq!(o, Outcome { kind: OutcomeKind::NonZeroExit, ran: true, exit_code: None, has_output: true });
        assert_eq!(outcome_label(&o), "Non-zero exit");
    }

    #[test]
    fn recovers_the_exit_code_from_nables_error_text_when_exit_code_is_null() {
        let o = task_outcome(&finished("PC", "FAILED", "", Some("error trying to run script: exit status 1001"))).unwrap();
        assert_eq!((o.kind, o.ran, o.exit_code, o.has_output), (OutcomeKind::NonZeroExit, true, Some(1001), false));
        assert_eq!(outcome_label(&o), "Exit 1001");
    }

    #[test]
    fn recognises_passes_timeouts_and_runs_that_never_started() {
        assert_eq!(task_outcome(&finished("PC", "SUCCEEDED", "ok", None)).unwrap().kind, OutcomeKind::Passed);
        assert_eq!(task_outcome(&finished("PC", "FAILED", "", Some("timed out"))).unwrap().kind, OutcomeKind::TimedOut);
        let expired = JobTask { task_status: Some("EXPIRED".into()), watching: false, ..task("PC") };
        let o = task_outcome(&expired).unwrap();
        assert_eq!((o.kind, o.ran), (OutcomeKind::Error, false));
    }

    #[test]
    fn no_outcome_while_the_task_is_unfinished() {
        assert!(task_outcome(&task("PC")).is_none());
    }

    #[test]
    fn headline_uses_the_scripts_output_not_nables_message_for_non_zero_exits() {
        let tasks = [finished("PC", "FAILED", "Fail - TPM not ready\nUEFI - False", Some(PARSE))];
        assert_eq!(group_headline(&group_results(&tasks)[0], &tasks), "Fail - TPM not ready");
    }

    #[test]
    fn nables_parse_message_is_recognised_as_its_wording_for_a_non_zero_exit() {
        assert!(is_generic_exit_message(PARSE));
        assert!(is_generic_exit_message("Something went wrong while Parsing The PowerShell Output"));
        assert!(!is_generic_exit_message("error trying to run script: exit status 1001"));
        assert!(!is_generic_exit_message(""));
    }

    #[test]
    fn endings_that_look_alarming_are_explained_in_plain_words() {
        let nonzero = task_outcome(&finished("PC", "FAILED", "report", Some(PARSE))).unwrap();
        let text = outcome_explanation(&nonzero).unwrap();
        assert!(text.contains("script ran and finished") && text.contains("non-zero exit code") && !text.to_lowercase().contains("parsing"), "{text}");
        let with_code = task_outcome(&finished("PC", "FAILED", "", Some("exit status 1001"))).unwrap();
        assert!(outcome_explanation(&with_code).unwrap().contains("exit code 1001"));
        let timed_out = task_outcome(&finished("PC", "FAILED", "", Some("timed out"))).unwrap();
        assert!(outcome_explanation(&timed_out).unwrap().contains("in time"));
        let cancelled = task_outcome(&finished("PC", "CANCELLED", "", None)).unwrap();
        assert!(outcome_explanation(&cancelled).unwrap().contains("cancelled"));
        // A pass needs no explaining, and a run that never ran has a real error message of its own.
        assert!(outcome_explanation(&task_outcome(&finished("PC", "SUCCEEDED", "ok", None)).unwrap()).is_none());
        let expired = JobTask { task_status: Some("EXPIRED".into()), watching: false, ..task("PC") };
        assert!(outcome_explanation(&task_outcome(&expired).unwrap()).is_none());
    }

    #[test]
    fn watch_window_is_the_script_timeout_plus_pickup_time() {
        assert_eq!(max_wait_seconds(60), 360);
        assert_eq!(max_wait_seconds(0), 3900);
    }

    #[test]
    fn status_words() {
        assert_eq!(task_status_word(&task("a")), "Waiting");
        assert_eq!(task_status_word(&finished("a", "SUCCEEDED", "ok", None)), "Passed");
        assert_eq!(task_status_word(&finished("a", "FAILED", "", Some("exit status 7"))), "Exit 7");
    }

    #[test]
    fn saved_tasks_use_the_desktop_apps_field_names() {
        let json = serde_json::to_value(JobTask { stop_reason: Some(StopReason::Manual), ..task("a") }).unwrap();
        assert_eq!(json["taskId"], "t-a");
        assert_eq!(json["watchStartedAt"], 0);
        assert_eq!(json["stopReason"], "manual");
        assert!(json["lastError"].is_null() && json["execution"].is_null());
    }
}
