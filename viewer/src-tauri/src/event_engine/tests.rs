use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use super::source::{open_shared_read, MAX_PHYSICAL_LINE, SHARE_DELETE, SHARE_READ, SHARE_WRITE};
use super::*;
use crate::event_engine::types::PENDING_LABEL;

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

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn event_engine_is_send_sync_and_starts_empty() {
    assert_send_sync::<EventEngine>();
    let engine = EventEngine::new();
    let status = engine.status();
    assert_eq!(status.source_state, SourceState::None);
    assert_eq!(status.generation, 0);
    assert_eq!(status.session_count, 0);
    assert!(engine.widget_snapshot().exchange_id.is_none());
    assert!(engine.event_content("missing").is_none());
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
        "sourcePath",
        "sourceState",
        "generation",
        "bytesRead",
        "sessionCount",
        "exchangeCount",
        "lastEventTimestampMs",
        "diagnostics",
    ] {
        assert!(status.get(key).is_some(), "missing {key}");
    }
    let diagnostics = status["diagnostics"].as_object().unwrap();
    for key in [
        "malformedLines",
        "oversizedLines",
        "unsupportedRecords",
        "duplicateEvents",
        "ioErrors",
        "lastError",
    ] {
        assert!(diagnostics.contains_key(key), "missing {key}");
    }

    let sessions = serde_json::to_value(engine.session_page(None, 10)).unwrap();
    assert!(sessions.get("nextCursor").is_some());
    assert!(sessions["items"][0].get("excerptExtracted").is_some());

    let content = serde_json::to_value(engine.event_content("event-1").unwrap()).unwrap();
    let object = content.as_object().unwrap();
    assert!(!object.contains_key("model"));
    assert!(!object.contains_key("reasoning"));
    for key in [
        "eventId",
        "exchangeId",
        "sessionId",
        "eventType",
        "speaker",
        "recipient",
        "timestampMs",
        "status",
        "durationMs",
        "error",
        "content",
    ] {
        assert!(object.contains_key(key), "missing {key}");
    }
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
    assert_eq!(engine.event_content("ok-1").unwrap().content, "keep");
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
    assert_eq!(
        engine.event_content("after-oversize").unwrap().content,
        "survived"
    );
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
    let content = engine.event_content("bom-1").unwrap();
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
    assert_eq!(engine.event_content("cafe-1").unwrap().content, "café");
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
    assert_eq!(engine.event_content("same-id").unwrap().content, "first");
    assert_eq!(engine.status().exchange_count, 1);

    let replacement = event_line("request", "same-id", "ex-c", Some("s-c"), Some("reborn"), 3);
    fs::remove_file(log.path()).unwrap();
    write_lines(&log, &[replacement]);
    assert!(engine.poll());
    let status = engine.status();
    assert_eq!(status.generation, 2);
    assert_eq!(status.diagnostics.duplicate_events, 1);
    assert_eq!(engine.event_content("same-id").unwrap().content, "reborn");
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
    assert!(engine.event_content("old-1").is_none());
    assert_eq!(engine.event_content("new-1").unwrap().content, "truncated");
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
    let page = engine.exchange_page("s-1", None, 10);
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

    let snapshot = engine.widget_snapshot();
    assert_eq!(snapshot.exchange_id.as_deref(), Some("ex-3"));
    assert_eq!(snapshot.pending_label.as_deref(), Some(PENDING_LABEL));
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

    let exchanges = engine.exchange_page("session-3", None, 2);
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
    let body = engine.event_content("huge-1").unwrap();
    assert_eq!(body.content, content);
    assert_eq!(body.content.chars().count(), 70_012);
    assert!(!body.content.contains('\u{fffd}'));

    let hit = engine.search("unique_token", None, 1);
    assert_eq!(hit.total, 1);
    assert_eq!(hit.items[0].match_offset, 65_000);
    assert!(hit.items[0].excerpt.starts_with("UNIQUE_TOKEN"));
    assert!(hit.items[0].excerpt.chars().count() <= 420);

    let preview = engine.widget_snapshot().completion.unwrap();
    assert_eq!(preview.content_length, 70_012);
    assert_eq!(preview.excerpt.chars().count(), 420);
    assert!(content.starts_with(&preview.excerpt));
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
    assert_eq!(
        engine.event_content("share-1").unwrap().content,
        "while open"
    );

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
    assert_eq!(
        engine.event_content("share-2").unwrap().content,
        "still writable"
    );
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
    assert_eq!(engine.event_content("back-1").unwrap().content, "returned");
}
