//! Durable schema-v1 job journal.
//!
//! The JSONL file is authoritative. `snapshot.json` is an atomic copy of the
//! latest record and is rebuilt from the journal when it is missing or stale.
//! Records store a request fingerprint, never prompt text, replies, credentials,
//! environment values, or command arguments. `JobView.reply` stays unset.
//!
//! `recover_startup` is explicit. This module does not inspect worker processes.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ask::ContextRef;
use crate::context::ContextRecovery;
use crate::fsx;
use crate::json::Json;
use crate::signals::fnv1a_64;

use super::{JobMode, JobSpec, JobState, JobView, LaneGrant, LanePlan, PathGrant};

const JOB_SCHEMA_VERSION: u64 = 1;
const MAX_RECORD_BYTES: usize = 65_536;
const MAX_ERROR_CHARS: usize = 4_096;
pub(crate) const MAX_LISTED_JOBS: usize = 100;
pub(crate) const STARTUP_RECOVERY_ERROR: &str = "startup_recovery";

const RECORD_FIELDS: &[&str] = &[
    "completion_event_id",
    "created_at_ms",
    "error",
    "exchange_id",
    "job_id",
    "mode",
    "record_type",
    "request_event_id",
    "request_fingerprint",
    "schema_version",
    "sequence",
    "session_id",
    "state",
    "updated_at_ms",
];

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobErrorKind {
    Invalid,
    Conflict,
    Corrupt,
    Io,
    Locked,
    IllegalTransition,
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JobError {
    pub kind: JobErrorKind,
    pub message: String,
}

impl JobError {
    fn new(kind: JobErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(JobErrorKind::Invalid, message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(JobErrorKind::Conflict, message)
    }

    fn corrupt(message: impl Into<String>) -> Self {
        Self::new(JobErrorKind::Corrupt, message)
    }

    fn io(message: impl Into<String>) -> Self {
        Self::new(JobErrorKind::Io, message)
    }

    fn locked(message: impl Into<String>) -> Self {
        Self::new(JobErrorKind::Locked, message)
    }

    fn illegal(message: impl Into<String>) -> Self {
        Self::new(JobErrorKind::IllegalTransition, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(JobErrorKind::NotFound, message)
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for JobError {}

#[derive(Clone, Debug, Default)]
pub(crate) struct JobStateEnv {
    pub job_state_dir: Option<PathBuf>,
    pub caller: Option<String>,
    pub localappdata: Option<PathBuf>,
}

impl JobStateEnv {
    pub(crate) fn from_process() -> Self {
        Self {
            job_state_dir: env_path("PARLEY_JOB_STATE_DIR"),
            caller: env_string("PARLEY_CALLER"),
            localappdata: env_path("LOCALAPPDATA"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NamespacePaths {
    pub root: PathBuf,
    pub caller: String,
}

impl NamespacePaths {
    pub(crate) fn directory(&self) -> PathBuf {
        self.root.join(&self.caller)
    }
}

pub(crate) fn resolve_namespace(env: &JobStateEnv) -> Result<NamespacePaths, JobError> {
    let caller = normalize_caller(env.caller.clone())?;
    let root = match env
        .job_state_dir
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        Some(path) => path.clone(),
        None => default_root(&env.localappdata)?,
    };
    Ok(NamespacePaths { root, caller })
}

#[derive(Clone, Debug)]
pub(crate) struct JobTransition {
    pub state: JobState,
    pub exchange_id: Option<String>,
    pub session_id: Option<String>,
    pub request_event_id: Option<String>,
    pub completion_event_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum BeginOutcome {
    Created(JobView),
    Existing(JobView),
}

impl BeginOutcome {
    pub(crate) fn view(&self) -> &JobView {
        match self {
            Self::Created(view) | Self::Existing(view) => view,
        }
    }

    pub(crate) fn created(&self) -> bool {
        matches!(self, Self::Created(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct JobRecord {
    job_id: String,
    state: JobState,
    mode: JobMode,
    request_fingerprint: String,
    exchange_id: Option<String>,
    session_id: Option<String>,
    request_event_id: Option<String>,
    completion_event_id: Option<String>,
    error: Option<String>,
    created_at_ms: u128,
    updated_at_ms: u128,
    sequence: u64,
}

impl JobRecord {
    fn view(&self) -> JobView {
        JobView {
            job_id: self.job_id.clone(),
            state: self.state,
            exchange_id: self.exchange_id.clone(),
            session_id: self.session_id.clone(),
            request_event_id: self.request_event_id.clone(),
            completion_event_id: self.completion_event_id.clone(),
            error: self.error.clone(),
            reply: None,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
        }
    }
}

enum Clock {
    System,
    #[cfg(test)]
    Manual(std::sync::Arc<AtomicU64>),
}

pub(crate) struct JobStore {
    namespace: NamespacePaths,
    clock: Clock,
}

impl JobStore {
    pub(crate) fn open(env: &JobStateEnv) -> Result<Self, JobError> {
        Ok(Self {
            namespace: resolve_namespace(env)?,
            clock: Clock::System,
        })
    }

    pub(crate) fn open_at(root: impl Into<PathBuf>, caller: &str) -> Result<Self, JobError> {
        Self::open(&JobStateEnv {
            job_state_dir: Some(root.into()),
            caller: Some(caller.to_string()),
            localappdata: None,
        })
    }

    #[cfg(test)]
    fn open_with_clock(root: PathBuf, caller: &str, clock: Clock) -> Result<Self, JobError> {
        Ok(Self {
            namespace: resolve_namespace(&JobStateEnv {
                job_state_dir: Some(root),
                caller: Some(caller.to_string()),
                localappdata: None,
            })?,
            clock,
        })
    }

    pub(crate) fn namespace_dir(&self) -> PathBuf {
        self.namespace.directory()
    }

    pub(crate) fn begin(&self, spec: &JobSpec) -> Result<BeginOutcome, JobError> {
        let job_id = normalize_uuid(&spec.job_id)?;
        let fingerprint = request_fingerprint(spec)?;
        let session_id = initial_session(spec)?;
        let dir = self.ensure_job_dir(&job_id)?;
        let _lock = lock_job(&dir)?;
        if let Some(existing) = load_job(&dir, &job_id)? {
            if existing.request_fingerprint != fingerprint {
                return Err(JobError::conflict(format!(
                    "conflict: job {job_id} fingerprint {} does not match {fingerprint}",
                    existing.request_fingerprint
                )));
            }
            if existing.mode != spec.mode {
                return Err(JobError::corrupt(
                    "corrupt: stored job mode does not match the request fingerprint",
                ));
            }
            return Ok(BeginOutcome::Existing(existing.view()));
        }
        let now = self.now_ms();
        let record = JobRecord {
            job_id: job_id.clone(),
            state: JobState::Preparing,
            mode: spec.mode,
            request_fingerprint: fingerprint,
            exchange_id: None,
            session_id,
            request_event_id: None,
            completion_event_id: None,
            error: None,
            created_at_ms: now,
            updated_at_ms: now,
            sequence: 1,
        };
        commit_record(&dir, &record)?;
        Ok(BeginOutcome::Created(record.view()))
    }

    pub(crate) fn transition(
        &self,
        job_id: &str,
        transition: &JobTransition,
    ) -> Result<JobView, JobError> {
        let job_id = normalize_uuid(job_id)?;
        let dir = self.namespace.directory().join(&job_id);
        if !dir.exists() {
            return Err(JobError::not_found(format!("job {job_id} was not found")));
        }
        let _lock = lock_job(&dir)?;
        let Some(current) = load_job(&dir, &job_id)? else {
            return Err(JobError::not_found(format!("job {job_id} was not found")));
        };
        let next = build_next(&current, transition, self.now_ms())?;
        if next == current {
            return Ok(current.view());
        }
        commit_record(&dir, &next)?;
        Ok(next.view())
    }

    pub(crate) fn get(&self, job_id: &str) -> Result<Option<JobView>, JobError> {
        let job_id = normalize_uuid(job_id)?;
        let dir = self.namespace.directory().join(&job_id);
        refuse_symlink(&dir)?;
        if !dir.exists() {
            return Ok(None);
        }
        if !dir.is_dir() {
            return Err(JobError::corrupt(format!(
                "corrupt: job path is not a directory: {}",
                dir.display()
            )));
        }
        let _lock = lock_job(&dir)?;
        Ok(load_job(&dir, &job_id)?.map(|record| record.view()))
    }

    pub(crate) fn list(&self, limit: usize) -> Result<Vec<JobView>, JobError> {
        if limit > MAX_LISTED_JOBS {
            return Err(JobError::invalid(format!(
                "invalid: list limit {limit} exceeds {MAX_LISTED_JOBS}"
            )));
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for job_id in job_ids(&self.namespace)? {
            let dir = self.namespace.directory().join(&job_id);
            let _lock = lock_job(&dir)?;
            if let Some(record) = load_job(&dir, &job_id)? {
                records.push(record);
            }
        }
        records.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then(right.sequence.cmp(&left.sequence))
                .then(right.job_id.cmp(&left.job_id))
        });
        records.truncate(limit);
        Ok(records.into_iter().map(|record| record.view()).collect())
    }

    /// Convert durable `preparing`, `running`, and `cancelling` jobs to
    /// `interrupted`. Call this once at process startup after confirming no
    /// worker is live. A corrupt journal aborts the pass before any rewrite.
    pub(crate) fn recover_startup(&self) -> Result<Vec<JobView>, JobError> {
        let ids = job_ids(&self.namespace)?;
        for job_id in &ids {
            let dir = self.namespace.directory().join(job_id);
            let _lock = lock_job(&dir)?;
            let _ = load_job(&dir, job_id)?;
        }
        let mut changed = Vec::new();
        for job_id in ids {
            let dir = self.namespace.directory().join(&job_id);
            let _lock = lock_job(&dir)?;
            let Some(current) = load_job(&dir, &job_id)? else {
                continue;
            };
            if !matches!(
                current.state,
                JobState::Preparing | JobState::Running | JobState::Cancelling
            ) {
                continue;
            }
            let transition = JobTransition {
                state: JobState::Interrupted,
                exchange_id: None,
                session_id: None,
                request_event_id: None,
                completion_event_id: None,
                error: Some(STARTUP_RECOVERY_ERROR.to_string()),
            };
            let next = build_next(&current, &transition, self.now_ms())?;
            commit_record(&dir, &next)?;
            changed.push(next.view());
        }
        changed.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then(right.job_id.cmp(&left.job_id))
        });
        Ok(changed)
    }

    fn ensure_job_dir(&self, job_id: &str) -> Result<PathBuf, JobError> {
        prepare_dir(&self.namespace.root)?;
        prepare_dir(&self.namespace.directory())?;
        let dir = self.namespace.directory().join(job_id);
        prepare_dir(&dir)?;
        Ok(dir)
    }

    fn now_ms(&self) -> u128 {
        match &self.clock {
            Clock::System => system_now_ms(),
            #[cfg(test)]
            Clock::Manual(counter) => u128::from(counter.fetch_add(10, Ordering::Relaxed)),
        }
    }
}

pub(crate) fn request_fingerprint(spec: &JobSpec) -> Result<String, JobError> {
    let canonical = canonical_request_json(spec)?;
    Ok(format!("{:016x}", fnv1a_64(&canonical)))
}

fn canonical_request_json(spec: &JobSpec) -> Result<String, JobError> {
    if spec.request.harness.is_empty() {
        return Err(JobError::invalid("invalid: harness must not be empty"));
    }
    let mut map = BTreeMap::new();
    map.insert(
        "allow_subagents".to_string(),
        Json::Bool(spec.request.allow_subagents),
    );
    map.insert("context".to_string(), context_json(&spec.request.context));
    map.insert(
        "cwd".to_string(),
        Json::Str(canonical_path(&spec.request.cwd)),
    );
    map.insert(
        "harness".to_string(),
        Json::Str(spec.request.harness.clone()),
    );
    map.insert(
        "lane_plan".to_string(),
        lane_plan_json(spec.lane_plan.as_ref())?,
    );
    map.insert(
        "max_context_chars".to_string(),
        opt_usize(spec.request.max_context_chars)?,
    );
    map.insert("max_turns".to_string(), opt_str(&spec.request.max_turns));
    map.insert(
        "mode".to_string(),
        Json::Str(spec.mode.as_str().to_string()),
    );
    map.insert("model".to_string(), opt_str(&spec.request.model));
    map.insert(
        "permission_mode".to_string(),
        opt_str(&spec.request.permission_mode),
    );
    map.insert("prompt".to_string(), Json::Str(spec.request.prompt.clone()));
    map.insert("provider".to_string(), opt_str(&spec.request.provider));
    map.insert(
        "reasoning_effort".to_string(),
        opt_str(&spec.request.reasoning_effort),
    );
    map.insert("resume_id".to_string(), opt_str(&spec.request.resume_id));
    map.insert(
        "schema".to_string(),
        Json::Str("parley-job-request-v1".to_string()),
    );
    map.insert("session_id".to_string(), opt_str(&spec.request.session_id));
    map.insert("yolo".to_string(), Json::Bool(spec.request.yolo));
    Ok(Json::Object(map).to_compact_string())
}

fn context_json(context: &Option<ContextRef>) -> Json {
    let Some(context) = context else {
        return Json::Null;
    };
    let recovery = match context.recovery {
        Some(ContextRecovery::Replay) => Json::Str("replay".to_string()),
        Some(ContextRecovery::Skip) => Json::Str("skip".to_string()),
        None => Json::Null,
    };
    let mut map = BTreeMap::new();
    map.insert("harness".to_string(), Json::Str(context.harness.clone()));
    map.insert(
        "mode".to_string(),
        Json::Str(context.mode.as_str().to_string()),
    );
    map.insert(
        "mode_explicit".to_string(),
        Json::Bool(context.mode_explicit),
    );
    map.insert("recovery".to_string(), recovery);
    map.insert("session".to_string(), Json::Str(context.session.clone()));
    Json::Object(map)
}

fn lane_plan_json(plan: Option<&LanePlan>) -> Result<Json, JobError> {
    let Some(plan) = plan else {
        return Ok(Json::Null);
    };
    let base_commit = normalize_commit(&plan.base_commit)?;
    let integration_cwd = nonempty_path(&plan.integration_cwd, "integration_cwd")?;
    let mut lanes: Vec<&LaneGrant> = plan.lanes.iter().collect();
    lanes.sort_by(|left, right| left.lane_id.cmp(&right.lane_id));
    let mut seen_ids = HashSet::new();
    let mut lane_values = Vec::with_capacity(lanes.len());
    for lane in lanes {
        if !seen_ids.insert(lane.lane_id.as_str()) {
            return Err(JobError::invalid(format!(
                "invalid: duplicate lane id {}",
                lane.lane_id
            )));
        }
        validate_lane_id(&lane.lane_id)?;
        let cwd = nonempty_path(&lane.cwd, "lane cwd")?;
        let mut grants: Vec<&PathGrant> = lane.writable_paths.iter().collect();
        grants.sort_by(|left, right| {
            canonical_path(&left.path)
                .cmp(&canonical_path(&right.path))
                .then(left.kind.as_str().cmp(right.kind.as_str()))
        });
        let mut seen_grants = HashSet::new();
        let mut grant_values = Vec::with_capacity(grants.len());
        for grant in grants {
            let path = nonempty_path(&grant.path, "grant path")?;
            let kind = grant.kind.as_str();
            if !seen_grants.insert((path.clone(), kind)) {
                return Err(JobError::invalid(format!(
                    "invalid: duplicate grant {path} ({kind})"
                )));
            }
            let mut map = BTreeMap::new();
            map.insert("kind".to_string(), Json::Str(kind.to_string()));
            map.insert("path".to_string(), Json::Str(path));
            grant_values.push(Json::Object(map));
        }
        let mut map = BTreeMap::new();
        map.insert("cwd".to_string(), Json::Str(cwd));
        map.insert("lane_id".to_string(), Json::Str(lane.lane_id.clone()));
        map.insert(
            "owner".to_string(),
            Json::Str(lane.owner.as_str().to_string()),
        );
        map.insert(
            "role".to_string(),
            Json::Str(lane.role.as_str().to_string()),
        );
        map.insert("writable_paths".to_string(), Json::Array(grant_values));
        lane_values.push(Json::Object(map));
    }
    let mut map = BTreeMap::new();
    map.insert("base_commit".to_string(), Json::Str(base_commit));
    map.insert("integration_cwd".to_string(), Json::Str(integration_cwd));
    map.insert("lanes".to_string(), Json::Array(lane_values));
    Ok(Json::Object(map))
}

fn build_next(
    current: &JobRecord,
    transition: &JobTransition,
    now: u128,
) -> Result<JobRecord, JobError> {
    let exchange_id = merge_token(&current.exchange_id, &transition.exchange_id, "exchange_id")?;
    let session_id = merge_token(&current.session_id, &transition.session_id, "session_id")?;
    let request_event_id = merge_token(
        &current.request_event_id,
        &transition.request_event_id,
        "request_event_id",
    )?;
    let completion_event_id = merge_completion(current, transition)?;
    let error = merge_error(&current.error, &transition.error, transition.state)?;
    let same_observation = exchange_id == current.exchange_id
        && session_id == current.session_id
        && request_event_id == current.request_event_id
        && completion_event_id == current.completion_event_id
        && error == current.error;
    if transition.state == current.state {
        if same_observation {
            return Ok(current.clone());
        }
        return Err(JobError::conflict(
            "conflict: job observation does not match the stored record",
        ));
    }
    if !transition_allowed(current.state, transition.state) {
        return Err(JobError::illegal(format!(
            "illegal: job transition from {} to {}",
            current.state.as_str(),
            transition.state.as_str()
        )));
    }
    Ok(JobRecord {
        job_id: current.job_id.clone(),
        state: transition.state,
        mode: current.mode,
        request_fingerprint: current.request_fingerprint.clone(),
        exchange_id,
        session_id,
        request_event_id,
        completion_event_id,
        error,
        created_at_ms: current.created_at_ms,
        updated_at_ms: now,
        sequence: current.sequence + 1,
    })
}

fn transition_allowed(from: JobState, to: JobState) -> bool {
    use JobState::{
        Cancelled, Cancelling, Failed, Interrupted, Preparing, Running, Succeeded, TimedOut,
    };
    matches!(
        (from, to),
        (Preparing, Running)
            | (Preparing, Failed)
            | (Preparing, Cancelled)
            | (Preparing, Interrupted)
            | (Running, Cancelling)
            | (Running, Succeeded)
            | (Running, Failed)
            | (Running, TimedOut)
            | (Running, Cancelled)
            | (Running, Interrupted)
            | (Cancelling, Cancelled)
            | (Cancelling, Failed)
            | (Cancelling, TimedOut)
            | (Cancelling, Interrupted)
    )
}

fn merge_token(
    current: &Option<String>,
    incoming: &Option<String>,
    label: &str,
) -> Result<Option<String>, JobError> {
    match (current, incoming) {
        (_, None) => Ok(current.clone()),
        (None, Some(value)) => {
            validate_token(label, value)?;
            Ok(Some(value.clone()))
        }
        (Some(existing), Some(value)) if existing == value => Ok(Some(existing.clone())),
        (Some(_), Some(_)) => Err(JobError::conflict(format!(
            "conflict: {label} does not match the stored record"
        ))),
    }
}

fn merge_completion(
    current: &JobRecord,
    transition: &JobTransition,
) -> Result<Option<String>, JobError> {
    if let Some(value) = &transition.completion_event_id {
        validate_token("completion_event_id", value)?;
        if !transition.state.is_terminal() {
            return Err(JobError::invalid(
                "invalid: completion_event_id requires a terminal state",
            ));
        }
        if let Some(existing) = &current.completion_event_id {
            if existing != value {
                return Err(JobError::conflict(
                    "conflict: completion_event_id does not match the stored record",
                ));
            }
        }
        return Ok(Some(value.clone()));
    }
    if !transition.state.is_terminal() && current.completion_event_id.is_some() {
        return Err(JobError::corrupt(
            "corrupt: non-terminal job has completion_event_id",
        ));
    }
    Ok(current.completion_event_id.clone())
}

fn merge_error(
    current: &Option<String>,
    incoming: &Option<String>,
    next: JobState,
) -> Result<Option<String>, JobError> {
    if next == JobState::Succeeded {
        if incoming.is_some() || current.is_some() {
            return Err(JobError::invalid(
                "invalid: succeeded cannot carry an error",
            ));
        }
        return Ok(None);
    }
    if !error_allowed(next) {
        if incoming.is_some() || current.is_some() {
            return Err(JobError::invalid(format!(
                "invalid: {} cannot record an error",
                next.as_str()
            )));
        }
        return Ok(None);
    }
    match (current, incoming) {
        (Some(existing), Some(value)) if existing == value => {
            validate_error(value)?;
            Ok(Some(existing.clone()))
        }
        (Some(_), Some(_)) => Err(JobError::conflict(
            "conflict: error does not match the stored record",
        )),
        (Some(existing), None) => Ok(Some(existing.clone())),
        (None, Some(value)) => {
            validate_error(value)?;
            Ok(Some(value.clone()))
        }
        (None, None) => Ok(None),
    }
}

fn error_allowed(state: JobState) -> bool {
    matches!(
        state,
        JobState::Failed | JobState::TimedOut | JobState::Cancelled | JobState::Interrupted
    )
}

fn record_shape_ok(record: &JobRecord) -> Result<(), JobError> {
    if record.updated_at_ms < record.created_at_ms {
        return Err(JobError::corrupt(
            "corrupt: job updated_at_ms precedes created_at_ms",
        ));
    }
    if record.error.is_some() && !error_allowed(record.state) {
        return Err(JobError::corrupt(
            "corrupt: error is not valid for the recorded state",
        ));
    }
    if record.completion_event_id.is_some() && !record.state.is_terminal() {
        return Err(JobError::corrupt(
            "corrupt: completion_event_id on a non-terminal state",
        ));
    }
    Ok(())
}

fn validate_chain(records: &[JobRecord], job_id: &str) -> Result<(), JobError> {
    let Some(first) = records.first() else {
        return Err(JobError::corrupt("corrupt: journal has no records"));
    };
    if first.sequence != 1 || first.state != JobState::Preparing || first.job_id != job_id {
        return Err(JobError::corrupt(
            "corrupt: journal must start with preparing for this job",
        ));
    }
    record_shape_ok(first)?;
    for pair in records.windows(2) {
        let previous = &pair[0];
        let next = &pair[1];
        if next.sequence != previous.sequence + 1 {
            return Err(JobError::corrupt(
                "corrupt: journal sequence is not contiguous",
            ));
        }
        if next.job_id != previous.job_id
            || next.mode != previous.mode
            || next.request_fingerprint != previous.request_fingerprint
            || next.created_at_ms != previous.created_at_ms
        {
            return Err(JobError::corrupt(
                "corrupt: journal identity fields changed",
            ));
        }
        if next.updated_at_ms < previous.updated_at_ms {
            return Err(JobError::corrupt("corrupt: journal time moved backwards"));
        }
        if !transition_allowed(previous.state, next.state) {
            return Err(JobError::corrupt(
                "corrupt: journal contains an illegal transition",
            ));
        }
        if !token_advanced(&previous.exchange_id, &next.exchange_id)
            || !token_advanced(&previous.session_id, &next.session_id)
            || !token_advanced(&previous.request_event_id, &next.request_event_id)
            || !token_advanced(&previous.completion_event_id, &next.completion_event_id)
            || !token_advanced(&previous.error, &next.error)
        {
            return Err(JobError::corrupt(
                "corrupt: journal changed a stored identifier",
            ));
        }
        record_shape_ok(next)?;
    }
    Ok(())
}

fn token_advanced(previous: &Option<String>, next: &Option<String>) -> bool {
    match (previous, next) {
        (None, _) => true,
        (Some(left), Some(right)) => left == right,
        (Some(_), None) => false,
    }
}

fn load_job(dir: &Path, job_id: &str) -> Result<Option<JobRecord>, JobError> {
    inspect_job_dir(dir)?;
    let journal_path = dir.join("journal.jsonl");
    let snapshot_path = dir.join("snapshot.json");
    refuse_symlink(&journal_path)?;
    refuse_symlink(&snapshot_path)?;
    if !journal_path.exists() {
        if snapshot_path.exists() {
            return Err(JobError::corrupt("corrupt: snapshot without a journal"));
        }
        return Ok(None);
    }
    let bytes = fs::read(&journal_path).map_err(|error| {
        JobError::io(format!(
            "io: read journal {}: {error}",
            journal_path.display()
        ))
    })?;
    let lines = split_journal(&bytes)?;
    if lines.is_empty() {
        if snapshot_path.exists() {
            return Err(JobError::corrupt(
                "corrupt: snapshot without a complete journal record",
            ));
        }
        return Ok(None);
    }
    let mut records = Vec::with_capacity(lines.len());
    for line in &lines {
        records.push(parse_record_line(line)?);
    }
    validate_chain(&records, job_id)?;
    let Some(head) = records.last().cloned() else {
        return Err(JobError::corrupt("corrupt: journal has no head record"));
    };
    if !snapshot_matches(&snapshot_path, &head)? {
        publish_snapshot(dir, &head)?;
    }
    Ok(Some(head))
}

fn commit_record(dir: &Path, record: &JobRecord) -> Result<(), JobError> {
    append_record(&dir.join("journal.jsonl"), record)?;
    if let Err(error) = publish_snapshot(dir, record) {
        return Err(JobError::io(format!(
            "io: journal committed but snapshot publish failed: {error}"
        )));
    }
    Ok(())
}

fn append_record(path: &Path, record: &JobRecord) -> Result<(), JobError> {
    refuse_symlink(path)?;
    let line = record_json(record).to_compact_string();
    if line.len() > MAX_RECORD_BYTES || line.bytes().any(|byte| byte == b'\n' || byte == b'\r') {
        return Err(JobError::invalid(
            "invalid: job record is not a single bounded line",
        ));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|error| JobError::io(format!("io: open journal {}: {error}", path.display())))?;
    clip_incomplete(&mut file, path)?;
    let mut encoded = line.into_bytes();
    encoded.push(b'\n');
    file.write_all(&encoded)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|error| JobError::io(format!("io: append journal {}: {error}", path.display())))?;
    Ok(())
}

fn publish_snapshot(dir: &Path, record: &JobRecord) -> Result<(), JobError> {
    let destination = dir.join("snapshot.json");
    refuse_symlink(&destination)?;
    let temporary = dir.join(format!(
        ".snapshot.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    refuse_symlink(&temporary)?;
    let mut bytes = record_json(record).to_compact_string().into_bytes();
    bytes.push(b'\n');
    let published = (|| -> Result<(), JobError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| {
                JobError::io(format!(
                    "io: create snapshot temp {}: {error}",
                    temporary.display()
                ))
            })?;
        file.write_all(&bytes)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                JobError::io(format!(
                    "io: write snapshot temp {}: {error}",
                    temporary.display()
                ))
            })?;
        drop(file);
        fs::rename(&temporary, &destination).map_err(|error| {
            JobError::io(format!(
                "io: replace snapshot {} -> {}: {error}",
                temporary.display(),
                destination.display()
            ))
        })?;
        sync_dir(dir);
        Ok(())
    })();
    if published.is_err() {
        let _ = fs::remove_file(&temporary);
    } else {
        sweep_snapshot_temps(dir);
    }
    published
}

fn snapshot_matches(path: &Path, record: &JobRecord) -> Result<bool, JobError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(JobError::io(format!(
                "io: read snapshot {}: {error}",
                path.display()
            )))
        }
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(false);
    };
    let line = text.trim();
    if line.is_empty() {
        return Ok(false);
    }
    Ok(parse_record_line(line).ok().as_ref() == Some(record))
}

fn split_journal(bytes: &[u8]) -> Result<Vec<String>, JobError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let mut end = index;
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        if end == start {
            return Err(JobError::corrupt(
                "corrupt: journal contains an empty record",
            ));
        }
        if end - start > MAX_RECORD_BYTES {
            return Err(JobError::corrupt(
                "corrupt: journal record exceeds 65536 bytes",
            ));
        }
        let text = std::str::from_utf8(&bytes[start..end])
            .map_err(|_| JobError::corrupt("corrupt: journal record is not utf-8"))?;
        lines.push(text.to_string());
        start = index + 1;
    }
    Ok(lines)
}

fn clip_incomplete(file: &mut File, path: &Path) -> Result<(), JobError> {
    let len = file
        .metadata()
        .map_err(|error| JobError::io(format!("io: inspect journal {}: {error}", path.display())))?
        .len();
    if len == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| JobError::io(format!("io: seek journal {}: {error}", path.display())))?;
    let mut buffered = Vec::new();
    file.read_to_end(&mut buffered)
        .map_err(|error| JobError::io(format!("io: read journal {}: {error}", path.display())))?;
    let keep = match buffered.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => (index as u64) + 1,
        None => 0,
    };
    if keep < len {
        file.set_len(keep).map_err(|error| {
            JobError::io(format!("io: trim journal {}: {error}", path.display()))
        })?;
    }
    file.seek(SeekFrom::End(0))
        .map_err(|error| JobError::io(format!("io: seek journal {}: {error}", path.display())))?;
    Ok(())
}

