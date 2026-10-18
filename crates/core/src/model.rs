//! The API's data shapes. They serialize exactly as the TypeScript app's
//! `types.ts` does (camelCase, `null` for missing values), because the desktop
//! app and the terminal app read and write the same saved run history.

use serde::{Deserialize, Serialize};

fn is_none<T>(v: &Option<T>) -> bool {
    v.is_none()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Organization {
    pub id: String,
    pub name: String,
    #[serde(rename = "__typename", default)]
    pub typename: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScriptInputDecl {
    pub name: String,
    pub variable: String,
    /// STRING, PASSWORD, BOOLEAN, INT, FLOAT or DATE_TIME (kept as text so a new type can't break loading).
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Script {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "is_none")]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<i64>,
    #[serde(default)]
    pub inputs: Option<Vec<ScriptInputDecl>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NameRef {
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StatusRef {
    pub status: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OsRef {
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub customer: Option<NameRef>,
    #[serde(default)]
    pub site: Option<NameRef>,
    #[serde(default)]
    pub agent_connection: Option<StatusRef>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub operating_system: Option<OsRef>,
}

impl Asset {
    /// A device known only by ID and name (e.g. from a past job).
    pub fn stub(id: &str, name: &str) -> Self {
        Self { id: id.into(), name: name.into(), customer: None, site: None, agent_connection: None, operating_system: None }
    }

    pub fn os_name(&self) -> Option<&str> {
        self.operating_system.as_ref().and_then(|o| o.name.as_deref())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OrgRef {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: String,
    pub name: String,
    /// "StaticTag" or "DynamicTag".
    #[serde(rename = "__typename", default)]
    pub typename: String,
    #[serde(default)]
    pub asset_count: Option<i64>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub organization: Option<OrgRef>,
    /// Dynamic tags only: the rule as a JSON string.
    #[serde(default, skip_serializing_if = "is_none")]
    pub filter: Option<String>,
}

impl Tag {
    pub fn is_dynamic(&self) -> bool {
        self.typename == "DynamicTag"
    }
}

/// The typed value object the API expects for script inputs and outputs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct InputValue {
    #[serde(default, skip_serializing_if = "is_none")]
    pub string_value: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub boolean_value: Option<bool>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub int_value: Option<i64>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub float_value: Option<f64>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub date_time_value: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OutputValue {
    pub name: String,
    pub variable: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub value: Option<InputValue>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionOutput {
    #[serde(default)]
    pub run_as: Option<String>,
    #[serde(default)]
    pub std_output: Option<String>,
    #[serde(default)]
    pub std_error: Option<String>,
    #[serde(default)]
    pub outputs: Option<Vec<OutputValue>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Execution {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub duration_milliseconds: Option<f64>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub output: Option<ExecutionOutput>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ScriptRef {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Executions {
    #[serde(default)]
    pub nodes: Vec<Option<Execution>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub id: String,
    /// Only present from `get_task`, and not on a just-created task.
    #[serde(default, skip_serializing_if = "is_none")]
    pub name: Option<String>,
    pub status: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub script: Option<ScriptRef>,
    #[serde(default)]
    pub executions: Option<Executions>,
}
