//! Reading what the endpoint script reports, whichever way it came back:
//! as Ecoverse output variables, or as the `NVOKE-RESULT {json}` line and exit
//! code in the zip N-central keeps for a direct task. Both become an
//! `EndpointResult`, and a launch result becomes a `LaunchOutcome` for the tracker.

use std::io::Read;

use serde::Deserialize;

use super::tracker::LaunchOutcome;
use crate::{
    model::TaskSnapshot,
    task_state::{JobTask, TaskPhase, apply_snapshot, outcome_label, task_outcome, task_phase},
};

const RESULT_PREFIX: &str = "NVOKE-RESULT ";
/// The most a decompressed output file may be: a script's output is small, and the zip is not authenticated.
const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;

/// What the script reported. A field the script didn't set is `-` there and empty or false here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EndpointResult {
    pub action: String,
    /// `staged`, `ok`, `cleaned` or `failed`.
    pub result: String,
    pub error_code: i64,
    pub message: String,
    pub script_version: String,
    pub task_created: bool,
    pub staged_at: String,
    pub task_state: String,
    pub cleaned: bool,
    /// The device's `status.json` as compact JSON text, or empty.
    pub status_json: String,
}

fn dash_to_empty(value: &str) -> String {
    if value.trim() == "-" { String::new() } else { value.trim().to_string() }
}

impl EndpointResult {
    /// One `NVOKE-RESULT {json}` line.
    pub fn from_line(line: &str) -> Result<Self, String> {
        let json = line.trim().strip_prefix(RESULT_PREFIX).ok_or("Not a result line.")?;
        let mut result: Self = serde_json::from_str(json).map_err(|e| format!("The device's result couldn't be read: {e}"))?;
        for text in [&mut result.message, &mut result.staged_at, &mut result.task_state, &mut result.status_json] {
            *text = dash_to_empty(text);
        }
        Ok(result)
    }

    /// The first result line in a block of output.
    pub fn from_text(text: &str) -> Option<Result<Self, String>> {
        text.lines().find(|l| l.trim_start().starts_with(RESULT_PREFIX)).map(Self::from_line)
    }

    /// Ecoverse output variables as (variable, value) pairs.
    pub fn from_outputs(outputs: &[(String, String)]) -> Self {
        let get = |name: &str| outputs.iter().find(|(v, _)| v == name).map(|(_, v)| dash_to_empty(v)).unwrap_or_default();
        Self {
            action: String::new(),
            result: get("Result"),
            error_code: get("ErrorCode").parse().unwrap_or(0),
            message: get("Message"),
            script_version: get("ScriptVersion"),
            task_created: get("TaskCreated") == "true",
            staged_at: get("StagedAt"),
            task_state: get("TaskState"),
            cleaned: get("Cleaned") == "true",
            status_json: get("StatusJson"),
        }
    }
}

/// The output variables of a task's first execution, as (variable, string value).
pub fn ecoverse_outputs(task: &JobTask) -> Vec<(String, String)> {
    task.execution
        .as_ref()
        .and_then(|e| e.output.as_ref())
        .and_then(|o| o.outputs.as_ref())
        .map(|values| values.iter().filter_map(|o| Some((o.variable.clone(), o.value.as_ref()?.string_value.clone()?))).collect())
        .unwrap_or_default()
}

/// A finished Ecoverse task's report, or None while it is still queued or running. A script that
/// failed still returns its outputs, so a report comes back for it too when it set any.
pub fn result_from_ecoverse(snapshot: &TaskSnapshot) -> Option<Result<EndpointResult, String>> {
    let task = apply_snapshot(&JobTask::new(&snapshot.id, None, "", 0), snapshot);
    match task_phase(&task) {
        TaskPhase::Queued | TaskPhase::Running | TaskPhase::Stopped => None,
        _ => {
            let result = EndpointResult::from_outputs(&ecoverse_outputs(&task));
            if result.result.is_empty() {
                let label = task_outcome(&task).map_or("Didn't run".to_string(), |o| outcome_label(&o));
                Some(Err(format!("The task ended without a report ({label}).")))
            } else {
                Some(Ok(result))
            }
        }
    }
}

