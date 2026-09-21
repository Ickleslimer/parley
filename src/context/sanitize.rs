//! Visible conversation sanitizer for Codex rollout records.

use crate::json::Json;

use super::error::ContextError;
use super::parse::{get_str, parse_json};
use super::reader::PhysicalRecord;

const AGENTS_PREFIX: &str = "# AGENTS.md instructions for";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VisibleMessage {
    pub role: String,
    pub phase: Option<String>,
    pub text: String,
    pub turn_id: String,
    pub message_id: String,
    pub turn_start_offset: u64,
    pub start_offset: u64,
    pub end_offset: u64,
}

pub(crate) fn visible_messages(
    records: &[PhysicalRecord],
) -> Result<Vec<VisibleMessage>, ContextError> {
    let mut current_turn: Option<(String, u64)> = None;
    let mut synthetic_n: u64 = 0;
    let mut last_synthetic: Option<String> = None;
    let mut out = Vec::new();

    for record in records {
        let json = parse_json(&record.text)?;
        if json.as_object().is_none() {
            return Err(ContextError::malformed(format!(
                "JSONL record at offset {} is not an object",
                record.start_offset
            )));
        }
        let rtype = get_str(&json, "type").unwrap_or("");
        if rtype == "turn_context" {
            if let Some(turn_id) = json
                .get("payload")
                .and_then(|payload| get_str(payload, "turn_id"))
                .filter(|value| !value.is_empty())
            {
                current_turn = Some((turn_id.to_string(), record.start_offset));
            }
            continue;
        }

        let Some(extracted) = extract_visible(&json, record)? else {
            continue;
        };
        let turn_id = extracted
            .explicit_turn
            .or_else(|| current_turn.as_ref().map(|(turn_id, _)| turn_id.clone()))
            .unwrap_or_else(|| {
                if extracted.role == "user" || last_synthetic.is_none() {
                    synthetic_n += 1;
                    last_synthetic = Some(format!("synthetic:{synthetic_n}"));
                }
                last_synthetic.clone().unwrap()
            });
        let turn_start_offset = current_turn
            .as_ref()
            .filter(|(current, _)| current == &turn_id)
            .map(|(_, offset)| *offset)
            .unwrap_or(record.start_offset);
        out.push(VisibleMessage {
            role: extracted.role,
            phase: extracted.phase,
            text: extracted.text,
            turn_id,
            message_id: extracted.message_id,
            turn_start_offset,
            start_offset: record.start_offset,
            end_offset: record.end_offset,
        });
    }
    Ok(out)
}

struct Extracted {
    role: String,
    phase: Option<String>,
    text: String,
    explicit_turn: Option<String>,
    message_id: String,
}

fn extract_visible(
    json: &Json,
    record: &PhysicalRecord,
) -> Result<Option<Extracted>, ContextError> {
    if get_str(json, "type") != Some("response_item") {
        return Ok(None);
    }
    let payload = json
        .get("payload")
        .ok_or_else(|| ContextError::malformed("response_item missing payload"))?;
    if payload.as_object().is_none() {
        return Err(ContextError::malformed(
            "response_item payload is not an object",
        ));
    }
    if get_str(payload, "type") != Some("message") {
        return Ok(None);
    }
    let role = match get_str(payload, "role") {
        Some("user") => "user",
        Some("assistant") => "assistant",
        _ => return Ok(None),
    };
    let phase = match get_str(payload, "phase") {
        None => None,
        Some("commentary") => Some("commentary".to_string()),
        Some("final_answer") => Some("final_answer".to_string()),
        Some(_) => return Ok(None),
    };
    if role == "user" && phase.is_some() {
        return Ok(None);
    }
    let text = message_text(payload, record.start_offset)?;
    if text.is_empty() {
        return Ok(None);
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with('<') || trimmed.starts_with(AGENTS_PREFIX) {
        return Ok(None);
    }
    let explicit_turn = json
        .get("turn_id")
        .and_then(Json::as_str)
        .or_else(|| get_str(payload, "turn_id"))
        .or_else(|| {
            payload
                .get("internal_chat_message_metadata_passthrough")
                .and_then(|metadata| get_str(metadata, "turn_id"))
        })
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let message_id = get_str(payload, "id")
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("offset:{}", record.start_offset));
    Ok(Some(Extracted {
        role: role.to_string(),
        phase,
        text,
        explicit_turn,
        message_id,
    }))
}

fn message_text(payload: &Json, offset: u64) -> Result<String, ContextError> {
    let content = payload
        .get("content")
        .ok_or_else(|| ContextError::malformed(format!("message at {offset} missing content")))?;
    let blocks = content.as_array().ok_or_else(|| {
        ContextError::malformed(format!("message at {offset} content is not an array"))
    })?;
    let mut parts = Vec::new();
    for block in blocks {
        let kind = get_str(block, "type").unwrap_or("");
        if kind != "input_text" && kind != "output_text" {
            continue;
        }
        let text = get_str(block, "text").ok_or_else(|| {
            ContextError::malformed(format!(
                "text block at {offset} is missing a string text field"
            ))
        })?;
        parts.push(text.to_string());
    }
    Ok(parts.join("\n"))
}

pub(crate) fn render_messages(messages: &[VisibleMessage]) -> String {
    let mut out = String::new();
    for (index, message) in messages.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&role_header(message));
        out.push('\n');
        out.push_str(&message.text);
        out.push('\n');
    }
    out
}

pub(crate) fn content_chars(messages: &[VisibleMessage]) -> u64 {
    messages
        .iter()
        .map(|message| message.text.chars().count() as u64)
        .sum()
}

fn role_header(message: &VisibleMessage) -> String {
    match (message.role.as_str(), message.phase.as_deref()) {
        ("user", _) => "[user]".to_string(),
        ("assistant", Some("commentary")) => "[assistant commentary]".to_string(),
        ("assistant", Some("final_answer")) => "[assistant final_answer]".to_string(),
        _ => "[assistant]".to_string(),
    }
}
