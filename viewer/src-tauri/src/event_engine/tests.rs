use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use super::source::{open_shared_read, MAX_PHYSICAL_LINE, SHARE_DELETE, SHARE_READ, SHARE_WRITE};
use super::*;
use crate::event_engine::types::{IdMatch, PENDING_LABEL};

static SEQ: AtomicU64 = AtomicU64::new(1);

struct TempLog {
    path: PathBuf,
}

impl TempLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "parley-viewer-event-engine-{}-{}-{label}.jsonl",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_file(&path);
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, bytes: &[u8]) {
        fs::write(&self.path, bytes).unwrap();
    }

    fn append(&self, bytes: &[u8]) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .share_mode(SHARE_READ | SHARE_WRITE | SHARE_DELETE)
            .open(&self.path)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
    }
}

impl Drop for TempLog {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn event_line(
    event_type: &str,
    event_id: &str,
    exchange_id: &str,
    session_id: Option<&str>,
    content: Option<&str>,
    timestamp_ms: u64,
) -> String {
    let (status, duration, error) = match event_type {
        "request" => ("started", Value::Null, Value::Null),
        "error" => ("failed", json!(9), json!("boom")),
        _ => ("ok", json!(9), Value::Null),
    };
    json!({
        "schema_version": 1,
        "event_type": event_type,
        "event_id": event_id,
        "exchange_id": exchange_id,
        "timestamp_ms": timestamp_ms,
        "source": "codex",
        "target": "grok",
        "cwd": "C:\\worker",
        "session_id": session_id,
        "session_action": session_id.map(|_| "new"),
        "content": content,
        "status": status,
        "duration_ms": duration,
        "error": error
    })
    .to_string()
}

fn write_lines(log: &TempLog, lines: &[String]) {
    let mut body = String::new();
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    log.write(body.as_bytes());
}

fn watching_engine(log: &TempLog) -> EventEngine {
    let engine = EventEngine::new();
    engine.set_source(Some(log.path().to_path_buf())).unwrap();
    engine.poll();
    engine
}

fn unique_event(engine: &EventEngine, event_id: &str) -> EventContent {
    match engine.match_event_id(event_id) {
        IdMatch::Unique(content) => content,
        other => panic!("expected unique event {event_id}, got {other:?}"),
    }
}

fn session_key(engine: &EventEngine, session_id: &str) -> String {
    match engine.match_session_id(session_id) {
        IdMatch::Unique(key) => key,
        other => panic!("expected unique session {session_id}, got {other:?}"),
    }
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn event_engine_is_send_sync_and_starts_empty() {
    assert_send_sync::<EventEngine>();
    let engine = EventEngine::new();
    let status = engine.status();
    assert_eq!(status.source_state, SourceState::None);
    assert_eq!(status.generation, 0);
    assert_eq!(status.session_count, 0);
    let feed = engine.widget_feed(None);
    assert!(feed.items.is_empty());
    assert_eq!(feed.total_exchanges, 0);
    assert_eq!(feed.total_events, 0);
    assert!(engine.event_content("missing").is_none());
    assert!(engine.response_for_exchange("missing").is_none());
    assert!(engine.latest_grok_response_before(u64::MAX, None).is_none());
}

#[test]
fn widget_feed_pages_oldest_to_newest_with_stable_opaque_anchors() {
    let log = TempLog::new("widget-feed-pages");
    let lines = (0..25)
        .map(|index| {
            event_line(
                "request",
                &format!("request-{index:02}"),
                &format!("exchange-{index:02}"),
                Some(&format!("session-{}", index / 5)),
                Some(&format!("message {index:02}")),
                index,
            )
        })
        .collect::<Vec<_>>();
    write_lines(&log, &lines);
    let engine = watching_engine(&log);

    let newest = engine.widget_feed(None);
    assert!(!newest.reset_required);
    assert_eq!(newest.items.len(), 20);
    assert_eq!(newest.total_exchanges, 25);
    assert_eq!(newest.total_events, 25);
    assert!(newest.has_earlier);
    assert_eq!(
        newest.items.first().unwrap().request.as_ref().unwrap().body,
        "message 05"
    );
    assert_eq!(
        newest.items.last().unwrap().request.as_ref().unwrap().body,
        "message 24"
    );
    let anchor = newest
        .next_before_exchange_key
        .as_deref()
        .expect("newest page should expose an older anchor");
    assert!(anchor.starts_with("pv1:exchange:"));

    let earlier = engine.widget_feed(Some(anchor));
    assert_eq!(earlier.items.len(), 5);
    assert!(!earlier.has_earlier);
    assert!(earlier.next_before_exchange_key.is_none());
    assert_eq!(
        earlier
            .items
            .first()
            .unwrap()
            .request
            .as_ref()
            .unwrap()
            .body,
        "message 00"
    );
    assert_eq!(
        earlier.items.last().unwrap().request.as_ref().unwrap().body,
        "message 04"
    );
    assert_eq!(earlier.history_token, newest.history_token);
}

#[test]
fn widget_feed_projects_matching_requests_and_bounds_unicode_scalars() {
    let log = TempLog::new("widget-feed-projection");
    let exact = "\u{1f9ec}".repeat(4_001);
    let framed = format!(
        "=== PARLEY_UNTRUSTED_CONTEXT_V1 exchange=ex-framed ===\n\
source: codex\n\
=== BEGIN_UNTRUSTED_TRANSCRIPT ===\n\
private history\n\
=== END_UNTRUSTED_TRANSCRIPT ===\n\
=== PARLEY_CURRENT_REQUEST_V1 exchange=ex-framed ===\n{exact}"
    );
    write_lines(
        &log,
        &[
            event_line(
                "request",
                "request-framed",
                "ex-framed",
                Some("session-framed"),
                Some(&framed),
                1,
            ),
            event_line(
                "request",
                "request-mismatch",
                "ex-mismatch",
                Some("session-framed"),
                Some("=== PARLEY_CURRENT_REQUEST_V1 exchange=other ===\nsecret"),
                2,
            ),
        ],
    );
    let engine = watching_engine(&log);
    let page = engine.widget_feed(None);
    let framed_message = page.items[0].request.as_ref().unwrap();
    assert_eq!(framed_message.body.chars().count(), 4_000);
    assert_eq!(framed_message.full_character_length, 4_001);
    assert!(framed_message.truncated);
    assert!(framed_message.context_omitted);
    assert_eq!(
        framed_message.projection,
        WidgetFeedProjection::CurrentRequest
    );
    assert!(!framed_message.body.contains("private history"));

    let full = engine
        .widget_feed_message(&framed_message.event_key)
        .expect("full projected message");
    assert_eq!(full.body, exact);
    assert_eq!(full.body.chars().count(), 4_001);
    assert!(!full.truncated);

    let withheld = page.items[1].request.as_ref().unwrap();
    assert_eq!(withheld.projection, WidgetFeedProjection::Withheld);
    assert_eq!(withheld.body, "");
    assert_eq!(withheld.full_character_length, 0);
    assert!(withheld.context_omitted);
}

#[test]
fn widget_feed_counts_completion_events_and_keeps_error_semantics() {
    let log = TempLog::new("widget-feed-updates");
    write_lines(
        &log,
        &[
            event_line(
                "request",
                "request-1",
                "exchange-1",
                Some("session-1"),
                Some("waiting"),
                1,
            ),
            event_line(
                "request",
                "request-2",
                "exchange-2",
                Some("session-1"),
                Some("will fail"),
                2,
            ),
            event_line("error", "error-2", "exchange-2", Some("session-1"), None, 3),
        ],
    );
    let engine = watching_engine(&log);
    let initial = engine.widget_feed(None);
    assert_eq!(initial.total_events, 3);
    assert_eq!(
        initial.items[0].pending_label.as_deref(),
        Some(PENDING_LABEL)
    );
    let error = initial.items[1].completion.as_ref().unwrap();
    assert_eq!(error.event_type, EventType::Error);
    assert_eq!(error.body, "boom");

    log.append(
        format!(
            "{}\n",
            event_line(
                "response",
                "response-1",
                "exchange-1",
                Some("session-1"),
                Some("complete"),
                4,
            )
        )
        .as_bytes(),
    );
    assert!(engine.poll());
    let updated = engine.widget_feed(None);
    assert_eq!(updated.total_events, 4);
    let completed = updated
        .items
        .iter()
        .find(|exchange| {
            exchange
                .request
                .as_ref()
                .is_some_and(|message| message.body == "waiting")
        })
        .unwrap();
    assert!(completed.pending_label.is_none());
    assert_eq!(completed.completion.as_ref().unwrap().body, "complete");
}

#[test]
fn widget_feed_history_token_and_anchor_fail_closed_after_replacement() {
    let log = TempLog::new("widget-feed-reset");
    write_lines(
        &log,
        &[
            event_line(
                "request",
                "request-old",
                "exchange-old",
                Some("session-old"),
                Some("old"),
                1,
            ),
            event_line(
                "request",
                "request-new",
                "exchange-new",
                Some("session-new"),
                Some("new"),
                2,
            ),
        ],
    );
    let engine = watching_engine(&log);
    let first = engine.widget_feed(None);
    let stale_anchor = first.items[0].exchange_key.clone();

    fs::remove_file(log.path()).unwrap();
    write_lines(
        &log,
        &[event_line(
            "request",
            "request-replacement",
            "exchange-replacement",
            Some("session-replacement"),
            Some("replacement"),
            3,
        )],
    );
    assert!(engine.poll());
    let reset = engine.widget_feed(Some(&stale_anchor));
    assert!(reset.reset_required);
    assert!(reset.items.is_empty());
    assert_ne!(reset.history_token, first.history_token);
    assert_eq!(reset.total_exchanges, 1);
}

#[test]
fn widget_feed_keeps_cross_source_raw_id_collisions_distinct() {
    let first = TempLog::new("widget-feed-source-a");
    let second = TempLog::new("widget-feed-source-b");
    write_lines(
        &first,
        &[event_line(
            "request",
            "request-shared",
            "exchange-shared",
            Some("session-shared"),
            Some("source a"),
            10,
        )],
    );
    write_lines(
        &second,
        &[event_line(
            "request",
            "request-shared",
            "exchange-shared",
            Some("session-shared"),
            Some("source b"),
            10,
        )],
    );
    let engine = EventEngine::new();
    engine
        .set_sources(vec![
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ])
        .unwrap();
    engine.poll();
    let page = engine.widget_feed(None);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].request.as_ref().unwrap().body, "source a");
    assert_eq!(page.items[1].request.as_ref().unwrap().body, "source b");
    assert_ne!(page.items[0].exchange_key, page.items[1].exchange_key);
    assert_ne!(
        page.items[0].request.as_ref().unwrap().event_key,
        page.items[1].request.as_ref().unwrap().event_key
    );
}

