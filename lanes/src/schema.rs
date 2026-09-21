use serde::{Deserialize, Serialize};

use crate::error::{Denial, LaneError};
use crate::hash::{hex_encode, sha256};

pub const SCHEMA_VERSION: u32 = 1;
pub const CHILD_DEPTH: u32 = 1;
pub const MAX_CHILD_SLOT: u32 = 1;
pub const MAX_CHILDREN: usize = 2;
pub const STATE_ENV: &str = "PARLEY_LANE_STATE_DIR";
pub const MAX_GRANT_BYTES: usize = 64 * 1024;
pub const WRITER_ROLE: &str = "two-chairs-writer";
pub const REVIEWER_ROLE: &str = "two-chairs-reviewer";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantState {
    Active,
    Consumed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChildRole {
    #[serde(rename = "two-chairs-writer")]
    Writer,
    #[serde(rename = "two-chairs-reviewer")]
    Reviewer,
}

impl ChildRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Writer => WRITER_ROLE,
            Self::Reviewer => REVIEWER_ROLE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantKind {
    File,
    Tree,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub volume_serial: String,
    pub file_index: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathGrant {
    pub access: Access,
    pub kind: GrantKind,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantDraft {
    pub grant_id: String,
    pub lane_id: String,
    pub role: ChildRole,
    pub canonical_cwd: std::path::PathBuf,
    pub worktree_common_dir: std::path::PathBuf,
    pub base_commit: String,
    pub path_grants: Vec<PathGrant>,
    pub model: String,
    pub depth: u32,
    pub child_slot: u32,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantRecord {
    pub schema_version: u32,
    pub grant_id: String,
    pub state: GrantState,
    pub lane_id: String,
    pub role: ChildRole,
    pub canonical_cwd: String,
    pub cwd_identity: FileIdentity,
    pub worktree_common_dir: String,
    pub common_dir_identity: FileIdentity,
    pub base_commit: String,
    pub path_grants: Vec<PathGrant>,
    pub model: String,
    pub depth: u32,
    pub child_slot: u32,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub consumed_at_ms: Option<u64>,
    pub metadata_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantFile {
    schema_version: u32,
    grant_id: String,
    state: GrantState,
    lane_id: String,
    role: ChildRole,
    canonical_cwd: String,
    cwd_identity: FileIdentity,
    worktree_common_dir: String,
    common_dir_identity: FileIdentity,
    base_commit: String,
    path_grants: Vec<PathGrant>,
    model: String,
    depth: u32,
    child_slot: u32,
    issued_at_ms: u64,
    expires_at_ms: u64,
    consumed_at_ms: Option<u64>,
    metadata_hash: String,
}

#[derive(Serialize)]
struct HashBody<'a> {
    schema_version: u32,
    grant_id: &'a str,
    state: GrantState,
    lane_id: &'a str,
    role: ChildRole,
    canonical_cwd: &'a str,
    cwd_identity: &'a FileIdentity,
    worktree_common_dir: &'a str,
    common_dir_identity: &'a FileIdentity,
    base_commit: &'a str,
    path_grants: &'a [PathGrant],
    model: &'a str,
    depth: u32,
    child_slot: u32,
    issued_at_ms: u64,
    expires_at_ms: u64,
    consumed_at_ms: Option<u64>,
}

pub fn validate_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub fn validate_model(value: &str) -> bool {
    validate_token(value)
}

pub fn validate_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn validate_draft_shape(draft: &GrantDraft) -> Result<(), LaneError> {
    if !validate_token(&draft.grant_id) {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant_id is not a safe token",
        ));
    }
    if !validate_token(&draft.lane_id) {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "lane_id is not a safe token",
        ));
    }
    if !validate_model(&draft.model) {
        return Err(LaneError::new(
            Denial::Model,
            "model is not a safe inherited model id",
        ));
    }
    if !validate_commit(&draft.base_commit) {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "base_commit must be 40 hexadecimal characters",
        ));
    }
    if draft.depth != CHILD_DEPTH {
        return Err(LaneError::new(
            Denial::Depth,
            "child grant depth must be one",
        ));
    }
    if draft.child_slot > MAX_CHILD_SLOT {
        return Err(LaneError::new(
            Denial::ChildSlot,
            "child slot must be 0 or 1",
        ));
    }
    if draft.expires_at_ms <= draft.issued_at_ms {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant expiry must be after issue time",
        ));
    }
    if draft.path_grants.is_empty() || draft.path_grants.len() > 32 {
        return Err(LaneError::new(
            Denial::Path,
            "path grants must contain one to 32 entries",
        ));
    }
    if !draft.canonical_cwd.is_absolute() || !draft.worktree_common_dir.is_absolute() {
        return Err(LaneError::new(
            Denial::Cwd,
            "cwd and worktree common-dir must be absolute",
        ));
    }
    Ok(())
}

pub fn seal(record: &GrantRecord) -> Result<Vec<u8>, LaneError> {
    let file = GrantFile::from_record(record);
    let mut sealed = file;
    sealed.metadata_hash = String::new();
    sealed.metadata_hash = hash_of(&sealed);
    let bytes = serde_json::to_vec(&sealed).map_err(|error| {
        LaneError::new(Denial::MalformedGrant, format!("encode grant: {error}"))
    })?;
    if bytes.len() > MAX_GRANT_BYTES {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant file exceeds bound",
        ));
    }
    Ok(bytes)
}

pub fn parse_grant(bytes: &[u8]) -> Result<GrantRecord, LaneError> {
    if bytes.len() > MAX_GRANT_BYTES {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant file exceeds bound",
        ));
    }
    let file: GrantFile = serde_json::from_slice(bytes)
        .map_err(|_| LaneError::new(Denial::MalformedGrant, "grant JSON is malformed"))?;
    let expected = hash_of(&file);
    if file.metadata_hash != expected {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant metadata hash does not match",
        ));
    }
    let record = file.into_record()?;
    validate_record(&record)?;
    Ok(record)
}

