use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Runtime, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use crate::event_engine::{
    EventContent, ExchangePage, SearchPage, SessionPage, WidgetBrowserSnapshot, WidgetSnapshot,
};
use crate::interactive_surface;
use crate::lifecycle;
use crate::peer_activity::{self, PeerActivitySnapshot};
use crate::peer_health::{self, HandoffSelection, PeerHealthSnapshot};
use crate::runtime::{AppState, MonitorInfo, ViewerStatus, WidgetSurfaceBoundsReport};
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
pub fn get_widget_browser<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
) -> Result<WidgetBrowserSnapshot, String> {
    require_widget_surface(&window)?;
    Ok(state.widget_browser.snapshot(&state.engine))
}

#[tauri::command]
pub fn widget_browse_older<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
) -> Result<WidgetBrowserSnapshot, String> {
    require_widget_surface(&window)?;
    let snapshot = state.widget_browser.older(&state.engine);
    record_surface_focus(&app, &state, "older", false);
    Ok(snapshot)
}

#[tauri::command]
pub fn widget_browse_newer<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
) -> Result<WidgetBrowserSnapshot, String> {
    require_widget_surface(&window)?;
    let snapshot = state.widget_browser.newer(&state.engine);
    record_surface_focus(&app, &state, "newer", false);
    Ok(snapshot)
}

#[tauri::command]
pub fn widget_browse_live<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
) -> Result<WidgetBrowserSnapshot, String> {
    require_widget_surface(&window)?;
    let snapshot = state.widget_browser.live(&state.engine);
    record_surface_focus(&app, &state, "live", false);
    Ok(snapshot)
}

#[tauri::command]
pub fn open_widget_exchange<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    require_widget_surface(&window)?;
    let selection = state.widget_browser.displayed_event(&state.engine)?;
    lifecycle::show_detail(&app)?;
    record_surface_focus(&app, &state, "open-transcript", true);
    app.emit_to(lifecycle::DETAIL_LABEL, "widget-open-exchange", selection)
        .map_err(|error| format!("failed to select the widget exchange in detail: {error}"))
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
    if let Err(error) = lifecycle::reconcile_interactive_mode(&app) {
        state.set_runtime_error(error);
    }
    Ok(saved)
}

#[tauri::command]
pub fn report_widget_surface_bounds<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
    report: WidgetSurfaceBoundsReport,
) -> Result<ViewerStatus, String> {
    if window.label() != lifecycle::WIDGET_LABEL {
        return Err("only the desktop underlay may report widget-surface bounds".to_string());
    }
    lifecycle::report_widget_surface_bounds(&app, report)?;
    Ok(state.status())
}

#[tauri::command]
pub fn widget_surface_ready<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
) -> Result<ViewerStatus, String> {
    if window.label() != crate::interactive_surface::SURFACE_LABEL {
        return Err("only the widget surface may report readiness".to_string());
    }
    lifecycle::widget_surface_ready(&app)?;
    Ok(state.status())
}

#[tauri::command]
pub fn retry_interactive_mode<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
) -> Result<ViewerStatus, String> {
    if window.label() != lifecycle::DETAIL_LABEL {
        return Err("interactive mode may be retried only from the detail window".to_string());
    }
    lifecycle::retry_interactive_mode(&app)?;
    Ok(state.status())
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

fn require_widget_surface<R: Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    if window.label() == interactive_surface::SURFACE_LABEL {
        Ok(())
    } else {
        Err("only the widget surface may browse desktop exchanges".to_string())
    }
}

fn record_surface_focus<R: Runtime>(
    app: &AppHandle<R>,
    state: &State<'_, AppState>,
    action: &str,
    intentional_focus: bool,
) {
    if let Err(error) = interactive_surface::record_focus_diagnostic(app, action, intentional_focus)
    {
        state.set_runtime_error(error);
    }
}