#[test]
fn selects_exact_exchange_response_and_latest_preceding_grok_reply() {
    let log = TempLog::new("handoff-selection");
    write_lines(
        &log,
        &[
            event_line(
                "response",
                "response-old",
                "exchange-old",
                Some("session-a"),
                Some("older"),
                10,
            ),
            event_line(
                "error",
                "error-newer",
                "exchange-error",
                Some("session-a"),
                None,
                15,
            ),
            event_line(
                "response",
                "response-a",
                "exchange-a",
                Some("session-a"),
                Some("session a"),
                20,
            ),
            event_line(
                "response",
                "response-b",
                "exchange-b",
                Some("session-b"),
                Some("session b"),
                25,
            ),
            event_line(
                "response",
                "response-after",
                "exchange-after",
                Some("session-a"),
                Some("too late"),
                30,
            ),
        ],
    );
    let engine = watching_engine(&log);

    assert_eq!(
        engine
            .match_exchange_response("exchange-a")
            .unique()
            .unwrap()
            .event_id,
        "response-a"
    );
    assert!(engine
        .match_exchange_response("exchange-error")
        .unique()
        .is_none());
    assert_eq!(
        engine
            .match_latest_grok_response_before(26, Some("session-a"))
            .unique()
            .unwrap()
            .event_id,
        "response-a"
    );
    assert_eq!(
        engine
            .match_latest_grok_response_before(26, None)
            .unique()
            .unwrap()
            .event_id,
        "response-b"
    );
}

