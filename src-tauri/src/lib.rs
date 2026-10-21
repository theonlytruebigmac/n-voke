//! Native side of the desktop app: a thin layer that exposes `nvoke-core` to the
//! web view as Tauri commands. The core owns the API key (OS keychain) and
//! performs every GraphQL request, so the token never reaches the web view and
//! requests aren't subject to browser CORS rules.

use nvoke_core::{
    AuditLogView, Core, GraphQlResponse, Settings,
    audit::AuditMeta,
    migration::{
        connect::ConnectReport,
        mapping::Override,
        finish::{CleanupReport, FinishView, ReportFormat},
        retire::{RetireReport, RetireView},
        structure::{StructurePreview, StructureReport},
        launchview::{CandidatePage, CandidateQuery, LaunchPreview, LaunchResult, LaunchView},
        mapview::{MapDetails, MapView},
        planview::{DeviceChoice, PlanAdded, PlanQuery, PlanView},
        prepare::{LinkChoice, PrepareView, ScriptSide},
        project::{Delivery, DestEcoverse, ProjectOptions, ScriptOrg},
        store::ProjectSummary,
        settings::MigrationSettings,
        trackview::{DeviceCommand, DeviceDetail, PollDigest, TrackQuery, TrackView},
        workflow::TokenReport,
    },
    ncentral::{EcoCredential, ServerView, model::ServerInfo},
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use tauri::{AppHandle, Emitter, Manager, State};

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

/// Destroys a project's saved data. Refused while another window has the project open; this window's tracker is stopped
/// and its work finished first, so nothing saves the project again afterwards.
#[tauri::command]
async fn project_delete(core: State<'_, Core>, trackers: State<'_, Trackers>, locks: State<'_, ProjectLocks>, id: String) -> Result<(), String> {
    trackers.stop(&id);
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    if !core.migration_lease(&id, &instance()).await? {
        return Err("This project is open in another N-voke window. Close it there first.".into());
    }
    core.migration_delete(&id).await
}

/// This window's name for a project's lease, so two windows (or the terminal app) don't change one project at once.
fn instance() -> String {
    format!("app-{}", std::process::id())
}

/// The running trackers, one per open project, so they can be stopped.
#[derive(Default)]
struct Trackers(Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>);

/// One lock per project. Every command that changes a project, and each tracker pass, holds it, so a long action (a
/// probe waits up to two minutes for the device) can't have its save refused because the tracker saved in between.
#[derive(Default)]
struct ProjectLocks(Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>);

impl ProjectLocks {
    fn get(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.0.lock().unwrap().entry(id.to_string()).or_default().clone()
    }
}

/// Projects whose bulk cleanup has been asked to stop.
#[derive(Default)]
struct CleanupStops(Mutex<HashSet<String>>);

impl Trackers {
    fn stop(&self, id: &str) {
        if let Some(handle) = self.0.lock().unwrap().remove(id) {
            handle.abort();
        }
    }
}

/// Lets go of the project's lease when its workspace closes, and stops its tracker.
#[tauri::command]
async fn project_close(core: State<'_, Core>, trackers: State<'_, Trackers>, id: String) -> Result<(), String> {
    trackers.stop(&id);
    core.migration_release_lease(&id, &instance()).await
}

/// Reads the project's devices now and then at the interval in the migration settings (a minute by default), saving what moved, and tells the page after each pass
/// with the `migration-polled` event. It runs only while the project is open.
#[tauri::command]
fn tracker_start(app: AppHandle, trackers: State<'_, Trackers>, id: String) {
    trackers.stop(&id);
    let task_id = id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        loop {
            let core = app.state::<Core>();
            let lock = app.state::<ProjectLocks>().get(&task_id);
            let held = lock.lock().await;
            let pass = core.migration_poll_digest(&task_id, &instance()).await;
            drop(held);
            let payload = match pass {
                Ok(digest) => serde_json::json!({ "id": task_id, "digest": digest, "error": null }),
                Err(error) => serde_json::json!({ "id": task_id, "digest": null, "error": error }),
            };
            let _ = app.emit("migration-polled", payload);
            tokio::time::sleep(Duration::from_secs(u64::from(core.migration_settings().poll_seconds))).await;
        }
    });
    trackers.0.lock().unwrap().insert(id, handle);
}

#[tauri::command]
fn tracker_stop(trackers: State<'_, Trackers>, id: String) {
    trackers.stop(&id);
}

/// One pass now.
#[tauri::command]
async fn project_poll(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String) -> Result<PollDigest, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_poll_digest(&id, &instance()).await
}

/// Drops what the app remembers of both servers (customers, devices, tokens), so the next read goes to them.
#[tauri::command]
async fn project_forget_reads(core: State<'_, Core>, id: String) -> Result<(), String> {
    core.migration_forget_reads(&id).await
}

