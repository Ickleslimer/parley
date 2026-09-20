#[cfg(not(windows))]
compile_error!("Parley Conversation Viewer is Windows-only");

mod commands;
pub mod event_engine;
mod launch;
mod lifecycle;
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
            commands::get_widget_snapshot,
            commands::list_sessions,
            commands::list_exchanges,
            commands::search_events,
            commands::get_event_content,
            commands::get_settings,
            commands::save_settings,
            commands::list_monitors,
            commands::select_event_log,
            commands::set_event_log,
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