fn parse_record_line(line: &str) -> Result<JobRecord, JobError> {
    ensure_single_json_value(line)?;
    let key_count = top_level_object_keys(line)?;
    let value = Json::parse(line).map_err(|error| {
        JobError::corrupt(format!("corrupt: journal record is not JSON: {error}"))
    })?;
    let Some(map) = value.as_object() else {
        return Err(JobError::corrupt(
            "corrupt: journal record must be an object",
        ));
    };
    if map.len() != key_count {
        return Err(JobError::corrupt(
            "corrupt: journal record contains duplicate fields",
        ));
    }
    if map.len() != RECORD_FIELDS.len() || RECORD_FIELDS.iter().any(|key| !map.contains_key(*key)) {
        return Err(JobError::corrupt(
            "corrupt: journal record fields do not match schema v1",
        ));
    }
    require_version(&value)?;
    if value.get("record_type").and_then(Json::as_str) != Some("transition") {
        return Err(JobError::corrupt("corrupt: unsupported job record type"));
    }
    let job_id = required_string(&value, "job_id")?;
    if !is_canonical_uuid(&job_id) {
        return Err(JobError::corrupt(
            "corrupt: journal job id is not a canonical UUID",
        ));
    }
    let state = JobState::parse(&required_string(&value, "state")?).map_err(JobError::corrupt)?;
    let mode = JobMode::parse(&required_string(&value, "mode")?).map_err(JobError::corrupt)?;
    let request_fingerprint = required_string(&value, "request_fingerprint")?;
    if !is_fingerprint(&request_fingerprint) {
        return Err(JobError::corrupt(
            "corrupt: journal fingerprint is not 16 lowercase hex characters",
        ));
    }
    let record = JobRecord {
        job_id,
        state,
        mode,
        request_fingerprint,
        exchange_id: optional_token(&value, "exchange_id")?,
        session_id: optional_token(&value, "session_id")?,
        request_event_id: optional_token(&value, "request_event_id")?,
        completion_event_id: optional_token(&value, "completion_event_id")?,
        error: optional_error(&value)?,
        created_at_ms: u128::from(required_u64(&value, "created_at_ms")?),
        updated_at_ms: u128::from(required_u64(&value, "updated_at_ms")?),
        sequence: required_u64(&value, "sequence")?,
    };
    if record.sequence == 0 {
        return Err(JobError::corrupt(
            "corrupt: journal sequence must start at 1",
        ));
    }
    record_shape_ok(&record)?;
    Ok(record)
}

