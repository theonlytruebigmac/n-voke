//! What the Plan step shows: the source's devices as the plan sees them today, which are ready, which are blocked
//! and why, and which are already in the project. Reading it changes nothing; adding the ready ones is
//! `migration_plan_saved`.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    planner::{Blocker, DeliveryPath, PlannedDevice, Warning},
    project::{Project, Stage},
    survey::{Survey, SurveyOptions},
};
use crate::{Core, audit::AuditEntry, ncentral::model::OrgUnit};

/// How long what the servers said is trusted when the plan is worked out again after a choice.
const PLAN_READ_FOR: Duration = Duration::from_secs(15 * 60);

fn plan_cache() -> &'static Mutex<HashMap<String, (Instant, Survey)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Survey)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Forgets what was read for a project, so the next plan reads the servers again. Called when something the read
/// depends on changes: the mapping, or the registration tokens.
pub(crate) fn forget_plan(project_id: &str) {
    plan_cache().lock().unwrap().remove(project_id);
}

/// What an operator can choose for one device in the plan.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "DeviceChoice.ts"))]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum DeviceChoice {
    /// Reach it this way, over the suggested path.
    UsePath { path: DeliveryPath },
    /// Go back to the suggested path.
    SuggestedPath,
    LeaveOut,
    /// Put a device that was left out back in.
    Include,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "PlanRow.ts"))]
#[serde(rename_all = "camelCase")]
pub struct PlanRow {
    pub device_id: String,
    pub name: String,
    /// The source customer, and its site when it has one.
    pub customer: String,
    pub site: Option<String>,
    /// The destination customer or site, when the mapping gives one.
    pub destination: Option<String>,
    pub path: Option<DeliveryPath>,
    pub blockers: Vec<Blocker>,
    pub warnings: Vec<Warning>,
    pub ready: bool,
    pub left_out: bool,
    /// The operator took this device out (as opposed to its customer or site being left out in Map).
    pub left_out_by_choice: bool,
    /// The operator chose the path, over the suggested one.
    pub path_chosen: bool,
    /// Already in the project, and where it is.
    pub stage: Option<Stage>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "BlockerCount.ts"))]
#[serde(rename_all = "camelCase")]
pub struct BlockerCount {
    pub blocker: Blocker,
    pub devices: usize,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "PlanShow.ts"))]
#[serde(rename_all = "camelCase")]
pub enum PlanShow {
    #[default]
    All,
    Ready,
    Blocked,
}

/// Which rows of the plan to send: a filter and a page, so a server with thousands of devices never sends them all.
#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "PlanQuery.ts"))]
#[serde(rename_all = "camelCase", default)]
pub struct PlanQuery {
    pub show: PlanShow,
    /// A source customer name, or None for all.
    pub customer: Option<String>,
    /// Matched (ignoring case) against the device, its customer and its destination.
    pub text: String,
    pub offset: usize,
    /// 0 means the default page.
    pub limit: usize,
}

const DEFAULT_PAGE: usize = 200;
const MAX_PAGE: usize = 1000;

impl PlanQuery {
    pub fn all() -> Self {
        Self::default()
    }

    fn keeps(&self, row: &PlanRow) -> bool {
        let blocked = !row.ready && !row.left_out;
        let shown = match self.show {
            PlanShow::All => true,
            PlanShow::Ready => row.ready,
            PlanShow::Blocked => blocked,
        };
        if !shown || self.customer.as_deref().is_some_and(|c| row.customer != c) {
            return false;
        }
        let term = self.text.trim().to_lowercase();
        term.is_empty() || row.name.to_lowercase().contains(&term) || row.customer.to_lowercase().contains(&term) || row.destination.as_deref().is_some_and(|d| d.to_lowercase().contains(&term))
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "PlanView.ts"))]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub total: usize,
    /// Ready now and not yet in the project.
    pub ready_new: usize,
    pub ready_ecoverse: usize,
    pub ready_rest: usize,
    pub blocked: usize,
    pub left_out: usize,
    pub in_project: usize,
    pub blocked_by: Vec<BlockerCount>,
    pub warnings: Vec<String>,
    /// Every source customer with devices, for the filter.
    pub customers: Vec<String>,
    /// How many rows match the query (the rows below are one page of them), and where the page starts.
    pub matching: usize,
    pub offset: usize,
    pub rows: Vec<PlanRow>,
}

