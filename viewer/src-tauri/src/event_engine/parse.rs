use serde_json::Value;

use super::types::EventType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedEvent {
    pub event_type: EventType,
    pub event_id: String,
    pub exchange_id: String,
    pub timestamp_ms: u64,
    pub source: String,
    pub target: String,
    pub session_id: Option<String>,
    pub content: Option<String>,
    pub status: String,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseOutcome {
    Malformed,
    Unsupported,
}

pub(crate) fn parse_record(line: &str) -> Result<ParsedEvent, ParseOutcome> {
    let value: Value = serde_json::from_str(line).map_err(|_| ParseOutcome::Malformed)?;
    let object = value.as_object().ok_or(ParseOutcome::Malformed)?;

    match object.get("schema_version") {
        None => return Err(ParseOutcome::Malformed),
        Some(version) if is_schema_v1(version) => {}
        Some(version) if version.is_number() => return Err(ParseOutcome::Unsupported),
        Some(_) => return Err(ParseOutcome::Malformed),
    }

    let event_type = match object.get("event_type") {
        None => return Err(ParseOutcome::Malformed),
        Some(Value::String(kind)) => match kind.as_str() {
            "request" => EventType::Request,
            "response" => EventType::Response,
            "error" => EventType::Error,
            _ => return Err(ParseOutcome::Unsupported),
        },
        Some(_) => return Err(ParseOutcome::Malformed),
    };

    let event_id = required_non_empty(object, "event_id")?;
    let exchange_id = required_non_empty(object, "exchange_id")?;
    let timestamp_ms = required_whole_u64(object, "timestamp_ms")?;
    let source = required_string(object, "source")?;
    let target = required_string(object, "target")?;
    let _cwd = required_string(object, "cwd")?;
    let session_id = nullable_string(object, "session_id")?;
    let _session_action = nullable_string(object, "session_action")?;
    let content = nullable_string(object, "content")?;
    let status = required_string(object, "status")?;
    let duration_ms = nullable_whole_u64(object, "duration_ms")?;
    let error = nullable_string(object, "error")?;

    Ok(ParsedEvent {
        event_type,
        event_id,
        exchange_id,
        timestamp_ms,
        source,
        target,
        session_id,
        content,
        status,
        duration_ms,
        error,
    })
}

fn is_schema_v1(value: &Value) -> bool {
    match value {
        Value::Number(number) => {
            number.as_u64() == Some(1)
                || number.as_i64().is_some_and(|value| value == 1)
                || number.as_f64().is_some_and(|value| value == 1.0)
        }
        _ => false,
    }
}

fn required_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, ParseOutcome> {
    match object.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(ParseOutcome::Malformed),
    }
}

fn required_non_empty(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, ParseOutcome> {
    let value = required_string(object, key)?;
    if value.is_empty() {
        Err(ParseOutcome::Malformed)
    } else {
        Ok(value)
    }
}

fn nullable_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ParseOutcome> {
    match object.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) | None => Err(ParseOutcome::Malformed),
    }
}

fn required_whole_u64(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<u64, ParseOutcome> {
    match object.get(key) {
        Some(value) => whole_u64(value).ok_or(ParseOutcome::Malformed),
        None => Err(ParseOutcome::Malformed),
    }
}

fn nullable_whole_u64(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, ParseOutcome> {
    match object.get(key) {
        Some(Value::Null) => Ok(None),
        Some(value) => Ok(Some(whole_u64(value).ok_or(ParseOutcome::Malformed)?)),
        None => Err(ParseOutcome::Malformed),
    }
}

fn whole_u64(value: &Value) -> Option<u64> {
    let Value::Number(number) = value else {
        return None;
    };
    if let Some(value) = number.as_u64() {
        return Some(value);
    }
    if let Some(value) = number.as_i64() {
        return u64::try_from(value).ok();
    }
    let value = number.as_f64()?;
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= u64::MAX as f64 {
        Some(value as u64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_line() -> String {
        json!({
            "schema_version": 1,
            "event_type": "request",
            "event_id": "event-1",
            "exchange_id": "exchange-1",
            "timestamp_ms": 10,
            "source": "codex",
            "target": "grok",
            "cwd": "C:\\worker",
            "session_id": "session-1",
            "session_action": "new",
            "content": "hello",
            "status": "started",
            "duration_ms": null,
            "error": null
        })
        .to_string()
    }

    #[test]
    fn accepts_complete_schema_v1_and_rejects_the_rest() {
        let parsed = parse_record(&valid_line()).unwrap();
        assert_eq!(parsed.event_id, "event-1");
        assert_eq!(parsed.content.as_deref(), Some("hello"));

        assert_eq!(parse_record("{not-json"), Err(ParseOutcome::Malformed));
        assert_eq!(parse_record("[]"), Err(ParseOutcome::Malformed));
        assert_eq!(
            parse_record(&valid_line().replace("\"event_id\":\"event-1\"", "\"event_id\":\"\"")),
            Err(ParseOutcome::Malformed)
        );

        let mut v2 = serde_json::from_str::<Value>(&valid_line()).unwrap();
        v2["schema_version"] = json!(2);
        assert_eq!(
            parse_record(&v2.to_string()),
            Err(ParseOutcome::Unsupported)
        );

        let mut other = serde_json::from_str::<Value>(&valid_line()).unwrap();
        other["event_type"] = json!("metric");
        assert_eq!(
            parse_record(&other.to_string()),
            Err(ParseOutcome::Unsupported)
        );

        let mut incomplete = serde_json::from_str::<Value>(&valid_line()).unwrap();
        incomplete.as_object_mut().unwrap().remove("error");
        assert_eq!(
            parse_record(&incomplete.to_string()),
            Err(ParseOutcome::Malformed)
        );
    }

    #[test]
    fn ignores_unknown_fields_without_inventing_model_metadata() {
        let mut value = serde_json::from_str::<Value>(&valid_line()).unwrap();
        value["model"] = json!("grok-4.6");
        value["reasoning"] = json!("hidden");
        let parsed = parse_record(&value.to_string()).unwrap();
        assert_eq!(parsed.content.as_deref(), Some("hello"));
    }
}
