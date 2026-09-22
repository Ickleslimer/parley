use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::fsutil::{self, FailKind};
use crate::schema::{
    PeerActivityDocument, PeerEvent, VisibleExcerpt, MAX_EXCERPT_CHARS, NOT_INFERRED,
    PEER_ACTIVITY_SCHEMA_VERSION,
};
use crate::sha256::sha256_hex;

const MAX_ROLLOUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENTS: usize = 32;
const MAX_LINE: usize = 8 * 1024 * 1024;

pub fn read_activity(
    rollout: Option<&Path>,
    codex_home: Option<&Path>,
    source_session_id: &str,
    now_ms: u64,
) -> PeerActivityDocument {
    match resolve_rollout(rollout, codex_home, source_session_id) {
        Ok(path) => match validate_rollout_identity(&path, source_session_id)
            .and_then(|_| fsutil::read_tail(&path, MAX_ROLLOUT_BYTES))
            .and_then(|bytes| parse_rollout(&bytes, source_session_id, now_ms))
        {
            Ok(document) => document,
            Err(kind) => PeerActivityDocument::unavailable_at(kind.as_str(), now_ms),
        },
        Err(kind) => PeerActivityDocument::unavailable_at(kind.as_str(), now_ms),
    }
}

fn resolve_rollout(
    rollout: Option<&Path>,
    codex_home: Option<&Path>,
    source_session_id: &str,
) -> Result<PathBuf, FailKind> {
    if !fsutil::valid_identifier(source_session_id) {
        return Err(FailKind::Session);
    }
    if let Some(path) = rollout {
        if !path.is_absolute() {
            return Err(FailKind::Path);
        }
        let canonical = fsutil::canonical_existing(path)?;
        if let Some(home) = codex_home {
            let home = fsutil::canonical_existing(home)?;
            if !canonical.starts_with(&home) {
                return Err(FailKind::Path);
            }
        }
        return Ok(canonical);
    }
    let Some(home) = codex_home else {
        return Err(FailKind::Missing);
    };
    let home = fsutil::canonical_existing(home)?;
    let sessions = home.join("sessions");
    if !sessions.exists() {
        return Err(FailKind::Missing);
    }
    let mut matches = Vec::new();
    walk_rollouts(&sessions, &home, source_session_id, 0, &mut matches)?;
    match matches.len() {
        0 => Err(FailKind::Missing),
        1 => Ok(matches.remove(0)),
        _ => Err(FailKind::Stale),
    }
}

fn walk_rollouts(
    directory: &Path,
    root: &Path,
    session_id: &str,
    depth: usize,
    matches: &mut Vec<PathBuf>,
) -> Result<(), FailKind> {
    if depth > 6 || matches.len() > 1 {
        return Ok(());
    }
    fsutil::refuse_reparse_chain(directory)?;
    let entries = fs::read_dir(directory).map_err(|error| {
        if fsutil::is_lock_denied(&error) {
            FailKind::Locked
        } else {
            FailKind::Io
        }
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            if fsutil::is_lock_denied(&error) {
                FailKind::Locked
            } else {
                FailKind::Io
            }
        })?;
        if matches.len() > 1 {
            break;
        }
        let path = entry.path();
        if !path.starts_with(root) {
            return Err(FailKind::Path);
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = metadata.file_type().is_symlink();
        if reparse {
            continue;
        }
        if metadata.is_dir() {
            walk_rollouts(&path, root, session_id, depth + 1, matches)?;
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".jsonl") && name.contains(session_id) {
            matches.push(path);
        }
    }
    Ok(())
}

fn validate_rollout_identity(path: &Path, source_session_id: &str) -> Result<(), FailKind> {
    let bytes = fsutil::read_prefix(path, MAX_LINE)?;
    let Some(index) = bytes.iter().position(|byte| *byte == b'\n') else {
        return Err(FailKind::Malformed);
    };
    let text = std::str::from_utf8(&bytes[..=index]).map_err(|_| FailKind::Malformed)?;
    let line = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .trim_end_matches(['\r', '\n'])
        .trim();
    if line.is_empty() || line.len() > MAX_LINE {
        return Err(FailKind::Malformed);
    }
    let value: Value = serde_json::from_str(line).map_err(|_| FailKind::Malformed)?;
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Err(FailKind::Stale);
    }
    match value.pointer("/payload/id").and_then(Value::as_str) {
        Some(id) if id == source_session_id => Ok(()),
        _ => Err(FailKind::Stale),
    }
}

