//! What the Map step shows: one row per source customer or site with where it goes, how that was decided, and
//! (read separately, because it costs a call per unit) how many devices it holds and whether the destination unit
//! has a usable registration token. The matching itself is in `mapping.rs`; this only shapes it for a screen.

use std::collections::{HashMap, HashSet};

use chrono::{Duration, Utc};
use serde::Serialize;

use super::{
    mapping::{Confidence, MappingEntry, MappingStatus, Override, apply_overrides, auto_map, destination_for},
    planner::token_targets,
    project::Project,
};
use crate::{
    Core,
    audit::AuditEntry,
    ncentral::model::{Device, OrgUnit, OrgUnitType, TokenStatus},
};

/// A token that ends within this long is shown as expiring.
const TOKEN_WARNING: Duration = Duration::days(7);

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "UnitKind.ts"))]
#[serde(rename_all = "camelCase")]
pub enum UnitKind {
    Customer,
    Site,
}

/// A customer or site, as a choice for a destination.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "UnitRef.ts"))]
#[serde(rename_all = "camelCase")]
pub struct UnitRef {
    pub id: String,
    pub name: String,
    pub kind: UnitKind,
    /// A site's customer, so two sites with one name can be told apart.
    pub parent_name: Option<String>,
}

/// How a row's destination was decided.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "MatchHow.ts"))]
#[serde(rename_all = "camelCase")]
pub enum MatchHow {
    /// Same name under the unit the parent was mapped to.
    Exact,
    /// Same name, but not under the parent's destination.
    ExactElsewhere,
    /// Names match once case, punctuation and company suffixes are ignored: worth a look.
    Similar,
    /// Chosen by the operator.
    Chosen,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "RowStatus.ts"))]
#[serde(rename_all = "camelCase")]
pub enum RowStatus {
    Mapped,
    /// Several destination units fit equally well; the operator picks.
    Ambiguous,
    Unmatched,
    LeftOut,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "MapRow.ts"))]
#[serde(rename_all = "camelCase")]
pub struct MapRow {
    pub source_id: String,
    pub source_name: String,
    pub kind: UnitKind,
    /// A site's customer on the source.
    pub parent_id: Option<String>,
    pub dest: Option<UnitRef>,
    pub how: Option<MatchHow>,
    pub status: RowStatus,
    /// The destination units that fit equally well when the row is ambiguous.
    pub candidates: Vec<UnitRef>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "MapView.ts"))]
#[serde(rename_all = "camelCase")]
pub struct MapView {
    pub source_label: String,
    pub dest_label: String,
    /// Customers in name order, each followed by its sites.
    pub rows: Vec<MapRow>,
    /// Every destination customer and site, for the picker.
    pub dest_units: Vec<UnitRef>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "TokenState.ts"))]
#[serde(rename_all = "camelCase")]
pub enum TokenState {
    Missing,
    Valid,
    Expiring,
    Expired,
    /// A token is set but its expiry can't be read, so it isn't trusted.
    Unknown,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "TokenRow.ts"))]
#[serde(rename_all = "camelCase")]
pub struct TokenRow {
    pub dest_id: String,
    pub dest_name: String,
    pub state: TokenState,
    /// The day it ends, `YYYY-MM-DD`, when known.
    pub expires: Option<String>,
}

/// The slower part of the Map step.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "MapDetails.ts"))]
#[serde(rename_all = "camelCase")]
pub struct MapDetails {
    /// Devices per source customer or site, as the plan would send them: a site's own, or the customer's that sit at
    /// no mapped site.
    #[cfg_attr(feature = "ts", ts(type = "Record<string, number>"))]
    pub devices: HashMap<String, u32>,
    /// One per destination unit that devices are going to.
    pub tokens: Vec<TokenRow>,
    pub warnings: Vec<String>,
}

fn kind_of(unit: &OrgUnit) -> Option<UnitKind> {
    match unit.org_unit_type {
        OrgUnitType::Customer => Some(UnitKind::Customer),
        OrgUnitType::Site => Some(UnitKind::Site),
        OrgUnitType::ServiceOrg => None,
    }
}

