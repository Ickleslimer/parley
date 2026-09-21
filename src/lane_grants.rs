use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::context::refuse_reparse_chain;
use crate::json::Json;
use crate::sha256::{hex_encode, sha256};

const SCHEMA_VERSION: u64 = 1;
const MAX_GRANT_BYTES: usize = 64 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrantState {
    Prepared,
    Claimed,
    Running,
    Consumed,
}

impl GrantState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Claimed => "claimed",
            Self::Running => "running",
            Self::Consumed => "consumed",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "claimed" => Ok(Self::Claimed),
            "running" => Ok(Self::Running),
            "consumed" => Ok(Self::Consumed),
            _ => Err("lane grant state is invalid".to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChildRole {
    Writer,
    Reviewer,
}

impl ChildRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Writer => "two-chairs-writer",
            Self::Reviewer => "two-chairs-reviewer",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "two-chairs-writer" => Ok(Self::Writer),
            "two-chairs-reviewer" => Ok(Self::Reviewer),
            _ => Err("lane grant role is invalid".to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrantKind {
    File,
    Tree,
}

impl GrantKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Tree => "tree",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "file" => Ok(Self::File),
            "tree" => Ok(Self::Tree),
            _ => Err("lane path grant kind is invalid".to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    volume_serial: String,
    file_index: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PathGrant {
    pub(crate) kind: GrantKind,
    pub(crate) path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct GrantDraft {
    pub(crate) grant_id: String,
    pub(crate) lane_id: String,
    pub(crate) role: ChildRole,
    pub(crate) canonical_cwd: PathBuf,
    pub(crate) worktree_common_dir: PathBuf,
    pub(crate) base_commit: String,
    pub(crate) path_grants: Vec<PathGrant>,
    pub(crate) model: String,
    pub(crate) reasoning_effort: String,
    pub(crate) child_slot: u64,
    pub(crate) issued_at_ms: u64,
    pub(crate) expires_at_ms: u64,
}

#[derive(Clone, Debug)]
struct GrantRecord {
    schema_version: u64,
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
    reasoning_effort: String,
    depth: u64,
    child_slot: u64,
    issued_at_ms: u64,
    expires_at_ms: u64,
    parent_session_id: Option<String>,
    spawn_tool_use_id: Option<String>,
    child_session_id: Option<String>,
    claimed_at_ms: Option<u64>,
    started_at_ms: Option<u64>,
    consumed_at_ms: Option<u64>,
    metadata_hash: String,
}

pub(crate) struct GrantSet {
    state_dir: PathBuf,
    grant_ids: Vec<String>,
}

impl GrantSet {
    pub(crate) fn create(state_dir: &Path, drafts: &[GrantDraft]) -> Result<Self, String> {
        if drafts.is_empty() || drafts.len() > 2 {
            return Err("guarded launches require one or two child grants".to_string());
        }
        prepare_state_dir(state_dir)?;
        let _lock = acquire_lock(state_dir)?;
        let mut existing = load_all(state_dir)?;
        let mut grant_ids = Vec::with_capacity(drafts.len());
        for draft in drafts {
            let record = match prepare_record(draft).and_then(|record| {
                validate_capacity(&existing, &record)?;
                atomic_create_new(&grant_path(state_dir, &record.grant_id)?, &record.seal()?)?;
                Ok(record)
            }) {
                Ok(record) => record,
                Err(error) => {
                    let cleanup = consume_created(state_dir, &grant_ids).err();
                    return Err(append_cleanup_error(error, cleanup));
                }
            };
            grant_ids.push(record.grant_id.clone());
            existing.push(record);
        }
        Ok(Self {
            state_dir: state_dir.to_path_buf(),
            grant_ids,
        })
    }

    pub(crate) fn consume(&self) -> Result<(), String> {
        let _lock = acquire_lock(&self.state_dir)?;
        let consumed_at_ms = now_ms()?;
        let mut failures = Vec::new();
        for grant_id in &self.grant_ids {
            if let Err(error) = consume_one(&self.state_dir, grant_id, consumed_at_ms) {
                failures.push(format!("{grant_id}: {error}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "consume guarded child grants: {}",
                failures.join("; ")
            ))
        }
    }
}

fn consume_created(state_dir: &Path, grant_ids: &[String]) -> Result<(), String> {
    let consumed_at_ms = now_ms()?;
    let mut failures = Vec::new();
    for grant_id in grant_ids {
        if let Err(error) = consume_one(state_dir, grant_id, consumed_at_ms) {
            failures.push(format!("{grant_id}: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn append_cleanup_error(mut error: String, cleanup: Option<String>) -> String {
    if let Some(cleanup) = cleanup {
        error.push_str(&format!("; partial grant cleanup also failed: {cleanup}"));
    }
    error
}

fn prepare_record(draft: &GrantDraft) -> Result<GrantRecord, String> {
    if !safe_token(&draft.grant_id) || !safe_token(&draft.lane_id) {
        return Err("lane grant identifiers are invalid".to_string());
    }
    if draft.child_slot > 1
        || draft.expires_at_ms <= draft.issued_at_ms
        || draft.path_grants.len() > 32
    {
        return Err("lane grant bounds are invalid".to_string());
    }
    if draft.model != "grok-4.7" || draft.reasoning_effort != "xhigh" {
        return Err("guarded child grants require grok-4.7/xhigh".to_string());
    }
    if draft.base_commit.len() != 40
        || !draft
            .base_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("lane grant base commit is invalid".to_string());
    }
    let canonical_cwd = observe_directory(&draft.canonical_cwd)?;
    let common_dir = observe_directory(&draft.worktree_common_dir)?;
    confirm_git_binding(
        &canonical_cwd.0,
        &common_dir.0,
        &common_dir.1,
        &draft.base_commit,
    )?;
    let grants = normalize_grants(&canonical_cwd.0, draft.role, &draft.path_grants)?;
    Ok(GrantRecord {
        schema_version: SCHEMA_VERSION,
        grant_id: draft.grant_id.clone(),
        state: GrantState::Prepared,
        lane_id: draft.lane_id.clone(),
        role: draft.role,
        canonical_cwd: canonical_cwd.0.to_string_lossy().into_owned(),
        cwd_identity: canonical_cwd.1,
        worktree_common_dir: common_dir.0.to_string_lossy().into_owned(),
        common_dir_identity: common_dir.1,
        base_commit: draft.base_commit.to_ascii_lowercase(),
        path_grants: grants,
        model: draft.model.clone(),
        reasoning_effort: draft.reasoning_effort.clone(),
        depth: 1,
        child_slot: draft.child_slot,
        issued_at_ms: draft.issued_at_ms,
        expires_at_ms: draft.expires_at_ms,
        parent_session_id: None,
        spawn_tool_use_id: None,
        child_session_id: None,
        claimed_at_ms: None,
        started_at_ms: None,
        consumed_at_ms: None,
        metadata_hash: String::new(),
    })
}

fn consume_one(state_dir: &Path, grant_id: &str, consumed_at_ms: u64) -> Result<(), String> {
    let path = grant_path(state_dir, grant_id)?;
    let mut record = read_record(&path)?;
    if record.state == GrantState::Consumed || record.consumed_at_ms.is_some() {
        return Err("grant is already consumed".to_string());
    }
    confirm_record_binding(&record)?;
    record.state = GrantState::Consumed;
    record.consumed_at_ms = Some(consumed_at_ms.max(record.issued_at_ms));
    record.metadata_hash.clear();
    atomic_replace(&path, &record.seal()?)?;
    let durable = read_record(&path)?;
    if durable.state != GrantState::Consumed {
        return Err("consumed transition was not durable".to_string());
    }
    Ok(())
}

impl GrantRecord {
    fn seal(&self) -> Result<Vec<u8>, String> {
        let body = self.json_without_hash();
        let digest = hex_encode(&sha256(body.as_bytes()));
        let full = format!(
            "{},\"metadata_hash\":\"{}\"}}",
            body.strip_suffix('}')
                .ok_or_else(|| "grant serialization failed".to_string())?,
            digest
        );
        if full.len() > MAX_GRANT_BYTES {
            return Err("lane grant exceeds 64 KiB".to_string());
        }
        Ok(full.into_bytes())
    }

    fn json_without_hash(&self) -> String {
        let mut fields = Vec::with_capacity(23);
        fields.push(format!("\"schema_version\":{}", self.schema_version));
        fields.push(string_field("grant_id", &self.grant_id));
        fields.push(string_field("state", self.state.as_str()));
        fields.push(string_field("lane_id", &self.lane_id));
        fields.push(string_field("role", self.role.as_str()));
        fields.push(string_field("canonical_cwd", &self.canonical_cwd));
        fields.push(format!(
            "\"cwd_identity\":{}",
            identity_json(&self.cwd_identity)
        ));
        fields.push(string_field(
            "worktree_common_dir",
            &self.worktree_common_dir,
        ));
        fields.push(format!(
            "\"common_dir_identity\":{}",
            identity_json(&self.common_dir_identity)
        ));
        fields.push(string_field("base_commit", &self.base_commit));
        fields.push(format!(
            "\"path_grants\":[{}]",
            self.path_grants
                .iter()
                .map(path_grant_json)
                .collect::<Vec<_>>()
                .join(",")
        ));
        fields.push(string_field("model", &self.model));
        fields.push(string_field("reasoning_effort", &self.reasoning_effort));
        fields.push(format!("\"depth\":{}", self.depth));
        fields.push(format!("\"child_slot\":{}", self.child_slot));
        fields.push(format!("\"issued_at_ms\":{}", self.issued_at_ms));
        fields.push(format!("\"expires_at_ms\":{}", self.expires_at_ms));
        fields.push(option_string_field(
            "parent_session_id",
            self.parent_session_id.as_deref(),
        ));
        fields.push(option_string_field(
            "spawn_tool_use_id",
            self.spawn_tool_use_id.as_deref(),
        ));
        fields.push(option_string_field(
            "child_session_id",
            self.child_session_id.as_deref(),
        ));
        fields.push(option_u64_field("claimed_at_ms", self.claimed_at_ms));
        fields.push(option_u64_field("started_at_ms", self.started_at_ms));
        fields.push(option_u64_field("consumed_at_ms", self.consumed_at_ms));
        format!("{{{}}}", fields.join(","))
    }
}

fn read_record(path: &Path) -> Result<GrantRecord, String> {
    let bytes = read_limited(path, MAX_GRANT_BYTES)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "lane grant is not UTF-8".to_string())?;
    let value = Json::parse(text).map_err(|_| "lane grant JSON is malformed".to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "lane grant JSON is not an object".to_string())?;
    let expected = [
        "schema_version",
        "grant_id",
        "state",
        "lane_id",
        "role",
        "canonical_cwd",
        "cwd_identity",
        "worktree_common_dir",
        "common_dir_identity",
        "base_commit",
        "path_grants",
        "model",
        "reasoning_effort",
        "depth",
        "child_slot",
        "issued_at_ms",
        "expires_at_ms",
        "parent_session_id",
        "spawn_tool_use_id",
        "child_session_id",
        "claimed_at_ms",
        "started_at_ms",
        "consumed_at_ms",
        "metadata_hash",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if object.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err("lane grant fields do not match schema v1".to_string());
    }
    let record = GrantRecord {
        schema_version: required_u64(&value, "schema_version")?,
        grant_id: required_string(&value, "grant_id")?,
        state: GrantState::parse(&required_string(&value, "state")?)?,
        lane_id: required_string(&value, "lane_id")?,
        role: ChildRole::parse(&required_string(&value, "role")?)?,
        canonical_cwd: required_string(&value, "canonical_cwd")?,
        cwd_identity: parse_identity(
            value
                .get("cwd_identity")
                .ok_or_else(|| "lane grant cwd identity is missing".to_string())?,
        )?,
        worktree_common_dir: required_string(&value, "worktree_common_dir")?,
        common_dir_identity: parse_identity(
            value
                .get("common_dir_identity")
                .ok_or_else(|| "lane grant common-dir identity is missing".to_string())?,
        )?,
        base_commit: required_string(&value, "base_commit")?,
        path_grants: parse_path_grants(
            value
                .get("path_grants")
                .ok_or_else(|| "lane path grants are missing".to_string())?,
        )?,
        model: required_string(&value, "model")?,
        reasoning_effort: required_string(&value, "reasoning_effort")?,
        depth: required_u64(&value, "depth")?,
        child_slot: required_u64(&value, "child_slot")?,
        issued_at_ms: required_u64(&value, "issued_at_ms")?,
        expires_at_ms: required_u64(&value, "expires_at_ms")?,
        parent_session_id: optional_string(&value, "parent_session_id")?,
        spawn_tool_use_id: optional_string(&value, "spawn_tool_use_id")?,
        child_session_id: optional_string(&value, "child_session_id")?,
        claimed_at_ms: optional_u64(&value, "claimed_at_ms")?,
        started_at_ms: optional_u64(&value, "started_at_ms")?,
        consumed_at_ms: optional_u64(&value, "consumed_at_ms")?,
        metadata_hash: required_string(&value, "metadata_hash")?,
    };
    validate_record(&record)?;
    let expected_hash = hex_encode(&sha256(record.json_without_hash().as_bytes()));
    if record.metadata_hash != expected_hash {
        return Err("lane grant metadata hash does not match".to_string());
    }
    Ok(record)
}

fn validate_record(record: &GrantRecord) -> Result<(), String> {
    if record.schema_version != SCHEMA_VERSION
        || !safe_token(&record.grant_id)
        || !safe_token(&record.lane_id)
        || record.depth != 1
        || record.child_slot > 1
        || record.expires_at_ms <= record.issued_at_ms
        || record.path_grants.len() > 32
        || record.metadata_hash.len() != 64
        || !record
            .metadata_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("lane grant metadata is invalid".to_string());
    }
    let transition_valid = match record.state {
        GrantState::Prepared => {
            record.parent_session_id.is_none()
                && record.spawn_tool_use_id.is_none()
                && record.child_session_id.is_none()
                && record.claimed_at_ms.is_none()
                && record.started_at_ms.is_none()
                && record.consumed_at_ms.is_none()
        }
        GrantState::Claimed => {
            record.parent_session_id.is_some()
                && record.spawn_tool_use_id.is_some()
                && record.child_session_id.is_none()
                && record.claimed_at_ms.is_some()
                && record.started_at_ms.is_none()
                && record.consumed_at_ms.is_none()
        }
        GrantState::Running => {
            record.parent_session_id.is_some()
                && record.spawn_tool_use_id.is_some()
                && record.child_session_id.is_some()
                && record.claimed_at_ms.is_some()
                && record.started_at_ms.is_some()
                && record.consumed_at_ms.is_none()
        }
        GrantState::Consumed => record.consumed_at_ms.is_some(),
    };
    if !transition_valid {
        return Err("lane grant lifecycle fields are invalid".to_string());
    }
    Ok(())
}

fn parse_identity(value: &Json) -> Result<FileIdentity, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "lane file identity is not an object".to_string())?;
    if object.len() != 2
        || !object.contains_key("volume_serial")
        || !object.contains_key("file_index")
    {
        return Err("lane file identity fields are invalid".to_string());
    }
    let volume_serial = required_string(value, "volume_serial")?;
    let file_index = required_string(value, "file_index")?;
    if !volume_serial.bytes().all(|byte| byte.is_ascii_digit())
        || !file_index.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("lane file identity values are invalid".to_string());
    }
    Ok(FileIdentity {
        volume_serial,
        file_index,
    })
}

fn parse_path_grants(value: &Json) -> Result<Vec<PathGrant>, String> {
    value
        .as_array()
        .ok_or_else(|| "lane path grants are not an array".to_string())?
        .iter()
        .map(|entry| {
            let object = entry
                .as_object()
                .ok_or_else(|| "lane path grant is not an object".to_string())?;
            if object.len() != 3 || object.get("access").and_then(Json::as_str) != Some("write") {
                return Err("lane path grant fields are invalid".to_string());
            }
            Ok(PathGrant {
                kind: GrantKind::parse(&required_string(entry, "kind")?)?,
                path: required_string(entry, "path")?,
            })
        })
        .collect()
}

fn string_field(name: &str, value: &str) -> String {
    format!("\"{name}\":\"{}\"", strict_escape_json(value))
}

fn option_string_field(name: &str, value: Option<&str>) -> String {
    match value {
        Some(value) => string_field(name, value),
        None => format!("\"{name}\":null"),
    }
}

fn option_u64_field(name: &str, value: Option<u64>) -> String {
    match value {
        Some(value) => format!("\"{name}\":{value}"),
        None => format!("\"{name}\":null"),
    }
}

fn identity_json(identity: &FileIdentity) -> String {
    format!(
        "{{\"volume_serial\":\"{}\",\"file_index\":\"{}\"}}",
        strict_escape_json(&identity.volume_serial),
        strict_escape_json(&identity.file_index)
    )
}

fn path_grant_json(grant: &PathGrant) -> String {
    format!(
        "{{\"access\":\"write\",\"kind\":\"{}\",\"path\":\"{}\"}}",
        grant.kind.as_str(),
        strict_escape_json(&grant.path)
    )
}

fn strict_escape_json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character <= '\u{1f}' => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

fn required_string(value: &Json, name: &str) -> Result<String, String> {
    value
        .get(name)
        .and_then(Json::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("lane grant {name} is missing or invalid"))
}

fn optional_string(value: &Json, name: &str) -> Result<Option<String>, String> {
    match value.get(name) {
        Some(Json::Null) => Ok(None),
        Some(Json::Str(value)) if !value.is_empty() => Ok(Some(value.clone())),
        _ => Err(format!("lane grant {name} is invalid")),
    }
}

fn required_u64(value: &Json, name: &str) -> Result<u64, String> {
    value
        .get(name)
        .and_then(Json::as_number)
        .filter(|number| number.is_finite() && *number >= 0.0 && number.fract() == 0.0)
        .and_then(|number| {
            let converted = number as u64;
            ((converted as f64) == number).then_some(converted)
        })
        .ok_or_else(|| format!("lane grant {name} is invalid"))
}

fn optional_u64(value: &Json, name: &str) -> Result<Option<u64>, String> {
    match value.get(name) {
        Some(Json::Null) => Ok(None),
        Some(_) => required_u64(value, name).map(Some),
        None => Err(format!("lane grant {name} is missing")),
    }
}

fn safe_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn now_ms() -> Result<u64, String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_string())?;
    u64::try_from(duration.as_millis()).map_err(|_| "system clock is out of range".to_string())
}

fn grant_path(state_dir: &Path, grant_id: &str) -> Result<PathBuf, String> {
    if !safe_token(grant_id) {
        return Err("lane grant id is invalid".to_string());
    }
    Ok(state_dir.join("grants").join(format!("{grant_id}.json")))
}

fn prepare_state_dir(state_dir: &Path) -> Result<(), String> {
    if !state_dir.is_absolute() {
        return Err("lane state directory must be absolute".to_string());
    }
    fs::create_dir_all(state_dir)
        .map_err(|error| format!("create lane state directory: {error}"))?;
    refuse_reparse_chain(state_dir).map_err(|error| error.to_string())?;
    let grants = state_dir.join("grants");
    fs::create_dir_all(&grants)
        .map_err(|error| format!("create lane grants directory: {error}"))?;
    refuse_reparse_chain(&grants).map_err(|error| error.to_string())
}

struct StateLock {
    _file: File,
}

fn acquire_lock(state_dir: &Path) -> Result<StateLock, String> {
    let path = state_dir.join("grants.lock");
    refuse_reparse_chain(&path).map_err(|error| error.to_string())?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options
        .open(&path)
        .map(|file| StateLock { _file: file })
        .map_err(|error| format!("acquire lane grant lock: {error}"))
}

fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_share_all(&mut options);
    let mut file = options
        .open(path)
        .map_err(|error| format!("open lane grant {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|error| format!("read lane grant {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > limit {
            return Err("lane grant exceeds 64 KiB".to_string());
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(bytes)
}

fn apply_share_all(options: &mut OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0x1 | 0x2 | 0x4);
    }
    #[cfg(not(windows))]
    let _ = options;
}

fn load_all(state_dir: &Path) -> Result<Vec<GrantRecord>, String> {
    let grants_dir = state_dir.join("grants");
    let mut records = Vec::new();
    for entry in
        fs::read_dir(&grants_dir).map_err(|error| format!("read lane grants directory: {error}"))?
    {
        let entry = entry.map_err(|error| format!("read lane grant entry: {error}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("inspect lane grant entry: {error}"))?;
        if metadata_is_reparse(&metadata) || !metadata.is_file() {
            return Err("lane grant entry is not a real file".to_string());
        }
        let grant_id = name.trim_end_matches(".json");
        let record = read_record(&entry.path())?;
        if record.grant_id != grant_id {
            return Err("lane grant file name does not match its id".to_string());
        }
        records.push(record);
    }
    Ok(records)
}

fn validate_capacity(existing: &[GrantRecord], candidate: &GrantRecord) -> Result<(), String> {
    let live = existing
        .iter()
        .filter(|record| {
            record.state != GrantState::Consumed
                && record.consumed_at_ms.is_none()
                && record.issued_at_ms < candidate.expires_at_ms
                && candidate.issued_at_ms < record.expires_at_ms
        })
        .collect::<Vec<_>>();
    if live.iter().any(|record| {
        record.grant_id == candidate.grant_id
            || record.lane_id.eq_ignore_ascii_case(&candidate.lane_id)
            || record.cwd_identity == candidate.cwd_identity
    }) {
        return Err("a live lane grant already owns this identifier or worktree".to_string());
    }
    let group = live
        .iter()
        .filter(|record| {
            record.common_dir_identity == candidate.common_dir_identity
                && record
                    .base_commit
                    .eq_ignore_ascii_case(&candidate.base_commit)
        })
        .collect::<Vec<_>>();
    if group.len() >= 2
        || group
            .iter()
            .any(|record| record.child_slot == candidate.child_slot)
    {
        return Err("the lane base already has two children or this child slot".to_string());
    }
    Ok(())
}

fn normalize_grants(
    cwd: &Path,
    role: ChildRole,
    grants: &[PathGrant],
) -> Result<Vec<PathGrant>, String> {
    if role == ChildRole::Reviewer && !grants.is_empty() {
        return Err("reviewer child grants may not contain write paths".to_string());
    }
    if role == ChildRole::Writer && grants.is_empty() {
        return Err("writer child grants require a write path".to_string());
    }
    let mut normalized = Vec::with_capacity(grants.len());
    for grant in grants {
        let parts = normalize_relative(&grant.path)?;
        let target = parts
            .iter()
            .fold(cwd.to_path_buf(), |path, part| path.join(part));
        refuse_reparse_chain(&target).map_err(|error| error.to_string())?;
        validate_grant_target(&target, grant.kind)?;
        let path = parts.join("/");
        let candidate = PathGrant {
            kind: grant.kind,
            path,
        };
        if normalized
            .iter()
            .any(|other| path_grants_overlap(other, &candidate))
        {
            return Err("child write grants overlap".to_string());
        }
        normalized.push(candidate);
    }
    Ok(normalized)
}

fn normalize_relative(value: &str) -> Result<Vec<String>, String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || value
            .chars()
            .any(|character| matches!(character, '*' | '?' | '[' | ']' | '{' | '}'))
    {
        return Err("lane path grant is not a safe relative path".to_string());
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => {
                let text = part.to_string_lossy();
                if text.eq_ignore_ascii_case(".git") && parts.is_empty() {
                    return Err("lane grants may not enter Git administration".to_string());
                }
                parts.push(text.into_owned());
            }
            _ => return Err("lane path grant contains traversal".to_string()),
        }
    }
    if parts.is_empty() {
        return Err("lane path grant is empty".to_string());
    }
    Ok(parts)
}

fn validate_grant_target(target: &Path, kind: GrantKind) -> Result<(), String> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata_is_reparse(&metadata) => {
            Err("lane grant target is a reparse point".to_string())
        }
        Ok(metadata) if kind == GrantKind::File && !metadata.is_file() => {
            Err("lane file grant target is not a file".to_string())
        }
        Ok(metadata) if kind == GrantKind::Tree && !metadata.is_dir() => {
            Err("lane tree grant target is not a directory".to_string())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && kind == GrantKind::File => {
            let parent = target
                .parent()
                .ok_or_else(|| "lane file grant has no parent".to_string())?;
            let metadata = fs::symlink_metadata(parent)
                .map_err(|error| format!("inspect lane file grant parent: {error}"))?;
            if metadata_is_reparse(&metadata) || !metadata.is_dir() {
                Err("lane file grant parent is not a real directory".to_string())
            } else {
                Ok(())
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err("lane tree grant target does not exist".to_string())
        }
        Err(error) => Err(format!("inspect lane grant target: {error}")),
    }
}

fn path_grants_overlap(left: &PathGrant, right: &PathGrant) -> bool {
    let left_parts = left.path.split('/').collect::<Vec<_>>();
    let right_parts = right.path.split('/').collect::<Vec<_>>();
    left_parts == right_parts
        || (left.kind == GrantKind::Tree && right_parts.starts_with(&left_parts))
        || (right.kind == GrantKind::Tree && left_parts.starts_with(&right_parts))
}

fn observe_directory(path: &Path) -> Result<(PathBuf, FileIdentity), String> {
    if !path.is_absolute() {
        return Err("lane directory must be absolute".to_string());
    }
    refuse_reparse_chain(path).map_err(|error| error.to_string())?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect lane directory {}: {error}", path.display()))?;
    if metadata_is_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!("lane directory is not real: {}", path.display()));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("canonicalize lane directory {}: {error}", path.display()))?;
    refuse_reparse_chain(&canonical).map_err(|error| error.to_string())?;
    let identity = file_identity(&canonical)?;
    Ok((canonical, identity))
}