fn parse_rollout(
    bytes: &[u8],
    source_session_id: &str,
    now_ms: u64,
) -> Result<PeerActivityDocument, FailKind> {
    let complete = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => &bytes[..=index],
        None => return Err(FailKind::Missing),
    };
    let text = std::str::from_utf8(complete).map_err(|_| FailKind::Malformed)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut visible = String::new();
    let mut events = Vec::new();
    let mut phase = "idle";
    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_LINE {
            return Err(FailKind::Malformed);
        }
        let value: Value = serde_json::from_str(line).map_err(|_| FailKind::Malformed)?;
        let record_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        let timestamp = record_timestamp(&value);
        if record_type == "session_meta" {
            let id = value
                .pointer("/payload/id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if id != source_session_id {
                return Err(FailKind::Stale);
            }
            continue;
        }
        if record_type != "response_item" {
            continue;
        }
        let payload = value.get("payload").ok_or(FailKind::Malformed)?;
        let payload_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
        match payload_type {
            "reasoning" | "summary" => {}
            "function_call" => {
                phase = "tool";
                if let Some(timestamp_ms) = timestamp {
                    push_event(
                        &mut events,
                        PeerEvent {
                            class: "tool_call".to_string(),
                            timestamp_ms,
                            tool_name: payload
                                .get("name")
                                .and_then(Value::as_str)
                                .and_then(safe_tool_name),
                            status: Some("started".to_string()),
                        },
                    );
                }
            }
            "function_call_output" => {
                phase = "tool";
                if let Some(timestamp_ms) = timestamp {
                    push_event(
                        &mut events,
                        PeerEvent {
                            class: "tool_call".to_string(),
                            timestamp_ms,
                            tool_name: None,
                            status: Some("closed".to_string()),
                        },
                    );
                }
            }
            "message" => {
                if let Some(observed) =
                    accept_message(payload, timestamp, &mut visible, &mut events)?
                {
                    phase = observed;
                }
            }
            _ => {}
        }
    }
    Ok(document_from(visible, events, phase, now_ms))
}

fn accept_message(
    payload: &Value,
    timestamp: Option<u64>,
    visible: &mut String,
    events: &mut Vec<PeerEvent>,
) -> Result<Option<&'static str>, FailKind> {
    let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
    let phase = payload.get("phase").and_then(Value::as_str);
    match role {
        "user" => {
            if let Some(timestamp_ms) = timestamp {
                push_event(
                    events,
                    PeerEvent {
                        class: "user_message".to_string(),
                        timestamp_ms,
                        tool_name: None,
                        status: None,
                    },
                );
            }
            Ok(Some("user"))
        }
        "assistant" => {
            if matches!(phase, Some(phase) if phase != "commentary" && phase != "final_answer") {
                return Ok(None);
            }
            let text = message_text(payload)?;
            let trimmed = text.trim_start();
            if trimmed.starts_with('<') || trimmed.starts_with("# AGENTS.md instructions for") {
                return Ok(None);
            }
            if text.is_empty() {
                return Ok(None);
            }
            let class = match phase {
                Some("final_answer") => "final_answer",
                _ => "commentary",
            };
            push_visible(visible, &text);
            if let Some(timestamp_ms) = timestamp {
                push_event(
                    events,
                    PeerEvent {
                        class: class.to_string(),
                        timestamp_ms,
                        tool_name: None,
                        status: None,
                    },
                );
            }
            Ok(Some(class))
        }
        _ => Ok(None),
    }
}

fn message_text(payload: &Value) -> Result<String, FailKind> {
    let blocks = payload
        .get("content")
        .and_then(Value::as_array)
        .ok_or(FailKind::Malformed)?;
    let mut result = String::new();
    for block in blocks {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "output_text" && kind != "input_text" {
            continue;
        }
        let Some(text) = block.get("text").and_then(Value::as_str) else {
            return Err(FailKind::Malformed);
        };
        result.push_str(text);
    }
    Ok(result)
}

