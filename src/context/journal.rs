//! Durable per-key JSONL cursor journal and reduced state machine.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fsx;
use crate::json::Json;
use crate::signals::fnv1a_64;

use super::error::{ContextError, ErrorKind};
use super::parse::{optional_str, parse_json, required_object, required_str, required_u64};
use super::plan::{
    fingerprint_records, plan_delta, plan_replay, plan_seed, ContextBounds, ContextPlan,
    MessageAnchor, PlanKind,
};
use super::reader::{ReadBatch, SharedJsonlReader};
use super::root::{source_harness, ResolvedSource};
use super::winfile::{
    open_exclusive_lock, open_shared_read, open_shared_read_write, refuse_reparse_chain,
    SourceIdentity,
};

static RECORD_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorState {
    Absent,
    InFlight,
    Committed,
    Uncertain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContextKey {
    pub target_session: String,
    pub source_harness: String,
    pub source_session: String,
}

impl ContextKey {
    pub(crate) fn new(
        target_session: impl Into<String>,
        source_harness_name: impl Into<String>,
        source_session: impl Into<String>,
    ) -> Result<Self, ContextError> {
        let key = Self {
            target_session: target_session.into(),
            source_harness: source_harness_name.into(),
            source_session: source_session.into(),
        };
        sanitize_id("target_session", &key.target_session)?;
        sanitize_id("source_harness", &key.source_harness)?;
        sanitize_id("source_session", &key.source_session)?;
        if key.source_harness != source_harness() {
            return Err(ContextError::unsupported(format!(
                "source harness {} is not supported by this context module",
                key.source_harness
            )));
        }
        Ok(key)
    }

    fn encoded(&self) -> String {
        format!(
            "{}__{}__{}",
            self.target_session, self.source_harness, self.source_session
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorSnapshot {
    pub state: CursorState,
    pub target_session: String,
    pub source_harness: String,
    pub source_session: String,
    pub source_identity: Option<SourceIdentity>,
    pub start_offset: u64,
    pub end_offset: u64,
    pub first_message: Option<MessageAnchor>,
    pub last_message: Option<MessageAnchor>,
    pub first_turn_id: Option<String>,
    pub last_turn_id: Option<String>,
    pub record_count: u64,
    pub character_count: u64,
    pub fingerprint: Option<String>,
    pub exchange_id: Option<String>,
    pub recovery_action: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StateDirEnv {
    pub parley_context_state_dir: Option<PathBuf>,
    pub localappdata: Option<PathBuf>,
}

impl StateDirEnv {
    pub(crate) fn from_process() -> Self {
        Self {
            parley_context_state_dir: env_path("PARLEY_CONTEXT_STATE_DIR"),
            localappdata: env_path("LOCALAPPDATA"),
        }
    }
}

pub(crate) fn resolve_state_root(env: &StateDirEnv) -> Result<PathBuf, ContextError> {
    if let Some(path) = env
        .parley_context_state_dir
        .as_ref()
        .filter(|value| !value.as_os_str().is_empty())
    {
        return Ok(path.clone());
    }
    #[cfg(windows)]
    {
        let local = env
            .localappdata
            .as_ref()
            .filter(|value| !value.as_os_str().is_empty())
            .cloned()
            .or_else(|| env_path("LOCALAPPDATA"));
        let Some(local) = local else {
            return Err(ContextError::not_found(
                "cannot resolve context state dir (PARLEY_CONTEXT_STATE_DIR or LOCALAPPDATA)",
            ));
        };
        Ok(local.join("Parley").join("context"))
    }
    #[cfg(not(windows))]
    {
        Err(ContextError::unsupported(
            "default context state dir is Windows-only; set PARLEY_CONTEXT_STATE_DIR",
        ))
    }
}

pub(crate) struct KeyLock {
    _file: File,
}

pub(crate) struct JournalStore {
    root: PathBuf,
    live_in_flight: Mutex<HashSet<String>>,
}

impl JournalStore {
    pub(crate) fn open(env: &StateDirEnv) -> Result<Self, ContextError> {
        let root = resolve_state_root(env)?;
        Ok(Self {
            root,
            live_in_flight: Mutex::new(HashSet::new()),
        })
    }

    #[cfg(test)]
    pub(crate) fn open_at(root: PathBuf) -> Self {
        Self {
            root,
            live_in_flight: Mutex::new(HashSet::new()),
        }
    }

    pub(crate) fn lock_key(&self, key: &ContextKey) -> Result<KeyLock, ContextError> {
        let dir = self.key_dir(key)?;
        refuse_reparse_chain(&self.root)?;
        std::fs::create_dir_all(&dir).map_err(|error| {
            ContextError::new(ErrorKind::Io, format!("create {}: {error}", dir.display()))
        })?;
        refuse_reparse_chain(&dir)?;
        let lock_path = dir.join("lock");
        fsx::refuse_if_symlink(&lock_path)
            .map_err(|error| ContextError::new(ErrorKind::Io, error))?;
        Ok(KeyLock {
            _file: open_exclusive_lock(&lock_path)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn inspect(&self, key: &ContextKey) -> Result<CursorSnapshot, ContextError> {
        let _lock = self.lock_key(key)?;
        self.inspect_locked(key)
    }

    pub(crate) fn plan_new_session_seed(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
        bounds: &ContextBounds,
    ) -> Result<ContextPlan, ContextError> {
        let _lock = self.lock_key(key)?;
        match self.inspect_locked(key)?.state {
            CursorState::Absent => {}
            other => {
                return Err(ContextError::state(format!(
                    "new target session {} already has {other:?} context state",
                    key.target_session
                )));
            }
        }
        self.verify_source_session(key, source)?;
        let mut reader = SharedJsonlReader::open(&source.path)?;
        self.verify_live_identity(source, reader.identity())?;
        let batch = reader.read_from(0)?;
        plan_seed(source, &batch.records, batch.complete_end, bounds)
    }

    pub(crate) fn plan_new_session_seed_read_only(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
        bounds: &ContextBounds,
    ) -> Result<ContextPlan, ContextError> {
        match self.inspect_locked(key)?.state {
            CursorState::Absent => {}
            other => {
                return Err(ContextError::state(format!(
                    "new target session {} already has {other:?} context state",
                    key.target_session
                )));
            }
        }
        self.verify_source_session(key, source)?;
        let mut reader = SharedJsonlReader::open(&source.path)?;
        self.verify_live_identity(source, reader.identity())?;
        let batch = reader.read_from(0)?;
        plan_seed(source, &batch.records, batch.complete_end, bounds)
    }

    pub(crate) fn plan_resume_delta(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
        bounds: &ContextBounds,
    ) -> Result<ContextPlan, ContextError> {
        let _lock = self.lock_key(key)?;
        let snapshot = self.inspect_locked(key)?;
        match snapshot.state {
            CursorState::Committed => {}
            CursorState::Absent => {
                return Err(ContextError::state(
                    "resume requires committed context state",
                ));
            }
            CursorState::Uncertain => {
                return Err(ContextError::state(
                    "uncertain context state requires replay or skip recovery",
                ));
            }
            CursorState::InFlight => {
                return Err(ContextError::state(
                    "in-flight context state cannot plan a resume delta",
                ));
            }
        }
        self.prepare_delta(key, source, &snapshot, bounds)
    }

    pub(crate) fn plan_resume_delta_read_only(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
        bounds: &ContextBounds,
    ) -> Result<ContextPlan, ContextError> {
        let snapshot = self.inspect_locked(key)?;
        match snapshot.state {
            CursorState::Committed => self.prepare_delta(key, source, &snapshot, bounds),
            CursorState::Absent => Err(ContextError::state(
                "resume requires committed context state",
            )),
            CursorState::Uncertain => Err(ContextError::state(
                "uncertain context state requires replay or skip recovery",
            )),
            CursorState::InFlight => Err(ContextError::state(
                "in-flight context state cannot plan a resume delta",
            )),
        }
    }

    pub(crate) fn plan_uncertain_replay(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
    ) -> Result<ContextPlan, ContextError> {
        let _lock = self.lock_key(key)?;
        let snapshot = self.inspect_locked(key)?;
        if snapshot.state != CursorState::Uncertain {
            return Err(ContextError::state(
                "replay is only valid for uncertain resume recovery",
            ));
        }
        self.verify_source_session(key, source)?;
        let mut reader = SharedJsonlReader::open(&source.path)?;
        self.verify_live_identity(source, reader.identity())?;
        self.verify_identity(&snapshot, reader.identity())?;
        let batch = reader.read_until(snapshot.start_offset, snapshot.end_offset)?;
        self.verify_range(&snapshot, &batch)?;
        plan_replay(
            source,
            &batch.records,
            snapshot.start_offset,
            snapshot.end_offset,
        )
    }

    pub(crate) fn plan_uncertain_replay_read_only(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
    ) -> Result<ContextPlan, ContextError> {
        let snapshot = self.inspect_locked(key)?;
        if snapshot.state != CursorState::Uncertain {
            return Err(ContextError::state(
                "replay is only valid for uncertain resume recovery",
            ));
        }
        self.verify_source_session(key, source)?;
        let mut reader = SharedJsonlReader::open(&source.path)?;
        self.verify_live_identity(source, reader.identity())?;
        self.verify_identity(&snapshot, reader.identity())?;
        let batch = reader.read_until(snapshot.start_offset, snapshot.end_offset)?;
        self.verify_range(&snapshot, &batch)?;
        plan_replay(
            source,
            &batch.records,
            snapshot.start_offset,
            snapshot.end_offset,
        )
    }

    pub(crate) fn attest_skip(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
        exchange_id: &str,
    ) -> Result<CursorSnapshot, ContextError> {
        let _lock = self.lock_key(key)?;
        let snapshot = self.inspect_locked(key)?;
        if snapshot.state != CursorState::Uncertain {
            return Err(ContextError::state(
                "skip is only valid for uncertain resume recovery",
            ));
        }
        self.verify_source_session(key, source)?;
        let mut reader = SharedJsonlReader::open(&source.path)?;
        self.verify_live_identity(source, reader.identity())?;
        self.verify_identity(&snapshot, reader.identity())?;
        let batch = reader.read_until(snapshot.start_offset, snapshot.end_offset)?;
        self.verify_range(&snapshot, &batch)?;
        let skip_plan = snapshot_to_plan(&snapshot)?;
        self.append_locked(
            key,
            &skip_plan,
            JournalState::Committed,
            exchange_id,
            Some("skip"),
        )?;
        self.clear_live(key);
        self.inspect_locked(key)
    }

    #[cfg(test)]
    pub(crate) fn plan_skip_then_delta(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
        bounds: &ContextBounds,
        exchange_id: &str,
    ) -> Result<ContextPlan, ContextError> {
        let _lock = self.lock_key(key)?;
        let snapshot = self.inspect_locked(key)?;
        if snapshot.state != CursorState::Uncertain {
            return Err(ContextError::state(
                "skip is only valid for uncertain resume recovery",
            ));
        }
        self.verify_source_session(key, source)?;
        let mut reader = SharedJsonlReader::open(&source.path)?;
        self.verify_live_identity(source, reader.identity())?;
        self.verify_identity(&snapshot, reader.identity())?;
        let skipped = reader.read_until(snapshot.start_offset, snapshot.end_offset)?;
        self.verify_range(&snapshot, &skipped)?;
        let skip_plan = snapshot_to_plan(&snapshot)?;
        self.append_locked(
            key,
            &skip_plan,
            JournalState::Committed,
            exchange_id,
            Some("skip"),
        )?;
        self.clear_live(key);
        let committed = self.inspect_locked(key)?;
        self.prepare_delta(key, source, &committed, bounds)
    }

    pub(crate) fn preview_skip_then_delta(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
        bounds: &ContextBounds,
    ) -> Result<ContextPlan, ContextError> {
        let _lock = self.lock_key(key)?;
        let snapshot = self.inspect_locked(key)?;
        if snapshot.state != CursorState::Uncertain {
            return Err(ContextError::state(
                "skip is only valid for uncertain resume recovery",
            ));
        }
        self.verify_source_session(key, source)?;
        let mut reader = SharedJsonlReader::open(&source.path)?;
        self.verify_live_identity(source, reader.identity())?;
        self.verify_identity(&snapshot, reader.identity())?;
        let skipped = reader.read_until(snapshot.start_offset, snapshot.end_offset)?;
        self.verify_range(&snapshot, &skipped)?;
        self.prepare_delta(key, source, &snapshot, bounds)
    }

    pub(crate) fn preview_skip_then_delta_read_only(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
        bounds: &ContextBounds,
    ) -> Result<ContextPlan, ContextError> {
        let snapshot = self.inspect_locked(key)?;
        if snapshot.state != CursorState::Uncertain {
            return Err(ContextError::state(
                "skip is only valid for uncertain resume recovery",
            ));
        }
        self.verify_source_session(key, source)?;
        let mut reader = SharedJsonlReader::open(&source.path)?;
        self.verify_live_identity(source, reader.identity())?;
        self.verify_identity(&snapshot, reader.identity())?;
        let skipped = reader.read_until(snapshot.start_offset, snapshot.end_offset)?;
        self.verify_range(&snapshot, &skipped)?;
        self.prepare_delta(key, source, &snapshot, bounds)
    }

    pub(crate) fn append_in_flight(
        &self,
        key: &ContextKey,
        plan: &ContextPlan,
        exchange_id: &str,
    ) -> Result<CursorSnapshot, ContextError> {
        let _lock = self.lock_key(key)?;
        let snapshot = self.inspect_locked(key)?;
        match (plan.kind, snapshot.state) {
            (PlanKind::Seed, CursorState::Absent) => {}
            (PlanKind::Delta, CursorState::Committed) => {}
            (PlanKind::Replay, CursorState::Uncertain) => {}
            (kind, state) => {
                return Err(ContextError::state(format!(
                    "cannot append {kind:?} in_flight from {state:?}"
                )));
            }
        }
        self.append_locked(key, plan, JournalState::InFlight, exchange_id, None)?;
        self.mark_live(key);
        self.inspect_locked(key)
    }

    pub(crate) fn commit(
        &self,
        key: &ContextKey,
        plan: &ContextPlan,
        exchange_id: &str,
    ) -> Result<CursorSnapshot, ContextError> {
        let _lock = self.lock_key(key)?;
        let snapshot = self.inspect_locked(key)?;
        if snapshot.state != CursorState::InFlight {
            return Err(ContextError::state(
                "commit requires a live in_flight context record",
            ));
        }
        if snapshot.fingerprint.as_deref() != Some(plan.fingerprint.as_str())
            || snapshot.start_offset != plan.start_offset
            || snapshot.end_offset != plan.end_offset
        {
            return Err(ContextError::mismatch(
                "commit plan does not match the in_flight cursor",
            ));
        }
        self.append_locked(key, plan, JournalState::Committed, exchange_id, None)?;
        self.clear_live(key);
        self.inspect_locked(key)
    }

    pub(crate) fn mark_uncertain(
        &self,
        key: &ContextKey,
        exchange_id: &str,
    ) -> Result<CursorSnapshot, ContextError> {
        let _lock = self.lock_key(key)?;
        let snapshot = self.inspect_locked(key)?;
        if snapshot.state != CursorState::InFlight {
            return Err(ContextError::state(
                "mark_uncertain requires a live in_flight context record",
            ));
        }
        let plan = snapshot_to_plan(&snapshot)?;
        self.append_locked(
            key,
            &plan,
            JournalState::Uncertain,
            exchange_id,
            Some("mark"),
        )?;
        self.clear_live(key);
        self.inspect_locked(key)
    }

    pub(crate) fn abort_to_committed(
        &self,
        key: &ContextKey,
        exchange_id: &str,
    ) -> Result<CursorSnapshot, ContextError> {
        let _lock = self.lock_key(key)?;
        let records = self.load_journal(key)?;
        let Some(last) = records.last() else {
            return Err(ContextError::state("abort requires an in_flight record"));
        };
        let is_live = self
            .live_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&key.encoded());
        if last.state != JournalState::InFlight || !is_live {
            return Err(ContextError::state(
                "abort is only valid for a live in_flight call that never spawned",
            ));
        }
        let plan = journal_to_plan(last)?;
        self.append_locked(
            key,
            &plan,
            JournalState::Aborted,
            exchange_id,
            Some("abort"),
        )?;
        self.clear_live(key);
        self.inspect_locked(key)
    }

    fn prepare_delta(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
        snapshot: &CursorSnapshot,
        bounds: &ContextBounds,
    ) -> Result<ContextPlan, ContextError> {
        self.verify_source_session(key, source)?;
        let mut reader = SharedJsonlReader::open(&source.path)?;
        self.verify_live_identity(source, reader.identity())?;
        self.verify_identity(snapshot, reader.identity())?;
        let prior = reader.read_until(snapshot.start_offset, snapshot.end_offset)?;
        self.verify_range(snapshot, &prior)?;
        let next = reader.read_from(snapshot.end_offset)?;
        plan_delta(
            source,
            &next.records,
            snapshot.end_offset,
            next.complete_end,
            bounds,
        )
    }

    fn inspect_locked(&self, key: &ContextKey) -> Result<CursorSnapshot, ContextError> {
        let records = self.load_journal(key)?;
        let Some(last) = records.last() else {
            return Ok(absent_snapshot(key));
        };
        if last.state == JournalState::Aborted {
            return match records[..records.len() - 1]
                .iter()
                .rev()
                .find(|record| record.state == JournalState::Committed)
            {
                Some(prior) => Ok(snapshot_from_record(prior, CursorState::Committed)),
                None => Ok(absent_snapshot(key)),
            };
        }
        let live = self
            .live_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&key.encoded());
        let state = match last.state {
            JournalState::Committed => CursorState::Committed,
            JournalState::Uncertain => CursorState::Uncertain,
            JournalState::InFlight if live => CursorState::InFlight,
            JournalState::InFlight => CursorState::Uncertain,
            JournalState::Aborted => unreachable!("aborted state handled above"),
        };
        Ok(snapshot_from_record(last, state))
    }

    fn load_journal(&self, key: &ContextKey) -> Result<Vec<JournalRecord>, ContextError> {
        let path = self.journal_path(key)?;
        let mut file = match open_shared_read(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let mut records = Vec::new();
        let mut offset = 0usize;
        if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            offset = 3;
        }
        while offset < bytes.len() {
            match bytes[offset..].iter().position(|&b| b == b'\n') {
                Some(rel) => {
                    let mut line = &bytes[offset..offset + rel];
                    if line.last() == Some(&b'\r') {
                        line = &line[..line.len().saturating_sub(1)];
                    }
                    offset += rel + 1;
                    if line.is_empty() {
                        continue;
                    }
                    let text = std::str::from_utf8(line).map_err(|_| {
                        ContextError::corrupt("journal contains an invalid UTF-8 line")
                    })?;
                    records.push(parse_journal_record(text).map_err(|error| {
                        ContextError::corrupt(format!("journal record: {}", error.message))
                    })?);
                }
                None => {
                    // Incomplete trailing line is never committed.
                    break;
                }
            }
        }
        Ok(records)
    }

    fn append_locked(
        &self,
        key: &ContextKey,
        plan: &ContextPlan,
        state: JournalState,
        exchange_id: &str,
        recovery_action: Option<&str>,
    ) -> Result<(), ContextError> {
        if exchange_id.is_empty() {
            return Err(ContextError::malformed("exchange_id must not be empty"));
        }
        let path = self.journal_path(key)?;
        refuse_reparse_chain(&self.root)?;
        fsx::refuse_if_symlink(&path).map_err(|error| ContextError::new(ErrorKind::Io, error))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                ContextError::new(
                    ErrorKind::Io,
                    format!("create {}: {error}", parent.display()),
                )
            })?;
        }
        refuse_reparse_chain(path.parent().unwrap_or(&self.root))?;
        let record = JournalRecord {
            schema_version: 1,
            record_id: new_record_id(),
            timestamp_ms: timestamp_ms(),
            state,
            target_session: key.target_session.clone(),
            source_harness: key.source_harness.clone(),
            source_session: key.source_session.clone(),
            source_identity: plan.source_identity.clone(),
            start_offset: plan.start_offset,
            end_offset: plan.end_offset,
            first_message: plan.first_message.clone(),
            last_message: plan.last_message.clone(),
            first_turn_id: plan.first_turn_id.clone(),
            last_turn_id: plan.last_turn_id.clone(),
            record_count: plan.record_count,
            character_count: plan.character_count,
            fingerprint: plan.fingerprint.clone(),
            exchange_id: exchange_id.to_string(),
            recovery_action: recovery_action.map(str::to_string),
        };
        let line = record.to_json().to_compact_string();
        if line.contains('\n') {
            return Err(ContextError::malformed("journal record contains a newline"));
        }
        let mut file = open_shared_read_write(&path)?;
        clip_incomplete_journal(&mut file)?;
        let mut encoded = Vec::from(line.as_bytes());
        encoded.push(b'\n');
        file.write_all(&encoded)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                ContextError::new(ErrorKind::Io, format!("append {}: {error}", path.display()))
            })
    }

    fn key_dir(&self, key: &ContextKey) -> Result<PathBuf, ContextError> {
        Ok(self
            .root
            .join(&key.target_session)
            .join(&key.source_harness)
            .join(&key.source_session))
    }

    fn journal_path(&self, key: &ContextKey) -> Result<PathBuf, ContextError> {
        Ok(self.key_dir(key)?.join("journal.jsonl"))
    }

    fn verify_source_session(
        &self,
        key: &ContextKey,
        source: &ResolvedSource,
    ) -> Result<(), ContextError> {
        if key.source_session != source.session_id {
            return Err(ContextError::mismatch(format!(
                "source session {} does not match key {}",
                source.session_id, key.source_session
            )));
        }
        Ok(())
    }

    fn verify_live_identity(
        &self,
        source: &ResolvedSource,
        live: &SourceIdentity,
    ) -> Result<(), ContextError> {
        if !source.identity.same_file(live) {
            return Err(ContextError::new(
                ErrorKind::Replaced,
                format!("source file identity changed for {}", source.session_id),
            ));
        }
        Ok(())
    }

    fn verify_identity(
        &self,
        snapshot: &CursorSnapshot,
        live: &SourceIdentity,
    ) -> Result<(), ContextError> {
        match &snapshot.source_identity {
            Some(stored) if stored.same_file(live) => Ok(()),
            Some(_) => Err(ContextError::new(
                ErrorKind::Replaced,
                "source file was replaced since the cursor was written",
            )),
            None => Err(ContextError::mismatch("cursor is missing source identity")),
        }
    }

    fn verify_range(
        &self,
        snapshot: &CursorSnapshot,
        batch: &ReadBatch,
    ) -> Result<(), ContextError> {
        if batch.complete_end != snapshot.end_offset {
            return Err(ContextError::mismatch(format!(
                "source end offset {} does not match cursor {}",
                batch.complete_end, snapshot.end_offset
            )));
        }
        let fingerprint = fingerprint_records(&batch.records);
        if snapshot.fingerprint.as_deref() != Some(fingerprint.as_str()) {
            return Err(ContextError::mismatch(
                "source fingerprint does not match the stored cursor",
            ));
        }
        let messages = super::sanitize::visible_messages(&batch.records)?;
        let first = messages.first().map(|message| MessageAnchor {
            byte_offset: message.start_offset,
            message_id: message.message_id.clone(),
            turn_id: message.turn_id.clone(),
        });
        let last = messages.last().map(|message| MessageAnchor {
            byte_offset: message.start_offset,
            message_id: message.message_id.clone(),
            turn_id: message.turn_id.clone(),
        });
        if snapshot.first_message.as_ref() != first.as_ref()
            || snapshot.last_message.as_ref() != last.as_ref()
        {
            return Err(ContextError::mismatch(
                "source message anchors do not match the stored cursor",
            ));
        }
        if snapshot.first_turn_id.as_ref() != messages.first().map(|message| &message.turn_id)
            || snapshot.last_turn_id.as_ref() != messages.last().map(|message| &message.turn_id)
        {
            return Err(ContextError::mismatch(
                "source turn anchors do not match the stored cursor",
            ));
        }
        Ok(())
    }

    fn mark_live(&self, key: &ContextKey) {
        self.live_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key.encoded());
    }

    fn clear_live(&self, key: &ContextKey) {
        self.live_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&key.encoded());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JournalState {
    InFlight,
    Committed,
    Uncertain,
    Aborted,
}

impl JournalState {
    fn as_str(self) -> &'static str {
        match self {
            Self::InFlight => "in_flight",
            Self::Committed => "committed",
            Self::Uncertain => "uncertain",
            Self::Aborted => "aborted",
        }
    }

    fn parse(value: &str) -> Result<Self, ContextError> {
        match value {
            "in_flight" => Ok(Self::InFlight),
            "committed" => Ok(Self::Committed),
            "uncertain" => Ok(Self::Uncertain),
            "aborted" => Ok(Self::Aborted),
            other => Err(ContextError::corrupt(format!(
                "unsupported journal state {other}"
            ))),
        }
    }
}

