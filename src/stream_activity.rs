use crate::json::Json;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivityClass {
    Thought,
    ToolCall,
    ToolCallUpdate,
    Text,
    Usage,
    Plan,
    AvailableCommands,
    End,
    Error,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SafeActivity {
    pub class: ActivityClass,
    pub timestamp_ms: u128,
    pub tool_name: Option<String>,
    pub status: Option<String>,
    pub visible_text: Option<String>,
}

pub(crate) fn decode_stream_line(
    line: &str,
    timestamp_ms: u128,
) -> Result<Option<SafeActivity>, String> {
    if line.is_empty() {
        return Err("streaming-json record is empty".to_string());
    }
    let key_count = validate_single_object(line)?;
    let value = Json::parse(line).map_err(|error| format!("malformed streaming-json: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "streaming-json record must be an object".to_string())?;
    if object.len() != key_count {
        return Err("streaming-json record contains duplicate top-level fields".to_string());
    }
    let event_type = value
        .get("type")
        .and_then(Json::as_str)
        .ok_or_else(|| "streaming-json record has no string type".to_string())?;
    let (class, tool_name, status, visible_text) = match event_type {
        "thought" => (ActivityClass::Thought, None, None, None),
        "tool_call" => (
            ActivityClass::ToolCall,
            safe_tool_name(value.get("toolName")),
            safe_status(value.get("status")),
            None,
        ),
        "tool_call_update" => (
            ActivityClass::ToolCallUpdate,
            safe_tool_name(value.get("toolName")),
            safe_status(value.get("status")),
            None,
        ),
        "text" => {
            let data = value
                .get("data")
                .and_then(Json::as_str)
                .ok_or_else(|| "streaming-json text record has no string data".to_string())?;
            (ActivityClass::Text, None, None, Some(data.to_string()))
        }
        "usage" => (
            ActivityClass::Usage,
            None,
            safe_status(value.get("stopReason")),
            None,
        ),
        "plan" => (ActivityClass::Plan, None, None, None),
        "available_commands" => (ActivityClass::AvailableCommands, None, None, None),
        "end" => (
            ActivityClass::End,
            None,
            safe_status(value.get("stopReason")),
            None,
        ),
        "error" => (
            ActivityClass::Error,
            None,
            Some("reported".to_string()),
            None,
        ),
        _ => (ActivityClass::Unknown, None, None, None),
    };
    Ok(Some(SafeActivity {
        class,
        timestamp_ms,
        tool_name,
        status,
        visible_text,
    }))
}

fn safe_tool_name(value: Option<&Json>) -> Option<String> {
    let value = value.and_then(Json::as_str)?;
    if value.is_empty()
        || value.len() > 80
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Some("other".to_string());
    }
    Some(value.to_string())
}

fn safe_status(value: Option<&Json>) -> Option<String> {
    let value = value.and_then(Json::as_str)?;
    Some(
        match value {
            "pending" | "in_progress" | "completed" | "failed" | "cancelled" | "end_turn"
            | "max_tokens" | "max_turn_requests" | "refusal" | "tool_use" | "pause_turn" => value,
            _ => "other",
        }
        .to_string(),
    )
}

fn validate_single_object(line: &str) -> Result<usize, String> {
    let bytes = line.as_bytes();
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .ok_or_else(|| "streaming-json record is empty".to_string())?;
    if bytes[start] != b'{' {
        return Err("streaming-json record must be an object".to_string());
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut key_count = 0usize;
    let mut root_end = None;
    for (index, byte) in bytes.iter().copied().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth = depth.saturating_add(1),
            b'}' | b']' => {
                if depth == 0 {
                    return Err("streaming-json record has an unmatched delimiter".to_string());
                }
                depth -= 1;
                if depth == 0 {
                    root_end = Some(index + 1);
                    break;
                }
            }
            b':' if depth == 1 => key_count = key_count.saturating_add(1),
            _ => {}
        }
    }
    if in_string || depth != 0 {
        return Err("streaming-json record is truncated".to_string());
    }
    let end = root_end.ok_or_else(|| "streaming-json record is truncated".to_string())?;
    if bytes[end..].iter().any(|byte| !byte.is_ascii_whitespace()) {
        return Err("streaming-json record has trailing data".to_string());
    }
    Ok(key_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_only_visible_text_exactly() {
        let activity = decode_stream_line(r#"{"type":"text","data":"hello\n\u732b"}"#, 42)
            .unwrap()
            .unwrap();
        assert_eq!(activity.class, ActivityClass::Text);
        assert_eq!(activity.timestamp_ms, 42);
        assert_eq!(activity.visible_text.as_deref(), Some("hello\n\u{732b}"));
    }

    #[test]
    fn discards_thought_and_raw_tool_payloads() {
        let thought = decode_stream_line(r#"{"type":"thought","data":"SECRET_REASONING"}"#, 1)
            .unwrap()
            .unwrap();
        assert_eq!(thought.class, ActivityClass::Thought);
        assert_eq!(thought.visible_text, None);
        assert!(!format!("{thought:?}").contains("SECRET_REASONING"));

        let tool = decode_stream_line(
            r#"{"type":"tool_call","toolName":"read_file","status":"in_progress","rawInput":{"path":"SECRET_PATH"},"content":["SECRET_OUTPUT"],"locations":["SECRET_LOCATION"]}"#,
            2,
        )
        .unwrap()
        .unwrap();
        assert_eq!(tool.class, ActivityClass::ToolCall);
        assert_eq!(tool.tool_name.as_deref(), Some("read_file"));
        assert_eq!(tool.status.as_deref(), Some("in_progress"));
        let rendered = format!("{tool:?}");
        for forbidden in ["SECRET_PATH", "SECRET_OUTPUT", "SECRET_LOCATION"] {
            assert!(!rendered.contains(forbidden));
        }
    }

    #[test]
    fn recognizes_terminal_and_unknown_records_without_copying_payloads() {
        let end = decode_stream_line(
            r#"{"type":"end","stopReason":"end_turn","usage":{"reasoning_tokens":99}}"#,
            3,
        )
        .unwrap()
        .unwrap();
        assert_eq!(end.class, ActivityClass::End);
        assert_eq!(end.status.as_deref(), Some("end_turn"));
        assert_eq!(end.visible_text, None);

        let unknown =
            decode_stream_line(r#"{"type":"future_private_event","payload":"SECRET"}"#, 4)
                .unwrap()
                .unwrap();
        assert_eq!(unknown.class, ActivityClass::Unknown);
        assert!(!format!("{unknown:?}").contains("SECRET"));
    }

    #[test]
    fn normalizes_untrusted_names_and_statuses() {
        let activity = decode_stream_line(
            r#"{"type":"tool_call","toolName":"../../secret path","status":"provider-secret"}"#,
            5,
        )
        .unwrap()
        .unwrap();
        assert_eq!(activity.tool_name.as_deref(), Some("other"));
        assert_eq!(activity.status.as_deref(), Some("other"));
    }

    #[test]
    fn rejects_malformed_ambiguous_and_trailing_records() {
        for line in [
            "",
            "[]",
            r#"{"data":"missing type"}"#,
            r#"{"type":"text"}"#,
            r#"{"type":"text","type":"thought","data":"x"}"#,
            r#"{"type":"end"} {"type":"end"}"#,
            r#"{"type":"end""#,
        ] {
            assert!(decode_stream_line(line, 1).is_err(), "{line}");
        }
    }

    #[test]
    fn marker_like_visible_content_is_not_interpreted_by_the_decoder() {
        let marker = "=== TWO_CHAIRS_HANDOFF_READY_V1 job=x handoff=y ===";
        let line = format!("{{\"type\":\"text\",\"data\":\"{}\"}}", marker);
        let activity = decode_stream_line(&line, 7).unwrap().unwrap();
        assert_eq!(activity.visible_text.as_deref(), Some(marker));
    }
}
