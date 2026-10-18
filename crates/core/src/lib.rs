//! Everything N-voke does that isn't a user interface: the API key (OS
//! keychain), settings, every GraphQL request, the audit log, and the encrypted
//! local store. The desktop app (Tauri) and the terminal app both sit on top of
//! this, and share the same keychain entries and data directories, so a run
//! started in one shows up in the other.

pub mod api;
pub mod audit;
pub mod collections;
pub mod inputs;
pub mod job_store;
pub mod jobs;
pub mod poll;
pub mod model;
pub mod run;
pub mod rules;
pub mod sync;
pub mod tags;
pub mod targets;
pub mod watch;
pub mod task_state;
pub mod vault;

use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use audit::{AuditEntry, AuditLog, AuditMeta};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vault::Vault;

/// The desktop app's bundle identifier. It names the keychain service and the
/// config and data directories, so it must never change (see docs/DEVELOPMENT.md).
pub const APP_IDENTIFIER: &str = "com.fraziersystems.nable-script-runner";
pub(crate) const KEYRING_SERVICE: &str = APP_IDENTIFIER;
const KEYRING_USER: &str = "api-key";
pub const DEFAULT_API_URL: &str = "https://api.n-able.com/graphql";

/// Non-secret settings, persisted as JSON in the app's config directory.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub api_url: String,
    /// How long run history is kept on disk, in days; 0 = don't save it at all.
    #[serde(default = "default_history_days")]
    pub history_days: u32,
}

fn default_history_days() -> u32 {
    30
}

impl Default for Config {
    fn default() -> Self {
        Self { api_url: DEFAULT_API_URL.to_string(), history_days: default_history_days() }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub api_url: String,
    pub has_api_key: bool,
    pub history_days: u32,
    /// Whether local data can be encrypted (needs the system keychain), and why not if not.
    pub encryption_available: bool,
    pub encryption_error: Option<String>,
}

#[derive(Serialize)]
pub struct GraphQlResponse {
    pub status: u16,
    pub body: Value,
}

/// Parses a response body. The gateway in front of the API refuses a bad or expired token with a
/// plain-text 401/403 ("denied by external authz, unauthorized: token is not active"), not with
/// GraphQL, so that is presented as an ordinary access-denied error instead of a parse failure.
pub(crate) fn response_body(status: u16, text: &str) -> Result<Value, String> {
    if let Ok(body) = serde_json::from_str(text) {
        return Ok(body);
    }
    let snippet: String = text.trim().chars().take(200).collect();
    if status == 401 || status == 403 {
        return Ok(json!({ "errors": [{ "message": snippet }] }));
    }
    Err(format!("HTTP {status}: the API returned a non-JSON response: {snippet}"))
}

/// What a mutation returns in dry-run mode, in place of being sent. Callers treat it as a
/// failed change (nothing happened), and can tell it from a real failure with `is_dry_run_error`.
pub const DRY_RUN_ERROR: &str = "Dry run: nothing was sent to N-able.";

pub fn is_dry_run_error(error: &str) -> bool {
    error == DRY_RUN_ERROR
}

/// A change that dry-run mode held back: what would have been sent.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DryRunEntry {
    pub timestamp: String,
    pub action: String,
    pub summary: String,
    /// The GraphQL operation name, e.g. "RunScript".
    pub operation: Option<String>,
    /// The exact variables that would have been sent, with password inputs redacted.
    pub variables: Value,
}

/// Most held-back changes kept; the oldest go first.
const MAX_DRY_RUN_ENTRIES: usize = 200;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogView {
    pub path: String,
    pub entries: Vec<AuditEntry>,
}