struct JournalRecord {
    schema_version: u64,
    record_id: String,
    timestamp_ms: u64,
    state: JournalState,
    target_session: String,
    source_harness: String,
    source_session: String,
    source_identity: SourceIdentity,
    start_offset: u64,
    end_offset: u64,
    first_message: Option<MessageAnchor>,
    last_message: Option<MessageAnchor>,
    first_turn_id: Option<String>,
    last_turn_id: Option<String>,
    record_count: u64,
    character_count: u64,
    fingerprint: String,
    exchange_id: String,
    recovery_action: Option<String>,
}

impl JournalRecord {
    fn to_json(&self) -> Json {
        let mut map = BTreeMap::new();
        map.insert(
            "schema_version".to_string(),
            Json::Number(self.schema_version as f64),
        );
        map.insert("record_id".to_string(), Json::Str(self.record_id.clone()));
        map.insert(
            "timestamp_ms".to_string(),
            Json::Number(self.timestamp_ms as f64),
        );
        map.insert(
            "state".to_string(),
            Json::Str(self.state.as_str().to_string()),
        );
        map.insert(
            "target_session".to_string(),
            Json::Str(self.target_session.clone()),
        );
        map.insert(
            "source_harness".to_string(),
            Json::Str(self.source_harness.clone()),
        );
        map.insert(
            "source_session".to_string(),
            Json::Str(self.source_session.clone()),
        );
        map.insert(
            "source_identity".to_string(),
            identity_json(&self.source_identity),
        );
        map.insert(
            "start_offset".to_string(),
            Json::Number(self.start_offset as f64),
        );
        map.insert(
            "end_offset".to_string(),
            Json::Number(self.end_offset as f64),
        );
        map.insert(
            "first_message_anchor".to_string(),
            anchor_json(self.first_message.as_ref()),
        );
        map.insert(
            "last_message_anchor".to_string(),
            anchor_json(self.last_message.as_ref()),
        );
        map.insert(
            "first_turn_id".to_string(),
            opt_str_json(self.first_turn_id.as_deref()),
        );
        map.insert(
            "last_turn_id".to_string(),
            opt_str_json(self.last_turn_id.as_deref()),
        );
        map.insert(
            "record_count".to_string(),
            Json::Number(self.record_count as f64),
        );
        map.insert(
            "character_count".to_string(),
            Json::Number(self.character_count as f64),
        );
        map.insert(
            "fingerprint".to_string(),
            Json::Str(self.fingerprint.clone()),
        );
        map.insert(
            "exchange_id".to_string(),
            Json::Str(self.exchange_id.clone()),
        );
        map.insert(
            "recovery_action".to_string(),
            opt_str_json(self.recovery_action.as_deref()),
        );
        Json::Object(map)
    }
}

