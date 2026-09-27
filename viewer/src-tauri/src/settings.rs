use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process;

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

const MIN_WIDTH: f64 = 320.0;
const MIN_HEIGHT: f64 = 180.0;
const MAX_DIMENSION: f64 = 16_384.0;
const MAX_OFFSET: f64 = 16_384.0;
const DEFAULT_WIDTH: f64 = 560.0;
const DEFAULT_HEIGHT: f64 = 360.0;
const LEGACY_DEFAULT_WIDTH: f64 = 440.0;
const LEGACY_DEFAULT_HEIGHT: f64 = 260.0;
const CURRENT_SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ViewerSettings {
    pub selected_log: Option<String>,
    #[serde(default)]
    pub selected_logs: Vec<String>,
    pub monitor_id: Option<String>,
    pub corner: Corner,
    pub offset_x: f64,
    pub offset_y: f64,
    pub width: f64,
    pub height: f64,
    pub launch_at_login: bool,
}

impl Default for ViewerSettings {
    fn default() -> Self {
        Self {
            selected_log: None,
            selected_logs: Vec::new(),
            monitor_id: None,
            corner: Corner::TopRight,
            offset_x: 24.0,
            offset_y: 24.0,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            launch_at_login: false,
        }
    }
}

impl ViewerSettings {
    pub fn sanitized(mut self) -> Self {
        let defaults = Self::default();
        self.migrate_logs();
        self.offset_x = sanitize_number(self.offset_x, defaults.offset_x, 0.0, MAX_OFFSET);
        self.offset_y = sanitize_number(self.offset_y, defaults.offset_y, 0.0, MAX_OFFSET);
        self.width = sanitize_number(self.width, defaults.width, MIN_WIDTH, MAX_DIMENSION);
        self.height = sanitize_number(self.height, defaults.height, MIN_HEIGHT, MAX_DIMENSION);
        self
    }

    pub fn set_logs(&mut self, logs: Vec<String>) -> Result<(), String> {
        self.selected_logs = logs
            .into_iter()
            .map(|path| {
                validate_source_path(PathBuf::from(path))
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.selected_log = self.selected_logs.first().cloned();
        Ok(())
    }

    pub fn add_log(&mut self, path: String) -> Result<bool, String> {
        let path = validate_source_path(PathBuf::from(path))?;
        let rendered = path.to_string_lossy().into_owned();
        let added = !self
            .selected_logs
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&rendered));
        if added {
            self.selected_logs.push(rendered);
        }
        self.selected_log = self.selected_logs.first().cloned();
        Ok(added)
    }

    pub fn remove_log(&mut self, path: &str) -> Result<bool, String> {
        let path = validate_source_path(Path::new(path))?;
        let rendered = path.to_string_lossy().into_owned();
        let before = self.selected_logs.len();
        self.selected_logs
            .retain(|existing| !existing.eq_ignore_ascii_case(&rendered));
        self.selected_log = self.selected_logs.first().cloned();
        Ok(self.selected_logs.len() != before)
    }

    fn migrate_logs(&mut self) {
        self.selected_logs
            .retain(|path| Path::new(path).is_absolute());
        if let Some(path) = self.selected_log.as_ref() {
            if !Path::new(path).is_absolute() {
                self.selected_log = None;
            }
        }
        if self.selected_logs.is_empty() {
            if let Some(path) = self.selected_log.clone() {
                self.selected_logs = vec![path];
            }
        }
        self.selected_log = self.selected_logs.first().cloned();
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsFile {
    #[serde(flatten)]
    pub viewer: ViewerSettings,
    #[serde(default)]
    pub autostart_initialized: bool,
    #[serde(default = "legacy_settings_version")]
    pub settings_version: u32,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            viewer: ViewerSettings::default(),
            autostart_initialized: false,
            settings_version: CURRENT_SETTINGS_VERSION,
        }
    }
}

