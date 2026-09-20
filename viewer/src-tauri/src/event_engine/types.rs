use serde::{Deserialize, Serialize};

pub(crate) const EXCERPT_LIMIT: usize = 420;
pub(crate) const PAGE_LIMIT_MIN: usize = 1;
pub(crate) const PAGE_LIMIT_MAX: usize = 200;
pub(crate) const PENDING_LABEL: &str = "Request logged; no response event yet";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceState {
    None,
    Missing,
    Watching,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Request,
    Response,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub malformed_lines: u64,
    pub oversized_lines: u64,
    pub unsupported_records: u64,
    pub duplicate_events: u64,
    pub io_errors: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub source_path: Option<String>,
    pub source_state: SourceState,
    pub generation: u64,
    pub bytes_read: u64,
    pub session_count: u64,
    pub exchange_count: u64,
    pub last_event_timestamp_ms: Option<u64>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    pub exchange_count: u64,
    pub latest_timestamp_ms: u64,
    pub latest_source: String,
    pub latest_target: String,
    pub latest_excerpt: String,
    pub excerpt_extracted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage {
    pub items: Vec<SessionSummary>,
    pub next_cursor: Option<u64>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePreview {
    pub event_id: String,
    pub event_type: EventType,
    pub speaker: String,
    pub recipient: String,
    pub timestamp_ms: u64,
    pub status: String,
    pub excerpt: String,
    pub excerpt_extracted: bool,
    pub content_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeSummary {
    pub exchange_id: String,
    pub session_id: String,
    pub timestamp_ms: u64,
    pub request: Option<MessagePreview>,
    pub completion: Option<MessagePreview>,
    pub pending_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangePage {
    pub items: Vec<ExchangeSummary>,
    pub next_cursor: Option<u64>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub event_id: String,
    pub exchange_id: String,
    pub session_id: String,
    pub event_type: EventType,
    pub timestamp_ms: u64,
    pub excerpt: String,
    pub match_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub items: Vec<SearchHit>,
    pub next_cursor: Option<u64>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventContent {
    pub event_id: String,
    pub exchange_id: String,
    pub session_id: String,
    pub event_type: EventType,
    pub speaker: String,
    pub recipient: String,
    pub timestamp_ms: u64,
    pub status: String,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetSnapshot {
    pub session_id: Option<String>,
    pub exchange_id: Option<String>,
    pub request: Option<MessagePreview>,
    pub completion: Option<MessagePreview>,
    pub pending_label: Option<String>,
}

impl WidgetSnapshot {
    pub(crate) fn empty() -> Self {
        Self {
            session_id: None,
            exchange_id: None,
            request: None,
            completion: None,
            pending_label: None,
        }
    }
}

pub(crate) fn clamp_page_limit(limit: usize) -> usize {
    limit.clamp(PAGE_LIMIT_MIN, PAGE_LIMIT_MAX)
}

pub(crate) fn speakers(event_type: EventType, source: &str, target: &str) -> (String, String) {
    match event_type {
        EventType::Response => (target.to_string(), source.to_string()),
        EventType::Request | EventType::Error => (source.to_string(), target.to_string()),
    }
}