fn record_json(record: &JobRecord) -> Json {
    let mut map = BTreeMap::new();
    map.insert(
        "completion_event_id".to_string(),
        json_opt(&record.completion_event_id),
    );
    map.insert(
        "created_at_ms".to_string(),
        Json::Number(record.created_at_ms as f64),
    );
    map.insert("error".to_string(), json_opt(&record.error));
    map.insert("exchange_id".to_string(), json_opt(&record.exchange_id));
    map.insert("job_id".to_string(), Json::Str(record.job_id.clone()));
    map.insert(
        "mode".to_string(),
        Json::Str(record.mode.as_str().to_string()),
    );
    map.insert(
        "record_type".to_string(),
        Json::Str("transition".to_string()),
    );
    map.insert(
        "request_event_id".to_string(),
        json_opt(&record.request_event_id),
    );
    map.insert(
        "request_fingerprint".to_string(),
        Json::Str(record.request_fingerprint.clone()),
    );
    map.insert(
        "schema_version".to_string(),
        Json::Number(JOB_SCHEMA_VERSION as f64),
    );
    map.insert("sequence".to_string(), Json::Number(record.sequence as f64));
    map.insert("session_id".to_string(), json_opt(&record.session_id));
    map.insert(
        "state".to_string(),
        Json::Str(record.state.as_str().to_string()),
    );
    map.insert(
        "updated_at_ms".to_string(),
        Json::Number(record.updated_at_ms as f64),
    );
    Json::Object(map)
}