#[test]
fn set_source_rejects_relative_paths_and_never_creates() {
    let engine = EventEngine::new();
    let error = engine
        .set_source(Some(PathBuf::from("relative.jsonl")))
        .unwrap_err();
    assert!(error.contains("absolute"));

    let log = TempLog::new("never-create");
    engine.set_source(Some(log.path().to_path_buf())).unwrap();
    assert!(!engine.poll());
    assert_eq!(engine.status().source_state, SourceState::Missing);
    assert!(!log.path().exists());
}

#[test]
fn status_and_pages_serialize_with_contract_camel_case() {
    let log = TempLog::new("camel");
    write_lines(
        &log,
        &[event_line(
            "request",
            "event-1",
            "exchange-1",
            Some("session-1"),
            Some("hello"),
            10,
        )],
    );
    let engine = watching_engine(&log);
    let status = serde_json::to_value(engine.status()).unwrap();
    for key in [
        "sourceState",
        "generation",
        "bytesRead",
        "sessionCount",
        "exchangeCount",
        "lastEventTimestampMs",
        "diagnostics",
        "sources",
    ] {
        assert!(status.get(key).is_some(), "missing {key}");
    }
    assert_eq!(status["sources"].as_array().unwrap().len(), 1);
    let diagnostics = status["diagnostics"].as_object().unwrap();
    for key in [
        "malformedLines",
        "oversizedLines",
        "unsupportedRecords",
        "duplicateEvents",
        "ioErrors",
        "aliasCollisions",
        "lastError",
    ] {
        assert!(diagnostics.contains_key(key), "missing {key}");
    }

    let sessions = serde_json::to_value(engine.session_page(None, 10)).unwrap();
    assert!(sessions.get("nextCursor").is_some());
    assert!(sessions["items"][0].get("excerptExtracted").is_some());

    let content = serde_json::to_value(unique_event(&engine, "event-1")).unwrap();
    let object = content.as_object().unwrap();
    assert!(!object.contains_key("model"));
    assert!(!object.contains_key("reasoning"));
    for key in [
        "eventKey",
        "exchangeKey",
        "sessionKey",
        "eventId",
        "exchangeId",
        "sessionId",
        "sourcePath",
        "eventType",
        "speaker",
        "recipient",
        "timestampMs",
        "status",
        "durationMs",
        "error",
        "content",
        "context",
    ] {
        assert!(object.contains_key(key), "missing {key}");
    }
    assert!(engine.event_content("event-1").is_none());
}

#[test]
fn skips_malformed_and_unsupported_records() {
    let log = TempLog::new("malformed");
    let mut body = String::new();
    body.push_str("{not json\n");
    body.push_str("[]\n");
    body.push_str(&event_line(
        "request",
        "ok-1",
        "ex-1",
        Some("s1"),
        Some("keep"),
        1,
    ));
    body.push('\n');
    let mut v2 = serde_json::from_str::<Value>(&event_line(
        "request",
        "bad-schema",
        "ex-2",
        Some("s1"),
        Some("no"),
        2,
    ))
    .unwrap();
    v2["schema_version"] = json!(2);
    body.push_str(&v2.to_string());
    body.push('\n');
    let mut metric = serde_json::from_str::<Value>(&event_line(
        "request",
        "bad-type",
        "ex-3",
        Some("s1"),
        Some("no"),
        3,
    ))
    .unwrap();
    metric["event_type"] = json!("metric");
    body.push_str(&metric.to_string());
    body.push('\n');
    body.push('\n');
    log.write(body.as_bytes());

    let engine = watching_engine(&log);
    let status = engine.status();
    assert_eq!(status.source_state, SourceState::Watching);
    assert_eq!(status.diagnostics.malformed_lines, 2);
    assert_eq!(status.diagnostics.unsupported_records, 2);
    assert_eq!(status.exchange_count, 1);
    assert_eq!(unique_event(&engine, "ok-1").content, "keep");
}