fn unit_ref(unit: &OrgUnit, units: &[OrgUnit]) -> Option<UnitRef> {
    let parent_name = match unit.org_unit_type {
        OrgUnitType::Site => unit.parent_id.as_deref().and_then(|p| units.iter().find(|u| u.org_unit_id == p)).map(|p| p.org_unit_name.clone()),
        _ => None,
    };
    Some(UnitRef { id: unit.org_unit_id.clone(), name: unit.org_unit_name.clone(), kind: kind_of(unit)?, parent_name })
}

fn how(entry: &MappingEntry) -> Option<MatchHow> {
    if entry.manual {
        return Some(MatchHow::Chosen);
    }
    entry.confidence.map(|c| match c {
        Confidence::ExactUnderParent => MatchHow::Exact,
        Confidence::ExactAnywhere => MatchHow::ExactElsewhere,
        Confidence::Normalized => MatchHow::Similar,
    })
}

/// The Map rows for two servers' units and the operator's decisions.
pub fn map_view(source_label: &str, dest_label: &str, source_units: &[OrgUnit], dest_units: &[OrgUnit], overrides: &HashMap<String, Override>) -> MapView {
    let mut mapping = auto_map(source_units, dest_units);
    apply_overrides(&mut mapping, overrides);
    let dest_ref = |id: &str| dest_units.iter().find(|u| u.org_unit_id == id).and_then(|u| unit_ref(u, dest_units));

    let mut rows: Vec<MapRow> = mapping
        .iter()
        .map(|entry| {
            let source = source_units.iter().find(|u| u.org_unit_id == entry.source_id);
            MapRow {
                source_id: entry.source_id.clone(),
                source_name: entry.source_name.clone(),
                kind: if entry.kind == OrgUnitType::Site { UnitKind::Site } else { UnitKind::Customer },
                parent_id: source.and_then(|u| u.parent_id.clone()).filter(|_| entry.kind == OrgUnitType::Site),
                dest: entry.dest_id.as_deref().and_then(dest_ref),
                how: how(entry),
                status: match entry.status() {
                    MappingStatus::Mapped => RowStatus::Mapped,
                    MappingStatus::Ambiguous => RowStatus::Ambiguous,
                    MappingStatus::Unmatched => RowStatus::Unmatched,
                    MappingStatus::Excluded => RowStatus::LeftOut,
                },
                candidates: entry.candidates.iter().filter_map(|c| dest_ref(c)).collect(),
            }
        })
        .collect();

    // Customers in name order, each followed by its sites; a site whose customer isn't listed goes last.
    let customers: HashSet<String> = rows.iter().filter(|r| r.kind == UnitKind::Customer).map(|r| r.source_id.clone()).collect();
    let key = |r: &MapRow| r.source_name.to_lowercase();
    let mut ordered: Vec<MapRow> = Vec::with_capacity(rows.len());
    let mut customer_rows: Vec<MapRow> = rows.iter().filter(|r| r.kind == UnitKind::Customer).cloned().collect();
    customer_rows.sort_by_key(|r| (key(r), r.source_id.clone()));
    for customer in customer_rows {
        let mut sites: Vec<MapRow> = rows.iter().filter(|r| r.kind == UnitKind::Site && r.parent_id.as_deref() == Some(customer.source_id.as_str())).cloned().collect();
        sites.sort_by_key(|r| (key(r), r.source_id.clone()));
        ordered.push(customer);
        ordered.extend(sites);
    }
    rows.retain(|r| r.kind == UnitKind::Site && r.parent_id.as_ref().is_none_or(|p| !customers.contains(p)));
    rows.sort_by_key(|r| (key(r), r.source_id.clone()));
    ordered.extend(rows);

    let mut dests: Vec<UnitRef> = dest_units.iter().filter_map(|u| unit_ref(u, dest_units)).collect();
    dests.sort_by_key(|u| (u.kind == UnitKind::Site, u.name.to_lowercase(), u.parent_name.clone().unwrap_or_default(), u.id.clone()));
    MapView { source_label: source_label.into(), dest_label: dest_label.into(), rows: ordered, dest_units: dests }
}