fn job_ids(namespace: &NamespacePaths) -> Result<Vec<String>, JobError> {
    let Some(dir) = inspect_namespace(namespace)? else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for entry in fs::read_dir(&dir)
        .map_err(|error| JobError::io(format!("io: read {}: {error}", dir.display())))?
    {
        let entry = entry.map_err(|error| JobError::io(format!("io: read job entry: {error}")))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(JobError::corrupt(
                "corrupt: job directory name is not utf-8",
            ));
        };
        let file_type = entry
            .file_type()
            .map_err(|error| JobError::io(format!("io: inspect {name}: {error}")))?;
        if !file_type.is_dir() || !is_canonical_uuid(name) {
            return Err(JobError::corrupt(format!(
                "corrupt: unexpected job namespace entry {name}"
            )));
        }
        ids.push(name.to_string());
    }
    ids.sort();
    Ok(ids)
}

fn inspect_namespace(namespace: &NamespacePaths) -> Result<Option<PathBuf>, JobError> {
    refuse_reparse_chain(&namespace.root)?;
    match fs::symlink_metadata(&namespace.root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(JobError::io(format!(
                "io: inspect {}: {error}",
                namespace.root.display()
            )))
        }
        Ok(metadata) if !metadata.is_dir() || is_reparse(&metadata) => {
            return Err(JobError::invalid(format!(
                "invalid: job state root is not a real directory: {}",
                namespace.root.display()
            )))
        }
        Ok(_) => {}
    }
    let dir = namespace.directory();
    refuse_reparse_chain(&dir)?;
    match fs::symlink_metadata(&dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(JobError::io(format!(
            "io: inspect {}: {error}",
            dir.display()
        ))),
        Ok(metadata) if metadata.is_dir() && !is_reparse(&metadata) => Ok(Some(dir)),
        Ok(_) => Err(JobError::invalid(format!(
            "invalid: job namespace is not a real directory: {}",
            dir.display()
        ))),
    }
}

fn inspect_job_dir(dir: &Path) -> Result<(), JobError> {
    for entry in fs::read_dir(dir)
        .map_err(|error| JobError::io(format!("io: read {}: {error}", dir.display())))?
    {
        let entry = entry.map_err(|error| JobError::io(format!("io: read job file: {error}")))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(JobError::corrupt("corrupt: job file name is not utf-8"));
        };
        if name == "journal.jsonl"
            || name == "snapshot.json"
            || name == "lock"
            || is_snapshot_tmp(name)
        {
            continue;
        }
        return Err(JobError::corrupt(format!(
            "corrupt: unexpected job file {name}"
        )));
    }
    Ok(())
}

fn prepare_dir(path: &Path) -> Result<(), JobError> {
    refuse_reparse_chain(path)?;
    fs::create_dir_all(path)
        .map_err(|error| JobError::io(format!("io: create {}: {error}", path.display())))?;
    refuse_reparse_chain(path)?;
    if !path.is_dir() {
        return Err(JobError::invalid(format!(
            "invalid: job state path is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn refuse_reparse_chain(path: &Path) -> Result<(), JobError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                current.push(component);
                continue;
            }
            _ => current.push(component),
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_reparse(&metadata) => {
                return Err(JobError::invalid(format!(
                    "invalid: refusing job state path through a reparse point: {}",
                    current.display()
                )))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(JobError::io(format!(
                    "io: inspect {}: {error}",
                    current.display()
                )))
            }
        }
    }
    Ok(())
}

fn lock_job(dir: &Path) -> Result<LockGuard, JobError> {
    let path = dir.join("lock");
    refuse_symlink(&path)?;
    open_lock(&path)
}

struct LockGuard {
    _file: File,
}

fn open_lock(path: &Path) -> Result<LockGuard, JobError> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(path)
        {
            Ok(file) => Ok(LockGuard { _file: file }),
            Err(error) if is_sharing_violation(&error) => Err(JobError::locked(format!(
                "locked: job lock is held: {}",
                path.display()
            ))),
            Err(error) => Err(JobError::io(format!(
                "io: open job lock {}: {error}",
                path.display()
            ))),
        }
    }
    #[cfg(not(windows))]
    {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
        {
            Ok(file) => Ok(LockGuard { _file: file }),
            Err(error) => Err(JobError::io(format!(
                "io: open job lock {}: {error}",
                path.display()
            ))),
        }
    }
}