#[test]
fn skips_oversized_physical_lines_then_parses_the_next_record() {
    let log = TempLog::new("oversized");
    let mut file = File::create(log.path()).unwrap();
    let chunk = vec![b'x'; 1024 * 1024];
    for _ in 0..8 {
        file.write_all(&chunk).unwrap();
    }
    file.write_all(b"x\n").unwrap();
    let valid = event_line(
        "request",
        "after-oversize",
        "ex-over",
        Some("s-over"),
        Some("survived"),
        4,
    );
    file.write_all(valid.as_bytes()).unwrap();
    file.write_all(b"\n").unwrap();
    file.flush().unwrap();
    drop(file);

    let engine = watching_engine(&log);
    let status = engine.status();
    assert_eq!(status.diagnostics.oversized_lines, 1);
    assert!(status.bytes_read > MAX_PHYSICAL_LINE as u64);
    assert_eq!(unique_event(&engine, "after-oversize").content, "survived");
}

#[test]
fn accepts_utf8_bom_and_crlf_line_endings() {
    let log = TempLog::new("bom-crlf");
    let line = event_line(
        "request",
        "bom-1",
        "ex-bom",
        Some("s-bom"),
        Some("from bom"),
        5,
    );
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(line.as_bytes());
    bytes.extend_from_slice(b"\r\n");
    log.write(&bytes);

    let engine = watching_engine(&log);
    let content = unique_event(&engine, "bom-1");
    assert_eq!(content.content, "from bom");
    assert_eq!(engine.status().generation, 1);
}

#[test]
fn retains_partial_writes_and_split_unicode_until_a_complete_line() {
    let log = TempLog::new("partial-unicode");
    let line = event_line(
        "request",
        "cafe-1",
        "ex-cafe",
        Some("s-cafe"),
        Some("café"),
        6,
    );
    let bytes = line.as_bytes();
    let cafe_at = line.find("café").unwrap();
    let split_at = cafe_at + "caf".len();
    assert_eq!(bytes[split_at], 0xC3);

    let engine = EventEngine::new();
    engine.set_source(Some(log.path().to_path_buf())).unwrap();
    log.append(&bytes[..split_at + 1]);
    assert!(engine.poll());
    assert!(engine.event_content("cafe-1").is_none());
    assert!(engine.status().bytes_read > 0);

    log.append(&bytes[split_at + 1..]);
    log.append(b"\n");
    assert!(engine.poll());
    assert_eq!(unique_event(&engine, "cafe-1").content, "café");
    assert!(!engine.poll());
}

#[test]
fn deduplicates_event_ids_only_inside_one_generation() {
    let log = TempLog::new("dedup");
    let first = event_line("request", "same-id", "ex-a", Some("s-a"), Some("first"), 1);
    let dup = event_line(
        "request",
        "same-id",
        "ex-b",
        Some("s-a"),
        Some("duplicate"),
        2,
    );
    write_lines(&log, &[first, dup]);
    let engine = watching_engine(&log);
    assert_eq!(engine.status().diagnostics.duplicate_events, 1);
    assert_eq!(unique_event(&engine, "same-id").content, "first");
    assert_eq!(engine.status().exchange_count, 1);

    let replacement = event_line("request", "same-id", "ex-c", Some("s-c"), Some("reborn"), 3);
    fs::remove_file(log.path()).unwrap();
    write_lines(&log, &[replacement]);
    assert!(engine.poll());
    let status = engine.status();
    assert_eq!(status.generation, 2);
    assert_eq!(status.diagnostics.duplicate_events, 1);
    assert_eq!(unique_event(&engine, "same-id").content, "reborn");
}

#[test]
fn rebuilds_on_truncation_and_in_place_replacement() {
    let log = TempLog::new("truncate");
    write_lines(
        &log,
        &[
            event_line("request", "old-1", "ex-old", Some("s-old"), Some("old"), 1),
            event_line(
                "response",
                "old-2",
                "ex-old",
                Some("s-old"),
                Some("old-reply"),
                2,
            ),
        ],
    );
    let engine = watching_engine(&log);
    assert_eq!(engine.status().generation, 1);
    assert_eq!(engine.status().exchange_count, 1);

    write_lines(
        &log,
        &[event_line(
            "request",
            "new-1",
            "ex-new",
            Some("s-new"),
            Some("truncated"),
            3,
        )],
    );
    assert!(engine.poll());
    let status = engine.status();
    assert!(status.generation >= 2);
    assert!(engine.match_event_id("old-1").unique().is_none());
    assert_eq!(unique_event(&engine, "new-1").content, "truncated");
}

