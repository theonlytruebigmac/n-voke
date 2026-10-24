//! Creating the customers and sites the destination is missing, so the devices that have nowhere to go get somewhere.
//! Customers are created first (under one service organization), then sites under their customer. Every creation is
//! audited. N-central has no delete call, so what is created here stays until someone removes it in N-central; the
//! preview says so.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::{
    mapping::{MappingEntry, MappingStatus, Override, apply_overrides, auto_map},
    project::Project,
};
use crate::{
    Core,
    audit::AuditEntry,
    ncentral::{
        capabilities::Capability,
        model::{OrgUnit, OrgUnitType},
    },
};
use serde_json::json;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "NewUnit.ts"))]
#[serde(rename_all = "camelCase")]
pub struct NewUnit {
    pub source_id: String,
    pub name: String,
    pub is_site: bool,
    /// The customer it goes under, by name: one that exists on the destination, or one being created here.
    pub under: String,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ServiceOrgChoice.ts"))]
#[serde(rename_all = "camelCase")]
pub struct ServiceOrgChoice {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "StructurePreview.ts"))]
#[serde(rename_all = "camelCase")]
pub struct StructurePreview {
    pub dest_host: String,
    /// Customers that would be created, then sites.
    pub customers: Vec<NewUnit>,
    pub sites: Vec<NewUnit>,
    /// Sites that can't be created because their customer is left out or hasn't been decided.
    pub skipped: Vec<String>,
    /// The destination's service organizations. One is chosen by default when there is only one.
    pub service_orgs: Vec<ServiceOrgChoice>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "StructureReport.ts"))]
#[serde(rename_all = "camelCase")]
pub struct StructureReport {
    pub customers_created: Vec<String>,
    pub sites_created: Vec<String>,
    /// Already on the destination under that name: nothing was created. Choose them in Map.
    pub already_there: Vec<String>,
    pub failed: Vec<(String, String)>,
    /// A dry run: what would have been created.
    pub would_create: Vec<String>,
    /// How many registration tokens were generated for the new units.
    pub tokens_generated: usize,
    /// Created, but missing from the destination's list afterwards: the signed-in user can't see them (yet).
    pub not_visible: Vec<String>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "StructureKind.ts"))]
#[serde(rename_all = "camelCase")]
pub enum StructureKind {
    Customer,
    Site,
    Tokens,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "StructureState.ts"))]
#[serde(rename_all = "camelCase")]
pub enum StructureState {
    Running,
    Created,
    AlreadyThere,
    WouldCreate,
    Failed,
}

/// One step of creating the missing structure, for the page to show as it happens.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "StructureProgress.ts"))]
#[serde(rename_all = "camelCase")]
pub struct StructureProgress {
    pub name: String,
    pub kind: StructureKind,
    /// 1-based, of `total` (the customers and sites; the token step reports `total` of `total`).
    pub position: usize,
    pub total: usize,
    pub state: StructureState,
    pub why: Option<String>,
}

fn entries(source: &[OrgUnit], dest: &[OrgUnit], overrides: &HashMap<String, Override>) -> Vec<MappingEntry> {
    let mut mapping = auto_map(source, dest);
    apply_overrides(&mut mapping, overrides, dest);
    mapping
}