/// One page of the source's devices as the plan sees them. Reads both servers when `refresh` is set or the last read is
/// old, so that takes a moment.
#[tauri::command]
async fn project_plan(core: State<'_, Core>, id: String, refresh: bool, query: PlanQuery) -> Result<PlanView, String> {
    core.migration_plan_view(&id, refresh, &query).await
}

/// Picks the path for one device, leaves it out, or puts it back.
#[tauri::command]
async fn project_device_choice(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, device_id: String, choice: DeviceChoice) -> Result<String, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_device_choice(&id, &instance(), &device_id, choice).await
}

/// Says whether the project's destination is linked to Ecoverse, which decides whether moved devices are checked there.
#[tauri::command]
async fn project_ecoverse_link(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, choice: LinkChoice) -> Result<(), String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_set_ecoverse_link(&id, &instance(), choice).await
}

#[tauri::command]
fn migration_settings_get(core: State<'_, Core>) -> MigrationSettings {
    core.migration_settings()
}

#[tauri::command]
fn migration_settings_save(core: State<'_, Core>, settings: MigrationSettings) -> Result<MigrationSettings, String> {
    core.save_migration_settings(settings)
}

/// Adds the ready devices to the project.
#[tauri::command]
async fn project_plan_add(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String) -> Result<PlanAdded, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_plan_add(&id, &instance()).await
}

#[tauri::command]
async fn project_launch(core: State<'_, Core>, id: String) -> Result<LaunchView, String> {
    core.migration_launch_view(&id).await
}

#[tauri::command]
async fn project_wave_create(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, label: String, max: usize) -> Result<String, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_wave_create(&id, &instance(), &label, max).await
}

/// The devices that could still go into a new wave, a page at a time.
#[tauri::command]
async fn project_wave_candidates(core: State<'_, Core>, id: String, query: CandidateQuery) -> Result<CandidatePage, String> {
    core.migration_wave_candidates(&id, &query).await
}

/// A wave of exactly the devices chosen.
#[tauri::command]
async fn project_wave_create_from(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, label: String, device_ids: Vec<String>) -> Result<String, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_wave_create_from(&id, &instance(), &label, device_ids).await
}

#[tauri::command]
async fn project_wave_preview(core: State<'_, Core>, id: String, wave: u32) -> Result<LaunchPreview, String> {
    core.migration_launch_preview(&id, wave).await
}

/// Launches a wave. With `dry_run` nothing is sent and the requests that would have been are returned.
#[tauri::command]
async fn project_wave_launch(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, wave: u32, dry_run: bool) -> Result<LaunchResult, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_launch_saved(&id, &instance(), wave, dry_run).await
}

#[tauri::command]
async fn project_track(core: State<'_, Core>, id: String, query: TrackQuery) -> Result<TrackView, String> {
    core.migration_track_view(&id, &query).await
}

#[tauri::command]
async fn project_device(core: State<'_, Core>, id: String, index: usize) -> Result<DeviceDetail, String> {
    core.migration_device_detail(&id, index).await
}

/// Does what the operator asked for one device, and says what happened.
#[tauri::command]
async fn project_device_command(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, index: usize, command: DeviceCommand) -> Result<String, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_device_command(&id, &instance(), index, command).await
}

#[tauri::command]
async fn project_finish(core: State<'_, Core>, id: String) -> Result<FinishView, String> {
    core.migration_finish_view(&id).await
}

/// Saves a report into the Downloads folder and returns where it went.
#[tauri::command]
async fn project_report(core: State<'_, Core>, id: String, format: ReportFormat) -> Result<String, String> {
    core.migration_finish_report(&id, format).await
}

/// Runs Cleanup on every healthy device that is fully working (or only on the `only` source device IDs), telling the page after each step with the
/// `migration-cleanup` event. Take a long time on many devices, so it can be stopped with `project_cleanup_cancel`.
#[tauri::command]
async fn project_cleanup(app: AppHandle, core: State<'_, Core>, locks: State<'_, ProjectLocks>, stops: State<'_, CleanupStops>, id: String, only: Option<Vec<String>>) -> Result<CleanupReport, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    stops.0.lock().unwrap().remove(&id);
    let result = core
        .migration_cleanup_healthy_with(
            &id,
            &instance(),
            only.as_deref(),
            &|progress| {
                let _ = app.emit("migration-cleanup", serde_json::json!({ "id": id, "progress": progress }));
            },
            &|| stops.0.lock().unwrap().contains(&id),
        )
        .await;
    stops.0.lock().unwrap().remove(&id);
    result
}

/// The customers and sites the destination is missing, and where each would go. Nothing is written.
#[tauri::command]
async fn project_structure_preview(core: State<'_, Core>, id: String, only: Option<Vec<String>>) -> Result<StructurePreview, String> {
    core.migration_structure_preview(&id, only).await
}