#[test]
fn pairs_request_response_and_error_and_exposes_pending_label() {
    let log = TempLog::new("pair");
    write_lines(
        &log,
        &[
            event_line(
                "request",
                "req-1",
                "ex-1",
                Some("s-1"),
                Some("task: Ship the engine"),
                10,
            ),
            event_line("response", "res-1", "ex-1", Some("s-1"), Some("done"), 11),
            event_line("request", "req-2", "ex-2", Some("s-1"), Some("second"), 20),
            event_line("error", "err-2", "ex-2", Some("s-1"), None, 21),
            event_line("request", "req-3", "ex-3", Some("s-1"), Some("waiting"), 30),
        ],
    );
    let engine = watching_engine(&log);
    let page = engine.exchange_page(&session_key(&engine, "s-1"), None, 10);
    assert_eq!(page.total, 3);
    assert_eq!(page.items[0].exchange_id, "ex-3");
    assert_eq!(page.items[0].pending_label.as_deref(), Some(PENDING_LABEL));
    assert!(page.items[0].completion.is_none());

    let completed = page
        .items
        .iter()
        .find(|item| item.exchange_id == "ex-1")
        .unwrap();
    let request = completed.request.as_ref().unwrap();
    assert_eq!(request.speaker, "codex");
    assert_eq!(request.recipient, "grok");
    assert!(request.excerpt_extracted);
    assert_eq!(request.excerpt, " Ship the engine");
    let response = completed.completion.as_ref().unwrap();
    assert_eq!(response.speaker, "grok");
    assert_eq!(response.recipient, "codex");
    assert_eq!(response.excerpt, "done");

    let failed = page
        .items
        .iter()
        .find(|item| item.exchange_id == "ex-2")
        .unwrap();
    assert_eq!(
        failed.completion.as_ref().unwrap().event_type,
        EventType::Error
    );
    assert_eq!(failed.completion.as_ref().unwrap().excerpt, "boom");
    assert_eq!(failed.completion.as_ref().unwrap().content_length, 4);
    assert_eq!(failed.pending_label, None);

    let error_search = engine.search("BOOM", None, 10);
    assert_eq!(error_search.total, 1);
    assert_eq!(error_search.items[0].event_id, "err-2");

    let feed = engine.widget_feed(None);
    let latest = feed.items.last().expect("latest feed exchange");
    assert_eq!(latest.pending_label.as_deref(), Some(PENDING_LABEL));
    assert_eq!(latest.request.as_ref().unwrap().body, "waiting");
}

#[test]
fn pages_sessions_exchanges_and_search_newest_first_with_clamped_limits() {
    let log = TempLog::new("pages");
    let mut lines = Vec::new();
    for session in 1..=3 {
        for exchange in 1..=3 {
            let ts = (session * 100 + exchange) as u64;
            lines.push(event_line(
                "request",
                &format!("req-{session}-{exchange}"),
                &format!("ex-{session}-{exchange}"),
                Some(&format!("session-{session}")),
                Some(&format!("needle-{session}-{exchange}")),
                ts,
            ));
        }
    }
    write_lines(&log, &lines);
    let engine = watching_engine(&log);

    let first = engine.session_page(None, 1);
    assert_eq!(first.total, 3);
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].session_id, "session-3");
    assert_eq!(first.next_cursor, Some(1));
    let second = engine.session_page(first.next_cursor, 1);
    assert_eq!(second.items[0].session_id, "session-2");
    let zero = engine.session_page(None, 0);
    assert_eq!(zero.items.len(), 1);

    let exchanges = engine.exchange_page(&session_key(&engine, "session-3"), None, 2);
    assert_eq!(exchanges.total, 3);
    assert_eq!(exchanges.items.len(), 2);
    assert_eq!(exchanges.items[0].exchange_id, "ex-3-3");
    assert_eq!(exchanges.next_cursor, Some(2));

    let search = engine.search("NEEDLE-2-1", None, 10);
    assert_eq!(search.total, 1);
    assert_eq!(search.items[0].event_id, "req-2-1");
    assert_eq!(search.items[0].match_offset, 0);
    assert!(engine.search("   ", None, 10).items.is_empty());
    assert!(engine.search("missing-term", None, 10).items.is_empty());
}

#[test]
fn retrieves_exact_content_above_sixty_thousand_characters() {
    let log = TempLog::new("huge");
    let mut content = "a".repeat(65_000);
    content.push_str("UNIQUE_TOKEN");
    content.push_str(&"b".repeat(5_000));
    write_lines(
        &log,
        &[event_line(
            "response",
            "huge-1",
            "ex-huge",
            Some("s-huge"),
            Some(&content),
            99,
        )],
    );
    let engine = watching_engine(&log);
    let body = unique_event(&engine, "huge-1");
    assert_eq!(body.content, content);
    assert_eq!(body.content.chars().count(), 70_012);
    assert!(!body.content.contains('\u{fffd}'));

    let hit = engine.search("unique_token", None, 1);
    assert_eq!(hit.total, 1);
    assert_eq!(hit.items[0].match_offset, 65_000);
    assert!(hit.items[0].excerpt.starts_with("UNIQUE_TOKEN"));
    assert!(hit.items[0].excerpt.chars().count() <= 420);

    let session = engine.session_page(None, 1).items.remove(0);
    let preview = engine
        .exchange_page(&session.session_key, None, 1)
        .items
        .remove(0)
        .completion
        .unwrap();
    assert_eq!(preview.content_length, 70_012);
    assert_eq!(preview.excerpt.chars().count(), 420);
    assert!(content.starts_with(&preview.excerpt));
    let feed_message = engine.widget_feed(None).items.remove(0).completion.unwrap();
    assert_eq!(feed_message.full_character_length, 70_012);
    assert_eq!(feed_message.body.chars().count(), 4_000);
    assert!(content.starts_with(&feed_message.body));
}

