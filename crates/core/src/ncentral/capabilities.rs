//! What a server's REST API offers, read from the server's own OpenAPI spec (`/api-explorer/openapi-spec.json`) instead
//! of guessed from its version. Each capability is the set of endpoints one part of the workflow needs. A server whose
//! spec can't be read is `Unknown`, which warns but never blocks: older releases may not serve the spec at all.

use std::collections::HashSet;

use serde_json::Value;

use super::rest::{Call, Method, RestError};
use crate::Core;

pub const SPEC_PATH: &str = "/api-explorer/openapi-spec.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    ReadOrgTree,
    ReadDevices,
    ReadRegistrationToken,
    GenerateToken,
    CreateStructure,
    RestLaunch,
    RetireRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Source,
    Destination,
}

impl Capability {
    /// The operations the capability needs, with path parameters written as `{}`.
    fn operations(self) -> &'static [(Method, &'static str)] {
        use Method::{Delete, Get, Post};
        match self {
            Capability::ReadOrgTree => &[(Get, "/api/org-units")],
            Capability::ReadDevices => &[(Get, "/api/org-units/{}/devices"), (Get, "/api/devices/{}"), (Get, "/api/devices/{}/assets")],
            Capability::ReadRegistrationToken => &[(Get, "/api/org-units/{}/registration-token")],
            Capability::GenerateToken => &[(Get, "/api/customers/{}/software/installers"), (Post, "/api/customers/{}/software/installers")],
            Capability::CreateStructure => &[(Post, "/api/service-orgs/{}/customers"), (Post, "/api/customers/{}/sites")],
            Capability::RestLaunch => &[(Post, "/api/scheduled-tasks/direct"), (Get, "/api/scheduled-tasks/{}/status/details")],
            Capability::RetireRecord => &[(Delete, "/api/devices/{}")],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Capability::ReadOrgTree => "reading the organization tree",
            Capability::ReadDevices => "reading devices",
            Capability::ReadRegistrationToken => "reading registration tokens",
            Capability::GenerateToken => "generating registration tokens",
            Capability::CreateStructure => "creating customers and sites",
            Capability::RestLaunch => "launching through N-central",
            Capability::RetireRecord => "removing old device records",
        }
    }

    /// What to do when the server can't offer it.
    pub fn workaround(self) -> &'static str {
        match self {
            Capability::GenerateToken => "Create the registration tokens in N-central.",
            Capability::CreateStructure => "Create the customers and sites in N-central, then map them.",
            Capability::RestLaunch => "Devices can only be launched through Ecoverse.",
            Capability::RetireRecord => "Delete the old records in N-central yourself.",
            Capability::ReadOrgTree | Capability::ReadDevices | Capability::ReadRegistrationToken => "N-voke can't work with this server.",
        }
    }

    /// Whether a project can't go on without it: `(capability, required)` for what a server does in a role.
    pub fn needs(role: Role, rest_only: bool) -> Vec<(Capability, bool)> {
        use Capability::*;
        match role {
            Role::Source => vec![(ReadOrgTree, true), (ReadDevices, true), (RestLaunch, rest_only), (RetireRecord, false)],
            Role::Destination => vec![(ReadOrgTree, true), (ReadDevices, true), (ReadRegistrationToken, true), (GenerateToken, false), (CreateStructure, false)],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityStatus {
    pub capability: Capability,
    /// The operations the spec doesn't list, like `POST /api/customers/{}/sites`.
    pub missing: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capabilities {
    /// The spec couldn't be read; the reason is for the person.
    Unknown(String),
    Known(Vec<CapabilityStatus>),
}

impl Capabilities {
    /// The operations missing for `capability`, when the spec is known and doesn't list them all.
    pub fn missing(&self, capability: Capability) -> Option<&[String]> {
        match self {
            Capabilities::Known(all) => all.iter().find(|s| s.capability == capability).map(|s| s.missing.as_slice()).filter(|m| !m.is_empty()),
            Capabilities::Unknown(_) => None,
        }
    }

    /// Everything offered. For tests of code that takes capabilities.
    #[doc(hidden)]
    pub fn complete() -> Self {
        Capabilities::Known(ALL.iter().map(|&capability| CapabilityStatus { capability, missing: vec![] }).collect())
    }
}

const ALL: [Capability; 7] = [
    Capability::ReadOrgTree,
    Capability::ReadDevices,
    Capability::ReadRegistrationToken,
    Capability::GenerateToken,
    Capability::CreateStructure,
    Capability::RestLaunch,
    Capability::RetireRecord,
];

/// `/api/devices/{deviceId}` and `/api/devices/{id}` are the same operation: the names of path parameters vary.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    for part in &mut parts {
        if part.starts_with('{') && part.ends_with('}') {
            *part = "{}";
        }
    }
    parts.join("/")
}

/// Reads the operations out of an OpenAPI document: its `paths`, each with the methods it lists.
pub fn evaluate(spec: &Value) -> Result<Capabilities, String> {
    let paths = spec.get("paths").and_then(Value::as_object).filter(|p| !p.is_empty()).ok_or("The spec lists no paths.")?;
    let mut offered: HashSet<(Method, String)> = HashSet::new();
    for (path, item) in paths {
        for method in [Method::Get, Method::Post, Method::Put, Method::Delete] {
            if item.get(method.as_str().to_ascii_lowercase()).is_some() {
                offered.insert((method, normalize(path)));
            }
        }
    }
    let statuses = ALL
        .iter()
        .map(|&capability| CapabilityStatus {
            capability,
            missing: capability.operations().iter().filter(|(m, p)| !offered.contains(&(*m, p.to_string()))).map(|(m, p)| format!("{} {p}", m.as_str())).collect(),
        })
        .collect();
    Ok(Capabilities::Known(statuses))
}

impl Core {
    /// What the server's REST API offers. Read once per server and remembered, until its JWT is replaced or it is
    /// removed; `refresh` reads it again (the Connect step).
    pub async fn ncentral_capabilities(&self, server_id: &str, refresh: bool) -> Capabilities {
        if self.rest.assume_complete() {
            return Capabilities::complete();
        }
        if !refresh && let Some(known) = self.rest.capabilities(server_id) {
            return known;
        }
        let _reading = self.rest.spec_read.lock().await;
        if !refresh && let Some(known) = self.rest.capabilities(server_id) {
            return known;
        }
        let read = self.ncentral_call(server_id, Call::get(SPEC_PATH)).await;
        let found = match read {
            Ok(spec) => evaluate(&spec).unwrap_or_else(|why| Capabilities::Unknown(format!("The server's API spec was unreadable: {why}"))),
            Err(RestError { status: Some(404), .. }) => Capabilities::Unknown("The server doesn't publish its API spec.".into()),
            Err(e) => Capabilities::Unknown(format!("The server's API spec couldn't be read: {}", e.message)),
        };
        self.rest.remember_capabilities(server_id, found.clone());
        found
    }

    /// Refuses an operation the server's spec says it doesn't have. An unreadable spec allows it: the call itself then
    /// shows what the server thinks.
    pub(crate) async fn require_capability(&self, server_id: &str, capability: Capability) -> Result<(), RestError> {
        let Some(missing) = self.ncentral_capabilities(server_id, false).await.missing(capability).map(<[String]>::to_vec) else { return Ok(()) };
        let label = self.ncentral_server(server_id).map(|s| s.label)?;
        Err(RestError::from(format!("{label} doesn't offer {} (its API spec lacks {}). {}", capability.name(), missing.join(", "), capability.workaround())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(operations: &[(&str, &str)]) -> Value {
        let mut paths = serde_json::Map::new();
        for (method, path) in operations {
            paths.entry(path.to_string()).or_insert_with(|| json!({})).as_object_mut().unwrap().insert(method.to_string(), json!({}));
        }
        json!({ "openapi": "3.0.1", "paths": paths })
    }

    const BASE: &[(&str, &str)] = &[
        ("get", "/api/org-units"),
        ("get", "/api/org-units/{orgUnitId}/devices"),
        ("get", "/api/devices/{deviceId}"),
        ("get", "/api/devices/{deviceId}/assets"),
        ("get", "/api/org-units/{orgUnitId}/registration-token"),
    ];

    #[test]
    fn parameter_names_do_not_matter_and_a_full_spec_misses_nothing() {
        let mut all = BASE.to_vec();
        all.extend([
            ("get", "/api/customers/{id}/software/installers"),
            ("post", "/api/customers/{id}/software/installers"),
            ("post", "/api/service-orgs/{soId}/customers"),
            ("post", "/api/customers/{customerId}/sites"),
            ("post", "/api/scheduled-tasks/direct"),
            ("get", "/api/scheduled-tasks/{taskId}/status/details"),
            ("delete", "/api/devices/{deviceId}"),
        ]);
        assert_eq!(evaluate(&spec(&all)).unwrap(), Capabilities::complete());
    }

    #[test]
    fn each_gap_names_the_operations_it_lacks_and_leaves_the_rest_alone() {
        let found = evaluate(&spec(BASE)).unwrap();
        assert_eq!(found.missing(Capability::ReadDevices), None);
        assert_eq!(found.missing(Capability::CreateStructure).unwrap(), ["POST /api/service-orgs/{}/customers", "POST /api/customers/{}/sites"]);
        assert_eq!(found.missing(Capability::RetireRecord).unwrap(), ["DELETE /api/devices/{}"]);

        // A path with the wrong method for it is a gap too: the device record can be read but not deleted.
        let mut read_only = BASE.to_vec();
        read_only.push(("post", "/api/customers/{id}/software/installers"));
        let found = evaluate(&spec(&read_only)).unwrap();
        assert_eq!(found.missing(Capability::GenerateToken).unwrap(), ["GET /api/customers/{}/software/installers"]);
    }

    #[test]
    fn a_document_without_paths_is_unreadable_not_empty() {
        assert!(evaluate(&json!({ "openapi": "3.0.1" })).is_err());
        assert!(evaluate(&json!({ "paths": {} })).is_err());
    }

    #[test]
    fn what_each_role_needs_marks_launching_required_only_for_a_rest_only_project() {
        let source = |rest_only| Capability::needs(Role::Source, rest_only).into_iter().find(|(c, _)| *c == Capability::RestLaunch).unwrap().1;
        assert!(!source(false) && source(true));
        assert!(Capability::needs(Role::Destination, false).contains(&(Capability::ReadRegistrationToken, true)));
    }

    use crate::ncentral::rest::testing::{Fake, auth_ok, ok, status};

    /// A server that answers with `spec` (or a 404 for `None`) and counts the writes it was sent.
    async fn server_with(spec: Option<Value>) -> (std::sync::Arc<Core>, std::sync::Arc<Fake>, String) {
        let fake = Fake::new(move |req, _| {
            let path = req.url.trim_start_matches("https://").split_once('/').unwrap().1.to_string();
            match (req.method, path.as_str()) {
                (_, "api/auth/authenticate") => auth_ok(3600),
                (Method::Get, "api-explorer/openapi-spec.json") => spec.clone().map_or_else(|| status(404, "{}"), ok),
                (Method::Post, _) => status(201, r#"{"customerId":900}"#),
                _ => status(404, "{}"),
            }
        });
        let core = Core::for_tests("capabilities");
        core.rest.set_assume_complete(false);
        let id = core.save_ncentral_server(None, "Lab", "lab.example.com", Some("jwt".into()), None).await.unwrap().id;
        core.set_rest_transport(fake.clone());
        (core, fake, id)
    }

    #[tokio::test]
    async fn a_write_the_spec_does_not_list_is_refused_before_anything_is_sent() {
        let (core, fake, id) = server_with(Some(spec(BASE))).await;
        let refused = core.ncentral_create_customer(&id, "1", "Acme").await.unwrap_err();
        assert!(refused.message.contains("Lab doesn't offer creating customers and sites") && refused.message.contains("Create the customers and sites in N-central"), "{}", refused.message);
        assert!(fake.paths().iter().all(|p| !p.starts_with("POST api/service-orgs")), "{:?}", fake.paths());
    }

    #[tokio::test]
    async fn a_write_the_spec_lists_goes_through_and_the_spec_is_read_once() {
        let mut with = BASE.to_vec();
        with.extend([("post", "/api/service-orgs/{soId}/customers"), ("post", "/api/customers/{customerId}/sites")]);
        let (core, fake, id) = server_with(Some(spec(&with))).await;
        assert_eq!(core.ncentral_create_customer(&id, "1", "Acme").await.unwrap(), "900");
        core.ncentral_create_customer(&id, "1", "Beta").await.unwrap();
        assert_eq!(fake.paths().iter().filter(|p| p.starts_with("GET api-explorer")).count(), 1);
    }

    #[tokio::test]
    async fn a_server_that_publishes_no_spec_is_not_blocked() {
        let (core, _fake, id) = server_with(None).await;
        assert!(matches!(core.ncentral_capabilities(&id, false).await, Capabilities::Unknown(_)));
        assert_eq!(core.ncentral_create_customer(&id, "1", "Acme").await.unwrap(), "900");
    }

    #[tokio::test]
    async fn replacing_the_jwt_makes_the_server_be_read_again() {
        let (core, fake, id) = server_with(Some(spec(BASE))).await;
        core.ncentral_capabilities(&id, false).await;
        core.ncentral_capabilities(&id, false).await;
        core.save_ncentral_server(Some(id.clone()), "Lab", "lab.example.com", Some("jwt-2".into()), None).await.unwrap();
        core.ncentral_capabilities(&id, false).await;
        assert_eq!(fake.paths().iter().filter(|p| p.starts_with("GET api-explorer")).count(), 2);
    }
}