/// What the destination is missing: unmatched customers, and unmatched sites whose customer exists or is being created.
/// `only` narrows it to those source IDs (and the customers they need).
pub fn plan_structure(source: &[OrgUnit], dest: &[OrgUnit], overrides: &HashMap<String, Override>, only: Option<&HashSet<String>>) -> (Vec<NewUnit>, Vec<NewUnit>, Vec<String>) {
    let mapping = entries(source, dest, overrides);
    let name_of = |id: &str| source.iter().find(|u| u.org_unit_id == id).map(|u| u.org_unit_name.clone()).unwrap_or_default();
    let dest_name = |id: &str| dest.iter().find(|u| u.org_unit_id == id).map(|u| u.org_unit_name.clone()).unwrap_or_default();
    let unmatched = |e: &MappingEntry, kind: OrgUnitType| e.kind == kind && e.status() == MappingStatus::Unmatched;

    let mut wanted_customers: Vec<&MappingEntry> = mapping.iter().filter(|e| unmatched(e, OrgUnitType::Customer)).collect();
    let mut sites: Vec<&MappingEntry> = mapping.iter().filter(|e| unmatched(e, OrgUnitType::Site)).collect();
    if let Some(only) = only {
        sites.retain(|e| only.contains(&e.source_id));
        let needed: HashSet<String> = sites.iter().filter_map(|e| source.iter().find(|u| u.org_unit_id == e.source_id).and_then(|u| u.parent_id.clone())).collect();
        wanted_customers.retain(|e| only.contains(&e.source_id) || needed.contains(&e.source_id));
    }
    let creating: HashSet<&str> = wanted_customers.iter().map(|e| e.source_id.as_str()).collect();

    let (mut new_sites, mut skipped) = (vec![], vec![]);
    for site in sites {
        let parent = source.iter().find(|u| u.org_unit_id == site.source_id).and_then(|u| u.parent_id.clone());
        let parent_entry = parent.as_deref().and_then(|p| mapping.iter().find(|e| e.source_id == p));
        let under = match parent_entry {
            Some(p) if creating.contains(p.source_id.as_str()) => Some(p.source_name.clone()),
            Some(p) if p.status() == MappingStatus::Mapped => p.dest_id.as_deref().map(dest_name),
            _ => None,
        };
        match under {
            Some(under) => new_sites.push(NewUnit { source_id: site.source_id.clone(), name: site.source_name.clone(), is_site: true, under }),
            None => skipped.push(format!("{} (its customer {} has no destination)", site.source_name, parent.as_deref().map(name_of).unwrap_or_default())),
        }
    }
    let customers = wanted_customers.iter().map(|e| NewUnit { source_id: e.source_id.clone(), name: e.source_name.clone(), is_site: false, under: String::new() }).collect();
    (customers, new_sites, skipped)
}

impl Core {
    /// Both servers' customers and sites. A preview may use what was read a moment ago; creating reads fresh, so that
    /// something created since (or by someone else) isn't created twice.
    async fn structure_units(&self, project: &Project, fresh: bool) -> Result<(String, Vec<OrgUnit>, Vec<OrgUnit>), String> {
        if project.source_server_id == project.dest_server_id {
            return Err("This project reinstalls on the same server, so there is nothing to create.".into());
        }
        let (source, dest) = (self.ncentral_server(&project.source_server_id)?, self.ncentral_server(&project.dest_server_id)?);
        self.require_capability(&dest.id, Capability::CreateStructure).await?;
        if fresh {
            super::readcache::forget_server(&dest.id);
        }
        let source_units = self.org_units_cached(&source.id).await.map_err(|e| format!("{}: {e}", source.label))?;
        let dest_units = self.org_units_cached(&dest.id).await.map_err(|e| format!("{}: {e}", dest.label))?;
        Ok((dest.host, source_units, dest_units))
    }

