//! The Connect step: is everything the project depends on reachable and signed in? A short checklist, each
//! item green, amber or red with a sentence saying what is wrong and what to do (docs/migration/11-gui-design.md,
//! screen 1). Nothing here changes anything.
//!
//! The decisions are in `connect_items`, which takes what was read and builds the list, so they are tested
//! without a network. `Core::migration_connect_check` reads the servers and Ecoverse and calls it.

use serde::Serialize;

use super::{
    ecoverse::is_linked,
    project::{DestEcoverse, Delivery},
    survey::TESTED_RELEASE,
};
use crate::{
    Core,
    ncentral::{
        EcoCredential,
        capabilities::{Capabilities, Capability, Role},
        model::ServerInfo,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ConnectLevel.ts"))]
#[serde(rename_all = "camelCase")]
pub enum ConnectLevel {
    Ok,
    /// Worth knowing; doesn't stop the project.
    Warn,
    /// Stops the project until fixed.
    Err,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ConnectItem.ts"))]
#[serde(rename_all = "camelCase")]
pub struct ConnectItem {
    /// Stable, so the page can keep an item's place between checks: `source`, `destination`, `server`, `version`,
    /// `ecoverse`, `destination-ecoverse`.
    pub id: String,
    pub level: ConnectLevel,
    pub title: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ConnectReport.ts"))]
#[serde(rename_all = "camelCase")]
pub struct ConnectReport {
    pub items: Vec<ConnectItem>,
    /// No red item: the project can go on to Map.
    pub can_continue: bool,
}

/// What reading one server gave.
pub struct ServerCheck {
    pub label: String,
    pub host: String,
    pub info: Result<ServerInfo, String>,
    /// What the server's REST API offers, from its own spec.
    pub capabilities: Capabilities,
}

/// What asking Ecoverse about the source server gave.
pub enum EcoCheck {
    /// The project's delivery is REST only.
    RestOnly,
    /// The source server isn't used through Ecoverse.
    ServerNotUsed,
    /// Ecoverse sees this many devices of the source server.
    Linked(usize),
    /// Ecoverse answered but shows no device of the source server.
    NotLinked,
    Failed(String),
}

/// How the destination's Ecoverse is read, when the project says it is linked to another tenant.
pub struct DestKey {
    pub label: String,
    /// A token of its own is saved for the destination server.
    pub own_token: bool,
    /// Why that token was refused when it was tried, if it was.
    pub token_error: Option<String>,
}

fn item(id: &str, level: ConnectLevel, title: impl Into<String>, detail: impl Into<String>) -> ConnectItem {
    ConnectItem { id: id.into(), level, title: title.into(), detail: detail.into() }
}

fn describe(info: &ServerInfo) -> String {
    format!("signed in · N-central {} · API {}", info.ncentral, info.version)
}

fn server_item(id: &str, role: &str, check: &ServerCheck) -> ConnectItem {
    let title = format!("{role} {}", check.label);
    match &check.info {
        Ok(info) => item(id, ConnectLevel::Ok, title, describe(info)),
        Err(e) => item(id, ConnectLevel::Err, title, format!("{e} Check the server's JWT under Manage servers, then check again.")),
    }
}

/// One item for what each server's REST API offers: red when something a project can't go on without is missing,
/// amber for what only limits it (with the workaround) or can't be read, green otherwise.
fn capabilities_item(source: &ServerCheck, dest: &ServerCheck, same_server: bool, rest_only: bool) -> ConnectItem {
    let checks: Vec<(&ServerCheck, Vec<Role>)> = if same_server { vec![(source, vec![Role::Source, Role::Destination])] } else { vec![(source, vec![Role::Source]), (dest, vec![Role::Destination])] };
    let (mut required, mut optional, mut unreadable) = (vec![], vec![], vec![]);
    for (check, roles) in checks.into_iter().filter(|(c, _)| c.info.is_ok()) {
        if let Capabilities::Unknown(why) = &check.capabilities {
            unreadable.push(format!("{}: {why}", check.label));
            continue;
        }
        let mut seen: Vec<(Capability, bool)> = vec![];
        for (capability, needed) in roles.iter().flat_map(|&r| Capability::needs(r, rest_only)) {
            match seen.iter_mut().find(|(c, _)| *c == capability) {
                Some(entry) => entry.1 |= needed,
                None => seen.push((capability, needed)),
            }
        }
        for (capability, needed) in seen {
            if let Some(missing) = check.capabilities.missing(capability) {
                let line = format!("{} can't do {}: its API spec lacks {}. {}", check.label, capability.name(), missing.join(", "), capability.workaround());
                if needed { required.push(line) } else { optional.push(line) }
            }
        }
    }
    let title = "REST API features";
    let mut detail: Vec<String> = required.iter().chain(&optional).cloned().collect();
    if !unreadable.is_empty() {
        detail.push(format!("{}. N-voke can't confirm what the server offers, so it tries and reports what it finds.", unreadable.join("; ")));
    }
    if !required.is_empty() {
        item("capabilities", ConnectLevel::Err, title, detail.join(" "))
    } else if !detail.is_empty() {
        item("capabilities", ConnectLevel::Warn, title, detail.join(" "))
    } else {
        item("capabilities", ConnectLevel::Ok, title, "every endpoint N-voke uses is offered")
    }
}

/// Builds the checklist from what was read.
pub fn connect_items(source: &ServerCheck, dest: &ServerCheck, same_server: bool, eco: &EcoCheck, dest_ecoverse: &DestEcoverse, dest_key: &DestKey) -> ConnectReport {
    let mut items = vec![];
    if same_server {
        items.push(server_item("server", "Server", source));
    } else {
        items.push(server_item("source", "Source", source));
        items.push(server_item("destination", "Destination", dest));
    }

    let old: Vec<String> = [source, dest]
        .into_iter()
        .filter(|c| c.info.as_ref().is_ok_and(|i| i.release().is_none_or(|r| r < TESTED_RELEASE)))
        .map(|c| format!("{} runs N-central {}", c.label, c.info.as_ref().map(|i| i.ncentral.as_str()).unwrap_or("?")))
        .fold(vec![], |mut acc, s| {
            if !acc.contains(&s) {
                acc.push(s);
            }
            acc
        });
    if !old.is_empty() {
        items.push(item(
            "version",
            ConnectLevel::Warn,
            "Older than the tested releases",
            format!("{}. Migration was tested on N-central {}.{} and later; older versions may behave differently.", old.join("; "), TESTED_RELEASE.0, TESTED_RELEASE.1),
        ));
    }

    items.push(capabilities_item(source, dest, same_server, matches!(eco, EcoCheck::RestOnly)));

    items.push(match eco {
        EcoCheck::RestOnly => item("ecoverse", ConnectLevel::Ok, "Ecoverse", "Not used: this project's delivery is REST only, so every device is reached through N-central."),
        EcoCheck::ServerNotUsed => item(
            "ecoverse",
            ConnectLevel::Ok,
            "Ecoverse",
            format!("Not used for {}: its devices are reached through N-central. Choose a key for it under Manage servers to use Ecoverse.", source.label),
        ),
        EcoCheck::Linked(n) => item("ecoverse", ConnectLevel::Ok, "Ecoverse", format!("sees {n} {} on {}", if *n == 1 { "device" } else { "devices" }, source.host)),
        EcoCheck::NotLinked => item(
            "ecoverse",
            ConnectLevel::Warn,
            "Ecoverse",
            format!("shows no device of {} in the chosen organizations, so every device is planned on the REST path.", source.host),
        ),
        EcoCheck::Failed(e) => item("ecoverse", ConnectLevel::Warn, "Ecoverse", format!("couldn't be read: {e} Devices are planned on the REST path until it can.")),
    });

    items.push(match dest_ecoverse {
        DestEcoverse::NotLinked => item(
            "destination-ecoverse",
            ConnectLevel::Ok,
            "Destination and Ecoverse",
            "Not linked yet: devices leave Ecoverse after the move, and progress after it is tracked over REST. Ecoverse agents are deployed on the destination once cloud eventing is on and SSO is enabled there.",
        ),
        DestEcoverse::SameTenant => item("destination-ecoverse", ConnectLevel::Ok, "Destination and Ecoverse", "Linked to the same Ecoverse tenant as the source: devices show up there again after the move."),
        DestEcoverse::OtherTenant { .. } if dest_key.token_error.is_some() => item(
            "destination-ecoverse",
            ConnectLevel::Warn,
            "Destination and Ecoverse",
            format!("Linked to another Ecoverse tenant, but {}'s own token didn't work: {} Replace it under Manage servers.", dest_key.label, dest_key.token_error.as_deref().unwrap_or_default()),
        ),
        DestEcoverse::OtherTenant { .. } if dest_key.own_token => item(
            "destination-ecoverse",
            ConnectLevel::Ok,
            "Destination and Ecoverse",
            format!("Linked to another Ecoverse tenant, read with {}'s own token: devices show up there after the move.", dest_key.label),
        ),
        DestEcoverse::OtherTenant { .. } => item(
            "destination-ecoverse",
            ConnectLevel::Warn,
            "Destination and Ecoverse",
            format!("Linked to another Ecoverse tenant, but {} has no Ecoverse token of its own, so the app's key would be used. That only works if the key can see that tenant. Add the token under Manage servers.", dest_key.label),
        ),
    });

    let can_continue = items.iter().all(|i| i.level != ConnectLevel::Err);
    ConnectReport { items, can_continue }
}

impl Core {
    /// Signs in and reads the version, then the API spec: read again every time, so a server upgraded since is seen.
    async fn read_server(&self, server: &crate::ncentral::ServerConfig) -> ServerCheck {
        let info = self.ncentral_server_info(&server.id).await.map_err(String::from);
        let capabilities = if info.is_ok() { self.ncentral_capabilities(&server.id, true).await } else { Capabilities::Unknown(String::new()) };
        ServerCheck { label: server.label.clone(), host: server.host.clone(), info, capabilities }
    }

    /// Reads both servers and Ecoverse for a saved project and builds the checklist.
    pub async fn migration_connect_check(&self, project_id: &str) -> Result<ConnectReport, String> {
        let project = self.migration_load(project_id).await?.ok_or("That project no longer exists.")?;
        let source = self.ncentral_server(&project.source_server_id)?;
        let dest = self.ncentral_server(&project.dest_server_id)?;
        let same = source.id == dest.id;

        let source_check = self.read_server(&source).await;
        let dest_check = if same { ServerCheck { label: source_check.label.clone(), host: source_check.host.clone(), info: source_check.info.clone(), capabilities: source_check.capabilities.clone() } } else { self.read_server(&dest).await };

        let eco = if project.options.delivery == Delivery::RestOnly {
            EcoCheck::RestOnly
        } else if !self.server_uses_ecoverse(&source.id) {
            EcoCheck::ServerNotUsed
        } else {
            match self.with_ecoverse(&source.id, self.eco_devices(&project.source_ecoverse_org_ids)).await {
                Ok(devices) if is_linked(&devices, &source.host) => EcoCheck::Linked(devices.iter().filter(|d| d.server_host().as_deref() == Some(source.host.as_str())).count()),
                Ok(_) => EcoCheck::NotLinked,
                Err(e) => EcoCheck::Failed(e),
            }
        };

        let dest_view = self.ncentral_servers().await?.into_iter().find(|s| s.id == dest.id);
        let own_token = dest_view.is_some_and(|s| s.ecoverse == EcoCredential::Own && s.has_ecoverse_token);
        let token_error = if own_token && matches!(project.dest_ecoverse, DestEcoverse::OtherTenant { .. }) { self.with_ecoverse(&dest.id, self.count_organizations()).await.err() } else { None };
        let dest_key = DestKey { label: dest.label.clone(), own_token, token_error };
        Ok(connect_items(&source_check, &dest_check, same, &eco, &project.dest_ecoverse, &dest_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(label: &str, host: &str, release: &str) -> ServerCheck {
        ServerCheck { label: label.into(), host: host.into(), info: Ok(ServerInfo { ncentral: release.into(), version: "1.3.23".into() }), capabilities: Capabilities::complete() }
    }
    fn bad(label: &str, host: &str, why: &str) -> ServerCheck {
        ServerCheck { label: label.into(), host: host.into(), info: Err(why.into()), capabilities: Capabilities::Unknown(String::new()) }
    }
    fn key() -> DestKey {
        DestKey { label: "New".into(), own_token: false, token_error: None }
    }
    fn ids(r: &ConnectReport) -> Vec<&str> {
        r.items.iter().map(|i| i.id.as_str()).collect()
    }
    fn level<'a>(r: &'a ConnectReport, id: &str) -> &'a ConnectLevel {
        &r.items.iter().find(|i| i.id == id).unwrap().level
    }

    #[test]
    fn two_healthy_servers_and_a_linked_ecoverse_can_continue() {
        let r = connect_items(&ok("Old", "old.example.com", "2026.4.0.25"), &ok("New", "new.example.com", "2026.4.1.11"), false, &EcoCheck::Linked(412), &DestEcoverse::NotLinked, &key());
        assert!(r.can_continue);
        assert_eq!(ids(&r), ["source", "destination", "capabilities", "ecoverse", "destination-ecoverse"]);
        assert!(r.items.iter().all(|i| i.level == ConnectLevel::Ok));
        assert!(r.items[0].detail.contains("N-central 2026.4.0.25") && r.items[0].detail.contains("API 1.3.23"));
        assert!(r.items[3].detail.contains("412 devices") && r.items[3].detail.contains("old.example.com"));
    }

    #[test]
    fn a_server_that_cannot_be_read_stops_the_project_and_says_what_to_do() {
        let r = connect_items(&ok("Old", "old.example.com", "2026.4.0.25"), &bad("New", "new.example.com", "401 Unauthorized."), false, &EcoCheck::RestOnly, &DestEcoverse::NotLinked, &key());
        assert!(!r.can_continue);
        assert_eq!(level(&r, "destination"), &ConnectLevel::Err);
        let detail = &r.items.iter().find(|i| i.id == "destination").unwrap().detail;
        assert!(detail.contains("401 Unauthorized.") && detail.contains("JWT"), "{detail}");
        assert_eq!(level(&r, "source"), &ConnectLevel::Ok);
    }

    #[test]
    fn the_same_server_is_one_item_and_is_read_once() {
        let same = ok("Lab", "lab.example.com", "2026.4.0.25");
        let r = connect_items(&same, &ok("Lab", "lab.example.com", "2026.4.0.25"), true, &EcoCheck::Linked(3), &DestEcoverse::SameTenant, &key());
        assert_eq!(ids(&r), ["server", "capabilities", "ecoverse", "destination-ecoverse"]);
        assert_eq!(r.items[0].title, "Server Lab");
        assert_eq!(r.items[3].detail, "Linked to the same Ecoverse tenant as the source: devices show up there again after the move.");
    }

    #[test]
    fn an_older_release_is_a_warning_naming_each_old_server_once() {
        let r = connect_items(&ok("Old", "old.example.com", "2026.3.1.2"), &ok("New", "new.example.com", "2025.1.0.1"), false, &EcoCheck::Linked(1), &DestEcoverse::NotLinked, &key());
        assert!(r.can_continue, "a warning doesn't stop the project");
        assert_eq!(level(&r, "version"), &ConnectLevel::Warn);
        let detail = &r.items.iter().find(|i| i.id == "version").unwrap().detail;
        assert!(detail.contains("Old runs N-central 2026.3.1.2") && detail.contains("New runs N-central 2025.1.0.1") && detail.contains("2026.4"), "{detail}");

        let same = connect_items(&ok("Lab", "lab.example.com", "2026.3.1.2"), &ok("Lab", "lab.example.com", "2026.3.1.2"), true, &EcoCheck::RestOnly, &DestEcoverse::NotLinked, &key());
        assert_eq!(same.items.iter().find(|i| i.id == "version").unwrap().detail.matches("Lab runs").count(), 1);
    }

    #[test]
    fn a_version_nobody_can_parse_is_treated_as_untested() {
        let r = connect_items(&ok("Old", "old.example.com", "weird"), &ok("New", "new.example.com", "2026.4.0.1"), false, &EcoCheck::Linked(1), &DestEcoverse::NotLinked, &key());
        assert_eq!(level(&r, "version"), &ConnectLevel::Warn);
    }

    #[test]
    fn ecoverse_problems_are_warnings_that_fall_back_to_the_rest_path() {
        let servers = (ok("Old", "old.example.com", "2026.4.0.25"), ok("New", "new.example.com", "2026.4.1.11"));
        for eco in [EcoCheck::NotLinked, EcoCheck::Failed("timed out.".into())] {
            let r = connect_items(&servers.0, &servers.1, false, &eco, &DestEcoverse::NotLinked, &key());
            assert!(r.can_continue);
            assert_eq!(level(&r, "ecoverse"), &ConnectLevel::Warn);
            assert!(r.items.iter().find(|i| i.id == "ecoverse").unwrap().detail.contains("REST"), "{eco_name}", eco_name = r.items[3].detail);
        }
    }

    #[test]
    fn an_unlinked_destination_is_explained_not_flagged() {
        let r = connect_items(&ok("Old", "old.example.com", "2026.4.0.25"), &ok("New", "new.example.com", "2026.4.1.11"), false, &EcoCheck::Linked(1), &DestEcoverse::NotLinked, &key());
        let i = r.items.iter().find(|i| i.id == "destination-ecoverse").unwrap();
        assert_eq!(i.level, ConnectLevel::Ok);
        assert!(i.detail.contains("cloud eventing") && i.detail.contains("SSO") && i.detail.contains("REST"));
    }

    #[test]
    fn a_rest_only_project_or_a_server_not_used_through_ecoverse_is_explained_not_flagged() {
        let servers = (ok("Old", "old.example.com", "2026.4.0.25"), ok("New", "new.example.com", "2026.4.1.11"));
        for (eco, words) in [(EcoCheck::RestOnly, "REST only"), (EcoCheck::ServerNotUsed, "Not used for Old")] {
            let r = connect_items(&servers.0, &servers.1, false, &eco, &DestEcoverse::NotLinked, &key());
            assert!(r.can_continue);
            let i = r.items.iter().find(|i| i.id == "ecoverse").unwrap();
            assert_eq!(i.level, ConnectLevel::Ok, "a choice isn't a problem");
            assert!(i.detail.contains(words), "{}", i.detail);
        }
    }

    #[test]
    fn another_tenant_needs_the_destinations_own_token_and_says_so_when_it_has_none() {
        let servers = (ok("Old", "old.example.com", "2026.4.0.25"), ok("New", "new.example.com", "2026.4.1.11"));
        let other = DestEcoverse::OtherTenant { tenant_id: "t".into() };
        let with = connect_items(&servers.0, &servers.1, false, &EcoCheck::Linked(1), &other, &DestKey { label: "New".into(), own_token: true, token_error: None });
        let i = with.items.iter().find(|i| i.id == "destination-ecoverse").unwrap();
        assert_eq!(i.level, ConnectLevel::Ok);
        assert!(i.detail.contains("New's own token"), "{}", i.detail);

        let without = connect_items(&servers.0, &servers.1, false, &EcoCheck::Linked(1), &other, &key());
        let i = without.items.iter().find(|i| i.id == "destination-ecoverse").unwrap();
        assert_eq!(i.level, ConnectLevel::Warn);
        assert!(i.detail.contains("no Ecoverse token of its own") && i.detail.contains("Manage servers"), "{}", i.detail);
        assert!(without.can_continue, "a warning doesn't stop the project");
    }

    #[test]
    fn a_destination_token_that_was_refused_is_a_warning_that_says_why() {
        let servers = (ok("Old", "old.example.com", "2026.4.0.25"), ok("New", "new.example.com", "2026.4.1.11"));
        let other = DestEcoverse::OtherTenant { tenant_id: "t".into() };
        let refused = DestKey { label: "New".into(), own_token: true, token_error: Some("HTTP 401.".into()) };
        let r = connect_items(&servers.0, &servers.1, false, &EcoCheck::Linked(1), &other, &refused);
        let i = r.items.iter().find(|i| i.id == "destination-ecoverse").unwrap();
        assert_eq!(i.level, ConnectLevel::Warn);
        assert!(i.detail.contains("New's own token didn't work: HTTP 401.") && i.detail.contains("Manage servers"), "{}", i.detail);
        assert!(r.can_continue);
    }

    fn lacking(label: &str, capabilities: &[Capability]) -> ServerCheck {
        let mut check = ok(label, &format!("{}.example.com", label.to_lowercase()), "2026.4.0.25");
        check.capabilities = match Capabilities::complete() {
            Capabilities::Known(all) => Capabilities::Known(all.into_iter().map(|mut s| {
                if capabilities.contains(&s.capability) {
                    s.missing = vec![format!("GET /api/{:?}", s.capability)];
                }
                s
            }).collect()),
            other => other,
        };
        check
    }

    #[test]
    fn a_missing_optional_feature_is_an_amber_item_that_names_the_workaround() {
        let r = connect_items(&ok("Old", "old.example.com", "2026.4.0.25"), &lacking("New", &[Capability::CreateStructure, Capability::RetireRecord]), false, &EcoCheck::Linked(1), &DestEcoverse::NotLinked, &key());
        let i = r.items.iter().find(|i| i.id == "capabilities").unwrap();
        assert_eq!(i.level, ConnectLevel::Warn);
        assert!(i.detail.contains("New can't do creating customers and sites") && i.detail.contains("Create the customers and sites in N-central"), "{}", i.detail);
        assert!(!i.detail.contains("removing old device records"), "retiring is the source's job, and Old has it: {}", i.detail);
        assert!(r.can_continue);
    }

    #[test]
    fn a_missing_required_feature_stops_the_project() {
        let r = connect_items(&ok("Old", "old.example.com", "2026.4.0.25"), &lacking("New", &[Capability::ReadRegistrationToken]), false, &EcoCheck::Linked(1), &DestEcoverse::NotLinked, &key());
        assert_eq!(level(&r, "capabilities"), &ConnectLevel::Err);
        assert!(!r.can_continue);
    }

    #[test]
    fn launching_through_n_central_is_required_only_when_the_project_is_rest_only() {
        let src = lacking("Old", &[Capability::RestLaunch]);
        let dst = ok("New", "new.example.com", "2026.4.0.25");
        let mixed = connect_items(&src, &dst, false, &EcoCheck::Linked(1), &DestEcoverse::NotLinked, &key());
        assert_eq!(level(&mixed, "capabilities"), &ConnectLevel::Warn);
        let rest = connect_items(&src, &dst, false, &EcoCheck::RestOnly, &DestEcoverse::NotLinked, &key());
        assert_eq!(level(&rest, "capabilities"), &ConnectLevel::Err);
    }

    #[test]
    fn a_spec_that_cannot_be_read_is_a_warning_not_a_stop() {
        let mut unread = ok("Old", "old.example.com", "2026.4.0.25");
        unread.capabilities = Capabilities::Unknown("The server doesn't publish its API spec.".into());
        let r = connect_items(&unread, &ok("New", "new.example.com", "2026.4.0.25"), false, &EcoCheck::Linked(1), &DestEcoverse::NotLinked, &key());
        assert_eq!(level(&r, "capabilities"), &ConnectLevel::Warn);
        assert!(r.items.iter().find(|i| i.id == "capabilities").unwrap().detail.contains("doesn't publish"));
        assert!(r.can_continue);
    }

    #[test]
    fn on_one_server_both_roles_count() {
        let same = lacking("Lab", &[Capability::GenerateToken, Capability::RetireRecord]);
        let r = connect_items(&same, &lacking("Lab", &[]), true, &EcoCheck::Linked(1), &DestEcoverse::NotLinked, &key());
        let detail = &r.items.iter().find(|i| i.id == "capabilities").unwrap().detail;
        assert!(detail.contains("generating registration tokens") && detail.contains("removing old device records"), "{detail}");
    }
}
