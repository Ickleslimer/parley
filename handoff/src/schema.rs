use serde::{Deserialize, Serialize};

pub const HANDOFF_SCHEMA_VERSION: u32 = 1;
pub const PEER_ACTIVITY_SCHEMA_VERSION: u32 = 1;
pub const MAX_REPORT_CHARS: usize = 256_000;
pub const MAX_EXCERPT_CHARS: usize = 4_096;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisibleExcerpt {
    pub available: bool,
    pub text: String,
    pub truncated: bool,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerActivityDocument {
    pub schema_version: u32,
    pub as_of_ms: u64,
    pub phase: String,
    pub process_state: String,
    pub last_activity_ms: Option<u64>,
    pub age_ms: Option<u64>,
    pub assessment: String,
    pub excerpt: VisibleExcerpt,
    pub unavailable: Option<String>,
}

impl PeerActivityDocument {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            schema_version: PEER_ACTIVITY_SCHEMA_VERSION,
            as_of_ms: 0,
            phase: "unavailable".to_string(),
            process_state: "unknown".to_string(),
            last_activity_ms: None,
            age_ms: None,
            assessment: "not_inferred".to_string(),
            excerpt: VisibleExcerpt {
                available: false,
                text: String::new(),
                truncated: false,
                fingerprint: String::new(),
            },
            unavailable: Some(reason.into()),
        }
    }
}