fn push_visible(buffer: &mut String, text: &str) {
    if !buffer.is_empty() {
        buffer.push('\n');
    }
    buffer.push_str(text);
    let count = buffer.chars().count();
    let max = MAX_EXCERPT_CHARS + 256;
    if count > max {
        let skip = count - MAX_EXCERPT_CHARS;
        *buffer = buffer.chars().skip(skip).collect();
    }
}

fn push_event(events: &mut Vec<PeerEvent>, event: PeerEvent) {
    events.push(event);
    if events.len() > MAX_EVENTS {
        let excess = events.len() - MAX_EVENTS;
        events.drain(..excess);
    }
}

fn document_from(
    visible: String,
    events: Vec<PeerEvent>,
    phase: &str,
    now_ms: u64,
) -> PeerActivityDocument {
    let (text, truncated) = tail_scalars(&visible, MAX_EXCERPT_CHARS);
    let has_visible = !text.is_empty();
    let last_activity_ms = events.iter().map(|event| event.timestamp_ms).max();
    PeerActivityDocument {
        schema_version: PEER_ACTIVITY_SCHEMA_VERSION,
        as_of_ms: now_ms,
        phase: phase.to_string(),
        process_state: "unknown".to_string(),
        last_activity_ms,
        age_ms: last_activity_ms.map(|value| now_ms.saturating_sub(value)),
        assessment: NOT_INFERRED.to_string(),
        excerpt: VisibleExcerpt {
            available: !text.is_empty(),
            fingerprint: if text.is_empty() {
                String::new()
            } else {
                sha256_hex(text.as_bytes())
            },
            truncated,
            text,
        },
        unavailable: None,
        freshness: Some(if last_activity_ms.is_some() {
            "timestamped".to_string()
        } else if has_visible || phase != "idle" {
            "timestamp_unavailable".to_string()
        } else {
            "no_activity_observed".to_string()
        }),
        events,
    }
}

fn tail_scalars(text: &str, max: usize) -> (String, bool) {
    let count = text.chars().count();
    if count <= max {
        (text.to_string(), false)
    } else {
        (text.chars().skip(count - max).collect(), true)
    }
}

fn safe_tool_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 64 {
        return None;
    }
    if trimmed
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn record_timestamp(value: &Value) -> Option<u64> {
    if let Some(millis) = value.get("timestamp_ms").and_then(Value::as_u64) {
        return Some(millis);
    }
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_ms)
}

pub fn parse_rfc3339_ms(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year: i32 = std::str::from_utf8(&bytes[0..4]).ok()?.parse().ok()?;
    let month: u32 = std::str::from_utf8(&bytes[5..7]).ok()?.parse().ok()?;
    let day: u32 = std::str::from_utf8(&bytes[8..10]).ok()?.parse().ok()?;
    let hour: u32 = std::str::from_utf8(&bytes[11..13]).ok()?.parse().ok()?;
    let minute: u32 = std::str::from_utf8(&bytes[14..16]).ok()?.parse().ok()?;
    let second: u32 = std::str::from_utf8(&bytes[17..19]).ok()?.parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &text[19..];
    let mut millis = 0_u32;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits: String = fraction
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        if digits.is_empty() || digits.len() > 9 {
            return None;
        }
        let parsed: u32 = digits.parse().ok()?;
        millis = match digits.len() {
            1 => parsed * 100,
            2 => parsed * 10,
            3 => parsed,
            len => parsed / 10_u32.pow((len - 3) as u32),
        };
        rest = &fraction[digits.len()..];
    }
    if rest != "Z" && rest != "z" {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600)?
        .checked_add(i64::from(minute) * 60)?
        .checked_add(i64::from(second))?;
    u64::try_from(seconds)
        .ok()
        .and_then(|value| value.checked_mul(1_000))
        .map(|value| value + u64::from(millis))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = (year - era * 400) as u64;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era =
        year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + u64::from(day_of_year);
    Some(i64::from(era) * 146_097 + day_of_era as i64 - 719_468)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}