fn confirm_record_binding(record: &GrantRecord) -> Result<(), String> {
    let (cwd, cwd_identity) = observe_directory(Path::new(&record.canonical_cwd))?;
    let (common, common_identity) = observe_directory(Path::new(&record.worktree_common_dir))?;
    if cwd_identity != record.cwd_identity || common_identity != record.common_dir_identity {
        return Err("lane grant directory identity changed".to_string());
    }
    confirm_git_binding(&cwd, &common, &common_identity, &record.base_commit)
}

fn confirm_git_binding(
    cwd: &Path,
    expected_common: &Path,
    expected_common_identity: &FileIdentity,
    expected_head: &str,
) -> Result<(), String> {
    let top_level = git_output(cwd, &["rev-parse", "--show-toplevel"])?;
    let top_level = fs::canonicalize(top_level.trim())
        .map_err(|error| format!("canonicalize Git top-level: {error}"))?;
    if path_key(&top_level) != path_key(cwd) {
        return Err("lane directory is no longer a Git worktree root".to_string());
    }
    let common = git_output(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let (common, common_identity) = observe_directory(Path::new(common.trim()))?;
    if path_key(&common) != path_key(expected_common)
        || &common_identity != expected_common_identity
    {
        return Err("lane Git common-dir identity changed".to_string());
    }
    let head = git_output(cwd, &["rev-parse", "--verify", "HEAD"])?;
    if !head.trim().eq_ignore_ascii_case(expected_head) {
        return Err("lane Git HEAD no longer matches the job base".to_string());
    }
    Ok(())
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("run Git lane verification: {error}"))?;
    if !output.status.success() {
        return Err("Git lane verification failed".to_string());
    }
    String::from_utf8(output.stdout).map_err(|_| "Git lane output is not UTF-8".to_string())
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase()
}