/// Where the app keeps its files. Matches the directories Tauri resolves for
/// the same bundle identifier, so existing installs are found as they were.
#[derive(Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self, String> {
        let config = dirs::config_dir().ok_or("No config directory")?;
        let data = dirs::data_dir().ok_or("No data directory")?;
        Ok(Self { config_dir: config.join(APP_IDENTIFIER), data_dir: data.join(APP_IDENTIFIER) })
    }

    fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    fn store_dir(&self) -> PathBuf {
        self.data_dir.join("store")
    }

    fn store_file(&self, name: &str) -> Result<PathBuf, String> {
        vault::validate_name(name)?;
        Ok(self.store_dir().join(format!("{name}.enc")))
    }
}

/// Keychain calls can block (D-Bus round trips, unlock prompts), so they run
/// off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

fn keyring_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| {
        format!("The system keychain is unavailable ({e}). On Linux, make sure a Secret Service provider such as GNOME Keyring or KWallet is running.")
    })
}

fn read_stored_key() -> Result<Option<String>, String> {
    match keyring_entry()?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Could not read the API key from the keychain: {e}")),
    }
}

pub struct Core {
    paths: Paths,
    http: reqwest::Client,
    /// Key read from the keychain once, so polling doesn't hit the keychain
    /// (and possibly an unlock prompt) every few seconds.
    api_key: Mutex<Option<String>>,
    vault: Arc<Vault>,
    audit: AuditLog,
    /// Dry run: reads work as normal, but nothing that changes N-able is sent.
    dry_run: AtomicBool,
    dry_run_log: Mutex<Vec<DryRunEntry>>,
}

impl Core {
    /// `app_version` goes into the User-Agent header. Dry-run mode starts on when
    /// `NVOKE_DRY_RUN` is set to anything but empty or 0.
    pub fn new(app_version: &str) -> Result<Self, String> {
        Self::new_in(Paths::resolve()?, app_version)
    }

