use super::types::{ContextDiagnostics, EventType, WidgetFeedProjection, EXCERPT_LIMIT};

const CONTEXT_MARK: &str = "=== PARLEY_UNTRUSTED_CONTEXT_V1 exchange=";
const CURRENT_MARK: &str = "=== PARLEY_CURRENT_REQUEST_V1 exchange=";
const CONTEXT_END: &str = " ===";
const R3_ENVELOPE_HEADER: &str = "R3 CODEX-GROK ENVELOPE";
const R3_ENVELOPE_END: [&str; 3] = [
    "Do not upgrade claim language or treat model agreement as validation.",
    "If the envelope conflicts with the scientific contract, fail closed and stop.",
    "Do not perform work outside the envelope.",
];
const LOCKED_RESPONSE_HEADER: &str = "TWO CHAIRS LOCKED RESPONSE CONTRACT";
const ACKNOWLEDGED_HANDOFF_HEADER: &str = "TWO CHAIRS ACKNOWLEDGED HANDOFF CONTRACT";
const HANDOFF_FOOTER_HEADER: &str = "TWO_CHAIRS_HANDOFF";
const HANDOFF_READY_PREFIX: &str = "=== TWO_CHAIRS_HANDOFF_READY_V1 job=";
const HANDOFF_READY_END_PREFIX: &str = "=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff=";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WidgetProjection<'a> {
    pub text: &'a str,
    pub projection: WidgetFeedProjection,
    pub context_omitted: bool,
}

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

pub(crate) fn widget_projection<'a>(
    content: &'a str,
    event_type: EventType,
    exchange_id: &str,
) -> WidgetProjection<'a> {
    match event_type {
        EventType::Error => WidgetProjection {
            text: content,
            projection: WidgetFeedProjection::Exact,
            context_omitted: false,
        },
        EventType::Response => project_response_speech(content),
        EventType::Request => {
            if let Some(section) = current_request_section(content, exchange_id) {
                return project_request_speech(
                    strip_marker_line_break(section),
                    WidgetFeedProjection::CurrentRequest,
                    content.contains(CONTEXT_MARK),
                );
            }
            if is_framed(content) {
                return WidgetProjection {
                    text: "",
                    projection: WidgetFeedProjection::Withheld,
                    context_omitted: true,
                };
            }
            project_request_speech(content, WidgetFeedProjection::Exact, false)
        }
    }
}

fn project_request_speech(
    content: &str,
    fallback_projection: WidgetFeedProjection,
    context_omitted: bool,
) -> WidgetProjection<'_> {
    let mut text = content;
    let mut changed = false;
    if let Some(remainder) = strip_r3_envelope(text) {
        text = remainder;
        changed = true;
    }
    if let Some(remainder) = strip_locked_contract_tail(text) {
        text = remainder;
        changed = true;
    }
    WidgetProjection {
        text,
        projection: if changed {
            WidgetFeedProjection::Speech
        } else {
            fallback_projection
        },
        context_omitted,
    }
}

fn project_response_speech(content: &str) -> WidgetProjection<'_> {
    let mut text = content;
    let mut changed = false;
    if let Some(remainder) = strip_handoff_ready_wrapper(text) {
        text = remainder;
        changed = true;
    }
    if let Some(remainder) = strip_handoff_footer(text) {
        text = remainder;
        changed = true;
    }
    WidgetProjection {
        text,
        projection: if changed {
            WidgetFeedProjection::Speech
        } else {
            WidgetFeedProjection::Exact
        },
        context_omitted: false,
    }
}

#[derive(Debug, Clone, Copy)]
struct LineSpan {
    start: usize,
    end: usize,
    next: usize,
}

fn line_spans(value: &str) -> Vec<LineSpan> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    while start < value.len() {
        if let Some(relative_end) = value[start..].find('\n') {
            let raw_end = start + relative_end;
            let end = if raw_end > start && value.as_bytes()[raw_end - 1] == b'\r' {
                raw_end - 1
            } else {
                raw_end
            };
            spans.push(LineSpan {
                start,
                end,
                next: raw_end + 1,
            });
            start = raw_end + 1;
        } else {
            spans.push(LineSpan {
                start,
                end: value.len(),
                next: value.len(),
            });
            break;
        }
    }
    spans
}

fn line_text(value: &str, span: LineSpan) -> &str {
    &value[span.start..span.end]
}

