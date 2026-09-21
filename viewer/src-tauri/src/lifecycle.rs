use std::env;
use std::ffi::OsString;
use std::thread;
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{
    App, AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow,
    Window, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_desktop_underlay::DesktopUnderlayExt;
use tauri_plugin_dialog::DialogExt;
use windows_sys::Win32::Foundation::{GetLastError, SetLastError, ERROR_SUCCESS, HWND};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GetClassNameW, GetParent, GetWindowLongPtrW, IsWindow,
    SendMessageTimeoutW, SetParent, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, HWND_BOTTOM,
    SMTO_NORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CHILD,
    WS_POPUP,
};

use crate::launch::{resolve_initial_sources, LaunchOptions, SourceOrigin};
use crate::peer_health;
use crate::runtime::{
    calculate_placement, AppState, MonitorInfo, UnderlayAction, UnderlayState, WorkArea,
};
use crate::settings::{load_settings, SettingsFile};

const WIDGET_LABEL: &str = "widget";
const DETAIL_LABEL: &str = "detail";
const TRAY_ID: &str = "parley-viewer-tray";
const MENU_OPEN: &str = "open-transcript";
const MENU_SELECT: &str = "select-log";
const MENU_WIDGET: &str = "widget-visible";
const MENU_AUTOSTART: &str = "launch-at-login";
const MENU_HEALTH_MUTED: &str = "peer-health-muted";
const MENU_HEALTH_TEST: &str = "peer-health-test";
const MENU_HEALTH_HANDOFF: &str = "peer-health-handoff";
const MENU_EXIT: &str = "exit";
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const PEER_HEALTH_INTERVAL: Duration = Duration::from_secs(1);

pub struct TrayControls<R: Runtime> {
    widget_visible: CheckMenuItem<R>,
    launch_at_login: CheckMenuItem<R>,
    health_muted: CheckMenuItem<R>,
    health_handoff: MenuItem<R>,
    base_icon: Option<Image<'static>>,
    unread_icon: Option<Image<'static>>,
}

pub fn setup_app(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let settings_path = app.path().app_config_dir()?.join("settings.json");
    let (mut settings, settings_error) = match load_settings(&settings_path) {
        Ok(settings) => (settings, None),
        Err(error) => (SettingsFile::default(), Some(error)),
    };
    let (launch, launch_error) = match LaunchOptions::parse(env::args_os()) {
        Ok(launch) => (launch, None),
        Err(error) => (LaunchOptions::default(), Some(error)),
    };
    let autostart_result = app.autolaunch().is_enabled();
    if let Ok(enabled) = autostart_result {
        settings.viewer.launch_at_login = enabled;
    }

    app.manage(AppState::new(settings_path, settings));
    let state = app.state::<AppState>();
    if let Some(error) = settings_error.or(launch_error) {
        state.set_runtime_error(error);
    }
    if let Err(error) = autostart_result {
        state.set_runtime_error(format!("failed to read launch-at-login state: {error}"));
    }
    if launch.exit {
        state.mark_exiting();
        app.handle().exit(0);
        return Ok(());
    }

    let saved_sources = state.settings().viewer.selected_logs;
    let environment_source = env::var_os("PARLEY_EVENT_LOG");
    let initial_sources = match resolve_initial_sources(&launch, environment_source, &saved_sources)
    {
        Ok(sources) => sources,
        Err(error) => {
            state.set_runtime_error(error);
            resolve_initial_sources(&launch, None, &saved_sources)?
        }
    };
    let persist_sources = initial_sources.origin == SourceOrigin::CommandLine;
    if let Err(error) = state.set_sources(initial_sources.paths, persist_sources) {
        state.set_runtime_error(error);
    }

    let force_tray_failure = env_flag("PARLEY_VIEWER_FORCE_TRAY_FAILURE");
    let tray_result = if force_tray_failure {
        Err("forced tray failure".to_string())
    } else {
        create_tray(app).map_err(|error| error.to_string())
    };
    match tray_result {
        Ok(()) => state.set_tray_available(true),
        Err(error) => {
            state.set_tray_available(false);
            state.set_runtime_error(format!("system tray unavailable: {error}"));
        }
    }

    let tray_available = state.runtime_snapshot().tray_available;
    if launch.show_detail() || !tray_available {
        if let Err(error) = show_detail(app.handle()) {
            state.set_runtime_error(error);
        }
    }
    spawn_supervisor(app.handle().clone());
    Ok(())
}