impl SettingsFile {
    pub fn sanitized(mut self) -> Self {
        if self.settings_version < CURRENT_SETTINGS_VERSION {
            if self.viewer.width == LEGACY_DEFAULT_WIDTH
                && self.viewer.height == LEGACY_DEFAULT_HEIGHT
            {
                self.viewer.width = DEFAULT_WIDTH;
                self.viewer.height = DEFAULT_HEIGHT;
            }
            self.settings_version = CURRENT_SETTINGS_VERSION;
        }
        self.viewer = self.viewer.sanitized();
        self
    }
}

fn legacy_settings_version() -> u32 {
    0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartReconcile {
    KeepPreference,
    EnableRegistration,
    DisableRegistration,
}

pub fn reconcile_autostart(
    initialized: bool,
    saved_launch_at_login: bool,
    registration_enabled: Option<bool>,
) -> AutostartReconcile {
    if !initialized {
        return AutostartReconcile::KeepPreference;
    }
    match (saved_launch_at_login, registration_enabled) {
        (true, Some(true)) => AutostartReconcile::KeepPreference,
        (true, _) => AutostartReconcile::EnableRegistration,
        (false, Some(true)) => AutostartReconcile::DisableRegistration,
        (false, _) => AutostartReconcile::KeepPreference,
    }
}

pub fn validate_source_path(path: impl AsRef<Path>) -> Result<PathBuf, String> {
    let path = path.as_ref();
    if !path.is_absolute() {
        return Err(format!(
            "event log path must be absolute: {}",
            path.display()
        ));
    }
    Ok(path.to_path_buf())
}

pub fn load_settings(path: &Path) -> Result<SettingsFile, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<SettingsFile>(&bytes)
            .map(SettingsFile::sanitized)
            .map_err(|error| format!("failed to parse settings: {error}")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(SettingsFile::default()),
        Err(error) => Err(format!("failed to read settings: {error}")),
    }
}