fn hash_of(file: &GrantFile) -> String {
    let body = HashBody {
        schema_version: file.schema_version,
        grant_id: &file.grant_id,
        state: file.state,
        lane_id: &file.lane_id,
        role: file.role,
        canonical_cwd: &file.canonical_cwd,
        cwd_identity: &file.cwd_identity,
        worktree_common_dir: &file.worktree_common_dir,
        common_dir_identity: &file.common_dir_identity,
        base_commit: &file.base_commit,
        path_grants: &file.path_grants,
        model: &file.model,
        depth: file.depth,
        child_slot: file.child_slot,
        issued_at_ms: file.issued_at_ms,
        expires_at_ms: file.expires_at_ms,
        consumed_at_ms: file.consumed_at_ms,
    };
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    hex_encode(&sha256(&bytes))
}

fn validate_record(record: &GrantRecord) -> Result<(), LaneError> {
    if record.schema_version != SCHEMA_VERSION {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "schema_version must be 1",
        ));
    }
    if !validate_token(&record.grant_id) || !validate_token(&record.lane_id) {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant identifiers are not safe tokens",
        ));
    }
    if !validate_model(&record.model) || !validate_commit(&record.base_commit) {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "model or base commit is malformed",
        ));
    }
    if record.depth != CHILD_DEPTH || record.child_slot > MAX_CHILD_SLOT {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "depth or child slot is outside the child contract",
        ));
    }
    if record.expires_at_ms <= record.issued_at_ms {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant lifetime is malformed",
        ));
    }
    if record.canonical_cwd.is_empty() || record.worktree_common_dir.is_empty() {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "canonical paths are empty",
        ));
    }
    if !identity_digits(&record.cwd_identity) || !identity_digits(&record.common_dir_identity) {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "file identity is malformed",
        ));
    }
    match record.state {
        GrantState::Active if record.consumed_at_ms.is_some() => {
            return Err(LaneError::new(
                Denial::MalformedGrant,
                "active grant carries a consumed timestamp",
            ));
        }
        GrantState::Consumed if record.consumed_at_ms.is_none() => {
            return Err(LaneError::new(
                Denial::MalformedGrant,
                "consumed grant is missing its transition timestamp",
            ));
        }
        GrantState::Active | GrantState::Consumed => {}
    }
    if record.path_grants.is_empty() || record.path_grants.len() > 32 {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "path grants are missing or unbounded",
        ));
    }
    let writes = record
        .path_grants
        .iter()
        .filter(|grant| grant.access == Access::Write)
        .count();
    match record.role {
        ChildRole::Writer if writes == 0 => {
            return Err(LaneError::new(
                Denial::MalformedGrant,
                "writer grant has no write path",
            ));
        }
        ChildRole::Reviewer if writes != 0 => {
            return Err(LaneError::new(
                Denial::MalformedGrant,
                "reviewer grant contains a write path",
            ));
        }
        ChildRole::Writer | ChildRole::Reviewer => {}
    }
    Ok(())
}

fn identity_digits(identity: &FileIdentity) -> bool {
    !identity.volume_serial.is_empty()
        && !identity.file_index.is_empty()
        && identity
            .volume_serial
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        && identity
            .file_index
            .bytes()
            .all(|byte| byte.is_ascii_digit())
}

impl GrantFile {
    fn from_record(record: &GrantRecord) -> Self {
        Self {
            schema_version: record.schema_version,
            grant_id: record.grant_id.clone(),
            state: record.state,
            lane_id: record.lane_id.clone(),
            role: record.role,
            canonical_cwd: record.canonical_cwd.clone(),
            cwd_identity: record.cwd_identity.clone(),
            worktree_common_dir: record.worktree_common_dir.clone(),
            common_dir_identity: record.common_dir_identity.clone(),
            base_commit: record.base_commit.clone(),
            path_grants: record.path_grants.clone(),
            model: record.model.clone(),
            depth: record.depth,
            child_slot: record.child_slot,
            issued_at_ms: record.issued_at_ms,
            expires_at_ms: record.expires_at_ms,
            consumed_at_ms: record.consumed_at_ms,
            metadata_hash: record.metadata_hash.clone(),
        }
    }

    fn into_record(self) -> Result<GrantRecord, LaneError> {
        Ok(GrantRecord {
            schema_version: self.schema_version,
            grant_id: self.grant_id,
            state: self.state,
            lane_id: self.lane_id,
            role: self.role,
            canonical_cwd: self.canonical_cwd,
            cwd_identity: self.cwd_identity,
            worktree_common_dir: self.worktree_common_dir,
            common_dir_identity: self.common_dir_identity,
            base_commit: self.base_commit,
            path_grants: self.path_grants,
            model: self.model,
            depth: self.depth,
            child_slot: self.child_slot,
            issued_at_ms: self.issued_at_ms,
            expires_at_ms: self.expires_at_ms,
            consumed_at_ms: self.consumed_at_ms,
            metadata_hash: self.metadata_hash,
        })
    }
}

pub fn contains_forbidden_material(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return true;
    };
    forbidden_key(&value)
}

fn forbidden_key(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, child)| {
            matches!(
                key.as_str(),
                "prompt"
                    | "reply"
                    | "content"
                    | "command"
                    | "env"
                    | "environment"
                    | "credential"
                    | "credentials"
                    | "secret"
                    | "token"
                    | "authorization"
            ) || forbidden_key(child)
        }),
        serde_json::Value::Array(items) => items.iter().any(forbidden_key),
        _ => false,
    }
}