#[test]
fn shared_read_allows_writers_and_delete_access() {
    let log = TempLog::new("share");
    log.write(b"");
    let mut writer = OpenOptions::new()
        .append(true)
        .share_mode(SHARE_READ | SHARE_WRITE)
        .open(log.path())
        .unwrap();
    let line = event_line(
        "request",
        "share-1",
        "ex-share",
        Some("s-share"),
        Some("while open"),
        1,
    );
    writer.write_all(line.as_bytes()).unwrap();
    writer.write_all(b"\n").unwrap();
    writer.flush().unwrap();

    let engine = watching_engine(&log);
    assert_eq!(engine.status().source_state, SourceState::Watching);
    assert_eq!(unique_event(&engine, "share-1").content, "while open");

    writer
        .write_all(
            event_line(
                "response",
                "share-2",
                "ex-share",
                Some("s-share"),
                Some("still writable"),
                2,
            )
            .as_bytes(),
        )
        .unwrap();
    writer.write_all(b"\n").unwrap();
    writer.flush().unwrap();
    assert!(engine.poll());
    assert_eq!(unique_event(&engine, "share-2").content, "still writable");
    drop(writer);

    let reader = open_shared_read(log.path()).unwrap();
    const DELETE_ACCESS: u32 = 0x0001_0000;
    let deleter = OpenOptions::new()
        .access_mode(DELETE_ACCESS)
        .share_mode(SHARE_READ | SHARE_WRITE | SHARE_DELETE)
        .open(log.path());
    assert!(
        deleter.is_ok(),
        "shared reader must grant delete sharing: {deleter:?}"
    );
    drop(deleter);
    drop(reader);
}

#[test]
fn missing_file_can_reappear_without_creating_it() {
    let log = TempLog::new("reappear");
    let engine = EventEngine::new();
    engine.set_source(Some(log.path().to_path_buf())).unwrap();
    assert!(!engine.poll());
    assert_eq!(engine.status().source_state, SourceState::Missing);
    assert!(!log.path().exists());

    write_lines(
        &log,
        &[event_line(
            "request",
            "back-1",
            "ex-back",
            Some("s-back"),
            Some("returned"),
            8,
        )],
    );
    assert!(engine.poll());
    assert_eq!(engine.status().source_state, SourceState::Watching);
    assert_eq!(engine.status().generation, 1);
    assert_eq!(unique_event(&engine, "back-1").content, "returned");
}

#[test]
fn does_not_pair_across_sources_or_generations_and_uses_opaque_keys() {
    let first = TempLog::new("multi-a");
    let second = TempLog::new("multi-b");
    write_lines(
        &first,
        &[
            event_line(
                "request",
                "shared-event",
                "shared-exchange",
                Some("shared-session"),
                Some("from-a"),
                10,
            ),
            event_line(
                "response",
                "resp-a",
                "shared-exchange",
                Some("shared-session"),
                Some("reply-a"),
                11,
            ),
        ],
    );
    write_lines(
        &second,
        &[event_line(
            "request",
            "shared-event",
            "shared-exchange",
            Some("shared-session"),
            Some("from-b"),
            20,
        )],
    );
    let engine = EventEngine::new();
    engine
        .set_sources(vec![
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ])
        .unwrap();
    engine.poll();

    let sessions = engine.session_page(None, 10);
    assert_eq!(sessions.total, 2);
    assert_eq!(sessions.items[0].session_id, "shared-session");
    assert_eq!(sessions.items[0].latest_excerpt, "from-b");
    assert_ne!(sessions.items[0].session_key, sessions.items[1].session_key);

    let first_key = session_key_for(&sessions, first.path());
    let first_page = engine.exchange_page(&first_key, None, 10);
    assert_eq!(first_page.total, 1);
    assert_eq!(
        first_page.items[0].completion.as_ref().unwrap().excerpt,
        "reply-a"
    );

    let second_key = session_key_for(&sessions, second.path());
    let second_page = engine.exchange_page(&second_key, None, 10);
    assert_eq!(second_page.total, 1);
    assert_eq!(
        second_page.items[0].pending_label.as_deref(),
        Some(PENDING_LABEL)
    );

    assert!(matches!(
        engine.match_event_id("shared-event"),
        IdMatch::Ambiguous { count: 2, .. }
    ));
    let unique_a = unique_event_from(&engine, "resp-a");
    assert_eq!(
        engine.event_content(&unique_a.event_key).unwrap().content,
        "reply-a"
    );
    assert!(engine.event_content("resp-a").is_none());
    assert!(engine
        .event_content(&first_page.items[0].exchange_key)
        .is_none());
    assert!(engine.response_for_exchange(&unique_a.event_key).is_none());
    assert_eq!(engine.exchange_page(&unique_a.event_key, None, 10).total, 0);

    let feed = engine.widget_feed(None);
    assert_eq!(
        feed.items.last().unwrap().request.as_ref().unwrap().body,
        "from-b"
    );
}

