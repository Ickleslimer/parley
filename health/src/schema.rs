use std::fmt;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 2;

pub fn accepted_schema(version: u32) -> bool {
    matches!(version, 1 | SCHEMA_VERSION)
}
pub const SOUND_COOLDOWN_MS: u64 = 60_000;
pub const STALE_AFTER_MS: u64 = 120_000;
pub const MAX_HOOK_STDIN: usize = 256 * 1024;
pub const MAX_INBOX_BYTES: usize = 64 * 1024;
pub const MAX_JOURNAL_LINE: usize = 64 * 1024;
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub const MAX_RECENT_INCIDENTS: usize = 20;
pub const MAX_ACTIVE_INCIDENTS: usize = 50;
pub const INBOX_BATCH: usize = 32;
pub const SNAPSHOT_RETRY_COUNT: u32 = 5;
pub const SNAPSHOT_RETRY_MS: u64 = 20;
pub const POLL_MS: u64 = 250;
pub const CODEX_SAMPLE_MS: u64 = 60_000;
pub const MAX_ID_LEN: usize = 128;
pub const MAX_CODE_LEN: usize = 80;
pub const MAX_PLAN_LEN: usize = 64;
pub const FREE_USAGE_EXHAUSTED: &str = "subscription:free-usage-exhausted";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthError(pub String);

impl fmt::Display for HealthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HealthError {}

impl From<io::Error> for HealthError {
    fn from(error: io::Error) -> Self {
        HealthError(error.to_string())
    }
}

impl From<serde_json::Error> for HealthError {
    fn from(error: serde_json::Error) -> Self {
        HealthError(error.to_string())
    }
}

impl HealthError {
    pub fn msg(message: impl Into<String>) -> Self {
        HealthError(message.into())
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedClass {
    UsageSample,
    QuotaExhausted,
    CapacityThrottle,
    TurnError,
    WatchdogKilled,
    McpStdoutUndelivered,
    HandoffUnacknowledged,
}

impl ClosedClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UsageSample => "usage_sample",
            Self::QuotaExhausted => "quota_exhausted",
            Self::CapacityThrottle => "capacity_throttle",
            Self::TurnError => "turn_error",
            Self::WatchdogKilled => "watchdog_killed",
            Self::McpStdoutUndelivered => "mcp_stdout_undelivered",
            Self::HandoffUnacknowledged => "handoff_unacknowledged",
        }
    }

    pub fn is_incident(self) -> bool {
        !matches!(self, Self::UsageSample)
    }

    pub fn is_audible(self) -> bool {
        matches!(
            self,
            Self::QuotaExhausted | Self::McpStdoutUndelivered | Self::HandoffUnacknowledged
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Codex,
    Grok,
    Parley,
    Viewer,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Parley => "parley",
            Self::Viewer => "viewer",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    RequestStarted,
    CodexSample,
    GrokStopFailure,
    ParleyObservation,
    PolicyDiagnostic,
    Acknowledge,
    Mute,
    TestSound,
    PeerAlertRequested,
    HandoffReceived,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentStatus {
    Active,
    Recovered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    Missing,
    Malformed,
    Locked,
    ArgumentsNotAllowed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthRecord {
    pub schema_version: u32,
    pub inbox_id: String,
    pub kind: InboxKind,
    pub as_of_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<ClosedClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_reached_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic_rate_limit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipped: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambiguous: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_incident_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sounded_incident_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sound_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_sound: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<String>,
}

impl HealthRecord {
    pub fn new(kind: InboxKind, inbox_id: String, as_of_ms: u64) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            inbox_id,
            kind,
            as_of_ms,
            recorded_ms: None,
            source: None,
            class: None,
            used_percent: None,
            resets_at: None,
            plan_type: None,
            rate_limit_reached_type: None,
            http_status: None,
            provider_code: None,
            generic_rate_limit: None,
            clipped: None,
            ambiguous: None,
            success: None,
            incident_id: None,
            session_id: None,
            event_id: None,
            exchange_id: None,
            recovered_incident_id: None,
            muted: None,
            sounded_incident_id: None,
            last_sound_ms: None,
            test_sound: None,
            diagnostic_code: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodexSampleView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_reached_type: Option<String>,
    pub as_of_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GrokObservationView {
    pub class: ClosedClass,
    pub as_of_ms: u64,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncidentView {
    pub incident_id: String,
    pub class: ClosedClass,
    pub source: Source,
    pub status: IncidentStatus,
    pub opened_ms: u64,
    pub as_of_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_ms: Option<u64>,
    pub acknowledged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct QueryDiagnostics {
    pub snapshot_missing: bool,
    pub snapshot_malformed: bool,
    pub snapshot_locked: bool,
    pub journal_incomplete_trailing: bool,
    pub malformed_journal_lines: u64,
    pub oversized_journal_lines: u64,
    pub unsupported_journal_records: u64,
    pub quarantined_inbox: u64,
    pub malformed_inbox: u64,
    pub sound_failures: u64,
    pub footer_missing: u64,
    pub codex_sample_failures: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_codex_sample_failure_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unavailable {
    pub reason: UnavailableReason,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryDocument {
    pub schema_version: u32,
    pub generated_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of_ms: Option<u64>,
    pub muted: bool,
    pub unread_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_codex_sample: Option<CodexSampleView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_grok_observation: Option<GrokObservationView>,
    pub active_incidents: Vec<IncidentView>,
    pub recent_incidents: Vec<IncidentView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<Unavailable>,
    pub stale: bool,
    pub diagnostics: QueryDiagnostics,
}

impl QueryDocument {
    pub fn unavailable(reason: UnavailableReason, generated_ms: u64) -> Self {
        let mut diagnostics = QueryDiagnostics::default();
        match reason {
            UnavailableReason::Missing => diagnostics.snapshot_missing = true,
            UnavailableReason::Malformed => diagnostics.snapshot_malformed = true,
            UnavailableReason::Locked => diagnostics.snapshot_locked = true,
            UnavailableReason::ArgumentsNotAllowed => {}
        }
        Self {
            schema_version: SCHEMA_VERSION,
            generated_ms,
            as_of_ms: None,
            muted: false,
            unread_count: 0,
            latest_codex_sample: None,
            latest_grok_observation: None,
            active_incidents: Vec::new(),
            recent_incidents: Vec::new(),
            unavailable: Some(Unavailable { reason }),
            stale: true,
            diagnostics,
        }
    }
}

pub fn bound_string(value: Option<String>, max: usize) -> Option<String> {
    value.and_then(|mut text| {
        if text.len() > max {
            text.truncate(max);
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub fn sanitize_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_ID_LEN {
        return None;
    }
    if trimmed
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

pub fn sanitize_code(value: Option<String>) -> Option<String> {
    bound_string(value, MAX_CODE_LEN).filter(|code| {
        let bytes = code.as_bytes();
        !bytes.is_empty()
            && bytes[0].is_ascii_alphanumeric()
            && bytes[1..].iter().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b':' | b'.')
            })
    })
}

pub fn sanitize_percent(value: Option<f64>) -> Option<f64> {
    value.filter(|percent| percent.is_finite() && *percent >= 0.0 && *percent <= 1000.0)
}

pub fn nonempty_reached_type(value: Option<&str>) -> Option<String> {
    value.and_then(|text| {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            bound_string(Some(trimmed.to_string()), MAX_CODE_LEN)
        }
    })
}

pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
