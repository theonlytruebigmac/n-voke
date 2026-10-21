//! Matching source customers and sites to destination ones. Automatic matching
//! never guesses: a name that fits more than one destination unit is reported as
//! ambiguous, for the operator to settle. Names are compared exactly under a
//! matching parent first, then exactly anywhere, then normalized (case,
//! punctuation, "Inc"/"LLC"). `externalId` is only a tie-breaker when both sides
//! have the same value; it is often blank and never required.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ncentral::model::{OrgUnit, OrgUnitType};

/// How a match was made, strongest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    /// Same name under the unit the parent was mapped to.
    ExactUnderParent,
    ExactAnywhere,
    Normalized,
}

/// What the operator decided for a source unit, overriding the automatic match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "Override.ts"))]
#[serde(rename_all = "camelCase", tag = "kind", content = "destId")]
pub enum Override {
    /// Map to this destination unit.
    MapTo(String),
    /// Leave this unit out of the migration.
    Exclude,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingEntry {
    pub source_id: String,
    pub source_name: String,
    pub kind: OrgUnitType,
    /// The destination unit, when matched or chosen.
    pub dest_id: Option<String>,
    /// How the automatic match was made; None for a manual choice or no match.
    pub confidence: Option<Confidence>,
    /// Destination units that fit equally well when the match is ambiguous.
    pub candidates: Vec<String>,
    pub excluded: bool,
    pub manual: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingStatus {
    Mapped,
    Ambiguous,
    Unmatched,
    Excluded,
}

impl MappingEntry {
    pub fn status(&self) -> MappingStatus {
        match (self.excluded, &self.dest_id, self.candidates.len()) {
            (true, ..) => MappingStatus::Excluded,
            (_, Some(_), _) => MappingStatus::Mapped,
            (_, None, n) if n > 1 => MappingStatus::Ambiguous,
            _ => MappingStatus::Unmatched,
        }
    }
}

/// Lowercase, `&` as "and", punctuation dropped, a trailing company suffix removed:
/// "Acme, Inc." and "ACME inc" both become "acme".
pub fn normalize_name(name: &str) -> String {
    const SUFFIXES: [&str; 14] = ["inc", "incorporated", "llc", "ltd", "limited", "corp", "corporation", "co", "company", "gmbh", "pty", "plc", "lp", "llp"];
    let spaced: String = name.to_lowercase().replace('&', " and ").chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    let mut words: Vec<&str> = spaced.split_whitespace().collect();
    while words.len() > 1 && words.last().is_some_and(|w| SUFFIXES.contains(w)) {
        words.pop();
    }
    words.join(" ")
}

struct Tree<'a> {
    units: &'a [OrgUnit],
    by_id: HashMap<&'a str, &'a OrgUnit>,
}

impl<'a> Tree<'a> {
    fn new(units: &'a [OrgUnit]) -> Self {
        Self { units, by_id: units.iter().map(|u| (u.org_unit_id.as_str(), u)).collect() }
    }

    fn parent(&self, unit: &OrgUnit) -> Option<&'a OrgUnit> {
        unit.parent_id.as_deref().and_then(|p| self.by_id.get(p).copied())
    }

    fn service_orgs(&self) -> Vec<&'a OrgUnit> {
        self.units.iter().filter(|u| u.org_unit_type == OrgUnitType::ServiceOrg).collect()
    }
}