#[test]
fn independent_replacement_truncation_and_missing_state() {
    let stable = TempLog::new("stable");
    let volatile = TempLog::new("volatile");
    write_lines(
        &stable,
        &[event_line(
            "request",
            "stable-1",
            "ex-stable",
            Some("s-stable"),
            Some("keep-me"),
            1,
        )],
    );
    write_lines(
        &volatile,
        &[event_line(
            "request",
            "volatile-1",
            "ex-vol",
            Some("s-vol"),
            Some("old-vol"),
            2,
        )],
    );
    let engine = EventEngine::new();
    engine
        .set_sources(vec![
            stable.path().to_path_buf(),
            volatile.path().to_path_buf(),
        ])
        .unwrap();
    engine.poll();
    assert_eq!(engine.status().exchange_count, 2);

    write_lines(
        &volatile,
        &[event_line(
            "request",
            "volatile-2",
            "ex-vol-2",
            Some("s-vol"),
            Some("new-vol"),
            3,
        )],
    );
    assert!(engine.poll());
    assert_eq!(unique_event(&engine, "stable-1").content, "keep-me");
    assert!(engine.match_event_id("volatile-1").unique().is_none());
    assert_eq!(unique_event(&engine, "volatile-2").content, "new-vol");

    fs::remove_file(volatile.path()).unwrap();
    assert!(engine.poll());
    let statuses = engine.source_statuses();
    assert_eq!(statuses[0].source_state, SourceState::Watching);
    assert_eq!(statuses[1].source_state, SourceState::Missing);
    assert_eq!(engine.status().source_state, SourceState::Degraded);
    assert_eq!(unique_event(&engine, "stable-1").content, "keep-me");
}

#[test]
fn alias_paths_are_deduplicated_and_diagnosed() {
    let log = TempLog::new("alias");
    write_lines(
        &log,
        &[event_line(
            "request",
            "alias-1",
            "ex-alias",
            Some("s-alias"),
            Some("once"),
            1,
        )],
    );
    let engine = EventEngine::new();
    engine
        .set_sources(vec![log.path().to_path_buf(), log.path().to_path_buf()])
        .unwrap();
    engine.poll();
    assert_eq!(engine.status().exchange_count, 1);
    assert_eq!(engine.status().diagnostics.alias_collisions, 1);
    let statuses = engine.source_statuses();
    assert_eq!(statuses.len(), 2);
    assert!(statuses[1].alias_of.is_some());
    assert_eq!(unique_event(&engine, "alias-1").content, "once");
}