fn exact_line_index(value: &str, spans: &[LineSpan], expected: &str) -> Option<usize> {
    let mut found = None;
    for (index, span) in spans.iter().copied().enumerate() {
        if line_text(value, span) == expected {
            if found.is_some() {
                return None;
            }
            found = Some(index);
        }
    }
    found
}

fn exact_line_sequence(
    value: &str,
    spans: &[LineSpan],
    expected: &[&str],
) -> Option<(usize, usize)> {
    let mut found = None;
    for start in 0..spans.len() {
        if start + expected.len() > spans.len() {
            break;
        }
        if expected
            .iter()
            .enumerate()
            .all(|(offset, line)| line_text(value, spans[start + offset]) == *line)
        {
            if found.is_some() {
                return None;
            }
            found = Some((start, start + expected.len() - 1));
        }
    }
    found
}

fn strip_r3_envelope(content: &str) -> Option<&str> {
    let spans = line_spans(content);
    if spans.first().map(|span| line_text(content, *span)) != Some(R3_ENVELOPE_HEADER) {
        return None;
    }
    let (_, end_index) = exact_line_sequence(content, &spans, &R3_ENVELOPE_END)?;
    let remainder_start = spans[end_index].next;
    if remainder_start == content.len() {
        return Some(&content[remainder_start..]);
    }
    let separator = spans.get(end_index + 1)?;
    if separator.start != remainder_start || !line_text(content, *separator).is_empty() {
        return None;
    }
    Some(&content[separator.next..])
}

fn strip_locked_contract_tail(content: &str) -> Option<&str> {
    let spans = line_spans(content);
    let header_index = exact_line_index(content, &spans, LOCKED_RESPONSE_HEADER)?;
    if header_index == 0 || !line_text(content, spans[header_index - 1]).is_empty() {
        return None;
    }
    if !valid_locked_contract_tail(content, &spans[header_index..]) {
        return None;
    }
    let separator_start = spans[header_index - 1].start;
    Some(trim_one_trailing_line_break(&content[..separator_start]))
}

fn valid_locked_contract_tail(content: &str, spans: &[LineSpan]) -> bool {
    let acknowledged = spans
        .iter()
        .position(|span| line_text(content, *span) == ACKNOWLEDGED_HANDOFF_HEADER);
    match acknowledged {
        None => valid_locked_response_contract(content, spans),
        Some(index) => {
            let second = spans[index + 1..]
                .iter()
                .any(|span| line_text(content, *span) == ACKNOWLEDGED_HANDOFF_HEADER);
            if second || index < 13 || !valid_locked_response_contract(content, &spans[..12]) {
                return false;
            }
            let separators = &spans[12..index];
            (1..=2).contains(&separators.len())
                && separators
                    .iter()
                    .all(|span| line_text(content, *span).is_empty())
                && valid_acknowledged_handoff_contract(content, &spans[index..])
        }
    }
}

fn valid_locked_response_contract(content: &str, spans: &[LineSpan]) -> bool {
    if spans.len() != 12 {
        return false;
    }
    let lines = spans
        .iter()
        .map(|span| line_text(content, *span))
        .collect::<Vec<_>>();
    lines[0] == LOCKED_RESPONSE_HEADER
        && lines[1]
            == "The current user-authorized request explicitly permits executing only the exact peer-health query below."
        && lines[2]
            == "Keep the normal final response self-contained. Immediately before finishing, invoke exactly this read-only command with no arguments, wrappers, redirects, or chaining:"
        && valid_helper_path(lines[3], "parley-health-query.exe")
        && lines[4]
            == "End the response with this exact field block, copying only current evidence from the query JSON:"
        && lines[5] == HANDOFF_FOOTER_HEADER
        && lines[6] == "peer: codex"
        && lines[7] == "evidence_class: <usage_sample|quota_exhausted|unavailable>"
        && lines[8] == "incident_id: <id|none>"
        && lines[9] == "as_of_ms: <integer|unknown>"
        && lines[10]
            == "Include event_id and exchange_id only when present. End with: continuity: not_authorized"
        && lines[11]
            == "Stale or unavailable evidence never means the peer is down. Do not retry the query or authorize continuation."
}

