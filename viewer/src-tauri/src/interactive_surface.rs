use std::fs::{self, OpenOptions};
use std::io::Write;
use std::mem;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, TryLockError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, FindWindowW,
    GetAncestor, GetClassNameW, GetClientRect, GetDesktopWindow, GetForegroundWindow, GetParent,
    GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, PostMessageW, SendMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    CHILDID_SELF, EVENT_OBJECT_HIDE, EVENT_OBJECT_REORDER, EVENT_OBJECT_SHOW, GA_ROOT,
    GWLP_WNDPROC, GWL_EXSTYLE, GWL_STYLE, GW_HWNDNEXT, GW_HWNDPREV, GW_OWNER, HWND_BOTTOM,
    HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, MA_NOACTIVATE, OBJID_CLIENT, OBJID_WINDOW,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSENDCHANGING,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, WINDOWPOS, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_APP, WM_MOUSEACTIVATE, WM_WINDOWPOSCHANGING, WNDPROC, WS_CAPTION,
    WS_CHILD, WS_DISABLED, WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME,
    WS_EX_NOACTIVATE, WS_EX_STATICEDGE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
};

use crate::runtime::WidgetSurfaceBoundsReport;

pub const SURFACE_LABEL: &str = "widget-surface";
const SURFACE_READY_TIMEOUT: Duration = Duration::from_secs(10);
const GEOMETRY_TOLERANCE_PX: i32 = 1;
const MAX_DESKTOP_BAND_WINDOWS: usize = 4_096;
const PRESENTATION_DIAGNOSTICS_ENV: &str = "PARLEY_VIEWER_PRESENTATION_DIAGNOSTICS";
const WM_REASSERT_DESKTOP_BAND: u32 = WM_APP + 0x32A;
const WM_INSTALL_DESKTOP_BAND_HOOK: u32 = WM_APP + 0x32B;
const WM_REMOVE_DESKTOP_BAND_HOOK: u32 = WM_APP + 0x32C;
const REASSERT_ORIGIN_POINTER: u64 = 1;
const REASSERT_ORIGIN_WIN_EVENT: u64 = 1 << 1;

static ORIGINAL_SURFACE_PROC: AtomicIsize = AtomicIsize::new(0);
static CONTROLLED_SURFACE_POSITION: AtomicBool = AtomicBool::new(false);
static SURFACE_REASSERT_PENDING: AtomicBool = AtomicBool::new(false);
static SURFACE_REASSERT_ORIGINS: AtomicU64 = AtomicU64::new(0);
static POINTER_REASSERT_SCHEDULED: AtomicU64 = AtomicU64::new(0);
static POINTER_REASSERT_APPLIED: AtomicU64 = AtomicU64::new(0);
static POINTER_REASSERT_SKIPPED: AtomicU64 = AtomicU64::new(0);
static BAND_EVENT_HOOK: AtomicIsize = AtomicIsize::new(0);
static BAND_EVENT_SURFACE_HWND: AtomicIsize = AtomicIsize::new(0);
static BAND_EVENT_OBSERVED: AtomicU64 = AtomicU64::new(0);
static BAND_EVENT_ACCEPTED: AtomicU64 = AtomicU64::new(0);
static BAND_EVENT_SCHEDULED: AtomicU64 = AtomicU64::new(0);
static BAND_EVENT_COALESCED: AtomicU64 = AtomicU64::new(0);
static BAND_EVENT_POST_FAILED: AtomicU64 = AtomicU64::new(0);
static BAND_EVENT_HOOK_INSTALL_FAILED: AtomicU64 = AtomicU64::new(0);
static BAND_REASSERT_NOOP: AtomicU64 = AtomicU64::new(0);
static BAND_REASSERT_RESTACKED: AtomicU64 = AtomicU64::new(0);
static BAND_REASSERT_RESCHEDULED: AtomicU64 = AtomicU64::new(0);
static BAND_REASSERT_HIDDEN: AtomicU64 = AtomicU64::new(0);
static SURFACE_POSITION_LOCK: Mutex<()> = Mutex::new(());
static LAST_MOUSEACTIVATE_FOREGROUND: AtomicIsize = AtomicIsize::new(0);
static BAND_HELPER_HWND: AtomicIsize = AtomicIsize::new(0);
static SURFACE_CREATED_AT: Mutex<Option<Instant>> = Mutex::new(None);
static PROCESS_STARTED: OnceLock<Instant> = OnceLock::new();

struct ControlledSurfacePositionGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ControlledSurfacePositionGuard {
    fn enter() -> Self {
        let lock = SURFACE_POSITION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::from_lock(lock)
    }

