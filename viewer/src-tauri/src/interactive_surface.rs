use std::fs::{self, OpenOptions};
use std::io::Write;
use std::mem;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, FindWindowW,
    GetForegroundWindow, GetParent, GetWindow, GetWindowLongPtrW, GetWindowRect, IsWindow,
    IsWindowVisible, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_WNDPROC, GWL_EXSTYLE,
    GWL_STYLE, GW_HWNDNEXT, GW_HWNDPREV, GW_OWNER, HWND_TOP, MA_NOACTIVATE, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, WINDOWPOS,
    WM_MOUSEACTIVATE, WM_WINDOWPOSCHANGING, WNDPROC, WS_CHILD, WS_DISABLED, WS_EX_APPWINDOW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::runtime::WidgetSurfaceBoundsReport;

pub const SURFACE_LABEL: &str = "widget-surface";
const SURFACE_READY_TIMEOUT: Duration = Duration::from_secs(10);
const GEOMETRY_TOLERANCE_PX: i32 = 1;

static ORIGINAL_SURFACE_PROC: AtomicIsize = AtomicIsize::new(0);
static CONTROLLED_SURFACE_POSITION: AtomicBool = AtomicBool::new(false);
static LAST_MOUSEACTIVATE_FOREGROUND: AtomicIsize = AtomicIsize::new(0);
static BAND_HELPER_HWND: AtomicIsize = AtomicIsize::new(0);
static SURFACE_CREATED_AT: Mutex<Option<Instant>> = Mutex::new(None);
static PROCESS_STARTED: OnceLock<Instant> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Waiting,
    TimedOut,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FocusDiagnostic<'a> {
    schema_version: u8,
    timestamp_ms: u64,
    action: &'a str,
    foreground_at_mouse_activate: String,
    foreground_after_command: String,
    unchanged: bool,
    intentional_focus: bool,
}