fn parse_journal_record(line: &str) -> Result<JournalRecord, ContextError> {
    let json = parse_json(line).map_err(|error| {
        ContextError::corrupt(format!("malformed journal record: {}", error.message))
    })?;
    let schema_version = required_u64(&json, "schema_version")
        .map_err(|error| ContextError::corrupt(error.message))?;
    if schema_version != 1 {
        return Err(ContextError::corrupt(format!(
            "unsupported journal schema {schema_version}"
        )));
    }
    let identity = parse_identity(required_object(&json, "source_identity")?)?;
    Ok(JournalRecord {
        schema_version,
        record_id: required_str(&json, "record_id")?,
        timestamp_ms: required_u64(&json, "timestamp_ms")?,
        state: JournalState::parse(&required_str(&json, "state")?)?,
        target_session: required_str(&json, "target_session")?,
        source_harness: required_str(&json, "source_harness")?,
        source_session: required_str(&json, "source_session")?,
        source_identity: identity,
        start_offset: required_u64(&json, "start_offset")?,
        end_offset: required_u64(&json, "end_offset")?,
        first_message: parse_anchor(json.get("first_message_anchor"))?,
        last_message: parse_anchor(json.get("last_message_anchor"))?,
        first_turn_id: optional_str(&json, "first_turn_id")?,
        last_turn_id: optional_str(&json, "last_turn_id")?,
        record_count: required_u64(&json, "record_count")?,
        character_count: required_u64(&json, "character_count")?,
        fingerprint: required_str(&json, "fingerprint")?,
        exchange_id: required_str(&json, "exchange_id")?,
        recovery_action: optional_str(&json, "recovery_action")?,
    })
}