    fn try_enter() -> Option<Self> {
        match SURFACE_POSITION_LOCK.try_lock() {
            Ok(lock) => Some(Self::from_lock(lock)),
            Err(TryLockError::Poisoned(poisoned)) => Some(Self::from_lock(poisoned.into_inner())),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    fn from_lock(lock: std::sync::MutexGuard<'static, ()>) -> Self {
        CONTROLLED_SURFACE_POSITION.store(true, Ordering::Release);
        Self { _lock: lock }
    }
}

impl Drop for ControlledSurfacePositionGuard {
    fn drop(&mut self) {
        CONTROLLED_SURFACE_POSITION.store(false, Ordering::Release);
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceActivityPhase {
    Poll,
    DomPaint,
    AnimationFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentVisibility {
    Visible,
    Hidden,
    Prerender,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WidgetSurfaceActivityReport {
    pub phase: SurfaceActivityPhase,
    pub sequence: u64,
    pub generation: u64,
    pub changed: bool,
    pub document_visibility: DocumentVisibility,
    pub monotonic_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SurfaceActivityDiagnostic {
    schema_version: u8,
    timestamp_ms: u64,
    phase: SurfaceActivityPhase,
    sequence: u64,
    generation: u64,
    changed: bool,
    document_visibility: DocumentVisibility,
    renderer_monotonic_ms: u64,
    surface_visible: bool,
    surface_previous: String,
    surface_next: String,
    next_process_id: Option<u32>,
    next_class: Option<String>,
    foreground: String,
    z_order_valid: Option<bool>,
    pointer_reassert_scheduled: u64,
    pointer_reassert_applied: u64,
    pointer_reassert_skipped: u64,
    band_event_hook_active: bool,
    band_event_observed: u64,
    band_event_accepted: u64,
    band_event_scheduled: u64,
    band_event_coalesced: u64,
    band_event_post_failed: u64,
    band_event_hook_install_failed: u64,
    band_reassert_noop: u64,
    band_reassert_restacked: u64,
    band_reassert_rescheduled: u64,
    band_reassert_hidden: u64,
}

pub fn monotonic_ms() -> u64 {
    PROCESS_STARTED
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub fn ensure_surface<R: Runtime>(
    app: &AppHandle<R>,
    rect: SurfaceRect,
    scale_factor: f64,
) -> Result<WebviewWindow<R>, String> {
    if let Some(surface) = app.get_webview_window(SURFACE_LABEL) {
        return Ok(surface);
    }
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return Err("widget-surface scale factor is invalid".to_string());
    }

    let surface = WebviewWindowBuilder::new(
        app,
        SURFACE_LABEL,
        WebviewUrl::App("index.html?view=widget-surface".into()),
    )
    .title("Parley conversation")
    .inner_size(
        f64::from(rect.width) / scale_factor,
        f64::from(rect.height) / scale_factor,
    )
    .position(
        f64::from(rect.x) / scale_factor,
        f64::from(rect.y) / scale_factor,
    )
    .decorations(false)
    .shadow(false)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .always_on_top(false)
    .skip_taskbar(true)
    .visible(true)
    .build()
    .map_err(|error| format!("failed to create widget surface: {error}"))?;

    let configure_result = (|| {
        let hwnd = raw_webview_hwnd(&surface)?;
        configure_surface_styles(hwnd)?;
        install_surface_window_proc(hwnd)?;
        verify_surface_styles(hwnd)?;
        request_desktop_band_event_hook_install(hwnd);
        Ok(())
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
    let underlay_rect = physical_client_rect(underlay_hwnd)?;
    derive_surface_rect_from_physical(underlay_rect, report)
}

fn physical_client_rect(hwnd: HWND) -> Result<RECT, String> {
    let mut client_rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client_rect) } == 0 {
        return Err(format!(
            "failed to read desktop-underlay client geometry: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut origin = POINT {
        x: client_rect.left,
        y: client_rect.top,
    };
    if unsafe { ClientToScreen(hwnd, &mut origin) } == 0 {
        return Err(format!(
            "failed to map desktop-underlay client geometry: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(screen_rect_from_client(client_rect, origin))
}

fn screen_rect_from_client(client_rect: RECT, origin: POINT) -> RECT {
    let width = client_rect.right.saturating_sub(client_rect.left);
    let height = client_rect.bottom.saturating_sub(client_rect.top);
    RECT {
        left: origin.x,
        top: origin.y,
        right: origin.x.saturating_add(width),
        bottom: origin.y.saturating_add(height),
    }
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
        return Err(format!(
            "widget-surface viewport does not match the desktop underlay client area (client={underlay_width}x{underlay_height}, reported={reported_width}x{reported_height})"
        ));
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

    verify_helper_styles(helper_hwnd)?;
    verify_surface_styles(surface_hwnd)?;
    position_desktop_pair(surface_hwnd, helper_hwnd, rect, false)?;
    reveal_surface(surface)?;
    configure_surface_styles(surface_hwnd)?;
    position_desktop_pair(surface_hwnd, helper_hwnd, rect, true)?;
    notify_surface_position_changed(surface)?;
    verify_surface(app, rect)
}

pub fn restack_surface<R: Runtime>(
    app: &AppHandle<R>,
    surface: &WebviewWindow<R>,
    rect: SurfaceRect,
) -> Result<(), String> {
    let helper_hwnd = band_helper()?;
    let surface_hwnd = raw_webview_hwnd(surface)?;
    verify_helper_styles(helper_hwnd)?;
    verify_surface_styles(surface_hwnd)?;
    position_desktop_pair(surface_hwnd, helper_hwnd, rect, false)?;
    notify_surface_position_changed(surface)?;
    verify_surface(app, rect)
}

pub fn recover_surface_z_order<R: Runtime>(
    app: &AppHandle<R>,
    surface: &WebviewWindow<R>,
    rect: SurfaceRect,
) -> Result<(), String> {
    let recovery = (|| {
        let helper = band_helper()?;
        let surface_hwnd = raw_webview_hwnd(surface)?;
        let icon_host = desktop_icon_host()?;
        verify_helper_styles(helper)?;
        verify_surface_styles(surface_hwnd)?;

        let band_state = surface_band_state(surface_hwnd, icon_host)?;
        match surface_band_recovery(band_state) {
            SurfaceBandRecovery::RebuildBand => {
                if unsafe { IsWindowVisible(surface_hwnd) } != 0 {
                    hide_surface(app)?;
                }
                position_and_restack(app, surface, rect)?;
                Ok(())
            }
            SurfaceBandRecovery::Restack => {
                if unsafe { IsWindowVisible(surface_hwnd) } == 0 {
                    position_and_restack(app, surface, rect)?;
                } else {
                    restack_surface(app, surface, rect)?;
                }
                Ok(())
            }
        }
    })();

    match recovery {
        Ok(outcome) => Ok(outcome),
        Err(error) => match hide_surface(app) {
            Ok(()) => Err(error),
            Err(hide_error) => Err(format!("{error}; {hide_error}")),
        },
    }
}

fn reveal_surface<R: Runtime>(surface: &WebviewWindow<R>) -> Result<(), String> {
    surface
        .show()
        .map_err(|error| format!("failed to show widget surface: {error}"))?;
    surface
        .as_ref()
        .show()
        .map_err(|error| format!("failed to show widget-surface webview: {error}"))
}

fn position_desktop_pair(
    surface: HWND,
    helper: HWND,
    rect: SurfaceRect,
    show: bool,
) -> Result<(), String> {
    let _controlled_position = ControlledSurfacePositionGuard::enter();
    position_desktop_pair_locked(surface, helper, rect, show)
}

fn position_desktop_pair_locked(
    surface: HWND,
    helper: HWND,
    rect: SurfaceRect,
    show: bool,
) -> Result<(), String> {
    let icon_host = desktop_icon_host()?;
    let anchor = desktop_band_anchor(icon_host, helper, surface)?;
    position_desktop_pair_after_locked(surface, helper, anchor, rect, show)
}

fn position_desktop_pair_after_locked(
    surface: HWND,
    helper: HWND,
    anchor: HWND,
    rect: SurfaceRect,
    show: bool,
) -> Result<(), String> {
    let mut flags = SWP_NOACTIVATE;
    if show {
        flags |= SWP_SHOWWINDOW;
    }
    let helper_positioned = unsafe {
        SetWindowPos(
            helper,
            anchor,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
        )
    };
    if helper_positioned == 0 {
        return Err(format!(
            "failed to place desktop-band helper above Explorer: {}",
            std::io::Error::last_os_error()
        ));
    }
    let surface_positioned = unsafe {
        SetWindowPos(
            surface,
            helper,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            flags,
        )
    };
    if surface_positioned == 0 {
        return Err(format!(
            "failed to position widget surface: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn desktop_band_anchor(icon_host: HWND, helper: HWND, surface: HWND) -> Result<HWND, String> {
    let shell_process_id = window_process_id(icon_host)?;
    let mut anchor = unsafe { GetWindow(icon_host, GW_HWNDPREV) };
    for _ in 0..MAX_DESKTOP_BAND_WINDOWS {
        if anchor.is_null() {
            return Err("Explorer icon host has no safe preceding z-order anchor".to_string());
        }
        match classify_anchor_candidate(
            anchor,
            helper,
            surface,
            window_is_topmost(anchor),
            shell_taskbar_window(anchor, shell_process_id),
        ) {
            AnchorCandidate::SkipOwned => {}
            AnchorCandidate::UseWindow => return Ok(anchor),
            AnchorCandidate::RejectTopmost => {
                return Err(
                    "Explorer icon host has no safe non-topmost preceding z-order anchor"
                        .to_string(),
                );
            }
        }
        anchor = unsafe { GetWindow(anchor, GW_HWNDPREV) };
    }
    Err("desktop-band anchor traversal exceeded its bound".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnchorCandidate {
    SkipOwned,
    UseWindow,
    RejectTopmost,
}

fn classify_anchor_candidate(
    candidate: HWND,
    helper: HWND,
    surface: HWND,
    topmost: bool,
    exact_shell_taskbar: bool,
) -> AnchorCandidate {
    if candidate == helper || candidate == surface || exact_shell_taskbar {
        AnchorCandidate::SkipOwned
    } else if topmost {
        AnchorCandidate::RejectTopmost
    } else {
        AnchorCandidate::UseWindow
    }
}

fn window_is_topmost(window: HWND) -> bool {
    unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0 }
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
    verify_helper_precedes_surface(helper_hwnd, surface_hwnd, icon_host)?;
    verify_windows_below_surface(surface_hwnd, icon_host)?;

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
    let client = physical_client_rect(surface_hwnd)?;
    let client_width = client.right.saturating_sub(client.left);
    let client_height = client.bottom.saturating_sub(client.top);
    if (client.left - expected.x).abs() > GEOMETRY_TOLERANCE_PX
        || (client.top - expected.y).abs() > GEOMETRY_TOLERANCE_PX
        || (client_width - expected.width).abs() > GEOMETRY_TOLERANCE_PX
        || (client_height - expected.height).abs() > GEOMETRY_TOLERANCE_PX
    {
        return Err(format!(
            "widget-surface client geometry differs from its validated column (expected={},{} {}x{}; actual={},{} {}x{})",
            expected.x,
            expected.y,
            expected.width,
            expected.height,
            client.left,
            client.top,
            client_width,
            client_height
        ));
    }
    Ok(())
}

fn verify_helper_precedes_surface(
    helper: HWND,
    surface: HWND,
    icon_host: HWND,
) -> Result<(), String> {
    let surface_process_id = window_process_id(surface)?;
    let shell_process_id = window_process_id(icon_host)?;
    let mut current = unsafe { GetWindow(helper, GW_HWNDNEXT) };
    let mut traversed = 0usize;
    while !current.is_null() && current != surface {
        if traversed >= MAX_DESKTOP_BAND_WINDOWS {
            return Err("desktop-band helper traversal exceeded its bound".to_string());
        }
        if !safe_intervening_window(current, surface_process_id)
            && !shell_proxy_window(current, shell_process_id)
            && !shell_taskbar_window(current, shell_process_id)
        {
            return Err(format!(
                "visible restored window {} separates the desktop-band helper from the widget surface",
                describe_window(current)
            ));
        }
        current = unsafe { GetWindow(current, GW_HWNDNEXT) };
        traversed = traversed.saturating_add(1);
    }
    if current != surface {
        return Err(format!(
            "desktop-band helper no longer precedes the widget surface (helper={}, surface={})",
            format_handle(helper as isize),
            format_handle(surface as isize)
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceBandState {
    Valid,
    OrdinaryWindowBelow(HWND),
    IconHostMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceBandRecovery {
    Restack,
    RebuildBand,
}

fn surface_band_recovery(state: SurfaceBandState) -> SurfaceBandRecovery {
    match state {
        SurfaceBandState::Valid => SurfaceBandRecovery::Restack,
        SurfaceBandState::OrdinaryWindowBelow(_) | SurfaceBandState::IconHostMissing => {
            SurfaceBandRecovery::RebuildBand
        }
    }
}

fn surface_band_state(surface: HWND, icon_host: HWND) -> Result<SurfaceBandState, String> {
    let surface_process_id = window_process_id(surface)?;
    let shell_process_id = window_process_id(icon_host)?;
    let mut current = unsafe { GetWindow(surface, GW_HWNDNEXT) };
    let mut traversed = 0usize;
    while !current.is_null() && current != icon_host {
        if traversed >= MAX_DESKTOP_BAND_WINDOWS {
            return Err("widget surface desktop-band traversal exceeded its bound".to_string());
        }
        if !safe_intervening_window(current, surface_process_id)
            && !shell_taskbar_window(current, shell_process_id)
        {
            return Ok(SurfaceBandState::OrdinaryWindowBelow(current));
        }
        current = unsafe { GetWindow(current, GW_HWNDNEXT) };
        traversed = traversed.saturating_add(1);
    }
    if current != icon_host {
        return Ok(SurfaceBandState::IconHostMissing);
    }
    Ok(SurfaceBandState::Valid)
}

fn verify_windows_below_surface(surface: HWND, icon_host: HWND) -> Result<(), String> {
    match surface_band_state(surface, icon_host)? {
        SurfaceBandState::Valid => Ok(()),
        SurfaceBandState::OrdinaryWindowBelow(window) => Err(format!(
            "visible restored window {} is below the widget surface",
            describe_window(window)
        )),
        SurfaceBandState::IconHostMissing => {
            Err("widget surface is not above the Explorer icon host".to_string())
        }
    }
}

fn window_is_below(surface: HWND, candidate: HWND) -> bool {
    if candidate.is_null() || candidate == surface {
        return false;
    }
    let mut current = unsafe { GetWindow(surface, GW_HWNDNEXT) };
    for _ in 0..MAX_DESKTOP_BAND_WINDOWS {
        if current.is_null() {
            return false;
        }
        if current == candidate {
            return true;
        }
        current = unsafe { GetWindow(current, GW_HWNDNEXT) };
    }
    false
}

fn safe_intervening_window(window: HWND, surface_process_id: u32) -> bool {
    let visible = unsafe { IsWindowVisible(window) } != 0;
    let minimized = unsafe { IsIconic(window) } != 0;
    let cloaked = window_is_cloaked(window);
    let same_process = window_process_id(window)
        .map(|process_id| process_id == surface_process_id)
        .unwrap_or(false);
    let inert_infrastructure = inert_infrastructure_window(window);
    safe_intervening_window_properties(
        visible,
        minimized,
        cloaked,
        same_process,
        inert_infrastructure,
    )
}

fn safe_intervening_window_properties(
    visible: bool,
    minimized: bool,
    cloaked: bool,
    same_process: bool,
    inert_infrastructure: bool,
) -> bool {
    !visible || minimized || cloaked || (same_process && inert_infrastructure)
}

fn window_is_cloaked(window: HWND) -> bool {
    let mut cloaked = 0u32;
    let result = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED as u32,
            std::ptr::addr_of_mut!(cloaked).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    result >= 0 && cloaked != 0
}

fn inert_infrastructure_window(window: HWND) -> bool {
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) } as u32;
    let extended = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } as u32;
    let required_extended = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
    if style & WS_POPUP == 0
        || extended & required_extended != required_extended
        || extended & WS_EX_APPWINDOW != 0
    {
        return false;
    }
    let mut bounds = RECT::default();
    if unsafe { GetWindowRect(window, &mut bounds) } == 0 {
        return false;
    }
    bounds.right.saturating_sub(bounds.left) <= 32 && bounds.bottom.saturating_sub(bounds.top) <= 32
}

fn shell_proxy_window(window: HWND, shell_process_id: u32) -> bool {
    let same_shell_process = window_process_id(window)
        .map(|process_id| process_id == shell_process_id)
        .unwrap_or(false);
    let class = window_class(window);
    shell_proxy_window_properties(same_shell_process, class.as_deref())
}

fn shell_proxy_window_properties(same_shell_process: bool, class: Option<&str>) -> bool {
    same_shell_process && class == Some("ProxyModalWindow")
}

fn shell_taskbar_window(window: HWND, shell_process_id: u32) -> bool {
    let same_shell_process = window_process_id(window)
        .map(|process_id| process_id == shell_process_id)
        .unwrap_or(false);
    let class = window_class(window);
    shell_taskbar_window_properties(same_shell_process, class.as_deref())
}

fn shell_taskbar_window_properties(same_shell_process: bool, class: Option<&str>) -> bool {
    same_shell_process && class == Some("Shell_TrayWnd")
}

fn window_process_id(window: HWND) -> Result<u32, String> {
    let mut process_id = 0u32;
    let thread_id = unsafe { GetWindowThreadProcessId(window, &mut process_id) };
    if thread_id == 0 || process_id == 0 {
        Err(format!(
            "failed to resolve window process identity: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(process_id)
    }
}

fn window_class(window: HWND) -> Option<String> {
    if window.is_null() {
        return None;
    }
    let mut buffer = [0u16; 128];
    let length = unsafe { GetClassNameW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    (length > 0).then(|| String::from_utf16_lossy(&buffer[..length as usize]))
}

fn describe_window(window: HWND) -> String {
    if window.is_null() {
        return "unavailable".to_string();
    }
    let process_id = window_process_id(window)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "unavailable".to_string());
    let class = window_class(window).unwrap_or_else(|| "unavailable".to_string());
    format!(
        "{} (pid={process_id}, class={class})",
        format_handle(window as isize)
    )
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub fn hide_surface<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let Some(surface) = app.get_webview_window(SURFACE_LABEL) else {
        return Ok(());
    };
    let hwnd = raw_webview_hwnd(&surface)?;
    hide_surface_window(hwnd)
}

fn hide_surface_window(hwnd: HWND) -> Result<(), String> {
    let _controlled_position = ControlledSurfacePositionGuard::enter();
    hide_surface_window_now(hwnd)
}

fn hide_surface_window_now(hwnd: HWND) -> Result<(), String> {
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
    }
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        Ok(())
    } else {
        Err("widget surface remained visible after a hide request".to_string())
    }
}

pub fn destroy_surface<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let Some(surface) = app.get_webview_window(SURFACE_LABEL) else {
        *lock_surface_created() = None;
        return Ok(());
    };
    hide_surface(app)?;
    let hwnd = raw_webview_hwnd(&surface)?;
    remove_desktop_band_event_hook(hwnd)?;
    restore_surface_window_proc(hwnd)?;
    surface
        .destroy()
        .map_err(|error| format!("failed to destroy widget surface: {error}"))?;
    LAST_MOUSEACTIVATE_FOREGROUND.store(0, Ordering::Release);
    SURFACE_REASSERT_PENDING.store(false, Ordering::Release);
    SURFACE_REASSERT_ORIGINS.store(0, Ordering::Release);
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

pub fn record_surface_activity<R: Runtime>(
    app: &AppHandle<R>,
    report: WidgetSurfaceActivityReport,
) -> Result<(), String> {
    validate_surface_activity(report)?;
    if !presentation_diagnostics_enabled() {
        return Ok(());
    }

    let surface = app
        .get_webview_window(SURFACE_LABEL)
        .ok_or_else(|| "widget surface is unavailable".to_string())?;
    let surface_hwnd = raw_webview_hwnd(&surface)?;
    let previous = unsafe { GetWindow(surface_hwnd, GW_HWNDPREV) };
    let next = unsafe { GetWindow(surface_hwnd, GW_HWNDNEXT) };
    let z_order_valid = desktop_icon_host()
        .ok()
        .map(|icon_host| verify_windows_below_surface(surface_hwnd, icon_host).is_ok());
    let diagnostic = SurfaceActivityDiagnostic {
        schema_version: 1,
        timestamp_ms: unix_timestamp_ms(),
        phase: report.phase,
        sequence: report.sequence,
        generation: report.generation,
        changed: report.changed,
        document_visibility: report.document_visibility,
        renderer_monotonic_ms: report.monotonic_ms,
        surface_visible: unsafe { IsWindowVisible(surface_hwnd) } != 0,
        surface_previous: format_handle(previous as isize),
        surface_next: format_handle(next as isize),
        next_process_id: window_process_id(next).ok(),
        next_class: window_class(next),
        foreground: format_handle(unsafe { GetForegroundWindow() } as isize),
        z_order_valid,
        pointer_reassert_scheduled: POINTER_REASSERT_SCHEDULED.load(Ordering::Acquire),
        pointer_reassert_applied: POINTER_REASSERT_APPLIED.load(Ordering::Acquire),
        pointer_reassert_skipped: POINTER_REASSERT_SKIPPED.load(Ordering::Acquire),
        band_event_hook_active: BAND_EVENT_HOOK.load(Ordering::Acquire) != 0,
        band_event_observed: BAND_EVENT_OBSERVED.load(Ordering::Acquire),
        band_event_accepted: BAND_EVENT_ACCEPTED.load(Ordering::Acquire),
        band_event_scheduled: BAND_EVENT_SCHEDULED.load(Ordering::Acquire),
        band_event_coalesced: BAND_EVENT_COALESCED.load(Ordering::Acquire),
        band_event_post_failed: BAND_EVENT_POST_FAILED.load(Ordering::Acquire),
        band_event_hook_install_failed: BAND_EVENT_HOOK_INSTALL_FAILED.load(Ordering::Acquire),
        band_reassert_noop: BAND_REASSERT_NOOP.load(Ordering::Acquire),
        band_reassert_restacked: BAND_REASSERT_RESTACKED.load(Ordering::Acquire),
        band_reassert_rescheduled: BAND_REASSERT_RESCHEDULED.load(Ordering::Acquire),
        band_reassert_hidden: BAND_REASSERT_HIDDEN.load(Ordering::Acquire),
    };
    let directory = app.path().app_local_data_dir().map_err(|error| {
        format!("failed to resolve presentation diagnostics directory: {error}")
    })?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("failed to create presentation diagnostics directory: {error}"))?;
    let path = directory.join("interactive-presentation.jsonl");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("failed to open presentation diagnostics: {error}"))?;
    serde_json::to_writer(&mut file, &diagnostic)
        .map_err(|error| format!("failed to serialize presentation diagnostics: {error}"))?;
    file.write_all(b"\n")
        .and_then(|_| file.flush())
        .map_err(|error| format!("failed to append presentation diagnostics: {error}"))
}

fn validate_surface_activity(report: WidgetSurfaceActivityReport) -> Result<(), String> {
    if report.sequence == 0 {
        return Err("widget-surface activity sequence must be positive".to_string());
    }
    if report.generation > report.sequence {
        return Err("widget-surface activity generation exceeds its sequence".to_string());
    }
    Ok(())
}

fn presentation_diagnostics_enabled() -> bool {
    std::env::var(PRESENTATION_DIAGNOSTICS_ENV)
        .ok()
        .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"))
}

fn notify_surface_position_changed<R: Runtime>(surface: &WebviewWindow<R>) -> Result<(), String> {
    surface
        .with_webview(|webview| unsafe {
            let _ = webview.controller().NotifyParentWindowPositionChanged();
        })
        .map_err(|error| format!("failed to notify WebView2 of widget-surface position: {error}"))
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
    let frame_styles = WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
    let edge_styles = WS_EX_CLIENTEDGE | WS_EX_DLGMODALFRAME | WS_EX_STATICEDGE | WS_EX_WINDOWEDGE;
    (
        (style & !(WS_CHILD | frame_styles)) | WS_POPUP,
        (extended & !(WS_EX_APPWINDOW | WS_EX_TOPMOST | WS_EX_TRANSPARENT | edge_styles))
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
    let prohibited_style =
        WS_CHILD | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
    let required_extended = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let prohibited_extended = WS_EX_APPWINDOW
        | WS_EX_TOPMOST
        | WS_EX_TRANSPARENT
        | WS_EX_CLIENTEDGE
        | WS_EX_DLGMODALFRAME
        | WS_EX_STATICEDGE
        | WS_EX_WINDOWEDGE;
    if style & required_style != required_style
        || style & prohibited_style != 0
        || extended & required_extended != required_extended
        || extended & prohibited_extended != 0
    {
        return Err(format!(
            "widget-surface native style invariant failed (style=0x{style:X}, extended=0x{extended:X})"
        ));
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

fn restore_surface_window_proc(window: HWND) -> Result<(), String> {
    let original = ORIGINAL_SURFACE_PROC.load(Ordering::Acquire);
    if original == 0 || unsafe { IsWindow(window) } == 0 {
        ORIGINAL_SURFACE_PROC.store(0, Ordering::Release);
        return Ok(());
    }
    let current = unsafe { GetWindowLongPtrW(window, GWLP_WNDPROC) };
    let expected = surface_window_proc as *const () as usize as isize;
    if current != expected {
        return Err("widget-surface no-activate guard was replaced unexpectedly".to_string());
    }
    let previous = unsafe { SetWindowLongPtrW(window, GWLP_WNDPROC, original) };
    if previous == 0 {
        return Err(format!(
            "failed to restore widget-surface window procedure: {}",
            std::io::Error::last_os_error()
        ));
    }
    ORIGINAL_SURFACE_PROC.store(0, Ordering::Release);
    Ok(())
}

unsafe extern "system" fn surface_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_INSTALL_DESKTOP_BAND_HOOK {
        return if install_desktop_band_event_hook(window) {
            1
        } else {
            0
        };
    }
    if message == WM_REMOVE_DESKTOP_BAND_HOOK {
        return if uninstall_desktop_band_event_hook() {
            1
        } else {
            0
        };
    }
    let z_order_request = if message == WM_WINDOWPOSCHANGING && lparam != 0 {
        classify_surface_z_order_request(window, &*(lparam as *const WINDOWPOS))
    } else {
        SurfaceZOrderRequest::NoChange
    };
    match surface_message_policy(
        message,
        CONTROLLED_SURFACE_POSITION.load(Ordering::Acquire),
        lparam != 0,
        z_order_request,
    ) {
        SurfaceMessagePolicy::ScheduleReassert => {
            LAST_MOUSEACTIVATE_FOREGROUND.store(GetForegroundWindow() as isize, Ordering::Release);
            match schedule_surface_reassert(window, REASSERT_ORIGIN_POINTER) {
                ReassertScheduleOutcome::Scheduled => {
                    POINTER_REASSERT_SCHEDULED.fetch_add(1, Ordering::AcqRel);
                }
                ReassertScheduleOutcome::Coalesced => {}
                ReassertScheduleOutcome::PostFailed => {
                    POINTER_REASSERT_SKIPPED.fetch_add(1, Ordering::AcqRel);
                }
            }
            return MA_NOACTIVATE as LRESULT;
        }
        SurfaceMessagePolicy::RunReassert => {
            let origins = SURFACE_REASSERT_ORIGINS.swap(0, Ordering::AcqRel);
            SURFACE_REASSERT_PENDING.store(false, Ordering::Release);
            apply_desktop_band_reassert(window, origins);
            return 0;
        }
        SurfaceMessagePolicy::PreserveZOrder => {
            let position = &mut *(lparam as *mut WINDOWPOS);
            position.flags |= SWP_NOZORDER;
        }
        SurfaceMessagePolicy::Forward => {}
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceMessagePolicy {
    ScheduleReassert,
    RunReassert,
    PreserveZOrder,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceZOrderRequest {
    NoChange,
    Downward,
    UpwardOrUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReassertScheduleOutcome {
    Scheduled,
    Coalesced,
    PostFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopBandReassertOutcome {
    Noop,
    Restacked,
    Reschedule,
    Hide,
}

fn surface_message_policy(
    message: u32,
    controlled_position: bool,
    has_position: bool,
    z_order_request: SurfaceZOrderRequest,
) -> SurfaceMessagePolicy {
    if message == WM_MOUSEACTIVATE {
        SurfaceMessagePolicy::ScheduleReassert
    } else if message == WM_REASSERT_DESKTOP_BAND {
        SurfaceMessagePolicy::RunReassert
    } else if message == WM_WINDOWPOSCHANGING
        && !controlled_position
        && has_position
        && z_order_request == SurfaceZOrderRequest::UpwardOrUnknown
    {
        SurfaceMessagePolicy::PreserveZOrder
    } else {
        SurfaceMessagePolicy::Forward
    }
}

fn classify_surface_z_order_request(surface: HWND, position: &WINDOWPOS) -> SurfaceZOrderRequest {
    let insert_after = position.hwndInsertAfter;
    let proven_safe_downward = if insert_after_requires_safe_downward_probe(insert_after) {
        safe_downward_insert_after(surface, insert_after)
    } else {
        false
    };
    classify_surface_z_order_request_properties(position.flags, insert_after, proven_safe_downward)
}

fn classify_surface_z_order_request_properties(
    flags: u32,
    insert_after: HWND,
    proven_safe_downward: bool,
) -> SurfaceZOrderRequest {
    if flags & SWP_NOZORDER != 0 || insert_after == HWND_NOTOPMOST {
        SurfaceZOrderRequest::NoChange
    } else if insert_after == HWND_BOTTOM {
        SurfaceZOrderRequest::Downward
    } else if insert_after.is_null() || insert_after == HWND_TOPMOST {
        SurfaceZOrderRequest::UpwardOrUnknown
    } else if proven_safe_downward {
        SurfaceZOrderRequest::Downward
    } else {
        SurfaceZOrderRequest::UpwardOrUnknown
    }
}

fn insert_after_requires_safe_downward_probe(insert_after: HWND) -> bool {
    !insert_after.is_null()
        && insert_after != HWND_BOTTOM
        && insert_after != HWND_NOTOPMOST
        && insert_after != HWND_TOPMOST
}

fn safe_downward_insert_after(surface: HWND, insert_after: HWND) -> bool {
    if insert_after.is_null()
        || insert_after == surface
        || window_is_topmost(insert_after)
        || !window_is_below(surface, insert_after)
    {
        return false;
    }
    let Ok(icon_host) = desktop_icon_host() else {
        return false;
    };
    if insert_after == icon_host {
        return true;
    }
    let Ok(surface_process_id) = window_process_id(surface) else {
        return false;
    };
    let Ok(shell_process_id) = window_process_id(icon_host) else {
        return false;
    };
    let mut current = unsafe { GetWindow(insert_after, GW_HWNDNEXT) };
    for _ in 0..MAX_DESKTOP_BAND_WINDOWS {
        if current.is_null() {
            return false;
        }
        if current == icon_host {
            return true;
        }
        if !safe_intervening_window(current, surface_process_id)
            && !shell_taskbar_window(current, shell_process_id)
        {
            return false;
        }
        current = unsafe { GetWindow(current, GW_HWNDNEXT) };
    }
    false
}

fn request_desktop_band_event_hook_install(window: HWND) {
    if unsafe { PostMessageW(window, WM_INSTALL_DESKTOP_BAND_HOOK, 0, 0) } == 0 {
        BAND_EVENT_HOOK_INSTALL_FAILED.fetch_add(1, Ordering::AcqRel);
    }
}

fn remove_desktop_band_event_hook(window: HWND) -> Result<(), String> {
    if unsafe { SendMessageW(window, WM_REMOVE_DESKTOP_BAND_HOOK, 0, 0) } == 0 {
        Err("failed to remove the desktop-band WinEvent hook on its owning thread".to_string())
    } else {
        Ok(())
    }
}

fn install_desktop_band_event_hook(window: HWND) -> bool {
    let existing = BAND_EVENT_HOOK.load(Ordering::Acquire) as HWINEVENTHOOK;
    if !existing.is_null() {
        if BAND_EVENT_SURFACE_HWND.load(Ordering::Acquire) == window as isize {
            return true;
        }
        if !uninstall_desktop_band_event_hook() {
            BAND_EVENT_HOOK_INSTALL_FAILED.fetch_add(1, Ordering::AcqRel);
            return false;
        }
    }

    BAND_EVENT_SURFACE_HWND.store(window as isize, Ordering::Release);
    let hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_REORDER,
            std::ptr::null_mut(),
            Some(desktop_band_win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if hook.is_null() {
        BAND_EVENT_SURFACE_HWND.store(0, Ordering::Release);
        BAND_EVENT_HOOK_INSTALL_FAILED.fetch_add(1, Ordering::AcqRel);
        false
    } else {
        BAND_EVENT_HOOK.store(hook as isize, Ordering::Release);
        true
    }
}

fn uninstall_desktop_band_event_hook() -> bool {
    let hook = BAND_EVENT_HOOK.load(Ordering::Acquire) as HWINEVENTHOOK;
    if hook.is_null() {
        BAND_EVENT_SURFACE_HWND.store(0, Ordering::Release);
        return true;
    }
    if unsafe { UnhookWinEvent(hook) } == 0 {
        return false;
    }
    BAND_EVENT_HOOK.store(0, Ordering::Release);
    BAND_EVENT_SURFACE_HWND.store(0, Ordering::Release);
    true
}

unsafe extern "system" fn desktop_band_win_event_proc(
    hook: HWINEVENTHOOK,
    event: u32,
    event_window: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time_ms: u32,
) {
    if hook as isize != BAND_EVENT_HOOK.load(Ordering::Acquire) {
        return;
    }
    BAND_EVENT_OBSERVED.fetch_add(1, Ordering::AcqRel);
    let surface = BAND_EVENT_SURFACE_HWND.load(Ordering::Acquire) as HWND;
    let helper = BAND_HELPER_HWND.load(Ordering::Acquire) as HWND;
    let desktop = GetDesktopWindow();
    let root = if event_window.is_null() {
        std::ptr::null_mut()
    } else {
        GetAncestor(event_window, GA_ROOT)
    };
    let event = DesktopBandEvent {
        kind: event,
        window: event_window,
        id_object,
        id_child,
        root,
    };
    let windows = DesktopBandWindows {
        surface,
        helper,
        desktop,
    };
    if !desktop_band_event_should_schedule(event, windows) {
        return;
    }
    BAND_EVENT_ACCEPTED.fetch_add(1, Ordering::AcqRel);
    match schedule_surface_reassert(surface, REASSERT_ORIGIN_WIN_EVENT) {
        ReassertScheduleOutcome::Scheduled => {
            BAND_EVENT_SCHEDULED.fetch_add(1, Ordering::AcqRel);
        }
        ReassertScheduleOutcome::Coalesced => {
            BAND_EVENT_COALESCED.fetch_add(1, Ordering::AcqRel);
        }
        ReassertScheduleOutcome::PostFailed => {
            BAND_EVENT_POST_FAILED.fetch_add(1, Ordering::AcqRel);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DesktopBandEvent {
    kind: u32,
    window: HWND,
    id_object: i32,
    id_child: i32,
    root: HWND,
}

#[derive(Debug, Clone, Copy)]
struct DesktopBandWindows {
    surface: HWND,
    helper: HWND,
    desktop: HWND,
}

fn desktop_band_event_should_schedule(
    event: DesktopBandEvent,
    windows: DesktopBandWindows,
) -> bool {
    if windows.surface.is_null()
        || event.window.is_null()
        || event.window == windows.surface
        || event.window == windows.helper
        || event.id_child != CHILDID_SELF as i32
    {
        return false;
    }
    match event.kind {
        EVENT_OBJECT_SHOW => event.id_object == OBJID_WINDOW && event.root == event.window,
        EVENT_OBJECT_HIDE => false,
        EVENT_OBJECT_REORDER => {
            event.window == windows.desktop
                && matches!(event.id_object, OBJID_CLIENT | OBJID_WINDOW)
        }
        _ => false,
    }
}

pub fn reassert_surface_after_pointer<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let surface = app
        .get_webview_window(SURFACE_LABEL)
        .ok_or_else(|| "widget surface is unavailable".to_string())?;
    let surface_hwnd = raw_webview_hwnd(&surface)?;
    LAST_MOUSEACTIVATE_FOREGROUND
        .store(unsafe { GetForegroundWindow() } as isize, Ordering::Release);
    POINTER_REASSERT_SCHEDULED.fetch_add(1, Ordering::AcqRel);
    let outcome = reassert_desktop_band(surface_hwnd);
    record_desktop_band_reassert_outcome(REASSERT_ORIGIN_POINTER, outcome);
    match outcome {
        DesktopBandReassertOutcome::Noop | DesktopBandReassertOutcome::Restacked => Ok(()),
        DesktopBandReassertOutcome::Reschedule => {
            match schedule_surface_reassert(surface_hwnd, REASSERT_ORIGIN_POINTER) {
                ReassertScheduleOutcome::Scheduled | ReassertScheduleOutcome::Coalesced => Ok(()),
                ReassertScheduleOutcome::PostFailed => {
                    Err("widget surface pointer reassertion could not be rescheduled".to_string())
                }
            }
        }
        DesktopBandReassertOutcome::Hide => match hide_surface_window_now(surface_hwnd) {
            Ok(()) => Err("widget surface pointer reassertion did not verify".to_string()),
            Err(error) => Err(format!(
                "widget surface pointer reassertion did not verify; {error}"
            )),
        },
    }
}

fn schedule_surface_reassert(window: HWND, origin: u64) -> ReassertScheduleOutcome {
    SURFACE_REASSERT_ORIGINS.fetch_or(origin, Ordering::AcqRel);
    if SURFACE_REASSERT_PENDING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return ReassertScheduleOutcome::Coalesced;
    }
    if unsafe { PostMessageW(window, WM_REASSERT_DESKTOP_BAND, 0, 0) } == 0 {
        SURFACE_REASSERT_PENDING.store(false, Ordering::Release);
        SURFACE_REASSERT_ORIGINS.store(0, Ordering::Release);
        ReassertScheduleOutcome::PostFailed
    } else {
        ReassertScheduleOutcome::Scheduled
    }
}

fn apply_desktop_band_reassert(surface: HWND, origins: u64) {
    let outcome = reassert_desktop_band(surface);
    record_desktop_band_reassert_outcome(origins, outcome);
    match outcome {
        DesktopBandReassertOutcome::Noop | DesktopBandReassertOutcome::Restacked => {}
        DesktopBandReassertOutcome::Reschedule => {
            let retry_origins = if origins == 0 {
                REASSERT_ORIGIN_WIN_EVENT
            } else {
                origins
            };
            let _ = schedule_surface_reassert(surface, retry_origins);
        }
        DesktopBandReassertOutcome::Hide => {
            let _ = hide_surface_window_now(surface);
        }
    }
}

fn record_desktop_band_reassert_outcome(origins: u64, outcome: DesktopBandReassertOutcome) {
    match outcome {
        DesktopBandReassertOutcome::Noop => {
            BAND_REASSERT_NOOP.fetch_add(1, Ordering::AcqRel);
        }
        DesktopBandReassertOutcome::Restacked => {
            BAND_REASSERT_RESTACKED.fetch_add(1, Ordering::AcqRel);
        }
        DesktopBandReassertOutcome::Reschedule => {
            BAND_REASSERT_RESCHEDULED.fetch_add(1, Ordering::AcqRel);
        }
        DesktopBandReassertOutcome::Hide => {
            BAND_REASSERT_HIDDEN.fetch_add(1, Ordering::AcqRel);
        }
    }
    if origins & REASSERT_ORIGIN_POINTER != 0 {
        match outcome {
            DesktopBandReassertOutcome::Noop | DesktopBandReassertOutcome::Restacked => {
                POINTER_REASSERT_APPLIED.fetch_add(1, Ordering::AcqRel);
            }
            DesktopBandReassertOutcome::Reschedule => {}
            DesktopBandReassertOutcome::Hide => {
                POINTER_REASSERT_SKIPPED.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

fn reassert_disposition(
    band_valid: bool,
    lock_acquired: bool,
    recovery_verified: bool,
) -> DesktopBandReassertOutcome {
    if band_valid {
        DesktopBandReassertOutcome::Noop
    } else if !lock_acquired {
        DesktopBandReassertOutcome::Reschedule
    } else if recovery_verified {
        DesktopBandReassertOutcome::Restacked
    } else {
        DesktopBandReassertOutcome::Hide
    }
}

fn reassert_desktop_band(surface: HWND) -> DesktopBandReassertOutcome {
    let helper = BAND_HELPER_HWND.load(Ordering::Acquire) as HWND;
    if surface.is_null()
        || helper.is_null()
        || unsafe { IsWindow(surface) } == 0
        || unsafe { IsWindow(helper) } == 0
    {
        return DesktopBandReassertOutcome::Hide;
    }
    if unsafe { IsWindowVisible(surface) } == 0 {
        return DesktopBandReassertOutcome::Noop;
    }
    let Ok(icon_host) = desktop_icon_host() else {
        return DesktopBandReassertOutcome::Hide;
    };
    let band_valid = verify_helper_styles(helper).is_ok()
        && verify_surface_styles(surface).is_ok()
        && verify_helper_precedes_surface(helper, surface, icon_host).is_ok()
        && verify_windows_below_surface(surface, icon_host).is_ok();
    if band_valid {
        return reassert_disposition(true, false, false);
    }
    let Some(_controlled_position) = ControlledSurfacePositionGuard::try_enter() else {
        return reassert_disposition(false, false, false);
    };
    let mut bounds = RECT::default();
    if unsafe { GetWindowRect(surface, &mut bounds) } == 0 {
        return reassert_disposition(false, true, false);
    }
    let rect = SurfaceRect {
        x: bounds.left,
        y: bounds.top,
        width: bounds.right.saturating_sub(bounds.left),
        height: bounds.bottom.saturating_sub(bounds.top),
    };
    if rect.width <= 0
        || rect.height <= 0
        || position_desktop_pair_locked(surface, helper, rect, false).is_err()
    {
        return reassert_disposition(false, true, false);
    }
    let recovered = verify_helper_styles(helper).is_ok()
        && verify_surface_styles(surface).is_ok()
        && verify_helper_precedes_surface(helper, surface, icon_host).is_ok()
        && verify_windows_below_surface(surface, icon_host).is_ok();
    reassert_disposition(false, true, recovered)
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
            WS_CHILD | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX,
            WS_EX_APPWINDOW
                | WS_EX_TOPMOST
                | WS_EX_TRANSPARENT
                | WS_EX_CLIENTEDGE
                | WS_EX_DLGMODALFRAME
                | WS_EX_STATICEDGE
                | WS_EX_WINDOWEDGE,
        );

        assert_eq!(style & WS_CHILD, 0);
        assert_eq!(style & WS_POPUP, WS_POPUP);
        assert_eq!(
            style & (WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX),
            0
        );
        assert_eq!(
            extended & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE),
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
        );
        assert_eq!(
            extended
                & (WS_EX_APPWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_TRANSPARENT
                    | WS_EX_CLIENTEDGE
                    | WS_EX_DLGMODALFRAME
                    | WS_EX_STATICEDGE
                    | WS_EX_WINDOWEDGE),
            0
        );
    }

    #[test]
    fn controlled_position_guard_holds_the_lock_and_resets_state() {
        assert!(!CONTROLLED_SURFACE_POSITION.load(Ordering::Acquire));
        {
            let _guard = ControlledSurfacePositionGuard::enter();
            assert!(CONTROLLED_SURFACE_POSITION.load(Ordering::Acquire));
            assert!(SURFACE_POSITION_LOCK.try_lock().is_err());
            assert!(ControlledSurfacePositionGuard::try_enter().is_none());
        }
        assert!(!CONTROLLED_SURFACE_POSITION.load(Ordering::Acquire));
        assert!(SURFACE_POSITION_LOCK.try_lock().is_ok());
    }

    #[test]
    fn desktop_band_allows_only_noncovering_intervening_windows() {
        assert!(safe_intervening_window_properties(
            false, false, false, false, false
        ));
        assert!(safe_intervening_window_properties(
            false, true, false, false, false
        ));
        assert!(safe_intervening_window_properties(
            true, true, false, false, false
        ));
        assert!(safe_intervening_window_properties(
            true, false, true, false, false
        ));
        assert!(safe_intervening_window_properties(
            true, false, false, true, true
        ));
        assert!(!safe_intervening_window_properties(
            true, false, false, false, true
        ));
        assert!(!safe_intervening_window_properties(
            true, false, false, true, false
        ));
    }

    #[test]
    fn desktop_band_allows_only_the_exact_explorer_proxy_between_helper_and_surface() {
        assert!(shell_proxy_window_properties(
            true,
            Some("ProxyModalWindow")
        ));
        assert!(!shell_proxy_window_properties(
            false,
            Some("ProxyModalWindow")
        ));
        assert!(!shell_proxy_window_properties(
            true,
            Some("ProxyModalWindowHost")
        ));
        assert!(!shell_proxy_window_properties(
            true,
            Some("proxymodalwindow")
        ));
        assert!(!shell_proxy_window_properties(true, None));
    }

    #[test]
    fn desktop_band_allows_only_the_exact_explorer_taskbar() {
        assert!(shell_taskbar_window_properties(true, Some("Shell_TrayWnd")));
        assert!(!shell_taskbar_window_properties(
            false,
            Some("Shell_TrayWnd")
        ));
        assert!(!shell_taskbar_window_properties(
            true,
            Some("Shell_SecondaryTrayWnd")
        ));
        assert!(!shell_taskbar_window_properties(
            true,
            Some("CabinetWClass")
        ));
        assert!(!shell_taskbar_window_properties(
            true,
            Some("ProxyModalWindow")
        ));
        assert!(!shell_taskbar_window_properties(
            true,
            Some("shell_traywnd")
        ));
        assert!(!shell_taskbar_window_properties(true, None));
    }

    #[test]
    fn desktop_band_anchor_uses_ordinary_and_rejects_topmost() {
        let helper = 1usize as HWND;
        let surface = 2usize as HWND;
        let ordinary = 3usize as HWND;
        let topmost = 4usize as HWND;

        assert_eq!(
            classify_anchor_candidate(helper, helper, surface, false, false),
            AnchorCandidate::SkipOwned
        );
        assert_eq!(
            classify_anchor_candidate(surface, helper, surface, false, false),
            AnchorCandidate::SkipOwned
        );
        assert_eq!(
            classify_anchor_candidate(ordinary, helper, surface, false, false),
            AnchorCandidate::UseWindow
        );
        assert_eq!(
            classify_anchor_candidate(topmost, helper, surface, true, false),
            AnchorCandidate::RejectTopmost
        );
        assert_eq!(
            classify_anchor_candidate(ordinary, helper, surface, false, true),
            AnchorCandidate::SkipOwned
        );
        assert_eq!(
            classify_anchor_candidate(topmost, helper, surface, true, true),
            AnchorCandidate::SkipOwned
        );
    }

    #[test]
    fn mouse_activation_defers_one_nonactivating_reassert() {
        assert_eq!(
            surface_message_policy(
                WM_MOUSEACTIVATE,
                false,
                false,
                SurfaceZOrderRequest::NoChange,
            ),
            SurfaceMessagePolicy::ScheduleReassert
        );
        assert_eq!(
            surface_message_policy(
                WM_REASSERT_DESKTOP_BAND,
                false,
                false,
                SurfaceZOrderRequest::NoChange,
            ),
            SurfaceMessagePolicy::RunReassert
        );
        assert_eq!(
            surface_message_policy(
                WM_WINDOWPOSCHANGING,
                false,
                true,
                SurfaceZOrderRequest::UpwardOrUnknown,
            ),
            SurfaceMessagePolicy::PreserveZOrder
        );
        assert_eq!(
            surface_message_policy(
                WM_WINDOWPOSCHANGING,
                true,
                true,
                SurfaceZOrderRequest::UpwardOrUnknown,
            ),
            SurfaceMessagePolicy::Forward
        );
        assert_eq!(
            surface_message_policy(
                WM_WINDOWPOSCHANGING,
                false,
                false,
                SurfaceZOrderRequest::UpwardOrUnknown,
            ),
            SurfaceMessagePolicy::Forward
        );
        assert_eq!(
            surface_message_policy(
                WM_WINDOWPOSCHANGING,
                false,
                true,
                SurfaceZOrderRequest::Downward,
            ),
            SurfaceMessagePolicy::Forward
        );
    }

    #[test]
    fn uncontrolled_surface_moves_allow_only_a_proven_safe_tail() {
        let helper = 3usize as HWND;
        let ordinary = 4usize as HWND;

        assert!(!insert_after_requires_safe_downward_probe(HWND_TOP));
        assert!(!insert_after_requires_safe_downward_probe(HWND_TOPMOST));
        assert!(!insert_after_requires_safe_downward_probe(HWND_BOTTOM));
        assert!(!insert_after_requires_safe_downward_probe(HWND_NOTOPMOST));
        assert!(insert_after_requires_safe_downward_probe(ordinary));

        assert_eq!(
            classify_surface_z_order_request_properties(SWP_NOZORDER, ordinary, false),
            SurfaceZOrderRequest::NoChange
        );
        assert_eq!(
            classify_surface_z_order_request_properties(0, HWND_BOTTOM, false),
            SurfaceZOrderRequest::Downward
        );
        assert_eq!(
            classify_surface_z_order_request_properties(0, HWND_NOTOPMOST, false),
            SurfaceZOrderRequest::NoChange
        );
        assert_eq!(
            classify_surface_z_order_request_properties(0, HWND_TOP, false),
            SurfaceZOrderRequest::UpwardOrUnknown
        );
        assert_eq!(
            classify_surface_z_order_request_properties(0, HWND_TOPMOST, false),
            SurfaceZOrderRequest::UpwardOrUnknown
        );
        assert_eq!(
            classify_surface_z_order_request_properties(0, helper, false),
            SurfaceZOrderRequest::UpwardOrUnknown
        );
        assert_eq!(
            classify_surface_z_order_request_properties(0, ordinary, true),
            SurfaceZOrderRequest::Downward
        );
        assert_eq!(
            classify_surface_z_order_request_properties(0, ordinary, false),
            SurfaceZOrderRequest::UpwardOrUnknown
        );
    }

    #[test]
    fn desktop_band_reassertion_is_noop_restack_reschedule_or_hide() {
        assert_eq!(
            reassert_disposition(true, false, false),
            DesktopBandReassertOutcome::Noop
        );
        assert_eq!(
            reassert_disposition(false, true, true),
            DesktopBandReassertOutcome::Restacked
        );
        assert_eq!(
            reassert_disposition(false, false, false),
            DesktopBandReassertOutcome::Reschedule
        );
        assert_eq!(
            reassert_disposition(false, true, false),
            DesktopBandReassertOutcome::Hide
        );
    }

    #[test]
    fn desktop_band_events_accept_only_top_level_show_and_desktop_reorder() {
        let surface = 1usize as HWND;
        let helper = 2usize as HWND;
        let desktop = 3usize as HWND;
        let top_level = 4usize as HWND;
        let child = 5usize as HWND;
        let windows = DesktopBandWindows {
            surface,
            helper,
            desktop,
        };
        let should_schedule = |kind, window, id_object, id_child, root| -> bool {
            desktop_band_event_should_schedule(
                DesktopBandEvent {
                    kind,
                    window,
                    id_object,
                    id_child,
                    root,
                },
                windows,
            )
        };

        assert!(should_schedule(
            EVENT_OBJECT_SHOW,
            top_level,
            OBJID_WINDOW,
            CHILDID_SELF as i32,
            top_level,
        ));
        assert!(!should_schedule(
            EVENT_OBJECT_SHOW,
            child,
            OBJID_WINDOW,
            CHILDID_SELF as i32,
            top_level,
        ));
        assert!(should_schedule(
            EVENT_OBJECT_REORDER,
            desktop,
            OBJID_CLIENT,
            CHILDID_SELF as i32,
            desktop,
        ));
        assert!(should_schedule(
            EVENT_OBJECT_REORDER,
            desktop,
            OBJID_WINDOW,
            CHILDID_SELF as i32,
            desktop,
        ));
        assert!(!should_schedule(
            EVENT_OBJECT_REORDER,
            child,
            OBJID_CLIENT,
            CHILDID_SELF as i32,
            child,
        ));
        assert!(!should_schedule(
            EVENT_OBJECT_HIDE,
            top_level,
            OBJID_WINDOW,
            CHILDID_SELF as i32,
            top_level,
        ));
        assert!(!should_schedule(
            EVENT_OBJECT_SHOW,
            surface,
            OBJID_WINDOW,
            CHILDID_SELF as i32,
            surface,
        ));
        assert!(!should_schedule(
            EVENT_OBJECT_SHOW,
            helper,
            OBJID_WINDOW,
            CHILDID_SELF as i32,
            helper,
        ));
        assert!(!should_schedule(
            EVENT_OBJECT_SHOW,
            top_level,
            OBJID_CLIENT,
            CHILDID_SELF as i32,
            top_level,
        ));
        assert!(!should_schedule(
            EVENT_OBJECT_SHOW,
            top_level,
            OBJID_WINDOW,
            1,
            top_level,
        ));
    }

    #[test]
    fn invalid_surface_band_states_rebuild_from_the_icon_host() {
        let ordinary = 4usize as HWND;

        assert_eq!(
            surface_band_recovery(SurfaceBandState::Valid),
            SurfaceBandRecovery::Restack
        );
        assert_eq!(
            surface_band_recovery(SurfaceBandState::OrdinaryWindowBelow(ordinary)),
            SurfaceBandRecovery::RebuildBand
        );
        assert_eq!(
            surface_band_recovery(SurfaceBandState::IconHostMissing),
            SurfaceBandRecovery::RebuildBand
        );
    }

    #[test]
    fn surface_activity_requires_monotonic_identifiers() {
        let valid = WidgetSurfaceActivityReport {
            phase: SurfaceActivityPhase::Poll,
            sequence: 2,
            generation: 1,
            changed: false,
            document_visibility: DocumentVisibility::Hidden,
            monotonic_ms: 5_000,
        };
        assert!(validate_surface_activity(valid).is_ok());
        assert!(validate_surface_activity(WidgetSurfaceActivityReport {
            sequence: 0,
            ..valid
        })
        .is_err());
        assert!(validate_surface_activity(WidgetSurfaceActivityReport {
            sequence: 2,
            generation: 3,
            ..valid
        })
        .is_err());
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
    fn client_geometry_excludes_the_underlay_frame() {
        let client = RECT {
            left: 0,
            top: 0,
            right: 544,
            bottom: 351,
        };
        let screen = screen_rect_from_client(client, POINT { x: 1_344, y: 25 });
        let report = WidgetSurfaceBoundsReport {
            left: 112.0,
            top: 20.0,
            width: 320.0,
            height: 300.0,
            viewport_width: 544.0,
            viewport_height: 351.0,
            device_pixel_ratio: 1.0,
        };

        assert_eq!(
            derive_surface_rect_from_physical(screen, report).unwrap(),
            SurfaceRect {
                x: 1_456,
                y: 45,
                width: 320,
                height: 300,
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