fn atomic_create_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    atomic_move(path, bytes, false)
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect existing lane grant: {error}"))?;
    if metadata_is_reparse(&metadata) || !metadata.is_file() {
        return Err("existing lane grant is not a real file".to_string());
    }
    atomic_move(path, bytes, true)
}

fn atomic_move(destination: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "lane grant path has no parent".to_string())?;
    refuse_reparse_chain(parent).map_err(|error| error.to_string())?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".grant.{}.{sequence}.tmp", process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("create temporary lane grant: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("write temporary lane grant: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("flush temporary lane grant: {error}"))?;
        drop(file);
        move_file(&temporary, destination, replace)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn move_file(source: &Path, destination: &Path, replace: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, target: *const u16, flags: u32) -> i32;
    }

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let flags = 0x8 | if replace { 0x1 } else { 0 };
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) } == 0 {
        return Err(format!(
            "publish lane grant: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn move_file(_source: &Path, _destination: &Path, _replace: bool) -> Result<(), String> {
    Err("atomic lane grant publication requires Windows".to_string())
}

#[cfg(windows)]
fn file_identity(path: &Path) -> Result<FileIdentity, String> {
    use std::ffi::c_void;
    use std::mem::zeroed;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct ByHandleFileInformation {
        file_attributes: u32,
        creation_time_low: u32,
        creation_time_high: u32,
        last_access_time_low: u32,
        last_access_time_high: u32,
        last_write_time_low: u32,
        last_write_time_high: u32,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(handle: *mut c_void, information: *mut c_void) -> i32;
    }

    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(0x0200_0000)
        .share_mode(0x1 | 0x2 | 0x4);
    let file = options
        .open(path)
        .map_err(|error| format!("open lane path identity {}: {error}", path.display()))?;
    let mut information = unsafe { zeroed::<ByHandleFileInformation>() };
    if unsafe {
        GetFileInformationByHandle(
            file.as_raw_handle(),
            (&mut information as *mut ByHandleFileInformation).cast(),
        )
    } == 0
    {
        return Err(format!(
            "read lane path identity: {}",
            std::io::Error::last_os_error()
        ));
    }
    let file_index =
        (u64::from(information.file_index_high) << 32) | u64::from(information.file_index_low);
    Ok(FileIdentity {
        volume_serial: information.volume_serial_number.to_string(),
        file_index: file_index.to_string(),
    })
}

#[cfg(not(windows))]
fn file_identity(_path: &Path) -> Result<FileIdentity, String> {
    Err("lane path identity requires Windows".to_string())
}

fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> GrantRecord {
        GrantRecord {
            schema_version: 1,
            grant_id: "00000000-0000-4000-8000-000000000000-c0".to_string(),
            state: GrantState::Prepared,
            lane_id: "child-1".to_string(),
            role: ChildRole::Writer,
            canonical_cwd: r"C:\repo\child".to_string(),
            cwd_identity: FileIdentity {
                volume_serial: "12".to_string(),
                file_index: "34".to_string(),
            },
            worktree_common_dir: r"C:\repo\.git".to_string(),
            common_dir_identity: FileIdentity {
                volume_serial: "12".to_string(),
                file_index: "56".to_string(),
            },
            base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            path_grants: vec![PathGrant {
                kind: GrantKind::Tree,
                path: "src/jobs".to_string(),
            }],
            model: "grok-4.7".to_string(),
            reasoning_effort: "xhigh".to_string(),
            depth: 1,
            child_slot: 0,
            issued_at_ms: 1_000,
            expires_at_ms: 2_000,
            parent_session_id: None,
            spawn_tool_use_id: None,
            child_session_id: None,
            claimed_at_ms: None,
            started_at_ms: None,
            consumed_at_ms: None,
            metadata_hash: String::new(),
        }
    }

    #[test]
    fn seals_and_parses_the_hook_schema() {
        let bytes = sample_record().seal().unwrap();
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            include_str!("../lanes/tests/fixtures/root-grant-v1.json").trim_end()
        );
        let path = std::env::temp_dir().join(format!(
            "parley-lane-grant-schema-{}-{}.json",
            process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, &bytes).unwrap();
        let parsed = read_record(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(parsed.state, GrantState::Prepared);
        assert_eq!(parsed.role, ChildRole::Writer);
        assert_eq!(parsed.path_grants[0].path, "src/jobs");
    }

    #[test]
    fn child_grants_overlap_by_logical_path() {
        let tree = PathGrant {
            kind: GrantKind::Tree,
            path: "src/jobs".to_string(),
        };
        let file = PathGrant {
            kind: GrantKind::File,
            path: "src/jobs/runtime.rs".to_string(),
        };
        let sibling = PathGrant {
            kind: GrantKind::Tree,
            path: "src/jobs-old".to_string(),
        };
        assert!(path_grants_overlap(&tree, &file));
        assert!(!path_grants_overlap(&tree, &sibling));
    }

    #[test]
    fn strict_json_escaping_covers_control_characters() {
        assert_eq!(strict_escape_json("a\u{1}b\n"), "a\\u0001b\\n");
        assert_eq!(
            crate::json::escape_json("simple"),
            strict_escape_json("simple")
        );
    }
}
