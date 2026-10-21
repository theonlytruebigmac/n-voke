//! The Ecoverse operations migration adds to the ones N-voke already uses:
//! publishing, replacing and deleting the endpoint script, cancelling stalled
//! launches, and reading the identity fields that link an Ecoverse asset to an
//! N-central device. Launching is the existing `run_script` with
//! `run_when_online` set. Everything goes through `Core::graphql`, so audit,
//! dry-run and 429 handling apply as for every other operation.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::planner::EcoAsset;
use crate::{
    Core,
    api::{RunInput, plural, throw_if_errors},
    inputs::InputType,
    model::StatusRef,
    ncentral::model::normalize_mac,
};

const MUTATION_ERRORS: &str = "errors { __typename ... on Error { message } }";

/// Fields that tie an asset to its N-central device and let old and new devices be matched.
/// One request: 30-odd fields, well inside the 130-field limit.
const ECO_DEVICE_FIELDS: &str = "id name agentConnection { status } \
    ncentralDevice { deviceId server { id origin } } \
    systemInfo { hostname serialNumber } networkInterfaces { macAddress } \
    operatingSystemInfo { name architecture type }";

fn id_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    crate::ncentral::model::lenient_opt_id(d)
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerRef {
    /// Includes the port, for example `https://old.example.com:443`.
    pub origin: Option<String>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NcentralLink {
    #[serde(default, deserialize_with = "id_string")]
    pub device_id: Option<String>,
    pub server: Option<ServerRef>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub hostname: Option<String>,
    pub serial_number: Option<String>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInterface {
    pub mac_address: Option<String>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OsInfo {
    pub name: Option<String>,
    pub architecture: Option<String>,
    /// `WINDOWS`, and so on.
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

/// An Ecoverse asset with what migration needs from it.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EcoDevice {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub agent_connection: Option<StatusRef>,
    #[serde(default)]
    pub ncentral_device: Option<NcentralLink>,
    #[serde(default)]
    pub system_info: Option<SystemInfo>,
    /// Null until inventory reports, and can hold empty addresses.
    #[serde(default)]
    pub network_interfaces: Option<Vec<Option<NetworkInterface>>>,
    #[serde(default)]
    pub operating_system_info: Option<OsInfo>,
}

impl EcoDevice {
    /// The agent has sent asset data: a host name or serial number, or a network address. An asset can exist and be
    /// `CONNECTED` for a long while before any of this arrives, and scripts are held until it does.
    pub fn has_data(&self) -> bool {
        let filled = |text: &Option<String>| text.as_deref().is_some_and(|t| !t.trim().is_empty());
        self.system_info.as_ref().is_some_and(|s| filled(&s.hostname) || filled(&s.serial_number)) || !self.macs().is_empty()
    }

    pub fn connected(&self) -> bool {
        self.agent_connection.as_ref().is_some_and(|c| c.status.eq_ignore_ascii_case("CONNECTED"))
    }

    pub fn macs(&self) -> Vec<String> {
        let mut macs: Vec<String> = self.network_interfaces.iter().flatten().flatten().filter_map(|n| n.mac_address.as_deref().and_then(normalize_mac)).collect();
        macs.sort();
        macs.dedup();
        macs
    }

    /// The N-central server this asset belongs to, as a bare host name.
    pub fn server_host(&self) -> Option<String> {
        self.ncentral_device.as_ref()?.server.as_ref()?.origin.as_deref().and_then(origin_host)
    }
}

/// `https://Old.example.com:443/` becomes `old.example.com`: origins carry a port, host names don't.
pub fn origin_host(origin: &str) -> Option<String> {
    let rest = origin.split_once("://").map_or(origin, |(_, r)| r);
    let host = rest.split(['/', ':']).next()?.trim().to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// The Ecoverse assets that belong to the N-central server `host`, by N-central device ID.
pub fn index_by_ncentral_id(devices: &[EcoDevice], host: &str) -> HashMap<String, EcoAsset> {
    devices
        .iter()
        .filter(|d| d.server_host().as_deref() == Some(host))
        .filter_map(|d| Some((d.ncentral_device.as_ref()?.device_id.clone()?, EcoAsset { asset_id: d.id.clone(), connected: d.connected() })))
        .collect()
}

/// What Ecoverse shows for a device that has moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EcoSeen {
    pub connected: bool,
    pub has_data: bool,
}

/// By N-central device ID on `host`: whether each has an asset, whether it is connected, and whether it has sent data.
pub fn dest_asset_states(devices: &[EcoDevice], host: &str) -> HashMap<String, EcoSeen> {
    devices
        .iter()
        .filter(|d| d.server_host().as_deref() == Some(host))
        .filter_map(|d| Some((d.ncentral_device.as_ref()?.device_id.clone()?, EcoSeen { connected: d.connected(), has_data: d.has_data() })))
        .collect()
}

/// Whether Ecoverse sees any device of the N-central server `host`: the server is linked to this tenant.
pub fn is_linked(devices: &[EcoDevice], host: &str) -> bool {
    devices.iter().any(|d| d.server_host().as_deref() == Some(host))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptOutput {
    pub name: String,
    pub variable: String,
    pub kind: InputType,
}

/// An Ecoverse script as published: the same definition is used to create it and (replacing everything) to update it.
pub struct ScriptDefinition {
    pub organization_id: String,
    /// At most 64 characters.
    pub name: String,
    /// At most 1024 characters.
    pub description: String,
    pub content: String,
    /// 1 to 3600 seconds (the API treats 0 as 3600).
    pub timeout_seconds: u32,
    /// With their default values.
    pub inputs: Vec<RunInput>,
    pub outputs: Vec<ScriptOutput>,
}

impl ScriptDefinition {
    pub fn validate(&self) -> Result<(), String> {
        let long = |text: &str, max: usize| text.chars().count() > max;
        if self.name.trim().is_empty() || long(&self.name, 64) {
            return Err("A script's name must be 1 to 64 characters.".into());
        }
        if long(&self.description, 1024) {
            return Err("A script's description can be at most 1024 characters.".into());
        }
        if self.timeout_seconds == 0 || self.timeout_seconds > 3600 {
            return Err("A script's timeout must be 1 to 3600 seconds.".into());
        }
        if self.content.trim().is_empty() {
            return Err("The script has no content.".into());
        }
        let names = self.inputs.iter().map(|i| (&i.name, &i.variable)).chain(self.outputs.iter().map(|o| (&o.name, &o.variable)));
        if let Some((name, variable)) = names.into_iter().find(|(n, v)| long(n, 48) || long(v, 48) || n.is_empty() || v.is_empty()) {
            return Err(format!("Script input or output \"{name}\" ({variable}): names and variables must be 1 to 48 characters."));
        }
        Ok(())
    }

    /// The mutation's `input`, as proven in the lab. Windows only; `scriptId` is added for an update.
    pub fn to_input(&self, script_id: Option<&str>) -> Value {
        let mut input = json!({
            "organizationId": self.organization_id,
            "name": self.name,
            "description": self.description,
            "language": "POWERSHELL",
            "isWindowsCompatible": true,
            "isLinuxCompatible": false,
            "isDarwinCompatible": false,
            "content": self.content,
            "timeoutSeconds": self.timeout_seconds,
            "inputs": self.inputs.iter().map(|i| json!({ "name": i.name, "variable": i.variable, "type": i.kind_name(), "value": i.value })).collect::<Vec<_>>(),
            "outputs": self.outputs.iter().map(|o| json!({ "name": o.name, "variable": o.variable, "type": (RunInput { name: String::new(), variable: String::new(), kind: o.kind, value: Default::default() }).kind_name() })).collect::<Vec<_>>(),
        });
        if let Some(id) = script_id {
            input["scriptId"] = json!(id);
        }
        input
    }
}

impl Core {
    /// Every asset in the organizations, with the fields that identify it. Follows pagination.
    /// With no organizations, every asset the key can see; callers pick their server's devices by host.
    pub async fn eco_devices(&self, org_ids: &[String]) -> Result<Vec<EcoDevice>, String> {
        if org_ids.is_empty() {
            return self.fetch_all_pages::<EcoDevice>("assetSearch", "", ECO_DEVICE_FIELDS, json!({}), "").await;
        }
        self.fetch_all_pages::<EcoDevice>("assetSearch", "inOrganizations: $orgIds", ECO_DEVICE_FIELDS, json!({ "orgIds": org_ids }), ", $orgIds: [ID!]!").await
    }

    /// Creates the script, or replaces the one with `existing_id`, and returns its ID.
    pub async fn publish_script(&self, definition: &ScriptDefinition, existing_id: Option<&str>) -> Result<String, String> {
        definition.validate()?;
        let (query, root, action, summary) = match existing_id {
            None => (
                format!("mutation CreateScript($input: ScriptCreateMutationInput!) {{ scriptCreate(input: $input) {{ item {{ id }} {MUTATION_ERRORS} }} }}"),
                "scriptCreate",
                "script.create",
                format!("Published script {}", definition.name),
            ),
            Some(_) => (
                format!("mutation UpdateScript($input: ScriptUpdateMutationInput!) {{ scriptUpdate(input: $input) {{ item {{ id }} {MUTATION_ERRORS} }} }}"),
                "scriptUpdate",
                "script.update",
                format!("Updated script {}", definition.name),
            ),
        };
        let data = self.mutate(&query, json!({ "input": definition.to_input(existing_id) }), action, summary).await?;
        throw_if_errors(&data[root], "Could not publish the script")?;
        data[root]["item"]["id"].as_str().map(String::from).ok_or_else(|| "The script was published but the API returned no ID.".to_string())
    }

    pub async fn delete_scripts(&self, script_ids: &[String], label: &str) -> Result<(), String> {
        let query = format!("mutation DeleteScripts($ids: [ID!]!) {{ scriptsDelete(input: {{ scriptIds: $ids }}) {{ items {{ id }} {MUTATION_ERRORS} }} }}");
        let data = self.mutate(&query, json!({ "ids": script_ids }), "script.delete", format!("Deleted script {label}")).await?;
        throw_if_errors(&data["scriptsDelete"], "Could not delete the script")
    }

    /// Cancels launches still waiting for a device. Only `errors` is selected: asking for the
    /// cancelled items makes the API fail with INTERNAL_SERVER_ERROR although the cancel works.
    pub async fn cancel_script_tasks(&self, task_ids: &[String]) -> Result<(), String> {
        let query = format!("mutation CancelScriptTasks($ids: [ID!]!) {{ scriptTasksCancel(input: {{ taskIds: $ids }}) {{ {MUTATION_ERRORS} }} }}");
        let data = self.mutate(&query, json!({ "ids": task_ids }), "script.cancel", format!("Cancelled {}", plural(task_ids.len(), "script task"))).await?;
        throw_if_errors(&data["scriptTasksCancel"], "Could not cancel the task")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DRY_RUN_ERROR, model::InputValue};

    fn text(value: &str) -> InputValue {
        InputValue { string_value: Some(value.into()), ..Default::default() }
    }

    fn definition() -> ScriptDefinition {
        ScriptDefinition {
            organization_id: "org-1".into(),
            name: "N-voke migration 0.1.0 (3f9a2c1)".into(),
            description: "Moves a device to another N-central server.".into(),
            content: "Write-Output 'x'\n".repeat(20),
            timeout_seconds: 900,
            inputs: vec![
                RunInput { name: "Action".into(), variable: "Action".into(), kind: InputType::String, value: text("Launch") },
                RunInput { name: "RegistrationToken".into(), variable: "RegistrationToken".into(), kind: InputType::Password, value: text("-") },
                RunInput { name: "ReviewDays".into(), variable: "ReviewDays".into(), kind: InputType::Int, value: InputValue { int_value: Some(7), ..Default::default() } },
            ],
            outputs: vec![ScriptOutput { name: "Result".into(), variable: "Result".into(), kind: InputType::String }, ScriptOutput { name: "Ok".into(), variable: "Ok".into(), kind: InputType::Boolean }],
        }
    }

    fn eco(id: &str, device_id: Option<&str>, origin: &str, status: &str) -> EcoDevice {
        serde_json::from_value(json!({
            "id": id, "name": format!("PC-{id}"), "agentConnection": { "status": status },
            "ncentralDevice": device_id.map(|d| json!({ "deviceId": d, "server": { "id": "s", "origin": origin } })),
            "systemInfo": { "hostname": "PC", "serialNumber": "VMW-1" },
            "networkInterfaces": [{ "macAddress": "AA-BB-CC-DD-EE-01" }, { "macAddress": "" }, null],
            "operatingSystemInfo": { "name": "Windows 11", "architecture": "64-bit", "type": "WINDOWS" }
        }))
        .unwrap()
    }

    #[test]
    fn the_input_matches_what_the_lab_proved_and_adds_the_id_for_an_update() {
        let create = definition().to_input(None);
        assert_eq!((create["language"].as_str(), create["isWindowsCompatible"].as_bool(), create["isLinuxCompatible"].as_bool()), (Some("POWERSHELL"), Some(true), Some(false)));
        assert_eq!(create["timeoutSeconds"], 900);
        assert_eq!(create["inputs"][1], json!({ "name": "RegistrationToken", "variable": "RegistrationToken", "type": "PASSWORD", "value": { "stringValue": "-" } }));
        assert_eq!(create["inputs"][2]["value"], json!({ "intValue": 7 }));
        assert_eq!(create["outputs"], json!([{ "name": "Result", "variable": "Result", "type": "STRING" }, { "name": "Ok", "variable": "Ok", "type": "BOOLEAN" }]));
        assert!(create.get("scriptId").is_none());
        assert_eq!(definition().to_input(Some("s-9"))["scriptId"], "s-9");
    }

    #[test]
    fn a_definition_the_api_would_refuse_is_refused_here() {
        assert!(definition().validate().is_ok());
        let mut d = definition();
        d.name = "x".repeat(65);
        assert!(d.validate().unwrap_err().contains("64"));
        let mut d = definition();
        d.timeout_seconds = 0;
        assert!(d.validate().is_err());
        d.timeout_seconds = 3601;
        assert!(d.validate().is_err());
        let mut d = definition();
        d.inputs[0].variable = "v".repeat(49);
        assert!(d.validate().unwrap_err().contains("48"));
        let mut d = definition();
        d.content = "  ".into();
        assert!(d.validate().is_err());
        let mut d = definition();
        d.description = "d".repeat(1025);
        assert!(d.validate().is_err());
    }

    #[test]
    fn origins_become_bare_host_names() {
        assert_eq!(origin_host("https://Source.Example.com:443").as_deref(), Some("source.example.com"));
        assert_eq!(origin_host("https://source.example.com/").as_deref(), Some("source.example.com"));
        assert_eq!(origin_host("source.example.com").as_deref(), Some("source.example.com"));
        assert_eq!(origin_host("https://"), None);
    }

    #[test]
    fn assets_are_indexed_by_n_central_id_for_one_server_only() {
        let devices = vec![
            eco("a1", Some("9001"), "https://src.example.com:443", "CONNECTED"),
            eco("a2", Some("9002"), "https://src.example.com:443", "DISCONNECTED"),
            eco("a3", Some("9003"), "https://other.example.com:443", "CONNECTED"),
            eco("a4", None, "", "CONNECTED"),
        ];
        let index = index_by_ncentral_id(&devices, "src.example.com");
        assert_eq!(index.len(), 2);
        assert_eq!(index["9001"], EcoAsset { asset_id: "a1".into(), connected: true });
        assert!(!index["9002"].connected);
        assert!(is_linked(&devices, "other.example.com") && !is_linked(&devices, "nowhere.example.com"));
    }

    #[test]
    fn identity_drops_empty_macs_and_tolerates_missing_inventory() {
        assert_eq!(eco("a1", Some("1"), "https://x:443", "CONNECTED").macs(), ["aabbccddee01"]);
        let bare: EcoDevice = serde_json::from_value(json!({ "id": "a9", "networkInterfaces": null, "ncentralDevice": { "deviceId": 42, "server": null } })).unwrap();
        assert!(bare.macs().is_empty() && !bare.connected() && bare.server_host().is_none());
        assert_eq!(bare.ncentral_device.unwrap().device_id.as_deref(), Some("42"), "a numeric device ID is read too");
    }

    #[tokio::test]
    async fn publishing_in_dry_run_lists_the_change_without_the_script_body_or_a_password() {
        let core = Core::for_tests("publish");
        core.set_dry_run(true);
        let mut d = definition();
        d.inputs[1].value = text("s3cret-default");
        assert_eq!(core.publish_script(&d, None).await.err().as_deref(), Some(DRY_RUN_ERROR));
        assert_eq!(core.publish_script(&d, Some("s-1")).await.err().as_deref(), Some(DRY_RUN_ERROR));
        let log = core.dry_run_log();
        assert_eq!((log[0].action.as_str(), log[0].operation.as_deref()), ("script.create", Some("CreateScript")));
        assert_eq!((log[1].action.as_str(), log[1].operation.as_deref()), ("script.update", Some("UpdateScript")));
        let shown = log[0].variables.to_string();
        assert!(!shown.contains("s3cret-default") && !shown.contains("Write-Output"), "{shown}");
        assert!(shown.contains("characters]"), "the body is shown as its size: {shown}");
        assert_eq!(log[1].variables["input"]["scriptId"], "s-1");
    }

    #[tokio::test]
    async fn deleting_and_cancelling_are_held_back_and_described() {
        let core = Core::for_tests("cancel");
        core.set_dry_run(true);
        assert_eq!(core.delete_scripts(&["s1".into()], "N-voke migration").await.err().as_deref(), Some(DRY_RUN_ERROR));
        assert_eq!(core.cancel_script_tasks(&["t1".into(), "t2".into()]).await.err().as_deref(), Some(DRY_RUN_ERROR));
        let log = core.dry_run_log();
        assert_eq!((log[0].action.as_str(), log[0].summary.as_str()), ("script.delete", "Deleted script N-voke migration"));
        assert_eq!((log[1].action.as_str(), log[1].summary.as_str()), ("script.cancel", "Cancelled 2 script tasks"));
        assert_eq!(log[1].variables["ids"], json!(["t1", "t2"]));
    }
}
