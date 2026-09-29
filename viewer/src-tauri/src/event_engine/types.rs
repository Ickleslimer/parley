use serde::{Deserialize, Serialize};

pub(crate) const EXCERPT_LIMIT: usize = 420;
pub(crate) const WIDGET_FEED_BODY_LIMIT: usize = 4_000;
pub(crate) const WIDGET_FEED_PAGE_SIZE: usize = 20;
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
    pub alias_collisions: u64,
    pub last_error: Option<String>,
}

impl Diagnostics {
    pub(crate) fn merge_from(&mut self, other: &Diagnostics) {
        self.malformed_lines += other.malformed_lines;
        self.oversized_lines += other.oversized_lines;
        self.unsupported_records += other.unsupported_records;
        self.duplicate_events += other.duplicate_events;
        self.io_errors += other.io_errors;
        self.alias_collisions += other.alias_collisions;
        if other.last_error.is_some() {
            self.last_error = other.last_error.clone();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub source_state: SourceState,
    pub generation: u64,
    pub bytes_read: u64,
    pub session_count: u64,
    pub exchange_count: u64,
    pub last_event_timestamp_ms: Option<u64>,
    pub diagnostics: Diagnostics,
    pub sources: Vec<SourceStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub path: String,
    pub identity: String,
    pub source_state: SourceState,
    pub generation: u64,
    pub bytes_read: u64,
    pub session_count: u64,
    pub exchange_count: u64,
    pub last_event_timestamp_ms: Option<u64>,
    pub diagnostics: Diagnostics,
    pub alias_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdMatch<T> {
    None,
    Unique(T),
    Ambiguous { count: usize, diagnostic: String },
}

impl<T> IdMatch<T> {
    pub fn unique(self) -> Option<T> {
        match self {
            Self::Unique(value) => Some(value),
            Self::None | Self::Ambiguous { .. } => None,
        }
    }

    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            Self::Ambiguous { diagnostic, .. } => Some(diagnostic.as_str()),
            Self::None | Self::Unique(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContextDiagnostics {
    pub source: Option<String>,
    pub mode: Option<String>,
    pub from_offset: Option<u64>,
    pub to_offset: Option<u64>,
    pub record_count: Option<u64>,
    pub character_count: Option<u64>,
    pub truncated: Option<bool>,
    pub recovery: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_key: String,
    pub session_id: String,
    pub source_path: String,
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
    pub event_key: String,
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
    pub exchange_key: String,
    pub session_key: String,
    pub exchange_id: String,
    pub session_id: String,
    pub source_path: String,
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
    pub event_key: String,
    pub exchange_key: String,
    pub session_key: String,
    pub event_id: String,
    pub exchange_id: String,
    pub session_id: String,
    pub source_path: String,
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
    pub event_key: String,
    pub exchange_key: String,
    pub session_key: String,
    pub event_id: String,
    pub exchange_id: String,
    pub session_id: String,
    pub source_path: String,
    pub event_type: EventType,
    pub speaker: String,
    pub recipient: String,
    pub timestamp_ms: u64,
    pub status: String,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
    pub content: String,
    pub context: Option<ContextDiagnostics>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WidgetFeedProjection {
    Exact,
    CurrentRequest,
    Speech,
    Withheld,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetFeedMessage {
    pub event_key: String,
    pub event_type: EventType,
    pub speaker: String,
    pub recipient: String,
    pub timestamp_ms: u64,
    pub status: String,
    pub body: String,
    pub full_character_length: u64,
    pub truncated: bool,
    pub projection: WidgetFeedProjection,
    pub context_omitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetFeedExchange {
    pub exchange_key: String,
    pub session_key: String,
    pub timestamp_ms: u64,
    pub request: Option<WidgetFeedMessage>,
    pub completion: Option<WidgetFeedMessage>,
    pub pending_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetFeedPage {
    pub history_token: String,
    pub items: Vec<WidgetFeedExchange>,
    pub next_before_exchange_key: Option<String>,
    pub has_earlier: bool,
    pub total_exchanges: u64,
    pub total_events: u64,
    pub reset_required: bool,
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