/// Automatic matches for every customer and site on the source, customers first
/// so a site can be matched under its customer's destination.
pub fn auto_map(source: &[OrgUnit], destination: &[OrgUnit]) -> Vec<MappingEntry> {
    let (src, dst) = (Tree::new(source), Tree::new(destination));
    let single_so_each = src.service_orgs().len() == 1 && dst.service_orgs().len() == 1;

    let mut ordered: Vec<&OrgUnit> = source.iter().filter(|u| u.org_unit_type != OrgUnitType::ServiceOrg).collect();
    ordered.sort_by_key(|u| (u.org_unit_type == OrgUnitType::Site, u.org_unit_name.to_lowercase(), u.org_unit_id.clone()));

    let mut mapped: HashMap<String, String> = HashMap::new();
    let mut entries = Vec::with_capacity(ordered.len());
    for unit in ordered {
        let same_kind: Vec<&OrgUnit> = destination.iter().filter(|d| d.org_unit_type == unit.org_unit_type).collect();
        // The destination unit that the source unit's parent maps to (or, for a customer, the matching service org).
        let parent_dest: Option<String> = match src.parent(unit) {
            Some(p) if p.org_unit_type == OrgUnitType::ServiceOrg => {
                if single_so_each {
                    dst.service_orgs().first().map(|s| s.org_unit_id.clone())
                } else {
                    dst.service_orgs().into_iter().find(|s| normalize_name(&s.org_unit_name) == normalize_name(&p.org_unit_name)).map(|s| s.org_unit_id.clone())
                }
            }
            Some(p) => mapped.get(&p.org_unit_id).cloned(),
            None => None,
        };

        let tiers: [(Confidence, Vec<&OrgUnit>); 3] = [
            (
                Confidence::ExactUnderParent,
                same_kind.iter().copied().filter(|d| d.org_unit_name == unit.org_unit_name && parent_dest.is_some() && d.parent_id == parent_dest).collect(),
            ),
            (Confidence::ExactAnywhere, same_kind.iter().copied().filter(|d| d.org_unit_name == unit.org_unit_name).collect()),
            (
                Confidence::Normalized,
                same_kind.iter().copied().filter(|d| normalize_name(&d.org_unit_name) == normalize_name(&unit.org_unit_name)).collect(),
            ),
        ];

        let mut entry = MappingEntry {
            source_id: unit.org_unit_id.clone(),
            source_name: unit.org_unit_name.clone(),
            kind: unit.org_unit_type,
            dest_id: None,
            confidence: None,
            candidates: vec![],
            excluded: false,
            manual: false,
        };
        for (confidence, candidates) in tiers {
            let chosen = match candidates.as_slice() {
                [] => continue,
                [only] => Some(*only),
                several => {
                    let confirmed: Vec<&OrgUnit> = several.iter().copied().filter(|d| unit.external_id.is_some() && d.external_id == unit.external_id).collect();
                    match confirmed.as_slice() {
                        [only] => Some(*only),
                        _ => {
                            entry.candidates = several.iter().map(|d| d.org_unit_id.clone()).collect();
                            None
                        }
                    }
                }
            };
            if let Some(found) = chosen {
                entry.dest_id = Some(found.org_unit_id.clone());
                entry.confidence = Some(confidence);
                mapped.insert(unit.org_unit_id.clone(), found.org_unit_id.clone());
            }
            break;
        }
        entries.push(entry);
    }
    entries
}

/// Applies the operator's decisions over the automatic mapping. A choice for a
/// unit that isn't in the list is ignored (the unit may have been deleted).
pub fn apply_overrides(entries: &mut [MappingEntry], overrides: &HashMap<String, Override>) {
    for entry in entries {
        match overrides.get(&entry.source_id) {
            Some(Override::MapTo(dest)) => {
                entry.dest_id = Some(dest.clone());
                entry.confidence = None;
                entry.candidates.clear();
                entry.excluded = false;
                entry.manual = true;
            }
            Some(Override::Exclude) => {
                entry.excluded = true;
                entry.manual = true;
            }
            None => {}
        }
    }
}

