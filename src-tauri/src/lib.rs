//! Native side of the app: owns the API key (OS keychain) and performs every
//! GraphQL request, so the token never reaches the web view and requests
//! aren't subject to browser CORS rules.

mod audit;
mod vault;

use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use audit::{AuditEntry, AuditLog, AuditMeta};
use vault::Vault;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager, State};

const KEYRING_SERVICE: &str = "com.fraziersystems.nable-script-runner";
const KEYRING_USER: &str = "api-key";
const DEFAULT_API_URL: &str = "https://api.n-able.com/graphql";

/// Non-secret settings, persisted as JSON in the app's config directory.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Config {
    api_url: String,
    /// How long run history is kept on disk, in days; 0 = don't save it at all.
    #[serde(default = "default_history_days")]
    history_days: u32,
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
struct Settings {
    api_url: String,
    has_api_key: bool,
    history_days: u32,
    /// Whether local data can be encrypted (needs the system keychain), and why not if not.
    encryption_available: bool,
    encryption_error: Option<String>,
}

#[derive(Serialize)]
struct GraphQlResponse {
    status: u16,
    body: Value,
}

struct AppState {
    http: reqwest::Client,
    /// Key read from the keychain once, so polling doesn't hit the keychain
    /// (and possibly an unlock prompt) every few seconds.
    api_key: Mutex<Option<String>>,
    vault: Arc<Vault>,
    audit: AuditLog,
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| format!("No config directory: {e}"))?;
    Ok(dir.join("config.json"))
}

fn load_config(app: &AppHandle) -> Config {
    config_path(app)
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(app: &AppHandle, config: &Config) -> Result<(), String> {
    let path = config_path(app)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

fn keyring_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| {
        format!("The system keychain is unavailable ({e}). On Linux, make sure a Secret Service provider such as GNOME Keyring or KWallet is running.")
    })
}

/// Keychain calls can block (D-Bus round trips, unlock prompts), so they run
/// off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

fn read_stored_key() -> Result<Option<String>, String> {
    match keyring_entry()?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Could not read the API key from the keychain: {e}")),
    }
}

async fn current_key(state: &State<'_, AppState>) -> Result<Option<String>, String> {
    if let Some(key) = state.api_key.lock().unwrap().clone() {
        return Ok(Some(key));
    }
    let key = blocking(read_stored_key).await?;
    *state.api_key.lock().unwrap() = key.clone();
    Ok(key)
}

#[tauri::command]
async fn get_settings(app: AppHandle, state: State<'_, AppState>) -> Result<Settings, String> {
    let has_api_key = current_key(&state).await?.is_some();
    let vault = state.vault.clone();
    let encryption = blocking(move || Ok(vault.status())).await?;
    let config = load_config(&app);
    Ok(Settings {
        api_url: config.api_url,
        has_api_key,
        history_days: config.history_days,
        encryption_available: encryption.is_ok(),
        encryption_error: encryption.err(),
    })
}

#[tauri::command]
async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    api_url: String,
    api_key: Option<String>,
    history_days: Option<u32>,
) -> Result<Settings, String> {
    let api_url = api_url.trim().to_string();
    let parsed = reqwest::Url::parse(&api_url).map_err(|e| format!("Invalid API URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("The API URL must use https://".into());
    }
    let previous = load_config(&app);
    let previous_url = previous.api_url.clone();
    let history_days = history_days.unwrap_or(previous.history_days).min(3650);
    save_config(&app, &Config { api_url: api_url.clone(), history_days })?;
    if previous.history_days != history_days {
        let summary = if history_days == 0 { "Stopped saving run history".to_string() } else { format!("Keep run history for {history_days} days") };
        let details = json!({ "from": previous.history_days, "to": history_days });
        state.audit.append(&app, &AuditEntry::new("settings.history", &summary, Ok(()), details));
    }
    if previous_url != api_url {
        let entry = AuditEntry::new(
            "settings.endpoint",
            &format!("Changed the API endpoint to {api_url}"),
            Ok(()),
            json!({ "from": previous_url, "to": api_url }),
        );
        state.audit.append(&app, &entry);
    }

    if let Some(key) = api_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
        let had_key = current_key(&state).await.ok().flatten().is_some();
        let to_store = key.clone();
        let result = blocking(move || {
            keyring_entry()?
                .set_password(&to_store)
                .map_err(|e| format!("Could not save the API key to the keychain: {e}"))
        })
        .await;
        let summary = if had_key { "Replaced the saved API key" } else { "Saved an API key" };
        state.audit.append(&app, &AuditEntry::new("settings.api_key", summary, result.clone(), Value::Null));
        result?;
        *state.api_key.lock().unwrap() = Some(key);
    }

    get_settings(app, state).await
}

#[tauri::command]
async fn clear_api_key(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let result = blocking(|| match keyring_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Could not remove the API key from the keychain: {e}")),
    })
    .await;
    state.audit.append(&app, &AuditEntry::new("settings.api_key", "Removed the saved API key", result.clone(), Value::Null));
    result?;
    *state.api_key.lock().unwrap() = None;
    Ok(())
}