/// What adding the ready devices did.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "PlanAdded.ts"))]
#[serde(rename_all = "camelCase")]
pub struct PlanAdded {
    pub added: usize,
    pub already_in: usize,
    pub blocked: usize,
}

fn name_of(units: &[OrgUnit], id: Option<&str>) -> Option<String> {
    let id = id?;
    units.iter().find(|u| u.org_unit_id == id).map(|u| u.org_unit_name.clone())
}

/// The rows and counts from a survey and what the project already holds.
pub fn plan_view(survey: &Survey, project: &Project, query: &PlanQuery) -> PlanView {
    let held: HashMap<&str, Stage> = project.devices.iter().map(|d| (d.source_device_id.as_str(), d.stage)).collect();
    let row = |d: &PlannedDevice| PlanRow {
        device_id: d.device_id.clone(),
        name: d.name.clone(),
        customer: name_of(&survey.source_units, d.source_customer_id.as_deref()).unwrap_or_else(|| "(no customer)".into()),
        site: name_of(&survey.source_units, d.source_site_id.as_deref()),
        destination: name_of(&survey.dest_units, d.dest_org_unit_id.as_deref()),
        path: d.path,
        blockers: d.blockers.clone(),
        warnings: d.warnings.clone(),
        ready: d.is_ready(),
        left_out: d.excluded,
        left_out_by_choice: project.left_out.contains(&d.device_id),
        path_chosen: project.path_overrides.contains_key(&d.device_id),
        stage: held.get(d.device_id.as_str()).copied(),
    };
    let mut rows: Vec<PlanRow> = survey.planned.iter().map(row).collect();
    rows.sort_by(|a, b| a.customer.to_lowercase().cmp(&b.customer.to_lowercase()).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.device_id.cmp(&b.device_id)));

    let ready_new: Vec<&PlanRow> = rows.iter().filter(|r| r.ready && r.stage.is_none()).collect();
    let mut customers: Vec<String> = rows.iter().map(|r| r.customer.clone()).collect();
    customers.dedup();
    let limit = if query.limit == 0 { DEFAULT_PAGE } else { query.limit.min(MAX_PAGE) };
    let kept: Vec<&PlanRow> = rows.iter().filter(|r| query.keeps(r)).collect();
    PlanView {
        total: rows.len(),
        ready_new: ready_new.len(),
        ready_ecoverse: ready_new.iter().filter(|r| r.path == Some(DeliveryPath::Ecoverse)).count(),
        ready_rest: ready_new.iter().filter(|r| r.path == Some(DeliveryPath::Rest)).count(),
        blocked: rows.iter().filter(|r| !r.ready && !r.left_out).count(),
        left_out: rows.iter().filter(|r| r.left_out).count(),
        in_project: rows.iter().filter(|r| r.stage.is_some()).count(),
        blocked_by: survey.summary.blocked_by.iter().map(|(blocker, devices)| BlockerCount { blocker: *blocker, devices: *devices }).collect(),
        warnings: survey.warnings.clone(),
        customers,
        matching: kept.len(),
        offset: query.offset,
        rows: kept.into_iter().skip(query.offset).take(limit).cloned().collect(),
    }
}

