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
            monitor_id: None,
            corner: Corner::BottomRight,
            offset_x: 24.0,
            offset_y: 24.0,
            width: 440.0,
            height: 260.0,
            launch_at_login: false,
        }
    }
}

impl ViewerSettings {
    pub fn sanitized(mut self) -> Self {
        let defaults = Self::default();
        if self
            .selected_log
            .as_ref()
            .is_some_and(|path| !Path::new(path).is_absolute())
        {
            self.selected_log = None;
        }
        self.offset_x = sanitize_number(self.offset_x, defaults.offset_x, 0.0, MAX_OFFSET);
        self.offset_y = sanitize_number(self.offset_y, defaults.offset_y, 0.0, MAX_OFFSET);
        self.width = sanitize_number(self.width, defaults.width, MIN_WIDTH, MAX_DIMENSION);
        self.height = sanitize_number(self.height, defaults.height, MIN_HEIGHT, MAX_DIMENSION);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct SettingsFile {
    #[serde(flatten)]
    pub viewer: ViewerSettings,
    pub autostart_initialized: bool,
}

impl SettingsFile {
    pub fn sanitized(mut self) -> Self {
        self.viewer = self.viewer.sanitized();
        self
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
    let bytes = serde_json::to_vec_pretty(settings)
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
        assert_eq!(settings.offset_x, 0.0);
        assert_eq!(settings.offset_y, 24.0);
        assert_eq!(settings.width, MIN_WIDTH);
        assert_eq!(settings.height, MAX_DIMENSION);
    }

    #[test]
    fn saves_and_replaces_settings_atomically() {
        let path = temp_settings_path();
        let mut first = SettingsFile::default();
        first.viewer.selected_log = Some(r"C:\logs\first.jsonl".to_string());
        save_settings(&path, &first).expect("first save should succeed");

        let mut second = first.clone();
        second.viewer.selected_log = Some(r"C:\logs\second.jsonl".to_string());
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
}
