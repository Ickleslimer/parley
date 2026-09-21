use super::types::{ContextDiagnostics, EXCERPT_LIMIT};

const CONTEXT_MARK: &str = "=== PARLEY_UNTRUSTED_CONTEXT_V1 exchange=";
const CURRENT_MARK: &str = "=== PARLEY_CURRENT_REQUEST_V1 exchange=";
const CONTEXT_END: &str = " ===";

pub(crate) fn bound_excerpt(text: &str) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= EXCERPT_LIMIT {
            break;
        }
        out.push(ch);
    }
    out
}

pub(crate) fn excerpt(content: &str, is_request: bool, exchange_id: &str) -> (String, bool) {
    if is_request {
        if let Some(section) = current_request_section(content, exchange_id) {
            if let Some(remainder) = task_remainder(section) {
                return (bound_excerpt(remainder), true);
            }
            return (
                bound_excerpt(section.trim_start_matches(['\r', '\n'])),
                false,
            );
        }
        if !is_framed(content) {
            if let Some(remainder) = task_remainder(content) {
                return (bound_excerpt(remainder), true);
            }
        }
    }
    (bound_excerpt(content), false)
}

pub(crate) fn parse_context_diagnostics(
    content: &str,
    exchange_id: &str,
) -> Option<ContextDiagnostics> {
    let header_mark = format!("{CONTEXT_MARK}{exchange_id}{CONTEXT_END}");
    let start = content.find(&header_mark)?;
    let rest = &content[start + header_mark.len()..];
    let header_end = rest
        .find("=== BEGIN_UNTRUSTED_TRANSCRIPT ===")
        .or_else(|| rest.find(CURRENT_MARK))
        .unwrap_or(rest.len());
    let header = &rest[..header_end];
    let mut diagnostics = ContextDiagnostics::default();
    let mut saw_field = false;
    for line in header.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "source" => {
                diagnostics.source = Some(value.to_string());
                saw_field = true;
            }
            "mode" => {
                diagnostics.mode = Some(value.to_string());
                saw_field = true;
            }
            "from_offset" => {
                diagnostics.from_offset = parse_u64(value);
                saw_field = true;
            }
            "to_offset" => {
                diagnostics.to_offset = parse_u64(value);
                saw_field = true;
            }
            "record_count" => {
                diagnostics.record_count = parse_u64(value);
                saw_field = true;
            }
            "character_count" => {
                diagnostics.character_count = parse_u64(value);
                saw_field = true;
            }
            "truncated" => {
                diagnostics.truncated = parse_bool(value);
                saw_field = true;
            }
            "recovery" => {
                diagnostics.recovery = Some(value.to_string());
                saw_field = true;
            }
            _ => {}
        }
    }
    saw_field.then_some(diagnostics)
}

pub(crate) fn window_from(content: &str, char_offset: usize) -> String {
    content
        .chars()
        .skip(char_offset)
        .take(EXCERPT_LIMIT)
        .collect()
}

fn is_framed(content: &str) -> bool {
    content.contains(CONTEXT_MARK) || content.contains(CURRENT_MARK)
}

fn current_request_section<'a>(content: &'a str, exchange_id: &str) -> Option<&'a str> {
    let needle = format!("{CURRENT_MARK}{exchange_id}{CONTEXT_END}");
    let idx = content.find(&needle)?;
    Some(&content[idx + needle.len()..])
}

fn task_remainder(content: &str) -> Option<&str> {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("task:") {
            return Some(rest);
        }
    }
    None
}

fn parse_u64(value: &str) -> Option<u64> {
    value.parse().ok()
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_engine::types::EXCERPT_LIMIT;

    #[test]
    fn extracts_matching_current_request_task_and_ignores_historical_task_lines() {
        let prompt = "\
=== PARLEY_UNTRUSTED_CONTEXT_V1 exchange=ex-1 ===
source: codex
session_id: session-old
mode: seed
from_offset: 10
to_offset: 20
record_count: 3
character_count: 99
truncated: false
recovery: replay
=== BEGIN_UNTRUSTED_TRANSCRIPT ===
task: historical task
=== END_UNTRUSTED_TRANSCRIPT ===
=== PARLEY_CURRENT_REQUEST_V1 exchange=ex-1 ===
policy:
- AGENTS.md
  task: Implement the engine
extra";
        let (text, extracted) = excerpt(prompt, true, "ex-1");
        assert!(extracted);
        assert_eq!(text, " Implement the engine");
        let context = parse_context_diagnostics(prompt, "ex-1").expect("context");
        assert_eq!(context.source.as_deref(), Some("codex"));
        assert_eq!(context.mode.as_deref(), Some("seed"));
        assert_eq!(context.from_offset, Some(10));
        assert_eq!(context.to_offset, Some(20));
        assert_eq!(context.record_count, Some(3));
        assert_eq!(context.character_count, Some(99));
        assert_eq!(context.truncated, Some(false));
        assert_eq!(context.recovery.as_deref(), Some("replay"));
    }

    #[test]
    fn framed_mismatched_exchange_does_not_use_legacy_task_fallback() {
        let prompt = "\
=== PARLEY_CURRENT_REQUEST_V1 exchange=ex-other ===
task: wrong envelope
=== PARLEY_UNTRUSTED_CONTEXT_V1 exchange=ex-other ===
source: codex
mode: delta
truncated: true
";
        let (text, extracted) = excerpt(prompt, true, "ex-1");
        assert!(!extracted);
        assert_eq!(text, bound_excerpt(prompt));
        assert!(parse_context_diagnostics(prompt, "ex-1").is_none());
    }

    #[test]
    fn unframed_events_keep_the_first_task_line_fallback() {
        let prompt = "policy:\n- AGENTS.md\n  task: Implement the engine\nextra";
        let (text, extracted) = excerpt(prompt, true, "ex-legacy");
        assert!(extracted);
        assert_eq!(text, " Implement the engine");

        let long: String = "n".repeat(EXCERPT_LIMIT + 40);
        let (prefix, extracted) = excerpt(&long, false, "ex-legacy");
        assert!(!extracted);
        assert_eq!(prefix.chars().count(), EXCERPT_LIMIT);
        assert!(long.starts_with(&prefix));
    }

    #[test]
    fn responses_do_not_extract_task_lines() {
        let body = "task: should stay a prefix only";
        let (text, extracted) = excerpt(body, false, "ex-1");
        assert!(!extracted);
        assert_eq!(text, body);
    }
}