fn valid_acknowledged_handoff_contract(content: &str, spans: &[LineSpan]) -> bool {
    if spans.len() != 14 {
        return false;
    }
    let lines = spans
        .iter()
        .map(|span| line_text(content, *span))
        .collect::<Vec<_>>();
    let Some((job_id, handoff_id)) = parse_handoff_ready_start(lines[5]) else {
        return false;
    };
    lines[0] == ACKNOWLEDGED_HANDOFF_HEADER
        && lines[1]
            == "This asynchronous write/review turn does not complete until its durable report is explicitly received by Codex. Receipt means delivery only, never approval, acceptance, recovery, or continuity authority."
        && lines[2]
            == "Hidden reasoning and raw tool payloads are never shared. If you need current Codex activity evidence, invoke exactly this zero-argument read-only command:"
        && valid_helper_path(lines[3], "parley-peer-peek.exe")
        && lines[4]
            == "When all bounded work and your self-contained report are complete, emit exactly one block with these collision-resistant markers:"
        && uuid_like(job_id)
        && uuid_like(handoff_id)
        && lines[6] == "<self-contained exact report>"
        && parse_handoff_ready_end(lines[7]) == Some(handoff_id)
        && lines[8]
            == "The durable report inside those markers must itself contain every other locked response requirement, including the complete TWO_CHAIRS_HANDOFF footer when required. Put that footer before the end marker, and emit no visible report text outside the markers."
        && lines[9]
            == "After emitting the complete block, remain in this same turn and invoke exactly the following zero-argument wait command. It waits at most five minutes and reports receipt/deadline evidence; repeat it only while receipt remains pending:"
        && valid_helper_path(lines[10], "parley-handoff-wait.exe")
        && lines[11]
            == "You may explicitly request one sparse user alert for this handoff by invoking exactly this zero-argument command, but elapsed time, silence, hook failure, or process exit never authorizes an automatic alert:"
        && valid_helper_path(lines[12], "parley-peer-alert.exe")
        && lines[13]
            == "Do not end the turn before receipt unless the runtime reports catastrophe or forces detachment. A Stop hook is only a thin gate; Parley runtime state is authoritative."
}

fn strip_handoff_ready_wrapper(content: &str) -> Option<&str> {
    let spans = line_spans(content);
    let first = *spans.first()?;
    let (job_id, handoff_id) = parse_handoff_ready_start(line_text(content, first))?;
    if !uuid_like(job_id) || !uuid_like(handoff_id) {
        return None;
    }
    let expected_end = format!("{HANDOFF_READY_END_PREFIX}{handoff_id} ===");
    let end_index = exact_line_index(content, &spans, &expected_end)?;
    if end_index + 1 != spans.len()
        || spans[1..]
            .iter()
            .any(|span| line_text(content, *span).starts_with(HANDOFF_READY_PREFIX))
        || end_index <= 1
    {
        return None;
    }
    let interior = &content[first.next..spans[end_index].start];
    Some(trim_one_trailing_line_break(interior))
}

fn strip_handoff_footer(content: &str) -> Option<&str> {
    let spans = line_spans(content);
    let header_index = exact_line_index(content, &spans, HANDOFF_FOOTER_HEADER)?;
    if header_index == 0
        || !line_text(content, spans[header_index - 1]).is_empty()
        || !valid_handoff_footer(content, &spans[header_index..])
    {
        return None;
    }
    let separator_start = spans[header_index - 1].start;
    Some(trim_one_trailing_line_break(&content[..separator_start]))
}

fn valid_handoff_footer(content: &str, spans: &[LineSpan]) -> bool {
    if !(6..=8).contains(&spans.len()) || line_text(content, spans[0]) != HANDOFF_FOOTER_HEADER {
        return false;
    }
    let mut peer = None;
    let mut evidence_class = None;
    let mut incident_id = None;
    let mut as_of_ms = None;
    let mut event_id = None;
    let mut exchange_id = None;
    let mut continuity = None;
    for span in &spans[1..] {
        let Some((key, value)) = line_text(content, *span).split_once(": ") else {
            return false;
        };
        if value.is_empty() || value.chars().any(char::is_control) {
            return false;
        }
        let slot = match key {
            "peer" => &mut peer,
            "evidence_class" => &mut evidence_class,
            "incident_id" => &mut incident_id,
            "as_of_ms" => &mut as_of_ms,
            "event_id" => &mut event_id,
            "exchange_id" => &mut exchange_id,
            "continuity" => &mut continuity,
            _ => return false,
        };
        if slot.replace(value).is_some() {
            return false;
        }
    }
    let valid_as_of_ms =
        as_of_ms.is_some_and(|value| value == "unknown" || value.parse::<u64>().is_ok());
    peer == Some("codex")
        && matches!(
            evidence_class,
            Some(
                "usage_sample"
                    | "quota_exhausted"
                    | "capacity_throttle"
                    | "turn_error"
                    | "watchdog_killed"
                    | "mcp_stdout_undelivered"
                    | "unavailable"
            )
        )
        && incident_id.is_some()
        && valid_as_of_ms
        && continuity == Some("not_authorized")
}

