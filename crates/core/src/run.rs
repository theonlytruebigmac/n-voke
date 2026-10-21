//! Preparing and starting a script run: validating the form, resolving the
//! chosen devices and tags into one de-duplicated device list, building the
//! request, and turning the API's answer into a job. A port of the logic in the
//! desktop app's `RunPage.tsx` and `TargetDevices.tsx`.

use std::collections::HashMap;

use chrono::{DateTime, Local, SecondsFormat, Utc};

use crate::{
    Core,
    api::{RunInput, RunResult, RunScriptInput},
    collections::Collections,
    inputs::{InputType, Raw, to_input_value, validate_input},
    jobs::Job,
    model::{Asset, Organization, Script, ScriptInputDecl},
    task_state::JobTask,
    targets::{Target, target_label},
};

/// The API's longest allowed script timeout, in seconds.
pub const MAX_TIMEOUT_SECONDS: i64 = 3600;
/// Task names are capped by the API.
const MAX_NAME_CHARS: usize = 128;

/// What the user typed or toggled for one script input.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Text(String),
    Bool(bool),
}

pub type InputValues = HashMap<String, FieldValue>;

impl FieldValue {
    fn as_raw(&self) -> Raw<'_> {
        match self {
            FieldValue::Text(t) => Raw::Text(t),
            FieldValue::Bool(b) => Raw::Bool(*b),
        }
    }

    fn is_blank(&self) -> bool {
        matches!(self, FieldValue::Text(t) if t.is_empty())
    }
}

/// "0–3600" when the timeout text isn't a whole number of seconds in range.
pub fn timeout_error(text: &str) -> Option<&'static str> {
    let ok = text.is_empty() || (text.chars().all(|c| c.is_ascii_digit()) && text.parse::<i64>().is_ok_and(|n| n <= MAX_TIMEOUT_SECONDS));
    if ok { None } else { Some("0–3600") }
}

/// The timeout the run will use: what was typed, else the script's own, else 0 (the API maximum).
pub fn effective_timeout(text: &str, script: &Script) -> i64 {
    if text.is_empty() { script.timeout_seconds.unwrap_or(0) } else { text.parse().unwrap_or(0) }
}

/// Problems with the typed input values, by variable name. Blank values are fine (the input is skipped).
pub fn input_errors(inputs: &[ScriptInputDecl], values: &InputValues) -> HashMap<String, String> {
    inputs
        .iter()
        .filter_map(|decl| {
            let error = match values.get(&decl.variable) {
                None => None,
                Some(FieldValue::Text(t)) => validate_input(InputType::parse(&decl.kind), Some(t)),
                Some(FieldValue::Bool(_)) => None,
            };
            error.map(|e| (decl.variable.clone(), e))
        })
        .collect()
}

/// The inputs to send: only those with a value, converted to their types.
pub fn build_run_inputs(inputs: &[ScriptInputDecl], values: &InputValues) -> Result<Vec<RunInput>, String> {
    let mut out = vec![];
    for decl in inputs {
        let Some(value) = values.get(&decl.variable).filter(|v| !v.is_blank()) else { continue };
        let kind = InputType::parse(&decl.kind);
        let converted = to_input_value(kind, value.as_raw()).map_err(|e| format!("{}: {e}", decl.name))?;
        out.push(RunInput { name: decl.name.clone(), variable: decl.variable.clone(), kind, value: converted });
    }
    Ok(out)
}

/// The name shown in N-able for the task: what the user typed, else "N-voke - <date and time>".
pub fn task_name(typed: &str, now: DateTime<Local>) -> String {
    let name = typed.trim();
    let name = if name.is_empty() { format!("N-voke - {}", now.format("%Y-%m-%d %H:%M")) } else { name.to_string() };
    name.chars().take(MAX_NAME_CHARS).collect()
}

/// A device in the run, and what put it there.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub asset: Asset,
    /// Names of the tags it came from; empty when it was picked directly (it can be both).
    pub via_tags: Vec<String>,
    /// Picked by name rather than through a tag.
    pub picked: bool,
}

