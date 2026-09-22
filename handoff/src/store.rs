use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::fsutil::{self, FailKind};
use crate::schema::{
    ActivityRecord, HandoffState, StoredRecord, HANDOFF_SCHEMA_VERSION, MAX_ACTIVITY_EVENTS,
    MAX_EXCERPT_CHARS, MAX_JOURNAL_LINE,
};

pub fn load_readonly(job_dir: &std::path::Path) -> Result<StoredRecord, FailKind> {
    let bytes = fsutil::read_shared(&journal_path(job_dir), fsutil::bounded_journal_bytes())?;
    parse_journal(&bytes)
}

pub fn repair_snapshot(job_dir: &std::path::Path) -> Result<StoredRecord, FailKind> {
    let _lock = fsutil::acquire_lock(job_dir)?;
    let record = load_readonly(job_dir)?;
    publish_if_stale(job_dir, &record)?;
    Ok(record)
}

pub fn update<F>(
    job_dir: &std::path::Path,
    now_ms: u64,
    mutate: F,
) -> Result<StoredRecord, FailKind>
where
    F: FnOnce(&mut StoredRecord) -> Result<bool, FailKind>,
{
    let _lock = fsutil::acquire_lock(job_dir)?;
    let original = load_readonly(job_dir)?;
    publish_if_stale(job_dir, &original)?;
    let mut record = original.clone();
    let changed = mutate(&mut record)?;
    if !changed {
        return Ok(original);
    }
    if !record.same_identity(&original) {
        return Err(FailKind::Malformed);
    }
    record.sequence = original.sequence.saturating_add(1);
    if record.sequence == original.sequence {
        return Err(FailKind::Malformed);
    }
    record.updated_at_ms = now_ms;
    validate_record(&record)?;
    append_record(job_dir, &record)?;
    publish_snapshot(job_dir, &record)?;
    Ok(record)
}

fn publish_if_stale(job_dir: &std::path::Path, record: &StoredRecord) -> Result<(), FailKind> {
    let snapshot = job_dir.join("snapshot.json");
    let stale = match fsutil::read_shared(&snapshot, MAX_JOURNAL_LINE + 16) {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(text) => parse_record(text.trim())
                .map(|parsed| parsed != *record)
                .unwrap_or(true),
            Err(_) => true,
        },
        Err(FailKind::Missing) => true,
        Err(error) => return Err(error),
    };
    if stale {
        publish_snapshot(job_dir, record)?;
    }
    Ok(())
}

fn publish_snapshot(job_dir: &std::path::Path, record: &StoredRecord) -> Result<(), FailKind> {
    let mut encoded = serde_json::to_string(record).map_err(|_| FailKind::Malformed)?;
    if encoded.len() > MAX_JOURNAL_LINE || encoded.contains(['\n', '\r']) {
        return Err(FailKind::Malformed);
    }
    encoded.push('\n');
    fsutil::atomic_replace(job_dir, "snapshot.json", encoded.as_bytes())
}

fn append_record(job_dir: &std::path::Path, record: &StoredRecord) -> Result<(), FailKind> {
    let encoded = serde_json::to_string(record).map_err(|_| FailKind::Malformed)?;
    if encoded.len() > MAX_JOURNAL_LINE || encoded.contains(['\n', '\r']) {
        return Err(FailKind::Malformed);
    }
    let path = journal_path(job_dir);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004);
    }
    let mut file = options.open(&path).map_err(|error| {
        if fsutil::is_lock_denied(&error) {
            FailKind::Locked
        } else {
            FailKind::Io
        }
    })?;
    clip_incomplete(&mut file)?;
    file.seek(SeekFrom::End(0)).map_err(|_| FailKind::Io)?;
    file.write_all(encoded.as_bytes())
        .map_err(|_| FailKind::Io)?;
    file.write_all(b"\n").map_err(|_| FailKind::Io)?;
    file.flush().map_err(|_| FailKind::Io)?;
    file.sync_all().map_err(|_| FailKind::Io)?;
    Ok(())
}

fn clip_incomplete(file: &mut std::fs::File) -> Result<(), FailKind> {
    file.seek(SeekFrom::Start(0)).map_err(|_| FailKind::Io)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|_| FailKind::Io)?;
    if bytes.len() > fsutil::bounded_journal_bytes() {
        return Err(FailKind::Malformed);
    }
    let keep = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    if keep < bytes.len() {
        file.set_len(keep as u64).map_err(|_| FailKind::Io)?;
    }
    Ok(())
}