pub fn monotonic_ms() -> u64 {
    PROCESS_STARTED
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub fn ensure_surface<R: Runtime>(app: &AppHandle<R>) -> Result<WebviewWindow<R>, String> {
    if let Some(surface) = app.get_webview_window(SURFACE_LABEL) {
        return Ok(surface);
    }

    let surface = WebviewWindowBuilder::new(
        app,
        SURFACE_LABEL,
        WebviewUrl::App("index.html?view=widget-surface".into()),
    )
    .title("Parley conversation")
    .inner_size(320.0, 220.0)
    .decorations(false)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .always_on_top(false)
    .skip_taskbar(true)
    .visible(false)
    .build()
    .map_err(|error| format!("failed to create widget surface: {error}"))?;

    let configure_result = (|| {
        let hwnd = raw_webview_hwnd(&surface)?;
        configure_surface_styles(hwnd)?;
        install_surface_window_proc(hwnd)?;
        verify_surface_styles(hwnd)
    })();
    if let Err(error) = configure_result {
        let _ = surface.destroy();
        return Err(error);
    }
    *lock_surface_created() = Some(Instant::now());
    Ok(surface)
}

pub fn ensure_band_helper() -> Result<HWND, String> {
    let existing = BAND_HELPER_HWND.load(Ordering::Acquire) as HWND;
    if !existing.is_null() && unsafe { IsWindow(existing) } != 0 {
        return Ok(existing);
    }

    let class_name = wide_null("STATIC");
    let title = wide_null("Parley desktop band");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_DISABLED,
            -32_000,
            -32_000,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err(format!(
            "failed to create desktop-band helper: {}",
            std::io::Error::last_os_error()
        ));
    }
    BAND_HELPER_HWND.store(hwnd as isize, Ordering::Release);
    if let Err(error) = configure_helper_styles(hwnd).and_then(|()| verify_helper_styles(hwnd)) {
        unsafe {
            DestroyWindow(hwnd);
        }
        BAND_HELPER_HWND.store(0, Ordering::Release);
        return Err(error);
    }
    let shown = unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOP,
            -32_000,
            -32_000,
            1,
            1,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    };
    if shown == 0 {
        unsafe {
            DestroyWindow(hwnd);
        }
        BAND_HELPER_HWND.store(0, Ordering::Release);
        return Err(format!(
            "failed to activate desktop-band helper: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(hwnd)
}

fn band_helper() -> Result<HWND, String> {
    let helper = BAND_HELPER_HWND.load(Ordering::Acquire) as HWND;
    if helper.is_null() || unsafe { IsWindow(helper) } == 0 {
        Err("desktop-band helper is unavailable".to_string())
    } else {
        Ok(helper)
    }
}

pub fn surface_readiness(ready: bool) -> Readiness {
    if ready {
        return Readiness::Waiting;
    }
    let created = *lock_surface_created();
    match created {
        Some(created) if created.elapsed() >= SURFACE_READY_TIMEOUT => Readiness::TimedOut,
        _ => Readiness::Waiting,
    }
}

pub fn reset_surface_handshake_deadline() {
    *lock_surface_created() = Some(Instant::now());
}

pub fn derive_surface_rect<R: Runtime>(
    underlay: &WebviewWindow<R>,
    report: WidgetSurfaceBoundsReport,
) -> Result<SurfaceRect, String> {
    let underlay_hwnd = raw_webview_hwnd(underlay)?;
    let mut underlay_rect = RECT::default();
    if unsafe { GetWindowRect(underlay_hwnd, &mut underlay_rect) } == 0 {
        return Err(format!(
            "failed to read desktop-underlay geometry: {}",
            std::io::Error::last_os_error()
        ));
    }
    derive_surface_rect_from_physical(underlay_rect, report)
}

pub fn physical_window_rect<R: Runtime>(window: &WebviewWindow<R>) -> Result<SurfaceRect, String> {
    let hwnd = raw_webview_hwnd(window)?;
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err(format!(
            "failed to read window geometry: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(SurfaceRect {
        x: rect.left,
        y: rect.top,
        width: rect.right.saturating_sub(rect.left),
        height: rect.bottom.saturating_sub(rect.top),
    })
}

fn derive_surface_rect_from_physical(
    underlay: RECT,
    report: WidgetSurfaceBoundsReport,
) -> Result<SurfaceRect, String> {
    let underlay_width = underlay.right.saturating_sub(underlay.left);
    let underlay_height = underlay.bottom.saturating_sub(underlay.top);
    let reported_width = (report.viewport_width * report.device_pixel_ratio).round() as i32;
    let reported_height = (report.viewport_height * report.device_pixel_ratio).round() as i32;
    if (underlay_width - reported_width).abs() > GEOMETRY_TOLERANCE_PX
        || (underlay_height - reported_height).abs() > GEOMETRY_TOLERANCE_PX
    {
        return Err("widget-surface viewport does not match the desktop underlay".to_string());
    }

    let x = underlay.left + (report.left * report.device_pixel_ratio).round() as i32;
    let y = underlay.top + (report.top * report.device_pixel_ratio).round() as i32;
    let width = (report.width * report.device_pixel_ratio).round() as i32;
    let height = (report.height * report.device_pixel_ratio).round() as i32;
    if width <= 0 || height <= 0 {
        return Err("widget-surface physical geometry is empty".to_string());
    }
    let right = x.saturating_add(width);
    let bottom = y.saturating_add(height);
    if x < underlay.left - GEOMETRY_TOLERANCE_PX
        || y < underlay.top - GEOMETRY_TOLERANCE_PX
        || right > underlay.right + GEOMETRY_TOLERANCE_PX
        || bottom > underlay.bottom + GEOMETRY_TOLERANCE_PX
    {
        return Err("widget-surface physical geometry escapes the desktop underlay".to_string());
    }
    Ok(SurfaceRect {
        x,
        y,
        width,
        height,
    })
}

pub fn position_and_restack<R: Runtime>(
    app: &AppHandle<R>,
    surface: &WebviewWindow<R>,
    rect: SurfaceRect,
) -> Result<(), String> {
    let helper_hwnd = band_helper()?;
    let surface_hwnd = raw_webview_hwnd(surface)?;
    let icon_host = desktop_icon_host()?;
    let above_icon_host = unsafe { GetWindow(icon_host, GW_HWNDPREV) };
    if above_icon_host.is_null() {
        return Err("desktop icon host has no safe normal-band predecessor".to_string());
    }

    verify_helper_styles(helper_hwnd)?;
    verify_surface_styles(surface_hwnd)?;
    let helper_positioned = unsafe {
        SetWindowPos(
            helper_hwnd,
            above_icon_host,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    };
    if helper_positioned == 0 {
        return Err(format!(
            "failed to place desktop-band helper: {}",
            std::io::Error::last_os_error()
        ));
    }

    CONTROLLED_SURFACE_POSITION.store(true, Ordering::Release);
    let surface_positioned = unsafe {
        SetWindowPos(
            surface_hwnd,
            helper_hwnd,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    };
    CONTROLLED_SURFACE_POSITION.store(false, Ordering::Release);
    if surface_positioned == 0 {
        return Err(format!(
            "failed to position widget surface: {}",
            std::io::Error::last_os_error()
        ));
    }
    verify_surface(app, rect)
}

pub fn verify_surface<R: Runtime>(app: &AppHandle<R>, expected: SurfaceRect) -> Result<(), String> {
    let helper_hwnd = BAND_HELPER_HWND.load(Ordering::Acquire) as HWND;
    if helper_hwnd.is_null() {
        return Err("desktop-band helper is unavailable".to_string());
    }
    let surface = app
        .get_webview_window(SURFACE_LABEL)
        .ok_or_else(|| "widget surface is unavailable".to_string())?;
    let surface_hwnd = raw_webview_hwnd(&surface)?;
    let icon_host = desktop_icon_host()?;
    if unsafe { IsWindow(helper_hwnd) } == 0 || unsafe { IsWindow(surface_hwnd) } == 0 {
        return Err("interactive desktop window was destroyed".to_string());
    }
    verify_helper_styles(helper_hwnd)?;
    verify_surface_styles(surface_hwnd)?;
    if unsafe { IsWindowVisible(surface_hwnd) } == 0 {
        return Err("widget surface is not visible".to_string());
    }
    if unsafe { GetWindow(helper_hwnd, GW_HWNDNEXT) } != surface_hwnd
        || unsafe { GetWindow(surface_hwnd, GW_HWNDNEXT) } != icon_host
    {
        return Err("widget surface left its desktop z-order band".to_string());
    }

    let mut actual = RECT::default();
    if unsafe { GetWindowRect(surface_hwnd, &mut actual) } == 0 {
        return Err(format!(
            "failed to verify widget-surface geometry: {}",
            std::io::Error::last_os_error()
        ));
    }
    let actual_width = actual.right.saturating_sub(actual.left);
    let actual_height = actual.bottom.saturating_sub(actual.top);
    if (actual.left - expected.x).abs() > GEOMETRY_TOLERANCE_PX
        || (actual.top - expected.y).abs() > GEOMETRY_TOLERANCE_PX
        || (actual_width - expected.width).abs() > GEOMETRY_TOLERANCE_PX
        || (actual_height - expected.height).abs() > GEOMETRY_TOLERANCE_PX
    {
        return Err("widget-surface geometry differs from its validated column".to_string());
    }
    Ok(())
}

pub fn hide_surface<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let Some(surface) = app.get_webview_window(SURFACE_LABEL) else {
        return Ok(());
    };
    let hwnd = raw_webview_hwnd(&surface)?;
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
    }
    Ok(())
}

pub fn destroy_surface<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let Some(surface) = app.get_webview_window(SURFACE_LABEL) else {
        *lock_surface_created() = None;
        return Ok(());
    };
    hide_surface(app)?;
    surface
        .destroy()
        .map_err(|error| format!("failed to destroy widget surface: {error}"))?;
    ORIGINAL_SURFACE_PROC.store(0, Ordering::Release);
    LAST_MOUSEACTIVATE_FOREGROUND.store(0, Ordering::Release);
    *lock_surface_created() = None;
    Ok(())
}

pub fn destroy_band_helper() -> Result<(), String> {
    let helper = BAND_HELPER_HWND.swap(0, Ordering::AcqRel) as HWND;
    if helper.is_null() {
        return Ok(());
    }
    if unsafe { DestroyWindow(helper) } == 0 {
        Err(format!(
            "failed to destroy desktop-band helper: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

pub fn record_focus_diagnostic<R: Runtime>(
    app: &AppHandle<R>,
    action: &str,
    intentional_focus: bool,
) -> Result<(), String> {
    let before = LAST_MOUSEACTIVATE_FOREGROUND.load(Ordering::Acquire);
    let after = unsafe { GetForegroundWindow() } as isize;
    let diagnostic = FocusDiagnostic {
        schema_version: 1,
        timestamp_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
        action,
        foreground_at_mouse_activate: format_handle(before),
        foreground_after_command: format_handle(after),
        unchanged: before != 0 && before == after,
        intentional_focus,
    };
    let directory = app
        .path()
        .app_local_data_dir()
        .map_err(|error| format!("failed to resolve interactive diagnostics directory: {error}"))?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("failed to create interactive diagnostics directory: {error}"))?;
    let path = directory.join("interactive-diagnostics.jsonl");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("failed to open interactive diagnostics: {error}"))?;
    serde_json::to_writer(&mut file, &diagnostic)
        .map_err(|error| format!("failed to serialize interactive diagnostics: {error}"))?;
    file.write_all(b"\n")
        .and_then(|_| file.flush())
        .map_err(|error| format!("failed to append interactive diagnostics: {error}"))
}

fn configure_surface_styles(window: HWND) -> Result<(), String> {
    let (style, extended) = normalized_surface_styles(
        unsafe { GetWindowLongPtrW(window, GWL_STYLE) } as u32,
        unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } as u32,
    );
    set_window_styles(window, style, extended, "widget surface")
}

fn configure_helper_styles(window: HWND) -> Result<(), String> {
    let style = (unsafe { GetWindowLongPtrW(window, GWL_STYLE) } as u32 & !WS_CHILD)
        | WS_POPUP
        | WS_DISABLED;
    let extended = (unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } as u32
        & !(WS_EX_APPWINDOW | WS_EX_TOPMOST | WS_EX_TRANSPARENT))
        | WS_EX_TOOLWINDOW
        | WS_EX_NOACTIVATE;
    set_window_styles(window, style, extended, "desktop-band helper")
}

fn normalized_surface_styles(style: u32, extended: u32) -> (u32, u32) {
    (
        (style & !WS_CHILD) | WS_POPUP,
        (extended & !(WS_EX_APPWINDOW | WS_EX_TOPMOST | WS_EX_TRANSPARENT))
            | WS_EX_TOOLWINDOW
            | WS_EX_NOACTIVATE,
    )
}

fn set_window_styles(
    window: HWND,
    style: u32,
    extended: u32,
    description: &str,
) -> Result<(), String> {
    unsafe {
        SetWindowLongPtrW(window, GWL_STYLE, style as isize);
        SetWindowLongPtrW(window, GWL_EXSTYLE, extended as isize);
    }
    let updated = unsafe {
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
    if updated == 0 {
        Err(format!(
            "failed to apply {description} styles: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

fn verify_surface_styles(window: HWND) -> Result<(), String> {
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) } as u32;
    let extended = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } as u32;
    let required_style = WS_POPUP;
    let prohibited_style = WS_CHILD;
    let required_extended = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let prohibited_extended = WS_EX_APPWINDOW | WS_EX_TOPMOST | WS_EX_TRANSPARENT;
    if style & required_style != required_style
        || style & prohibited_style != 0
        || extended & required_extended != required_extended
        || extended & prohibited_extended != 0
    {
        return Err("widget-surface native style invariant failed".to_string());
    }
    if !unsafe { GetParent(window) }.is_null() || !unsafe { GetWindow(window, GW_OWNER) }.is_null()
    {
        return Err("widget surface acquired a parent or owner".to_string());
    }
    Ok(())
}

fn verify_helper_styles(window: HWND) -> Result<(), String> {
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) } as u32;
    let extended = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } as u32;
    if style & (WS_POPUP | WS_DISABLED) != WS_POPUP | WS_DISABLED
        || style & WS_CHILD != 0
        || extended & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
        || extended & (WS_EX_APPWINDOW | WS_EX_TOPMOST | WS_EX_TRANSPARENT) != 0
    {
        return Err("desktop-band helper native style invariant failed".to_string());
    }
    if !unsafe { GetParent(window) }.is_null() || !unsafe { GetWindow(window, GW_OWNER) }.is_null()
    {
        return Err("desktop-band helper acquired a parent or owner".to_string());
    }
    Ok(())
}