impl Core {
    /// Plans the source's devices. Reads both servers (and Ecoverse) the first time, which is slow on a large server, then
    /// works the plan out again from what was read when only the operator's choices change. `refresh` reads again.
    pub async fn migration_plan_view(&self, id: &str, refresh: bool, query: &PlanQuery) -> Result<PlanView, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let cached = if refresh { None } else { plan_cache().lock().unwrap().get(id).filter(|(at, _)| at.elapsed() < PLAN_READ_FOR).map(|(_, s)| s.clone()) };
        let survey = match cached {
            Some(mut survey) => {
                survey.replan(&project, project.rest_scripts.contains_key(&project.source_server_id));
                survey
            }
            None => {
                let options = SurveyOptions {
                    source_server_id: project.source_server_id.clone(),
                    dest_server_id: project.dest_server_id.clone(),
                    ecoverse_org_ids: project.source_ecoverse_org_ids.clone(),
                    customer: None,
                    rest_script_ready: project.rest_scripts.contains_key(&project.source_server_id),
                };
                let survey = self.migration_survey(&options, Some(&project)).await?;
                plan_cache().lock().unwrap().insert(id.to_string(), (Instant::now(), survey.clone()));
                survey
            }
        };
        Ok(plan_view(&survey, &project, query))
    }

    /// Records the operator's choice for one device, and applies it to the device when it is already in the project and
    /// hasn't been launched. The plan shows it at once, since nothing has to be read again.
    pub async fn migration_device_choice(&self, id: &str, instance: &str, device_id: &str, choice: DeviceChoice) -> Result<String, String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        let held = project.devices.iter().position(|d| d.source_device_id == device_id);
        let cached_asset = plan_cache().lock().unwrap().get(id).and_then(|(_, s)| s.inputs.eco.get(device_id).map(|a| a.asset_id.clone()));
        let name = held.map(|i| project.devices[i].name.clone()).or_else(|| plan_cache().lock().unwrap().get(id).and_then(|(_, s)| s.planned.iter().find(|d| d.device_id == device_id).map(|d| d.name.clone()))).unwrap_or_else(|| device_id.to_string());
        let now = chrono::Utc::now();
        let only_planned = |project: &Project, what: &str| -> Result<(), String> {
            match held.map(|i| project.devices[i].stage) {
                Some(Stage::Planned) | None => Ok(()),
                Some(stage) => Err(format!("{name} is {stage:?}: {what} can only be changed before it is launched.")),
            }
        };
        let message = match choice {
            DeviceChoice::UsePath { path } => {
                only_planned(&project, "the path")?;
                project.path_overrides.insert(device_id.to_string(), path);
                if let Some(i) = held {
                    let device = &mut project.devices[i];
                    if path == DeliveryPath::Ecoverse && device.asset_id.is_none() {
                        device.asset_id = cached_asset.clone();
                    }
                    if path == DeliveryPath::Ecoverse && device.asset_id.is_none() {
                        project.path_overrides.remove(device_id);
                        return Err(format!("Ecoverse can't reach {name}: it has no Ecoverse asset on this server. Use the N-central path."));
                    }
                    device.path = Some(path);
                }
                format!("{name} will be reached through {}.", if path == DeliveryPath::Ecoverse { "Ecoverse" } else { "N-central" })
            }
            DeviceChoice::SuggestedPath => {
                only_planned(&project, "the path")?;
                project.path_overrides.remove(device_id);
                if held.is_some() {
                    // The suggested path comes from the plan, so a device already in the project needs the plan read.
                    let survey = plan_cache().lock().unwrap().get(id).map(|(_, s)| s.clone()).ok_or("Read the servers again first (Read servers again), then go back to the suggested path.")?;
                    let mut survey = survey;
                    survey.replan(&project, project.rest_scripts.contains_key(&project.source_server_id));
                    if let (Some(i), Some(planned)) = (held, survey.planned.iter().find(|d| d.device_id == device_id)) {
                        project.devices[i].path = planned.path.or(project.devices[i].path);
                        project.devices[i].asset_id = planned.asset_id.clone().or(project.devices[i].asset_id.take());
                    }
                }
                format!("{name} is back on the suggested path.")
            }
            DeviceChoice::LeaveOut => {
                only_planned(&project, "leaving it out")?;
                project.left_out.insert(device_id.to_string());
                if let Some(i) = held {
                    let device = &mut project.devices[i];
                    if device.stage == Stage::Planned {
                        device.move_to(Stage::Excluded, now, "left out by the operator")?;
                        device.wave = None;
                    }
                }
                format!("{name} is left out.")
            }
            DeviceChoice::Include => {
                project.left_out.remove(device_id);
                if let Some(i) = held {
                    let device = &mut project.devices[i];
                    if device.stage == Stage::Excluded {
                        device.move_to(Stage::Planned, now, "put back by the operator")?;
                    }
                }
                format!("{name} is back in.")
            }
        };
        self.record(&AuditEntry::new("migration.device.choice", &format!("{message} (project {})", project.name), Ok(()), json!({ "project": project.id, "device": device_id })));
        self.migration_save(&mut project).await?;
        Ok(message)
    }

    /// Adds the ready devices to the project; the others stay out until what blocks them is fixed.
    pub async fn migration_plan_add(&self, id: &str, instance: &str) -> Result<PlanAdded, String> {
        let (_, report) = self.migration_plan_saved(id, instance, None).await?;
        Ok(PlanAdded { added: report.added, already_in: report.already_in, blocked: report.blocked.len() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        migration::{
            planner::PlanSummary,
            project::DestEcoverse,
            survey::{ServerSummary, Survey},
        },
        ncentral::model::{OrgUnitType, ServerInfo},
    };

    fn unit(id: &str, name: &str, kind: OrgUnitType, parent: Option<&str>) -> OrgUnit {
        OrgUnit { org_unit_id: id.into(), org_unit_name: name.into(), org_unit_type: kind, parent_id: parent.map(String::from), external_id: None }
    }

    fn planned(id: &str, name: &str, customer: &str, site: Option<&str>, dest: Option<&str>, path: Option<DeliveryPath>, blockers: Vec<Blocker>) -> PlannedDevice {
        PlannedDevice {
            device_id: id.into(),
            name: name.into(),
            source_customer_id: Some(customer.into()),
            source_site_id: site.map(String::from),
            dest_org_unit_id: dest.map(String::from),
            path,
            asset_id: None,
            blockers,
            warnings: vec![],
            excluded: false,
        }
    }

    fn survey(planned: Vec<PlannedDevice>) -> Survey {
        let info = || ServerInfo { version: "1".into(), ncentral: "2026.4.1.1".into() };
        let summary = crate::migration::planner::summarize(&planned);
        Survey {
            inputs: crate::migration::survey::SurveyInputs { devices: vec![], tokens: Default::default(), eco: Default::default(), ecoverse_linked: None, source_server_id: "a".into() },
            source: ServerSummary { label: "Old".into(), host: "old".into(), info: info() },
            destination: ServerSummary { label: "New".into(), host: "new".into(), info: info() },
            warnings: vec!["careful".into()],
            source_units: vec![unit("1", "Zeta", OrgUnitType::Customer, None), unit("2", "Acme", OrgUnitType::Customer, None), unit("21", "HQ", OrgUnitType::Site, Some("2"))],
            dest_units: vec![unit("110", "Acme", OrgUnitType::Customer, None)],
            mapping: vec![],
            tokens: Default::default(),
            ecoverse_linked: None,
            planned,
            summary: PlanSummary { ..summary },
        }
    }

    #[test]
    fn rows_are_sorted_by_customer_then_name_and_say_where_each_goes() {
        let s = survey(vec![
            planned("3", "PC-B", "1", None, None, None, vec![Blocker::Unmapped]),
            planned("2", "PC-Z", "2", Some("21"), Some("110"), Some(DeliveryPath::Rest), vec![]),
            planned("1", "PC-A", "2", Some("21"), Some("110"), Some(DeliveryPath::Ecoverse), vec![]),
        ]);
        let p = Project::new("p".into(), "x", "a", "b", DestEcoverse::NotLinked, chrono::Utc::now());
        let view = plan_view(&s, &p, &PlanQuery::all());
        assert_eq!(view.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["PC-A", "PC-Z", "PC-B"]);
        assert_eq!((view.rows[0].customer.as_str(), view.rows[0].site.as_deref(), view.rows[0].destination.as_deref()), ("Acme", Some("HQ"), Some("Acme")));
        assert_eq!((view.rows[2].customer.as_str(), view.rows[2].destination.clone()), ("Zeta", None));
    }

    #[test]
    fn counts_separate_new_ready_devices_from_blocked_left_out_and_already_added() {
        let mut left = planned("4", "PC-L", "2", None, None, None, vec![]);
        left.excluded = true;
        let s = survey(vec![
            planned("1", "PC-A", "2", None, Some("110"), Some(DeliveryPath::Ecoverse), vec![]),
            planned("2", "PC-B", "2", None, Some("110"), Some(DeliveryPath::Rest), vec![]),
            planned("3", "PC-C", "1", None, None, None, vec![Blocker::Unmapped, Blocker::TokenMissing]),
            left,
        ]);
        let mut p = Project::new("p".into(), "x", "a", "b", DestEcoverse::NotLinked, chrono::Utc::now());
        p.add_planned(&[s.planned[1].clone()], chrono::Utc::now());
        let view = plan_view(&s, &p, &PlanQuery::all());
        assert_eq!((view.total, view.ready_new, view.ready_ecoverse, view.ready_rest), (4, 1, 1, 0), "PC-B is already in the project");
        assert_eq!((view.blocked, view.left_out, view.in_project), (1, 1, 1));
        assert_eq!(view.rows.iter().find(|r| r.device_id == "2").unwrap().stage, Some(Stage::Planned));
        assert_eq!(view.blocked_by.iter().map(|b| (b.blocker, b.devices)).collect::<Vec<_>>(), [(Blocker::Unmapped, 1), (Blocker::TokenMissing, 1)]);
        assert_eq!(view.warnings, ["careful"]);
    }

    fn device_json(id: &str, customer: &str) -> crate::ncentral::model::Device {
        serde_json::from_value(json!({ "deviceId": id, "longName": format!("PC-{id}"), "customerId": customer, "osId": "winnt", "lastApplianceCheckinTime": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string() })).unwrap()
    }

    fn real_survey() -> Survey {
        use crate::{migration::mapping::auto_map, ncentral::model::TokenStatus};
        let source_units = vec![unit("1", "Acme", OrgUnitType::Customer, None)];
        let dest_units = vec![unit("110", "Acme", OrgUnitType::Customer, None)];
        let mapping = auto_map(&source_units, &dest_units);
        let mut tokens = std::collections::HashMap::new();
        tokens.insert("110".to_string(), TokenStatus::Valid { expires: chrono::DateTime::parse_from_rfc3339("2099-01-01T00:00:00Z").unwrap() });
        let inputs = crate::migration::survey::SurveyInputs { devices: vec![device_json("1", "1"), device_json("2", "1")], tokens, eco: Default::default(), ecoverse_linked: Some(true), source_server_id: "a".into() };
        let mut survey = survey(vec![]);
        survey.inputs = inputs;
        survey.source_units = source_units;
        survey.dest_units = dest_units;
        survey.mapping = mapping;
        survey
    }

    #[test]
    fn the_plan_is_worked_out_again_from_what_was_read_with_the_operators_choices() {
        let mut survey = real_survey();
        let mut p = Project::new("p".into(), "x", "a", "b", DestEcoverse::NotLinked, chrono::Utc::now());
        survey.replan(&p, true);
        assert!(survey.planned.iter().all(|d| d.is_ready() && d.path == Some(DeliveryPath::Rest)), "no Ecoverse assets, so N-central");

        p.path_overrides.insert("1".into(), DeliveryPath::Ecoverse);
        p.left_out.insert("2".into());
        survey.replan(&p, true);
        let by = |id: &str| survey.planned.iter().find(|d| d.device_id == id).unwrap();
        assert!(by("1").blockers.contains(&Blocker::NoEcoversePath) && by("1").warnings.contains(&Warning::PathOverridden), "Ecoverse chosen for a device with no asset is explained, not silently used");
        assert!(by("2").excluded && !by("2").is_ready());
        let view = plan_view(&survey, &p, &PlanQuery::all());
        let row = |id: &str| view.rows.iter().find(|r| r.device_id == id).unwrap().clone();
        assert!(row("1").path_chosen && !row("1").left_out_by_choice);
        assert!(row("2").left_out_by_choice && row("2").left_out);

        survey.replan(&Project::new("p".into(), "x", "a", "b", DestEcoverse::NotLinked, chrono::Utc::now()), false);
        assert!(survey.planned.iter().all(|d| d.blockers.contains(&Blocker::RestScriptMissing)), "the script flag is read each time, so uploading it changes the plan at once");
    }

    async fn saved_with_two_planned() -> (std::sync::Arc<Core>, String) {
        let core = Core::for_tests("choice");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let mut p = core.migration_create_full("Choice", &src.id, &dst.id, DestEcoverse::NotLinked, vec![], vec![]).await.unwrap();
        p.add_planned(&[planned("1", "PC-1", "1", None, Some("110"), Some(DeliveryPath::Ecoverse), vec![]), planned("2", "PC-2", "1", None, Some("110"), Some(DeliveryPath::Rest), vec![])], chrono::Utc::now());
        p.devices[0].asset_id = Some("a1".into());
        p.auto_wave("", None, chrono::Utc::now()).unwrap();
        core.migration_save(&mut p).await.unwrap();
        (core, p.id)
    }

    #[tokio::test]
    async fn leaving_a_device_out_takes_it_out_of_its_wave_and_including_it_puts_it_back_planned() {
        let (core, id) = saved_with_two_planned().await;
        core.migration_device_choice(&id, "me", "2", DeviceChoice::LeaveOut).await.unwrap();
        let p = core.migration_load(&id).await.unwrap().unwrap();
        assert_eq!((p.devices[1].stage, p.devices[1].wave), (Stage::Excluded, None));
        assert!(p.left_out.contains("2"));
        core.migration_device_choice(&id, "me", "2", DeviceChoice::Include).await.unwrap();
        let p = core.migration_load(&id).await.unwrap().unwrap();
        assert_eq!(p.devices[1].stage, Stage::Planned);
        assert!(p.left_out.is_empty());
    }

    #[tokio::test]
    async fn a_path_can_be_chosen_for_a_planned_device_but_ecoverse_needs_an_asset_and_a_launched_device_is_left_alone() {
        let (core, id) = saved_with_two_planned().await;
        core.migration_device_choice(&id, "me", "1", DeviceChoice::UsePath { path: DeliveryPath::Rest }).await.unwrap();
        let p = core.migration_load(&id).await.unwrap().unwrap();
        assert_eq!((p.devices[0].path, p.path_overrides.get("1").copied()), (Some(DeliveryPath::Rest), Some(DeliveryPath::Rest)));

        let error = core.migration_device_choice(&id, "me", "2", DeviceChoice::UsePath { path: DeliveryPath::Ecoverse }).await.unwrap_err();
        assert!(error.contains("no Ecoverse asset"), "{error}");
        assert!(!core.migration_load(&id).await.unwrap().unwrap().path_overrides.contains_key("2"), "a refused choice isn't kept");

        let mut p = core.migration_load(&id).await.unwrap().unwrap();
        p.devices[0].stage = Stage::Launched;
        core.migration_save(&mut p).await.unwrap();
        for choice in [DeviceChoice::LeaveOut, DeviceChoice::UsePath { path: DeliveryPath::Ecoverse }] {
            assert!(core.migration_device_choice(&id, "me", "1", choice).await.unwrap_err().contains("before it is launched"));
        }
        assert!(core.migration_device_choice(&id, "someone", "2", DeviceChoice::LeaveOut).await.unwrap_err().contains("Another N-voke window"));
    }

    #[test]
    fn a_query_filters_by_show_customer_and_text_and_sends_one_page_with_the_total() {
        let mut devices = vec![];
        for n in 0..250 {
            devices.push(planned(&n.to_string(), &format!("PC-{n:03}"), "2", None, Some("110"), Some(DeliveryPath::Rest), vec![]));
        }
        devices.push(planned("900", "MAC-1", "1", None, None, None, vec![Blocker::NotWindows]));
        let s = survey(devices);
        let p = Project::new("p".into(), "x", "a", "b", DestEcoverse::NotLinked, chrono::Utc::now());

        let first = plan_view(&s, &p, &PlanQuery::all());
        assert_eq!((first.total, first.matching, first.rows.len(), first.offset), (251, 251, 200, 0), "a default page of 200, with the full count");
        assert_eq!(first.customers, ["Acme", "Zeta"]);

        let second = plan_view(&s, &p, &PlanQuery { offset: 200, ..PlanQuery::all() });
        assert_eq!((second.rows.len(), second.rows[0].name.as_str()), (51, "PC-200"));

        let blocked = plan_view(&s, &p, &PlanQuery { show: PlanShow::Blocked, ..PlanQuery::all() });
        assert_eq!((blocked.matching, blocked.rows[0].name.as_str()), (1, "MAC-1"));
        let ready_zeta = plan_view(&s, &p, &PlanQuery { show: PlanShow::Ready, customer: Some("Zeta".into()), ..PlanQuery::all() });
        assert_eq!(ready_zeta.matching, 0);
        let searched = plan_view(&s, &p, &PlanQuery { text: " pc-24 ".into(), limit: 3, ..PlanQuery::all() });
        assert_eq!((searched.matching, searched.rows.len()), (10, 3), "PC-240 to PC-249, three to a page");

        let huge = plan_view(&s, &p, &PlanQuery { limit: 100_000, ..PlanQuery::all() });
        assert_eq!(huge.rows.len(), 251, "a page is capped, and this one is smaller than the cap");
    }
}