pub fn handle_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != DETAIL_LABEL {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        let app = window.app_handle();
        let exiting = app
            .try_state::<AppState>()
            .is_some_and(|state| state.runtime_snapshot().exiting);
        if !exiting {
            api.prevent_close();
            let _ = window.hide();
        }
    }
}

pub fn handle_second_instance<R: Runtime>(app: &AppHandle<R>, args: Vec<String>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let options = match LaunchOptions::parse(args.into_iter().map(OsString::from)) {
        Ok(options) => options,
        Err(error) => {
            state.set_runtime_error(error);
            let _ = show_detail(app);
            return;
        }
    };
    if options.exit {
        exit_app(app);
        return;
    }
    if !options.event_logs.is_empty() {
        if let Err(error) = state.set_sources(options.event_logs.clone(), true) {
            state.set_runtime_error(error);
        }
    }
    if options.autostart {
        state.request_widget(true);
        sync_widget_check(app);
    }
    if options.show_detail() {
        if let Err(error) = show_detail(app) {
            state.set_runtime_error(error);
        }
    }
}

pub fn show_detail<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = app
        .get_webview_window(DETAIL_LABEL)
        .ok_or_else(|| "detail window is unavailable".to_string())?;
    window
        .show()
        .map_err(|error| format!("failed to show detail window: {error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("failed to focus detail window: {error}"))?;
    initialize_autostart(app);
    Ok(())
}

pub fn list_monitors<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<MonitorInfo>, String> {
    let primary_id = app
        .primary_monitor()
        .map_err(|error| format!("failed to query primary monitor: {error}"))?
        .as_ref()
        .map(monitor_id);
    let monitors = app
        .available_monitors()
        .map_err(|error| format!("failed to list monitors: {error}"))?;
    Ok(monitors
        .iter()
        .enumerate()
        .map(|(index, monitor)| {
            let id = monitor_id(monitor);
            MonitorInfo {
                id: id.clone(),
                name: monitor
                    .name()
                    .cloned()
                    .unwrap_or_else(|| format!("Display {}", index + 1)),
                primary: primary_id.as_ref() == Some(&id),
            }
        })
        .collect())
}

pub fn apply_widget_placement<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let settings = state.settings().viewer;
    let monitors = app
        .available_monitors()
        .map_err(|error| format!("failed to list monitors: {error}"))?;
    let primary_id = app
        .primary_monitor()
        .map_err(|error| format!("failed to query primary monitor: {error}"))?
        .as_ref()
        .map(monitor_id);
    let selected = settings
        .monitor_id
        .as_ref()
        .and_then(|id| monitors.iter().find(|monitor| monitor_id(monitor) == *id))
        .or_else(|| {
            primary_id
                .as_ref()
                .and_then(|id| monitors.iter().find(|monitor| monitor_id(monitor) == *id))
        })
        .or_else(|| monitors.first())
        .ok_or_else(|| "no display is available for the widget".to_string())?;
    let work_area = selected.work_area();
    let placement = calculate_placement(
        &settings,
        WorkArea {
            x: work_area.position.x,
            y: work_area.position.y,
            width: work_area.size.width,
            height: work_area.size.height,
            scale_factor: selected.scale_factor(),
        },
    );
    let window = widget_window(app)?;
    if desktop_parent_is_valid(&window) {
        set_attached_placement(&window, placement)
    } else {
        window
            .set_size(PhysicalSize::new(placement.width, placement.height))
            .map_err(|error| format!("failed to size widget: {error}"))?;
        window
            .set_position(PhysicalPosition::new(placement.x, placement.y))
            .map_err(|error| format!("failed to position widget: {error}"))
    }
}

pub fn request_widget<R: Runtime>(app: &AppHandle<R>, visible: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    state.request_widget(visible);
    sync_widget_check(app);
    if !visible {
        detach_widget(app)?;
    }
    Ok(())
}

