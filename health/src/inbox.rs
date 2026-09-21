use std::fs;
use std::path::{Path, PathBuf};

use crate::fsutil;
use crate::paths::HealthPaths;
use crate::schema::{
    sanitize_id, HealthError, HealthRecord, InboxKind, MAX_INBOX_BYTES, SCHEMA_VERSION,
};

pub fn write_record(paths: &HealthPaths, record: &HealthRecord) -> Result<PathBuf, HealthError> {
    paths.ensure()?;
    let inbox_id = sanitize_id(&record.inbox_id)
        .ok_or_else(|| HealthError::msg("inbox_id missing or invalid"))?;
    if record.schema_version != SCHEMA_VERSION {
        return Err(HealthError::msg("inbox schema_version must be 1"));
    }
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() > MAX_INBOX_BYTES {
        return Err(HealthError::msg("inbox record exceeds bound"));
    }
    let file_name = format!("{}-{inbox_id}.json", record.as_of_ms);
    let dest = paths.inbox().join(file_name);
    fsutil::atomic_write(&dest, &bytes)?;
    Ok(dest)
}

pub fn acknowledge_record(incident_id: &str, as_of_ms: u64) -> Result<HealthRecord, HealthError> {
    let incident_id =
        sanitize_id(incident_id).ok_or_else(|| HealthError::msg("invalid incident_id"))?;
    let mut record = HealthRecord::new(
        InboxKind::Acknowledge,
        format!("ack-{incident_id}-{as_of_ms}"),
        as_of_ms,
    );
    record.source = Some(crate::schema::Source::Viewer);
    record.incident_id = Some(incident_id);
    Ok(record)
}

pub fn mute_record(muted: bool, as_of_ms: u64) -> HealthRecord {
    let mut record = HealthRecord::new(
        InboxKind::Mute,
        format!("mute-{muted}-{as_of_ms}"),
        as_of_ms,
    );
    record.source = Some(crate::schema::Source::Viewer);
    record.muted = Some(muted);
    record
}

pub fn test_sound_record(as_of_ms: u64) -> HealthRecord {
    let mut record = HealthRecord::new(
        InboxKind::TestSound,
        format!("test-sound-{as_of_ms}"),
        as_of_ms,
    );
    record.source = Some(crate::schema::Source::Viewer);
    record.test_sound = Some(true);
    record
}

pub fn list_ready(paths: &HealthPaths, limit: usize) -> Result<Vec<PathBuf>, HealthError> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(paths.inbox()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(error) => return Err(error.into()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => name,
            None => continue,
        };
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        files.push(path);
    }
    files.sort();
    files.truncate(limit);
    Ok(files)
}

pub fn read_record(path: &Path) -> Result<HealthRecord, InboxReadError> {
    let read = fsutil::read_bounded(path, MAX_INBOX_BYTES + 1)
        .map_err(|error| InboxReadError::Io(error.to_string()))?;
    if read.clipped {
        return Err(InboxReadError::Malformed(
            "inbox record exceeds bound".into(),
        ));
    }
    let parsed: HealthRecord = serde_json::from_slice(fsutil::strip_bom(&read.bytes))
        .map_err(|error| InboxReadError::Malformed(error.to_string()))?;
    if parsed.schema_version != SCHEMA_VERSION {
        return Err(InboxReadError::Malformed("unsupported inbox schema".into()));
    }
    if sanitize_id(&parsed.inbox_id).is_none() {
        return Err(InboxReadError::Malformed("invalid inbox_id".into()));
    }
    Ok(parsed)
}

pub fn quarantine(paths: &HealthPaths, path: &Path) -> Result<(), HealthError> {
    paths.ensure()?;
    let name = path
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("unknown.json"));
    let dest = unique_dest(&paths.quarantine().join(name));
    match fs::rename(path, &dest) {
        Ok(()) => Ok(()),
        Err(_) => fsutil::replace_file(path, &dest).map(|_| {
            let _ = fs::remove_file(path);
        }),
    }
}

pub fn remove_consumed(path: &Path) -> Result<(), HealthError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[derive(Debug)]
pub enum InboxReadError {
    Io(String),
    Malformed(String),
}

fn unique_dest(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("quarantine");
    let ext = path
        .extension()
        .and_then(|name| name.to_str())
        .unwrap_or("json");
    path.with_file_name(format!(
        "{stem}-{}.{ext}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}