    /// Like `new`, with the config and data directories given (for tests).
    pub fn new_in(paths: Paths, app_version: &str) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(format!("n-voke/{app_version}"))
            .build()
            .map_err(|e| format!("Could not build the HTTP client: {e}"))?;
        let vault = Arc::new(Vault::new());
        let audit = AuditLog::new(vault.clone(), paths.data_dir.clone());
        let dry_run = std::env::var_os("NVOKE_DRY_RUN").is_some_and(|v| !v.is_empty() && v != "0");
        Ok(Self { paths, http, api_key: Mutex::new(None), vault, audit, dry_run: AtomicBool::new(dry_run), dry_run_log: Mutex::new(vec![]) })
    }

    /// A core for tests of code built on it: its own throwaway directory, a fake API key, and an
    /// API URL nothing listens on. Any request it makes fails at once, so a test can never reach
    /// N-able or touch the real keychain, settings or history.
    #[doc(hidden)]
    pub fn for_tests(name: &str) -> Arc<Core> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("nvoke-test-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let paths = Paths { config_dir: dir.join("config"), data_dir: dir.join("data") };
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(paths.config_file(), r#"{ "apiUrl": "https://127.0.0.1:9/graphql", "historyDays": 30 }"#).unwrap();
        let core = Core::new_in(paths, "test").unwrap();
        *core.api_key.lock().unwrap() = Some("test-key-not-real".into());
        core.set_dry_run(false);
        Arc::new(core)
    }

    pub fn set_dry_run(&self, on: bool) {
        self.dry_run.store(on, Ordering::SeqCst);
    }

    pub fn is_dry_run(&self) -> bool {
        self.dry_run.load(Ordering::SeqCst)
    }

    /// Whether this operation must not be sent: any mutation, while dry run is on.
    fn holds_back(&self, query: &str) -> bool {
        self.is_dry_run() && audit::is_mutation(query)
    }

    /// The changes held back so far, oldest first.
    pub fn dry_run_log(&self) -> Vec<DryRunEntry> {
        self.dry_run_log.lock().unwrap().clone()
    }

    fn hold_back(&self, query: &str, variables: &Value, audit: Option<&AuditMeta>) {
        let operation = audit::operation_name(query);
        let (action, summary) = match audit {
            Some(meta) => (meta.action.clone(), meta.summary.clone()),
            None => ("mutation".to_string(), format!("Ran {}", operation.as_deref().unwrap_or("a mutation"))),
        };
        let mut log = self.dry_run_log.lock().unwrap();
        log.push(DryRunEntry {
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            action,
            summary,
            operation,
            variables: audit::redact(variables),
        });
        if log.len() > MAX_DRY_RUN_ENTRIES {
            log.remove(0);
        }
    }

    /// Converts any plain-JSON audit lines from older versions. Call once at startup, off the UI thread.
    pub fn encrypt_existing_audit(&self) {
        self.audit.encrypt_existing();
    }

    fn load_config(&self) -> Config {
        fs::read_to_string(self.paths.config_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_config(&self, config: &Config) -> Result<(), String> {
        let path = self.paths.config_file();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| format!("Could not write {}: {e}", path.display()))
    }

    async fn current_key(&self) -> Result<Option<String>, String> {
        if let Some(key) = self.api_key.lock().unwrap().clone() {
            return Ok(Some(key));
        }
        let key = blocking(read_stored_key).await?;
        *self.api_key.lock().unwrap() = key.clone();
        Ok(key)
    }

    pub async fn get_settings(&self) -> Result<Settings, String> {
        let has_api_key = self.current_key().await?.is_some();
        let vault = self.vault.clone();
        let encryption = blocking(move || Ok(vault.status())).await?;
        let config = self.load_config();
        Ok(Settings {
            api_url: config.api_url,
            has_api_key,
            history_days: config.history_days,
            encryption_available: encryption.is_ok(),
            encryption_error: encryption.err(),
        })
    }

    pub async fn save_settings(&self, api_url: String, api_key: Option<String>, history_days: Option<u32>) -> Result<Settings, String> {
        let api_url = api_url.trim().to_string();
        let parsed = reqwest::Url::parse(&api_url).map_err(|e| format!("Invalid API URL: {e}"))?;
        if parsed.scheme() != "https" {
            return Err("The API URL must use https://".into());
        }
        let previous = self.load_config();
        let previous_url = previous.api_url.clone();
        let history_days = history_days.unwrap_or(previous.history_days).min(3650);
        self.save_config(&Config { api_url: api_url.clone(), history_days })?;
        if previous.history_days != history_days {
            let summary = if history_days == 0 { "Stopped saving run history".to_string() } else { format!("Keep run history for {history_days} days") };
            let details = json!({ "from": previous.history_days, "to": history_days });
            self.audit.append(&AuditEntry::new("settings.history", &summary, Ok(()), details));
        }
        if previous_url != api_url {
            let entry = AuditEntry::new(
                "settings.endpoint",
                &format!("Changed the API endpoint to {api_url}"),
                Ok(()),
                json!({ "from": previous_url, "to": api_url }),
            );
            self.audit.append(&entry);
        }

        if let Some(key) = api_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
            let had_key = self.current_key().await.ok().flatten().is_some();
            let to_store = key.clone();
            let result = blocking(move || {
                keyring_entry()?
                    .set_password(&to_store)
                    .map_err(|e| format!("Could not save the API key to the keychain: {e}"))
            })
            .await;
            let summary = if had_key { "Replaced the saved API key" } else { "Saved an API key" };
            self.audit.append(&AuditEntry::new("settings.api_key", summary, result.clone(), Value::Null));
            result?;
            *self.api_key.lock().unwrap() = Some(key);
        }

        self.get_settings().await
    }

    pub async fn clear_api_key(&self) -> Result<(), String> {
        let result = blocking(|| match keyring_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Could not remove the API key from the keychain: {e}")),
        })
        .await;
        self.audit.append(&AuditEntry::new("settings.api_key", "Removed the saved API key", result.clone(), Value::Null));
        result?;
        *self.api_key.lock().unwrap() = None;
        Ok(())
    }

    /// Sends one GraphQL operation and returns the HTTP status with the parsed
    /// body. GraphQL-level `errors` are left for the caller to interpret, since
    /// some queries legitimately return partial data.
    ///
    /// Every mutation is written to the audit log here, whether or not the caller
    /// supplied `audit` context, so nothing that changes N-able goes unrecorded.
    pub async fn graphql(&self, query: &str, variables: &Value, audit: Option<AuditMeta>) -> Result<GraphQlResponse, String> {
        // Every mutation in the app passes through here, so this one check covers them all.
        // Nothing is sent and nothing is audited: the audit log records changes that happened.
        if self.holds_back(query) {
            self.hold_back(query, variables, audit.as_ref());
            return Err(DRY_RUN_ERROR.into());
        }
        let result = self.send_graphql(query, variables).await;

        if audit::is_mutation(query) {
            let operation = audit::operation_name(query);
            let outcome = match &result {
                Err(e) => Err(e.clone()),
                Ok(r) if r.status >= 400 => Err(format!("HTTP {}", r.status)),
                Ok(r) => match audit::response_errors(&r.body) {
                    errors if errors.is_empty() => Ok(()),
                    errors => Err(errors.join("; ")),
                },
            };
            let (action, summary) = match &audit {
                Some(meta) => (meta.action.clone(), meta.summary.clone()),
                None => ("mutation".to_string(), format!("Ran {}", operation.as_deref().unwrap_or("a mutation"))),
            };
            let details = json!({ "operation": operation, "variables": audit::redact(variables) });
            self.audit.append(&AuditEntry::new(&action, &summary, outcome, details));
        }

        result
    }

    async fn send_graphql(&self, query: &str, variables: &Value) -> Result<GraphQlResponse, String> {
        let key = self.current_key().await?.ok_or("No API key is configured. Add one in Settings.")?;
        let url = self.load_config().api_url;

        // Developer aid: NVOKE_LOG_REQUESTS=1 prints one line per API request.
        if std::env::var_os("NVOKE_LOG_REQUESTS").is_some() {
            let condensed = query.split_whitespace().collect::<Vec<_>>().join(" ");
            let op: String = condensed.chars().take(110).collect();
            eprintln!("[graphql] {op}");
        }

        let response = self
            .http
            .post(&url)
            .bearer_auth(key)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|e| format!("Request to {url} failed: {e}"))?;

        let status = response.status().as_u16();
        let text = response.text().await.map_err(|e| format!("Could not read the response: {e}"))?;
        let body = response_body(status, &text)?;
        Ok(GraphQlResponse { status, body })
    }

    pub fn audit_log(&self, limit: Option<usize>) -> Result<AuditLogView, String> {
        let path = self.audit.path().display().to_string();
        let entries = self.audit.read(limit.unwrap_or(500))?;
        Ok(AuditLogView { path, entries })
    }

    /// Records a change made outside `graphql` (for example by a UI-only action).
    pub fn record(&self, entry: &AuditEntry) {
        self.audit.append(entry);
    }

    // -----------------------------------------------------------------------
    // Encrypted local storage (job history, data cache): named blobs.
    // -----------------------------------------------------------------------

    /// The stored value, or None if there isn't one. Errors only when encryption
    /// is unavailable, so the caller can fall back to keeping data in memory.
    pub async fn store_get(&self, name: &str) -> Result<Option<String>, String> {
        let path = self.paths.store_file(name)?;
        let vault = self.vault.clone();
        blocking(move || {
            vault.status()?;
            let bytes = match fs::read(&path) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(format!("Couldn't read {}: {e}", path.display())),
            };
            match vault.open(&bytes).and_then(|p| String::from_utf8(p).map_err(|e| e.to_string())) {
                Ok(text) => Ok(Some(text)),
                Err(e) => {
                    // Wrong key (e.g. keychain reset) or a damaged file: keep it aside, start fresh.
                    let aside = vault::set_aside(&path);
                    eprintln!("{e}; moved to {}", aside.display());
                    Ok(None)
                }
            }
        })
        .await
    }

    pub async fn store_set(&self, name: &str, value: String) -> Result<(), String> {
        let path = self.paths.store_file(name)?;
        let vault = self.vault.clone();
        blocking(move || vault::write_private(&path, &vault.seal(value.as_bytes())?)).await
    }

    pub async fn store_remove(&self, name: &str) -> Result<(), String> {
        let path = self.paths.store_file(name)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Deletes all stored history and cached data (not the audit log or settings).
    pub async fn clear_local_data(&self) -> Result<(), String> {
        let result = match fs::remove_dir_all(self.paths.store_dir()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Couldn't clear local data: {e}")),
        };
        self.audit.append(&AuditEntry::new("data.clear", "Cleared run history and cached data", result.clone(), Value::Null));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{inputs::InputType, model::InputValue};

    /// A core that keeps its files in a throwaway directory, so tests never touch the real ones.
    fn core(name: &str) -> (Core, PathBuf) {
        let dir = std::env::temp_dir().join(format!("nvoke-core-test-{name}-{}", std::process::id()));
        let paths = Paths { config_dir: dir.join("config"), data_dir: dir.join("data") };
        (Core::new_in(paths, "test").unwrap(), dir)
    }

    #[test]
    fn json_bodies_are_parsed_whatever_the_status() {
        assert_eq!(response_body(200, r#"{"data":{"x":1}}"#).unwrap(), json!({ "data": { "x": 1 } }));
        assert_eq!(response_body(400, r#"{"errors":[{"message":"bad"}]}"#).unwrap()["errors"][0]["message"], "bad");
    }

    #[test]
    fn the_gateways_plain_text_refusal_becomes_an_access_denied_error() {
        let body = response_body(403, "denied by external authz, unauthorized: token is not active\n").unwrap();
        assert_eq!(body["errors"][0]["message"], "denied by external authz, unauthorized: token is not active");
        assert_eq!(response_body(401, "Unauthorized").unwrap()["errors"][0]["message"], "Unauthorized");
        // Through the usual interpretation it reads as access denied, not as a parse failure.
        let error = api::interpret(403, &body, false).unwrap_err();
        assert!(api::is_access_denied(&error), "{error}");
    }

    #[test]
    fn other_non_json_responses_are_still_reported_as_such() {
        let error = response_body(502, "<html>Bad gateway</html>").unwrap_err();
        assert!(error.starts_with("HTTP 502: the API returned a non-JSON response"), "{error}");
        assert!(response_body(200, "").is_err());
    }

    #[tokio::test]
    async fn in_dry_run_a_mutation_is_held_back_before_anything_is_sent() {
        let (core, dir) = core("held");
        core.set_dry_run(true);
        // There is no API key in this core: a real send would fail with "No API key", not with the dry-run error.
        let result = core.graphql("mutation CreateTag($input: X!) { staticTagCreate(input: $input) { item { id } } }", &json!({ "input": { "name": "Servers" } }), None).await;
        assert_eq!(result.err().as_deref(), Some(DRY_RUN_ERROR));
        let log = core.dry_run_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].operation.as_deref(), Some("CreateTag"));
        assert_eq!(log[0].variables["input"]["name"], "Servers");
        // Nothing happened, so nothing is recorded as a change.
        assert!(core.audit_log(None).unwrap().entries.is_empty(), "the audit log must not record a dry run");
        assert!(!dir.join("data").join("audit.jsonl").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn every_typed_mutation_is_covered_and_describes_itself() {
        let (core, dir) = core("typed");
        core.set_dry_run(true);
        let ids = vec!["t1".to_string()];
        let devices = vec!["a1".to_string(), "a2".to_string()];
        let results = vec![
            core.create_static_tag("o1", "Servers", "", "").await.err(),
            core.update_static_tag("t1", "Servers", "desc").await.err(),
            core.create_dynamic_tag("o1", "Win10", "", &json!({ "and": [] }), "").await.err(),
            core.update_dynamic_tag("t1", "Win10", "", &json!({ "and": [] })).await.err(),
            core.assign_static_tags(&ids, &devices, Some("Servers")).await.err(),
            core.unassign_static_tags(&ids, &devices, None).await.err(),
            core.delete_tags(&ids, None).await.err(),
        ];
        assert!(results.iter().all(|r| r.as_deref() == Some(DRY_RUN_ERROR)), "{results:?}");
        let summaries: Vec<String> = core.dry_run_log().into_iter().map(|e| e.summary).collect();
        assert_eq!(summaries.len(), 7);
        assert!(summaries.contains(&"Added 2 devices to Servers".to_string()), "{summaries:?}");
        assert!(summaries.contains(&"Deleted 1 tag".to_string()), "{summaries:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_held_back_run_shows_what_would_be_sent_but_never_a_password() {
        let (core, dir) = core("run");
        core.set_dry_run(true);
        let input = api::RunScriptInput {
            script_id: "s1".into(),
            script_name: "Cleanup".into(),
            target_label: "PatchGroupTwo".into(),
            asset_ids: vec!["a1".into(), "a2".into()],
            name: "Run Cleanup".into(),
            timeout_seconds: 120,
            inputs: vec![
                api::RunInput { name: "Mode".into(), variable: "mode".into(), kind: InputType::String, value: InputValue { string_value: Some("fast".into()), ..Default::default() } },
                api::RunInput { name: "Pass".into(), variable: "pw".into(), kind: InputType::Password, value: InputValue { string_value: Some("s3cret-value".into()), ..Default::default() } },
            ],
        };
        assert_eq!(core.run_script(&input).await.err().as_deref(), Some(DRY_RUN_ERROR));
        let entry = &core.dry_run_log()[0];
        assert_eq!(entry.action, "script.run");
        assert_eq!(entry.summary, "Ran Cleanup on 2 devices (PatchGroupTwo)");
        assert_eq!(entry.variables["input"]["assetIds"], json!(["a1", "a2"]));
        assert_eq!(entry.variables["input"]["configuration"]["scriptId"], "s1");
        let text = entry.variables.to_string();
        assert!(text.contains("fast") && text.contains("[redacted]"));
        assert!(!text.contains("s3cret-value"), "a password must never appear in the dry-run log: {text}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn only_mutations_are_held_back_so_browsing_still_works() {
        let (core, dir) = core("decision");
        core.set_dry_run(true);
        assert!(core.holds_back("mutation RunScript($input: X!) { a }"));
        assert!(core.holds_back("\n # note\n mutation { a }"));
        assert!(!core.holds_back("query FindScripts($orgIds: [ID!]!) { scriptSearch(inOrganizations: $orgIds) { nodes { id } } }"));
        assert!(!core.holds_back("{ asset(id: \"x\") { name } }"));
        core.set_dry_run(false);
        assert!(!core.holds_back("mutation RunScript($input: X!) { a }"), "off means off");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_log_keeps_only_the_most_recent_entries_and_dry_run_can_be_switched() {
        let (core, dir) = core("cap");
        assert!(!core.is_dry_run() || std::env::var_os("NVOKE_DRY_RUN").is_some());
        core.set_dry_run(true);
        assert!(core.is_dry_run());
        for i in 0..MAX_DRY_RUN_ENTRIES + 5 {
            core.hold_back("mutation M { x }", &json!({ "i": i }), None);
        }
        let log = core.dry_run_log();
        assert_eq!(log.len(), MAX_DRY_RUN_ENTRIES);
        assert_eq!(log[0].variables["i"], 5, "the oldest entries go first");
        let _ = fs::remove_dir_all(dir);
    }
}