pub fn set_launch_at_login<R: Runtime>(
    app: &AppHandle<R>,
    enabled: bool,
) -> Result<crate::settings::ViewerSettings, String> {
    let manager = app.autolaunch();
    let change = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    change.map_err(|error| format!("failed to update launch-at-login: {error}"))?;
    let state = app.state::<AppState>();
    match state.set_launch_at_login(enabled, true) {
        Ok(settings) => {
            sync_autostart_check(app);
            Ok(settings)
        }
        Err(error) => {
            let _ = if enabled {
                manager.disable()
            } else {
                manager.enable()
            };
            Err(error)
        }
    }
}

pub fn select_log_from_tray<R: Runtime>(app: &AppHandle<R>) {
    let picker_app = app.clone();
    app.dialog()
        .file()
        .set_title("Select Parley event log")
        .add_filter("JSON Lines", &["jsonl"])
        .pick_file(move |selection| {
            let Some(selection) = selection else {
                return;
            };
            let state = picker_app.state::<AppState>();
            match selection.into_path() {
                Ok(path) => {
                    if let Err(error) = state.add_source(path, true) {
                        state.set_runtime_error(error);
                    }
                }
                Err(error) => state.set_runtime_error(format!(
                    "selected event log is not a filesystem path: {error}"
                )),
            }
        });
}

pub fn exit_app<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    state.mark_exiting();
    if let Err(error) = detach_widget(app) {
        state.set_runtime_error(error);
    }
    app.exit(0);
}

pub fn detach_widget<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = widget_window(app)?;
    window
        .hide()
        .map_err(|error| format!("failed to hide widget: {error}"))?;
    if window.is_desktop_underlay() {
        window
            .set_desktop_underlay(false)
            .map_err(|error| format!("failed to detach desktop underlay: {error}"))?;
    }
    detach_shell_parent(&window)?;
    app.state::<AppState>().record_detached();
    Ok(())
}

fn create_tray(app: &App) -> tauri::Result<()> {
    let settings = app.state::<AppState>().settings().viewer;
    let (unread_count, muted) = peer_health::tray_state();
    let open = MenuItem::with_id(app, MENU_OPEN, "Open Transcript", true, None::<&str>)?;
    let select = MenuItem::with_id(app, MENU_SELECT, "Add Event Log...", true, None::<&str>)?;
    let widget_visible = CheckMenuItem::with_id(
        app,
        MENU_WIDGET,
        "Show/Hide Widget",
        true,
        true,
        None::<&str>,
    )?;
    let launch_at_login = CheckMenuItem::with_id(
        app,
        MENU_AUTOSTART,
        "Launch at Login",
        true,
        settings.launch_at_login,
        None::<&str>,
    )?;
    let health_muted = CheckMenuItem::with_id(
        app,
        MENU_HEALTH_MUTED,
        "Mute incident chime",
        true,
        muted,
        None::<&str>,
    )?;
    let health_test = MenuItem::with_id(
        app,
        MENU_HEALTH_TEST,
        "Test Two Chairs chime",
        true,
        None::<&str>,
    )?;
    let health_handoff = MenuItem::with_id(
        app,
        MENU_HEALTH_HANDOFF,
        handoff_menu_label(unread_count),
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let health_separator = PredefinedMenuItem::separator(app)?;
    let exit = MenuItem::with_id(app, MENU_EXIT, "Exit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &select,
            &widget_visible,
            &launch_at_login,
            &separator,
            &health_handoff,
            &health_muted,
            &health_test,
            &health_separator,
            &exit,
        ],
    )?;
    let base_icon = app
        .default_window_icon()
        .map(|icon| icon.clone().to_owned());
    let unread_icon = base_icon.as_ref().map(badged_tray_icon);
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(tray_tooltip(unread_count))
        .show_menu_on_left_click(true)
        .on_menu_event(handle_tray_menu)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                let _ = show_detail(tray.app_handle());
            }
        });
    if let Some(icon) = if unread_count > 0 {
        unread_icon.clone()
    } else {
        base_icon.clone()
    } {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    app.manage(TrayControls {
        widget_visible,
        launch_at_login,
        health_muted,
        health_handoff,
        base_icon,
        unread_icon,
    });
    Ok(())
}