#[cfg(windows)]
fn is_sharing_violation(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32) | Some(33))
}

#[cfg(windows)]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn refuse_symlink(path: &Path) -> Result<(), JobError> {
    fsx::refuse_if_symlink(path).map_err(JobError::invalid)
}

fn sync_dir(dir: &Path) {
    if let Ok(file) = OpenOptions::new().read(true).open(dir) {
        let _ = file.sync_all();
    }
}

fn sweep_snapshot_temps(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_snapshot_tmp(name) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn is_snapshot_tmp(name: &str) -> bool {
    name.starts_with(".snapshot.") && name.ends_with(".tmp")
}

fn ensure_single_json_value(line: &str) -> Result<(), JobError> {
    let bytes = line.as_bytes();
    let mut index = skip_ascii_ws(bytes, 0);
    if index >= bytes.len() {
        return Err(JobError::corrupt("corrupt: journal record is empty"));
    }
    index = skip_json_value(bytes, index)?;
    index = skip_ascii_ws(bytes, index);
    if index != bytes.len() {
        return Err(JobError::corrupt(
            "corrupt: journal record has trailing data",
        ));
    }
    Ok(())
}

fn top_level_object_keys(line: &str) -> Result<usize, JobError> {
    let bytes = line.as_bytes();
    let mut index = skip_ascii_ws(bytes, 0);
    if bytes.get(index) != Some(&b'{') {
        return Err(JobError::corrupt(
            "corrupt: journal record must be an object",
        ));
    }
    index += 1;
    let mut count = 0usize;
    loop {
        index = skip_ascii_ws(bytes, index);
        if index >= bytes.len() {
            return Err(JobError::corrupt("corrupt: truncated journal record"));
        }
        if bytes[index] == b'}' {
            if count == 0 {
                return Ok(0);
            }
            return Err(JobError::corrupt(
                "corrupt: journal record has a trailing comma",
            ));
        }
        index = skip_string(bytes, index)?;
        count += 1;
        index = skip_ascii_ws(bytes, index);
        if bytes.get(index) != Some(&b':') {
            return Err(JobError::corrupt(
                "corrupt: journal record is missing a colon",
            ));
        }
        index += 1;
        index = skip_ascii_ws(bytes, index);
        index = skip_json_value(bytes, index)?;
        index = skip_ascii_ws(bytes, index);
        if index >= bytes.len() {
            return Err(JobError::corrupt("corrupt: truncated journal record"));
        }
        match bytes[index] {
            b',' => index += 1,
            b'}' => return Ok(count),
            _ => return Err(JobError::corrupt("corrupt: journal record is malformed")),
        }
    }
}

fn skip_json_value(bytes: &[u8], index: usize) -> Result<usize, JobError> {
    if index >= bytes.len() {
        return Err(JobError::corrupt("corrupt: truncated journal record"));
    }
    match bytes[index] {
        b'{' => skip_container(bytes, index, b'{', b'}'),
        b'[' => skip_container(bytes, index, b'[', b']'),
        b'"' => skip_string(bytes, index),
        b't' => skip_literal(bytes, index, b"true"),
        b'f' => skip_literal(bytes, index, b"false"),
        b'n' => skip_literal(bytes, index, b"null"),
        b'-' | b'0'..=b'9' => skip_number(bytes, index),
        _ => Err(JobError::corrupt(
            "corrupt: journal record is not a JSON value",
        )),
    }
}

fn skip_container(bytes: &[u8], start: usize, open: u8, close: u8) -> Result<usize, JobError> {
    let mut index = start + 1;
    loop {
        index = skip_ascii_ws(bytes, index);
        if index >= bytes.len() {
            return Err(JobError::corrupt("corrupt: truncated journal record"));
        }
        if bytes[index] == close {
            return Ok(index + 1);
        }
        if open == b'{' {
            index = skip_string(bytes, index)?;
            index = skip_ascii_ws(bytes, index);
            if bytes.get(index) != Some(&b':') {
                return Err(JobError::corrupt(
                    "corrupt: journal record is missing a colon",
                ));
            }
            index += 1;
            index = skip_ascii_ws(bytes, index);
        }
        index = skip_json_value(bytes, index)?;
        index = skip_ascii_ws(bytes, index);
        if index >= bytes.len() {
            return Err(JobError::corrupt("corrupt: truncated journal record"));
        }
        match bytes[index] {
            b',' => {
                index += 1;
                let next = skip_ascii_ws(bytes, index);
                if bytes.get(next) == Some(&close) {
                    return Err(JobError::corrupt(
                        "corrupt: journal record has a trailing comma",
                    ));
                }
            }
            byte if byte == close => return Ok(index + 1),
            _ => return Err(JobError::corrupt("corrupt: journal record is malformed")),
        }
    }
}

fn skip_string(bytes: &[u8], start: usize) -> Result<usize, JobError> {
    if bytes.get(start) != Some(&b'"') {
        return Err(JobError::corrupt(
            "corrupt: journal record expected a string",
        ));
    }
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Ok(index + 1),
            b'\\' => {
                index += 1;
                if index >= bytes.len() {
                    return Err(JobError::corrupt("corrupt: truncated journal string"));
                }
                if bytes[index] == b'u' {
                    if index + 5 > bytes.len()
                        || !bytes[index + 1..index + 5]
                            .iter()
                            .all(|byte| byte.is_ascii_hexdigit())
                    {
                        return Err(JobError::corrupt(
                            "corrupt: invalid unicode escape in journal string",
                        ));
                    }
                    index += 5;
                } else {
                    index += 1;
                }
            }
            byte if byte < 0x20 => {
                return Err(JobError::corrupt(
                    "corrupt: raw control character in journal string",
                ))
            }
            _ => index += 1,
        }
    }
    Err(JobError::corrupt("corrupt: truncated journal string"))
}

fn skip_number(bytes: &[u8], start: usize) -> Result<usize, JobError> {
    let mut index = start;
    if bytes[index] == b'-' {
        index += 1;
    }
    let digits = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == digits {
        return Err(JobError::corrupt("corrupt: invalid journal number"));
    }
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        let fraction = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index == fraction {
            return Err(JobError::corrupt("corrupt: invalid journal number"));
        }
    }
    if index < bytes.len() && (bytes[index] == b'e' || bytes[index] == b'E') {
        index += 1;
        if index < bytes.len() && (bytes[index] == b'+' || bytes[index] == b'-') {
            index += 1;
        }
        let exponent = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index == exponent {
            return Err(JobError::corrupt("corrupt: invalid journal number"));
        }
    }
    Ok(index)
}

fn skip_literal(bytes: &[u8], start: usize, literal: &[u8]) -> Result<usize, JobError> {
    if bytes[start..].starts_with(literal) {
        Ok(start + literal.len())
    } else {
        Err(JobError::corrupt("corrupt: invalid journal literal"))
    }
}

fn skip_ascii_ws(bytes: &[u8], mut index: usize) -> usize {
    while matches!(bytes.get(index), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        index += 1;
    }
    index
}

fn required_string(object: &Json, key: &str) -> Result<String, JobError> {
    object
        .get(key)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| JobError::corrupt(format!("corrupt: missing string field {key}")))
}

fn optional_string(object: &Json, key: &str) -> Result<Option<String>, JobError> {
    match object.get(key) {
        Some(Json::Null) => Ok(None),
        Some(Json::Str(value)) => Ok(Some(value.clone())),
        _ => Err(JobError::corrupt(format!(
            "corrupt: field {key} must be a string or null"
        ))),
    }
}