/// Devices per source unit and a token row per destination unit they go to.
pub fn map_details(devices: &[Device], source_units: &[OrgUnit], dest_units: &[OrgUnit], overrides: &HashMap<String, Override>, statuses: &HashMap<String, TokenStatus>) -> MapDetails {
    let mut mapping = auto_map(source_units, dest_units);
    apply_overrides(&mut mapping, overrides);

    let mut counts: HashMap<String, u32> = HashMap::new();
    for device in devices {
        if let Some(entry) = destination_for(&mapping, device.customer_id.as_deref(), device.site_id.as_deref()) {
            *counts.entry(entry.source_id.clone()).or_default() += 1;
        }
    }

    let name = |id: &str| dest_units.iter().find(|u| u.org_unit_id == id).map_or_else(|| id.to_string(), |u| u.org_unit_name.clone());
    let mut tokens: Vec<TokenRow> = token_targets(devices, &mapping)
        .into_iter()
        .filter_map(|dest_id| {
            let status = statuses.get(&dest_id)?;
            let (state, expires) = match status {
                TokenStatus::Missing => (TokenState::Missing, None),
                TokenStatus::Valid { expires } => (TokenState::Valid, Some(expires.format("%Y-%m-%d").to_string())),
                TokenStatus::Expiring { expires } => (TokenState::Expiring, Some(expires.format("%Y-%m-%d").to_string())),
                TokenStatus::Expired { expires } => (TokenState::Expired, Some(expires.format("%Y-%m-%d").to_string())),
                TokenStatus::UnknownExpiry => (TokenState::Unknown, None),
            };
            Some(TokenRow { dest_name: name(&dest_id), dest_id, state, expires })
        })
        .collect();
    tokens.sort_by(|a, b| a.dest_name.to_lowercase().cmp(&b.dest_name.to_lowercase()).then(a.dest_id.cmp(&b.dest_id)));
    MapDetails { devices: counts, tokens, warnings: vec![] }
}

impl Core {
    async fn migration_units(&self, project: &Project) -> Result<(String, String, Vec<OrgUnit>, Vec<OrgUnit>), String> {
        let (source, dest) = (self.ncentral_server(&project.source_server_id)?, self.ncentral_server(&project.dest_server_id)?);
        let source_units = self.org_units_cached(&source.id).await.map_err(|e| format!("{}: {e}", source.label))?;
        let dest_units = if dest.id == source.id { source_units.clone() } else { self.org_units_cached(&dest.id).await.map_err(|e| format!("{}: {e}", dest.label))? };
        Ok((source.label, dest.label, source_units, dest_units))
    }