/// Where a source device goes: its site's destination when the site is mapped,
/// otherwise its customer's. An excluded site excludes its devices. None when
/// the device's units aren't in the mapping at all.
pub fn destination_for<'a>(entries: &'a [MappingEntry], customer_id: Option<&str>, site_id: Option<&str>) -> Option<&'a MappingEntry> {
    let find = |id: Option<&str>| id.and_then(|id| entries.iter().find(|e| e.source_id == id));
    let (site, customer) = (find(site_id), find(customer_id));
    match site {
        Some(s) if matches!(s.status(), MappingStatus::Mapped | MappingStatus::Excluded) => Some(s),
        _ => customer.or(site),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: &str, name: &str, kind: OrgUnitType, parent: Option<&str>) -> OrgUnit {
        OrgUnit { org_unit_id: id.into(), org_unit_name: name.into(), org_unit_type: kind, parent_id: parent.map(String::from), external_id: None }
    }
    use OrgUnitType::{Customer, ServiceOrg, Site};

    fn entry<'a>(entries: &'a [MappingEntry], source_id: &str) -> &'a MappingEntry {
        entries.iter().find(|e| e.source_id == source_id).unwrap()
    }

    #[test]
    fn names_are_normalized_for_case_punctuation_and_company_suffixes() {
        assert_eq!(normalize_name("Acme, Inc."), "acme");
        assert_eq!(normalize_name("ACME inc"), "acme");
        assert_eq!(normalize_name("Smith & Sons LLC"), "smith and sons");
        assert_eq!(normalize_name("Smith and Sons"), "smith and sons");
        assert_eq!(normalize_name("Inc"), "inc", "a name that is only a suffix is kept");
        assert_eq!(normalize_name("O'Brien-Jones Ltd."), "o brien jones");
    }

    #[test]
    fn same_names_match_exactly_and_sites_follow_their_customer() {
        let source = vec![unit("1", "SO", ServiceOrg, None), unit("10", "Acme", Customer, Some("1")), unit("11", "HQ", Site, Some("10")), unit("20", "Globex", Customer, Some("1")), unit("21", "HQ", Site, Some("20"))];
        let dest = vec![unit("100", "SO", ServiceOrg, None), unit("110", "Acme", Customer, Some("100")), unit("111", "HQ", Site, Some("110")), unit("120", "Globex", Customer, Some("100")), unit("121", "HQ", Site, Some("120"))];
        let map = auto_map(&source, &dest);
        assert_eq!(map.len(), 4, "the service org isn't mapped");
        assert_eq!(entry(&map, "10").dest_id.as_deref(), Some("110"));
        assert_eq!(entry(&map, "10").confidence, Some(Confidence::ExactUnderParent));
        // Two sites named HQ: each goes to the HQ under its own customer's destination.
        assert_eq!(entry(&map, "11").dest_id.as_deref(), Some("111"));
        assert_eq!(entry(&map, "21").dest_id.as_deref(), Some("121"));
        assert!(map.iter().all(|e| e.status() == MappingStatus::Mapped));
    }

    #[test]
    fn an_exact_name_elsewhere_and_a_normalized_name_are_weaker_matches() {
        let source = vec![unit("1", "Old SO", ServiceOrg, None), unit("10", "Acme", Customer, Some("1")), unit("20", "Globex, Inc.", Customer, Some("1"))];
        let dest = vec![unit("100", "New SO", ServiceOrg, None), unit("101", "Other SO", ServiceOrg, None), unit("110", "Acme", Customer, Some("101")), unit("120", "Globex", Customer, Some("100"))];
        let map = auto_map(&source, &dest);
        assert_eq!((entry(&map, "10").dest_id.as_deref(), entry(&map, "10").confidence), (Some("110"), Some(Confidence::ExactAnywhere)));
        assert_eq!((entry(&map, "20").dest_id.as_deref(), entry(&map, "20").confidence), (Some("120"), Some(Confidence::Normalized)));
        assert!(Confidence::ExactUnderParent < Confidence::Normalized);
    }

    #[test]
    fn a_name_that_fits_two_destinations_is_ambiguous_never_guessed() {
        let source = vec![unit("1", "SO", ServiceOrg, None), unit("10", "Acme", Customer, Some("1"))];
        let dest = vec![unit("100", "Alpha", ServiceOrg, None), unit("101", "Beta", ServiceOrg, None), unit("110", "Acme", Customer, Some("100")), unit("111", "Acme", Customer, Some("101"))];
        // Neither destination service org has the source's name, so no Acme is "under the parent".
        let map = auto_map(&source, &dest);
        let acme = entry(&map, "10");
        assert_eq!(acme.status(), MappingStatus::Ambiguous);
        assert_eq!(acme.dest_id, None);
        assert_eq!(acme.candidates, ["110", "111"]);
    }

    #[test]
    fn a_matching_external_id_settles_an_ambiguity() {
        let mut source_unit = unit("10", "Acme", Customer, Some("1"));
        source_unit.external_id = Some("CRM-7".into());
        let source = vec![unit("1", "SO", ServiceOrg, None), source_unit];
        let mut other = unit("111", "Acme", Customer, Some("101"));
        other.external_id = Some("CRM-7".into());
        let dest = vec![unit("100", "Alpha", ServiceOrg, None), unit("101", "Beta", ServiceOrg, None), unit("110", "Acme", Customer, Some("100")), other];
        assert_eq!(entry(&auto_map(&source, &dest), "10").dest_id.as_deref(), Some("111"));
    }

    #[test]
    fn units_with_no_counterpart_stay_unmatched_and_kinds_never_cross() {
        let source = vec![unit("1", "SO", ServiceOrg, None), unit("10", "Acme", Customer, Some("1")), unit("11", "Acme", Site, Some("10"))];
        let dest = vec![unit("100", "SO", ServiceOrg, None), unit("110", "Acme", Site, Some("100"))];
        let map = auto_map(&source, &dest);
        assert_eq!(entry(&map, "10").status(), MappingStatus::Unmatched, "a customer is never matched to a site");
        assert_eq!(entry(&map, "11").dest_id.as_deref(), Some("110"));
    }

    #[test]
    fn many_source_sites_can_share_one_destination() {
        let source = vec![unit("1", "SO", ServiceOrg, None), unit("10", "Acme", Customer, Some("1")), unit("11", "HQ", Site, Some("10")), unit("12", "Head Office", Site, Some("10"))];
        let dest = vec![unit("100", "SO", ServiceOrg, None), unit("110", "Acme", Customer, Some("100")), unit("111", "HQ", Site, Some("110"))];
        let mut map = auto_map(&source, &dest);
        let overrides = HashMap::from([("12".to_string(), Override::MapTo("111".into()))]);
        apply_overrides(&mut map, &overrides);
        assert_eq!(entry(&map, "11").dest_id, entry(&map, "12").dest_id);
        assert!(entry(&map, "12").manual && !entry(&map, "11").manual);
    }

    #[test]
    fn overrides_exclude_or_replace_and_unknown_units_are_ignored() {
        let source = vec![unit("1", "SO", ServiceOrg, None), unit("10", "Acme", Customer, Some("1")), unit("20", "Globex", Customer, Some("1"))];
        let dest = vec![unit("100", "SO", ServiceOrg, None), unit("110", "Acme", Customer, Some("100"))];
        let mut map = auto_map(&source, &dest);
        let overrides = HashMap::from([("10".to_string(), Override::Exclude), ("20".to_string(), Override::MapTo("110".into())), ("999".to_string(), Override::Exclude)]);
        apply_overrides(&mut map, &overrides);
        assert_eq!(entry(&map, "10").status(), MappingStatus::Excluded);
        assert_eq!((entry(&map, "20").status(), entry(&map, "20").dest_id.as_deref()), (MappingStatus::Mapped, Some("110")));
    }

    #[test]
    fn a_device_goes_to_its_sites_destination_else_its_customers() {
        let source = vec![unit("1", "SO", ServiceOrg, None), unit("10", "Acme", Customer, Some("1")), unit("11", "HQ", Site, Some("10")), unit("12", "Branch", Site, Some("10"))];
        let dest = vec![unit("100", "SO", ServiceOrg, None), unit("110", "Acme", Customer, Some("100")), unit("111", "HQ", Site, Some("110"))];
        let map = auto_map(&source, &dest);
        let target = |site: Option<&str>| destination_for(&map, Some("10"), site).and_then(|e| e.dest_id.clone());
        assert_eq!(target(Some("11")).as_deref(), Some("111"));
        assert_eq!(target(Some("12")).as_deref(), Some("110"), "an unmapped site falls back to its customer");
        assert_eq!(target(None).as_deref(), Some("110"));
        assert!(destination_for(&map, Some("404"), None).is_none());
        let mut excluded = map.clone();
        apply_overrides(&mut excluded, &HashMap::from([("11".to_string(), Override::Exclude)]));
        assert_eq!(destination_for(&excluded, Some("10"), Some("11")).map(|e| e.status()), Some(MappingStatus::Excluded));
    }
}
