//! Native side of the desktop app: a thin layer that exposes `nvoke-core` to the
//! web view as Tauri commands. The core owns the API key (OS keychain) and
//! performs every GraphQL request, so the token never reaches the web view and
//! requests aren't subject to browser CORS rules.

use nvoke_core::{
    AuditLogView, Core, GraphQlResponse, Settings,
    audit::AuditMeta,
    migration::{connect::ConnectReport, project::{Delivery, DestEcoverse}, store::ProjectSummary},
    ncentral::{EcoCredential, ServerView, model::ServerInfo},
};
use serde_json::Value;
use tauri::{Manager, State};

#[tauri::command]
async fn get_settings(core: State<'_, Core>) -> Result<Settings, String> {
    core.get_settings().await
}

#[tauri::command]
async fn save_settings(
    core: State<'_, Core>,
    api_url: String,
    api_key: Option<String>,
    history_days: Option<u32>,
) -> Result<Settings, String> {
    core.save_settings(api_url, api_key, history_days).await
}

#[tauri::command]
async fn clear_api_key(core: State<'_, Core>) -> Result<(), String> {
    core.clear_api_key().await
}

#[tauri::command]
async fn graphql(core: State<'_, Core>, query: String, variables: Value, audit: Option<AuditMeta>) -> Result<GraphQlResponse, String> {
    core.graphql(&query, &variables, audit).await
}

#[tauri::command]
async fn get_audit_log(core: State<'_, Core>, limit: Option<usize>) -> Result<AuditLogView, String> {
    core.audit_log(limit)
}

// Migration. Every type that crosses this boundary is generated into the web view (`npm run bindings`).

/// The saved N-central servers, without their JWTs.
#[tauri::command]
async fn servers_list(core: State<'_, Core>) -> Result<Vec<ServerView>, String> {
    core.ncentral_servers().await
}

/// Adds a server, or updates the one with `id`. The JWT, when given, goes to the keychain and never comes back.
#[tauri::command]
async fn server_save(
    core: State<'_, Core>,
    id: Option<String>,
    label: String,
    host: String,
    jwt: Option<String>,
    ecoverse_tenant: Option<String>,
) -> Result<ServerView, String> {
    core.save_ncentral_server(id, &label, &host, jwt, ecoverse_tenant).await
}

/// Chooses which Ecoverse key reaches a server's devices. A token, when given, goes to the keychain and never comes back.
#[tauri::command]
async fn server_ecoverse(core: State<'_, Core>, id: String, mode: EcoCredential, token: Option<String>) -> Result<ServerView, String> {
    core.set_server_ecoverse(&id, mode, token).await
}

/// Signs in to a saved server and reads its version.
#[tauri::command]
async fn server_test(core: State<'_, Core>, id: String) -> Result<ServerInfo, String> {
    core.ncentral_server_info(&id).await.map_err(String::from)
}

/// Removes a server and its JWT, unless a project uses it.
#[tauri::command]
async fn server_remove(core: State<'_, Core>, id: String) -> Result<(), String> {
    if let Some(project) = core.migration_project_using_server(&id).await? {
        let label = core.ncentral_servers().await?.into_iter().find(|s| s.id == id).map_or(id.clone(), |s| s.label);
        return Err(format!("{label} is used by the project {project}: delete that project first."));
    }
    core.remove_ncentral_server(&id).await
}

/// Every migration project, most recently changed first.
#[tauri::command]
async fn projects_list(core: State<'_, Core>) -> Result<Vec<ProjectSummary>, String> {
    core.migration_list().await
}

#[tauri::command]
async fn project_summary(core: State<'_, Core>, id: String) -> Result<ProjectSummary, String> {
    core.migration_summary(&id).await
}

#[tauri::command]
async fn project_create(
    core: State<'_, Core>,
    name: String,
    source_server_id: String,
    dest_server_id: String,
    dest_ecoverse: DestEcoverse,
    source_ecoverse_orgs: Vec<String>,
    delivery: Delivery,
) -> Result<ProjectSummary, String> {
    let mut project = core.migration_create_full(&name, &source_server_id, &dest_server_id, dest_ecoverse, source_ecoverse_orgs, vec![]).await?;
    if project.options.delivery != delivery {
        project.options.delivery = delivery;
        core.migration_save(&mut project).await?;
    }
    Ok(ProjectSummary::of(&project))
}

#[tauri::command]
async fn project_delete(core: State<'_, Core>, id: String) -> Result<(), String> {
    core.migration_delete(&id).await
}

/// The Connect checklist: both servers and Ecoverse, read now.
#[tauri::command]
async fn project_connect_check(core: State<'_, Core>, id: String) -> Result<ConnectReport, String> {
    core.migration_connect_check(&id).await
}

// The web view keeps nothing on disk itself; it reads and writes named
// encrypted blobs through these.

#[tauri::command]
async fn store_get(core: State<'_, Core>, name: String) -> Result<Option<String>, String> {
    core.store_get(&name).await
}

#[tauri::command]
async fn store_set(core: State<'_, Core>, name: String, value: String) -> Result<(), String> {
    core.store_set(&name, value).await
}

#[tauri::command]
async fn store_remove(core: State<'_, Core>, name: String) -> Result<(), String> {
    core.store_remove(&name).await
}

#[tauri::command]
async fn clear_local_data(core: State<'_, Core>) -> Result<(), String> {
    core.clear_local_data().await
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

    let core = Core::new(env!("CARGO_PKG_VERSION")).expect("failed to initialise the N-voke core");

    tauri::Builder::default()
        .manage(core)
        .setup(|app| {
            // Convert any plain-JSON audit lines from older versions, off the main thread.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || handle.state::<Core>().encrypt_existing_audit());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            clear_api_key,
            graphql,
            get_audit_log,
            servers_list,
            server_save,
            server_ecoverse,
            server_test,
            server_remove,
            projects_list,
            project_summary,
            project_create,
            project_delete,
            project_connect_check,
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
