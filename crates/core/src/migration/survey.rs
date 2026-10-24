//! Reading everything a migration plan depends on and putting it together: the
//! Connect checks, the mapping, the devices, the destination's registration tokens
//! and what Ecoverse knows, then the plan. Nothing here changes anything. It is
//! what the `nvoke migrate-plan` command prints, and what the Map and Plan screens
//! will be built on.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{Duration, Utc};

use super::{
    ecoverse::{index_by_ncentral_id, is_linked},
    mapping::{MappingEntry, MappingStatus, apply_overrides, auto_map, normalize_name},
    planner::{DeliveryPath, EcoAsset, PlanInput, PlanOptions, PlanSummary, PlannedDevice, plan, summarize, token_targets},
    project::{Delivery, Project},
};
use crate::{
    Core,
    ncentral::model::{Device, OrgUnit, ServerInfo, TokenStatus},
};

/// A token that ends within this long is reported as expiring.
const TOKEN_WARNING: Duration = Duration::days(7);
/// What N-voke was tested against: N-central 2026.4 and later.
pub(crate) const TESTED_RELEASE: (u32, u32) = (2026, 4);

pub struct SurveyOptions {
    pub source_server_id: String,
    pub dest_server_id: String,
    /// Source-side Ecoverse organizations to search for assets. Empty skips Ecoverse.
    pub ecoverse_org_ids: Vec<String>,
    /// Only source customers whose name contains this (case and punctuation ignored).
    pub customer: Option<String>,
    /// The endpoint script has been uploaded to the source server.
    pub rest_script_ready: bool,
}

#[derive(Clone)]
pub struct ServerSummary {
    pub label: String,
    pub host: String,
    pub info: ServerInfo,
}

/// What the survey read, kept so the plan can be worked out again (with new choices) without reading the servers.
#[derive(Clone)]
pub struct SurveyInputs {
    pub devices: Vec<Device>,
    pub tokens: HashMap<String, TokenStatus>,
    pub eco: HashMap<String, EcoAsset>,
    pub ecoverse_linked: Option<bool>,
    pub source_server_id: String,
}

#[derive(Clone)]
pub struct Survey {
    pub inputs: SurveyInputs,
    pub source: ServerSummary,
    pub destination: ServerSummary,
    pub warnings: Vec<String>,
    pub source_units: Vec<OrgUnit>,
    pub dest_units: Vec<OrgUnit>,
    pub mapping: Vec<MappingEntry>,
    pub tokens: HashMap<String, TokenStatus>,
    /// Whether Ecoverse was searched and sees the source server's devices.
    pub ecoverse_linked: Option<bool>,
    pub planned: Vec<PlannedDevice>,
    pub summary: PlanSummary,
}

