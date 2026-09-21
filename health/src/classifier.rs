use serde_json::Value;

use crate::schema::{
    bound_string, nonempty_reached_type, sanitize_code, sanitize_percent, ClosedClass, Source,
    FREE_USAGE_EXHAUSTED, MAX_PLAN_LEN,
};

#[derive(Clone, Debug, PartialEq)]
pub struct CodexUsageSample {
    pub used_percent: Option<f64>,
    pub resets_at: Option<String>,
    pub plan_type: Option<String>,
    pub rate_limit_reached_type: Option<String>,
    pub as_of_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrokErrorEvidence {
    pub http_status: Option<u16>,
    pub provider_code: Option<String>,
    pub generic_rate_limit: bool,
    pub clipped: bool,
    pub ambiguous: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classification {
    pub class: ClosedClass,
    pub source: Source,
}

pub fn classify_codex(sample: &CodexUsageSample) -> Classification {
    let reached = nonempty_reached_type(sample.rate_limit_reached_type.as_deref());
    Classification {
        class: if reached.is_some() {
            ClosedClass::QuotaExhausted
        } else {
            ClosedClass::UsageSample
        },
        source: Source::Codex,
    }
}

pub fn classify_grok(evidence: &GrokErrorEvidence) -> Classification {
    let class = if evidence.clipped || evidence.ambiguous {
        ClosedClass::TurnError
    } else if evidence.http_status == Some(429)
        && evidence.provider_code.as_deref() == Some(FREE_USAGE_EXHAUSTED)
    {
        ClosedClass::QuotaExhausted
    } else if evidence.http_status == Some(503)
        || evidence.http_status == Some(529)
        || evidence.generic_rate_limit
        || evidence.http_status == Some(429)
    {
        ClosedClass::CapacityThrottle
    } else {
        ClosedClass::TurnError
    };
    Classification {
        class,
        source: Source::Grok,
    }
}

pub fn classify_parley(success: bool, class_hint: Option<ClosedClass>) -> Classification {
    let class = if success {
        ClosedClass::UsageSample
    } else {
        match class_hint {
            Some(ClosedClass::WatchdogKilled) => ClosedClass::WatchdogKilled,
            Some(ClosedClass::McpStdoutUndelivered) => ClosedClass::McpStdoutUndelivered,
            Some(ClosedClass::CapacityThrottle) => ClosedClass::CapacityThrottle,
            Some(ClosedClass::QuotaExhausted) => ClosedClass::TurnError,
            Some(ClosedClass::TurnError) | Some(ClosedClass::UsageSample) | None => {
                ClosedClass::TurnError
            }
        }
    };
    Classification {
        class,
        source: Source::Parley,
    }
}

pub fn sanitize_codex(sample: CodexUsageSample) -> CodexUsageSample {
    CodexUsageSample {
        used_percent: sanitize_percent(sample.used_percent),
        resets_at: bound_string(sample.resets_at, MAX_PLAN_LEN),
        plan_type: bound_string(sample.plan_type, MAX_PLAN_LEN),
        rate_limit_reached_type: nonempty_reached_type(sample.rate_limit_reached_type.as_deref()),
        as_of_ms: sample.as_of_ms,
    }
}

pub fn extract_grok_error(value: &Value, clipped: bool) -> GrokErrorEvidence {
    let mut evidence = GrokErrorEvidence {
        http_status: None,
        provider_code: None,
        generic_rate_limit: false,
        clipped,
        ambiguous: false,
    };
    if clipped {
        evidence.ambiguous = true;
        return evidence;
    }
    let mut budget = 64_u32;
    walk(value, 0, &mut budget, &mut evidence);
    if evidence.http_status.is_none()
        && evidence.provider_code.is_none()
        && !evidence.generic_rate_limit
    {
        if let Some(text) = unstructured_text(value) {
            scan_unstructured(text, &mut evidence);
        } else {
            evidence.ambiguous = true;
        }
    }
    evidence
}

fn walk(value: &Value, depth: u8, budget: &mut u32, evidence: &mut GrokErrorEvidence) {
    if depth > 6 || *budget == 0 {
        return;
    }
    *budget -= 1;
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                consider_field(key, child, evidence);
                walk(child, depth + 1, budget, evidence);
            }
        }
        Value::Array(items) => {
            for child in items.iter().take(8) {
                walk(child, depth + 1, budget, evidence);
            }
        }
        _ => {}
    }
}