fn parse_identity(object: &Json) -> Result<SourceIdentity, ContextError> {
    let volume = required_str(object, "volume_serial")?;
    let index = required_str(object, "file_index")?;
    Ok(SourceIdentity {
        volume_serial: volume.parse::<u32>().map_err(|_| {
            ContextError::corrupt("journal source_identity.volume_serial is not a u32")
        })?,
        file_index: index.parse::<u64>().map_err(|_| {
            ContextError::corrupt("journal source_identity.file_index is not a u64")
        })?,
        canonical_path: required_str(object, "path_fingerprint")?,
    })
}

fn parse_anchor(value: Option<&Json>) -> Result<Option<MessageAnchor>, ContextError> {
    match value {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Object(_)) => {
            let object = value.unwrap();
            Ok(Some(MessageAnchor {
                byte_offset: required_u64(object, "byte_offset")?,
                message_id: required_str(object, "message_id")?,
                turn_id: required_str(object, "turn_id")?,
            }))
        }
        Some(_) => Err(ContextError::corrupt("journal message anchor is malformed")),
    }
}

fn identity_json(identity: &SourceIdentity) -> Json {
    let mut map = BTreeMap::new();
    map.insert(
        "volume_serial".to_string(),
        Json::Str(identity.volume_serial.to_string()),
    );
    map.insert(
        "file_index".to_string(),
        Json::Str(identity.file_index.to_string()),
    );
    map.insert(
        "path_fingerprint".to_string(),
        Json::Str(format!("{:016x}", fnv1a_64(&identity.canonical_path))),
    );
    Json::Object(map)
}

