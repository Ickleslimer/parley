use std::env;
use std::path::{Path, PathBuf};

use crate::command::HelperKind;
use crate::fsutil::{self, FailKind};
use crate::schema::{StoredRecord, DEFAULT_WAIT_SLICE_SECS, MAX_WAIT_SLICE_SECS};
use crate::sha256::sha256_hex;

#[derive(Clone, Debug)]
pub struct Binding {
    pub state_dir: PathBuf,
    pub job_dir: PathBuf,
    pub job_id: String,
    pub handoff_id: String,
    pub capability: String,
    pub source_session_id: String,
    pub target_session_id: String,
    pub wait_slice_secs: u64,
    pub peek_exe: PathBuf,
    pub wait_exe: PathBuf,
    pub alert_exe: PathBuf,
    pub hook_exe: PathBuf,
    pub health_inbox: Option<PathBuf>,
    pub codex_rollout: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct RuntimeFacts {
    pub pid: u32,
    pub ancestors: Vec<u32>,
    pub exe: PathBuf,
    pub cwd: PathBuf,
}

impl RuntimeFacts {
    pub fn capture() -> Result<Self, FailKind> {
        Ok(Self {
            pid: std::process::id(),
            ancestors: fsutil::ancestor_pids(std::process::id()),
            exe: env::current_exe().map_err(|_| FailKind::Io)?,
            cwd: env::current_dir().map_err(|_| FailKind::Io)?,
        })
    }

    pub fn contains_process(&self, process_id: u64) -> bool {
        u64::from(self.pid) == process_id
            || self
                .ancestors
                .iter()
                .any(|ancestor| u64::from(*ancestor) == process_id)
    }
}

impl Binding {
    pub fn from_env() -> Result<Self, FailKind> {
        let wait_slice_secs = match env::var("PARLEY_HANDOFF_WAIT_SLICE_SEC") {
            Ok(value) => parse_slice(&value)?,
            Err(env::VarError::NotPresent) => DEFAULT_WAIT_SLICE_SECS,
            Err(_) => return Err(FailKind::Malformed),
        };
        Ok(Self {
            state_dir: required_path("PARLEY_HANDOFF_STATE_DIR")?,
            job_dir: required_path("PARLEY_HANDOFF_JOB_DIR")?,
            job_id: required_string("PARLEY_HANDOFF_JOB_ID")?,
            handoff_id: required_string("PARLEY_HANDOFF_ID")?,
            capability: required_string("PARLEY_HANDOFF_CAPABILITY")?,
            source_session_id: required_string("PARLEY_HANDOFF_SOURCE_SESSION_ID")?,
            target_session_id: required_string("PARLEY_HANDOFF_TARGET_SESSION_ID")?,
            wait_slice_secs,
            peek_exe: required_path("PARLEY_HANDOFF_PEER_PEEK_EXE")?,
            wait_exe: required_path("PARLEY_HANDOFF_WAIT_EXE")?,
            alert_exe: required_path("PARLEY_HANDOFF_ALERT_EXE")?,
            hook_exe: required_path("PARLEY_HANDOFF_HOOK_EXE")?,
            health_inbox: optional_path("PARLEY_HEALTH_INBOX")?,
            codex_rollout: optional_path("PARLEY_HANDOFF_CODEX_ROLLOUT")?,
            codex_home: codex_home_from_env()?,
        })
    }

    pub fn helper_exe(&self, kind: HelperKind) -> &Path {
        match kind {
            HelperKind::Peek => &self.peek_exe,
            HelperKind::Wait => &self.wait_exe,
            HelperKind::Alert => &self.alert_exe,
            HelperKind::Hook => &self.hook_exe,
        }
    }
}

pub fn validate_binding(
    binding: &Binding,
    facts: &RuntimeFacts,
    role: HelperKind,
    record: &StoredRecord,
    now_ms: u64,
) -> Result<(), FailKind> {
    fsutil::require_job_dir(&binding.state_dir, &binding.job_dir, &binding.job_id)?;
    if !fsutil::same_file(&facts.exe, binding.helper_exe(role))? {
        return Err(FailKind::FileIdentity);
    }
    if record.job_id != binding.job_id || record.handoff_id != binding.handoff_id {
        return Err(FailKind::Session);
    }
    if record.source_session_id.as_deref() != Some(binding.source_session_id.as_str())
        || record.target_session_id.as_deref() != Some(binding.target_session_id.as_str())
    {
        return Err(FailKind::Session);
    }
    let actual_hash = sha256_hex(binding.capability.as_bytes());
    if !actual_hash.eq_ignore_ascii_case(&record.capability_hash) {
        return Err(FailKind::Capability);
    }
    let Some(process_id) = record.process_id else {
        return Err(FailKind::Process);
    };
    if !facts.contains_process(process_id) {
        return Err(FailKind::Process);
    }
    if capability_expired(record, now_ms, binding.wait_slice_secs) {
        return Err(FailKind::Expired);
    }
    Ok(())
}

pub fn capability_expired(record: &StoredRecord, now_ms: u64, slice_secs: u64) -> bool {
    match record.deadline_ms {
        Some(deadline) => now_ms > deadline.saturating_add(slice_secs.saturating_mul(1_000)),
        None => false,
    }
}

fn parse_slice(value: &str) -> Result<u64, FailKind> {
    let parsed = value
        .trim()
        .parse::<u64>()
        .map_err(|_| FailKind::Malformed)?;
    if parsed == 0 || parsed > MAX_WAIT_SLICE_SECS {
        return Err(FailKind::Malformed);
    }
    Ok(parsed)
}

fn required_string(name: &str) -> Result<String, FailKind> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(FailKind::Missing),
    }
}

fn required_path(name: &str) -> Result<PathBuf, FailKind> {
    let value = required_string(name)?;
    let path = PathBuf::from(&value);
    if !path.is_absolute() {
        return Err(FailKind::Path);
    }
    Ok(path)
}

fn optional_path(name: &str) -> Result<Option<PathBuf>, FailKind> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(FailKind::Path);
            }
            Ok(Some(path))
        }
        Ok(_) => Err(FailKind::Path),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(FailKind::Io),
    }
}

fn codex_home_from_env() -> Result<Option<PathBuf>, FailKind> {
    for name in ["PARLEY_CODEX_HOME", "CODEX_HOME"] {
        if let Some(value) = env::var_os(name) {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(FailKind::Path);
            }
            return Ok(Some(path));
        }
    }
    for name in ["HOME", "USERPROFILE"] {
        if let Some(value) = env::var_os(name) {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(FailKind::Path);
            }
            return Ok(Some(path.join(".codex")));
        }
    }
    Ok(None)
}