/// How an Ecoverse launch task stands. A script that fails still returns its outputs, so its own message is used.
pub fn launch_outcome_from_ecoverse(snapshot: &TaskSnapshot) -> LaunchOutcome {
    let task = apply_snapshot(&JobTask::new(&snapshot.id, None, "", 0), snapshot);
    let reported = EndpointResult::from_outputs(&ecoverse_outputs(&task));
    match task_phase(&task) {
        TaskPhase::Queued | TaskPhase::Stopped => LaunchOutcome::NoExecution,
        TaskPhase::Running => LaunchOutcome::Running,
        TaskPhase::Succeeded if reported.task_created && reported.result == "staged" => LaunchOutcome::Staged,
        TaskPhase::Succeeded => LaunchOutcome::Failed(if reported.message.is_empty() { "The launcher ran but didn't register the scheduled task.".into() } else { reported.message }),
        TaskPhase::NonZero | TaskPhase::Failed => {
            let label = task_outcome(&task).map_or("Didn't run".to_string(), |o| outcome_label(&o));
            LaunchOutcome::Failed(if reported.message.is_empty() { label } else { format!("{label}: {}", reported.message) })
        }
    }
}

/// How a REST launch task stands: `status` and `message` from `/status/details`, and the parsed output once it exists.
/// `Pending` is a task waiting for an offline device, and is not a failure.
pub fn launch_outcome_from_rest(status: &str, message: &str, output: Option<&RestOutput>) -> LaunchOutcome {
    match status.trim().to_ascii_lowercase().as_str() {
        "" | "pending" => LaunchOutcome::NoExecution,
        "in progress" | "running" => LaunchOutcome::Running,
        "success" => match output.and_then(|o| o.result.as_ref()) {
            Some(r) if r.task_created && r.result == "staged" => LaunchOutcome::Staged,
            Some(r) => LaunchOutcome::Failed(if r.message.is_empty() { "The launcher ran but didn't register the scheduled task.".into() } else { r.message.clone() }),
            // The task ran, but its output isn't available (yet): not proof either way.
            None => LaunchOutcome::Running,
        },
        _ => {
            let detail = output.and_then(|o| o.result.as_ref()).map(|r| r.message.clone()).filter(|m| !m.is_empty());
            let exit = output.and_then(|o| o.exit_code).map(|c| format!(" (exit {c})")).unwrap_or_default();
            LaunchOutcome::Failed(format!("{}{exit}", detail.unwrap_or_else(|| if message.is_empty() { "The task failed on the device.".into() } else { message.into() })))
        }
    }
}

/// What a REST task's output file holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestOutput {
    pub exit_code: Option<i64>,
    pub result: Option<EndpointResult>,
}

impl RestOutput {
    /// From `Output-Formatted.txt`: an `Exit Code: n` line and the script's console output.
    pub fn from_text(text: &str) -> Self {
        let exit_code = text.lines().find_map(|l| {
            let rest = l.trim().strip_prefix("Exit Code:")?;
            rest.trim().parse().ok()
        });
        Self { exit_code, result: EndpointResult::from_text(text).and_then(Result::ok) }
    }

    pub fn from_zip(zip: &[u8]) -> Result<Self, String> {
        let bytes = read_zip_entry(zip, "Output-Formatted.txt")?;
        Ok(Self::from_text(&String::from_utf8_lossy(&bytes)))
    }
}