fn handle_tray_menu<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        MENU_OPEN => {
            if let Err(error) = show_detail(app) {
                app.state::<AppState>().set_runtime_error(error);
            }
        }
        MENU_SELECT => select_log_from_tray(app),
        MENU_WIDGET => {
            let visible = !app.state::<AppState>().runtime_snapshot().widget_requested;
            if let Err(error) = request_widget(app, visible) {
                app.state::<AppState>().set_runtime_error(error);
            }
        }
        MENU_AUTOSTART => {
            let enabled = !app.state::<AppState>().settings().viewer.launch_at_login;
            if let Err(error) = set_launch_at_login(app, enabled) {
                app.state::<AppState>().set_runtime_error(error);
                sync_autostart_check(app);
            }
        }
        MENU_HEALTH_MUTED => {
            let muted = !peer_health::tray_state().1;
            let worker_app = app.clone();
            thread::spawn(move || {
                if let Err(error) = peer_health::set_muted(muted) {
                    worker_app
                        .state::<AppState>()
                        .set_runtime_error(format!("failed to update peer-health mute: {error}"));
                }
                sync_peer_health_tray(&worker_app);
            });
        }
        MENU_HEALTH_TEST => {
            if let Err(error) = peer_health::test_chime() {
                app.state::<AppState>()
                    .set_runtime_error(format!("failed to request peer-health chime: {error}"));
            }
        }
        MENU_HEALTH_HANDOFF => {
            if let Err(error) = show_detail(app) {
                app.state::<AppState>().set_runtime_error(error);
            } else if let Err(error) = app.emit_to(DETAIL_LABEL, "peer-health-open-handoff", ()) {
                app.state::<AppState>()
                    .set_runtime_error(format!("failed to open latest handoff: {error}"));
            }
        }
        MENU_EXIT => exit_app(app),
        _ => {}
    }
}

fn initialize_autostart<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    let settings = state.settings();
    if settings.autostart_initialized {
        if settings.viewer.launch_at_login {
            if let Err(error) = app.autolaunch().enable() {
                state.set_runtime_error(format!("failed to refresh launch-at-login: {error}"));
            }
        }
        return;
    }
    if let Err(error) = set_launch_at_login(app, true) {
        state.set_runtime_error(error);
    }
}

fn spawn_supervisor(app: AppHandle) {
    thread::spawn(move || {
        let started = Instant::now();
        let mut last_peer_health_poll = None;
        while !app.state::<AppState>().should_stop() {
            let state = app.state::<AppState>();
            state.engine.poll();
            let elapsed = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            match state.next_underlay_action(elapsed) {
                UnderlayAction::Attach => attach_widget(&app, elapsed),
                UnderlayAction::HealthCheck => check_widget_health(&app, elapsed),
                UnderlayAction::Detach => {
                    if let Err(error) = detach_widget(&app) {
                        state.set_runtime_error(error);
                    }
                }
                UnderlayAction::None => {}
            }
            if last_peer_health_poll
                .map(|last: Instant| last.elapsed() >= PEER_HEALTH_INTERVAL)
                .unwrap_or(true)
            {
                sync_peer_health_tray(&app);
                last_peer_health_poll = Some(Instant::now());
            }
            thread::sleep(POLL_INTERVAL);
        }
    });
}

fn attach_widget<R: Runtime>(app: &AppHandle<R>, now_ms: u64) {
    let state = app.state::<AppState>();
    state.set_underlay_state(UnderlayState::Attaching);
    let result = (|| -> Result<(), String> {
        let window = widget_window(app)?;
        window
            .hide()
            .map_err(|error| format!("failed to hide widget before attach: {error}"))?;
        if env_flag("PARLEY_VIEWER_FORCE_UNDERLAY_FAILURE") {
            return Err("forced desktop-underlay failure".to_string());
        }
        if window.is_desktop_underlay() {
            window
                .set_desktop_underlay(false)
                .map_err(|error| format!("failed to reset desktop underlay: {error}"))?;
        }
        detach_shell_parent(&window)?;
        apply_widget_placement(app)?;
        window
            .set_position(PhysicalPosition::new(-32_000, -32_000))
            .map_err(|error| format!("failed to stage widget off-screen: {error}"))?;
        window
            .show()
            .map_err(|error| format!("failed to activate widget off-screen: {error}"))?;
        let plugin_error = window
            .set_desktop_underlay(true)
            .err()
            .map(|error| error.to_string());
        if !desktop_parent_is_valid(&window) {
            if window.is_desktop_underlay() {
                let _ = window.set_desktop_underlay(false);
            }
            attach_shell_parent(&window).map_err(|repair_error| match plugin_error {
                Some(plugin_error) => format!(
                    "desktop-underlay plugin failed ({plugin_error}); shell attach failed ({repair_error})"
                ),
                None => format!(
                    "desktop-underlay parent verification failed; shell attach failed ({repair_error})"
                ),
            })?;
        }
        if !desktop_parent_is_valid(&window) {
            return Err(
                "desktop-underlay parent verification failed before positioning".to_string(),
            );
        }
        apply_widget_placement(app)?;
        if !desktop_parent_is_valid(&window) {
            return Err("desktop-underlay parent was lost during positioning".to_string());
        }
        place_below_desktop_icons(&window)?;
        if !desktop_parent_is_valid(&window) {
            let _ = window.hide();
            if window.is_desktop_underlay() {
                let _ = window.set_desktop_underlay(false);
            }
            let _ = detach_shell_parent(&window);
            return Err("desktop-underlay parent was lost during z-ordering".to_string());
        }
        Ok(())
    })();
    match result {
        Ok(()) => state.record_attach_result(now_ms, true),
        Err(error) => {
            if let Ok(window) = widget_window(app) {
                let _ = window.hide();
            }
            state.record_attach_result(now_ms, false);
            state.set_runtime_error(error);
        }
    }
}