pub fn parse_journal(bytes: &[u8]) -> Result<StoredRecord, FailKind> {
    if bytes.len() > fsutil::bounded_journal_bytes() {
        return Err(FailKind::Malformed);
    }
    let Some(index) = bytes.iter().rposition(|byte| *byte == b'\n') else {
        return Err(FailKind::Missing);
    };
    let complete = &bytes[..=index];
    let text = std::str::from_utf8(complete).map_err(|_| FailKind::Malformed)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut latest: Option<StoredRecord> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_JOURNAL_LINE {
            return Err(FailKind::Malformed);
        }
        let record = parse_record(line)?;
        if let Some(previous) = &latest {
            if record.sequence != previous.sequence.saturating_add(1)
                || !record.same_identity(previous)
                || record.updated_at_ms < previous.updated_at_ms
            {
                return Err(FailKind::Malformed);
            }
        } else if record.sequence != 1 {
            return Err(FailKind::Malformed);
        }
        latest = Some(record);
    }
    latest.ok_or(FailKind::Missing)
}

fn parse_record(text: &str) -> Result<StoredRecord, FailKind> {
    let record: StoredRecord = serde_json::from_str(text).map_err(|_| FailKind::Malformed)?;
    validate_record(&record)?;
    Ok(record)
}

fn validate_record(record: &StoredRecord) -> Result<(), FailKind> {
    if record.schema_version != HANDOFF_SCHEMA_VERSION || record.sequence == 0 {
        return Err(FailKind::Malformed);
    }
    if HandoffState::parse(&record.state).is_none() {
        return Err(FailKind::Malformed);
    }
    if !bounded_id(&record.job_id)
        || !bounded_id(&record.handoff_id)
        || !option_ok(record.source_session_id.as_deref(), bounded_id)
        || !option_ok(record.target_session_id.as_deref(), bounded_id)
        || !is_sha256_hex(&record.capability_hash)
        || !bounded_text(&record.process_state, 80)
        || !bounded_text(&record.phase, 80)
        || !option_ok(record.report_fingerprint.as_deref(), is_sha256_hex)
        || !option_ok(record.alert_incident_id.as_deref(), bounded_id)
    {
        return Err(FailKind::Malformed);
    }
    if record.created_at_ms > record.updated_at_ms
        || record
            .process_id
            .is_some_and(|value| value == 0 || value > u64::from(u32::MAX))
        || record.excerpt.chars().count() > MAX_EXCERPT_CHARS
    {
        return Err(FailKind::Malformed);
    }
    let state = HandoffState::parse(&record.state).ok_or(FailKind::Malformed)?;
    if matches!(
        state,
        HandoffState::Ready
            | HandoffState::AwaitingAck
            | HandoffState::Acknowledged
            | HandoffState::DetachedUnacked
    ) && (record.report_fingerprint.is_none()
        || record.ready_at_ms.is_none()
        || record.deadline_ms.is_none())
    {
        return Err(FailKind::Malformed);
    }
    if state == HandoffState::Acknowledged && record.receipt_at_ms.is_none() {
        return Err(FailKind::Malformed);
    }
    if record.activities.len() > MAX_ACTIVITY_EVENTS {
        return Err(FailKind::Malformed);
    }
    for activity in &record.activities {
        if !valid_activity(activity) {
            return Err(FailKind::Malformed);
        }
    }
    Ok(())
}

fn valid_activity(activity: &ActivityRecord) -> bool {
    matches!(
        activity.class.as_str(),
        "thought"
            | "tool_call"
            | "tool_call_update"
            | "text"
            | "usage"
            | "plan"
            | "available_commands"
            | "end"
            | "error"
            | "unknown"
    ) && option_ok(activity.tool_name.as_deref(), |value| {
        bounded_text(value, 80)
    }) && option_ok(activity.status.as_deref(), |value| bounded_text(value, 120))
}

fn bounded_id(value: &str) -> bool {
    fsutil::valid_identifier(value)
}

fn option_ok(value: Option<&str>, check: impl Fn(&str) -> bool) -> bool {
    match value {
        Some(value) => check(value),
        None => true,
    }
}

fn bounded_text(value: &str, max_chars: usize) -> bool {
    let count = value.chars().count();
    (1..=max_chars).contains(&count) && value.chars().all(|character| !character.is_control())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn journal_path(job_dir: &std::path::Path) -> std::path::PathBuf {
    job_dir.join("journal.jsonl")
}
