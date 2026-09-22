use std::path::PathBuf;

use tauri::{AppHandle, Runtime, State};
use tauri_plugin_dialog::DialogExt;

use crate::event_engine::{EventContent, ExchangePage, SearchPage, SessionPage, WidgetSnapshot};
use crate::lifecycle;
use crate::peer_activity::{self, PeerActivitySnapshot};
use crate::peer_health::{self, HandoffSelection, PeerHealthSnapshot};
use crate::runtime::{AppState, MonitorInfo, ViewerStatus};
use crate::settings::ViewerSettings;

#[tauri::command]
pub fn get_viewer_status(state: State<'_, AppState>) -> ViewerStatus {
    state.status()
}

#[tauri::command]
pub fn get_widget_snapshot(state: State<'_, AppState>) -> WidgetSnapshot {
    state.engine.widget_snapshot()
}

#[tauri::command]
pub fn list_sessions(state: State<'_, AppState>, cursor: Option<u64>, limit: usize) -> SessionPage {
    state.engine.session_page(cursor, limit)
}

#[tauri::command]
pub fn list_exchanges(
    state: State<'_, AppState>,
    session_key: String,
    cursor: Option<u64>,
    limit: usize,
) -> ExchangePage {
    state.engine.exchange_page(&session_key, cursor, limit)
}

#[tauri::command]
pub fn search_events(
    state: State<'_, AppState>,
    query: String,
    cursor: Option<u64>,
    limit: usize,
) -> SearchPage {
    state.engine.search(&query, cursor, limit)
}

#[tauri::command]
pub fn get_event_content(state: State<'_, AppState>, event_key: String) -> Option<EventContent> {
    state.engine.event_content(&event_key)
}

#[tauri::command]
pub fn get_peer_health() -> PeerHealthSnapshot {
    peer_health::snapshot()
}

#[tauri::command]
pub fn acknowledge_peer_incident(incident_id: String) -> Result<PeerHealthSnapshot, String> {
    peer_health::acknowledge(&incident_id)
}

#[tauri::command]
pub fn set_peer_health_muted(muted: bool) -> Result<PeerHealthSnapshot, String> {
    peer_health::set_muted(muted)
}

#[tauri::command]
pub fn test_peer_health_chime() -> Result<(), String> {
    peer_health::test_chime()
}

#[tauri::command]
pub fn open_latest_handoff(state: State<'_, AppState>) -> HandoffSelection {
    peer_health::open_latest_handoff(&state.engine)
}

#[tauri::command]
pub fn get_peer_activity() -> PeerActivitySnapshot {
    peer_activity::snapshot()
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> ViewerSettings {
    state.settings().viewer
}

#[tauri::command]
pub fn save_settings<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    settings: ViewerSettings,
) -> Result<ViewerSettings, String> {
    let saved = state.save_placement(settings)?;
    if let Err(error) = lifecycle::apply_widget_placement(&app) {
        state.set_runtime_error(error);
    }
    Ok(saved)
}

#[tauri::command]
pub fn list_monitors<R: Runtime>(app: AppHandle<R>) -> Result<Vec<MonitorInfo>, String> {
    lifecycle::list_monitors(&app)
}

#[tauri::command]
pub async fn select_event_log<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> Result<ViewerStatus, String> {
    let picker_app = app.clone();
    let selection = tauri::async_runtime::spawn_blocking(move || {
        picker_app
            .dialog()
            .file()
            .set_title("Select Parley event log")
            .add_filter("JSON Lines", &["jsonl"])
            .blocking_pick_file()
    })
    .await
    .map_err(|error| format!("event-log picker failed: {error}"))?;
    if let Some(selection) = selection {
        let path = selection
            .into_path()
            .map_err(|error| format!("selected event log is not a filesystem path: {error}"))?;
        state.add_source(path, true)?;
    }
    Ok(state.status())
}

#[tauri::command]
pub fn set_event_log(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<ViewerStatus, String> {
    state.set_sources(path.map(PathBuf::from).into_iter().collect(), true)?;
    Ok(state.status())
}

#[tauri::command]
pub fn set_event_logs(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<ViewerStatus, String> {
    state.set_sources(paths.into_iter().map(PathBuf::from).collect(), true)?;
    Ok(state.status())
}

#[tauri::command]
pub fn add_event_log(state: State<'_, AppState>, path: String) -> Result<ViewerStatus, String> {
    state.add_source(PathBuf::from(path), true)?;
    Ok(state.status())
}

#[tauri::command]
pub fn remove_event_log(state: State<'_, AppState>, path: String) -> Result<ViewerStatus, String> {
    state.remove_source(PathBuf::from(path), true)?;
    Ok(state.status())
}

#[tauri::command]
pub fn set_widget_visible<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    visible: bool,
) -> Result<ViewerStatus, String> {
    lifecycle::request_widget(&app, visible)?;
    Ok(state.status())
}

#[tauri::command]
pub fn set_launch_at_login<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
) -> Result<ViewerSettings, String> {
    lifecycle::set_launch_at_login(&app, enabled)
}

#[tauri::command]
pub fn show_detail<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    lifecycle::show_detail(&app)
}

#[tauri::command]
pub fn exit_app<R: Runtime>(app: AppHandle<R>) {
    lifecycle::exit_app(&app);
}