    /// What creating the missing structure would do. Nothing is written.
    pub async fn migration_structure_preview(&self, id: &str, only: Option<Vec<String>>) -> Result<StructurePreview, String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        let (host, source, dest) = self.structure_units(&project, false).await?;
        let only: Option<HashSet<String>> = only.map(|o| o.into_iter().collect());
        let (customers, sites, skipped) = plan_structure(&source, &dest, &project.mapping_overrides, only.as_ref());
        let service_orgs = dest.iter().filter(|u| u.org_unit_type == OrgUnitType::ServiceOrg).map(|u| ServiceOrgChoice { id: u.org_unit_id.clone(), name: u.org_unit_name.clone() }).collect();
        Ok(StructurePreview { dest_host: host, customers, sites, skipped, service_orgs })
    }

    /// Creates the missing customers, then their sites, mapping each source unit to what was created and saving after
    /// each so that nothing created is lost. Then generates registration tokens for the new units. With `dry_run`
    /// nothing is sent.
    pub async fn migration_structure_create(&self, id: &str, instance: &str, service_org_id: &str, only: Option<Vec<String>>, dry_run: bool) -> Result<StructureReport, String> {
        self.migration_structure_create_with(id, instance, service_org_id, only, dry_run, &|_| {}).await
    }

    /// The same, telling `progress` as each customer, site and the token step starts and finishes.
    pub async fn migration_structure_create_with(
        &self,
        id: &str,
        instance: &str,
        service_org_id: &str,
        only: Option<Vec<String>>,
        dry_run: bool,
        progress: &(dyn Fn(StructureProgress) + Sync),
    ) -> Result<StructureReport, String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        let (host, source, dest) = self.structure_units(&project, true).await?;
        if !dest.iter().any(|u| u.org_unit_type == OrgUnitType::ServiceOrg && u.org_unit_id == service_org_id) {
            return Err("Choose one of the destination's service organizations to create the customers under.".into());
        }
        let only: Option<HashSet<String>> = only.map(|o| o.into_iter().collect());
        let (customers, sites, _) = plan_structure(&source, &dest, &project.mapping_overrides, only.as_ref());

        let was_dry = self.is_dry_run();
        if dry_run {
            self.set_dry_run(true);
        }
        let mut report = StructureReport::default();
        let mut created = false;
        // Where each source customer is on the destination now: matched already, or created by this run, so a site finds its customer without re-matching every unit.
        let mut customer_dest: HashMap<String, String> = entries(&source, &dest, &project.mapping_overrides)
            .into_iter()
            .filter(|e| e.kind == OrgUnitType::Customer && e.status() == MappingStatus::Mapped)
            .filter_map(|e| Some((e.source_id, e.dest_id?)))
            .collect();
        let parent_of: HashMap<&str, &str> = source.iter().filter_map(|u| Some((u.org_unit_id.as_str(), u.parent_id.as_deref()?))).collect();
        let mut made: Vec<(String, String)> = vec![];
        let total = customers.len() + sites.len();
        let mut position = 0;
        for customer in &customers {
            position += 1;
            let step = |state, why| StructureProgress { name: customer.name.clone(), kind: StructureKind::Customer, position, total, state, why };
            progress(step(StructureState::Running, None));
            match self.ncentral_create_customer(&project.dest_server_id, service_org_id, &customer.name).await {
                Ok(new_id) => {
                    made.push((customer.name.clone(), new_id.clone()));
                    customer_dest.insert(customer.source_id.clone(), new_id.clone());
                    let _ = self.migration_set_mapping(&mut project, &customer.source_id, Override::MapTo(new_id));
                    report.customers_created.push(customer.name.clone());
                    created = true;
                    self.migration_save(&mut project).await?;
                    progress(step(StructureState::Created, None));
                }
                Err(e) if e.is_dry_run() => {
                    report.would_create.push(format!("Customer {}", customer.name));
                    progress(step(StructureState::WouldCreate, None));
                }
                Err(e) if e.is_already_there() => {
                    report.already_there.push(customer.name.clone());
                    progress(step(StructureState::AlreadyThere, None));
                }
                Err(e) => {
                    progress(step(StructureState::Failed, Some(e.message.clone())));
                    report.failed.push((customer.name.clone(), e.message));
                }
            }
        }
        for site in &sites {
            position += 1;
            let step = |state, why| StructureProgress { name: site.name.clone(), kind: StructureKind::Site, position, total, state, why };
            // The customer a site goes under is wherever its source customer is mapped now, including one just created.
            let parent = parent_of.get(site.source_id.as_str()).map(|p| p.to_string());
            let under = parent.as_ref().and_then(|p| customer_dest.get(p)).cloned();
            let Some(under) = under else {
                if dry_run && customers.iter().any(|c| Some(&c.source_id) == parent.as_ref()) {
                    report.would_create.push(format!("Site {} under {}", site.name, site.under));
                    progress(step(StructureState::WouldCreate, None));
                } else {
                    let why = format!("Its customer {} has no destination, so the site can't be created.", site.under);
                    progress(step(StructureState::Failed, Some(why.clone())));
                    report.failed.push((site.name.clone(), why));
                }
                continue;
            };
            progress(step(StructureState::Running, None));
            match self.ncentral_create_site(&project.dest_server_id, &under, &site.name).await {
                Ok(new_id) => {
                    made.push((site.name.clone(), new_id.clone()));
                    let _ = self.migration_set_mapping(&mut project, &site.source_id, Override::MapTo(new_id));
                    report.sites_created.push(site.name.clone());
                    created = true;
                    self.migration_save(&mut project).await?;
                    progress(step(StructureState::Created, None));
                }
                Err(e) if e.is_dry_run() => {
                    report.would_create.push(format!("Site {} under {}", site.name, site.under));
                    progress(step(StructureState::WouldCreate, None));
                }
                Err(e) if e.is_already_there() => {
                    report.already_there.push(site.name.clone());
                    progress(step(StructureState::AlreadyThere, None));
                }
                Err(e) => {
                    progress(step(StructureState::Failed, Some(e.message.clone())));
                    report.failed.push((site.name.clone(), e.message));
                }
            }
        }
        if dry_run && !was_dry {
            self.set_dry_run(false);
        }
        if created {
            // Read the destination again, as the signed-in user sees it: a unit it can't see can't be mapped to or given a token.
            super::readcache::forget_server(&project.dest_server_id);
            if let Ok(now) = self.org_units_cached(&project.dest_server_id).await {
                let listed: HashSet<&str> = now.iter().map(|n| n.org_unit_id.as_str()).collect();
                report.not_visible = made.iter().filter(|(_, id)| !listed.contains(id.as_str())).map(|(name, _)| name.clone()).collect();
            }
            self.record(&AuditEntry::new(
                "migration.structure.done",
                &format!("Created {} customers and {} sites on {host} for {}", report.customers_created.len(), report.sites_created.len(), project.name),
                Ok(()),
                json!({ "project": project.id, "customers": report.customers_created.len(), "sites": report.sites_created.len(), "failed": report.failed.len() }),
            ));
            // The new units have no registration token yet; the plan can't add their devices without one.
            let step = |state, why| StructureProgress { name: "Registration tokens".into(), kind: StructureKind::Tokens, position: total, total, state, why };
            progress(step(StructureState::Running, None));
            match self.migration_generate_tokens(&project, None).await {
                Ok(tokens) => {
                    report.tokens_generated = tokens.generated.len();
                    progress(step(StructureState::Created, None));
                }
                Err(e) => {
                    progress(step(StructureState::Failed, Some(e.clone())));
                    report.failed.push(("Registration tokens".into(), e));
                }
            }
        }
        if created {
            super::readcache::forget_server(&project.dest_server_id);
            super::trackview::forget_names(&project.dest_server_id);
        }
        super::planview::forget_plan(id);
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        migration::project::DestEcoverse,
        ncentral::rest::testing::{Fake, auth_ok, ok, status},
    };
    use OrgUnitType::{Customer, ServiceOrg, Site};

    fn unit(id: &str, name: &str, kind: OrgUnitType, parent: Option<&str>) -> OrgUnit {
        OrgUnit { org_unit_id: id.into(), org_unit_name: name.into(), org_unit_type: kind, parent_id: parent.map(String::from), external_id: None }
    }

    fn source() -> Vec<OrgUnit> {
        vec![
            unit("1", "Old SO", ServiceOrg, None),
            unit("10", "Acme", Customer, Some("1")),
            unit("11", "Newco", Customer, Some("1")),
            unit("12", "Ghost", Customer, Some("1")),
            unit("20", "HQ", Site, Some("10")),
            unit("21", "Depot", Site, Some("10")),
            unit("22", "Lab", Site, Some("11")),
            unit("23", "Attic", Site, Some("12")),
        ]
    }

    fn dest() -> Vec<OrgUnit> {
        vec![unit("100", "New SO", ServiceOrg, None), unit("110", "Acme", Customer, Some("100")), unit("210", "HQ", Site, Some("110"))]
    }

    #[test]
    fn it_lists_unmatched_customers_then_unmatched_sites_under_a_customer_that_exists_or_is_being_created() {
        let mut overrides = HashMap::new();
        overrides.insert("12".to_string(), Override::Exclude);
        let (customers, sites, skipped) = plan_structure(&source(), &dest(), &overrides, None);
        assert_eq!(customers.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Newco"], "Ghost is left out, Acme exists");
        let by_name: Vec<(&str, &str)> = sites.iter().map(|s| (s.name.as_str(), s.under.as_str())).collect();
        assert_eq!(by_name, [("Depot", "Acme"), ("Lab", "Newco")], "a site goes under the destination customer, or the one being created");
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].starts_with("Attic") && skipped[0].contains("Ghost"));
    }

    #[test]
    fn choosing_a_site_brings_the_customer_it_needs() {
        let only: HashSet<String> = ["22".to_string()].into();
        let (customers, sites, _) = plan_structure(&source(), &dest(), &HashMap::new(), Some(&only));
        assert_eq!((customers.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), sites.len()), (vec!["Newco"], 1));
        let only: HashSet<String> = ["21".to_string()].into();
        let (customers, sites, _) = plan_structure(&source(), &dest(), &HashMap::new(), Some(&only));
        assert_eq!((customers.len(), sites.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()), (0, vec!["Depot"]));
    }

    /// A destination that lists `dest()`, creates what it is asked to with IDs from 900, and refuses one name.
    fn lab(refuse: &'static str) -> std::sync::Arc<Fake> {
        Fake::new(move |req, n| {
            let path = req.url.trim_start_matches("https://").split_once('/').unwrap().1.split('?').next().unwrap().to_string();
            let host = req.url.trim_start_matches("https://").split_once('/').unwrap().0.to_string();
            if path == "api/auth/authenticate" {
                return auth_ok(3600);
            }
            if path == "api/org-units" {
                let units = if host == "src.example.com" { source() } else { dest() };
                let items: Vec<_> = units.iter().map(|u| json!({ "orgUnitId": u.org_unit_id.parse::<u64>().unwrap(), "orgUnitName": u.org_unit_name, "orgUnitType": match u.org_unit_type { ServiceOrg => "SO", Customer => "CUSTOMER", Site => "SITE" }, "parentId": u.parent_id.as_ref().map(|p| p.parse::<u64>().unwrap()) })).collect();
                return ok(json!({ "data": items, "totalPages": 1, "totalItems": items.len(), "page": 1, "pageSize": 1000 }));
            }
            if path == "api/server-info" {
                return ok(json!({ "ncentral": "2026.4.0.25", "version": "1.3.23" }));
            }
            if path.ends_with("/devices") {
                return ok(json!({ "data": [], "totalPages": 1, "totalItems": 0, "page": 1, "pageSize": 1000 }));
            }
            if req.method == crate::ncentral::rest::Method::Post {
                let name = req.body.as_ref().and_then(|b| b.get("customerName").or_else(|| b.get("siteName"))).and_then(|v| v.as_str()).unwrap_or("");
                if name == refuse {
                    return status(400, r#"{"status":400,"message":"BAD REQUEST: OrganizationUnitAlreadyExistException: Site Create - Site exists or Site ID error."}"#);
                }
                if path.ends_with("/customers") {
                    return status(201, &json!({ "customerId": 900 + n }).to_string());
                }
                if path.ends_with("/sites") {
                    return status(201, &json!({ "data": { "siteId": 950 + n } }).to_string());
                }
            }
            if path.ends_with("/software/installers") || path.ends_with("/registration-token") {
                return ok(json!({ "data": { "registrationToken": "abcd-1234-efgh", "registrationTokenExpiryDate": "2999-01-01T00:00:00-05:00" } }));
            }
            status(404, "{}")
        })
    }

    async fn setup(fake: &std::sync::Arc<Fake>) -> (std::sync::Arc<Core>, String) {
        let core = Core::for_tests("structure");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        core.set_rest_transport(fake.clone());
        let p = core.migration_create_full("Struct", &src.id, &dst.id, DestEcoverse::NotLinked, vec![], vec![]).await.unwrap();
        (core, p.id)
    }

    #[tokio::test]
    async fn the_preview_reads_both_servers_and_writes_nothing() {
        let fake = lab("");
        let (core, id) = setup(&fake).await;
        let preview = core.migration_structure_preview(&id, None).await.unwrap();
        assert_eq!((preview.customers.len(), preview.sites.len(), preview.dest_host.as_str()), (2, 3, "dst.example.com"));
        assert_eq!(preview.service_orgs, [ServiceOrgChoice { id: "100".into(), name: "New SO".into() }]);
        assert!(fake.paths().iter().all(|p| !p.starts_with("POST api/service-orgs") && !p.starts_with("POST api/customers")));
    }

    #[tokio::test]
    async fn a_preview_asked_again_reads_the_servers_once_and_reading_again_reads_them_again() {
        let fake = lab("");
        let (core, id) = setup(&fake).await;
        let reads = || fake.paths().iter().filter(|p| p.starts_with("GET api/org-units")).count();
        core.migration_structure_preview(&id, None).await.unwrap();
        core.migration_structure_preview(&id, Some(vec!["22".into()])).await.unwrap();
        assert_eq!(reads(), 2, "one read of each server for two previews");
        core.migration_forget_reads(&id).await.unwrap();
        core.migration_structure_preview(&id, None).await.unwrap();
        assert_eq!(reads(), 4);
    }

    #[tokio::test]
    async fn creating_makes_customers_before_sites_maps_each_to_what_was_created_and_survives_a_conflict() {
        let fake = lab("Depot");
        let (core, id) = setup(&fake).await;
        let report = core.migration_structure_create(&id, "me", "100", None, false).await.unwrap();
        assert_eq!(report.customers_created, ["Ghost", "Newco"]);
        assert_eq!(report.sites_created, ["Attic", "Lab"]);
        assert_eq!(report.already_there, ["Depot"]);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.not_visible.len(), 4, "this destination's list never shows what was created: {:?}", report.not_visible);

        let posts: Vec<String> = fake.paths().into_iter().filter(|p| p.starts_with("POST api/service-orgs") || p.ends_with("/sites")).collect();
        assert!(posts[0].starts_with("POST api/service-orgs/100/customers") && posts[1].starts_with("POST api/service-orgs/100/customers"), "customers first: {posts:?}");
        assert!(posts[2].starts_with("POST api/customers/9") && posts[2].ends_with("/sites"), "a site goes under the customer just created: {posts:?}");

        let saved = core.migration_load(&id).await.unwrap().unwrap();
        let new_id = |source: &str| match saved.mapping_overrides.get(source) {
            Some(Override::MapTo(d)) => d.clone(),
            other => panic!("{source}: {other:?}"),
        };
        assert!(new_id("11").starts_with('9') && new_id("22").starts_with('9'));
        assert!(!saved.mapping_overrides.contains_key("21"), "a conflict isn't guessed at");
    }

    #[tokio::test]
    async fn each_step_is_reported_as_it_starts_and_finishes_including_a_failure_and_the_tokens() {
        let fake = lab("Lab");
        let (core, id) = setup(&fake).await;
        let seen = std::sync::Mutex::new(vec![]);
        let report = core.migration_structure_create_with(&id, "me", "100", None, false, &|p| seen.lock().unwrap().push((p.name, p.kind, p.position, p.total, p.state, p.why))).await.unwrap();
        let seen = seen.into_inner().unwrap();

        let of = |name: &str| seen.iter().filter(|s| s.0 == name).map(|s| s.4).collect::<Vec<_>>();
        assert_eq!(of("Newco"), [StructureState::Running, StructureState::Created]);
        assert_eq!(of("Lab"), [StructureState::Running, StructureState::AlreadyThere]);
        assert_eq!(of("Registration tokens"), [StructureState::Running, StructureState::Created]);
        assert!(seen.iter().filter(|s| s.1 != StructureKind::Tokens).all(|s| s.3 == 5 && (1..=5).contains(&s.2)));
        assert_eq!(report.customers_created.len() + report.sites_created.len(), 4);
    }

    #[tokio::test]
    async fn a_destination_that_cannot_create_units_refuses_the_preview_and_the_creation_once_with_the_reason() {
        let fake = lab("");
        let (core, id) = setup(&fake).await;
        core.rest.set_assume_complete(false);
        let spec_fake = Fake::new(|req, _| {
            let path = req.url.trim_start_matches("https://").split_once('/').unwrap().1.to_string();
            match path.as_str() {
                "api/auth/authenticate" => auth_ok(3600),
                "api-explorer/openapi-spec.json" => ok(json!({ "openapi": "3.0.1", "paths": { "/api/org-units": { "get": {} } } })),
                _ => status(404, "{}"),
            }
        });
        core.set_rest_transport(spec_fake.clone());
        let why = core.migration_structure_preview(&id, None).await.unwrap_err();
        assert!(why.contains("doesn't offer creating customers and sites") && why.contains("Create the customers and sites in N-central"), "{why}");
        let why = core.migration_structure_create(&id, "me", "100", None, false).await.unwrap_err();
        assert!(why.contains("doesn't offer creating customers and sites"), "{why}");
        assert!(spec_fake.paths().iter().all(|p| !p.starts_with("POST api/service-orgs") && !p.contains("/sites")), "{:?}", spec_fake.paths());
    }

    #[tokio::test]
    async fn hundreds_of_customers_with_their_sites_are_created_in_one_pass_each_site_under_its_own_customer() {
        const CUSTOMERS: usize = 150;
        const SITES_EACH: usize = 4;
        let mut units = vec![unit("1", "Old SO", ServiceOrg, None)];
        for c in 0..CUSTOMERS {
            let cid = (1000 + c).to_string();
            units.push(unit(&cid, &format!("Customer {c:03}"), Customer, Some("1")));
            for s in 0..SITES_EACH {
                units.push(unit(&(100_000 + c * 10 + s).to_string(), &format!("Site {c:03}-{s}"), Site, Some(&cid)));
            }
        }
        let fake = Fake::new(move |req, n| {
            let (host, path) = req.url.trim_start_matches("https://").split_once('/').map(|(h, p)| (h.to_string(), p.split('?').next().unwrap().to_string())).unwrap();
            match (req.method, path.as_str()) {
                (_, "api/auth/authenticate") => auth_ok(3600),
                (_, "api/server-info") => ok(json!({ "ncentral": "2026.4.0.25", "version": "1.3.23" })),
                (_, "api/org-units") => {
                    let listed = if host == "src.example.com" { units.clone() } else { vec![unit("100", "New SO", ServiceOrg, None)] };
                    let items: Vec<_> = listed.iter().map(|u| json!({ "orgUnitId": u.org_unit_id.parse::<u64>().unwrap(), "orgUnitName": u.org_unit_name, "orgUnitType": match u.org_unit_type { ServiceOrg => "SO", Customer => "CUSTOMER", Site => "SITE" }, "parentId": u.parent_id.as_ref().map(|p| p.parse::<u64>().unwrap()) })).collect();
                    ok(json!({ "data": items, "totalPages": 1, "totalItems": items.len(), "page": 1, "pageSize": 1000 }))
                }
                (_, p) if p.ends_with("/devices") => ok(json!({ "data": [], "totalPages": 1, "totalItems": 0, "page": 1, "pageSize": 1000 })),
                (crate::ncentral::rest::Method::Post, p) if p.ends_with("/customers") => status(201, &json!({ "customerId": 500_000 + n }).to_string()),
                (crate::ncentral::rest::Method::Post, p) if p.ends_with("/sites") => status(201, &json!({ "siteId": 900_000 + n }).to_string()),
                _ => status(404, "{}"),
            }
        });
        let (core, id) = setup(&fake).await;

        let started = std::time::Instant::now();
        let report = core.migration_structure_create(&id, "me", "100", None, false).await.unwrap();
        assert_eq!((report.customers_created.len(), report.sites_created.len()), (CUSTOMERS, CUSTOMERS * SITES_EACH));
        assert!(report.failed.is_empty(), "{:?}", &report.failed[..report.failed.len().min(3)]);
        let customer_posts = fake.paths().iter().filter(|p| p.starts_with("POST api/service-orgs/100/customers")).count();
        assert_eq!(customer_posts, CUSTOMERS);
        let orphans = fake.paths().iter().filter(|p| p.starts_with("POST api/customers/") && p.ends_with("/sites") && !p.starts_with("POST api/customers/5")).count();
        assert_eq!(orphans, 0, "every site goes under a customer this run created");
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "took {:?}: creation should be linear in the number of units", started.elapsed());
    }

    #[tokio::test]
    async fn a_dry_run_lists_what_would_be_created_and_sends_nothing() {
        let fake = lab("");
        let (core, id) = setup(&fake).await;
        let report = core.migration_structure_create(&id, "me", "100", None, true).await.unwrap();
        assert_eq!(report.would_create.len(), 5, "{:?}", report.would_create);
        assert!(report.customers_created.is_empty() && !core.is_dry_run());
        assert!(fake.paths().iter().all(|p| !p.starts_with("POST api/service-orgs") && !p.contains("/sites")));
        let saved = core.migration_load(&id).await.unwrap().unwrap();
        assert!(saved.mapping_overrides.is_empty());
    }

    #[tokio::test]
    async fn a_service_org_that_is_not_the_destinations_is_refused() {
        let fake = lab("");
        let (core, id) = setup(&fake).await;
        assert!(core.migration_structure_create(&id, "me", "999", None, false).await.unwrap_err().contains("service organizations"));
    }
}
