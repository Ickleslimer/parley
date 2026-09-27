use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::event_engine::{Diagnostics, EngineStatus, EventEngine, SourceState, SourceStatus};
use crate::settings::{
    save_settings, validate_source_path, Corner, DesktopMode, SettingsFile, ViewerSettings,
};
use crate::widget_browser::WidgetBrowser;

const MAX_ATTACH_ATTEMPTS: u8 = 6;
const ATTACH_RETRY_MS: u64 = 1_000;
const ATTACH_COOLDOWN_MS: u64 = 15_000;
const HEALTH_CHECK_MS: u64 = 2_000;
const SURFACE_CORRECTION_WINDOW_MS: u64 = 10_000;
const MAX_SURFACE_CORRECTIONS: usize = 3;
const MIN_SURFACE_WIDTH: f64 = 176.0;
const MIN_SURFACE_HEIGHT: f64 = 132.0;
const SURFACE_BOUNDS_EPSILON: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UnderlayState {
    Detached,
    Attaching,
    Attached,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesktopRuntimeState {
    Passive,
    InteractiveStarting,
    Interactive,
    PassiveFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesktopFallbackReason {
    DevelopmentGateClosed,
    PreferencePassive,
    UnderlayUnavailable,
    SurfaceCreateFailed,
    SurfaceDocumentNotReady,
    SurfaceBoundsInvalid,
    SurfaceStyleInvalid,
    SurfaceOwnerInvalid,
    SurfaceGeometryMismatch,
    SurfaceZOrderInvalid,
    SurfaceRestackLimit,
    ExplorerLost,
    SurfaceDestroyFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerStatus {
    pub source_state: SourceState,
    pub generation: u64,
    pub bytes_read: u64,
    pub session_count: u64,
    pub exchange_count: u64,
    pub last_event_timestamp_ms: Option<u64>,
    pub tray_available: bool,
    pub underlay_state: UnderlayState,
    pub desktop_runtime_state: DesktopRuntimeState,
    pub desktop_fallback_reason: Option<DesktopFallbackReason>,
    pub widget_visible: bool,
    pub diagnostics: Diagnostics,
    pub sources: Vec<SourceStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub id: String,
    pub name: String,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetSurfaceBoundsReport {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub device_pixel_ratio: f64,
}

impl WidgetSurfaceBoundsReport {
    pub fn validated(self) -> Result<Self, String> {
        let values = [
            self.left,
            self.top,
            self.width,
            self.height,
            self.viewport_width,
            self.viewport_height,
            self.device_pixel_ratio,
        ];
        if values.iter().any(|value| !value.is_finite()) {
            return Err("widget-surface bounds must contain only finite values".to_string());
        }
        if self.left < 0.0 || self.top < 0.0 {
            return Err("widget-surface bounds must start inside the widget viewport".to_string());
        }
        if self.width < MIN_SURFACE_WIDTH || self.height < MIN_SURFACE_HEIGHT {
            return Err(
                "widget-surface bounds are too small for the interaction controls".to_string(),
            );
        }
        if self.viewport_width <= 0.0
            || self.viewport_height <= 0.0
            || self.device_pixel_ratio <= 0.0
        {
            return Err("widget-surface viewport and scale must be positive".to_string());
        }
        if self.left + self.width > self.viewport_width + SURFACE_BOUNDS_EPSILON
            || self.top + self.height > self.viewport_height + SURFACE_BOUNDS_EPSILON
        {
            return Err("widget-surface bounds extend outside the widget viewport".to_string());
        }
        Ok(self)
    }
}

#[derive(Debug, Clone)]
struct RuntimeFlags {
    tray_available: bool,
    underlay_state: UnderlayState,
    desktop_runtime_state: DesktopRuntimeState,
    desktop_fallback_reason: Option<DesktopFallbackReason>,
    widget_requested: bool,
    surface_bounds: Option<WidgetSurfaceBoundsReport>,
    surface_ready: bool,
    surface_fallback_latched: bool,
    surface_corrections_ms: VecDeque<u64>,
    exiting: bool,
    last_error: Option<String>,
}

impl Default for RuntimeFlags {
    fn default() -> Self {
        Self {
            tray_available: false,
            underlay_state: UnderlayState::Detached,
            desktop_runtime_state: DesktopRuntimeState::Passive,
            desktop_fallback_reason: None,
            widget_requested: true,
            surface_bounds: None,
            surface_ready: false,
            surface_fallback_latched: false,
            surface_corrections_ms: VecDeque::new(),
            exiting: false,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub tray_available: bool,
    pub underlay_state: UnderlayState,
    pub widget_requested: bool,
    pub desktop_runtime_state: DesktopRuntimeState,
    pub desktop_fallback_reason: Option<DesktopFallbackReason>,
    pub surface_ready: bool,
    pub surface_fallback_latched: bool,
    pub exiting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InteractiveSnapshot {
    pub bounds: Option<WidgetSurfaceBoundsReport>,
    pub surface_ready: bool,
    pub fallback_latched: bool,
    pub state: DesktopRuntimeState,
}

#[derive(Debug)]
pub struct AppState {
    pub engine: EventEngine,
    pub widget_browser: WidgetBrowser,
    settings: Mutex<SettingsFile>,
    settings_path: PathBuf,
    runtime: Mutex<RuntimeFlags>,
    retry: Mutex<UnderlayRetry>,
    pub stop: AtomicBool,
}

impl AppState {
    pub fn new(settings_path: PathBuf, settings: SettingsFile) -> Self {
        Self {
            engine: EventEngine::new(),
            widget_browser: WidgetBrowser::new(),
            settings: Mutex::new(settings),
            settings_path,
            runtime: Mutex::new(RuntimeFlags::default()),
            retry: Mutex::new(UnderlayRetry::default()),
            stop: AtomicBool::new(false),
        }
    }

    pub fn settings(&self) -> SettingsFile {
        self.lock_settings().clone()
    }

    pub fn save_placement(&self, incoming: ViewerSettings) -> Result<ViewerSettings, String> {
        let current = self.settings();
        let mut next = incoming.sanitized();
        next.selected_log = current.viewer.selected_log;
        next.selected_logs = current.viewer.selected_logs;
        next.launch_at_login = current.viewer.launch_at_login;
        let updated = SettingsFile {
            viewer: next.clone(),
            autostart_initialized: current.autostart_initialized,
            settings_version: current.settings_version,
        };
        save_settings(&self.settings_path, &updated)?;
        *self.lock_settings() = updated;
        Ok(next)
    }

    pub fn initialize_desktop_mode(&self, mode: DesktopMode, gate_open: bool) {
        let mut runtime = self.lock_runtime();
        match (mode, gate_open) {
            (DesktopMode::Passive, _) => {
                runtime.desktop_runtime_state = DesktopRuntimeState::Passive;
                runtime.desktop_fallback_reason = Some(DesktopFallbackReason::PreferencePassive);
            }
            (DesktopMode::Interactive, false) => {
                runtime.desktop_runtime_state = DesktopRuntimeState::PassiveFallback;
                runtime.desktop_fallback_reason =
                    Some(DesktopFallbackReason::DevelopmentGateClosed);
            }
            (DesktopMode::Interactive, true) => {
                runtime.desktop_runtime_state = DesktopRuntimeState::InteractiveStarting;
                runtime.desktop_fallback_reason = None;
            }
        }
    }

    pub fn set_sources(&self, sources: Vec<PathBuf>, persist: bool) -> Result<(), String> {
        let sources = sources
            .into_iter()
            .map(validate_source_path)
            .collect::<Result<Vec<_>, _>>()?;
        if persist {
            let mut updated = self.settings();
            updated.viewer.set_logs(
                sources
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
            )?;
            save_settings(&self.settings_path, &updated)?;
            *self.lock_settings() = updated;
        }
        self.engine.set_sources(sources)?;
        self.engine.poll();
        Ok(())
    }

    pub fn add_source(&self, source: PathBuf, persist: bool) -> Result<(), String> {
        let source = validate_source_path(source)?;
        let mut added = true;
        if persist {
            let mut updated = self.settings();
            added = updated
                .viewer
                .add_log(source.to_string_lossy().into_owned())?;
            if added {
                save_settings(&self.settings_path, &updated)?;
                *self.lock_settings() = updated;
            }
        }
        if added {
            self.engine.add_source(source)?;
            self.engine.poll();
        }
        Ok(())
    }

    pub fn remove_source(&self, source: PathBuf, persist: bool) -> Result<(), String> {
        let source = validate_source_path(source)?;
        let mut removed = true;
        if persist {
            let mut updated = self.settings();
            removed = updated.viewer.remove_log(&source.to_string_lossy())?;
            if removed {
                save_settings(&self.settings_path, &updated)?;
                *self.lock_settings() = updated;
            }
        }
        if removed {
            self.engine.remove_source(&source)?;
        }
        self.engine.poll();
        Ok(())
    }

    pub fn set_launch_at_login(
        &self,
        enabled: bool,
        initialized: bool,
    ) -> Result<ViewerSettings, String> {
        let mut updated = self.settings();
        updated.viewer.launch_at_login = enabled;
        updated.autostart_initialized = initialized;
        save_settings(&self.settings_path, &updated)?;
        let viewer = updated.viewer.clone();
        *self.lock_settings() = updated;
        Ok(viewer)
    }

    pub fn status(&self) -> ViewerStatus {
        let EngineStatus {
            source_state,
            generation,
            bytes_read,
            session_count,
            exchange_count,
            last_event_timestamp_ms,
            mut diagnostics,
            sources,
        } = self.engine.status();
        let runtime = self.lock_runtime();
        if let Some(error) = runtime.last_error.as_ref() {
            diagnostics.last_error = Some(error.clone());
        }
        ViewerStatus {
            source_state,
            generation,
            bytes_read,
            session_count,
            exchange_count,
            last_event_timestamp_ms,
            tray_available: runtime.tray_available,
            underlay_state: runtime.underlay_state,
            desktop_runtime_state: runtime.desktop_runtime_state,
            desktop_fallback_reason: runtime.desktop_fallback_reason,
            widget_visible: runtime.widget_requested
                && runtime.underlay_state == UnderlayState::Attached,
            diagnostics,
            sources,
        }
    }

    pub fn runtime_snapshot(&self) -> RuntimeSnapshot {
        let runtime = self.lock_runtime();
        RuntimeSnapshot {
            tray_available: runtime.tray_available,
            underlay_state: runtime.underlay_state,
            widget_requested: runtime.widget_requested,
            desktop_runtime_state: runtime.desktop_runtime_state,
            desktop_fallback_reason: runtime.desktop_fallback_reason,
            surface_ready: runtime.surface_ready,
            surface_fallback_latched: runtime.surface_fallback_latched,
            exiting: runtime.exiting,
        }
    }

    pub fn set_tray_available(&self, available: bool) {
        let mut runtime = self.lock_runtime();
        runtime.tray_available = available;
        if !available {
            runtime.widget_requested = false;
            runtime.underlay_state = UnderlayState::Degraded;
        }
    }

    pub fn request_widget(&self, visible: bool) {
        let mut runtime = self.lock_runtime();
        runtime.widget_requested = visible && runtime.tray_available;
        if !runtime.widget_requested && runtime.tray_available {
            runtime.underlay_state = UnderlayState::Detached;
        }
    }

    pub fn set_underlay_state(&self, state: UnderlayState) {
        self.lock_runtime().underlay_state = state;
    }

    pub fn set_runtime_error(&self, error: impl Into<String>) {
        self.lock_runtime().last_error = Some(error.into());
    }

    pub fn report_surface_bounds(&self, report: WidgetSurfaceBoundsReport) -> Result<(), String> {
        self.lock_runtime().surface_bounds = Some(report.validated()?);
        Ok(())
    }

    pub fn mark_surface_ready(&self) {
        self.lock_runtime().surface_ready = true;
    }

    pub fn invalidate_surface_bounds(&self) {
        self.lock_runtime().surface_bounds = None;
    }

    pub fn interactive_snapshot(&self) -> InteractiveSnapshot {
        let runtime = self.lock_runtime();
        InteractiveSnapshot {
            bounds: runtime.surface_bounds,
            surface_ready: runtime.surface_ready,
            fallback_latched: runtime.surface_fallback_latched,
            state: runtime.desktop_runtime_state,
        }
    }

    pub fn mark_interactive_starting(&self) {
        let mut runtime = self.lock_runtime();
        runtime.desktop_runtime_state = DesktopRuntimeState::InteractiveStarting;
        runtime.desktop_fallback_reason = None;
    }

    pub fn mark_interactive(&self) {
        let mut runtime = self.lock_runtime();
        runtime.desktop_runtime_state = DesktopRuntimeState::Interactive;
        runtime.desktop_fallback_reason = None;
        runtime.surface_fallback_latched = false;
    }

    pub fn mark_surface_destroyed(&self) {
        let mut runtime = self.lock_runtime();
        runtime.surface_ready = false;
    }

    pub fn mark_surface_hidden(&self) {
        let mut runtime = self.lock_runtime();
        runtime.desktop_runtime_state = DesktopRuntimeState::Passive;
        runtime.desktop_fallback_reason = None;
        runtime.surface_ready = false;
        runtime.surface_corrections_ms.clear();
    }

    pub fn fallback_interactive(&self, reason: DesktopFallbackReason, latch: bool) {
        let mut runtime = self.lock_runtime();
        runtime.desktop_runtime_state = DesktopRuntimeState::PassiveFallback;
        runtime.desktop_fallback_reason = Some(reason);
        runtime.surface_ready = false;
        runtime.surface_fallback_latched |= latch;
    }

    pub fn mark_passive_preference(&self) {
        let mut runtime = self.lock_runtime();
        runtime.desktop_runtime_state = DesktopRuntimeState::Passive;
        runtime.desktop_fallback_reason = Some(DesktopFallbackReason::PreferencePassive);
        runtime.surface_ready = false;
        runtime.surface_fallback_latched = false;
        runtime.surface_corrections_ms.clear();
    }

    pub fn retry_interactive(&self, gate_open: bool) -> Result<(), String> {
        let mode = self.settings().viewer.desktop_mode;
        if mode != DesktopMode::Interactive {
            self.mark_passive_preference();
            return Err("interactive desktop mode is disabled in settings".to_string());
        }
        if !gate_open {
            self.fallback_interactive(DesktopFallbackReason::DevelopmentGateClosed, false);
            return Err("interactive desktop mode is behind the development gate".to_string());
        }
        let mut runtime = self.lock_runtime();
        runtime.desktop_runtime_state = DesktopRuntimeState::InteractiveStarting;
        runtime.desktop_fallback_reason = None;
        runtime.surface_ready = false;
        runtime.surface_fallback_latched = false;
        runtime.surface_corrections_ms.clear();
        Ok(())
    }

    pub fn allow_surface_correction(&self, now_ms: u64) -> bool {
        let mut runtime = self.lock_runtime();
        while runtime
            .surface_corrections_ms
            .front()
            .is_some_and(|timestamp| {
                now_ms.saturating_sub(*timestamp) >= SURFACE_CORRECTION_WINDOW_MS
            })
        {
            runtime.surface_corrections_ms.pop_front();
        }
        if runtime.surface_corrections_ms.len() >= MAX_SURFACE_CORRECTIONS {
            return false;
        }
        runtime.surface_corrections_ms.push_back(now_ms);
        true
    }

    pub fn mark_exiting(&self) {
        self.stop.store(true, Ordering::Release);
        self.lock_runtime().exiting = true;
    }

    pub fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    pub fn next_underlay_action(&self, now_ms: u64) -> UnderlayAction {
        let runtime = self.runtime_snapshot();
        self.lock_retry().next_action(now_ms, runtime)
    }

    pub fn record_attach_result(&self, now_ms: u64, succeeded: bool) {
        let state = self.lock_retry().record_attach(now_ms, succeeded);
        self.set_underlay_state(state);
    }

    pub fn record_health_result(&self, now_ms: u64, healthy: bool) {
        let state = self.lock_retry().record_health(now_ms, healthy);
        self.set_underlay_state(state);
    }

    pub fn record_detached(&self) {
        self.lock_retry().record_detached();
        if self.runtime_snapshot().tray_available {
            self.set_underlay_state(UnderlayState::Detached);
        }
    }

    fn lock_settings(&self) -> MutexGuard<'_, SettingsFile> {
        self.settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_runtime(&self) -> MutexGuard<'_, RuntimeFlags> {
        self.runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_retry(&self) -> MutexGuard<'_, UnderlayRetry> {
        self.retry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlayAction {
    None,
    Attach,
    HealthCheck,
    Detach,
}

#[derive(Debug, Default)]
struct UnderlayRetry {
    failures: u8,
    next_attempt_ms: u64,
    next_health_ms: u64,
}

impl UnderlayRetry {
    fn next_action(&mut self, now_ms: u64, runtime: RuntimeSnapshot) -> UnderlayAction {
        if runtime.exiting || !runtime.tray_available || !runtime.widget_requested {
            return if runtime.underlay_state == UnderlayState::Attached {
                UnderlayAction::Detach
            } else {
                UnderlayAction::None
            };
        }
        if runtime.underlay_state == UnderlayState::Attached {
            return if now_ms >= self.next_health_ms {
                UnderlayAction::HealthCheck
            } else {
                UnderlayAction::None
            };
        }
        if now_ms >= self.next_attempt_ms {
            UnderlayAction::Attach
        } else {
            UnderlayAction::None
        }
    }

    fn record_attach(&mut self, now_ms: u64, succeeded: bool) -> UnderlayState {
        if succeeded {
            self.failures = 0;
            self.next_health_ms = now_ms.saturating_add(HEALTH_CHECK_MS);
            UnderlayState::Attached
        } else {
            self.failures = self.failures.saturating_add(1);
            if self.failures >= MAX_ATTACH_ATTEMPTS {
                self.failures = 0;
                self.next_attempt_ms = now_ms.saturating_add(ATTACH_COOLDOWN_MS);
                UnderlayState::Degraded
            } else {
                self.next_attempt_ms = now_ms.saturating_add(ATTACH_RETRY_MS);
                UnderlayState::Attaching
            }
        }
    }

    fn record_health(&mut self, now_ms: u64, healthy: bool) -> UnderlayState {
        if healthy {
            self.next_health_ms = now_ms.saturating_add(HEALTH_CHECK_MS);
            UnderlayState::Attached
        } else {
            self.next_attempt_ms = now_ms;
            UnderlayState::Attaching
        }
    }

    fn record_detached(&mut self) {
        self.failures = 0;
        self.next_attempt_ms = 0;
        self.next_health_ms = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetPlacement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn calculate_placement(settings: &ViewerSettings, area: WorkArea) -> WidgetPlacement {
    let scale = if area.scale_factor.is_finite() && area.scale_factor > 0.0 {
        area.scale_factor
    } else {
        1.0
    };
    let width = scaled(settings.width, scale).clamp(1, area.width.max(1));
    let height = scaled(settings.height, scale).clamp(1, area.height.max(1));
    let max_x_offset = area.width.saturating_sub(width);
    let max_y_offset = area.height.saturating_sub(height);
    let offset_x = scaled(settings.offset_x, scale).min(max_x_offset);
    let offset_y = scaled(settings.offset_y, scale).min(max_y_offset);
    let left = i64::from(area.x);
    let top = i64::from(area.y);
    let right = left + i64::from(area.width.saturating_sub(width));
    let bottom = top + i64::from(area.height.saturating_sub(height));
    let x = match settings.corner {
        Corner::TopLeft | Corner::BottomLeft => left + i64::from(offset_x),
        Corner::TopRight | Corner::BottomRight => right - i64::from(offset_x),
    };
    let y = match settings.corner {
        Corner::TopLeft | Corner::TopRight => top + i64::from(offset_y),
        Corner::BottomLeft | Corner::BottomRight => bottom - i64::from(offset_y),
    };
    WidgetPlacement {
        x: clamp_i32(x),
        y: clamp_i32(y),
        width,
        height,
    }
}

fn scaled(value: f64, scale: f64) -> u32 {
    (value * scale).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

fn clamp_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_snapshot(tray_available: bool, underlay_state: UnderlayState) -> RuntimeSnapshot {
        RuntimeSnapshot {
            tray_available,
            underlay_state,
            widget_requested: true,
            desktop_runtime_state: DesktopRuntimeState::Passive,
            desktop_fallback_reason: None,
            surface_ready: false,
            surface_fallback_latched: false,
            exiting: false,
        }
    }

    #[test]
    fn tray_failure_prevents_underlay_attachment() {
        let mut retry = UnderlayRetry::default();
        let runtime = runtime_snapshot(false, UnderlayState::Detached);
        assert_eq!(retry.next_action(0, runtime), UnderlayAction::None);
    }

    #[test]
    fn attachment_attempts_are_bounded_before_cooldown() {
        let mut retry = UnderlayRetry::default();
        let runtime = runtime_snapshot(true, UnderlayState::Attaching);
        let mut now = 0;
        let mut state = UnderlayState::Attaching;
        for attempt in 0..MAX_ATTACH_ATTEMPTS {
            assert_eq!(retry.next_action(now, runtime), UnderlayAction::Attach);
            state = retry.record_attach(now, false);
            if attempt + 1 < MAX_ATTACH_ATTEMPTS {
                now += ATTACH_RETRY_MS;
            }
        }
        assert_eq!(state, UnderlayState::Degraded);
        assert_eq!(retry.next_action(now, runtime), UnderlayAction::None);
        assert_eq!(
            retry.next_action(now + ATTACH_COOLDOWN_MS, runtime),
            UnderlayAction::Attach
        );
    }

    #[test]
    fn health_loss_schedules_immediate_reattachment() {
        let mut retry = UnderlayRetry::default();
        assert_eq!(retry.record_attach(10, true), UnderlayState::Attached);
        assert_eq!(retry.record_health(2_010, false), UnderlayState::Attaching);
        let runtime = runtime_snapshot(true, UnderlayState::Attaching);
        assert_eq!(retry.next_action(2_010, runtime), UnderlayAction::Attach);
    }

    #[test]
    fn surface_bounds_fail_closed_outside_the_underlay() {
        let valid = WidgetSurfaceBoundsReport {
            left: 112.0,
            top: 20.0,
            width: 336.0,
            height: 320.0,
            viewport_width: 560.0,
            viewport_height: 360.0,
            device_pixel_ratio: 1.25,
        };
        assert_eq!(valid.validated(), Ok(valid));
        assert!(WidgetSurfaceBoundsReport {
            width: 500.0,
            ..valid
        }
        .validated()
        .is_err());
        assert!(WidgetSurfaceBoundsReport {
            device_pixel_ratio: f64::NAN,
            ..valid
        }
        .validated()
        .is_err());
    }

    #[test]
    fn surface_corrections_are_bounded_per_ten_second_window() {
        let state = AppState::new(PathBuf::from("settings.json"), SettingsFile::default());
        assert!(state.allow_surface_correction(0));
        assert!(state.allow_surface_correction(1));
        assert!(state.allow_surface_correction(2));
        assert!(!state.allow_surface_correction(3));
        assert!(state.allow_surface_correction(SURFACE_CORRECTION_WINDOW_MS));
    }

    #[test]
    fn runtime_fallback_does_not_rewrite_interactive_preference() {
        let state = AppState::new(PathBuf::from("settings.json"), SettingsFile::default());
        state.initialize_desktop_mode(DesktopMode::Interactive, true);
        state.fallback_interactive(DesktopFallbackReason::SurfaceZOrderInvalid, true);

        assert_eq!(
            state.runtime_snapshot().desktop_runtime_state,
            DesktopRuntimeState::PassiveFallback
        );
        assert_eq!(
            state.settings().viewer.desktop_mode,
            DesktopMode::Interactive
        );
    }

    #[test]
    fn placement_respects_corner_scale_and_negative_monitor_origin() {
        let settings = ViewerSettings {
            corner: Corner::BottomRight,
            offset_x: 24.0,
            offset_y: 24.0,
            width: 440.0,
            height: 260.0,
            ..ViewerSettings::default()
        };
        let placement = calculate_placement(
            &settings,
            WorkArea {
                x: -1_920,
                y: 0,
                width: 1_920,
                height: 1_040,
                scale_factor: 1.0,
            },
        );
        assert_eq!(
            placement,
            WidgetPlacement {
                x: -464,
                y: 756,
                width: 440,
                height: 260,
            }
        );
    }

    #[test]
    fn placement_clamps_oversized_widget_and_offsets() {
        let settings = ViewerSettings {
            corner: Corner::TopLeft,
            offset_x: 1_000.0,
            offset_y: 1_000.0,
            width: 2_000.0,
            height: 2_000.0,
            ..ViewerSettings::default()
        };
        let placement = calculate_placement(
            &settings,
            WorkArea {
                x: 100,
                y: 200,
                width: 800,
                height: 600,
                scale_factor: 1.25,
            },
        );
        assert_eq!(
            placement,
            WidgetPlacement {
                x: 100,
                y: 200,
                width: 800,
                height: 600,
            }
        );
    }
}