fn consider_field(key: &str, value: &Value, evidence: &mut GrokErrorEvidence) {
    let key = key.to_ascii_lowercase();
    if matches!(
        key.as_str(),
        "status" | "http_status" | "httpstatus" | "status_code" | "statuscode"
    ) {
        if let Some(status) = as_http_status(value) {
            record_http_status(evidence, status);
        }
    }
    if matches!(
        key.as_str(),
        "code" | "provider_code" | "providercode" | "error_code" | "errorcode" | "type"
    ) {
        if let Some(code) = as_code(value) {
            if is_generic_rate_limit(&code) {
                evidence.generic_rate_limit = true;
            }
            let sanitized = sanitize_code(Some(code));
            if sanitized.as_deref() == Some(FREE_USAGE_EXHAUSTED)
                || evidence.provider_code.is_none()
            {
                evidence.provider_code = sanitized;
            }
        }
    }
    if matches!(
        key.as_str(),
        "errordetails" | "error_details" | "message" | "detail" | "reason"
    ) {
        if let Value::String(text) = value {
            scan_unstructured(text, evidence);
        }
    }
}

fn as_http_status(value: &Value) -> Option<u16> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| (100..600).contains(n)),
        Value::String(text) => text
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|n| (100..600).contains(n)),
        _ => None,
    }
}

fn as_code(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

fn record_http_status(evidence: &mut GrokErrorEvidence, status: u16) {
    if status == 429 || evidence.http_status != Some(429) {
        evidence.http_status = Some(status);
    }
}

fn is_generic_rate_limit(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
    matches!(
        code.as_str(),
        "rate_limit" | "rate-limit" | "rate_limited" | "rate_limit_exceeded" | "rate_limit_reached"
    )
}

fn unstructured_text(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    for key in ["error", "message", "detail", "reason"] {
        if let Some(Value::String(text)) = object.get(key) {
            return Some(text);
        }
        if let Some(Value::Object(inner)) = object.get(key) {
            if let Some(Value::String(text)) = inner.get("message") {
                return Some(text);
            }
        }
    }
    None
}

fn scan_unstructured(text: &str, evidence: &mut GrokErrorEvidence) {
    if text.len() > 2048 {
        evidence.clipped = true;
        evidence.ambiguous = true;
        return;
    }
    if text.contains(FREE_USAGE_EXHAUSTED) {
        evidence.provider_code = Some(FREE_USAGE_EXHAUSTED.to_string());
    }
    if contains_status_token(text, 429) {
        record_http_status(evidence, 429);
    } else if contains_status_token(text, 503) {
        record_http_status(evidence, 503);
    } else if contains_status_token(text, 529) {
        record_http_status(evidence, 529);
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("rate_limit")
        || lower.contains("rate-limit")
        || lower.contains("rate limited")
    {
        evidence.generic_rate_limit = true;
    }
    if evidence.http_status.is_none()
        && evidence.provider_code.is_none()
        && !evidence.generic_rate_limit
    {
        evidence.ambiguous = true;
    }
}

fn contains_status_token(text: &str, status: u16) -> bool {
    let needle = status.to_string();
    let bytes = text.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut index = 0;
    while index + needle_bytes.len() <= bytes.len() {
        if &bytes[index..index + needle_bytes.len()] == needle_bytes {
            let before = if index == 0 {
                true
            } else {
                !bytes[index - 1].is_ascii_digit()
            };
            let after_index = index + needle_bytes.len();
            let after = if after_index >= bytes.len() {
                true
            } else {
                !bytes[after_index].is_ascii_digit()
            };
            if before && after {
                return true;
            }
        }
        index += 1;
    }
    false
}