#[test]
fn hard_link_aliases_are_deduplicated_by_file_identity() {
    let log = TempLog::new("hard-link-source");
    write_lines(
        &log,
        &[event_line(
            "request",
            "hard-link-1",
            "ex-hard-link",
            Some("s-hard-link"),
            Some("once"),
            1,
        )],
    );
    let alias = log.path().with_file_name(format!(
        "parley-viewer-event-engine-{}-{}-hard-link-alias.jsonl",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_file(&alias);
    fs::hard_link(log.path(), &alias).unwrap();

    let engine = EventEngine::new();
    engine
        .set_sources(vec![log.path().to_path_buf(), alias.clone()])
        .unwrap();
    engine.poll();
    assert_eq!(engine.status().exchange_count, 1);
    assert_eq!(engine.status().diagnostics.alias_collisions, 1);
    assert!(engine.source_statuses()[1].alias_of.is_some());
    assert_eq!(unique_event(&engine, "hard-link-1").content, "once");

    fs::remove_file(alias).unwrap();
}

#[test]
fn aggregate_order_is_timestamp_then_configured_source_then_key() {
    let older = TempLog::new("order-a");
    let newer_same_ts = TempLog::new("order-b");
    write_lines(
        &older,
        &[event_line(
            "request",
            "a-1",
            "ex-a",
            Some("session-a"),
            Some("aaa"),
            50,
        )],
    );
    write_lines(
        &newer_same_ts,
        &[event_line(
            "request",
            "b-1",
            "ex-b",
            Some("session-b"),
            Some("bbb"),
            50,
        )],
    );
    let engine = EventEngine::new();
    engine
        .set_sources(vec![
            older.path().to_path_buf(),
            newer_same_ts.path().to_path_buf(),
        ])
        .unwrap();
    engine.poll();
    let sessions = engine.session_page(None, 10);
    assert_eq!(sessions.items[0].session_id, "session-a");
    assert_eq!(sessions.items[1].session_id, "session-b");
}

#[test]
#[ignore = "requires explicit local read-only acceptance logs"]
fn acceptance_reads_real_multi_logs_without_gui() {
    let raw = std::env::var("PARLEY_VIEWER_ACCEPTANCE_LOGS_JSON")
        .expect("PARLEY_VIEWER_ACCEPTANCE_LOGS_JSON must be a JSON path array");
    let configured: Vec<PathBuf> = serde_json::from_str::<Vec<String>>(&raw)
        .expect("acceptance log paths must be valid JSON")
        .into_iter()
        .map(PathBuf::from)
        .collect();
    assert!(configured.len() >= 2);
    assert!(configured
        .iter()
        .all(|path| path.is_absolute() && path.is_file()));

    let expected_exchange = std::env::var("PARLEY_VIEWER_ACCEPTANCE_EXCHANGE")
        .expect("PARLEY_VIEWER_ACCEPTANCE_EXCHANGE must name the globally newest exchange");
    let expected_source = std::env::var("PARLEY_VIEWER_ACCEPTANCE_SOURCE")
        .expect("PARLEY_VIEWER_ACCEPTANCE_SOURCE must name its configured log");
    let engine = EventEngine::new();
    engine.set_sources(configured.clone()).unwrap();
    engine.poll();

    let status = engine.status();
    assert_eq!(status.sources.len(), configured.len());
    assert!(status.sources.iter().all(|source| {
        source.source_state == SourceState::Watching
            && source.alias_of.is_none()
            && source.bytes_read > 0
    }));
    let feed = engine.widget_feed(None);
    let newest = feed.items.last().expect("globally newest exchange");
    let newest_message = newest
        .completion
        .as_ref()
        .or(newest.request.as_ref())
        .expect("globally newest exchange message");
    let newest_content = engine
        .event_content(&newest_message.event_key)
        .expect("globally newest event content");
    assert_eq!(newest_content.exchange_id, expected_exchange);
    let completion = engine
        .response_for_exchange(&newest.exchange_key)
        .expect("globally newest exchange completion");
    let actual_source = fs::canonicalize(&completion.source_path).unwrap();
    let expected_source = fs::canonicalize(&expected_source).unwrap();
    assert!(actual_source
        .to_string_lossy()
        .eq_ignore_ascii_case(&expected_source.to_string_lossy()));
    assert_eq!(
        status.last_event_timestamp_ms,
        newest
            .completion
            .as_ref()
            .or(newest.request.as_ref())
            .map(|message| message.timestamp_ms)
    );
    eprintln!(
        "sources={} sessions={} exchanges={} newest_exchange={} newest_source={}",
        status.sources.len(),
        status.session_count,
        status.exchange_count,
        expected_exchange,
        completion.source_path
    );
}

#[test]
fn extracts_matching_context_marker_and_keeps_context_off_the_widget() {
    let log = TempLog::new("framed");
    let framed = "=== PARLEY_UNTRUSTED_CONTEXT_V1 exchange=ex-frame ===\nsource: codex\nmode: seed\nfrom_offset: 4\nto_offset: 8\nrecord_count: 2\ncharacter_count: 16\ntruncated: true\nrecovery: skip\n=== BEGIN_UNTRUSTED_TRANSCRIPT ===\ntask: historical\n=== END_UNTRUSTED_TRANSCRIPT ===\n=== PARLEY_CURRENT_REQUEST_V1 exchange=ex-frame ===\npolicy:\n  task: Current framed task\n".to_string();
    write_lines(
        &log,
        &[event_line(
            "request",
            "frame-1",
            "ex-frame",
            Some("s-frame"),
            Some(&framed),
            9,
        )],
    );
    let engine = watching_engine(&log);
    let content = unique_event(&engine, "frame-1");
    let context = content.context.expect("context diagnostics");
    assert_eq!(context.source.as_deref(), Some("codex"));
    assert_eq!(context.mode.as_deref(), Some("seed"));
    assert_eq!(context.from_offset, Some(4));
    assert_eq!(context.truncated, Some(true));
    assert_eq!(context.recovery.as_deref(), Some("skip"));
    let feed = engine.widget_feed(None);
    let request = feed.items[0].request.as_ref().unwrap();
    assert_eq!(request.body, "policy:\n  task: Current framed task\n");
    assert_eq!(request.projection, WidgetFeedProjection::CurrentRequest);
    assert!(request.context_omitted);
    let encoded = serde_json::to_value(&feed).unwrap();
    assert!(encoded.get("context").is_none());
}

#[test]
fn widget_feed_follows_timestamp_source_then_key_order() {
    let older = TempLog::new("browser-order-a");
    let newer_same_ts = TempLog::new("browser-order-b");
    write_lines(
        &older,
        &[
            event_line(
                "request",
                "a-old",
                "ex-a-old",
                Some("session-a"),
                Some("old"),
                10,
            ),
            event_line(
                "request",
                "a-new",
                "ex-a-new",
                Some("session-a"),
                Some("new"),
                30,
            ),
        ],
    );
    write_lines(
        &newer_same_ts,
        &[
            event_line("request", "b-z", "ex-b-z", Some("session-b"), Some("z"), 30),
            event_line("request", "b-m", "ex-b-m", Some("session-b"), Some("m"), 30),
        ],
    );
    let engine = EventEngine::new();
    engine
        .set_sources(vec![
            older.path().to_path_buf(),
            newer_same_ts.path().to_path_buf(),
        ])
        .unwrap();
    engine.poll();

    let bodies = engine
        .widget_feed(None)
        .items
        .into_iter()
        .map(|exchange| exchange.request.expect("request").body)
        .collect::<Vec<_>>();
    assert_eq!(
        bodies,
        vec![
            "old".to_string(),
            "new".to_string(),
            "m".to_string(),
            "z".to_string()
        ]
    );
}

fn session_key_for(page: &SessionPage, path: &Path) -> String {
    let path = path.to_string_lossy();
    page.items
        .iter()
        .find(|session| session.source_path == path)
        .map(|session| session.session_key.clone())
        .unwrap_or_else(|| panic!("missing session for {path}"))
}

fn unique_event_from(engine: &EventEngine, event_id: &str) -> EventContent {
    unique_event(engine, event_id)
}
