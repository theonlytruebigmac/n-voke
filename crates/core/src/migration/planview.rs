//! What the Plan step shows: the source's devices as the plan sees them today, which are ready, which are blocked
//! and why, and which are already in the project. Reading it changes nothing; adding the ready ones is
//! `migration_plan_saved`.

use std::collections::HashMap;

use serde::Serialize;

use super::{
    planner::{Blocker, DeliveryPath, PlannedDevice, Warning},
    project::{Project, Stage},
    survey::{Survey, SurveyOptions},
};
use crate::{Core, ncentral::model::OrgUnit};

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
pub fn plan_view(survey: &Survey, project: &Project) -> PlanView {
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
        stage: held.get(d.device_id.as_str()).copied(),
    };
    let mut rows: Vec<PlanRow> = survey.planned.iter().map(row).collect();
    rows.sort_by(|a, b| a.customer.to_lowercase().cmp(&b.customer.to_lowercase()).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.device_id.cmp(&b.device_id)));

    let ready_new: Vec<&PlanRow> = rows.iter().filter(|r| r.ready && r.stage.is_none()).collect();
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
        rows,
    }
}

impl Core {
    /// Reads both servers (and Ecoverse) and plans the source's devices. Slow on a large server, so the page asks for
    /// it when the step opens and again on request.
    pub async fn migration_plan_view(&self, id: &str) -> Result<PlanView, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let options = SurveyOptions {
            source_server_id: project.source_server_id.clone(),
            dest_server_id: project.dest_server_id.clone(),
            ecoverse_org_ids: project.source_ecoverse_org_ids.clone(),
            customer: None,
            rest_script_ready: project.rest_scripts.contains_key(&project.source_server_id),
        };
        let survey = self.migration_survey(&options, Some(&project)).await?;
        Ok(plan_view(&survey, &project))
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
        let view = plan_view(&s, &p);
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
        let view = plan_view(&s, &p);
        assert_eq!((view.total, view.ready_new, view.ready_ecoverse, view.ready_rest), (4, 1, 1, 0), "PC-B is already in the project");
        assert_eq!((view.blocked, view.left_out, view.in_project), (1, 1, 1));
        assert_eq!(view.rows.iter().find(|r| r.device_id == "2").unwrap().stage, Some(Stage::Planned));
        assert_eq!(view.blocked_by.iter().map(|b| (b.blocker, b.devices)).collect::<Vec<_>>(), [(Blocker::Unmapped, 1), (Blocker::TokenMissing, 1)]);
        assert_eq!(view.warnings, ["careful"]);
    }
}
