use serde::{Deserialize, Serialize};

pub const HANDOFF_SCHEMA_VERSION: u32 = 1;
pub const PEER_ACTIVITY_SCHEMA_VERSION: u32 = 1;
pub const MAX_REPORT_CHARS: usize = 256_000;
pub const MAX_EXCERPT_CHARS: usize = 4_096;
pub const MAX_ACTIVITY_EVENTS: usize = 64;
pub const MAX_JOURNAL_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_JOURNAL_LINE: usize = 512 * 1024;
pub const MAX_WAIT_SLICE_SECS: u64 = 300;
pub const DEFAULT_WAIT_SLICE_SECS: u64 = 300;
pub const NOT_INFERRED: &str = "not_inferred";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffState {
    Pending,
    Ready,
    AwaitingAck,
    Acknowledged,
    DetachedUnacked,
    CatastropheExit,
}

impl HandoffState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::AwaitingAck => "awaiting_ack",
            Self::Acknowledged => "acknowledged",
            Self::DetachedUnacked => "detached_unacked",
            Self::CatastropheExit => "catastrophe_exit",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "pending" => Self::Pending,
            "ready" => Self::Ready,
            "awaiting_ack" => Self::AwaitingAck,
            "acknowledged" => Self::Acknowledged,
            "detached_unacked" => Self::DetachedUnacked,
            "catastrophe_exit" => Self::CatastropheExit,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisibleExcerpt {
    pub available: bool,
    pub text: String,
    pub truncated: bool,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerEvent {
    pub class: String,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerActivityDocument {
    pub schema_version: u32,
    pub as_of_ms: u64,
    pub phase: String,
    pub process_state: String,
    pub last_activity_ms: Option<u64>,
    pub age_ms: Option<u64>,
    pub assessment: String,
    pub excerpt: VisibleExcerpt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<PeerEvent>,
}

impl PeerActivityDocument {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::unavailable_at(reason, 0)
    }

    pub fn unavailable_at(reason: impl Into<String>, as_of_ms: u64) -> Self {
        Self {
            schema_version: PEER_ACTIVITY_SCHEMA_VERSION,
            as_of_ms,
            phase: "unavailable".to_string(),
            process_state: "unknown".to_string(),
            last_activity_ms: None,
            age_ms: None,
            assessment: NOT_INFERRED.to_string(),
            excerpt: VisibleExcerpt {
                available: false,
                text: String::new(),
                truncated: false,
                fingerprint: String::new(),
            },
            unavailable: Some(reason.into()),
            freshness: Some("unavailable".to_string()),
            events: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityRecord {
    pub class: String,
    pub timestamp_ms: u64,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// Sanitized handoff journal record. Exact report text lives only in `report.txt`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredRecord {
    pub schema_version: u32,
    pub sequence: u64,
    pub job_id: String,
    pub handoff_id: String,
    pub state: String,
    pub source_session_id: Option<String>,
    pub target_session_id: Option<String>,
    pub capability_hash: String,
    pub process_id: Option<u64>,
    pub process_state: String,
    pub phase: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub last_activity_ms: Option<u64>,
    pub ready_at_ms: Option<u64>,
    pub deadline_ms: Option<u64>,
    pub receipt_at_ms: Option<u64>,
    pub wait_until_ms: Option<u64>,
    pub report_fingerprint: Option<String>,
    pub alert_incident_id: Option<String>,
    pub excerpt: String,
    pub excerpt_truncated: bool,
    pub activities: Vec<ActivityRecord>,
}

impl StoredRecord {
    pub fn same_identity(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.job_id == other.job_id
            && self.handoff_id == other.handoff_id
            && self.source_session_id == other.source_session_id
            && self.target_session_id == other.target_session_id
            && self.capability_hash == other.capability_hash
            && self.created_at_ms == other.created_at_ms
    }

    pub fn receipt_observed(&self) -> bool {
        self.receipt_at_ms.is_some() || self.state == HandoffState::Acknowledged.as_str()
    }

    pub fn durable_unacknowledged(&self) -> bool {
        if self.receipt_observed() || self.state == HandoffState::CatastropheExit.as_str() {
            return false;
        }
        self.report_fingerprint.is_some()
            || matches!(
                self.state.as_str(),
                "ready" | "awaiting_ack" | "detached_unacked"
            )
    }
}
