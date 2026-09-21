use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ask::AskRequest;
use crate::fsx;
use crate::json::Json;

static EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);
static EVENT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) struct ExchangeLog {
    path: Option<PathBuf>,
    exchange_id: String,
    source: String,
    target: String,
    cwd: String,
    session_id: Option<String>,
    session_action: Option<String>,
    request_receipt: EventReceipt,
}

#[derive(Clone, Debug)]
pub(crate) struct EventReceipt {
    pub(crate) event_id: String,
    pub(crate) exchange_id: String,
    pub(crate) timestamp_ms: u128,
    pub(crate) event_type: String,
    pub(crate) session_id: Option<String>,
    pub(crate) logged: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ExchangeReceipt {
    pub(crate) request: EventReceipt,
    pub(crate) completion: EventReceipt,
    pub(crate) target: String,
}

impl ExchangeLog {
    pub(crate) fn allocate_exchange_id() -> String {
        new_id("exchange")
    }

    pub(crate) fn start_with_exchange_id(
        req: &AskRequest,
        prompt: &str,
        exchange_id: String,
    ) -> Result<Self, String> {
        let path = env::var_os("PARLEY_EVENT_LOG").map(PathBuf::from);
        let mut log = Self {
            path,
            exchange_id: exchange_id.clone(),
            source: env::var("PARLEY_CALLER").unwrap_or_else(|_| "parley".to_string()),
            target: req.harness.clone(),
            cwd: req.cwd.to_string_lossy().to_string(),
            session_id: req.session_id.clone().or_else(|| req.resume_id.clone()),
            session_action: if req.session_id.is_some() {
                Some("new".to_string())
            } else if req.resume_id.is_some() {
                Some("resume".to_string())
            } else {
                None
            },
            request_receipt: EventReceipt {
                event_id: String::new(),
                exchange_id: exchange_id.clone(),
                timestamp_ms: 0,
                event_type: "request".to_string(),
                session_id: req.session_id.clone().or_else(|| req.resume_id.clone()),
                logged: false,
            },
        };
        log.request_receipt = log.write("request", Some(prompt), "started", None, None)?;
        Ok(log)
    }

    pub(crate) fn success(&self, reply: &str, duration_ms: u128) -> Result<EventReceipt, String> {
        self.write("response", Some(reply), "ok", Some(duration_ms), None)
    }

    pub(crate) fn failure(
        &self,
        status: &str,
        error: &str,
        duration_ms: u128,
    ) -> Result<EventReceipt, String> {
        self.write("error", None, status, Some(duration_ms), Some(error))
    }

    pub(crate) fn request_receipt(&self) -> &EventReceipt {
        &self.request_receipt
    }

    pub(crate) fn exchange_receipt(&self, completion: EventReceipt) -> ExchangeReceipt {
        ExchangeReceipt {
            request: self.request_receipt.clone(),
            completion,
            target: self.target.clone(),
        }
    }