fn check_widget_health<R: Runtime>(app: &AppHandle<R>, now_ms: u64) {
    let state = app.state::<AppState>();
    if let Err(error) = apply_widget_placement(app) {
        state.set_runtime_error(error);
    }
    let healthy = widget_window(app)
        .map(|window| desktop_parent_is_valid(&window))
        .unwrap_or(false);
    if !healthy {
        if let Ok(window) = widget_window(app) {
            let _ = window.hide();
            if window.is_desktop_underlay() {
                let _ = window.set_desktop_underlay(false);
            }
            let _ = detach_shell_parent(&window);
        }
        state.set_runtime_error("desktop underlay was lost; scheduling reattachment");
    }
    state.record_health_result(now_ms, healthy);
}

fn desktop_parent_is_valid<R: Runtime>(window: &WebviewWindow<R>) -> bool {
    let Ok(hwnd) = raw_hwnd(window) else {
        return false;
    };
    let parent = unsafe { GetParent(hwnd) };
    !parent.is_null()
        && unsafe { IsWindow(parent) } != 0
        && matches!(window_class(parent).as_deref(), Some("WorkerW" | "Progman"))
}

fn attach_shell_parent<R: Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    let hwnd = raw_hwnd(window)?;
    let parent = shell_underlay_parent()?;
    set_child_style(hwnd, true)?;
    if let Err(error) = set_parent_checked(hwnd, parent) {
        let _ = set_child_style(hwnd, false);
        return Err(error);
    }
    if unsafe { GetParent(hwnd) } != parent {
        let _ = set_parent_checked(hwnd, std::ptr::null_mut());
        let _ = set_child_style(hwnd, false);
        return Err("Explorer rejected the desktop-underlay parent".to_string());
    }
    place_below_desktop_icons(window)
}

fn detach_shell_parent<R: Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    let hwnd = raw_hwnd(window)?;
    let parent = unsafe { GetParent(hwnd) };
    if !parent.is_null() {
        set_parent_checked(hwnd, std::ptr::null_mut())?;
    }
    set_child_style(hwnd, false)
}