fn anchor_json(anchor: Option<&MessageAnchor>) -> Json {
    match anchor {
        None => Json::Null,
        Some(anchor) => {
            let mut map = BTreeMap::new();
            map.insert(
                "byte_offset".to_string(),
                Json::Number(anchor.byte_offset as f64),
            );
            map.insert(
                "message_id".to_string(),
                Json::Str(anchor.message_id.clone()),
            );
            map.insert("turn_id".to_string(), Json::Str(anchor.turn_id.clone()));
            Json::Object(map)
        }
    }
}

fn opt_str_json(value: Option<&str>) -> Json {
    match value {
        Some(value) => Json::Str(value.to_string()),
        None => Json::Null,
    }
}

fn snapshot_to_plan(snapshot: &CursorSnapshot) -> Result<ContextPlan, ContextError> {
    let identity = snapshot
        .source_identity
        .clone()
        .ok_or_else(|| ContextError::mismatch("cursor is missing source identity"))?;
    Ok(ContextPlan {
        kind: PlanKind::Replay,
        text: String::new(),
        truncated_front: false,
        source_identity: identity,
        source_session: snapshot.source_session.clone(),
        start_offset: snapshot.start_offset,
        end_offset: snapshot.end_offset,
        first_message: snapshot.first_message.clone(),
        last_message: snapshot.last_message.clone(),
        first_turn_id: snapshot.first_turn_id.clone(),
        last_turn_id: snapshot.last_turn_id.clone(),
        record_count: snapshot.record_count,
        character_count: snapshot.character_count,
        fingerprint: snapshot.fingerprint.clone().unwrap_or_default(),
    })
}