fn install_surface_window_proc(window: HWND) -> Result<(), String> {
    let previous = unsafe {
        SetWindowLongPtrW(
            window,
            GWLP_WNDPROC,
            surface_window_proc as *const () as usize as isize,
        )
    };
    if previous == 0 {
        return Err(format!(
            "failed to install widget-surface no-activate guard: {}",
            std::io::Error::last_os_error()
        ));
    }
    ORIGINAL_SURFACE_PROC.store(previous, Ordering::Release);
    Ok(())
}

unsafe extern "system" fn surface_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_MOUSEACTIVATE {
        LAST_MOUSEACTIVATE_FOREGROUND.store(GetForegroundWindow() as isize, Ordering::Release);
        return MA_NOACTIVATE as LRESULT;
    }
    if message == WM_WINDOWPOSCHANGING
        && !CONTROLLED_SURFACE_POSITION.load(Ordering::Acquire)
        && lparam != 0
    {
        let position = &mut *(lparam as *mut WINDOWPOS);
        position.flags |= SWP_NOZORDER | SWP_NOACTIVATE;
    }
    let original = ORIGINAL_SURFACE_PROC.load(Ordering::Acquire);
    if original == 0 {
        DefWindowProcW(window, message, wparam, lparam)
    } else {
        CallWindowProcW(
            mem::transmute::<isize, WNDPROC>(original),
            window,
            message,
            wparam,
            lparam,
        )
    }
}