fn place_below_desktop_icons<R: Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    let hwnd = raw_hwnd(window)?;
    let insert_after = underlay_z_anchor(hwnd);
    let result = unsafe {
        SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    };
    if result == 0 {
        Err(format!(
            "failed to place widget below desktop icons: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

fn set_attached_placement<R: Runtime>(
    window: &WebviewWindow<R>,
    placement: crate::runtime::WidgetPlacement,
) -> Result<(), String> {
    let hwnd = raw_hwnd(window)?;
    let insert_after = underlay_z_anchor(hwnd);
    let width = i32::try_from(placement.width).unwrap_or(i32::MAX);
    let height = i32::try_from(placement.height).unwrap_or(i32::MAX);
    let result = unsafe {
        SetWindowPos(
            hwnd,
            insert_after,
            placement.x,
            placement.y,
            width,
            height,
            SWP_NOACTIVATE,
        )
    };
    if result == 0 {
        Err(format!(
            "failed to place attached widget: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

fn underlay_z_anchor(window: HWND) -> HWND {
    let parent = unsafe { GetParent(window) };
    if parent.is_null() {
        return HWND_BOTTOM;
    }
    let defview_class = wide_null("SHELLDLL_DefView");
    let defview = unsafe {
        FindWindowExW(
            parent,
            std::ptr::null_mut(),
            defview_class.as_ptr(),
            std::ptr::null(),
        )
    };
    if defview.is_null() {
        HWND_BOTTOM
    } else {
        defview
    }
}

fn shell_underlay_parent() -> Result<HWND, String> {
    let progman_class = wide_null("Progman");
    let defview_class = wide_null("SHELLDLL_DefView");
    let worker_class = wide_null("WorkerW");
    let progman = unsafe { FindWindowW(progman_class.as_ptr(), std::ptr::null()) };
    if progman.is_null() {
        return Err("Explorer Progman window is unavailable".to_string());
    }
    unsafe {
        SendMessageTimeoutW(
            progman,
            0x052C,
            0x0000_000D,
            0x0000_0001,
            SMTO_NORMAL,
            1_000,
            std::ptr::null_mut(),
        );
    }
    let mut host = if unsafe {
        FindWindowExW(
            progman,
            std::ptr::null_mut(),
            defview_class.as_ptr(),
            std::ptr::null(),
        )
    }
    .is_null()
    {
        std::ptr::null_mut()
    } else {
        progman
    };
    let mut worker = std::ptr::null_mut();
    while host.is_null() {
        worker = unsafe {
            FindWindowExW(
                std::ptr::null_mut(),
                worker,
                worker_class.as_ptr(),
                std::ptr::null(),
            )
        };
        if worker.is_null() {
            break;
        }
        let defview = unsafe {
            FindWindowExW(
                worker,
                std::ptr::null_mut(),
                defview_class.as_ptr(),
                std::ptr::null(),
            )
        };
        if !defview.is_null() {
            host = worker;
        }
    }
    if host.is_null() {
        return Err("Explorer desktop icon host is unavailable".to_string());
    }
    let next_worker = unsafe {
        FindWindowExW(
            std::ptr::null_mut(),
            host,
            worker_class.as_ptr(),
            std::ptr::null(),
        )
    };
    if next_worker.is_null() {
        Ok(host)
    } else {
        Ok(next_worker)
    }
}

fn set_parent_checked(window: HWND, parent: HWND) -> Result<(), String> {
    unsafe {
        SetLastError(ERROR_SUCCESS);
        let previous = SetParent(window, parent);
        let error = GetLastError();
        if previous.is_null() && error != ERROR_SUCCESS {
            return Err(format!(
                "failed to set desktop-underlay parent: {}",
                std::io::Error::from_raw_os_error(error as i32)
            ));
        }
    }
    Ok(())
}

fn set_child_style(window: HWND, child: bool) -> Result<(), String> {
    let current = unsafe { GetWindowLongPtrW(window, GWL_STYLE) } as u32;
    let next = if child {
        (current & !WS_POPUP) | WS_CHILD
    } else {
        (current & !WS_CHILD) | WS_POPUP
    };
    unsafe {
        SetLastError(ERROR_SUCCESS);
        let previous = SetWindowLongPtrW(window, GWL_STYLE, next as isize);
        let error = GetLastError();
        if previous == 0 && error != ERROR_SUCCESS {
            return Err(format!(
                "failed to update desktop-underlay window style: {}",
                std::io::Error::from_raw_os_error(error as i32)
            ));
        }
    }
    let result = unsafe {
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
    };
    if result == 0 {
        Err(format!(
            "failed to apply desktop-underlay window style: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

fn raw_hwnd<R: Runtime>(window: &WebviewWindow<R>) -> Result<HWND, String> {
    window
        .hwnd()
        .map(|hwnd| hwnd.0.cast())
        .map_err(|error| format!("failed to resolve widget window handle: {error}"))
}

fn window_class(window: HWND) -> Option<String> {
    let mut buffer = [0_u16; 64];
    let length = unsafe { GetClassNameW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    if length <= 0 {
        None
    } else {
        Some(String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn widget_window<R: Runtime>(app: &AppHandle<R>) -> Result<WebviewWindow<R>, String> {
    app.get_webview_window(WIDGET_LABEL)
        .ok_or_else(|| "widget window is unavailable".to_string())
}

fn monitor_id(monitor: &tauri::Monitor) -> String {
    let name = monitor.name().map(String::as_str).unwrap_or("Display");
    let position = monitor.position();
    let size = monitor.size();
    format!(
        "{name}|{},{}|{}x{}",
        position.x, position.y, size.width, size.height
    )
}

fn sync_widget_check<R: Runtime>(app: &AppHandle<R>) {
    let Some(controls) = app.try_state::<TrayControls<R>>() else {
        return;
    };
    let checked = app.state::<AppState>().runtime_snapshot().widget_requested;
    let _ = controls.widget_visible.set_checked(checked);
}

fn sync_autostart_check<R: Runtime>(app: &AppHandle<R>) {
    let Some(controls) = app.try_state::<TrayControls<R>>() else {
        return;
    };
    let checked = app.state::<AppState>().settings().viewer.launch_at_login;
    let _ = controls.launch_at_login.set_checked(checked);
}

fn sync_peer_health_tray<R: Runtime>(app: &AppHandle<R>) {
    let Some(controls) = app.try_state::<TrayControls<R>>() else {
        return;
    };
    let (unread_count, muted) = peer_health::tray_state();
    let _ = controls.health_muted.set_checked(muted);
    let _ = controls
        .health_handoff
        .set_text(handoff_menu_label(unread_count));
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tray_tooltip(unread_count)));
        let icon = if unread_count > 0 {
            controls.unread_icon.clone()
        } else {
            controls.base_icon.clone()
        };
        if let Some(icon) = icon {
            let _ = tray.set_icon(Some(icon));
        }
    }
}

fn handoff_menu_label(unread_count: u64) -> String {
    if unread_count == 0 {
        "Open Latest Handoff".to_string()
    } else {
        format!("Open Latest Handoff ({unread_count} unread)")
    }
}

fn tray_tooltip(unread_count: u64) -> String {
    if unread_count == 0 {
        "Parley Conversation Viewer".to_string()
    } else {
        format!("Parley Conversation Viewer · {unread_count} unread incident(s)")
    }
}

fn badged_tray_icon(base: &Image<'_>) -> Image<'static> {
    let width = base.width();
    let height = base.height();
    let mut rgba = base.rgba().to_vec();
    let radius = (width.min(height) / 5).max(2);
    let center_x = width.saturating_sub(radius + 1);
    let center_y = radius + 1;
    let radius_squared = i64::from(radius) * i64::from(radius);
    for y in 0..height {
        for x in 0..width {
            let dx = i64::from(x) - i64::from(center_x);
            let dy = i64::from(y) - i64::from(center_y);
            if dx * dx + dy * dy <= radius_squared {
                let offset = ((y * width + x) * 4) as usize;
                if offset + 3 < rgba.len() {
                    rgba[offset] = 238;
                    rgba[offset + 1] = 75;
                    rgba[offset + 2] = 86;
                    rgba[offset + 3] = 255;
                }
            }
        }
    }
    Image::new_owned(rgba, width, height)
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_flags_are_opt_in() {
        let key = format!("PARLEY_VIEWER_TEST_FLAG_{}", std::process::id());
        env::remove_var(&key);
        assert!(!env_flag(&key));
        env::set_var(&key, "true");
        assert!(env_flag(&key));
        env::set_var(&key, "0");
        assert!(!env_flag(&key));
        env::remove_var(&key);
    }

    #[test]
    fn tray_labels_are_quiet_until_incidents_are_unread() {
        assert_eq!(handoff_menu_label(0), "Open Latest Handoff");
        assert_eq!(handoff_menu_label(2), "Open Latest Handoff (2 unread)");
        assert!(!tray_tooltip(0).contains("unread"));
        assert!(tray_tooltip(1).contains("1 unread"));
    }

    #[test]
    fn unread_badge_changes_only_a_bounded_icon_region() {
        let base = Image::new_owned(vec![0; 16 * 16 * 4], 16, 16);
        let badged = badged_tray_icon(&base);
        let changed = badged
            .rgba()
            .chunks_exact(4)
            .filter(|pixel| pixel[3] != 0)
            .count();
        assert!(changed > 0);
        assert!(changed < 16 * 16 / 2);
    }
}