fn optional_token(object: &Json, key: &str) -> Result<Option<String>, JobError> {
    match optional_string(object, key)? {
        Some(value) => {
            validate_token(key, &value).map_err(|error| JobError::corrupt(error.message))?;
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

fn optional_error(object: &Json) -> Result<Option<String>, JobError> {
    match optional_string(object, "error")? {
        Some(value) => {
            validate_error(&value).map_err(|error| JobError::corrupt(error.message))?;
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

fn required_u64(object: &Json, key: &str) -> Result<u64, JobError> {
    match object.get(key) {
        Some(Json::Number(value)) => json_u64(*value)
            .ok_or_else(|| JobError::corrupt(format!("corrupt: field {key} is not an integer"))),
        _ => Err(JobError::corrupt(format!(
            "corrupt: missing integer field {key}"
        ))),
    }
}

fn json_u64(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    let converted = value as u64;
    (converted as f64 == value).then_some(converted)
}

fn require_version(object: &Json) -> Result<(), JobError> {
    match object.get("schema_version") {
        Some(Json::Number(value)) if *value == 1.0 => Ok(()),
        _ => Err(JobError::corrupt("corrupt: unsupported job schema")),
    }
}

fn json_opt(value: &Option<String>) -> Json {
    match value {
        Some(text) => Json::Str(text.clone()),
        None => Json::Null,
    }
}

fn opt_str(value: &Option<String>) -> Json {
    json_opt(value)
}

fn opt_usize(value: Option<usize>) -> Result<Json, JobError> {
    let Some(value) = value else {
        return Ok(Json::Null);
    };
    let value = u64::try_from(value)
        .map_err(|_| JobError::invalid("invalid: max_context_chars is too large"))?;
    let number = value as f64;
    if number as u64 != value {
        return Err(JobError::invalid(
            "invalid: max_context_chars is not an exact integer",
        ));
    }
    Ok(Json::Number(number))
}

fn initial_session(spec: &JobSpec) -> Result<Option<String>, JobError> {
    match spec
        .request
        .session_id
        .as_deref()
        .or(spec.request.resume_id.as_deref())
    {
        None => Ok(None),
        Some(value) => {
            validate_token("session_id", value)?;
            Ok(Some(value.to_string()))
        }
    }
}

fn validate_token(label: &str, value: &str) -> Result<(), JobError> {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
    {
        Ok(())
    } else {
        Err(JobError::invalid(format!(
            "invalid: {label} must be 1-128 ASCII letters, digits, or . _ - :"
        )))
    }
}

fn validate_error(value: &str) -> Result<(), JobError> {
    if value.is_empty() || value.chars().count() > MAX_ERROR_CHARS {
        return Err(JobError::invalid(format!(
            "invalid: error must be 1-{MAX_ERROR_CHARS} characters"
        )));
    }
    if value.chars().any(|ch| ch.is_control() || !ch.is_ascii()) {
        return Err(JobError::invalid(
            "invalid: error must be ASCII without control characters",
        ));
    }
    Ok(())
}

fn validate_lane_id(value: &str) -> Result<(), JobError> {
    if !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('.')
        && !value.contains("..")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        Ok(())
    } else {
        Err(JobError::invalid(format!("invalid: lane id {value}")))
    }
}

fn normalize_commit(value: &str) -> Result<String, JobError> {
    let lower = value.to_ascii_lowercase();
    if lower.len() == 40 && lower.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(lower)
    } else {
        Err(JobError::invalid(
            "invalid: lane plan base_commit must be a 40-character hexadecimal git commit",
        ))
    }
}

fn nonempty_path(path: &Path, label: &str) -> Result<String, JobError> {
    let text = canonical_path(path);
    if text.is_empty() {
        Err(JobError::invalid(format!(
            "invalid: {label} must not be empty"
        )))
    } else {
        Ok(text)
    }
}

fn canonical_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalize_caller(caller: Option<String>) -> Result<String, JobError> {
    let Some(caller) = caller else {
        return Ok("parley".to_string());
    };
    if caller.is_empty()
        || caller.len() > 64
        || caller.starts_with('.')
        || caller.contains("..")
        || !caller
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(JobError::invalid(
            "invalid: PARLEY_CALLER must be one path segment of letters, digits, '.', '_' or '-'",
        ));
    }
    Ok(caller)
}

fn normalize_uuid(value: &str) -> Result<String, JobError> {
    let lower = value.to_ascii_lowercase();
    if is_uuid(&lower) {
        Ok(lower)
    } else {
        Err(JobError::invalid(format!(
            "invalid: job id must be a UUID, got {value}"
        )))
    }
}

fn is_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    value.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_hexdigit(),
    })
}

fn is_canonical_uuid(value: &str) -> bool {
    is_uuid(value) && value.bytes().all(|byte| !byte.is_ascii_uppercase())
}

fn is_fingerprint(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(windows)]
fn default_root(localappdata: &Option<PathBuf>) -> Result<PathBuf, JobError> {
    let Some(local) = localappdata
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    else {
        return Err(JobError::invalid(
            "invalid: cannot resolve job state dir (PARLEY_JOB_STATE_DIR or LOCALAPPDATA)",
        ));
    };
    Ok(local.join("Parley").join("jobs"))
}