fn le16(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn le32(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// The contents of one file in a zip, found through the central directory (sizes in local headers can be zero).
/// Handles stored and deflated entries; anything else (Zip64, encryption) is an error.
pub fn read_zip_entry(zip: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let bad = || "The task's output file isn't a readable zip.".to_string();
    let search_from = zip.len().saturating_sub(22 + 65535);
    let eocd = (search_from..zip.len().saturating_sub(21)).rev().find(|&i| zip[i..].starts_with(&[0x50, 0x4b, 0x05, 0x06])).ok_or_else(bad)?;
    let entries = le16(zip, eocd + 10).ok_or_else(bad)?;
    let mut at = le32(zip, eocd + 16).ok_or_else(bad)?;
    for _ in 0..entries {
        if !zip.get(at..).is_some_and(|b| b.starts_with(&[0x50, 0x4b, 0x01, 0x02])) {
            return Err(bad());
        }
        let (method, compressed, name_len, extra_len, comment_len, local) =
            (le16(zip, at + 10).ok_or_else(bad)?, le32(zip, at + 20).ok_or_else(bad)?, le16(zip, at + 28).ok_or_else(bad)?, le16(zip, at + 30).ok_or_else(bad)?, le16(zip, at + 32).ok_or_else(bad)?, le32(zip, at + 42).ok_or_else(bad)?);
        let entry_name = zip.get(at + 46..at + 46 + name_len).ok_or_else(bad)?;
        if String::from_utf8_lossy(entry_name).rsplit(['/', '\\']).next().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            if !zip.get(local..).is_some_and(|b| b.starts_with(&[0x50, 0x4b, 0x03, 0x04])) {
                return Err(bad());
            }
            let data_at = local + 30 + le16(zip, local + 26).ok_or_else(bad)? + le16(zip, local + 28).ok_or_else(bad)?;
            let data = zip.get(data_at..data_at + compressed).ok_or_else(bad)?;
            return match method {
                0 => Ok(data.iter().copied().take(MAX_OUTPUT_BYTES as usize).collect()),
                8 => {
                    let mut out = Vec::new();
                    flate2::read::DeflateDecoder::new(data).take(MAX_OUTPUT_BYTES).read_to_end(&mut out).map_err(|_| bad())?;
                    Ok(out)
                }
                _ => Err("The task's output file uses a zip feature N-voke doesn't read.".into()),
            };
        }
        at += 46 + name_len + extra_len + comment_len;
    }
    Err(format!("The task's output file has no {name}."))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;
    use crate::model::{Execution, ExecutionOutput, Executions, InputValue, OutputValue};

    /// A zip holding one file, written by hand so the reader is tested on a known layout. The local header
    /// carries zero sizes, as zips written with a data descriptor do.
    pub(crate) fn zip_of(name: &str, content: &[u8], deflate: bool) -> Vec<u8> {
        let data = if deflate {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(content).unwrap();
            e.finish().unwrap()
        } else {
            content.to_vec()
        };
        let method: u16 = if deflate { 8 } else { 0 };
        let mut z = Vec::new();
        z.extend([0x50, 0x4b, 0x03, 0x04]);
        z.extend([20, 0, 8, 0]);
        z.extend(method.to_le_bytes());
        z.extend([0; 4]);
        z.extend([0; 4]); // crc
        z.extend([0; 8]); // sizes: in the data descriptor
        z.extend((name.len() as u16).to_le_bytes());
        z.extend([0, 0]);
        z.extend(name.as_bytes());
        let local = 0u32;
        z.extend(&data);
        let cd_at = z.len() as u32;
        z.extend([0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 8, 0]);
        z.extend(method.to_le_bytes());
        z.extend([0; 4]);
        z.extend([0; 4]);
        z.extend((data.len() as u32).to_le_bytes());
        z.extend((content.len() as u32).to_le_bytes());
        z.extend((name.len() as u16).to_le_bytes());
        z.extend([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend(local.to_le_bytes());
        z.extend(name.as_bytes());
        let cd_len = z.len() as u32 - cd_at;
        z.extend([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 1, 0, 1, 0]);
        z.extend(cd_len.to_le_bytes());
        z.extend(cd_at.to_le_bytes());
        z.extend([0, 0]);
        z
    }

    const LINE: &str = r#"NVOKE-RESULT {"action":"Launch","result":"staged","errorCode":0,"message":"Staged: scheduled task registered","scriptVersion":"0.1.0+3f9a2c1","taskCreated":true,"stagedAt":"2026-10-20T12:00:00Z","taskState":"Ready; last run -","cleaned":false,"statusJson":"{\"stage\":\"staged\"}"}"#;

    #[test]
    fn a_result_line_is_read_with_dashes_as_empty() {
        let r = EndpointResult::from_line(LINE).unwrap();
        assert_eq!((r.result.as_str(), r.error_code, r.task_created, r.cleaned), ("staged", 0, true, false));
        assert_eq!((r.script_version.as_str(), r.staged_at.as_str()), ("0.1.0+3f9a2c1", "2026-10-20T12:00:00Z"));
        assert_eq!(r.status_json, r#"{"stage":"staged"}"#);
        let failed = EndpointResult::from_line(r#"NVOKE-RESULT {"result":"failed","errorCode":1004,"message":"DestServer 'x' is not a host name","statusJson":"-","stagedAt":"-"}"#).unwrap();
        assert_eq!((failed.error_code, failed.status_json.as_str(), failed.staged_at.as_str(), failed.task_created), (1004, "", "", false));
        assert!(EndpointResult::from_line("hello").is_err());
        assert!(EndpointResult::from_line("NVOKE-RESULT {not json").is_err());
    }

    #[test]
    fn the_result_is_found_among_other_console_output() {
        let text = format!("Some line\r\n{LINE}\r\nExit Code: 0\r\n");
        assert_eq!(EndpointResult::from_text(&text).unwrap().unwrap().result, "staged");
        assert!(EndpointResult::from_text("nothing here").is_none());
    }

    #[test]
    fn a_zip_entry_is_read_stored_or_deflated_and_a_damaged_zip_is_an_error() {
        let text = format!("Start: now\r\nExit Code: 7\r\n{LINE}\r\n");
        for deflate in [false, true] {
            let zip = zip_of("Output-Formatted.txt", text.as_bytes(), deflate);
            assert_eq!(read_zip_entry(&zip, "output-formatted.TXT").unwrap(), text.as_bytes(), "deflate = {deflate}");
            let out = RestOutput::from_zip(&zip).unwrap();
            assert_eq!(out.exit_code, Some(7));
            assert_eq!(out.result.unwrap().result, "staged");
        }
        let zip = zip_of("other.txt", b"x", true);
        assert!(read_zip_entry(&zip, "Output-Formatted.txt").unwrap_err().contains("no Output-Formatted.txt"));
        assert!(read_zip_entry(b"not a zip at all, but long enough to search through", "x").is_err());
        assert!(read_zip_entry(&[], "x").is_err());
        let mut cut = zip_of("Output-Formatted.txt", b"hello", true);
        cut.truncate(cut.len() - 10);
        assert!(read_zip_entry(&cut, "Output-Formatted.txt").is_err());
    }

    type FakeExecution<'a> = (&'a str, Option<&'a str>, Vec<(&'a str, &'a str)>);

    fn snapshot(task_status: &str, execution: Option<FakeExecution>) -> TaskSnapshot {
        let nodes = execution
            .map(|(status, error, outputs)| {
                vec![Some(Execution {
                    id: "e1".into(),
                    status: status.into(),
                    exit_code: None,
                    error_message: error.map(String::from),
                    started_at: None,
                    updated_at: None,
                    duration_milliseconds: None,
                    output: Some(ExecutionOutput {
                        run_as: None,
                        std_output: None,
                        std_error: None,
                        outputs: Some(outputs.into_iter().map(|(v, s)| OutputValue { name: v.into(), variable: v.into(), kind: "STRING".into(), value: Some(InputValue { string_value: Some(s.into()), ..Default::default() }) }).collect()),
                    }),
                })]
            })
            .unwrap_or_default();
        TaskSnapshot { id: "t1".into(), name: None, status: task_status.into(), created_at: None, script: None, executions: Some(Executions { nodes }) }
    }

    #[test]
    fn an_ecoverse_launch_is_staged_only_when_the_script_says_it_registered_the_task() {
        let staged = vec![("Result", "staged"), ("TaskCreated", "true"), ("ErrorCode", "0"), ("Message", "ok")];
        assert_eq!(launch_outcome_from_ecoverse(&snapshot("COMPLETED", Some(("SUCCEEDED", None, staged)))), LaunchOutcome::Staged);
        let no_task = vec![("Result", "ok"), ("TaskCreated", "false"), ("Message", "-")];
        assert!(matches!(launch_outcome_from_ecoverse(&snapshot("COMPLETED", Some(("SUCCEEDED", None, no_task)))), LaunchOutcome::Failed(m) if m.contains("didn't register")));
        assert_eq!(launch_outcome_from_ecoverse(&snapshot("IN_PROGRESS", None)), LaunchOutcome::NoExecution);
        assert_eq!(launch_outcome_from_ecoverse(&snapshot("IN_PROGRESS", Some(("IN_PROGRESS", None, vec![])))), LaunchOutcome::Running);
    }

    #[test]
    fn a_failed_ecoverse_launch_says_what_the_script_said() {
        let outputs = vec![("Result", "failed"), ("ErrorCode", "1004"), ("Message", "DestServer 'x' is not a host name"), ("TaskCreated", "false")];
        let outcome = launch_outcome_from_ecoverse(&snapshot("FAILED", Some(("FAILED", Some("exit status 1004"), outputs))));
        match outcome {
            LaunchOutcome::Failed(why) => assert!(why.contains("1004") && why.contains("is not a host name"), "{why}"),
            other => panic!("{other:?}"),
        }
        let cancelled = launch_outcome_from_ecoverse(&snapshot("CANCELED", None));
        assert!(matches!(cancelled, LaunchOutcome::Failed(_)), "{cancelled:?}");
    }

    #[test]
    fn a_rest_launch_waits_while_pending_and_needs_the_output_to_be_called_staged() {
        let staged = RestOutput { exit_code: Some(0), result: Some(EndpointResult::from_line(LINE).unwrap()) };
        assert_eq!(launch_outcome_from_rest("Pending", "", None), LaunchOutcome::NoExecution);
        assert_eq!(launch_outcome_from_rest("In Progress", "", None), LaunchOutcome::Running);
        assert_eq!(launch_outcome_from_rest("Success", "ok", Some(&staged)), LaunchOutcome::Staged);
        assert_eq!(launch_outcome_from_rest("Success", "ok", None), LaunchOutcome::Running, "the output zip isn't there yet");
        let failed = RestOutput { exit_code: Some(1004), result: Some(EndpointResult { message: "bad token".into(), ..Default::default() }) };
        assert_eq!(launch_outcome_from_rest("Failed", "Failed execution on remote device.", Some(&failed)), LaunchOutcome::Failed("bad token (exit 1004)".into()));
        assert_eq!(launch_outcome_from_rest("Failed", "Failed execution on remote device.", None), LaunchOutcome::Failed("Failed execution on remote device.".into()));
        let not_staged = RestOutput { exit_code: Some(0), result: Some(EndpointResult { result: "ok".into(), ..Default::default() }) };
        assert!(matches!(launch_outcome_from_rest("Success", "", Some(&not_staged)), LaunchOutcome::Failed(_)));
    }

    #[test]
    fn output_text_gives_the_exit_code_and_ignores_garbage_lines() {
        assert_eq!(RestOutput::from_text("Exit Code: -1\nNVOKE-RESULT {broken").exit_code, Some(-1));
        assert!(RestOutput::from_text("Exit Code: -1\nNVOKE-RESULT {broken").result.is_none());
        assert_eq!(RestOutput::from_text("no exit here").exit_code, None);
    }
}