impl Core {
    pub async fn migration_survey(&self, options: &SurveyOptions, project: Option<&Project>) -> Result<Survey, String> {
        let source = self.ncentral_server(&options.source_server_id)?;
        let dest = self.ncentral_server(&options.dest_server_id)?;
        let mut warnings = vec![];

        let mut summaries = Vec::new();
        for server in [&source, &dest] {
            let info = self.ncentral_server_info(&server.id).await.map_err(|e| format!("{}: {e}", server.label))?;
            if info.release().is_none_or(|r| r < TESTED_RELEASE) {
                warnings.push(format!("{} runs N-central {}; migration was tested on 2026.4 and later.", server.label, info.ncentral));
            }
            summaries.push(ServerSummary { label: server.label.clone(), host: server.host.clone(), info });
        }
        let destination = summaries.pop().expect("two servers");
        let source_summary = summaries.pop().expect("two servers");

        let source_units = self.org_units_cached(&source.id).await.map_err(|e| format!("{}: {e}", source.label))?;
        let dest_units = if dest.id == source.id { source_units.clone() } else { self.org_units_cached(&dest.id).await.map_err(|e| format!("{}: {e}", dest.label))? };
        let mut mapping = auto_map(&source_units, &dest_units);
        if let Some(project) = project {
            apply_overrides(&mut mapping, &project.mapping_overrides, &dest_units);
        }

        let wanted = options.customer.as_deref().map(normalize_name).filter(|n| !n.is_empty());
        let customers: Vec<&OrgUnit> = source_units
            .iter()
            .filter(|u| u.org_unit_type == crate::ncentral::model::OrgUnitType::Customer)
            .filter(|u| wanted.as_ref().is_none_or(|w| normalize_name(&u.org_unit_name).contains(w.as_str())))
            .collect();
        if customers.is_empty() {
            return Err(match &options.customer {
                Some(name) => format!("No customer on {} matches \"{name}\".", source.label),
                None => format!("{} has no customers.", source.label),
            });
        }
        let mut devices: BTreeMap<String, Device> = BTreeMap::new();
        for customer in customers {
            for device in self.org_unit_devices_cached(&source.id, &customer.org_unit_id).await.map_err(|e| format!("{}: {e}", source.label))? {
                devices.entry(device.device_id.clone()).or_insert(device);
            }
        }
        let devices: Vec<Device> = devices.into_values().collect();

        // Tokens: read one per destination unit that devices will go to.
        let now = Utc::now();
        let mut tokens = HashMap::new();
        for target in token_targets(&devices, &mapping) {
            match self.registration_token_cached(&dest.id, &target).await {
                Ok(token) => {
                    tokens.insert(target, token.status(now, TOKEN_WARNING));
                }
                Err(e) => {
                    let name = dest_units.iter().find(|u| u.org_unit_id == target).map_or(target.as_str(), |u| u.org_unit_name.as_str());
                    warnings.push(format!("Could not read the registration token for {name} on {}: {e}", dest.label));
                }
            }
        }

        // Ecoverse is read when the project wants it and the source server uses it. A failed read isn't fatal: every
        // device is then planned on the REST path and the report says why.
        let use_ecoverse = self.server_uses_ecoverse(&source.id) && project.is_none_or(|p| p.options.delivery == Delivery::EcoverseFirst);
        let (eco, ecoverse_linked) = if !use_ecoverse {
            (HashMap::new(), None)
        } else {
            match self.with_ecoverse(&source.id, self.eco_devices(&options.ecoverse_org_ids)).await {
                Ok(assets) => {
                    let linked = is_linked(&assets, &source.host);
                    if !linked {
                        warnings.push(format!("Ecoverse doesn't show any device of {}, so every device is planned on the REST path.", source.host));
                    }
                    (index_by_ncentral_id(&assets, &source.host), Some(linked))
                }
                Err(error) => {
                    warnings.push(format!("Ecoverse couldn't be read ({error}), so every device is planned on the REST path."));
                    (HashMap::new(), None)
                }
            }
        };

        let inputs = SurveyInputs { devices, tokens: tokens.clone(), eco, ecoverse_linked, source_server_id: source.id.clone() };
        let planned = plan_devices(&inputs, &mapping, project, options.rest_script_ready, now);
        let summary = summarize(&planned);
        Ok(Survey {
            inputs, source: ServerSummary { label: source_summary.label, host: source_summary.host, info: source_summary.info }, destination, warnings, source_units, dest_units, mapping, tokens, ecoverse_linked, planned, summary })
    }
}

/// The plan for the surveyed devices, with the project's choices (path overrides, devices left out) when there is one.
fn plan_devices(inputs: &SurveyInputs, mapping: &[MappingEntry], project: Option<&Project>, rest_script_ready: bool, now: chrono::DateTime<Utc>) -> Vec<PlannedDevice> {
    let (no_overrides, no_left_out) = (HashMap::new(), BTreeSet::new());
    plan(&PlanInput {
        devices: &inputs.devices,
        mapping,
        tokens: &inputs.tokens,
        eco: &inputs.eco,
        ecoverse_available: inputs.ecoverse_linked == Some(true),
        rest_script_ready: rest_script_ready || project.is_some_and(|p| p.rest_scripts.contains_key(&inputs.source_server_id)),
        path_overrides: project.map_or(&no_overrides, |p| &p.path_overrides),
        excluded: project.map_or(&no_left_out, |p| &p.left_out),
        now,
        options: PlanOptions::default(),
    })
}

impl Survey {
    /// Works the plan out again from what was read, with the project's current choices. Nothing is read from a server.
    pub fn replan(&mut self, project: &Project, rest_script_ready: bool) {
        self.planned = plan_devices(&self.inputs, &self.mapping, Some(project), rest_script_ready, Utc::now());
        self.summary = summarize(&self.planned);
    }