    async fn migration_project(&self, id: &str) -> Result<Project, String> {
        self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))
    }

    /// The customers and sites of both servers, matched, with the operator's choices applied. Nothing is written.
    pub async fn migration_map_view(&self, id: &str) -> Result<MapView, String> {
        let project = self.migration_project(id).await?;
        let (source_label, dest_label, source_units, dest_units) = self.migration_units(&project).await?;
        Ok(map_view(&source_label, &dest_label, &source_units, &dest_units, &project.mapping_overrides))
    }

    /// Device counts and registration-token states. Reads every source customer's devices and one token per
    /// destination unit they go to, so it is asked for after the rows are on screen.
    pub async fn migration_map_details(&self, id: &str) -> Result<MapDetails, String> {
        let project = self.migration_project(id).await?;
        let (source_label, dest_label, source_units, dest_units) = self.migration_units(&project).await?;
        let mut mapping = auto_map(&source_units, &dest_units);
        apply_overrides(&mut mapping, &project.mapping_overrides);

        let mut by_id: HashMap<String, Device> = HashMap::new();
        for customer in source_units.iter().filter(|u| u.org_unit_type == OrgUnitType::Customer) {
            for device in self.org_unit_devices_cached(&project.source_server_id, &customer.org_unit_id).await.map_err(|e| format!("{source_label}: {e}"))? {
                by_id.entry(device.device_id.clone()).or_insert(device);
            }
        }
        let devices: Vec<Device> = by_id.into_values().collect();

        let now = Utc::now();
        let (mut statuses, mut warnings) = (HashMap::new(), vec![]);
        for target in token_targets(&devices, &mapping) {
            match self.registration_token_cached(&project.dest_server_id, &target).await {
                Ok(token) => {
                    statuses.insert(target, token.status(now, TOKEN_WARNING));
                }
                Err(e) => {
                    let name = dest_units.iter().find(|u| u.org_unit_id == target).map_or(target.as_str(), |u| u.org_unit_name.as_str());
                    warnings.push(format!("Could not read the registration token for {name} on {dest_label}: {e}"));
                }
            }
        }
        let mut details = map_details(&devices, &source_units, &dest_units, &project.mapping_overrides, &statuses);
        details.warnings = warnings;
        Ok(details)
    }

    /// Chooses a destination for a source unit, or leaves it out. Holds the project's lease while it saves.
    pub async fn migration_map_set(&self, id: &str, instance: &str, source_id: &str, decision: Override) -> Result<(), String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        self.migration_set_mapping(&mut project, source_id, decision)?;
        super::planview::forget_plan(id);
        self.migration_save(&mut project).await
    }

    /// Drops the operator's choice for a source unit, so the automatic match applies again.
    pub async fn migration_map_clear(&self, id: &str, instance: &str, source_id: &str) -> Result<(), String> {
        let mut project = self.migration_open_for_write(id, instance).await?;
        if project.mapping_overrides.remove(source_id).is_some() {
            super::planview::forget_plan(id);
            self.record(&AuditEntry::new("migration.mapping.set", &format!("Went back to the automatic match for source {source_id}"), Ok(()), serde_json::json!({ "project": project.id, "source": source_id })));
            self.migration_save(&mut project).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ncentral::model::OrgUnitType::{Customer, ServiceOrg, Site};

    fn unit(id: &str, name: &str, kind: OrgUnitType, parent: Option<&str>) -> OrgUnit {
        OrgUnit { org_unit_id: id.into(), org_unit_name: name.into(), org_unit_type: kind, parent_id: parent.map(String::from), external_id: None }
    }

    fn source() -> Vec<OrgUnit> {
        vec![
            unit("1", "Old SO", ServiceOrg, None),
            unit("10", "Zeta Corp", Customer, Some("1")),
            unit("11", "Acme, Inc.", Customer, Some("1")),
            unit("20", "Warehouse", Site, Some("10")),
            unit("21", "HQ", Site, Some("11")),
            unit("22", "Annex", Site, Some("11")),
        ]
    }

    fn dest() -> Vec<OrgUnit> {
        vec![
            unit("2", "New SO", ServiceOrg, None),
            unit("110", "Acme", Customer, Some("2")),
            unit("100", "Zeta Corp", Customer, Some("2")),
            unit("210", "HQ", Site, Some("110")),
            unit("200", "Warehouse", Site, Some("100")),
            unit("201", "Warehouse", Site, Some("110")),
        ]
    }

    #[test]
    fn rows_list_customers_by_name_each_followed_by_its_sites_and_say_how_they_matched() {
        let view = map_view("Old", "New", &source(), &dest(), &HashMap::new());
        let order: Vec<&str> = view.rows.iter().map(|r| r.source_name.as_str()).collect();
        assert_eq!(order, ["Acme, Inc.", "Annex", "HQ", "Zeta Corp", "Warehouse"]);
        let acme = &view.rows[0];
        assert_eq!((acme.status, acme.how, acme.dest.as_ref().map(|d| d.name.as_str())), (RowStatus::Mapped, Some(MatchHow::Similar), Some("Acme")));
        let hq = view.rows.iter().find(|r| r.source_id == "21").unwrap();
        assert_eq!((hq.how, hq.parent_id.as_deref()), (Some(MatchHow::Exact), Some("11")));
        let annex = view.rows.iter().find(|r| r.source_id == "22").unwrap();
        assert_eq!((annex.status, annex.dest.is_none()), (RowStatus::Unmatched, true));
    }

    #[test]
    fn the_picker_lists_customers_then_sites_and_names_a_sites_customer() {
        let view = map_view("Old", "New", &source(), &dest(), &HashMap::new());
        assert_eq!(view.dest_units.iter().map(|u| u.kind).collect::<Vec<_>>(), [UnitKind::Customer, UnitKind::Customer, UnitKind::Site, UnitKind::Site, UnitKind::Site]);
        let warehouses: Vec<_> = view.dest_units.iter().filter(|u| u.name == "Warehouse").map(|u| u.parent_name.as_deref()).collect();
        assert_eq!(warehouses, [Some("Acme"), Some("Zeta Corp")], "two sites with one name are told apart by their customer");
        assert!(view.dest_units.iter().all(|u| u.name != "New SO"), "service orgs aren't a choice");
    }

    #[test]
    fn a_choice_or_a_leave_out_shows_as_the_operators_and_the_rest_is_untouched() {
        let mut overrides = HashMap::new();
        overrides.insert("22".to_string(), Override::MapTo("210".into()));
        overrides.insert("20".to_string(), Override::Exclude);
        let view = map_view("Old", "New", &source(), &dest(), &overrides);
        let row = |id: &str| view.rows.iter().find(|r| r.source_id == id).unwrap();
        assert_eq!((row("22").status, row("22").how), (RowStatus::Mapped, Some(MatchHow::Chosen)));
        assert_eq!((row("20").status, row("20").how), (RowStatus::LeftOut, Some(MatchHow::Chosen)));
        assert_eq!(row("21").how, Some(MatchHow::Exact));
    }

    #[test]
    fn devices_are_counted_where_the_plan_would_send_them_and_each_destination_gets_one_token_row() {
        let device = |id: &str, customer: &str, site: Option<&str>| -> Device {
            serde_json::from_value(serde_json::json!({ "deviceId": id, "longName": format!("PC-{id}"), "customerId": customer, "siteId": site })).unwrap()
        };
        let devices = vec![device("1", "11", Some("21")), device("2", "11", Some("21")), device("3", "11", None), device("4", "10", Some("20"))];
        let mut statuses = HashMap::new();
        statuses.insert("210".to_string(), TokenStatus::Missing);
        statuses.insert("110".to_string(), TokenStatus::UnknownExpiry);
        statuses.insert("200".to_string(), TokenStatus::Missing);
        let details = map_details(&devices, &source(), &dest(), &HashMap::new(), &statuses);
        assert_eq!((details.devices.get("21"), details.devices.get("11"), details.devices.get("20")), (Some(&2), Some(&1), Some(&1)));
        assert_eq!(details.tokens.iter().map(|t| (t.dest_name.as_str(), t.state)).collect::<Vec<_>>(), [("Acme", TokenState::Unknown), ("HQ", TokenState::Missing), ("Warehouse", TokenState::Missing)]);
    }

    #[tokio::test]
    async fn choices_are_saved_with_the_lease_and_can_be_taken_back() {
        let core = Core::for_tests("mapset");
        core.set_empty_ecoverse();
        let src = core.save_ncentral_server(None, "Old", "src.example.com", Some("j1".into()), None).await.unwrap();
        let dst = core.save_ncentral_server(None, "New", "dst.example.com", Some("j2".into()), None).await.unwrap();
        let project = core.migration_create_full("Map", &src.id, &dst.id, crate::migration::project::DestEcoverse::NotLinked, vec![], vec![]).await.unwrap();

        core.migration_map_set(&project.id, "me", "22", Override::MapTo("210".into())).await.unwrap();
        assert_eq!(core.migration_load(&project.id).await.unwrap().unwrap().mapping_overrides.get("22"), Some(&Override::MapTo("210".into())));
        assert!(core.migration_map_set(&project.id, "someone-else", "22", Override::Exclude).await.unwrap_err().contains("Another N-voke window"));
        assert!(core.migration_map_set(&project.id, "me", "not-a-number", Override::Exclude).await.is_err());

        core.migration_map_clear(&project.id, "me", "22").await.unwrap();
        assert!(core.migration_load(&project.id).await.unwrap().unwrap().mapping_overrides.is_empty());
        core.migration_map_clear(&project.id, "me", "22").await.unwrap();
    }
}
