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
    ncentral::{EcoCredential, model::ServerInfo},
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
    /// Reads both servers and Ecoverse for a saved project and builds the checklist.
    pub async fn migration_connect_check(&self, project_id: &str) -> Result<ConnectReport, String> {
        let project = self.migration_load(project_id).await?.ok_or("That project no longer exists.")?;
        let source = self.ncentral_server(&project.source_server_id)?;
        let dest = self.ncentral_server(&project.dest_server_id)?;
        let same = source.id == dest.id;

        let read = |label: String, host: String, info: Result<ServerInfo, String>| ServerCheck { label, host, info };
        let source_check = read(source.label.clone(), source.host.clone(), self.ncentral_server_info(&source.id).await.map_err(String::from));
        let dest_check = if same {
            read(source.label.clone(), source.host.clone(), source_check.info.clone())
        } else {
            read(dest.label.clone(), dest.host.clone(), self.ncentral_server_info(&dest.id).await.map_err(String::from))
        };

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
        let dest_key = DestKey { label: dest.label.clone(), own_token: dest_view.is_some_and(|s| s.ecoverse == EcoCredential::Own && s.has_ecoverse_token) };
        Ok(connect_items(&source_check, &dest_check, same, &eco, &project.dest_ecoverse, &dest_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(label: &str, host: &str, release: &str) -> ServerCheck {
        ServerCheck { label: label.into(), host: host.into(), info: Ok(ServerInfo { ncentral: release.into(), version: "1.3.23".into() }) }
    }
    fn bad(label: &str, host: &str, why: &str) -> ServerCheck {
        ServerCheck { label: label.into(), host: host.into(), info: Err(why.into()) }
    }
    fn key() -> DestKey {
        DestKey { label: "New".into(), own_token: false }
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
        assert_eq!(ids(&r), ["source", "destination", "ecoverse", "destination-ecoverse"]);
        assert!(r.items.iter().all(|i| i.level == ConnectLevel::Ok));
        assert!(r.items[0].detail.contains("N-central 2026.4.0.25") && r.items[0].detail.contains("API 1.3.23"));
        assert!(r.items[2].detail.contains("412 devices") && r.items[2].detail.contains("old.example.com"));
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
        assert_eq!(ids(&r), ["server", "ecoverse", "destination-ecoverse"]);
        assert_eq!(r.items[0].title, "Server Lab");
        assert_eq!(r.items[2].detail, "Linked to the same Ecoverse tenant as the source: devices show up there again after the move.");
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
            assert!(r.items.iter().find(|i| i.id == "ecoverse").unwrap().detail.contains("REST"), "{eco_name}", eco_name = r.items[2].detail);
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
        let with = connect_items(&servers.0, &servers.1, false, &EcoCheck::Linked(1), &other, &DestKey { label: "New".into(), own_token: true });
        let i = with.items.iter().find(|i| i.id == "destination-ecoverse").unwrap();
        assert_eq!(i.level, ConnectLevel::Ok);
        assert!(i.detail.contains("New's own token"), "{}", i.detail);

        let without = connect_items(&servers.0, &servers.1, false, &EcoCheck::Linked(1), &other, &key());
        let i = without.items.iter().find(|i| i.id == "destination-ecoverse").unwrap();
        assert_eq!(i.level, ConnectLevel::Warn);
        assert!(i.detail.contains("no Ecoverse token of its own") && i.detail.contains("Manage servers"), "{}", i.detail);
        assert!(without.can_continue, "a warning doesn't stop the project");
    }
}