    /// A plain-text report for a terminal.
    pub fn render(&self) -> String {
        let name_of = |units: &[OrgUnit], id: &str| units.iter().find(|u| u.org_unit_id == id).map_or_else(|| format!("#{id}"), |u| format!("{} [{id}]", u.org_unit_name));
        let mut out = String::new();
        let line = |out: &mut String, text: String| {
            out.push_str(&text);
            out.push('\n');
        };
        for (role, s) in [("Source", &self.source), ("Destination", &self.destination)] {
            line(&mut out, format!("{role}: {} ({}), N-central {}, REST API {}", s.label, s.host, s.info.ncentral, s.info.version));
        }
        match self.ecoverse_linked {
            None => line(&mut out, "Ecoverse: not searched".into()),
            Some(true) => line(&mut out, "Ecoverse: sees the source server's devices".into()),
            Some(false) => line(&mut out, "Ecoverse: does not see the source server's devices".into()),
        }
        for warning in &self.warnings {
            line(&mut out, format!("! {warning}"));
        }

        let mapped = self.mapping.iter().filter(|e| e.status() == MappingStatus::Mapped).count();
        line(&mut out, format!("\nMapping: {mapped} of {} customers and sites matched", self.mapping.len()));
        for entry in &self.mapping {
            let to = match (entry.status(), &entry.dest_id) {
                (MappingStatus::Mapped, Some(dest)) => format!("{} ({})", name_of(&self.dest_units, dest), entry.confidence.map_or("chosen".to_string(), |c| format!("{c:?}"))),
                (MappingStatus::Excluded, _) => "excluded".to_string(),
                (MappingStatus::Ambiguous, _) => format!("AMBIGUOUS: {}", entry.candidates.iter().map(|c| name_of(&self.dest_units, c)).collect::<Vec<_>>().join(", ")),
                _ => "NO MATCH".to_string(),
            };
            line(&mut out, format!("  {:?} {} [{}] -> {to}", entry.kind, entry.source_name, entry.source_id));
        }

        let s = &self.summary;
        line(&mut out, format!("\nPlan: {} devices: {} ready ({} Ecoverse, {} REST), {} left out", s.total, s.ready, s.ready_ecoverse, s.ready_rest, s.excluded));
        for (blocker, count) in &s.blocked_by {
            line(&mut out, format!("  {count} blocked: {}", blocker.message()));
        }
        for device in &self.planned {
            let path = match device.path {
                Some(DeliveryPath::Ecoverse) => "Ecoverse",
                Some(DeliveryPath::Rest) => "REST",
                None => "-",
            };
            let dest = device.dest_org_unit_id.as_deref().map_or("-".to_string(), |d| name_of(&self.dest_units, d));
            let flags: Vec<String> = device.blockers.iter().map(|b| format!("BLOCKED {b:?}")).chain(device.warnings.iter().map(|w| format!("{w:?}"))).collect();
            line(&mut out, format!("  {:<28} -> {:<24} {:<8} {}", device.name, dest, path, flags.join(", ")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ncentral::rest::{
        RestRequest,
        testing::{Fake, auth_ok, ok},
    };

    fn units(prefix: u32) -> serde_json::Value {
        let id = |n: u32| (prefix + n).to_string();
        json!({ "data": [
            { "orgUnitId": id(0), "orgUnitName": "SO", "orgUnitType": "SO" },
            { "orgUnitId": id(10), "orgUnitName": "Acme", "orgUnitType": "CUSTOMER", "parentId": id(0) },
            { "orgUnitId": id(11), "orgUnitName": "HQ", "orgUnitType": "SITE", "parentId": id(10) },
            { "orgUnitId": id(20), "orgUnitName": "Globex", "orgUnitType": "CUSTOMER", "parentId": id(0) }
        ], "totalPages": 1 })
    }

    fn lab(req: &RestRequest, _: usize) -> crate::ncentral::rest::RestResponse {
        let url = req.url.trim_start_matches("https://");
        let (host, path) = url.split_once('/').unwrap();
        let path = path.split('?').next().unwrap();
        match (host, path) {
            (_, "api/auth/authenticate") => auth_ok(3600),
            ("src.example.com", "api/server-info") => ok(json!({ "ncentral": "2026.4.0.25", "version": "1.3.23" })),
            ("dst.example.com", "api/server-info") => ok(json!({ "ncentral": "2025.5.0.1", "version": "1.3.0" })),
            ("src.example.com", "api/org-units") => ok(units(0)),
            ("dst.example.com", "api/org-units") => ok(units(100)),
            ("src.example.com", "api/org-units/10/devices") => ok(json!({ "data": [
                { "deviceId": 1, "longName": "ACME-PC1", "osId": "winnt", "customerId": 10, "siteId": 11, "lastApplianceCheckinTime": "2999-01-01T00:00:00Z" },
                { "deviceId": 2, "longName": "ACME-PC2", "osId": "winnt", "customerId": 10 },
                { "deviceId": 3, "longName": "ACME-MAC", "osId": "macos", "customerId": 10 }
            ], "totalPages": 1 })),
            ("src.example.com", "api/org-units/20/devices") => ok(json!({ "data": [], "totalPages": 1 })),
            ("dst.example.com", "api/org-units/110/registration-token") => ok(json!({ "data": { "registrationToken": "tok-secret-110", "registrationTokenExpiryDate": "2999-01-01T00:00:00-05:00[America/New_York]" } })),
            ("dst.example.com", "api/org-units/111/registration-token") => ok(json!({ "data": { "registrationToken": null, "registrationTokenExpiryDate": null } })),
            other => panic!("unexpected request {other:?}"),
        }
    }

    async fn setup() -> (std::sync::Arc<Core>, String, String, std::sync::Arc<Fake>) {
        let core = Core::for_tests("survey");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let fake = Fake::new(lab);
        core.set_rest_transport(fake.clone());
        (core, src.id, dst.id, fake)
    }

    #[tokio::test]
    async fn a_survey_connects_maps_reads_tokens_and_plans() {
        let (core, src, dst, fake) = setup().await;
        let options = SurveyOptions { source_server_id: src, dest_server_id: dst, ecoverse_org_ids: vec![], customer: Some("acme".into()), rest_script_ready: true };
        let survey = core.migration_survey(&options, None).await.unwrap();

        assert_eq!(survey.mapping.len(), 3, "two customers and a site");
        assert_eq!(survey.planned.len(), 3, "only Acme's devices were read");
        let by_name = |n: &str| survey.planned.iter().find(|d| d.name == n).unwrap();
        // PC1 is in the HQ site, whose destination has no token; PC2 goes to the customer.
        assert_eq!(by_name("ACME-PC1").dest_org_unit_id.as_deref(), Some("111"));
        assert_eq!(by_name("ACME-PC1").blockers, [crate::migration::planner::Blocker::TokenMissing]);
        assert!(by_name("ACME-PC2").is_ready());
        assert_eq!(by_name("ACME-MAC").blockers, [crate::migration::planner::Blocker::NotWindows]);
        assert_eq!((survey.summary.ready, survey.summary.ready_rest, survey.summary.ready_ecoverse), (1, 1, 0));
        assert!(survey.warnings.iter().any(|w| w.contains("New runs N-central 2025.5.0.1")), "{:?}", survey.warnings);
        assert!(fake.seen().iter().all(|r| r.method == crate::ncentral::rest::Method::Get || r.url.ends_with("/authenticate")), "a survey only reads");
    }

    #[tokio::test]
    async fn the_report_names_the_mapping_the_blockers_and_never_a_token() {
        let (core, src, dst, _) = setup().await;
        let options = SurveyOptions { source_server_id: src, dest_server_id: dst, ecoverse_org_ids: vec![], customer: None, rest_script_ready: false };
        let text = core.migration_survey(&options, None).await.unwrap().render();
        assert!(text.contains("Source: Old (src.example.com), N-central 2026.4.0.25, REST API 1.3.23"), "{text}");
        assert!(text.contains("Mapping: 3 of 3 customers and sites matched"), "{text}");
        assert!(text.contains("Customer Acme [10] -> Acme [110] (ExactUnderParent)"), "{text}");
        assert!(text.contains("BLOCKED RestScriptMissing"), "{text}");
        assert!(text.contains("Not a Windows device"), "{text}");
        assert!(!text.contains("tok-secret"), "{text}");
    }

    #[tokio::test]
    async fn an_unknown_customer_is_an_error_not_an_empty_plan() {
        let (core, src, dst, _) = setup().await;
        let options = SurveyOptions { source_server_id: src, dest_server_id: dst, ecoverse_org_ids: vec![], customer: Some("initech".into()), rest_script_ready: true };
        assert!(core.migration_survey(&options, None).await.err().unwrap().contains("No customer on Old matches \"initech\""));
    }
}
