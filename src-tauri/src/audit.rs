//! Local, append-only audit log of everything the app changes: every GraphQL
//! mutation (logged automatically by the `graphql` command) and settings
//! changes. Stored as JSON Lines in the app's data directory.

use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::vault::{self, Vault};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Manager};

const FILE_NAME: &str = "audit.jsonl";
/// Past this size the log is rotated to `audit.1.jsonl` (one previous file kept).
const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Human-readable context the UI attaches to a mutation.
#[derive(Deserialize, Clone)]
pub struct AuditMeta {
    pub action: String,
    pub summary: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub timestamp: String,
    pub user: String,
    pub action: String,
    pub summary: String,
    /// "ok" or "error".
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

impl AuditEntry {
    pub fn new(action: &str, summary: &str, outcome: Result<(), String>, details: Value) -> Self {
        Self {
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            user: os_user(),
            action: action.to_string(),
            summary: summary.to_string(),
            outcome: if outcome.is_ok() { "ok" } else { "error" }.to_string(),
            error: outcome.err(),
            details,
        }
    }
}

/// Serializes writers so concurrent commands can't interleave lines.
///
/// Each line is one record, sealed with the vault (base64). If encryption is
/// unavailable (no system keychain), records are still written, as plain JSON,
/// because losing the audit trail would be worse; Settings flags this.
pub struct AuditLog {
    lock: Mutex<()>,
    vault: Arc<Vault>,
}

impl AuditLog {
    pub fn new(vault: Arc<Vault>) -> Self {
        Self { lock: Mutex::new(()), vault }
    }

    pub fn path(app: &AppHandle) -> Result<PathBuf, String> {
        let dir = app.path().app_data_dir().map_err(|e| format!("No data directory: {e}"))?;
        Ok(dir.join(FILE_NAME))
    }

    /// Appends an entry. Failures are reported but never block the action itself.
    pub fn append(&self, app: &AppHandle, entry: &AuditEntry) {
        if let Err(e) = self.try_append(app, entry) {
            eprintln!("audit log write failed: {e}");
        }
    }

    fn encode(&self, entry: &AuditEntry) -> Result<String, String> {
        let json = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        Ok(self.vault.seal_line(&json).unwrap_or(json))
    }

    /// Plain JSON lines (from older versions, or written without a keychain) start with `{`.
    fn decode(&self, line: &str) -> Option<AuditEntry> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let json = if line.starts_with('{') { line.to_string() } else { self.vault.open_line(line).ok()? };
        serde_json::from_str(&json).ok()
    }

    fn try_append(&self, app: &AppHandle, entry: &AuditEntry) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap();
        let path = Self::path(app)?;
        if let Some(dir) = path.parent() {
            vault::create_private_dir(dir)?;
        }
        if fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
            fs::rename(&path, path.with_file_name("audit.1.jsonl")).map_err(|e| e.to_string())?;
        }
        let mut line = self.encode(entry)?;
        line.push('\n');
        vault::private_append(&path)?.write_all(line.as_bytes()).map_err(|e| e.to_string())
    }

    /// Most recent entries first (current file only).
    pub fn read(&self, app: &AppHandle, limit: usize) -> Result<Vec<AuditEntry>, String> {
        let _guard = self.lock.lock().unwrap();
        let path = Self::path(app)?;
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(format!("Could not read the audit log: {e}")),
        };
        Ok(text.lines().rev().filter_map(|l| self.decode(l)).take(limit).collect())
    }

    /// Re-writes any plain-JSON lines (from older versions) encrypted. Runs at startup.
    pub fn encrypt_existing(&self, app: &AppHandle) {
        if self.vault.status().is_err() {
            return;
        }
        let _guard = self.lock.lock().unwrap();
        let Ok(path) = Self::path(app) else { return };
        for file in [path.clone(), path.with_file_name("audit.1.jsonl")] {
            let Ok(text) = fs::read_to_string(&file) else { continue };
            if !text.lines().any(|l| l.trim_start().starts_with('{')) {
                continue;
            }
            let lines: Vec<String> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| if l.trim_start().starts_with('{') { self.vault.seal_line(l.trim()).unwrap_or_else(|_| l.to_string()) } else { l.to_string() })
                .collect();
            let body = lines.join("\n") + "\n";
            if let Err(e) = vault::write_private(&file, body.as_bytes()) {
                eprintln!("couldn't encrypt {}: {e}", file.display());
            }
        }
    }
}

fn os_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

/// True when a GraphQL document is a mutation (ignores leading comments/whitespace).
pub fn is_mutation(query: &str) -> bool {
    query
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .flat_map(str::split_whitespace)
        .next()
        .is_some_and(|w| w == "mutation" || w.starts_with("mutation(") || w.starts_with("mutation{"))
}

/// "mutation RunScript($input: ...)" -> "RunScript".
pub fn operation_name(query: &str) -> Option<String> {
    let rest = query.trim_start().strip_prefix("mutation")?.trim_start();
    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty()).then_some(name)
}

/// Blanks out values of PASSWORD-typed script inputs anywhere in the variables.
pub fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let is_password = map.get("type").and_then(Value::as_str) == Some("PASSWORD");
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), if is_password && k == "value" { Value::String("[redacted]".into()) } else { redact(v) });
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        other => other.clone(),
    }
}

/// Collects mutation-level errors (`data.<field>.errors[].message`) and
/// top-level GraphQL errors from a response body.
pub fn response_errors(body: &Value) -> Vec<String> {
    let mut errors: Vec<String> = body
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|e| e.get("message").and_then(Value::as_str).unwrap_or("Unknown error").to_string())
        .collect();
    if let Some(data) = body.get("data").and_then(Value::as_object) {
        for field in data.values() {
            for e in field.get("errors").and_then(Value::as_array).into_iter().flatten() {
                let message = e
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| e.get("__typename").and_then(Value::as_str))
                    .unwrap_or("Unknown error");
                errors.push(message.to_string());
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_mutations() {
        assert!(is_mutation("mutation RunScript($input: X!) { a }"));
        assert!(is_mutation("\n  # comment\n  mutation { a }"));
        assert!(!is_mutation("query GetTask($id: ID!) { task(id: $id) { id } }"));
        assert!(!is_mutation("{ asset(id: \"x\") { name } }"));
    }

    #[test]
    fn extracts_operation_names() {
        assert_eq!(operation_name("mutation RunScript($input: X!) {}").as_deref(), Some("RunScript"));
        assert_eq!(operation_name("mutation { a }"), None);
    }

    #[test]
    fn redacts_password_inputs_only() {
        let vars = json!({ "input": { "configuration": { "inputs": [
            { "variable": "pw", "type": "PASSWORD", "value": { "stringValue": "s3cret" } },
            { "variable": "app", "type": "STRING", "value": { "stringValue": "chrome" } }
        ] } } });
        let out = redact(&vars);
        let inputs = &out["input"]["configuration"]["inputs"];
        assert_eq!(inputs[0]["value"], json!("[redacted]"));
        assert_eq!(inputs[1]["value"]["stringValue"], json!("chrome"));
    }

    #[test]
    fn finds_mutation_level_errors() {
        let body = json!({ "data": { "staticTagCreate": { "item": null, "errors": [
            { "__typename": "StaticTagAlreadyExistsError", "message": "Tag with this name already exists" }
        ] } } });
        assert_eq!(response_errors(&body), vec!["Tag with this name already exists"]);
        assert!(response_errors(&json!({ "data": { "x": { "errors": null } } })).is_empty());
    }
}