/// Sends one GraphQL operation and returns the HTTP status with the parsed
/// body. GraphQL-level `errors` are left for the caller to interpret, since
/// some queries legitimately return partial data.
///
/// Every mutation is written to the audit log here, whether or not the UI
/// supplied `audit` context, so nothing that changes N-able goes unrecorded.
#[tauri::command]
async fn graphql(
    app: AppHandle,
    state: State<'_, AppState>,
    query: String,
    variables: Value,
    audit: Option<AuditMeta>,
) -> Result<GraphQlResponse, String> {
    let result = send_graphql(&app, &state, &query, &variables).await;

    if audit::is_mutation(&query) {
        let operation = audit::operation_name(&query);
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
        let details = json!({ "operation": operation, "variables": audit::redact(&variables) });
        state.audit.append(&app, &AuditEntry::new(&action, &summary, outcome, details));
    }

    result
}

async fn send_graphql(app: &AppHandle, state: &State<'_, AppState>, query: &str, variables: &Value) -> Result<GraphQlResponse, String> {
    let key = current_key(state)
        .await?
        .ok_or("No API key is configured. Add one in Settings.")?;
    let url = load_config(app).api_url;

    // Developer aid: NVOKE_LOG_REQUESTS=1 prints one line per API request.
    if std::env::var_os("NVOKE_LOG_REQUESTS").is_some() {
        let condensed = query.split_whitespace().collect::<Vec<_>>().join(" ");
        let op: String = condensed.chars().take(110).collect();
        eprintln!("[graphql] {op}");
    }

    let response = state
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditLogView {
    path: String,
    entries: Vec<AuditEntry>,
}

#[tauri::command]
async fn get_audit_log(app: AppHandle, state: State<'_, AppState>, limit: Option<usize>) -> Result<AuditLogView, String> {
    let path = AuditLog::path(&app)?.display().to_string();
    let entries = state.audit.read(&app, limit.unwrap_or(500))?;
    Ok(AuditLogView { path, entries })
}

// ---------------------------------------------------------------------------
// Encrypted local storage (job history, data cache). The web view keeps
// nothing on disk itself; it reads and writes named blobs through these.
// ---------------------------------------------------------------------------

fn store_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| format!("No data directory: {e}"))?.join("store"))
}

fn store_path(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    vault::validate_name(name)?;
    Ok(store_dir(app)?.join(format!("{name}.enc")))
}

/// The stored value, or None if there isn't one. Errors only when encryption
/// is unavailable, so the caller can fall back to keeping data in memory.
#[tauri::command]
async fn store_get(app: AppHandle, state: State<'_, AppState>, name: String) -> Result<Option<String>, String> {
    let path = store_path(&app, &name)?;
    let vault = state.vault.clone();
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

#[tauri::command]
async fn store_set(app: AppHandle, state: State<'_, AppState>, name: String, value: String) -> Result<(), String> {
    let path = store_path(&app, &name)?;
    let vault = state.vault.clone();
    blocking(move || vault::write_private(&path, &vault.seal(value.as_bytes())?)).await
}

#[tauri::command]
async fn store_remove(app: AppHandle, name: String) -> Result<(), String> {
    let path = store_path(&app, &name)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Deletes all stored history and cached data (not the audit log or settings).
#[tauri::command]
async fn clear_local_data(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let dir = store_dir(&app)?;
    let result = match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Couldn't clear local data: {e}")),
    };
    state.audit.append(&app, &AuditEntry::new("data.clear", "Cleared run history and cached data", result.clone(), Value::Null));
    result
}

/// Wipes the web view's own storage (localStorage etc.). Earlier versions kept
/// data there in plain text; once it has been moved into the encrypted store,
/// this removes the old copy, including anything left in free database pages.
#[tauri::command]
fn purge_webview_storage(window: tauri::WebviewWindow) -> Result<(), String> {
    window.clear_all_browsing_data().map_err(|e| e.to_string())
}

/// WebKitGTK's DMA-BUF renderer crashes on NVIDIA Linux hosts under Wayland
/// ("Error 71 (Protocol error) dispatching to Wayland display").
fn should_disable_dmabuf(is_linux: bool, has_nvidia: bool, is_user_configured: bool) -> bool {
    is_linux && has_nvidia && !is_user_configured
}

fn configure_graphics_before_webview() {
    if should_disable_dmabuf(
        cfg!(target_os = "linux"),
        std::path::Path::new("/proc/driver/nvidia").exists(),
        std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some(),
    ) {
        // Runs before Tauri creates threads or a webview. A user-provided value is kept intact.
        unsafe {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    configure_graphics_before_webview();

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("n-voke/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("failed to build HTTP client");

    tauri::Builder::default()
        .manage({
            let vault = Arc::new(Vault::new());
            AppState { http, api_key: Mutex::new(None), audit: AuditLog::new(vault.clone()), vault }
        })
        .setup(|app| {
            // Convert any plain-JSON audit lines from older versions, off the main thread.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || handle.state::<AppState>().audit.encrypt_existing(&handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            clear_api_key,
            graphql,
            get_audit_log,
            store_get,
            store_set,
            store_remove,
            clear_local_data,
            purge_webview_storage
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::should_disable_dmabuf;

    #[test]
    fn disables_dmabuf_only_for_unconfigured_linux_nvidia_hosts() {
        assert!(should_disable_dmabuf(true, true, false));
        assert!(!should_disable_dmabuf(true, true, true));
        assert!(!should_disable_dmabuf(true, false, false));
        assert!(!should_disable_dmabuf(false, true, false));
    }
}