impl Resolved {
    /// Where it came from, for display: "Added directly", tag names, or both.
    pub fn source(&self) -> String {
        let mut parts: Vec<&str> = vec![];
        if self.picked {
            parts.push("Added directly");
        }
        parts.extend(self.via_tags.iter().map(String::as_str));
        parts.join(", ")
    }
}

/// Merges directly picked devices and tag members into one list with each device once, sorted by name.
/// `truncated` says a tag has more devices than were returned.
pub fn merge_targets(picked: &[Asset], tags: Vec<(String, crate::api::Devices)>) -> (Vec<Resolved>, bool) {
    let mut order: Vec<String> = vec![];
    let mut by_id: HashMap<String, Resolved> = HashMap::new();
    for a in picked {
        if !by_id.contains_key(&a.id) {
            order.push(a.id.clone());
            by_id.insert(a.id.clone(), Resolved { asset: a.clone(), via_tags: vec![], picked: true });
        }
    }
    let mut truncated = false;
    for (tag_name, devices) in tags {
        truncated |= devices.total_count > devices.assets.len() as i64;
        for a in devices.assets {
            match by_id.get_mut(&a.id) {
                Some(existing) => {
                    if !existing.via_tags.contains(&tag_name) {
                        existing.via_tags.push(tag_name.clone());
                    }
                }
                None => {
                    order.push(a.id.clone());
                    by_id.insert(a.id.clone(), Resolved { asset: a, via_tags: vec![tag_name.clone()], picked: false });
                }
            }
        }
    }
    let mut list: Vec<Resolved> = order.into_iter().filter_map(|id| by_id.remove(&id)).collect();
    list.sort_by(|a, b| a.asset.name.to_lowercase().cmp(&b.asset.name.to_lowercase()));
    (list, truncated)
}

/// Looks up each chosen tag's devices and merges them with the directly picked ones.
pub async fn resolve_targets(collections: &Collections, org_id: &str, targets: &[Target]) -> Result<(Vec<Resolved>, bool), String> {
    let picked: Vec<Asset> = targets.iter().filter_map(|t| if let Target::Device(a) = t { Some(a.clone()) } else { None }).collect();
    let mut tags = vec![];
    for t in targets {
        if let Target::Tag(tag) = t {
            tags.push((tag.name.clone(), collections.tag_members(org_id, &tag.id).await?));
        }
    }
    Ok(merge_targets(&picked, tags))
}

/// Whether a device's agent is known to be offline.
pub fn is_offline(asset: &Asset) -> bool {
    asset.agent_connection.as_ref().is_some_and(|c| c.status != "CONNECTED")
}

/// Everything needed to start a run.
pub struct RunRequest {
    pub org: Organization,
    pub script: Script,
    pub targets: Vec<Target>,
    /// The final device list: the resolved devices minus any the user left out.
    pub devices: Vec<Asset>,
    pub values: InputValues,
    pub task_name: String,
    pub timeout: String,
}

/// What is wrong with a request, if anything.
#[derive(Debug, Default, PartialEq)]
pub struct Problems {
    pub timeout: Option<&'static str>,
    pub inputs: HashMap<String, String>,
}

impl Problems {
    pub fn any(&self) -> bool {
        self.timeout.is_some() || !self.inputs.is_empty()
    }
}

pub fn check(script: &Script, values: &InputValues, timeout: &str) -> Problems {
    let decls = script.inputs.as_deref().unwrap_or(&[]);
    Problems { timeout: timeout_error(timeout), inputs: input_errors(decls, values) }
}

fn build_request(req: &RunRequest, name: &str) -> Result<RunScriptInput, String> {
    let decls = req.script.inputs.as_deref().unwrap_or(&[]);
    Ok(RunScriptInput {
        script_id: req.script.id.clone(),
        script_name: req.script.name.clone(),
        target_label: target_label(&req.targets),
        asset_ids: req.devices.iter().map(|d| d.id.clone()).collect(),
        name: name.to_string(),
        timeout_seconds: effective_timeout(&req.timeout, &req.script),
        inputs: build_run_inputs(decls, &req.values)?,
        run_when_online: false,
    })
}