fn desktop_icon_host() -> Result<HWND, String> {
    let progman_class = wide_null("Progman");
    let defview_class = wide_null("SHELLDLL_DefView");
    let worker_class = wide_null("WorkerW");
    let progman = unsafe { FindWindowW(progman_class.as_ptr(), std::ptr::null()) };
    if !progman.is_null()
        && !unsafe {
            FindWindowExW(
                progman,
                std::ptr::null_mut(),
                defview_class.as_ptr(),
                std::ptr::null(),
            )
        }
        .is_null()
    {
        return Ok(progman);
    }

    let mut worker = std::ptr::null_mut();
    loop {
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
            return Ok(worker);
        }
    }
    Err("Explorer desktop icon host is unavailable".to_string())
}

fn raw_webview_hwnd<R: Runtime>(window: &WebviewWindow<R>) -> Result<HWND, String> {
    window
        .hwnd()
        .map(|hwnd| hwnd.0.cast())
        .map_err(|error| format!("failed to resolve widget-surface handle: {error}"))
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn lock_surface_created() -> std::sync::MutexGuard<'static, Option<Instant>> {
    SURFACE_CREATED_AT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn format_handle(handle: isize) -> String {
    if handle == 0 {
        "unavailable".to_string()
    } else {
        format!("0x{:X}", handle as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> WidgetSurfaceBoundsReport {
        WidgetSurfaceBoundsReport {
            left: 120.0,
            top: 24.0,
            width: 320.0,
            height: 300.0,
            viewport_width: 560.0,
            viewport_height: 360.0,
            device_pixel_ratio: 1.5,
        }
    }

    #[test]
    fn style_normalization_enforces_nonactivating_tool_window() {
        let (style, extended) = normalized_surface_styles(
            WS_CHILD,
            WS_EX_APPWINDOW | WS_EX_TOPMOST | WS_EX_TRANSPARENT,
        );

        assert_eq!(style & WS_CHILD, 0);
        assert_eq!(style & WS_POPUP, WS_POPUP);
        assert_eq!(
            extended & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE),
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
        );
        assert_eq!(
            extended & (WS_EX_APPWINDOW | WS_EX_TOPMOST | WS_EX_TRANSPARENT),
            0
        );
    }

    #[test]
    fn physical_geometry_matches_validated_css_bounds() {
        let underlay = RECT {
            left: 1_000,
            top: 100,
            right: 1_840,
            bottom: 640,
        };

        assert_eq!(
            derive_surface_rect_from_physical(underlay, report()).unwrap(),
            SurfaceRect {
                x: 1_180,
                y: 136,
                width: 480,
                height: 450,
            }
        );
    }

    #[test]
    fn geometry_rejects_viewport_mismatch() {
        let underlay = RECT {
            left: 0,
            top: 0,
            right: 838,
            bottom: 540,
        };

        assert!(derive_surface_rect_from_physical(underlay, report()).is_err());
    }
}
