//! Everything N-voke does that isn't a user interface: the API key (OS
//! keychain), settings, every GraphQL request, the audit log, and the encrypted
//! local store. The desktop app (Tauri) and the terminal app both sit on top of
//! this, and share the same keychain entries and data directories, so a run
//! started in one shows up in the other.

pub mod audit;
pub mod vault;

use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
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
}

impl Core {
    /// `app_version` goes into the User-Agent header.
    pub fn new(app_version: &str) -> Result<Self, String> {
        let paths = Paths::resolve()?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(format!("n-voke/{app_version}"))
            .build()
            .map_err(|e| format!("Could not build the HTTP client: {e}"))?;
        let vault = Arc::new(Vault::new());
        let audit = AuditLog::new(vault.clone(), paths.data_dir.clone());
        Ok(Self { paths, http, api_key: Mutex::new(None), vault, audit })
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
        let body = serde_json::from_str(&text).map_err(|_| {
            let snippet: String = text.chars().take(200).collect();
            format!("HTTP {status}: the API returned a non-JSON response: {snippet}")
        })?;
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