/// The job to watch, from what was started.
pub fn job_from_result(req: &RunRequest, name: &str, result: RunResult, now: DateTime<Utc>) -> Job {
    let names: HashMap<&str, &str> = req.devices.iter().map(|d| (d.id.as_str(), d.name.as_str())).collect();
    let now_ms = now.timestamp_millis();
    Job {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        script_id: Some(req.script.id.clone()),
        script: Some(req.script.clone()),
        script_name: req.script.name.clone(),
        org_id: Some(req.org.id.clone()),
        org_name: Some(req.org.name.clone()),
        target_label: target_label(&req.targets),
        created_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
        timeout_seconds: effective_timeout(&req.timeout, &req.script),
        warnings: Some(result.warnings),
        migration_project: None,
        tasks: result
            .tasks
            .into_iter()
            .map(|(task_id, asset_id)| {
                let device = asset_id.as_deref().and_then(|id| names.get(id)).copied().unwrap_or("Unknown device");
                JobTask::new(&task_id, asset_id.as_deref(), device, now_ms)
            })
            .collect(),
    }
}

/// Validates, sends and returns the job to watch. In dry-run mode the send is held back and
/// this fails with `DRY_RUN_ERROR`.
pub async fn start_run(core: &Core, req: &RunRequest) -> Result<Job, String> {
    if req.devices.is_empty() {
        return Err("Choose at least one device.".into());
    }
    let problems = check(&req.script, &req.values, &req.timeout);
    if problems.any() {
        return Err("Fix the highlighted fields first.".into());
    }
    let name = task_name(&req.task_name, Local::now());
    let result = core.run_script(&build_request(req, &name)?).await?;
    Ok(job_from_result(req, &name, result, Utc::now()))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::{api::Devices, model::{StatusRef, Tag}};

    fn decl(name: &str, variable: &str, kind: &str) -> ScriptInputDecl {
        ScriptInputDecl { name: name.into(), variable: variable.into(), kind: kind.into() }
    }

    fn script(inputs: Vec<ScriptInputDecl>, timeout: Option<i64>) -> Script {
        Script { id: "s1".into(), name: "Cleanup".into(), updated_at: None, language: "POWERSHELL".into(), description: None, timeout_seconds: timeout, inputs: Some(inputs) }
    }

    fn asset(id: &str, name: &str) -> Asset {
        Asset::stub(id, name)
    }

    fn text(s: &str) -> FieldValue {
        FieldValue::Text(s.into())
    }

    #[test]
    fn timeouts_must_be_whole_seconds_up_to_an_hour() {
        for ok in ["", "0", "60", "3600"] {
            assert_eq!(timeout_error(ok), None, "{ok:?}");
        }
        for bad in ["abc", "3601", "-5", "1.5", " 5", "99999999999999999999"] {
            assert_eq!(timeout_error(bad), Some("0–3600"), "{bad:?}");
        }
    }

    #[test]
    fn the_timeout_used_is_the_typed_one_then_the_scripts_then_the_maximum() {
        let with = script(vec![], Some(120));
        assert_eq!(effective_timeout("30", &with), 30);
        assert_eq!(effective_timeout("", &with), 120);
        assert_eq!(effective_timeout("", &script(vec![], None)), 0);
        assert_eq!(effective_timeout("0", &with), 0, "an explicit 0 means the API maximum");
    }

    #[test]
    fn typed_inputs_are_checked_against_their_types_and_blanks_are_fine() {
        let decls = [decl("Count", "n", "INT"), decl("When", "t", "DATE_TIME"), decl("Mode", "m", "STRING"), decl("Fast", "f", "BOOLEAN")];
        let mut values = InputValues::new();
        values.insert("n".into(), text("abc"));
        values.insert("t".into(), text(""));
        values.insert("m".into(), text("anything"));
        values.insert("f".into(), FieldValue::Bool(true));
        let errors = input_errors(&decls, &values);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors["n"], "expected a whole number");
        values.insert("n".into(), text("7"));
        assert!(input_errors(&decls, &values).is_empty());
    }

    #[test]
    fn only_inputs_with_a_value_are_sent_and_an_unticked_box_still_counts() {
        let decls = [decl("Count", "n", "INT"), decl("Mode", "m", "STRING"), decl("Fast", "f", "BOOLEAN"), decl("Pass", "pw", "PASSWORD"), decl("Skipped", "s", "STRING")];
        let mut values = InputValues::new();
        values.insert("n".into(), text(" 42 "));
        values.insert("m".into(), text(""));
        values.insert("f".into(), FieldValue::Bool(false));
        values.insert("pw".into(), text(" s3cret "));
        let sent = build_run_inputs(&decls, &values).unwrap();
        let variables: Vec<&str> = sent.iter().map(|i| i.variable.as_str()).collect();
        assert_eq!(variables, ["n", "f", "pw"], "blank and never-touched inputs are skipped; false is a value");
        assert_eq!(sent[0].value.int_value, Some(42));
        assert_eq!(sent[1].value.boolean_value, Some(false));
        assert_eq!(sent[2].value.string_value.as_deref(), Some(" s3cret "), "passwords are not trimmed");
    }

    #[test]
    fn a_bad_value_names_the_input_that_has_it() {
        let mut values = InputValues::new();
        values.insert("n".into(), text("x"));
        assert_eq!(build_run_inputs(&[decl("Count", "n", "INT")], &values).err().as_deref(), Some("Count: expected a whole number"));
    }

    #[test]
    fn task_names_default_to_the_date_and_are_capped() {
        let at = Local.with_ymd_and_hms(2026, 10, 17, 13, 5, 0).unwrap();
        assert_eq!(task_name("  ", at), "N-voke - 2026-10-17 13:05");
        assert_eq!(task_name("  Weekly cleanup ", at), "Weekly cleanup");
        assert_eq!(task_name(&"x".repeat(300), at).chars().count(), 128);
        assert_eq!(task_name(&"é".repeat(300), at).chars().count(), 128, "counts characters, not bytes");
    }

    #[test]
    fn devices_picked_directly_and_through_tags_appear_once_with_all_their_sources() {
        let picked = [asset("a2", "bravo"), asset("a1", "Alpha")];
        let tags = vec![
            ("PatchGroupTwo".to_string(), Devices { assets: vec![asset("a1", "Alpha"), asset("a3", "charlie")], total_count: 2 }),
            ("Servers".to_string(), Devices { assets: vec![asset("a3", "charlie")], total_count: 1 }),
        ];
        let (list, truncated) = merge_targets(&picked, tags);
        assert!(!truncated);
        assert_eq!(list.iter().map(|r| r.asset.name.as_str()).collect::<Vec<_>>(), ["Alpha", "bravo", "charlie"], "sorted by name, ignoring case");
        assert_eq!(list[0].source(), "Added directly, PatchGroupTwo");
        assert_eq!(list[1].source(), "Added directly");
        assert_eq!(list[2].source(), "PatchGroupTwo, Servers");
    }

    #[test]
    fn a_tag_with_more_devices_than_were_returned_is_flagged() {
        let (_, truncated) = merge_targets(&[], vec![("Big".into(), Devices { assets: vec![asset("a", "a")], total_count: 5000 })]);
        assert!(truncated);
    }

    #[test]
    fn the_same_device_picked_twice_is_one_device() {
        let (list, _) = merge_targets(&[asset("a", "a"), asset("a", "a")], vec![]);
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn offline_means_the_agent_is_known_to_be_disconnected() {
        let mut a = asset("a", "a");
        assert!(!is_offline(&a), "unknown is not offline");
        a.agent_connection = Some(StatusRef { status: "CONNECTED".into() });
        assert!(!is_offline(&a));
        a.agent_connection = Some(StatusRef { status: "DISCONNECTED".into() });
        assert!(is_offline(&a));
    }

    fn request() -> RunRequest {
        let tag = Tag { id: "t1".into(), name: "PatchGroupTwo".into(), typename: "StaticTag".into(), asset_count: Some(1), updated_at: None, description: None, organization: None, filter: None };
        RunRequest {
            org: Organization { id: "o1".into(), name: "Acme".into(), typename: "Customer".into() },
            script: script(vec![decl("Count", "n", "INT")], Some(120)),
            targets: vec![Target::Tag(tag), Target::Device(asset("a9", "SLS-1"))],
            devices: vec![asset("a1", "PC-1"), asset("a2", "PC-2")],
            values: InputValues::new(),
            task_name: String::new(),
            timeout: String::new(),
        }
    }

    #[test]
    fn the_job_remembers_what_was_run_where_and_names_each_device() {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        let req = request();
        let t1 = B64.encode("task-1:a1");
        let t2 = B64.encode("task-2:a2");
        let t3 = B64.encode("task-3:gone");
        let result = RunResult { tasks: vec![(t1.clone(), Some("a1".into())), (t2, Some("a2".into())), (t3, Some("gone".into())), ("opaque".into(), None)], warnings: vec!["PC-9 skipped".into()] };
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let job = job_from_result(&req, "Run Cleanup", result, now);
        assert_eq!(job.script_name, "Cleanup");
        assert_eq!(job.org_name.as_deref(), Some("Acme"));
        assert_eq!(job.target_label, "PatchGroupTwo + 1 device");
        assert_eq!(job.created_at, "2026-10-17T12:00:00.000Z");
        assert_eq!(job.timeout_seconds, 120);
        assert_eq!(job.warnings.as_deref(), Some(&["PC-9 skipped".to_string()][..]));
        assert!(job.script.is_some(), "the full script is kept so it can be run again");
        let names: Vec<&str> = job.tasks.iter().map(|t| t.device_name.as_str()).collect();
        assert_eq!(names, ["PC-1", "PC-2", "Unknown device", "Unknown device"]);
        assert!(job.tasks.iter().all(|t| t.watching), "new tasks are watched straight away");
        assert_eq!(job.tasks[0].task_id, t1);
        assert_eq!(job.tasks[0].watch_started_at, now.timestamp_millis());
        assert_eq!(uuid::Uuid::parse_str(&job.id).unwrap().get_version_num(), 4);
    }

    /// A core in a throwaway directory, so tests never touch the real files.
    fn core(name: &str) -> (Core, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("nvoke-run-test-{name}-{}", std::process::id()));
        let paths = crate::Paths { config_dir: dir.join("config"), data_dir: dir.join("data") };
        (Core::new_in(paths, "test").unwrap(), dir)
    }

    #[tokio::test]
    async fn a_run_needs_devices_and_valid_fields_before_anything_is_sent() {
        let (core, dir) = core("validation");
        core.set_dry_run(true); // belt and braces: even a bug here could not send anything
        let mut req = request();
        req.devices.clear();
        assert_eq!(start_run(&core, &req).await.err().as_deref(), Some("Choose at least one device."));
        let mut req = request();
        req.timeout = "9999".into();
        assert_eq!(start_run(&core, &req).await.err().as_deref(), Some("Fix the highlighted fields first."));
        req.timeout.clear();
        req.values.insert("n".into(), text("abc"));
        assert_eq!(start_run(&core, &req).await.err().as_deref(), Some("Fix the highlighted fields first."));
        assert!(core.dry_run_log().is_empty(), "nothing reached the point of being sent");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn in_dry_run_a_valid_run_is_held_back_with_the_request_it_would_have_sent() {
        let (core, dir) = core("dry");
        core.set_dry_run(true);
        let mut req = request();
        req.values.insert("n".into(), text("42"));
        req.task_name = "Weekly".into();
        assert_eq!(start_run(&core, &req).await.err().as_deref(), Some(crate::DRY_RUN_ERROR));
        let entry = &core.dry_run_log()[0];
        assert_eq!(entry.summary, "Ran Cleanup on 2 devices (PatchGroupTwo + 1 device)");
        assert_eq!(entry.variables["input"]["assetIds"], serde_json::json!(["a1", "a2"]));
        assert_eq!(entry.variables["input"]["configuration"]["name"], "Weekly");
        assert_eq!(entry.variables["input"]["configuration"]["timeoutSeconds"], 120);
        assert_eq!(entry.variables["input"]["configuration"]["inputs"][0]["value"]["intValue"], 42);
        let _ = std::fs::remove_dir_all(dir);
    }
}