fn parse_handoff_ready_start(line: &str) -> Option<(&str, &str)> {
    let body = line
        .strip_prefix(HANDOFF_READY_PREFIX)?
        .strip_suffix(" ===")?;
    body.split_once(" handoff=")
}

fn parse_handoff_ready_end(line: &str) -> Option<&str> {
    line.strip_prefix(HANDOFF_READY_END_PREFIX)?
        .strip_suffix(" ===")
}

fn uuid_like(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

fn valid_helper_path(value: &str, executable: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() > 3
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
        && !value.chars().any(char::is_control)
        && value.to_ascii_lowercase().ends_with(executable)
}

fn trim_one_trailing_line_break(value: &str) -> &str {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
        .unwrap_or(value)
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

fn strip_marker_line_break(section: &str) -> &str {
    section
        .strip_prefix("\r\n")
        .or_else(|| section.strip_prefix('\n'))
        .unwrap_or(section)
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

    #[test]
    fn widget_projection_keeps_only_validated_speech_from_the_matching_request() {
        let prompt = format!(
            "\
=== PARLEY_UNTRUSTED_CONTEXT_V1 exchange=ex-1 ===
source: codex
=== BEGIN_UNTRUSTED_TRANSCRIPT ===
private historical context
=== END_UNTRUSTED_TRANSCRIPT ===
=== PARLEY_CURRENT_REQUEST_V1 exchange=ex-1 ===
R3 CODEX-GROK ENVELOPE
policy: exact fixture
Do not upgrade claim language or treat model agreement as validation.
If the envelope conflicts with the scientific contract, fail closed and stop.
Do not perform work outside the envelope.

My instinct is to preserve this exact colleague speech.

{}",
            locked_response_contract()
        );
        let projected = widget_projection(&prompt, EventType::Request, "ex-1");
        assert_eq!(
            projected.text,
            "My instinct is to preserve this exact colleague speech."
        );
        assert_eq!(projected.projection, WidgetFeedProjection::Speech);
        assert!(projected.context_omitted);
        assert!(!projected.text.contains("private historical context"));
    }

    #[test]
    fn widget_projection_withholds_mismatched_framing() {
        let prompt = "=== PARLEY_CURRENT_REQUEST_V1 exchange=other ===\ntask: wrong";
        let projected = widget_projection(prompt, EventType::Request, "ex-1");
        assert_eq!(projected.text, "");
        assert_eq!(projected.projection, WidgetFeedProjection::Withheld);
        assert!(projected.context_omitted);
    }

    #[test]
    fn widget_projection_strips_the_exact_async_request_contract() {
        let prompt = format!(
            "R3 CODEX-GROK ENVELOPE\r\npolicy: exact fixture\r\n{}\r\n{}\r\n{}\r\n\r\nLet us inspect the actual evidence together.\r\n\r\n{}\r\n\r\n\r\n{}",
            R3_ENVELOPE_END[0],
            R3_ENVELOPE_END[1],
            R3_ENVELOPE_END[2],
            locked_response_contract().replace('\n', "\r\n"),
            acknowledged_handoff_contract().replace('\n', "\r\n")
        );
        let projected = widget_projection(&prompt, EventType::Request, "ex-1");
        assert_eq!(
            projected.text,
            "Let us inspect the actual evidence together."
        );
        assert_eq!(projected.projection, WidgetFeedProjection::Speech);
        assert!(!projected.context_omitted);
    }

    #[test]
    fn widget_projection_unwraps_a_report_and_keeps_decision_speech() {
        let job_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let handoff_id = "11111111-2222-4333-8444-555555555555";
        let response = format!(
            "=== TWO_CHAIRS_HANDOFF_READY_V1 job={job_id} handoff={handoff_id} ===\nWe agree on the narrow parser.\n\nDECISION\nchosen: preserve speech\nrejected: prose heuristics\n\n{}\n=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff={handoff_id} ===",
            handoff_footer()
        );
        let projected = widget_projection(&response, EventType::Response, "ex-1");
        assert_eq!(
            projected.text,
            "We agree on the narrow parser.\n\nDECISION\nchosen: preserve speech\nrejected: prose heuristics"
        );
        assert_eq!(projected.projection, WidgetFeedProjection::Speech);
        assert!(!projected.context_omitted);
    }

    #[test]
    fn widget_projection_leaves_errors_and_malformed_protocol_text_exact() {
        let error = format!("Execution failed\n\n{}", handoff_footer());
        let projected = widget_projection(&error, EventType::Error, "ex-1");
        assert_eq!(projected.text, error);
        assert_eq!(projected.projection, WidgetFeedProjection::Exact);

        let malformed = format!(
            "R3 CODEX-GROK ENVELOPE\n{}\n{}\n{}\n{}\n\nordinary speech",
            R3_ENVELOPE_END[0], R3_ENVELOPE_END[1], R3_ENVELOPE_END[2], R3_ENVELOPE_END[2]
        );
        let projected = widget_projection(&malformed, EventType::Request, "ex-1");
        assert_eq!(projected.text, malformed);
        assert_eq!(projected.projection, WidgetFeedProjection::Exact);
    }

    #[test]
    fn widget_projection_does_not_remove_marker_words_inside_speech() {
        let response = "The literal TWO_CHAIRS_HANDOFF remains part of this sentence.\n\nDECISION\nchosen: keep it";
        let projected = widget_projection(response, EventType::Response, "ex-1");
        assert_eq!(projected.text, response);
        assert_eq!(projected.projection, WidgetFeedProjection::Exact);
    }

    fn locked_response_contract() -> &'static str {
        "TWO CHAIRS LOCKED RESPONSE CONTRACT\nThe current user-authorized request explicitly permits executing only the exact peer-health query below.\nKeep the normal final response self-contained. Immediately before finishing, invoke exactly this read-only command with no arguments, wrappers, redirects, or chaining:\nC:\\Parley\\health\\parley-health-query.exe\nEnd the response with this exact field block, copying only current evidence from the query JSON:\nTWO_CHAIRS_HANDOFF\npeer: codex\nevidence_class: <usage_sample|quota_exhausted|unavailable>\nincident_id: <id|none>\nas_of_ms: <integer|unknown>\nInclude event_id and exchange_id only when present. End with: continuity: not_authorized\nStale or unavailable evidence never means the peer is down. Do not retry the query or authorize continuation."
    }

    fn acknowledged_handoff_contract() -> &'static str {
        "TWO CHAIRS ACKNOWLEDGED HANDOFF CONTRACT\nThis asynchronous write/review turn does not complete until its durable report is explicitly received by Codex. Receipt means delivery only, never approval, acceptance, recovery, or continuity authority.\nHidden reasoning and raw tool payloads are never shared. If you need current Codex activity evidence, invoke exactly this zero-argument read-only command:\nC:\\Parley\\handoff\\parley-peer-peek.exe\nWhen all bounded work and your self-contained report are complete, emit exactly one block with these collision-resistant markers:\n=== TWO_CHAIRS_HANDOFF_READY_V1 job=aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee handoff=11111111-2222-4333-8444-555555555555 ===\n<self-contained exact report>\n=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff=11111111-2222-4333-8444-555555555555 ===\nThe durable report inside those markers must itself contain every other locked response requirement, including the complete TWO_CHAIRS_HANDOFF footer when required. Put that footer before the end marker, and emit no visible report text outside the markers.\nAfter emitting the complete block, remain in this same turn and invoke exactly the following zero-argument wait command. It waits at most five minutes and reports receipt/deadline evidence; repeat it only while receipt remains pending:\nC:\\Parley\\handoff\\parley-handoff-wait.exe\nYou may explicitly request one sparse user alert for this handoff by invoking exactly this zero-argument command, but elapsed time, silence, hook failure, or process exit never authorizes an automatic alert:\nC:\\Parley\\handoff\\parley-peer-alert.exe\nDo not end the turn before receipt unless the runtime reports catastrophe or forces detachment. A Stop hook is only a thin gate; Parley runtime state is authoritative."
    }

    fn handoff_footer() -> &'static str {
        "TWO_CHAIRS_HANDOFF\npeer: codex\nevidence_class: usage_sample\nincident_id: none\nas_of_ms: 42\ncontinuity: not_authorized"
    }
}
