#[cfg(not(windows))]
compile_error!("Parley Conversation Viewer is Windows-only");

mod commands;
pub mod event_engine;
mod interactive_surface;
mod launch;
mod lifecycle;
mod peer_activity;
mod peer_health;
mod runtime;
mod settings;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, args, _working_directory| lifecycle::handle_second_instance(app, args),
        ))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .arg("--autostart")
                .app_name("Parley Conversation Viewer")
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_desktop_underlay::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_viewer_status,
            commands::get_widget_feed,
            commands::get_widget_message,
            commands::open_widget_event,
            commands::list_sessions,
            commands::list_exchanges,
            commands::get_conversation_page,
            commands::search_events,
            commands::get_event_content,
            commands::get_peer_health,
            commands::acknowledge_peer_incident,
            commands::set_peer_health_muted,
            commands::test_peer_health_chime,
            commands::open_latest_handoff,
            commands::get_peer_activity,
            commands::get_settings,
            commands::save_settings,
            commands::report_widget_surface_bounds,
            commands::widget_surface_ready,
            commands::report_widget_surface_activity,
            commands::widget_surface_pointer_down,
            commands::retry_interactive_mode,
            commands::list_monitors,
            commands::select_event_log,
            commands::set_event_log,
            commands::set_event_logs,
            commands::add_event_log,
            commands::remove_event_log,
            commands::set_widget_visible,
            commands::set_launch_at_login,
            commands::show_detail,
            commands::exit_app,
        ])
        .on_window_event(lifecycle::handle_window_event)
        .setup(lifecycle::setup_app)
        .run(tauri::generate_context!())
        .expect("failed to run Parley Conversation Viewer");
}
