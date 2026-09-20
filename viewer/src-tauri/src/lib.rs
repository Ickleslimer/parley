#[cfg(not(windows))]
compile_error!("Parley Conversation Viewer is Windows-only");

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|_, _, _| {}))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_desktop_underlay::init())
        .run(tauri::generate_context!())
        .expect("failed to run Parley Conversation Viewer");
}