    fn write(
        &self,
        event_type: &str,
        content: Option<&str>,
        status: &str,
        duration_ms: Option<u128>,
        error: Option<&str>,
    ) -> Result<EventReceipt, String> {
        let event_id = new_id("event");
        let event_timestamp_ms = timestamp_ms();
        let receipt = EventReceipt {
            event_id: event_id.clone(),
            exchange_id: self.exchange_id.clone(),
            timestamp_ms: event_timestamp_ms,
            event_type: event_type.to_string(),
            session_id: self.session_id.clone(),
            logged: self.path.is_some(),
        };
        let Some(path) = &self.path else {
            return Ok(receipt);
        };
        let mut map = BTreeMap::new();
        map.insert("schema_version".to_string(), Json::Number(1.0));
        map.insert("event_type".to_string(), Json::Str(event_type.to_string()));
        map.insert("event_id".to_string(), Json::Str(event_id));
        map.insert(
            "exchange_id".to_string(),
            Json::Str(self.exchange_id.clone()),
        );
        map.insert(
            "timestamp_ms".to_string(),
            Json::Number(event_timestamp_ms as f64),
        );
        map.insert("source".to_string(), Json::Str(self.source.clone()));
        map.insert("target".to_string(), Json::Str(self.target.clone()));
        map.insert("cwd".to_string(), Json::Str(self.cwd.clone()));
        map.insert(
            "session_id".to_string(),
            self.session_id.clone().map(Json::Str).unwrap_or(Json::Null),
        );
        map.insert(
            "session_action".to_string(),
            self.session_action
                .clone()
                .map(Json::Str)
                .unwrap_or(Json::Null),
        );
        map.insert(
            "content".to_string(),
            content
                .map(|value| Json::Str(value.to_string()))
                .unwrap_or(Json::Null),
        );
        map.insert("status".to_string(), Json::Str(status.to_string()));
        map.insert(
            "duration_ms".to_string(),
            duration_ms
                .map(|value| Json::Number(value as f64))
                .unwrap_or(Json::Null),
        );
        map.insert(
            "error".to_string(),
            error
                .map(|value| Json::Str(value.to_string()))
                .unwrap_or(Json::Null),
        );
        append_line(path, &Json::Object(map).to_compact_string())?;
        Ok(receipt)
    }
}

fn append_line(path: &Path, line: &str) -> Result<(), String> {
    let lock = EVENT_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    fsx::refuse_if_symlink(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open event log {}: {error}", path.display()))?;
    let mut encoded = Vec::with_capacity(line.len() + 2);
    if file
        .metadata()
        .and_then(|metadata| {
            if metadata.len() == 0 {
                return Ok(false);
            }
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0_u8; 1];
            file.read_exact(&mut last)?;
            Ok(last[0] != b'\n')
        })
        .map_err(|error| format!("inspect event log {}: {error}", path.display()))?
    {
        encoded.push(b'\n');
    }
    encoded.extend_from_slice(line.as_bytes());
    encoded.push(b'\n');
    file.write_all(&encoded)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("append event log {}: {error}", path.display()))
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn new_id(kind: &str) -> String {
    format!(
        "{kind}-{}-{}-{}",
        timestamp_ms(),
        std::process::id(),
        EVENT_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "parley-event-log-{}-{}-{name}",
            std::process::id(),
            EVENT_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn log_at(path: PathBuf) -> ExchangeLog {
        ExchangeLog {
            path: Some(path),
            exchange_id: "exchange-1".to_string(),
            source: "codex".to_string(),
            target: "grok".to_string(),
            cwd: "C:\\worker".to_string(),
            session_id: Some("01a06582-d66e-7811-b0c9-0b0266e17903".to_string()),
            session_action: Some("new".to_string()),
            request_receipt: EventReceipt {
                event_id: "request-event-1".to_string(),
                exchange_id: "exchange-1".to_string(),
                timestamp_ms: 1,
                event_type: "request".to_string(),
                session_id: Some("01a06582-d66e-7811-b0c9-0b0266e17903".to_string()),
                logged: true,
            },
        }
    }

    #[test]
    fn appends_escaped_json_lines_without_truncating() {
        let path = temp_path("events.jsonl");
        append_line(
            &path,
            &Json::Str("one\n\"two\"".to_string()).to_compact_string(),
        )
        .unwrap();
        append_line(&path, "{\"ok\":true}").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            Json::parse(lines[0]).unwrap().as_str(),
            Some("one\n\"two\"")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn separates_an_incomplete_tail_before_the_next_durable_event() {
        let path = temp_path("partial-tail.jsonl");
        fs::write(&path, br#"{"partial":true"#).unwrap();
        append_line(&path, "{\"ok\":true}").unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines, vec![r#"{"partial":true"#, r#"{"ok":true}"#]);
        assert!(Json::parse(lines[0]).is_err());
        assert!(Json::parse(lines[1]).is_ok());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn refuses_to_append_when_parent_is_a_file() {
        let parent = temp_path("parent-file");
        fs::write(&parent, "not a directory").unwrap();
        let error = append_line(&parent.join("events.jsonl"), "{}").unwrap_err();
        assert!(error.contains("create") || error.contains("open event log"));
        fs::remove_file(parent).unwrap();
    }

    #[test]
    fn writes_exact_v1_request_and_response_schema() {
        let path = temp_path("schema.jsonl");
        let log = log_at(path.clone());
        log.write(
            "request",
            Some("line one\n\"quoted\""),
            "started",
            None,
            None,
        )
        .unwrap();
        log.success("exact reply", 42).unwrap();

        let lines = fs::read_to_string(&path).unwrap();
        let events = lines
            .lines()
            .map(|line| Json::parse(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 2);
        let expected_keys = [
            "content",
            "cwd",
            "duration_ms",
            "error",
            "event_id",
            "event_type",
            "exchange_id",
            "schema_version",
            "session_action",
            "session_id",
            "source",
            "status",
            "target",
            "timestamp_ms",
        ];
        for event in &events {
            let Json::Object(fields) = event else {
                panic!("expected object");
            };
            assert_eq!(
                fields.keys().map(String::as_str).collect::<Vec<_>>(),
                expected_keys
            );
        }
        assert_eq!(
            events[0].get("event_type").and_then(Json::as_str),
            Some("request")
        );
        assert_eq!(
            events[0].get("content").and_then(Json::as_str),
            Some("line one\n\"quoted\"")
        );
        assert_eq!(
            events[1].get("event_type").and_then(Json::as_str),
            Some("response")
        );
        assert_eq!(
            events[1].get("content").and_then(Json::as_str),
            Some("exact reply")
        );
        assert_eq!(
            events[1].get("duration_ms").and_then(Json::as_number),
            Some(42.0)
        );
        assert_eq!(
            events[0].get("exchange_id").and_then(Json::as_str),
            events[1].get("exchange_id").and_then(Json::as_str)
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn completion_append_failure_is_reported_after_request_write() {
        let parent = temp_path("late-failure-parent");
        let path = parent.join("events.jsonl");
        let log = log_at(path.clone());
        log.write("request", Some("prompt"), "started", None, None)
            .unwrap();
        fs::remove_file(&path).unwrap();
        fs::remove_dir(&parent).unwrap();
        fs::write(&parent, "blocks directory recreation").unwrap();
        let error = log.success("captured reply", 10).unwrap_err();
        assert!(error.contains("create") || error.contains("open event log"));
        fs::remove_file(parent).unwrap();
    }
}