#[cfg(not(windows))]
fn default_root(_localappdata: &Option<PathBuf>) -> Result<PathBuf, JobError> {
    Err(JobError::invalid(
        "invalid: default job state dir is Windows-only; set PARLEY_JOB_STATE_DIR",
    ))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).and_then(|value| {
        if value.is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    })
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn system_now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::ask::{AskRequest, ContextRef};
    use crate::context::{ContextMode, ContextRecovery};
    use crate::jobs::{
        GrantKind, JobMode, JobSpec, JobState, LaneGrant, LaneOwner, LanePlan, LaneRole, PathGrant,
    };

    const JOB_A: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const JOB_B: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    const JOB_C: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";
    const SESSION: &str = "b4ade00b-915b-4cd9-be37-73313f2ce59a";
    const PROMPT: &str = "PROMPT_TEXT_SHOULD_NOT_PERSIST";
    const CWD: &str = r"D:\CWD_SHOULD_NOT_PERSIST";
    const LANE_PATH: &str = r"D:\LANE_PATH_SHOULD_NOT_PERSIST";
    const CONTEXT_SESSION: &str = "CONTEXT_SESSION_SHOULD_NOT_PERSIST";
    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    struct Fixture {
        root: PathBuf,
        store: JobStore,
    }

    impl Fixture {
        fn new(caller: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("parley-job-journal-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let store = JobStore::open_with_clock(
                root.clone(),
                caller,
                Clock::Manual(Arc::new(AtomicU64::new(1_000_000))),
            )
            .unwrap();
            Self { root, store }
        }

        fn journal(&self, job_id: &str) -> PathBuf {
            self.store
                .namespace_dir()
                .join(job_id.to_ascii_lowercase())
                .join("journal.jsonl")
        }

        fn snapshot(&self, job_id: &str) -> PathBuf {
            self.store
                .namespace_dir()
                .join(job_id.to_ascii_lowercase())
                .join("snapshot.json")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn request(prompt: &str) -> AskRequest {
        AskRequest {
            harness: "grok".to_string(),
            prompt: prompt.to_string(),
            model: Some("grok-4.7".to_string()),
            reasoning_effort: Some("xhigh".to_string()),
            provider: None,
            cwd: PathBuf::from(CWD),
            permission_mode: Some("auto".to_string()),
            max_turns: Some("30".to_string()),
            session_id: Some(SESSION.to_string()),
            resume_id: None,
            yolo: false,
            context: None,
            max_context_chars: None,
            allow_subagents: false,
        }
    }

    fn spec(job_id: &str, prompt: &str) -> JobSpec {
        JobSpec {
            job_id: job_id.to_string(),
            mode: JobMode::Write,
            request: request(prompt),
            lane_plan: None,
        }
    }

    fn lane(id: &str, path: &str) -> LaneGrant {
        LaneGrant {
            lane_id: id.to_string(),
            owner: LaneOwner::GrokParent,
            role: LaneRole::Writer,
            cwd: PathBuf::from(path),
            writable_paths: vec![
                PathGrant {
                    path: PathBuf::from(format!("{path}\\src")),
                    kind: GrantKind::Tree,
                },
                PathGrant {
                    path: PathBuf::from(format!("{path}\\README.md")),
                    kind: GrantKind::File,
                },
            ],
        }
    }

    fn rich_spec(prompt: &str) -> JobSpec {
        let mut job = spec(JOB_A, prompt);
        job.request.context = Some(ContextRef {
            harness: "codex".to_string(),
            session: CONTEXT_SESSION.to_string(),
            mode: ContextMode::Auto,
            mode_explicit: true,
            recovery: Some(ContextRecovery::Replay),
        });
        job.lane_plan = Some(LanePlan {
            base_commit: COMMIT.to_string(),
            integration_cwd: PathBuf::from(LANE_PATH),
            lanes: vec![lane("grok-parent", LANE_PATH), lane("codex", LANE_PATH)],
        });
        job
    }

    fn transition(
        state: JobState,
        exchange_id: Option<&str>,
        request_event_id: Option<&str>,
        completion_event_id: Option<&str>,
        error: Option<&str>,
    ) -> JobTransition {
        JobTransition {
            state,
            exchange_id: exchange_id.map(str::to_string),
            session_id: None,
            request_event_id: request_event_id.map(str::to_string),
            completion_event_id: completion_event_id.map(str::to_string),
            error: error.map(str::to_string),
        }
    }

    fn tree_text(dir: &Path) -> String {
        let mut out = String::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.push_str(&tree_text(&path));
            } else if let Ok(bytes) = fs::read(&path) {
                out.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        out
    }

    fn complete_lines(path: &Path) -> Vec<String> {
        let text = fs::read_to_string(path).unwrap();
        assert!(
            text.ends_with('\n'),
            "durable journal must end with a newline"
        );
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn resolves_caller_namespace_without_creating_it() {
        let root = std::env::temp_dir().join(format!(
            "parley-job-resolve-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let env = JobStateEnv {
            job_state_dir: Some(root.clone()),
            caller: Some("codex".to_string()),
            localappdata: None,
        };
        let namespace = resolve_namespace(&env).unwrap();
        assert_eq!(namespace.directory(), root.join("codex"));
        assert!(!root.exists());
        let store = JobStore::open(&env).unwrap();
        assert!(store.list(1).unwrap().is_empty());
        assert!(!root.exists());

        for caller in ["", ".codex", "..", r"..\evil", "a/b", "a..b"] {
            let env = JobStateEnv {
                job_state_dir: Some(root.clone()),
                caller: Some(caller.to_string()),
                localappdata: None,
            };
            assert!(resolve_namespace(&env).is_err(), "{caller}");
        }

        #[cfg(windows)]
        {
            let env = JobStateEnv {
                job_state_dir: None,
                caller: None,
                localappdata: Some(PathBuf::from(r"C:\Users\mrdyl\AppData\Local")),
            };
            let namespace = resolve_namespace(&env).unwrap();
            assert_eq!(namespace.caller, "parley");
            assert_eq!(
                namespace.root,
                PathBuf::from(r"C:\Users\mrdyl\AppData\Local\Parley\jobs")
            );
            assert!(resolve_namespace(&JobStateEnv::default()).is_err());
        }
    }

    #[test]
    fn fingerprints_are_canonical_and_prompts_are_not_stored() {
        let mut forward = rich_spec(PROMPT);
        let reverse = {
            let mut job = rich_spec(PROMPT);
            let plan = job.lane_plan.as_mut().unwrap();
            plan.lanes.reverse();
            for lane in &mut plan.lanes {
                lane.writable_paths.reverse();
            }
            job
        };
        let forward_hash = request_fingerprint(&forward).unwrap();
        assert_eq!(forward_hash, request_fingerprint(&reverse).unwrap());
        assert_eq!(forward_hash.len(), 16);
        forward.request.prompt = "different prompt".to_string();
        assert_ne!(forward_hash, request_fingerprint(&forward).unwrap());
        forward.request.prompt = PROMPT.to_string();
        forward.request.yolo = true;
        assert_ne!(forward_hash, request_fingerprint(&forward).unwrap());
        forward.request.yolo = false;
        forward.request.context.as_mut().unwrap().recovery = Some(ContextRecovery::Skip);
        assert_ne!(forward_hash, request_fingerprint(&forward).unwrap());

        let fixture = Fixture::new("codex");
        let created = fixture.store.begin(&rich_spec(PROMPT)).unwrap();
        assert!(created.created());
        assert_eq!(created.view().reply, None);
        let stored = tree_text(&fixture.store.namespace_dir());
        assert!(stored.contains(&forward_hash));
        for secret in [PROMPT, CWD, LANE_PATH, CONTEXT_SESSION, COMMIT, "reply"] {
            assert!(!stored.contains(secret), "{secret} leaked into the journal");
        }
        assert!(!stored.contains("\"prompt\""));
    }

    #[test]
    fn begin_is_idempotent_and_rejects_a_different_fingerprint() {
        let fixture = Fixture::new("codex");
        let created = fixture.store.begin(&spec(JOB_A, "alpha")).unwrap();
        assert!(created.created());
        assert_eq!(created.view().state, JobState::Preparing);
        assert_eq!(created.view().session_id.as_deref(), Some(SESSION));
        let before = fs::read(fixture.journal(JOB_A)).unwrap();
        let snapshot = fs::read(fixture.snapshot(JOB_A)).unwrap();
        let again = fixture.store.begin(&spec(JOB_A, "alpha")).unwrap();
        assert!(!again.created());
        assert_eq!(again.view().updated_at_ms, created.view().updated_at_ms);
        assert_eq!(fs::read(fixture.journal(JOB_A)).unwrap(), before);

        let conflict = fixture.store.begin(&spec(JOB_A, "beta")).unwrap_err();
        assert_eq!(conflict.kind, JobErrorKind::Conflict);
        assert_eq!(fs::read(fixture.journal(JOB_A)).unwrap(), before);
        assert_eq!(fs::read(fixture.snapshot(JOB_A)).unwrap(), snapshot);
        assert!(!conflict.message.contains("beta"));
    }

    #[test]
    fn transitions_are_durable_and_illegal_edges_do_not_append() {
        let fixture = Fixture::new("codex");
        fixture.store.begin(&spec(JOB_A, "alpha")).unwrap();
        let running = fixture
            .store
            .transition(
                JOB_A,
                &transition(
                    JobState::Running,
                    Some("exchange-1"),
                    Some("event-request"),
                    None,
                    None,
                ),
            )
            .unwrap();
        assert_eq!(running.state, JobState::Running);
        assert_eq!(running.exchange_id.as_deref(), Some("exchange-1"));
        assert_eq!(running.session_id.as_deref(), Some(SESSION));
        assert!(running.reply.is_none());
        let lines = complete_lines(&fixture.journal(JOB_A));
        assert_eq!(lines.len(), 2);
        assert!(fixture.snapshot(JOB_A).is_file());
        assert!(temps(&fixture, JOB_A).is_empty());

        let replay = fixture
            .store
            .transition(
                JOB_A,
                &transition(
                    JobState::Running,
                    Some("exchange-1"),
                    Some("event-request"),
                    None,
                    None,
                ),
            )
            .unwrap();
        assert_eq!(replay.updated_at_ms, running.updated_at_ms);
        assert_eq!(complete_lines(&fixture.journal(JOB_A)).len(), 2);

        let conflict = fixture
            .store
            .transition(
                JOB_A,
                &transition(JobState::Running, Some("exchange-2"), None, None, None),
            )
            .unwrap_err();
        assert_eq!(conflict.kind, JobErrorKind::Conflict);
        assert_eq!(complete_lines(&fixture.journal(JOB_A)).len(), 2);

        let illegal = fixture
            .store
            .transition(
                JOB_A,
                &transition(JobState::Succeeded, None, None, None, None),
            )
            .unwrap();
        assert_eq!(illegal.state, JobState::Succeeded);
        assert_eq!(illegal.completion_event_id.as_deref(), None);
        assert_eq!(illegal.request_event_id.as_deref(), Some("event-request"));
        assert_eq!(illegal.exchange_id.as_deref(), Some("exchange-1"));
        assert!(illegal.error.is_none());
        assert!(illegal.reply.is_none());

        fixture.store.begin(&spec(JOB_B, "beta")).unwrap();
        let blocked = fixture
            .store
            .transition(
                JOB_B,
                &transition(JobState::Succeeded, None, None, None, None),
            )
            .unwrap_err();
        assert_eq!(blocked.kind, JobErrorKind::IllegalTransition);
        assert_eq!(complete_lines(&fixture.journal(JOB_B)).len(), 1);
        let preserved = fs::read(fixture.snapshot(JOB_A)).unwrap();
        assert_eq!(fs::read(fixture.snapshot(JOB_A)).unwrap(), preserved);
    }

    #[test]
    fn documented_transitions_are_closed() {
        use JobState::*;
        let states = [
            Preparing,
            Running,
            Cancelling,
            Succeeded,
            Failed,
            TimedOut,
            Cancelled,
            Interrupted,
        ];
        let allowed = [
            (Preparing, Running),
            (Preparing, Failed),
            (Preparing, Cancelled),
            (Preparing, Interrupted),
            (Running, Cancelling),
            (Running, Succeeded),
            (Running, Failed),
            (Running, TimedOut),
            (Running, Cancelled),
            (Running, Interrupted),
            (Cancelling, Cancelled),
            (Cancelling, Failed),
            (Cancelling, TimedOut),
            (Cancelling, Interrupted),
        ];
        for from in states {
            for to in states {
                assert_eq!(
                    transition_allowed(from, to),
                    allowed.contains(&(from, to)),
                    "{from:?} -> {to:?}"
                );
            }
        }
    }

    #[test]
    fn partial_trailing_line_is_recovered_and_clipped() {
        let fixture = Fixture::new("codex");
        fixture.store.begin(&spec(JOB_A, "alpha")).unwrap();
        let mut bytes = fs::read(fixture.journal(JOB_A)).unwrap();
        bytes.extend_from_slice(b"UNFINISHED_TAIL");
        fs::write(fixture.journal(JOB_A), &bytes).unwrap();
        let view = fixture.store.get(JOB_A).unwrap().unwrap();
        assert_eq!(view.state, JobState::Preparing);
        assert!(fs::read(fixture.journal(JOB_A))
            .unwrap()
            .ends_with(b"UNFINISHED_TAIL"));

        let crlf = fs::read_to_string(fixture.journal(JOB_A))
            .unwrap()
            .trim_end_matches("UNFINISHED_TAIL")
            .replace('\n', "\r\n");
        fs::write(fixture.journal(JOB_A), crlf.as_bytes()).unwrap();
        assert_eq!(
            fixture.store.get(JOB_A).unwrap().unwrap().state,
            JobState::Preparing
        );

        bytes = fs::read(fixture.journal(JOB_A)).unwrap();
        bytes.extend_from_slice(b"UNFINISHED_TAIL");
        fs::write(fixture.journal(JOB_A), &bytes).unwrap();
        fixture
            .store
            .transition(
                JOB_A,
                &transition(JobState::Running, Some("exchange-1"), None, None, None),
            )
            .unwrap();
        let text = fs::read_to_string(fixture.journal(JOB_A)).unwrap();
        assert!(text.ends_with('\n'));
        assert!(!text.contains("UNFINISHED_TAIL"));
        assert_eq!(text.lines().count(), 2);
        assert_eq!(
            fixture.store.get(JOB_A).unwrap().unwrap().state,
            JobState::Running
        );
    }

    #[test]
    fn malformed_complete_record_fails_closed_without_rewriting() {
        let fixture = Fixture::new("codex");
        fixture.store.begin(&spec(JOB_A, "alpha")).unwrap();
        fixture.store.begin(&spec(JOB_B, "beta")).unwrap();
        let journal = fixture.journal(JOB_B);
        let snapshot = fs::read(fixture.snapshot(JOB_B)).unwrap();
        let mut corrupt = fs::read(&journal).unwrap();
        corrupt.extend_from_slice(b"{\"schema_version\":2,\"record_type\":\"transition\"}\n");
        fs::write(&journal, &corrupt).unwrap();

        let failure = fixture.store.get(JOB_B).unwrap_err();
        assert_eq!(failure.kind, JobErrorKind::Corrupt);
        assert_eq!(fs::read(&journal).unwrap(), corrupt);
        assert_eq!(fs::read(fixture.snapshot(JOB_B)).unwrap(), snapshot);
        assert_eq!(
            fixture.store.get(JOB_A).unwrap().unwrap().state,
            JobState::Preparing
        );
        assert_eq!(
            fixture.store.list(10).unwrap_err().kind,
            JobErrorKind::Corrupt
        );
        assert_eq!(
            fixture.store.recover_startup().unwrap_err().kind,
            JobErrorKind::Corrupt
        );
        assert_eq!(fs::read(&journal).unwrap(), corrupt);
        assert_eq!(
            fixture.store.get(JOB_A).unwrap().unwrap().state,
            JobState::Preparing
        );

        fs::write(
            fixture.store.namespace_dir().join(JOB_B).join("note.txt"),
            b"x",
        )
        .unwrap();
        assert_eq!(
            fixture.store.get(JOB_B).unwrap_err().kind,
            JobErrorKind::Corrupt
        );
    }

    #[test]
    fn stale_snapshot_is_repaired_from_the_journal() {
        let fixture = Fixture::new("codex");
        fixture.store.begin(&spec(JOB_A, "alpha")).unwrap();
        fixture
            .store
            .transition(
                JOB_A,
                &transition(JobState::Running, Some("exchange-1"), None, None, None),
            )
            .unwrap();
        fs::write(fixture.snapshot(JOB_A), b"{\"schema_version\":1}\n").unwrap();
        let view = fixture.store.get(JOB_A).unwrap().unwrap();
        assert_eq!(view.state, JobState::Running);
        let repaired = fs::read_to_string(fixture.snapshot(JOB_A)).unwrap();
        assert!(repaired.contains("\"state\":\"running\""));
        assert!(!repaired.contains(PROMPT));
        let stable = fs::read(fixture.snapshot(JOB_A)).unwrap();
        let _ = fixture.store.get(JOB_A).unwrap();
        assert_eq!(fs::read(fixture.snapshot(JOB_A)).unwrap(), stable);
    }

    #[test]
    fn startup_recovery_interrupts_only_unfinished_states() {
        let fixture = Fixture::new("codex");
        fixture.store.begin(&spec(JOB_A, "prepare")).unwrap();
        fixture.store.begin(&spec(JOB_B, "run")).unwrap();
        fixture
            .store
            .transition(
                JOB_B,
                &transition(JobState::Running, Some("exchange-b"), None, None, None),
            )
            .unwrap();
        fixture.store.begin(&spec(JOB_C, "cancel")).unwrap();
        fixture
            .store
            .transition(
                JOB_C,
                &transition(JobState::Running, Some("exchange-c"), None, None, None),
            )
            .unwrap();
        fixture
            .store
            .transition(
                JOB_C,
                &transition(JobState::Cancelling, None, None, None, None),
            )
            .unwrap();

        let succeeded_id = "dddddddd-dddd-dddd-dddd-dddddddddddd";
        let failed_id = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
        let interrupted_id = "ffffffff-ffff-ffff-ffff-ffffffffffff";
        fixture.store.begin(&spec(succeeded_id, "done")).unwrap();
        fixture
            .store
            .transition(
                succeeded_id,
                &transition(JobState::Running, Some("exchange-d"), None, None, None),
            )
            .unwrap();
        fixture
            .store
            .transition(
                succeeded_id,
                &transition(JobState::Succeeded, None, None, Some("event-done"), None),
            )
            .unwrap();
        fixture.store.begin(&spec(failed_id, "fail")).unwrap();
        fixture
            .store
            .transition(
                failed_id,
                &transition(JobState::Failed, None, None, None, Some("preflight_failed")),
            )
            .unwrap();
        fixture.store.begin(&spec(interrupted_id, "stop")).unwrap();
        fixture
            .store
            .transition(
                interrupted_id,
                &transition(
                    JobState::Interrupted,
                    None,
                    None,
                    None,
                    Some("operator_stop"),
                ),
            )
            .unwrap();

        let succeeded_at = fixture
            .store
            .get(succeeded_id)
            .unwrap()
            .unwrap()
            .updated_at_ms;
        let failed_before = complete_lines(&fixture.journal(failed_id)).len();
        let interrupted_before = complete_lines(&fixture.journal(interrupted_id)).len();
        let changed = fixture.store.recover_startup().unwrap();
        assert_eq!(changed.len(), 3);
        for job_id in [JOB_A, JOB_B, JOB_C] {
            let view = fixture.store.get(job_id).unwrap().unwrap();
            assert_eq!(view.state, JobState::Interrupted);
            assert_eq!(view.error.as_deref(), Some(STARTUP_RECOVERY_ERROR));
            assert!(view.reply.is_none());
        }
        let succeeded = fixture.store.get(succeeded_id).unwrap().unwrap();
        assert_eq!(succeeded.state, JobState::Succeeded);
        assert_eq!(succeeded.updated_at_ms, succeeded_at);
        assert_eq!(succeeded.completion_event_id.as_deref(), Some("event-done"));
        let failed = fixture.store.get(failed_id).unwrap().unwrap();
        assert_eq!(failed.state, JobState::Failed);
        assert_eq!(failed.error.as_deref(), Some("preflight_failed"));
        assert_eq!(
            complete_lines(&fixture.journal(failed_id)).len(),
            failed_before
        );
        assert_eq!(
            complete_lines(&fixture.journal(interrupted_id)).len(),
            interrupted_before
        );

        let preparing_lines = complete_lines(&fixture.journal(JOB_A)).len();
        assert!(fixture.store.recover_startup().unwrap().is_empty());
        assert_eq!(
            complete_lines(&fixture.journal(JOB_A)).len(),
            preparing_lines
        );
    }

    #[test]
    fn list_is_newest_first_and_bounded() {
        let fixture = Fixture::new("codex");
        fixture.store.begin(&spec(JOB_A, "a")).unwrap();
        fixture.store.begin(&spec(JOB_B, "b")).unwrap();
        fixture.store.begin(&spec(JOB_C, "c")).unwrap();
        let listed = fixture.store.list(2).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|view| view.job_id.as_str())
                .collect::<Vec<_>>(),
            vec![JOB_C, JOB_B]
        );
        fixture
            .store
            .transition(
                JOB_A,
                &transition(JobState::Running, Some("exchange-a"), None, None, None),
            )
            .unwrap();
        let listed = fixture.store.list(2).unwrap();
        assert_eq!(listed[0].job_id, JOB_A);
        assert_eq!(listed[1].job_id, JOB_C);
        assert!(fixture.store.list(0).unwrap().is_empty());
        assert_eq!(
            fixture.store.list(MAX_LISTED_JOBS + 1).unwrap_err().kind,
            JobErrorKind::Invalid
        );
        assert_eq!(fixture.store.list(MAX_LISTED_JOBS).unwrap().len(), 3);
    }

    #[test]
    fn callers_are_isolated_and_uuids_are_canonical() {
        let fixture = Fixture::new("codex");
        let other = JobStore::open_at(&fixture.root, "grok-parent").unwrap();
        fixture.store.begin(&spec(JOB_A, "alpha")).unwrap();
        assert!(other.list(10).unwrap().is_empty());
        assert!(other.get(JOB_A).unwrap().is_none());

        let uppercase = "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE";
        fixture.store.begin(&spec(uppercase, "case")).unwrap();
        let canonical = uppercase.to_ascii_lowercase();
        assert!(fixture
            .store
            .namespace_dir()
            .join(&canonical)
            .join("journal.jsonl")
            .is_file());
        assert_eq!(
            fixture.store.get(&canonical).unwrap().unwrap().job_id,
            canonical
        );
        assert_eq!(
            fixture
                .store
                .begin(&spec(uppercase, "case"))
                .unwrap()
                .view()
                .job_id,
            canonical
        );
        assert_eq!(
            fixture
                .store
                .begin(&spec("not-a-uuid", "x"))
                .unwrap_err()
                .kind,
            JobErrorKind::Invalid
        );
        assert_eq!(
            fixture.store.get("not-a-uuid").unwrap_err().kind,
            JobErrorKind::Invalid
        );
        assert_eq!(
            fixture
                .store
                .transition(
                    "ffffffff-ffff-ffff-ffff-ffffffffffff",
                    &transition(JobState::Running, None, None, None, None)
                )
                .unwrap_err()
                .kind,
            JobErrorKind::NotFound
        );
    }

    fn temps(fixture: &Fixture, job_id: &str) -> Vec<String> {
        fs::read_dir(fixture.store.namespace_dir().join(job_id))
            .unwrap()
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().to_string_lossy().into_owned();
                name.contains(".tmp").then_some(name)
            })
            .collect()
    }
}