pub fn save_settings(path: &Path, settings: &SettingsFile) -> Result<(), String> {
    let mut settings = settings.clone();
    settings.viewer.migrate_logs();
    let parent = path
        .parent()
        .ok_or_else(|| "settings path has no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create settings folder: {error}"))?;
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("settings.json");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", process::id()));
    let bytes = serde_json::to_vec_pretty(&settings)
        .map_err(|error| format!("failed to encode settings: {error}"))?;
    let write_result = (|| -> Result<(), String> {
        let mut file = File::create(&temp_path)
            .map_err(|error| format!("failed to create settings temp file: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("failed to write settings temp file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("failed to flush settings temp file: {error}"))?;
        move_replace(&temp_path, path)
            .map_err(|error| format!("failed to replace settings file: {error}"))
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn sanitize_number(value: f64, default: f64, minimum: f64, maximum: f64) -> f64 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        default
    }
}

fn move_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide_null(source.as_os_str());
    let destination = wide_null(destination.as_os_str());
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_settings_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("parley-viewer-settings-{}-{nonce}", process::id()))
            .join("settings.json")
    }

    #[test]
    fn sanitizes_untrusted_values() {
        let settings = ViewerSettings {
            selected_log: Some("relative.jsonl".to_string()),
            offset_x: -8.0,
            offset_y: f64::INFINITY,
            width: 10.0,
            height: 100_000.0,
            ..ViewerSettings::default()
        }
        .sanitized();

        assert_eq!(settings.selected_log, None);
        assert!(settings.selected_logs.is_empty());
        assert_eq!(settings.offset_x, 0.0);
        assert_eq!(settings.offset_y, 24.0);
        assert_eq!(settings.width, MIN_WIDTH);
        assert_eq!(settings.height, MAX_DIMENSION);
    }

    #[test]
    fn missing_settings_use_the_top_right_fresh_install_default() {
        let path = temp_settings_path();
        let settings = load_settings(&path).expect("missing settings should use defaults");

        assert_eq!(settings.viewer.corner, Corner::TopRight);
        assert_eq!(settings.viewer.offset_x, 24.0);
        assert_eq!(settings.viewer.offset_y, 24.0);
        assert_eq!(settings.viewer.width, DEFAULT_WIDTH);
        assert_eq!(settings.viewer.height, DEFAULT_HEIGHT);
    }

    #[test]
    fn preserves_an_explicit_bottom_right_choice() {
        let settings = serde_json::from_str::<SettingsFile>(
            r#"{
                "settingsVersion": 1,
                "corner": "bottom-right",
                "offsetX": 24.0,
                "offsetY": 24.0,
                "width": 560.0,
                "height": 360.0,
                "launchAtLogin": false
            }"#,
        )
        .expect("explicit settings should parse")
        .sanitized();

        assert_eq!(settings.settings_version, CURRENT_SETTINGS_VERSION);
        assert_eq!(settings.viewer.corner, Corner::BottomRight);
    }

    #[test]
    fn migrates_the_legacy_default_size_without_moving_the_widget() {
        let settings = serde_json::from_str::<SettingsFile>(
            r#"{
                "selectedLog": "C:\\logs\\events.jsonl",
                "monitorId": null,
                "corner": "top-right",
                "offsetX": 28.0,
                "offsetY": 24.0,
                "width": 440.0,
                "height": 260.0,
                "launchAtLogin": true,
                "autostartInitialized": true
            }"#,
        )
        .expect("legacy settings should parse")
        .sanitized();

        assert_eq!(settings.settings_version, CURRENT_SETTINGS_VERSION);
        assert_eq!(settings.viewer.corner, Corner::TopRight);
        assert_eq!(settings.viewer.offset_x, 28.0);
        assert_eq!(settings.viewer.offset_y, 24.0);
        assert_eq!(settings.viewer.width, DEFAULT_WIDTH);
        assert_eq!(settings.viewer.height, DEFAULT_HEIGHT);
        assert_eq!(
            settings.viewer.selected_logs,
            vec![r"C:\logs\events.jsonl".to_string()]
        );
        assert_eq!(
            settings.viewer.selected_log.as_deref(),
            Some(r"C:\logs\events.jsonl")
        );
    }

    #[test]
    fn migrates_legacy_selected_log_and_writes_both_fields() {
        let settings = serde_json::from_str::<SettingsFile>(
            r#"{
                "selectedLog": "C:\\logs\\legacy.jsonl",
                "monitorId": null,
                "corner": "bottom-right",
                "offsetX": 24.0,
                "offsetY": 24.0,
                "width": 560.0,
                "height": 360.0,
                "launchAtLogin": false
            }"#,
        )
        .expect("legacy selected log should parse")
        .sanitized();
        assert_eq!(
            settings.viewer.selected_logs,
            vec![r"C:\logs\legacy.jsonl".to_string()]
        );
        assert_eq!(
            settings.viewer.selected_log.as_deref(),
            Some(r"C:\logs\legacy.jsonl")
        );

        let path = temp_settings_path();
        save_settings(&path, &settings).expect("save should succeed");
        let raw = fs::read_to_string(&path).expect("settings should be readable");
        assert!(raw.contains("selectedLog"));
        assert!(raw.contains("selectedLogs"));
        fs::remove_dir_all(path.parent().unwrap()).expect("temp settings should be removable");
    }

    #[test]
    fn preserves_an_explicit_legacy_size_after_migration() {
        let settings = SettingsFile {
            viewer: ViewerSettings {
                width: LEGACY_DEFAULT_WIDTH,
                height: LEGACY_DEFAULT_HEIGHT,
                ..ViewerSettings::default()
            },
            settings_version: CURRENT_SETTINGS_VERSION,
            ..SettingsFile::default()
        }
        .sanitized();

        assert_eq!(settings.viewer.width, LEGACY_DEFAULT_WIDTH);
        assert_eq!(settings.viewer.height, LEGACY_DEFAULT_HEIGHT);
    }

    #[test]
    fn saves_and_replaces_settings_atomically() {
        let path = temp_settings_path();
        let mut first = SettingsFile::default();
        first
            .viewer
            .set_logs(vec![r"C:\logs\first.jsonl".to_string()])
            .unwrap();
        save_settings(&path, &first).expect("first save should succeed");

        let mut second = first.clone();
        second
            .viewer
            .set_logs(vec![
                r"C:\logs\second.jsonl".to_string(),
                r"C:\logs\third.jsonl".to_string(),
            ])
            .unwrap();
        second.autostart_initialized = true;
        save_settings(&path, &second).expect("replacement save should succeed");

        assert_eq!(load_settings(&path).expect("settings should load"), second);
        let parent = path.parent().expect("settings path should have a parent");
        assert_eq!(
            fs::read_dir(parent)
                .expect("settings folder should exist")
                .count(),
            1
        );
        fs::remove_dir_all(parent).expect("temporary settings folder should be removable");
    }

    #[test]
    fn requires_absolute_source_paths() {
        assert!(validate_source_path(r"C:\logs\events.jsonl").is_ok());
        assert!(validate_source_path("events.jsonl").is_err());
    }

    #[test]
    fn autostart_reconciliation_matrix() {
        use AutostartReconcile::{DisableRegistration, EnableRegistration, KeepPreference};

        assert_eq!(
            reconcile_autostart(false, false, Some(false)),
            KeepPreference
        );
        assert_eq!(
            reconcile_autostart(false, false, Some(true)),
            KeepPreference
        );
        assert_eq!(
            reconcile_autostart(false, true, Some(false)),
            KeepPreference
        );
        assert_eq!(reconcile_autostart(false, false, None), KeepPreference);

        assert_eq!(reconcile_autostart(true, true, Some(true)), KeepPreference);
        assert_eq!(
            reconcile_autostart(true, true, Some(false)),
            EnableRegistration
        );
        assert_eq!(reconcile_autostart(true, true, None), EnableRegistration);

        assert_eq!(
            reconcile_autostart(true, false, Some(false)),
            KeepPreference
        );
        assert_eq!(
            reconcile_autostart(true, false, Some(true)),
            DisableRegistration
        );
        assert_eq!(reconcile_autostart(true, false, None), KeepPreference);
    }

    #[test]
    fn installer_hooks_do_not_delete_viewer_config_or_settings() {
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/windows/hooks.nsh"));
        assert!(
            source.contains("!macro NSIS_HOOK_POSTUNINSTALL"),
            "NSIS_HOOK_POSTUNINSTALL must remain so settings persistence is an explicit installer contract"
        );

        let commands = nsis_active_commands(source);
        assert!(
            commands
                .iter()
                .any(|command| command.contains("!macro NSIS_HOOK_POSTUNINSTALL")),
            "parsed hooks must include NSIS_HOOK_POSTUNINSTALL"
        );
        for command in &commands {
            assert!(
                !command_deletes_viewer_config(command),
                "installer hooks must not delete com.ickleslimer.parley-viewer or settings.json; offending command: {command}"
            );
        }

        const OLD_POSTUNINSTALL_WIPE: &str = r#"
!macro NSIS_HOOK_POSTUNINSTALL
  RMDir /r "$APPDATA\com.ickleslimer.parley-viewer"
!macroend
"#;
        assert!(
            nsis_active_commands(OLD_POSTUNINSTALL_WIPE)
                .iter()
                .any(|command| command_deletes_viewer_config(command)),
            "regression predicate must reject the previous RMDir /r config wipe"
        );
    }

    fn nsis_active_commands(source: &str) -> Vec<String> {
        source
            .lines()
            .filter_map(|line| {
                let mut code = line;
                if let Some((left, _)) = code.split_once(';') {
                    code = left;
                }
                if let Some((left, _)) = code.split_once('#') {
                    code = left;
                }
                let trimmed = code.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .collect()
    }

    fn nsis_verb(command: &str) -> String {
        command
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_start_matches('!')
            .to_ascii_lowercase()
    }

    fn command_deletes_viewer_config(command: &str) -> bool {
        let verb = nsis_verb(command);
        command
            .to_ascii_lowercase()
            .contains("com.ickleslimer.parley-viewer")
            && (verb == "rmdir" || verb == "delete")
    }
}