fn journal_to_plan(record: &JournalRecord) -> Result<ContextPlan, ContextError> {
    Ok(ContextPlan {
        kind: PlanKind::Delta,
        text: String::new(),
        truncated_front: false,
        source_identity: record.source_identity.clone(),
        source_session: record.source_session.clone(),
        start_offset: record.start_offset,
        end_offset: record.end_offset,
        first_message: record.first_message.clone(),
        last_message: record.last_message.clone(),
        first_turn_id: record.first_turn_id.clone(),
        last_turn_id: record.last_turn_id.clone(),
        record_count: record.record_count,
        character_count: record.character_count,
        fingerprint: record.fingerprint.clone(),
    })
}

fn absent_snapshot(key: &ContextKey) -> CursorSnapshot {
    CursorSnapshot {
        state: CursorState::Absent,
        target_session: key.target_session.clone(),
        source_harness: key.source_harness.clone(),
        source_session: key.source_session.clone(),
        source_identity: None,
        start_offset: 0,
        end_offset: 0,
        first_message: None,
        last_message: None,
        first_turn_id: None,
        last_turn_id: None,
        record_count: 0,
        character_count: 0,
        fingerprint: None,
        exchange_id: None,
        recovery_action: None,
    }
}

fn snapshot_from_record(record: &JournalRecord, state: CursorState) -> CursorSnapshot {
    CursorSnapshot {
        state,
        target_session: record.target_session.clone(),
        source_harness: record.source_harness.clone(),
        source_session: record.source_session.clone(),
        source_identity: Some(record.source_identity.clone()),
        start_offset: record.start_offset,
        end_offset: record.end_offset,
        first_message: record.first_message.clone(),
        last_message: record.last_message.clone(),
        first_turn_id: record.first_turn_id.clone(),
        last_turn_id: record.last_turn_id.clone(),
        record_count: record.record_count,
        character_count: record.character_count,
        fingerprint: Some(record.fingerprint.clone()),
        exchange_id: Some(record.exchange_id.clone()),
        recovery_action: record.recovery_action.clone(),
    }
}

fn sanitize_id(label: &str, value: &str) -> Result<(), ContextError> {
    if value.is_empty() {
        return Err(ContextError::malformed(format!(
            "{label} must not be empty"
        )));
    }
    if value == "." || value == ".." || value.contains("..") {
        return Err(ContextError::malformed(format!(
            "{label} contains a forbidden path element"
        )));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err(ContextError::malformed(format!(
            "{label} contains unsupported characters"
        )));
    }
    Ok(())
}

fn new_record_id() -> String {
    format!(
        "crec-{}-{}-{}",
        timestamp_ms(),
        std::process::id(),
        RECORD_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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

fn clip_incomplete_journal(file: &mut File) -> Result<(), ContextError> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let keep = match buf.iter().rposition(|&byte| byte == b'\n') {
        Some(index) => (index + 1) as u64,
        None => 0,
    };
    if keep < len {
        file.set_len(keep)?;
    }
    file.seek(SeekFrom::End(0))?;
    Ok(())
}