/// Creates the missing customers and sites on the destination, then their registration tokens. With `dry_run` nothing is sent.
#[tauri::command]
async fn project_structure_create(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, service_org_id: String, only: Option<Vec<String>>, dry_run: bool) -> Result<StructureReport, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_structure_create(&id, &instance(), &service_org_id, only, dry_run).await
}

/// What retiring the old source records would delete, and why anything isn't offered yet.
#[tauri::command]
async fn project_retire_view(core: State<'_, Core>, id: String) -> Result<RetireView, String> {
    core.migration_retire_view(&id).await
}

/// Deletes the old source records of the cleaned-up devices whose review period is over. With `dry_run` nothing is sent.
#[tauri::command]
async fn project_retire(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, dry_run: bool) -> Result<RetireReport, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_retire_sources(&id, &instance(), dry_run).await
}

/// Asks a running bulk cleanup to stop after the device it is on.
#[tauri::command]
fn project_cleanup_cancel(stops: State<'_, CleanupStops>, id: String) {
    stops.0.lock().unwrap().insert(id);
}

/// The Map step's rows: both servers' customers and sites, matched.
#[tauri::command]
async fn project_map(core: State<'_, Core>, id: String) -> Result<MapView, String> {
    core.migration_map_view(&id).await
}

/// Device counts and registration-token states for the Map step. Slower than the rows, so asked for separately.
#[tauri::command]
async fn project_map_details(core: State<'_, Core>, id: String) -> Result<MapDetails, String> {
    core.migration_map_details(&id).await
}

#[tauri::command]
async fn project_map_set(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, source_id: String, decision: Override) -> Result<(), String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_map_set(&id, &instance(), &source_id, decision).await
}

#[tauri::command]
async fn project_map_clear(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, source_id: String) -> Result<(), String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_map_clear(&id, &instance(), &source_id).await
}

/// Creates a registration token for every destination unit that has none. Writes to the destination server.
#[tauri::command]
async fn project_tokens_generate(core: State<'_, Core>, id: String) -> Result<TokenReport, String> {
    let project = core.migration_load(&id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
    core.migration_generate_tokens(&project, None).await
}

/// Where the endpoint script stands, and the project's options.
#[tauri::command]
async fn project_prepare(core: State<'_, Core>, id: String) -> Result<PrepareView, String> {
    core.migration_prepare_view(&id).await
}

/// Ecoverse organizations (on the source server's key) to publish the script in.
#[tauri::command]
async fn project_script_orgs(core: State<'_, Core>, id: String, side: ScriptSide, term: String) -> Result<Vec<ScriptOrg>, String> {
    core.migration_search_script_orgs(&id, side, &term).await
}

/// The organization to publish in when nobody has chosen one: the tenant's single service organization, if there is one.
#[tauri::command]
async fn project_default_org(core: State<'_, Core>, id: String, side: ScriptSide) -> Result<Option<ScriptOrg>, String> {
    core.migration_default_script_org_for(&id, side).await
}

#[tauri::command]
async fn project_publish(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, side: ScriptSide, org: ScriptOrg) -> Result<String, String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_prepare_publish(&id, &instance(), side, org).await
}

/// Saves the script into the Downloads folder and returns where it went.
#[tauri::command]
async fn project_export_script(core: State<'_, Core>) -> Result<String, String> {
    core.migration_prepare_export().map(|p| p.display().to_string())
}

#[tauri::command]
async fn project_rest_script(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, side: ScriptSide, item_id: String, version: String) -> Result<(), String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_prepare_rest_script(&id, &instance(), side, &item_id, &version).await
}

#[tauri::command]
async fn project_options(core: State<'_, Core>, locks: State<'_, ProjectLocks>, id: String, options: ProjectOptions) -> Result<(), String> {
    let lock = locks.get(&id);
    let _held = lock.lock().await;
    core.migration_prepare_options(&id, &instance(), options).await
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
        .manage(Trackers::default())
        .manage(ProjectLocks::default())
        .manage(CleanupStops::default())
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
            project_close,
            tracker_start,
            tracker_stop,
            project_poll,
            project_plan,
            project_forget_reads,
            project_device_choice,
            project_wave_candidates,
            project_wave_create_from,
            migration_settings_get,
            migration_settings_save,
            project_plan_add,
            project_launch,
            project_wave_create,
            project_wave_preview,
            project_wave_launch,
            project_track,
            project_device,
            project_device_command,
            project_finish,
            project_report,
            project_cleanup,
            project_cleanup_cancel,
            project_structure_preview,
            project_structure_create,
            project_retire_view,
            project_retire,
            project_map,
            project_map_details,
            project_map_set,
            project_map_clear,
            project_tokens_generate,
            project_prepare,
            project_script_orgs,
            project_default_org,
            project_publish,
            project_export_script,
            project_rest_script,
            project_options,
            project_ecoverse_link,
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
