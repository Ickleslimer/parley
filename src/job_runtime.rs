//! Long-lived asynchronous Grok jobs for the MCP stdio server.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ask::{self, AskFailure, AskSpawnObserver, AskSpawnReceipt};
use crate::context::refuse_reparse_chain;
use crate::event_log::{EventReceipt, ExchangeReceipt};
use crate::grok_subagents::{append_parent_contract, GuardedSubagentLaunch};
use crate::job_lock::ExecutionLease;
use crate::jobs::{
    BeginOutcome, HandoffMode, JobMode, JobSpec, JobState, JobStateEnv, JobStore, JobTransition,
    JobView, LaneOwner, LaneRole, MAX_LISTED_JOBS,
};
use crate::json::Json;
use crate::lane_plan;
use crate::policy::RuntimePolicy;
use crate::process::{CancellationToken, Captured, OutputStream, Timeouts};
use crate::profile_namespace;
use crate::sha256::{hex_encode, sha256};
use crate::stream_activity::{decode_stream_line, ActivityClass, SafeActivity};

const HANDOFF_SCHEMA_VERSION: u64 = 1;
const PEER_ACTIVITY_SCHEMA_VERSION: u64 = 1;
const MAX_HANDOFF_REPORT_CHARS: usize = 256_000;
const MAX_STREAM_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ACTIVITY_EVENTS: usize = 64;
const MAX_HANDOFF_JOURNAL_BYTES: usize = 8 * 1024 * 1024;
const MAX_HANDOFF_JOURNAL_LINE_BYTES: usize = 512 * 1024;
const ACTIVITY_FLUSH_INTERVAL_MS: u128 = 120_000;
static HANDOFF_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(crate) struct JobRuntime {
    inner: Option<Arc<RuntimeInner>>,
}

struct RuntimeInner {
    store: Arc<JobStore>,
    active: Mutex<HashMap<String, ActiveJob>>,
    terminal: Mutex<HashMap<String, TerminalPayload>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    event_log: Option<PathBuf>,
    escrow_dir: PathBuf,
    handoff_dir: Option<PathBuf>,
    handoff_lock: Mutex<()>,
    max_concurrent: usize,
}

#[derive(Clone)]
struct ActiveJob {
    cancellation: CancellationToken,
    session_key: String,
    worktree_keys: Vec<String>,
}

#[derive(Clone)]
struct TerminalPayload {
    reply: Option<String>,
    error: Option<String>,
    receipt: Option<ExchangeReceipt>,
    state_override: Option<JobState>,
}

pub(crate) struct JobLookup {
    pub(crate) view: JobView,
    pub(crate) delivery: Option<ExchangeReceipt>,
    pub(crate) handoff: Option<HandoffView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HandoffView {
    pub(crate) handoff_id: String,
    pub(crate) state: String,
    pub(crate) report: Option<String>,
    pub(crate) report_fingerprint: Option<String>,
    pub(crate) ready_at_ms: Option<u128>,
    pub(crate) deadline_ms: Option<u128>,
    pub(crate) receipt_at_ms: Option<u128>,
    pub(crate) alert_incident_id: Option<String>,
}

enum StartSignal {
    Running,
    Terminal,
    Failed(String),
}

impl JobRuntime {
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self { inner: None }
    }

    pub(crate) fn from_env() -> Result<Self, String> {
        if !env_bool("PARLEY_ASYNC_JOBS_ENABLED")?.unwrap_or(false) {
            return Ok(Self { inner: None });
        }
        #[cfg(not(windows))]
        return Err("locked asynchronous jobs require Windows Job Objects".to_string());

        let state_root = env::var_os("PARLEY_JOB_STATE_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| "PARLEY_ASYNC_JOBS_ENABLED requires PARLEY_JOB_STATE_DIR".to_string())?;
        if !state_root.is_absolute() {
            return Err("PARLEY_JOB_STATE_DIR must be absolute".to_string());
        }
        let require_id = env_bool("PARLEY_JOB_REQUIRE_EXPLICIT_ID")?.unwrap_or(true);
        if !require_id {
            return Err("asynchronous jobs require caller-generated UUIDs".to_string());
        }
        let max_concurrent = env::var("PARLEY_JOB_MAX_CONCURRENT")
            .ok()
            .map(|value| {
                value.trim().parse::<usize>().map_err(|_| {
                    format!("PARLEY_JOB_MAX_CONCURRENT must be a positive integer, got {value}")
                })
            })
            .transpose()?
            .unwrap_or(1);
        if max_concurrent != 1 {
            return Err(
                "locked asynchronous jobs currently require PARLEY_JOB_MAX_CONCURRENT=1"
                    .to_string(),
            );
        }
        let profile_namespace = profile_namespace::from_env()?;
        let store = Arc::new(
            JobStore::open(&JobStateEnv {
                job_state_dir: Some(state_root.clone()),
                caller: Some(profile_namespace.clone()),
                localappdata: env::var_os("LOCALAPPDATA").map(PathBuf::from),
            })
            .map_err(|error| error.to_string())?,
        );
        store
            .recover_startup()
            .map_err(|error| format!("recover asynchronous jobs: {error}"))?;
        let escrow_dir = state_root.join("escrow").join(&profile_namespace);
        let handoff_dir = env::var_os("PARLEY_HANDOFF_STATE_DIR")
            .map(PathBuf::from)
            .map(|root| root.join(&profile_namespace));
        Ok(Self {
            inner: Some(Arc::new(RuntimeInner {
                store,
                active: Mutex::new(HashMap::new()),
                terminal: Mutex::new(HashMap::new()),
                threads: Mutex::new(Vec::new()),
                event_log: env::var_os("PARLEY_EVENT_LOG").map(PathBuf::from),
                escrow_dir,
                handoff_dir,
                handoff_lock: Mutex::new(()),
                max_concurrent,
            })),
        })
    }

    pub(crate) fn start(&self, mut spec: JobSpec) -> Result<JobLookup, String> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| "asynchronous jobs are disabled".to_string())?
            .clone();
        reap_finished(&inner);
        if spec.request.harness != "grok" && spec.request.harness != "gr" {
            return Err("start_agent_job supports only Grok".to_string());
        }
        validate_handoff_turn_request(spec.handoff_mode, spec.request.max_turns.as_deref())?;

        let (mut request, policy) = ask::prepare_for_job(&spec.request)?;
        if request.harness != "grok" {
            return Err("start_agent_job supports only Grok".to_string());
        }
        canonicalize_handoff_turn_limit(spec.handoff_mode, &mut request.max_turns);
        let existing = inner
            .store
            .get(&spec.job_id)
            .map_err(|error| error.to_string())?
            .is_some();
        spec.lane_plan = match (spec.mode, spec.lane_plan.take()) {
            (JobMode::Write, Some(plan)) if existing => {
                Some(lane_plan::normalize_existing(plan, &policy)?)
            }
            (JobMode::Write, Some(plan)) => {
                Some(lane_plan::validate(plan, spec.mode, &request.cwd, &policy)?)
            }
            (JobMode::Write, None) => return Err("locked write jobs require lane_plan".to_string()),
            (JobMode::Review | JobMode::Probe, Some(_)) => {
                return Err("lane_plan is only valid for job_mode=write".to_string())
            }
            (JobMode::Review | JobMode::Probe, None) => None,
        };
        let guarded_launch =
            GuardedSubagentLaunch::from_plan(&spec.job_id, spec.lane_plan.as_ref(), &policy)?;
        request.allow_subagents = guarded_launch
            .as_ref()
            .is_some_and(GuardedSubagentLaunch::has_children);
        if let Some(plan) = &spec.lane_plan {
            append_parent_contract(&mut request.prompt, plan, &spec.job_id);
        }
        spec.request = request;

        let session_key = spec
            .request
            .session_id
            .as_deref()
            .or(spec.request.resume_id.as_deref())
            .ok_or_else(|| "asynchronous jobs require session_id or resume_id".to_string())?
            .to_ascii_lowercase();
        let worktree_keys = writer_worktree_keys(&spec);
        let begin = inner
            .store
            .begin(&spec)
            .map_err(|error| error.to_string())?;
        if let BeginOutcome::Existing(view) = begin {
            return self.hydrate(view);
        }
        let job_id = begin.view().job_id.clone();
        let handoff = if spec.handoff_mode == HandoffMode::Required {
            let config = policy
                .handoff()
                .ok_or_else(|| "handoff_mode=required has no locked handoff policy".to_string())?;
            let root = inner.handoff_dir.as_ref().ok_or_else(|| {
                "handoff_mode=required has no handoff state directory".to_string()
            })?;
            let handoff = match HandoffSession::initialize(
                root,
                &job_id,
                spec.request
                    .context
                    .as_ref()
                    .filter(|context| context.harness == "codex")
                    .map(|context| context.session.as_str()),
                spec.request
                    .session_id
                    .as_deref()
                    .or(spec.request.resume_id.as_deref()),
                config,
                &inner.handoff_lock,
            ) {
                Ok(handoff) => handoff,
                Err(error) => {
                    let _ = inner.store.transition(
                        &job_id,
                        &terminal_transition(JobState::Failed, None, "handoff_preflight_failed"),
                    );
                    return Err(format!("handoff preflight failed: {error}"));
                }
            };
            append_handoff_contract(&mut spec.request.prompt, &job_id, &handoff, config);
            Some(handoff)
        } else {
            None
        };
        let cancellation = CancellationToken::default();
        {
            let mut active = inner
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let conflict = if active.len() >= inner.max_concurrent {
                Some("another asynchronous job is already active for this profile".to_string())
            } else if active.values().any(|job| job.session_key == session_key) {
                Some(format!(
                    "another asynchronous job already owns target session {session_key}"
                ))
            } else {
                active
                    .values()
                    .find_map(|job| {
                        job.worktree_keys
                            .iter()
                            .find(|key| worktree_keys.contains(*key))
                    })
                    .map(|overlap| {
                        format!("another asynchronous job already owns worktree {overlap}")
                    })
            };
            if let Some(error) = conflict {
                drop(active);
                let _ = inner.store.transition(
                    &job_id,
                    &terminal_transition(JobState::Failed, None, "concurrency_conflict"),
                );
                if let Some(handoff) = &handoff {
                    let _ = handoff.finish_without_success();
                }
                return Err(error);
            }
            active.insert(
                job_id.clone(),
                ActiveJob {
                    cancellation: cancellation.clone(),
                    session_key,
                    worktree_keys,
                },
            );
        }
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let worker_inner = Arc::clone(&inner);
        let worker_job_id = job_id.clone();
        let execution = WorkerExecution {
            policy,
            guarded_launch,
            handoff: handoff.clone(),
            cancellation,
        };
        let handle = thread::Builder::new()
            .name(format!("parley-job-{job_id}"))
            .spawn(move || run_worker(worker_inner, worker_job_id, spec, execution, started_tx))
            .map_err(|error| {
                inner
                    .active
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&job_id);
                let _ = inner.store.transition(
                    &job_id,
                    &terminal_transition(JobState::Failed, None, "worker_spawn_failed"),
                );
                if let Some(handoff) = &handoff {
                    let _ = handoff.finish_without_success();
                }
                format!("spawn asynchronous job worker: {error}")
            })?;
        inner
            .threads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(handle);

        match started_rx.recv() {
            Ok(StartSignal::Running | StartSignal::Terminal) => {
                let view = inner
                    .store
                    .get(&job_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("job {job_id} disappeared after start"))?;
                self.hydrate(view)
            }
            Ok(StartSignal::Failed(error)) => Err(error),
            Err(error) => Err(format!(
                "asynchronous job worker ended before reporting spawn outcome: {error}"
            )),
        }
    }

    pub(crate) fn get(&self, job_id: &str) -> Result<Option<JobLookup>, String> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| "asynchronous jobs are disabled".to_string())?;
        reap_finished(inner);
        inner
            .store
            .get(job_id)
            .map_err(|error| error.to_string())?
            .map(|view| self.hydrate(view))
            .transpose()
    }

    pub(crate) fn peek(&self, job_id: &str, since_ms: Option<u128>) -> Result<Json, String> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| "asynchronous jobs are disabled".to_string())?;
        reap_finished(inner);
        if inner
            .store
            .get(job_id)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err(format!("job {job_id} was not found"));
        }
        let root = inner
            .handoff_dir
            .as_ref()
            .ok_or_else(|| "peer activity is unavailable for this profile".to_string())?;
        let directory = root.join("jobs").join(job_id);
        if !directory.exists() {
            return Err(format!("job {job_id} has no acknowledgement activity"));
        }
        let record = load_handoff_readonly(&directory)?;
        Ok(peer_activity_json(&record, since_ms))
    }

    pub(crate) fn acknowledge(&self, job_id: &str, handoff_id: &str) -> Result<JobLookup, String> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| "asynchronous jobs are disabled".to_string())?;
        reap_finished(inner);
        let view = inner
            .store
            .get(job_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("job {job_id} was not found"))?;
        let root = inner
            .handoff_dir
            .as_ref()
            .ok_or_else(|| "acknowledged handoffs are unavailable for this profile".to_string())?;
        {
            let _guard = inner
                .handoff_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let directory = root.join("jobs").join(job_id);
            if !directory.exists() {
                return Err(format!("job {job_id} has no acknowledgement handoff"));
            }
            acknowledge_handoff_record(&directory, job_id, handoff_id)?;
        }
        self.hydrate(view)
    }

    pub(crate) fn list(&self, limit: usize) -> Result<Vec<JobView>, String> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| "asynchronous jobs are disabled".to_string())?;
        reap_finished(inner);
        if limit > MAX_LISTED_JOBS {
            return Err(format!("list limit may not exceed {MAX_LISTED_JOBS}"));
        }
        inner.store.list(limit).map_err(|error| error.to_string())
    }

    pub(crate) fn cancel(&self, job_id: &str) -> Result<JobLookup, String> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| "asynchronous jobs are disabled".to_string())?;
        reap_finished(inner);
        let view = inner
            .store
            .get(job_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("job {job_id} was not found"))?;
        if view.state.is_terminal() {
            return self.hydrate(view);
        }
        let cancellation = inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&view.job_id)
            .map(|job| job.cancellation.clone())
            .ok_or_else(|| format!("job {} has no live contained worker", view.job_id))?;
        let view = if view.state == JobState::Running {
            inner
                .store
                .transition(
                    &view.job_id,
                    &JobTransition {
                        state: JobState::Cancelling,
                        exchange_id: None,
                        session_id: None,
                        request_event_id: None,
                        completion_event_id: None,
                        error: None,
                    },
                )
                .map_err(|error| error.to_string())?
        } else if view.state == JobState::Cancelling {
            view
        } else {
            return Err(format!(
                "job {} cannot be cancelled from state {}",
                view.job_id,
                view.state.as_str()
            ));
        };
        cancellation.cancel();
        self.hydrate(view)
    }

    pub(crate) fn drain(&self) {
        let Some(inner) = &self.inner else {
            return;
        };
        let handles = std::mem::take(
            &mut *inner
                .threads
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for handle in handles {
            let _ = handle.join();
        }
    }

    fn hydrate(&self, mut view: JobView) -> Result<JobLookup, String> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| "asynchronous jobs are disabled".to_string())?;
        let memory = inner
            .terminal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&view.job_id)
            .cloned();
        if let Some(state) = memory.as_ref().and_then(|payload| payload.state_override) {
            view.state = state;
        }
        if !view.state.is_terminal() {
            let handoff = match &inner.handoff_dir {
                Some(root) => read_handoff_view(root, &view.job_id)?,
                None => None,
            };
            return Ok(JobLookup {
                view,
                delivery: None,
                handoff,
            });
        }
        let payload = match memory {
            Some(payload) => Some(payload),
            None => hydrate_from_event_log(&view, inner.event_log.as_deref())?
                .or_else(|| read_escrow(&inner.escrow_dir, &view.job_id).ok().flatten()),
        };
        let delivery = payload.as_ref().and_then(|payload| payload.receipt.clone());
        if let Some(payload) = payload {
            view.reply = payload.reply;
            if payload.error.is_some() {
                view.error = payload.error;
            }
        }
        let handoff = match &inner.handoff_dir {
            Some(root) => read_handoff_view(root, &view.job_id)?,
            None => None,
        };
        Ok(JobLookup {
            view,
            delivery,
            handoff,
        })
    }
}

fn validate_handoff_turn_request(
    handoff_mode: HandoffMode,
    max_turns: Option<&str>,
) -> Result<(), String> {
    if handoff_mode == HandoffMode::Required && max_turns.is_some() {
        return Err(
            "max_turns is not accepted for acknowledgement-required asynchronous jobs".to_string(),
        );
    }
    Ok(())
}

fn canonicalize_handoff_turn_limit(handoff_mode: HandoffMode, max_turns: &mut Option<String>) {
    if handoff_mode == HandoffMode::Required {
        *max_turns = None;
    }
}

struct WorkerExecution {
    policy: RuntimePolicy,
    guarded_launch: Option<GuardedSubagentLaunch>,
    handoff: Option<HandoffSession>,
    cancellation: CancellationToken,
}

fn run_worker(
    inner: Arc<RuntimeInner>,
    job_id: String,
    spec: JobSpec,
    execution: WorkerExecution,
    started_tx: mpsc::SyncSender<StartSignal>,
) {
    let WorkerExecution {
        policy,
        guarded_launch,
        handoff,
        cancellation,
    } = execution;
    let sent = Arc::new(AtomicBool::new(false));
    let observer = JobObserver {
        store: Arc::clone(&inner.store),
        job_id: job_id.clone(),
        require_lane_result: spec.mode == JobMode::Write && spec.lane_plan.is_some(),
        handoff: handoff.clone(),
        started_tx: started_tx.clone(),
        sent: Arc::clone(&sent),
    };
    let extra_paths = extra_writer_worktrees(&spec);
    let extra_lease = match ExecutionLease::acquire_worktrees(extra_paths, &job_id) {
        Ok(lease) => lease,
        Err(error) => {
            finish_pre_spawn_failure(
                &inner,
                &job_id,
                "worktree_lease_failed",
                error,
                &started_tx,
                &sent,
            );
            return;
        }
    };

    let result = ask::run_prepared_with_receipt_controlled(
        &spec.request,
        &policy,
        guarded_launch.as_ref(),
        &cancellation,
        &observer,
    );
    drop(extra_lease);
    match result {
        Ok(outcome) => {
            let reply = outcome.captured.reply();
            let cancelled = cancellation.is_cancelled() || outcome.captured.cancelled;
            let (state, error_code) = if cancelled {
                (JobState::Cancelled, Some("cancelled"))
            } else if outcome.captured.timed_out {
                (JobState::TimedOut, Some("watchdog_timeout"))
            } else if reply.is_ok() {
                (JobState::Succeeded, None)
            } else {
                (JobState::Failed, Some("agent_failed"))
            };
            let exact_reply = reply.as_ref().ok().cloned();
            let exact_error = reply.err();
            let transition = receipt_transition(state, &outcome.receipt, error_code);
            finish_terminal(
                &inner,
                &job_id,
                transition,
                TerminalPayload {
                    reply: exact_reply,
                    error: exact_error,
                    receipt: logged_response_receipt(&outcome.receipt),
                    state_override: None,
                },
                &started_tx,
                &sent,
            );
        }
        Err(failure) => {
            if let Some(handoff) = &handoff {
                let _ = handoff.finish_without_success();
            }
            finish_ask_failure(
                &inner,
                &job_id,
                failure,
                cancellation.is_cancelled(),
                &started_tx,
                &sent,
            )
        }
    }
}

struct JobObserver {
    store: Arc<JobStore>,
    job_id: String,
    require_lane_result: bool,
    handoff: Option<HandoffSession>,
    started_tx: mpsc::SyncSender<StartSignal>,
    sent: Arc<AtomicBool>,
}

impl AskSpawnObserver for JobObserver {
    fn configure_invocation(
        &self,
        invocation: &mut crate::harness::Invocation,
    ) -> Result<(), String> {
        match &self.handoff {
            Some(handoff) => handoff.configure_invocation(invocation),
            None => Ok(()),
        }
    }

    fn timeouts(&self, defaults: Timeouts) -> Timeouts {
        self.handoff
            .as_ref()
            .map(HandoffSession::timeouts)
            .unwrap_or(defaults)
    }

    fn temporary_file_max_age(&self, timeouts: Timeouts) -> Duration {
        self.handoff
            .as_ref()
            .map(HandoffSession::temporary_file_max_age)
            .unwrap_or_else(|| timeouts.overall.saturating_add(Duration::from_secs(300)))
    }

    fn process_created(&self, _receipt: &AskSpawnReceipt) -> Result<(), String> {
        match &self.handoff {
            Some(handoff) => handoff.process_created(_receipt.process_id),
            None => Ok(()),
        }
    }

    fn process_resumed(&self, receipt: &AskSpawnReceipt) -> Result<(), String> {
        if let Some(handoff) = &self.handoff {
            handoff.process_resumed(receipt.process_id)?;
        }
        self.store
            .transition(
                &self.job_id,
                &JobTransition {
                    state: JobState::Running,
                    exchange_id: Some(receipt.exchange_id.clone()),
                    session_id: receipt.session_id.clone(),
                    request_event_id: Some(receipt.request_event_id.clone()),
                    completion_event_id: None,
                    error: None,
                },
            )
            .map_err(|error| format!("persist running job state: {error}"))?;
        self.sent.store(true, Ordering::Release);
        self.started_tx
            .send(StartSignal::Running)
            .map_err(|error| format!("report running job state: {error}"))
    }

    fn validate_completed_reply(&self, reply: &str) -> Result<(), String> {
        if !self.require_lane_result {
            return Ok(());
        }
        validate_lane_result(reply, &self.job_id)
    }

    fn observe_output(&self, stream: OutputStream, chunk: &[u8]) -> Result<bool, String> {
        match &self.handoff {
            Some(handoff) => handoff.observe_output(stream, chunk),
            None => Ok(true),
        }
    }

    fn pause_execution_clock(&self) -> bool {
        self.handoff
            .as_ref()
            .is_some_and(HandoffSession::verified_wait_active)
    }

    fn timeout_reason(&self) -> Option<String> {
        self.handoff
            .as_ref()
            .and_then(HandoffSession::timeout_reason)
    }

    fn durable_reply_ready(&self) -> bool {
        self.handoff
            .as_ref()
            .is_some_and(HandoffSession::report_is_durable)
    }

    fn completed_reply(&self, captured: &Captured) -> Result<Option<String>, String> {
        match &self.handoff {
            Some(handoff) => handoff.complete(captured).map(Some),
            None => Ok(None),
        }
    }
}

#[derive(Clone)]
struct HandoffSession {
    state_dir: PathBuf,
    directory: PathBuf,
    job_id: String,
    handoff_id: String,
    capability: String,
    source_session_id: Option<String>,
    target_session_id: Option<String>,
    config: crate::policy::HandoffPolicy,
    stream: Arc<Mutex<HandoffStream>>,
}

#[derive(Default)]
struct HandoffStream {
    pending: Vec<u8>,
    first_line: bool,
    visible_tail: String,
    visible_dirty: bool,
    next_visible_starts_message: bool,
    visible_message_start: Option<usize>,
    saw_end: bool,
    parser_failed: Option<String>,
    last_persisted_activity: Option<ActivityRecord>,
    last_visible_persisted_ms: Option<u128>,
    pending_activities: Vec<ActivityRecord>,
    report_persisted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActivityRecord {
    class: String,
    timestamp_ms: u128,
    tool_name: Option<String>,
    status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredHandoff {
    sequence: u64,
    job_id: String,
    handoff_id: String,
    state: String,
    source_session_id: Option<String>,
    target_session_id: Option<String>,
    capability_hash: String,
    process_id: Option<u32>,
    process_state: String,
    phase: String,
    created_at_ms: u128,
    updated_at_ms: u128,
    last_activity_ms: Option<u128>,
    ready_at_ms: Option<u128>,
    deadline_ms: Option<u128>,
    receipt_at_ms: Option<u128>,
    wait_until_ms: Option<u128>,
    report_fingerprint: Option<String>,
    alert_incident_id: Option<String>,
    excerpt: String,
    excerpt_truncated: bool,
    activities: Vec<ActivityRecord>,
}

impl HandoffSession {
    fn initialize(
        root: &Path,
        job_id: &str,
        source_session_id: Option<&str>,
        target_session_id: Option<&str>,
        config: &crate::policy::HandoffPolicy,
        process_lock: &Mutex<()>,
    ) -> Result<Self, String> {
        let _guard = process_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let profile_parent = root
            .parent()
            .ok_or_else(|| "handoff profile state root has no parent".to_string())?;
        if path_key(profile_parent) != path_key(&config.state_dir) {
            return Err("handoff profile state root is outside its configured base".to_string());
        }
        let source_session_id = source_session_id.unwrap_or("unavailable").to_string();
        prepare_handoff_root(root)?;
        let directory = root.join("jobs").join(job_id);
        crate::context::refuse_reparse_chain(&directory).map_err(|error| error.to_string())?;
        fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "create handoff state directory {}: {error}",
                directory.display()
            )
        })?;
        crate::context::refuse_reparse_chain(&directory).map_err(|error| error.to_string())?;
        let _file_lock = HandoffFileLock::acquire(&directory)?;
        if snapshot_path(&directory).exists() || journal_path(&directory).exists() {
            return Err(format!("handoff state already exists for job {job_id}"));
        }
        let handoff_id = new_handoff_id(job_id);
        let capability = new_capability(job_id, &handoff_id);
        let now = now_ms();
        let record = StoredHandoff {
            sequence: 1,
            job_id: job_id.to_string(),
            handoff_id: handoff_id.clone(),
            state: "pending".to_string(),
            source_session_id: Some(source_session_id.clone()),
            target_session_id: target_session_id.map(str::to_string),
            capability_hash: digest_text(&capability),
            process_id: None,
            process_state: "preparing".to_string(),
            phase: "preparing".to_string(),
            created_at_ms: now,
            updated_at_ms: now,
            last_activity_ms: None,
            ready_at_ms: None,
            deadline_ms: None,
            receipt_at_ms: None,
            wait_until_ms: None,
            report_fingerprint: None,
            alert_incident_id: None,
            excerpt: String::new(),
            excerpt_truncated: false,
            activities: Vec::new(),
        };
        commit_handoff(&directory, &record)?;
        Ok(Self {
            state_dir: root.to_path_buf(),
            directory,
            job_id: job_id.to_string(),
            handoff_id,
            capability,
            source_session_id: Some(source_session_id),
            target_session_id: target_session_id.map(str::to_string),
            config: config.clone(),
            stream: Arc::new(Mutex::new(HandoffStream {
                first_line: true,
                ..HandoffStream::default()
            })),
        })
    }

    fn configure_invocation(
        &self,
        invocation: &mut crate::harness::Invocation,
    ) -> Result<(), String> {
        let output_positions = invocation
            .args
            .iter()
            .enumerate()
            .filter_map(|(index, argument)| (argument == "--output-format").then_some(index))
            .collect::<Vec<_>>();
        if output_positions.len() != 1 || output_positions[0] + 1 >= invocation.args.len() {
            return Err(
                "acknowledgement jobs require exactly one Grok --output-format".to_string(),
            );
        }
        invocation.args[output_positions[0] + 1] = "streaming-json".to_string();
        let values = [
            (
                "PARLEY_HANDOFF_STATE_DIR",
                self.state_dir.to_string_lossy().to_string(),
            ),
            (
                "PARLEY_HANDOFF_JOB_DIR",
                self.directory.to_string_lossy().to_string(),
            ),
            ("PARLEY_HANDOFF_JOB_ID", self.job_id.clone()),
            ("PARLEY_HANDOFF_ID", self.handoff_id.clone()),
            ("PARLEY_HANDOFF_CAPABILITY", self.capability.clone()),
            (
                "PARLEY_HANDOFF_SOURCE_SESSION_ID",
                self.source_session_id
                    .clone()
                    .unwrap_or_else(|| "unavailable".to_string()),
            ),
            (
                "PARLEY_HANDOFF_TARGET_SESSION_ID",
                self.target_session_id
                    .clone()
                    .unwrap_or_else(|| "unavailable".to_string()),
            ),
            (
                "PARLEY_HANDOFF_WAIT_SLICE_SEC",
                self.config.wait_slice_secs.to_string(),
            ),
            (
                "PARLEY_HANDOFF_ACK_CEILING_SEC",
                self.config.acknowledgement_ceiling_secs.to_string(),
            ),
            (
                "PARLEY_HANDOFF_PEER_PEEK_EXE",
                self.config.peer_peek_exe.to_string_lossy().to_string(),
            ),
            (
                "PARLEY_HANDOFF_WAIT_EXE",
                self.config.wait_exe.to_string_lossy().to_string(),
            ),
            (
                "PARLEY_HANDOFF_ALERT_EXE",
                self.config.alert_exe.to_string_lossy().to_string(),
            ),
            (
                "PARLEY_HANDOFF_HOOK_EXE",
                self.config.hook_exe.to_string_lossy().to_string(),
            ),
        ];
        for (key, value) in values {
            invocation.env.insert(key.to_string(), value);
        }
        Ok(())
    }

    fn timeouts(&self) -> Timeouts {
        Timeouts {
            overall: Duration::from_secs(self.config.execution_ceiling_secs),
            idle: Duration::ZERO,
        }
    }

    fn temporary_file_max_age(&self) -> Duration {
        Duration::from_secs(
            self.config
                .execution_ceiling_secs
                .saturating_add(self.config.acknowledgement_ceiling_secs)
                .saturating_add(self.config.wait_slice_secs)
                .saturating_add(300),
        )
    }

    fn process_created(&self, process_id: u32) -> Result<(), String> {
        self.update(|record| {
            record.process_id = Some(process_id);
            record.process_state = "created_suspended".to_string();
            record.phase = "starting".to_string();
            Ok(())
        })
    }

    fn process_resumed(&self, process_id: u32) -> Result<(), String> {
        self.update(|record| {
            if record.process_id != Some(process_id) {
                return Err("handoff process identity changed before resume".to_string());
            }
            record.process_state = "alive".to_string();
            record.phase = "running".to_string();
            Ok(())
        })
    }

    fn observe_output(&self, stream: OutputStream, chunk: &[u8]) -> Result<bool, String> {
        if stream == OutputStream::Stderr {
            if !chunk.is_empty() {
                let mut stream_state = self
                    .stream
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let activity = ActivityRecord {
                    class: "error".to_string(),
                    timestamp_ms: now_ms(),
                    tool_name: None,
                    status: Some("stderr_output".to_string()),
                };
                enqueue_activity(&mut stream_state, activity.clone());
                if activity_persistence_due(&stream_state, &activity, false, false) {
                    let persist_visible = stream_state.visible_dirty;
                    self.persist_pending_activity(&mut stream_state, persist_visible)?;
                }
            }
            return Ok(false);
        }
        let mut stream_state = self
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(error) = &stream_state.parser_failed {
            return Err(error.clone());
        }
        stream_state.pending.extend_from_slice(chunk);
        if stream_state.pending.len() > MAX_STREAM_LINE_BYTES
            && !stream_state.pending.contains(&b'\n')
        {
            let error = format!("Grok streaming-json record exceeds {MAX_STREAM_LINE_BYTES} bytes");
            stream_state.parser_failed = Some(error.clone());
            return Err(error);
        }
        while let Some(newline) = stream_state.pending.iter().position(|byte| *byte == b'\n') {
            let mut line = stream_state.pending.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if stream_state.first_line {
                stream_state.first_line = false;
                if line.starts_with(&[0xef, 0xbb, 0xbf]) {
                    line.drain(..3);
                }
            }
            if line.len() > MAX_STREAM_LINE_BYTES {
                let error =
                    format!("Grok streaming-json record exceeds {MAX_STREAM_LINE_BYTES} bytes");
                stream_state.parser_failed = Some(error.clone());
                return Err(error);
            }
            let text = match std::str::from_utf8(&line) {
                Ok(text) => text,
                Err(_) => {
                    let error = "Grok streaming-json record is not UTF-8".to_string();
                    stream_state.parser_failed = Some(error.clone());
                    return Err(error);
                }
            };
            let timestamp = now_ms();
            let activity = match decode_stream_line(text, timestamp) {
                Ok(activity) => activity,
                Err(error) => {
                    stream_state.parser_failed = Some(error.clone());
                    return Err(error);
                }
            };
            if let Some(activity) = activity {
                if let Err(error) = self.accept_activity(&mut stream_state, activity) {
                    stream_state.parser_failed = Some(error.clone());
                    return Err(error);
                }
            }
        }
        Ok(false)
    }

    fn accept_activity(
        &self,
        stream: &mut HandoffStream,
        activity: SafeActivity,
    ) -> Result<(), String> {
        if stream.saw_end {
            return Err("Grok streaming-json emitted a record after terminal end".to_string());
        }
        if activity.class == ActivityClass::End {
            stream.saw_end = true;
        }
        if matches!(
            activity.class,
            ActivityClass::ToolCall | ActivityClass::ToolCallUpdate
        ) && !stream.visible_tail.is_empty()
        {
            stream.next_visible_starts_message = true;
        }
        let has_visible_text = activity.visible_text.is_some();
        if let Some(text) = activity.visible_text.as_deref() {
            let previous_len = stream.visible_tail.len();
            let removed = append_visible_tail(&mut stream.visible_tail, text);
            stream.visible_message_start = stream
                .visible_message_start
                .and_then(|start| start.checked_sub(removed));
            if stream.next_visible_starts_message {
                stream.visible_message_start = previous_len.checked_sub(removed);
                stream.next_visible_starts_message = false;
            }
            stream.visible_dirty = true;
        }
        let record = ActivityRecord {
            class: activity_class_name(activity.class).to_string(),
            timestamp_ms: activity.timestamp_ms,
            tool_name: activity
                .tool_name
                .as_deref()
                .map(|value| sanitize_label(value, 80)),
            status: activity
                .status
                .as_deref()
                .map(|value| sanitize_label(value, 120)),
        };
        enqueue_activity(stream, record.clone());
        let extracted_report = if has_visible_text && !stream.report_persisted {
            extract_handoff_report(
                &stream.visible_tail,
                &self.job_id,
                &self.handoff_id,
                stream.visible_message_start,
            )?
        } else {
            if has_visible_text {
                reject_duplicate_handoff_start(
                    &stream.visible_tail,
                    &self.job_id,
                    &self.handoff_id,
                )?;
            }
            None
        };
        let report = if stream.report_persisted {
            None
        } else {
            extracted_report
        };
        let force = activity.class == ActivityClass::End || report.is_some();
        if activity_persistence_due(stream, &record, has_visible_text, force) {
            let persist_visible = stream.visible_dirty || force;
            self.persist_pending_activity(stream, persist_visible)?;
        }
        if let Some(report) = report {
            self.persist_report(&report)?;
            stream.report_persisted = true;
        }
        Ok(())
    }

    fn persist_pending_activity(
        &self,
        stream: &mut HandoffStream,
        persist_visible: bool,
    ) -> Result<(), String> {
        if stream.pending_activities.is_empty() {
            return Ok(());
        }
        let activities = stream.pending_activities.clone();
        self.record_activities(
            &activities,
            persist_visible.then_some(stream.visible_tail.as_str()),
        )?;
        let latest = activities
            .last()
            .cloned()
            .ok_or_else(|| "peer activity batch unexpectedly became empty".to_string())?;
        stream.pending_activities.clear();
        note_persisted_activity(stream, latest, persist_visible);
        if persist_visible {
            stream.visible_dirty = false;
        }
        Ok(())
    }

    fn record_activities(
        &self,
        activities: &[ActivityRecord],
        visible_tail: Option<&str>,
    ) -> Result<(), String> {
        let latest = activities
            .last()
            .ok_or_else(|| "peer activity batch is empty".to_string())?;
        self.update(|record| {
            record.last_activity_ms = Some(latest.timestamp_ms);
            record.phase = phase_for_activity(&latest.class).to_string();
            record.activities.extend_from_slice(activities);
            if record.activities.len() > MAX_ACTIVITY_EVENTS {
                let excess = record.activities.len() - MAX_ACTIVITY_EVENTS;
                record.activities.drain(..excess);
            }
            if let Some(visible) = visible_tail {
                let (excerpt, truncated) = tail_scalars(visible, self.config.excerpt_chars);
                record.excerpt = excerpt;
                record.excerpt_truncated = truncated;
            }
            Ok(())
        })
    }

    fn persist_report(&self, report: &str) -> Result<(), String> {
        if report.chars().count() > MAX_HANDOFF_REPORT_CHARS {
            return Err(format!(
                "handoff report exceeds {MAX_HANDOFF_REPORT_CHARS} characters"
            ));
        }
        let fingerprint = digest_text(report);
        let path = report_path(&self.directory);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(report.as_bytes())
                    .and_then(|_| file.flush())
                    .and_then(|_| file.sync_all())
                    .map_err(|error| format!("write handoff report {}: {error}", path.display()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = read_bounded_report(&path)?;
                if existing != report {
                    return Err(
                        "handoff report evidence already exists with different content".to_string(),
                    );
                }
            }
            Err(error) => return Err(format!("create handoff report {}: {error}", path.display())),
        }
        self.update(|record| {
            if let Some(existing) = &record.report_fingerprint {
                if existing != &fingerprint {
                    return Err("handoff report fingerprint changed after durability".to_string());
                }
                return Ok(());
            }
            let ready_at = now_ms();
            record.state = "ready".to_string();
            record.report_fingerprint = Some(fingerprint.clone());
            record.ready_at_ms = Some(ready_at);
            record.deadline_ms = Some(
                ready_at
                    .saturating_add(u128::from(self.config.acknowledgement_ceiling_secs) * 1_000),
            );
            record.phase = "handoff_ready".to_string();
            Ok(())
        })?;
        self.update(|record| {
            if record.state == "ready" {
                record.state = "awaiting_ack".to_string();
                record.phase = "awaiting_ack".to_string();
            }
            Ok(())
        })
    }

    fn report_is_durable(&self) -> bool {
        self.read().is_ok_and(|record| {
            record.report_fingerprint.is_some() && report_path(&self.directory).is_file()
        })
    }

    fn verified_wait_active(&self) -> bool {
        let Ok(record) = self.read() else {
            return false;
        };
        let now = now_ms();
        let maximum = now.saturating_add(u128::from(self.config.wait_slice_secs + 5) * 1_000);
        matches!(record.state.as_str(), "ready" | "awaiting_ack")
            && record.process_state == "alive"
            && record
                .wait_until_ms
                .is_some_and(|until| until > now && until <= maximum)
    }

    fn timeout_reason(&self) -> Option<String> {
        let record = self.read().ok()?;
        if !matches!(record.state.as_str(), "ready" | "awaiting_ack") {
            return None;
        }
        let deadline = record.deadline_ms?;
        let grace = u128::from(self.config.wait_slice_secs) * 1_000;
        (now_ms() >= deadline.saturating_add(grace)).then(|| {
            "handoff acknowledgement ceiling and final decision window expired".to_string()
        })
    }

    fn complete(&self, captured: &Captured) -> Result<String, String> {
        let stream = self
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let incomplete = !stream.pending.is_empty();
        let saw_end = stream.saw_end;
        let parser_error = stream.parser_failed.clone();
        drop(stream);
        let record = self.read()?;
        let report = record
            .report_fingerprint
            .as_ref()
            .map(|_| read_bounded_report(&report_path(&self.directory)))
            .transpose()?;
        let acknowledged = record.receipt_at_ms.is_some() && record.state == "acknowledged";
        if captured.success && parser_error.is_none() && !incomplete && saw_end && acknowledged {
            if let Some(report) = report {
                self.update(|record| {
                    record.process_state = "exited".to_string();
                    record.phase = "completed".to_string();
                    record.wait_until_ms = None;
                    Ok(())
                })?;
                return Ok(report);
            }
        }
        self.finish_without_success()?;
        if let Some(error) = parser_error {
            return Err(format!("streaming-json parser failed: {error}"));
        }
        if incomplete {
            return Err("Grok streaming-json ended with an incomplete record".to_string());
        }
        if !saw_end {
            return Err("Grok streaming-json omitted the terminal end record".to_string());
        }
        if report.is_none() {
            return Err("Grok exited without a durable marked handoff report".to_string());
        }
        if !acknowledged {
            return Err("Grok exited before Codex acknowledged the durable handoff".to_string());
        }
        Err(captured.failure_message())
    }

    fn finish_without_success(&self) -> Result<(), String> {
        self.update(|record| {
            record.process_state = "exited".to_string();
            record.wait_until_ms = None;
            if record.receipt_at_ms.is_some() {
                record.state = "catastrophe_exit".to_string();
                record.phase = "catastrophe_exit".to_string();
            } else if record.report_fingerprint.is_some() {
                record.state = "detached_unacked".to_string();
                record.phase = "detached_unacked".to_string();
            } else {
                record.state = "catastrophe_exit".to_string();
                record.phase = "catastrophe_exit".to_string();
            }
            Ok(())
        })
    }

    fn read(&self) -> Result<StoredHandoff, String> {
        let _lock = HandoffFileLock::acquire(&self.directory)?;
        load_handoff(&self.directory)
    }

    fn update(
        &self,
        mutate: impl FnOnce(&mut StoredHandoff) -> Result<(), String>,
    ) -> Result<(), String> {
        let _lock = HandoffFileLock::acquire(&self.directory)?;
        let mut record = load_handoff(&self.directory)?;
        if record.job_id != self.job_id || record.handoff_id != self.handoff_id {
            return Err("handoff state identity changed".to_string());
        }
        let before = handoff_json(&record).to_compact_string();
        mutate(&mut record)?;
        let after = handoff_json(&record).to_compact_string();
        if before == after {
            return Ok(());
        }
        record.sequence = record.sequence.saturating_add(1);
        record.updated_at_ms = now_ms();
        commit_handoff(&self.directory, &record)
    }
}

fn append_handoff_contract(
    prompt: &mut String,
    job_id: &str,
    handoff: &HandoffSession,
    config: &crate::policy::HandoffPolicy,
) {
    prompt.push_str("\n\nTWO CHAIRS ACKNOWLEDGED HANDOFF CONTRACT\n");
    prompt.push_str("This asynchronous write/review turn does not complete until its durable report is explicitly received by Codex. Receipt means delivery only, never approval, acceptance, recovery, or continuity authority.\n");
    prompt.push_str("Hidden reasoning and raw tool payloads are never shared. If you need current Codex activity evidence, invoke exactly this zero-argument read-only command:\n");
    prompt.push_str(&config.peer_peek_command);
    prompt.push('\n');
    prompt.push_str("When all bounded work and your self-contained report are complete, emit exactly one block with these collision-resistant markers:\n");
    prompt.push_str(&format!(
        "=== TWO_CHAIRS_HANDOFF_READY_V1 job={job_id} handoff={} ===\n<self-contained exact report>\n=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff={} ===\n",
        handoff.handoff_id, handoff.handoff_id
    ));
    prompt.push_str("The durable report inside those markers must itself contain every other locked response requirement, including the complete TWO_CHAIRS_HANDOFF footer when required. Put that footer before the end marker, and emit no visible report text outside the markers.\n");
    prompt.push_str("After emitting the complete block, remain in this same turn and invoke exactly the following zero-argument wait command. It waits at most five minutes and reports receipt/deadline evidence; repeat it only while receipt remains pending:\n");
    prompt.push_str(&config.wait_command);
    prompt.push('\n');
    prompt.push_str("You may explicitly request one sparse user alert for this handoff by invoking exactly this zero-argument command, but elapsed time, silence, hook failure, or process exit never authorizes an automatic alert:\n");
    prompt.push_str(&config.alert_command);
    prompt.push('\n');
    prompt.push_str("Do not end the turn before receipt unless the runtime reports catastrophe or forces detachment. A Stop hook is only a thin gate; Parley runtime state is authoritative.\n");
}

fn activity_class_name(class: ActivityClass) -> &'static str {
    match class {
        ActivityClass::Thought => "thought",
        ActivityClass::ToolCall => "tool_call",
        ActivityClass::ToolCallUpdate => "tool_call_update",
        ActivityClass::Text => "text",
        ActivityClass::Usage => "usage",
        ActivityClass::Plan => "plan",
        ActivityClass::AvailableCommands => "available_commands",
        ActivityClass::End => "end",
        ActivityClass::Error => "error",
        ActivityClass::Unknown => "unknown",
    }
}

fn activity_persistence_due(
    stream: &HandoffStream,
    activity: &ActivityRecord,
    has_visible_text: bool,
    force: bool,
) -> bool {
    if force {
        return true;
    }
    let Some(previous) = &stream.last_persisted_activity else {
        return true;
    };
    activity.timestamp_ms.saturating_sub(previous.timestamp_ms) >= ACTIVITY_FLUSH_INTERVAL_MS
        || (has_visible_text && stream.last_visible_persisted_ms.is_none())
}

fn enqueue_activity(stream: &mut HandoffStream, activity: ActivityRecord) {
    if let Some(previous) = stream.pending_activities.last_mut() {
        if previous.class == activity.class
            && previous.tool_name == activity.tool_name
            && previous.status == activity.status
        {
            previous.timestamp_ms = activity.timestamp_ms;
            return;
        }
    }
    stream.pending_activities.push(activity);
    if stream.pending_activities.len() > MAX_ACTIVITY_EVENTS {
        let excess = stream.pending_activities.len() - MAX_ACTIVITY_EVENTS;
        stream.pending_activities.drain(..excess);
    }
}

fn note_persisted_activity(
    stream: &mut HandoffStream,
    activity: ActivityRecord,
    persisted_visible: bool,
) {
    if persisted_visible {
        stream.last_visible_persisted_ms = Some(activity.timestamp_ms);
    }
    stream.last_persisted_activity = Some(activity);
}

fn phase_for_activity(class: &str) -> &'static str {
    match class {
        "thought" => "reasoning",
        "tool_call" | "tool_call_update" => "tool_use",
        "text" => "visible_output",
        "plan" => "planning",
        "usage" => "usage",
        "end" => "ending",
        "error" => "error",
        _ => "active",
    }
}

fn append_visible_tail(buffer: &mut String, text: &str) -> usize {
    buffer.push_str(text);
    let maximum = MAX_HANDOFF_REPORT_CHARS.saturating_mul(2);
    let count = buffer.chars().count();
    if count <= maximum {
        return 0;
    }
    let keep_from = buffer
        .char_indices()
        .nth(count - maximum)
        .map(|(index, _)| index)
        .unwrap_or(0);
    buffer.drain(..keep_from);
    keep_from
}

fn extract_handoff_report(
    visible: &str,
    job_id: &str,
    handoff_id: &str,
    accepted_start_boundary: Option<usize>,
) -> Result<Option<String>, String> {
    let start_marker =
        format!("=== TWO_CHAIRS_HANDOFF_READY_V1 job={job_id} handoff={handoff_id} ===");
    let end_marker = format!("=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff={handoff_id} ===");
    let starts = visible.match_indices(&start_marker).collect::<Vec<_>>();
    if starts.len() > 1 {
        return Err("Grok emitted more than one matching handoff start marker".to_string());
    }
    let Some((start, _)) = starts.first().copied() else {
        return Ok(None);
    };
    if start > 0
        && visible.as_bytes().get(start - 1) != Some(&b'\n')
        && accepted_start_boundary != Some(start)
    {
        return Err("handoff start marker is not on its own line".to_string());
    }
    let body_start = start + start_marker.len();
    let remainder = &visible[body_start..];
    let separator_len = if remainder.starts_with("\r\n") {
        2
    } else if remainder.starts_with('\n') {
        1
    } else if remainder.is_empty() || remainder == "\r" {
        return Ok(None);
    } else {
        return Err("handoff start marker is not newline terminated".to_string());
    };
    let search_start = body_start + separator_len;
    let Some(relative_end) = visible[search_start..].find(&format!("\n{end_marker}")) else {
        if visible[search_start..].chars().count() > MAX_HANDOFF_REPORT_CHARS {
            return Err(format!(
                "handoff report exceeds {MAX_HANDOFF_REPORT_CHARS} characters"
            ));
        }
        return Ok(None);
    };
    let end_start = search_start + relative_end + 1;
    let end = end_start + end_marker.len();
    if visible[end..].starts_with(|character: char| character != '\n' && character != '\r') {
        return Err("handoff end marker is not line terminated".to_string());
    }
    let report = visible[start..end].to_string();
    if report.chars().count() > MAX_HANDOFF_REPORT_CHARS {
        return Err(format!(
            "handoff report exceeds {MAX_HANDOFF_REPORT_CHARS} characters"
        ));
    }
    Ok(Some(report))
}

fn reject_duplicate_handoff_start(
    visible: &str,
    job_id: &str,
    handoff_id: &str,
) -> Result<(), String> {
    let start_marker =
        format!("=== TWO_CHAIRS_HANDOFF_READY_V1 job={job_id} handoff={handoff_id} ===");
    if visible.match_indices(&start_marker).count() > 1 {
        return Err("Grok emitted more than one matching handoff start marker".to_string());
    }
    Ok(())
}

fn tail_scalars(value: &str, maximum: usize) -> (String, bool) {
    let count = value.chars().count();
    if count <= maximum {
        return (value.to_string(), false);
    }
    let start = value
        .char_indices()
        .nth(count - maximum)
        .map(|(index, _)| index)
        .unwrap_or(0);
    (value[start..].to_string(), true)
}

fn sanitize_label(value: &str, maximum: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(maximum)
        .collect()
}

fn prepare_handoff_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err("handoff state root must be absolute".to_string());
    }
    crate::context::refuse_reparse_chain(root).map_err(|error| error.to_string())?;
    fs::create_dir_all(root.join("jobs"))
        .map_err(|error| format!("create handoff state root {}: {error}", root.display()))?;
    crate::context::refuse_reparse_chain(root).map_err(|error| error.to_string())
}

fn snapshot_path(directory: &Path) -> PathBuf {
    directory.join("snapshot.json")
}

fn journal_path(directory: &Path) -> PathBuf {
    directory.join("journal.jsonl")
}

fn report_path(directory: &Path) -> PathBuf {
    directory.join("report.txt")
}

fn commit_handoff(directory: &Path, record: &StoredHandoff) -> Result<(), String> {
    let encoded = handoff_json(record).to_compact_string();
    if encoded.len() > MAX_HANDOFF_JOURNAL_LINE_BYTES || encoded.contains(['\n', '\r']) {
        return Err("handoff transition is not one bounded JSON line".to_string());
    }
    let journal = journal_path(directory);
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    apply_handoff_share_all(&mut options);
    let mut file = options
        .open(&journal)
        .map_err(|error| format!("open handoff journal {}: {error}", journal.display()))?;
    clip_incomplete_handoff_journal(&mut file, &journal)?;
    let projected = file
        .metadata()
        .map_err(|error| format!("inspect handoff journal {}: {error}", journal.display()))?
        .len()
        .saturating_add(encoded.len() as u64)
        .saturating_add(1);
    if projected > MAX_HANDOFF_JOURNAL_BYTES as u64 {
        return Err(format!(
            "handoff journal {} exceeds its {} byte bound",
            journal.display(),
            MAX_HANDOFF_JOURNAL_BYTES
        ));
    }
    file.seek(SeekFrom::End(0))
        .map_err(|error| format!("seek handoff journal {}: {error}", journal.display()))?;
    file.write_all(encoded.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("append handoff journal {}: {error}", journal.display()))?;
    publish_handoff_snapshot(directory, &encoded)
}

fn publish_handoff_snapshot(directory: &Path, encoded: &str) -> Result<(), String> {
    let temporary = directory.join(format!(
        ".snapshot.{}.{}.tmp",
        std::process::id(),
        HANDOFF_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut snapshot = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("create handoff snapshot {}: {error}", temporary.display()))?;
    snapshot
        .write_all(encoded.as_bytes())
        .and_then(|_| snapshot.write_all(b"\n"))
        .and_then(|_| snapshot.flush())
        .and_then(|_| snapshot.sync_all())
        .map_err(|error| format!("write handoff snapshot {}: {error}", temporary.display()))?;
    drop(snapshot);
    let published = fs::rename(&temporary, snapshot_path(directory)).map_err(|error| {
        format!(
            "publish handoff snapshot {}: {error}",
            snapshot_path(directory).display()
        )
    });
    if published.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    published
}

fn clip_incomplete_handoff_journal(file: &mut File, path: &Path) -> Result<(), String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("seek handoff journal {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("read handoff journal {}: {error}", path.display()))?;
    if bytes.len() > MAX_HANDOFF_JOURNAL_BYTES {
        return Err(format!(
            "handoff journal {} exceeds its {} byte bound",
            path.display(),
            MAX_HANDOFF_JOURNAL_BYTES
        ));
    }
    let keep = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    if keep < bytes.len() {
        file.set_len(keep as u64)
            .map_err(|error| format!("trim handoff journal {}: {error}", path.display()))?;
    }
    Ok(())
}

fn load_handoff(directory: &Path) -> Result<StoredHandoff, String> {
    let snapshot = snapshot_path(directory);
    let latest = load_handoff_readonly(directory)?;
    let snapshot_matches = fs::read(&snapshot)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| parse_handoff(text.trim()).ok())
        .is_some_and(|record| record == latest);
    if !snapshot_matches {
        publish_handoff_snapshot(directory, &handoff_json(&latest).to_compact_string())?;
    }
    Ok(latest)
}

fn load_handoff_readonly(directory: &Path) -> Result<StoredHandoff, String> {
    let journal = journal_path(directory);
    let bytes = read_handoff_shared(&journal, MAX_HANDOFF_JOURNAL_BYTES)?;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| &bytes[..=index])
        .unwrap_or_default();
    let text =
        std::str::from_utf8(complete).map_err(|_| "handoff journal is not UTF-8".to_string())?;
    let mut latest: Option<StoredHandoff> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.len() > MAX_HANDOFF_JOURNAL_LINE_BYTES {
            return Err("handoff journal contains an oversized transition".to_string());
        }
        let record = parse_handoff(line)?;
        if let Some(previous) = &latest {
            if record.sequence != previous.sequence.saturating_add(1)
                || record.job_id != previous.job_id
                || record.handoff_id != previous.handoff_id
            {
                return Err(
                    "handoff journal transition sequence or identity is invalid".to_string()
                );
            }
        } else if record.sequence != 1 {
            return Err("handoff journal must begin at sequence 1".to_string());
        }
        latest = Some(record);
    }
    latest.ok_or_else(|| "handoff journal contains no complete records".to_string())
}

fn read_handoff_shared(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_handoff_share_all(&mut options);
    let mut file = options
        .open(path)
        .map_err(|error| format!("read handoff journal {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = file
            .read(&mut chunk)
            .map_err(|error| format!("read handoff journal {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) > limit {
            return Err(format!(
                "handoff journal {} exceeds its {} byte bound",
                path.display(),
                limit
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(bytes)
}

fn apply_handoff_share_all(options: &mut OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004);
    }
    #[cfg(not(windows))]
    let _ = options;
}

fn acknowledge_handoff_record(
    directory: &Path,
    job_id: &str,
    handoff_id: &str,
) -> Result<StoredHandoff, String> {
    let _lock = HandoffFileLock::acquire(directory)?;
    let mut record = load_handoff(directory)?;
    if record.job_id != job_id {
        return Err("job_id does not match the durable handoff".to_string());
    }
    if record.handoff_id != handoff_id {
        return Err("handoff_id does not match the job's durable handoff".to_string());
    }
    match record.state.as_str() {
        "acknowledged" => Ok(record),
        "ready" | "awaiting_ack" | "detached_unacked" => {
            record.state = "acknowledged".to_string();
            record.receipt_at_ms = Some(now_ms());
            record.updated_at_ms = now_ms();
            record.sequence = record.sequence.saturating_add(1);
            record.phase = if record.process_state == "exited" {
                "detached_received".to_string()
            } else {
                "acknowledged".to_string()
            };
            record.wait_until_ms = None;
            commit_handoff(directory, &record)?;
            Ok(record)
        }
        "pending" => Err("handoff report is not ready for receipt".to_string()),
        "catastrophe_exit" => Err("catastrophe exit produced no receivable handoff".to_string()),
        state => Err(format!("handoff cannot be acknowledged from state {state}")),
    }
}

fn handoff_json(record: &StoredHandoff) -> Json {
    let mut fields = BTreeMap::new();
    fields.insert(
        "schema_version".to_string(),
        Json::Number(HANDOFF_SCHEMA_VERSION as f64),
    );
    fields.insert("sequence".to_string(), Json::Number(record.sequence as f64));
    fields.insert("job_id".to_string(), Json::Str(record.job_id.clone()));
    fields.insert(
        "handoff_id".to_string(),
        Json::Str(record.handoff_id.clone()),
    );
    fields.insert("state".to_string(), Json::Str(record.state.clone()));
    fields.insert(
        "source_session_id".to_string(),
        optional_json(&record.source_session_id),
    );
    fields.insert(
        "target_session_id".to_string(),
        optional_json(&record.target_session_id),
    );
    fields.insert(
        "capability_hash".to_string(),
        Json::Str(record.capability_hash.clone()),
    );
    fields.insert(
        "process_id".to_string(),
        record
            .process_id
            .map(|value| Json::Number(value as f64))
            .unwrap_or(Json::Null),
    );
    fields.insert(
        "process_state".to_string(),
        Json::Str(record.process_state.clone()),
    );
    fields.insert("phase".to_string(), Json::Str(record.phase.clone()));
    fields.insert(
        "created_at_ms".to_string(),
        Json::Number(record.created_at_ms as f64),
    );
    fields.insert(
        "updated_at_ms".to_string(),
        Json::Number(record.updated_at_ms as f64),
    );
    for (name, value) in [
        ("last_activity_ms", record.last_activity_ms),
        ("ready_at_ms", record.ready_at_ms),
        ("deadline_ms", record.deadline_ms),
        ("receipt_at_ms", record.receipt_at_ms),
        ("wait_until_ms", record.wait_until_ms),
    ] {
        fields.insert(
            name.to_string(),
            value
                .map(|value| Json::Number(value as f64))
                .unwrap_or(Json::Null),
        );
    }
    fields.insert(
        "report_fingerprint".to_string(),
        optional_json(&record.report_fingerprint),
    );
    fields.insert(
        "alert_incident_id".to_string(),
        optional_json(&record.alert_incident_id),
    );
    fields.insert("excerpt".to_string(), Json::Str(record.excerpt.clone()));
    fields.insert(
        "excerpt_truncated".to_string(),
        Json::Bool(record.excerpt_truncated),
    );
    fields.insert(
        "activities".to_string(),
        Json::Array(
            record
                .activities
                .iter()
                .map(|activity| {
                    let mut fields = BTreeMap::new();
                    fields.insert("class".to_string(), Json::Str(activity.class.clone()));
                    fields.insert(
                        "timestamp_ms".to_string(),
                        Json::Number(activity.timestamp_ms as f64),
                    );
                    fields.insert("tool_name".to_string(), optional_json(&activity.tool_name));
                    fields.insert("status".to_string(), optional_json(&activity.status));
                    Json::Object(fields)
                })
                .collect(),
        ),
    );
    Json::Object(fields)
}

fn parse_handoff(text: &str) -> Result<StoredHandoff, String> {
    let value = Json::parse(text).map_err(|error| format!("parse handoff state: {error}"))?;
    if value.get("schema_version").and_then(Json::as_number) != Some(HANDOFF_SCHEMA_VERSION as f64)
    {
        return Err("handoff state has an unsupported schema version".to_string());
    }
    let activities = value
        .get("activities")
        .and_then(Json::as_array)
        .unwrap_or(&[])
        .iter()
        .map(|activity| {
            Ok(ActivityRecord {
                class: required_handoff_string(activity, "class")?,
                timestamp_ms: required_handoff_number(activity, "timestamp_ms")?,
                tool_name: optional_handoff_string(activity, "tool_name")?,
                status: optional_handoff_string(activity, "status")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if activities.len() > MAX_ACTIVITY_EVENTS {
        return Err("handoff activity history exceeds its bound".to_string());
    }
    let state = required_handoff_string(&value, "state")?;
    if !matches!(
        state.as_str(),
        "pending"
            | "ready"
            | "awaiting_ack"
            | "acknowledged"
            | "detached_unacked"
            | "catastrophe_exit"
    ) {
        return Err("handoff state value is invalid".to_string());
    }
    Ok(StoredHandoff {
        sequence: required_handoff_number(&value, "sequence")? as u64,
        job_id: required_handoff_string(&value, "job_id")?,
        handoff_id: required_handoff_string(&value, "handoff_id")?,
        state,
        source_session_id: optional_handoff_string(&value, "source_session_id")?,
        target_session_id: optional_handoff_string(&value, "target_session_id")?,
        capability_hash: required_handoff_string(&value, "capability_hash")?,
        process_id: optional_handoff_number(&value, "process_id")?.map(|value| value as u32),
        process_state: required_handoff_string(&value, "process_state")?,
        phase: required_handoff_string(&value, "phase")?,
        created_at_ms: required_handoff_number(&value, "created_at_ms")?,
        updated_at_ms: required_handoff_number(&value, "updated_at_ms")?,
        last_activity_ms: optional_handoff_number(&value, "last_activity_ms")?,
        ready_at_ms: optional_handoff_number(&value, "ready_at_ms")?,
        deadline_ms: optional_handoff_number(&value, "deadline_ms")?,
        receipt_at_ms: optional_handoff_number(&value, "receipt_at_ms")?,
        wait_until_ms: optional_handoff_number(&value, "wait_until_ms")?,
        report_fingerprint: optional_handoff_string(&value, "report_fingerprint")?,
        alert_incident_id: optional_handoff_string(&value, "alert_incident_id")?,
        excerpt: required_handoff_string(&value, "excerpt")?,
        excerpt_truncated: value
            .get("excerpt_truncated")
            .and_then(Json::as_bool)
            .ok_or_else(|| "handoff excerpt_truncated is missing".to_string())?,
        activities,
    })
}

fn required_handoff_string(value: &Json, name: &str) -> Result<String, String> {
    value
        .get(name)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("handoff {name} is missing"))
}

fn optional_handoff_string(value: &Json, name: &str) -> Result<Option<String>, String> {
    match value.get(name) {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_string()))
            .ok_or_else(|| format!("handoff {name} is not a string or null")),
    }
}

fn required_handoff_number(value: &Json, name: &str) -> Result<u128, String> {
    value
        .get(name)
        .and_then(Json::as_number)
        .filter(|value| value.is_finite() && *value >= 0.0 && value.fract() == 0.0)
        .map(|value| value as u128)
        .ok_or_else(|| format!("handoff {name} is missing or invalid"))
}

fn optional_handoff_number(value: &Json, name: &str) -> Result<Option<u128>, String> {
    match value.get(name) {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value
            .as_number()
            .filter(|value| value.is_finite() && *value >= 0.0 && value.fract() == 0.0)
            .map(|value| Some(value as u128))
            .ok_or_else(|| format!("handoff {name} is not an integer or null")),
    }
}

fn read_bounded_report(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("read handoff report {}: {error}", path.display()))?;
    let report = String::from_utf8(bytes)
        .map_err(|_| format!("handoff report {} is not UTF-8", path.display()))?;
    if report.chars().count() > MAX_HANDOFF_REPORT_CHARS {
        return Err(format!(
            "handoff report {} exceeds {MAX_HANDOFF_REPORT_CHARS} characters",
            path.display()
        ));
    }
    Ok(report)
}

fn read_handoff_view(root: &Path, job_id: &str) -> Result<Option<HandoffView>, String> {
    let directory = root.join("jobs").join(job_id);
    if !directory.exists() {
        return Ok(None);
    }
    let record = load_handoff_readonly(&directory)?;
    let report = record
        .report_fingerprint
        .as_ref()
        .map(|_| read_bounded_report(&report_path(&directory)))
        .transpose()?;
    Ok(Some(HandoffView {
        handoff_id: record.handoff_id,
        state: record.state,
        report,
        report_fingerprint: record.report_fingerprint,
        ready_at_ms: record.ready_at_ms,
        deadline_ms: record.deadline_ms,
        receipt_at_ms: record.receipt_at_ms,
        alert_incident_id: record.alert_incident_id,
    }))
}

struct HandoffFileLock {
    _file: File,
}

impl HandoffFileLock {
    fn acquire(directory: &Path) -> Result<Self, String> {
        let path = directory.join("lock");
        let mut last_error = None;
        for _ in 0..100 {
            match open_handoff_lock(&path) {
                Ok(file) => return Ok(Self { _file: file }),
                Err(error) if is_handoff_lock_busy(&error) => {
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(format!("open handoff lock {}: {error}", path.display())),
            }
        }
        Err(format!(
            "handoff state is locked: {}: {}",
            path.display(),
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "timeout".to_string())
        ))
    }
}

#[cfg(windows)]
fn open_handoff_lock(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
}

#[cfg(not(windows))]
fn open_handoff_lock(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

#[cfg(windows)]
fn is_handoff_lock_busy(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32) | Some(33))
}

#[cfg(not(windows))]
fn is_handoff_lock_busy(_error: &std::io::Error) -> bool {
    false
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn digest_text(value: &str) -> String {
    hex_encode(&sha256(value.as_bytes()))
}

fn new_handoff_id(job_id: &str) -> String {
    let seed = format!(
        "handoff\0{job_id}\0{}\0{}\0{}",
        now_ms(),
        std::process::id(),
        HANDOFF_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let mut bytes = sha256(seed.as_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

fn new_capability(job_id: &str, handoff_id: &str) -> String {
    digest_text(&format!(
        "capability\0{job_id}\0{handoff_id}\0{}\0{}\0{}",
        now_ms(),
        std::process::id(),
        HANDOFF_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn validate_lane_result(reply: &str, job_id: &str) -> Result<(), String> {
    let normalized = reply.replace("\r\n", "\n");
    let expected_job = format!("job_id: {job_id}");
    let lines = normalized.lines().collect::<Vec<_>>();
    if lines.windows(3).any(|window| {
        window[0] == "TWO_CHAIRS_LANE_RESULT"
            && window[1] == expected_job
            && window[2] == "status: completed"
    }) {
        Ok(())
    } else {
        Err(format!(
            "write job reply omitted the exact completed lane result for job {job_id}"
        ))
    }
}

fn finish_pre_spawn_failure(
    inner: &Arc<RuntimeInner>,
    job_id: &str,
    error_code: &str,
    exact_error: String,
    started_tx: &mpsc::SyncSender<StartSignal>,
    sent: &AtomicBool,
) {
    let transition = terminal_transition(JobState::Failed, None, error_code);
    finish_terminal(
        inner,
        job_id,
        transition,
        TerminalPayload {
            reply: None,
            error: Some(exact_error),
            receipt: None,
            state_override: None,
        },
        started_tx,
        sent,
    );
}

fn finish_ask_failure(
    inner: &Arc<RuntimeInner>,
    job_id: &str,
    failure: AskFailure,
    cancellation_requested: bool,
    started_tx: &mpsc::SyncSender<StartSignal>,
    sent: &AtomicBool,
) {
    let receipt = failure.receipt.as_deref().cloned();
    let state = if cancellation_requested || failure.cancelled {
        JobState::Cancelled
    } else if failure.timed_out {
        JobState::TimedOut
    } else {
        JobState::Failed
    };
    let code = if cancellation_requested || failure.cancelled {
        "cancelled"
    } else if failure.timed_out {
        "timed_out"
    } else if failure.captured_reply.is_some() && receipt.is_none() {
        "completion_logging_failed"
    } else if failure.captured_reply.is_some() {
        "post_completion_failed"
    } else {
        "preflight_or_execution_failed"
    };
    let transition = match &receipt {
        Some(receipt) => receipt_transition(state, receipt, Some(code)),
        None => terminal_transition(state, None, code),
    };
    let payload = TerminalPayload {
        reply: failure.captured_reply.clone(),
        error: Some(failure.message.clone()),
        receipt: receipt.as_ref().and_then(logged_response_receipt),
        state_override: None,
    };
    if failure.captured_reply.is_some() && receipt.is_none() {
        let _ = write_escrow(&inner.escrow_dir, job_id, &payload, None);
    }
    finish_terminal(inner, job_id, transition, payload, started_tx, sent);
}

fn reap_finished(inner: &RuntimeInner) {
    let finished = {
        let mut threads = inner
            .threads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut running = Vec::with_capacity(threads.len());
        let mut finished = Vec::new();
        for handle in threads.drain(..) {
            if handle.is_finished() {
                finished.push(handle);
            } else {
                running.push(handle);
            }
        }
        *threads = running;
        finished
    };
    for handle in finished {
        let _ = handle.join();
    }
}

fn finish_terminal(
    inner: &Arc<RuntimeInner>,
    job_id: &str,
    transition: JobTransition,
    mut payload: TerminalPayload,
    started_tx: &mpsc::SyncSender<StartSignal>,
    sent: &AtomicBool,
) {
    let transition_result = inner
        .store
        .transition(job_id, &transition)
        .or_else(|error| {
            let cancelling = inner
                .store
                .get(job_id)
                .ok()
                .flatten()
                .is_some_and(|view| view.state == JobState::Cancelling);
            if !cancelling {
                return Err(error);
            }
            inner.store.transition(
                job_id,
                &JobTransition {
                    state: JobState::Cancelled,
                    exchange_id: transition.exchange_id.clone(),
                    session_id: transition.session_id.clone(),
                    request_event_id: transition.request_event_id.clone(),
                    completion_event_id: transition.completion_event_id.clone(),
                    error: Some("cancelled".to_string()),
                },
            )
        });
    if let Err(error) = transition_result {
        let captured_error = payload.error.take();
        payload.state_override = Some(JobState::Failed);
        payload.error = Some(format!(
            "terminal job state persistence failed; do not retry automatically: {error}{}",
            captured_error
                .as_deref()
                .map(|exact| format!("; captured error: {exact}"))
                .unwrap_or_default()
        ));
        let escrow_error = write_escrow(
            &inner.escrow_dir,
            job_id,
            &payload,
            Some(&error.to_string()),
        )
        .err()
        .map(|escrow| format!("; escrow also failed: {escrow}"))
        .unwrap_or_default();
        inner
            .terminal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(job_id.to_string(), payload);
        if !sent.swap(true, Ordering::AcqRel) {
            let _ = started_tx.send(StartSignal::Failed(format!(
                "persist terminal job state failed: {error}{escrow_error}"
            )));
        }
    } else {
        inner
            .terminal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(job_id.to_string(), payload);
        if !sent.swap(true, Ordering::AcqRel) {
            let _ = started_tx.send(StartSignal::Terminal);
        }
    }
    inner
        .active
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(job_id);
}

fn receipt_transition(
    state: JobState,
    receipt: &ExchangeReceipt,
    error: Option<&str>,
) -> JobTransition {
    JobTransition {
        state,
        exchange_id: Some(receipt.request.exchange_id.clone()),
        session_id: receipt.request.session_id.clone(),
        request_event_id: Some(receipt.request.event_id.clone()),
        completion_event_id: Some(receipt.completion.event_id.clone()),
        error: error.map(str::to_string),
    }
}

fn terminal_transition(
    state: JobState,
    completion_event_id: Option<String>,
    error: &str,
) -> JobTransition {
    JobTransition {
        state,
        exchange_id: None,
        session_id: None,
        request_event_id: None,
        completion_event_id,
        error: Some(error.to_string()),
    }
}

fn logged_response_receipt(receipt: &ExchangeReceipt) -> Option<ExchangeReceipt> {
    (receipt.completion.logged
        && receipt.completion.event_type == "response"
        && receipt.target == "grok")
        .then(|| receipt.clone())
}

fn writer_worktree_keys(spec: &JobSpec) -> Vec<String> {
    let mut keys = spec
        .lane_plan
        .as_ref()
        .map(|plan| {
            plan.lanes
                .iter()
                .filter(|lane| lane.role == LaneRole::Writer)
                .map(|lane| path_key(&lane.cwd))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![path_key(&spec.request.cwd)]);
    keys.sort();
    keys.dedup();
    keys
}

fn extra_writer_worktrees(spec: &JobSpec) -> Vec<PathBuf> {
    let parent = path_key(&spec.request.cwd);
    spec.lane_plan
        .as_ref()
        .map(|plan| {
            plan.lanes
                .iter()
                .filter(|lane| {
                    lane.role == LaneRole::Writer
                        && lane.owner != LaneOwner::GrokParent
                        && path_key(&lane.cwd) != parent
                })
                .map(|lane| lane.cwd.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn hydrate_from_event_log(
    view: &JobView,
    path: Option<&Path>,
) -> Result<Option<TerminalPayload>, String> {
    let (Some(path), Some(completion_id)) = (path, view.completion_event_id.as_deref()) else {
        return Ok(None);
    };
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read event log {}: {error}", path.display())),
    };
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| &bytes[..=index])
        .unwrap_or_default();
    let text = std::str::from_utf8(complete)
        .map_err(|_| format!("event log {} is not UTF-8", path.display()))?;
    let mut request = None;
    let mut completion = None;
    let mut completion_value = None;
    for line in text.lines() {
        let Ok(value) = Json::parse(line.trim_end_matches('\r')) else {
            continue;
        };
        let event_id = value.get("event_id").and_then(Json::as_str);
        if event_id == view.request_event_id.as_deref() {
            request = event_receipt(&value);
        }
        if event_id == Some(completion_id) {
            completion = event_receipt(&value);
            completion_value = Some(value);
        }
    }
    let Some(value) = completion_value else {
        return Ok(None);
    };
    let event_type = value.get("event_type").and_then(Json::as_str).unwrap_or("");
    let reply = (event_type == "response")
        .then(|| {
            value
                .get("content")
                .and_then(Json::as_str)
                .map(str::to_string)
        })
        .flatten();
    let error = (event_type == "error")
        .then(|| {
            value
                .get("error")
                .and_then(Json::as_str)
                .map(str::to_string)
        })
        .flatten();
    let receipt = match (request, completion) {
        (Some(request), Some(completion)) if event_type == "response" => Some(ExchangeReceipt {
            request,
            completion,
            target: value
                .get("target")
                .and_then(Json::as_str)
                .unwrap_or("grok")
                .to_string(),
        }),
        _ => None,
    };
    Ok(Some(TerminalPayload {
        reply,
        error,
        receipt,
        state_override: None,
    }))
}

fn event_receipt(value: &Json) -> Option<EventReceipt> {
    Some(EventReceipt {
        event_id: value.get("event_id")?.as_str()?.to_string(),
        exchange_id: value.get("exchange_id")?.as_str()?.to_string(),
        timestamp_ms: value.get("timestamp_ms")?.as_number()? as u128,
        event_type: value.get("event_type")?.as_str()?.to_string(),
        session_id: value
            .get("session_id")
            .and_then(Json::as_str)
            .map(str::to_string),
        logged: true,
    })
}

fn write_escrow(
    directory: &Path,
    job_id: &str,
    payload: &TerminalPayload,
    state_error: Option<&str>,
) -> Result<(), String> {
    refuse_reparse_chain(directory).map_err(|error| error.to_string())?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("create escrow directory {}: {error}", directory.display()))?;
    refuse_reparse_chain(directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("job-{job_id}.json"));
    let mut fields = BTreeMap::new();
    fields.insert("schema_version".to_string(), Json::Number(1.0));
    fields.insert("job_id".to_string(), Json::Str(job_id.to_string()));
    fields.insert(
        "reply".to_string(),
        payload.reply.clone().map(Json::Str).unwrap_or(Json::Null),
    );
    fields.insert(
        "error".to_string(),
        payload.error.clone().map(Json::Str).unwrap_or(Json::Null),
    );
    fields.insert(
        "state_error".to_string(),
        state_error
            .map(|value| Json::Str(value.to_string()))
            .unwrap_or(Json::Null),
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("create diagnostic escrow {}: {error}", path.display()))?;
    let mut encoded = Json::Object(fields).to_compact_string().into_bytes();
    encoded.push(b'\n');
    file.write_all(&encoded)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("write diagnostic escrow {}: {error}", path.display()))
}

fn read_escrow(directory: &Path, job_id: &str) -> Result<Option<TerminalPayload>, String> {
    let path = directory.join(format!("job-{job_id}.json"));
    let mut text = String::new();
    match File::open(&path) {
        Ok(mut file) => {
            file.read_to_string(&mut text)
                .map_err(|error| format!("read diagnostic escrow {}: {error}", path.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "open diagnostic escrow {}: {error}",
                path.display()
            ))
        }
    }
    let value = Json::parse(text.trim())
        .map_err(|error| format!("parse diagnostic escrow {}: {error}", path.display()))?;
    if value.get("schema_version").and_then(Json::as_number) != Some(1.0)
        || value.get("job_id").and_then(Json::as_str) != Some(job_id)
    {
        return Err(format!(
            "diagnostic escrow {} has invalid identity",
            path.display()
        ));
    }
    Ok(Some(TerminalPayload {
        reply: value
            .get("reply")
            .and_then(Json::as_str)
            .map(str::to_string),
        error: value
            .get("error")
            .and_then(Json::as_str)
            .map(str::to_string),
        receipt: None,
        state_override: None,
    }))
}

pub(crate) fn view_json(view: &JobView, include_payload: bool) -> Json {
    let mut fields = BTreeMap::new();
    fields.insert("job_id".to_string(), Json::Str(view.job_id.clone()));
    fields.insert(
        "state".to_string(),
        Json::Str(view.state.as_str().to_string()),
    );
    fields.insert("exchange_id".to_string(), optional_json(&view.exchange_id));
    fields.insert("session_id".to_string(), optional_json(&view.session_id));
    fields.insert(
        "request_event_id".to_string(),
        optional_json(&view.request_event_id),
    );
    fields.insert(
        "completion_event_id".to_string(),
        optional_json(&view.completion_event_id),
    );
    fields.insert(
        "created_at_ms".to_string(),
        Json::Number(view.created_at_ms as f64),
    );
    fields.insert(
        "updated_at_ms".to_string(),
        Json::Number(view.updated_at_ms as f64),
    );
    fields.insert("error".to_string(), optional_json(&view.error));
    if include_payload {
        fields.insert("reply".to_string(), optional_json(&view.reply));
    }
    Json::Object(fields)
}

pub(crate) fn lookup_json(lookup: &JobLookup, include_payload: bool) -> Json {
    let mut value = view_json(&lookup.view, include_payload);
    if let Some(fields) = value.as_object().cloned() {
        let mut fields = fields;
        fields.insert(
            "handoff".to_string(),
            lookup
                .handoff
                .as_ref()
                .map(handoff_view_json)
                .unwrap_or(Json::Null),
        );
        value = Json::Object(fields);
    }
    value
}

fn handoff_view_json(view: &HandoffView) -> Json {
    let mut fields = BTreeMap::new();
    fields.insert("handoff_id".to_string(), Json::Str(view.handoff_id.clone()));
    fields.insert("state".to_string(), Json::Str(view.state.clone()));
    fields.insert("report".to_string(), optional_json(&view.report));
    fields.insert(
        "fingerprint".to_string(),
        optional_json(&view.report_fingerprint),
    );
    fields.insert(
        "ready_at_ms".to_string(),
        optional_number_json(view.ready_at_ms),
    );
    fields.insert(
        "deadline_ms".to_string(),
        optional_number_json(view.deadline_ms),
    );
    fields.insert(
        "receipt_at_ms".to_string(),
        optional_number_json(view.receipt_at_ms),
    );
    fields.insert(
        "receipt_state".to_string(),
        Json::Str(if view.receipt_at_ms.is_some() {
            "received".to_string()
        } else {
            "pending".to_string()
        }),
    );
    fields.insert(
        "alert_incident_id".to_string(),
        optional_json(&view.alert_incident_id),
    );
    Json::Object(fields)
}

fn peer_activity_json(record: &StoredHandoff, since_ms: Option<u128>) -> Json {
    let as_of = now_ms();
    let mut fields = BTreeMap::new();
    fields.insert(
        "schema_version".to_string(),
        Json::Number(PEER_ACTIVITY_SCHEMA_VERSION as f64),
    );
    fields.insert("as_of_ms".to_string(), Json::Number(as_of as f64));
    fields.insert("job_id".to_string(), Json::Str(record.job_id.clone()));
    fields.insert(
        "handoff_id".to_string(),
        Json::Str(record.handoff_id.clone()),
    );
    fields.insert("phase".to_string(), Json::Str(record.phase.clone()));
    fields.insert(
        "process_state".to_string(),
        Json::Str(record.process_state.clone()),
    );
    fields.insert(
        "last_activity_ms".to_string(),
        optional_number_json(record.last_activity_ms),
    );
    fields.insert(
        "age_ms".to_string(),
        optional_number_json(
            record
                .last_activity_ms
                .map(|value| as_of.saturating_sub(value)),
        ),
    );
    fields.insert(
        "freshness".to_string(),
        Json::Str(if record.last_activity_ms.is_some() {
            "observed".to_string()
        } else {
            "no_activity_observed".to_string()
        }),
    );
    fields.insert(
        "assessment".to_string(),
        Json::Str("not_inferred".to_string()),
    );
    let mut excerpt = BTreeMap::new();
    excerpt.insert(
        "available".to_string(),
        Json::Bool(!record.excerpt.is_empty()),
    );
    excerpt.insert("text".to_string(), Json::Str(record.excerpt.clone()));
    excerpt.insert(
        "truncated".to_string(),
        Json::Bool(record.excerpt_truncated),
    );
    excerpt.insert(
        "fingerprint".to_string(),
        Json::Str(if record.excerpt.is_empty() {
            String::new()
        } else {
            digest_text(&record.excerpt)
        }),
    );
    fields.insert("excerpt".to_string(), Json::Object(excerpt));
    fields.insert(
        "events".to_string(),
        Json::Array(
            record
                .activities
                .iter()
                .filter(|activity| since_ms.is_none_or(|since| activity.timestamp_ms >= since))
                .map(|activity| {
                    let mut fields = BTreeMap::new();
                    fields.insert("class".to_string(), Json::Str(activity.class.clone()));
                    fields.insert(
                        "timestamp_ms".to_string(),
                        Json::Number(activity.timestamp_ms as f64),
                    );
                    fields.insert("tool_name".to_string(), optional_json(&activity.tool_name));
                    fields.insert("status".to_string(), optional_json(&activity.status));
                    Json::Object(fields)
                })
                .collect(),
        ),
    );
    Json::Object(fields)
}

fn optional_number_json(value: Option<u128>) -> Json {
    value
        .map(|value| Json::Number(value as f64))
        .unwrap_or(Json::Null)
}

fn optional_json(value: &Option<String>) -> Json {
    value.clone().map(Json::Str).unwrap_or(Json::Null)
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase()
}

fn env_bool(name: &str) -> Result<Option<bool>, String> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(error) => return Err(format!("read {name}: {error}")),
    };
    let parsed = match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => return Err(format!("{name} must be true or false, got {value}")),
    };
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn temp(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "parley-job-runtime-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn test_handoff_policy(root: &Path) -> crate::policy::HandoffPolicy {
        let executable = root.join("helper.exe");
        crate::policy::HandoffPolicy {
            state_dir: root.to_path_buf(),
            peer_peek_exe: executable.clone(),
            wait_exe: executable.clone(),
            alert_exe: executable.clone(),
            hook_exe: executable,
            peer_peek_command: "peer-peek".to_string(),
            wait_command: "handoff-wait".to_string(),
            alert_command: "peer-alert".to_string(),
            wait_slice_secs: 300,
            execution_ceiling_secs: 21_600,
            acknowledgement_ceiling_secs: 21_600,
            excerpt_chars: 4_096,
        }
    }

    #[test]
    fn acknowledgement_jobs_reject_caller_caps_and_clear_locked_defaults() {
        assert!(validate_handoff_turn_request(HandoffMode::Required, Some("12")).is_err());
        assert!(validate_handoff_turn_request(HandoffMode::Required, None).is_ok());

        let mut turns = Some("30".to_string());
        canonicalize_handoff_turn_limit(HandoffMode::Required, &mut turns);
        assert_eq!(turns, None);

        let mut probe_turns = Some("30".to_string());
        canonicalize_handoff_turn_limit(HandoffMode::None, &mut probe_turns);
        assert_eq!(probe_turns.as_deref(), Some("30"));
    }

    fn test_handoff_session(name: &str) -> HandoffSession {
        let base = temp(name);
        let root = base.join("profile-test");
        let policy = test_handoff_policy(&base);
        HandoffSession::initialize(
            &root,
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            Some("codex-session"),
            Some("grok-session"),
            &policy,
            &Mutex::new(()),
        )
        .unwrap()
    }

    #[test]
    fn acknowledgement_jobs_protect_temporary_files_for_full_wall_lifetime() {
        let session = test_handoff_session("temporary-file-lifetime");
        assert_eq!(
            session.temporary_file_max_age(),
            Duration::from_secs(43_800)
        );
    }

    #[test]
    fn acknowledgement_jobs_without_context_bind_an_explicit_unavailable_source() {
        let base = temp("unavailable-source-session");
        let root = base.join("profile-test");
        let policy = test_handoff_policy(&base);
        let session = HandoffSession::initialize(
            &root,
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            None,
            Some("grok-session"),
            &policy,
            &Mutex::new(()),
        )
        .unwrap();
        assert_eq!(session.source_session_id.as_deref(), Some("unavailable"));
        assert_eq!(
            session.read().unwrap().source_session_id.as_deref(),
            Some("unavailable")
        );
    }

    #[test]
    fn handoff_helpers_receive_the_profile_namespaced_state_root() {
        let base = temp("profile-state-root");
        let profile = base.join("profile-test");
        let policy = test_handoff_policy(&base);
        let session = HandoffSession::initialize(
            &profile,
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            Some("codex-session"),
            Some("grok-session"),
            &policy,
            &Mutex::new(()),
        )
        .unwrap();
        let mut invocation = crate::harness::Invocation::new(
            "grok",
            vec!["--output-format".to_string(), "plain".to_string()],
        );
        session.configure_invocation(&mut invocation).unwrap();

        assert_eq!(
            invocation.env.get("PARLEY_HANDOFF_STATE_DIR"),
            Some(&profile.to_string_lossy().to_string())
        );
        assert_eq!(
            invocation.env.get("PARLEY_HANDOFF_JOB_DIR"),
            Some(&session.directory.to_string_lossy().to_string())
        );
    }

    #[test]
    fn handoff_contract_keeps_locked_response_requirements_inside_markers() {
        let session = test_handoff_session("nested-response-contract");
        let policy = session.config.clone();
        let mut prompt = String::new();
        append_handoff_contract(&mut prompt, &session.job_id, &session, &policy);

        assert!(prompt.contains("including the complete TWO_CHAIRS_HANDOFF footer when required"));
        assert!(prompt.contains("Put that footer before the end marker"));
        assert!(prompt.contains("emit no visible report text outside the markers"));
    }

    fn text_stream_record(text: &str) -> String {
        format!(
            "{{\"type\":\"text\",\"data\":{}}}",
            Json::Str(text.to_string()).to_compact_string()
        )
    }

    #[test]
    fn escrow_is_create_new_and_round_trips_exact_payload() {
        let directory = temp("escrow");
        let payload = TerminalPayload {
            reply: Some("exact\nreply".to_string()),
            error: Some("non-retriable logging failure".to_string()),
            receipt: None,
            state_override: None,
        };
        let job_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        write_escrow(&directory, job_id, &payload, None).unwrap();
        assert!(write_escrow(&directory, job_id, &payload, None).is_err());
        let loaded = read_escrow(&directory, job_id).unwrap().unwrap();
        assert_eq!(loaded.reply, payload.reply);
        assert_eq!(loaded.error, payload.error);
    }

    #[test]
    fn event_hydration_ignores_incomplete_tail_and_returns_delivery_receipt() {
        let directory = temp("events");
        let path = directory.join("events.jsonl");
        let request = r#"{"schema_version":1,"event_type":"request","event_id":"request-1","exchange_id":"exchange-1","timestamp_ms":10,"source":"codex","target":"grok","cwd":"C:\\lane","session_id":"session-1","session_action":"new","content":"task","status":"started","duration_ms":null,"error":null}"#;
        let response = r#"{"schema_version":1,"event_type":"response","event_id":"response-1","exchange_id":"exchange-1","timestamp_ms":20,"source":"codex","target":"grok","cwd":"C:\\lane","session_id":"session-1","session_action":"new","content":"exact reply","status":"ok","duration_ms":10,"error":null}"#;
        fs::write(&path, format!("{request}\n{response}\n{{\"partial\":")).unwrap();
        let view = JobView {
            job_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            state: JobState::Succeeded,
            exchange_id: Some("exchange-1".to_string()),
            session_id: Some("session-1".to_string()),
            request_event_id: Some("request-1".to_string()),
            completion_event_id: Some("response-1".to_string()),
            error: None,
            reply: None,
            created_at_ms: 10,
            updated_at_ms: 20,
        };
        let payload = hydrate_from_event_log(&view, Some(&path)).unwrap().unwrap();
        assert_eq!(payload.reply.as_deref(), Some("exact reply"));
        assert!(payload.receipt.is_some());
    }

    #[test]
    fn list_json_omits_terminal_payload() {
        let view = JobView {
            job_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            state: JobState::Succeeded,
            exchange_id: None,
            session_id: None,
            request_event_id: None,
            completion_event_id: None,
            error: None,
            reply: Some("secret reply".to_string()),
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        let json = view_json(&view, false).to_compact_string();
        assert!(!json.contains("reply"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn lane_result_validation_rejects_partial_or_mismatched_replies() {
        let job_id = "32f0a181-6822-416c-b2c0-1a5fd4062d9f";
        let valid = format!(
            "report\n\nTWO_CHAIRS_LANE_RESULT\r\njob_id: {job_id}\r\nstatus: completed\r\n\r\nTWO_CHAIRS_HANDOFF"
        );
        assert!(validate_lane_result(&valid, job_id).is_ok());
        assert!(validate_lane_result("I will write the file now.", job_id).is_err());
        assert!(validate_lane_result(
            "TWO_CHAIRS_LANE_RESULT\njob_id: other\nstatus: completed",
            job_id
        )
        .is_err());
        assert!(validate_lane_result(
            &format!("TWO_CHAIRS_LANE_RESULT\njob_id: {job_id}\nstatus: completed_with_errors"),
            job_id
        )
        .is_err());
    }

    #[test]
    fn report_extraction_is_exact_bounded_and_collision_resistant() {
        let job_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let handoff_id = "11111111-2222-4333-8444-555555555555";
        let start =
            format!("=== TWO_CHAIRS_HANDOFF_READY_V1 job={job_id} handoff={handoff_id} ===");
        let end = format!("=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff={handoff_id} ===");
        assert_eq!(
            extract_handoff_report(&start, job_id, handoff_id, None).unwrap(),
            None
        );
        assert_eq!(
            extract_handoff_report(&format!("{start}\r"), job_id, handoff_id, None).unwrap(),
            None
        );
        assert!(
            extract_handoff_report(&format!("{start}x"), job_id, handoff_id, None)
                .unwrap_err()
                .contains("newline terminated")
        );
        let exact = format!("{start}\nexact report 🌍\n{end}");
        assert_eq!(
            extract_handoff_report(&format!("commentary\n{exact}\n"), job_id, handoff_id, None,)
                .unwrap()
                .as_deref(),
            Some(exact.as_str())
        );
        assert!(
            extract_handoff_report(&format!("{exact}\n{exact}\n"), job_id, handoff_id, None,)
                .unwrap_err()
                .contains("more than one")
        );
        assert!(extract_handoff_report(
            &format!("prefix{start}\nreport\n{end}"),
            job_id,
            handoff_id,
            None,
        )
        .unwrap_err()
        .contains("own line"));
        let oversized = format!("{start}\n{}\n{end}", "x".repeat(MAX_HANDOFF_REPORT_CHARS));
        assert!(extract_handoff_report(&oversized, job_id, handoff_id, None)
            .unwrap_err()
            .contains("exceeds"));
        let crlf = format!("{start}\r\nexact report\r\n{end}");
        assert_eq!(
            extract_handoff_report(&crlf, job_id, handoff_id, None)
                .unwrap()
                .as_deref(),
            Some(crlf.as_str())
        );
    }

    #[test]
    fn report_start_accepts_a_distinct_visible_message_boundary_only() {
        let session = test_handoff_session("visible-message-boundary");
        let report = format!(
            "=== TWO_CHAIRS_HANDOFF_READY_V1 job={} handoff={} ===\nreport\n=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff={} ===",
            session.job_id, session.handoff_id, session.handoff_id
        );
        let mut stream = session
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        session
            .accept_activity(
                &mut stream,
                SafeActivity {
                    class: ActivityClass::Text,
                    timestamp_ms: 1_000,
                    tool_name: None,
                    status: None,
                    visible_text: Some("pre-tool commentary".to_string()),
                },
            )
            .unwrap();
        session
            .accept_activity(
                &mut stream,
                SafeActivity {
                    class: ActivityClass::ToolCallUpdate,
                    timestamp_ms: 1_001,
                    tool_name: None,
                    status: Some("completed".to_string()),
                    visible_text: None,
                },
            )
            .unwrap();
        session
            .accept_activity(
                &mut stream,
                SafeActivity {
                    class: ActivityClass::Text,
                    timestamp_ms: 1_002,
                    tool_name: None,
                    status: None,
                    visible_text: Some(report[..1].to_string()),
                },
            )
            .unwrap();
        session
            .accept_activity(
                &mut stream,
                SafeActivity {
                    class: ActivityClass::Text,
                    timestamp_ms: 1_003,
                    tool_name: None,
                    status: None,
                    visible_text: Some(report[1..].to_string()),
                },
            )
            .unwrap();
        assert!(stream.report_persisted);
        drop(stream);
        assert_eq!(
            read_bounded_report(&report_path(&session.directory)).unwrap(),
            report
        );

        let embedded = format!("pre-tool commentary{report}");
        assert!(
            extract_handoff_report(&embedded, &session.job_id, &session.handoff_id, None,)
                .unwrap_err()
                .contains("own line")
        );
    }

    #[test]
    fn fragmented_stream_persists_only_safe_activity_and_exact_report() {
        let session = test_handoff_session("fragmented-stream");
        session.process_created(42).unwrap();
        session.process_resumed(42).unwrap();
        let start = format!(
            "=== TWO_CHAIRS_HANDOFF_READY_V1 job={} handoff={} ===",
            session.job_id, session.handoff_id
        );
        let end = format!(
            "=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff={} ===",
            session.handoff_id
        );
        let exact = format!("{start}\ncompleted 猫 report\n{end}");
        let visible = format!("visible commentary 🌍\n{exact}\n");
        let payload = format!(
            "\u{feff}{{\"type\":\"thought\",\"data\":\"SECRET_REASONING\"}}\r\n{{\"type\":\"tool_call\",\"toolName\":\"read_file\",\"status\":\"in_progress\",\"rawInput\":{{\"path\":\"SECRET_PATH\"}},\"content\":\"SECRET_OUTPUT\"}}\r\n{}\r\n{{\"type\":\"end\",\"stopReason\":\"end_turn\"}}\r\n",
            text_stream_record(&visible)
        );
        for chunk in payload.as_bytes().chunks(7) {
            assert!(!session.observe_output(OutputStream::Stdout, chunk).unwrap());
        }
        let record = session.read().unwrap();
        assert_eq!(record.state, "awaiting_ack");
        assert_eq!(
            read_bounded_report(&report_path(&session.directory)).unwrap(),
            exact
        );
        let persisted = format!(
            "{}\n{}",
            fs::read_to_string(snapshot_path(&session.directory)).unwrap(),
            fs::read_to_string(journal_path(&session.directory)).unwrap()
        );
        for forbidden in ["SECRET_REASONING", "SECRET_PATH", "SECRET_OUTPUT"] {
            assert!(!persisted.contains(forbidden));
        }
        let activity = peer_activity_json(&record, None).to_compact_string();
        assert!(activity.contains("visible commentary"));
        assert!(!activity.contains("stalled"));
        assert!(!activity.contains("SECRET_"));

        let acknowledged =
            acknowledge_handoff_record(&session.directory, &session.job_id, &session.handoff_id)
                .unwrap();
        assert_eq!(acknowledged.state, "acknowledged");
        let completed = session
            .complete(&Captured {
                stdout: String::new(),
                stderr: String::new(),
                success: true,
                timed_out: false,
                cancelled: false,
            })
            .unwrap();
        assert_eq!(completed, exact);
    }

    #[test]
    fn high_frequency_transition_activity_is_coalesced_below_reader_bound() {
        let session = test_handoff_session("coalesced-transitions");
        let base = 1_000_000_u128;
        let mut stream = session
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for second in 0..=21_600_u128 {
            let (class, tool_name, status) = match second % 5 {
                0 => (ActivityClass::Thought, None, None),
                1 => (ActivityClass::AvailableCommands, None, None),
                2 => (ActivityClass::Usage, None, None),
                3 => (
                    ActivityClass::ToolCall,
                    Some("read_file".to_string()),
                    Some("pending".to_string()),
                ),
                _ => (
                    ActivityClass::ToolCallUpdate,
                    None,
                    Some("completed".to_string()),
                ),
            };
            session
                .accept_activity(
                    &mut stream,
                    SafeActivity {
                        class,
                        timestamp_ms: base + second * 1_000,
                        tool_name,
                        status,
                        visible_text: None,
                    },
                )
                .unwrap();
        }
        drop(stream);

        let journal = fs::read(journal_path(&session.directory)).unwrap();
        assert!(journal.len() < MAX_HANDOFF_JOURNAL_BYTES);
        assert!(journal.iter().filter(|byte| **byte == b'\n').count() <= 190);
        let record = load_handoff_readonly(&session.directory).unwrap();
        assert_eq!(record.phase, "reasoning");
        assert_eq!(record.last_activity_ms, Some(base + 21_600 * 1_000));
        assert!(record.activities.len() <= MAX_ACTIVITY_EVENTS);
    }

    #[test]
    fn activity_transitions_and_terminal_flush_current_visible_excerpt() {
        let session = test_handoff_session("activity-transitions");
        let activities = [
            SafeActivity {
                class: ActivityClass::Thought,
                timestamp_ms: 1_000,
                tool_name: None,
                status: None,
                visible_text: None,
            },
            SafeActivity {
                class: ActivityClass::Thought,
                timestamp_ms: 1_001,
                tool_name: None,
                status: None,
                visible_text: None,
            },
            SafeActivity {
                class: ActivityClass::ToolCall,
                timestamp_ms: 1_002,
                tool_name: Some("read_file".to_string()),
                status: Some("in_progress".to_string()),
                visible_text: None,
            },
            SafeActivity {
                class: ActivityClass::ToolCall,
                timestamp_ms: 1_003,
                tool_name: Some("read_file".to_string()),
                status: Some("in_progress".to_string()),
                visible_text: None,
            },
            SafeActivity {
                class: ActivityClass::ToolCallUpdate,
                timestamp_ms: 1_004,
                tool_name: Some("read_file".to_string()),
                status: Some("completed".to_string()),
                visible_text: None,
            },
            SafeActivity {
                class: ActivityClass::Text,
                timestamp_ms: 1_005,
                tool_name: None,
                status: None,
                visible_text: Some("first".to_string()),
            },
            SafeActivity {
                class: ActivityClass::Text,
                timestamp_ms: 1_006,
                tool_name: None,
                status: None,
                visible_text: Some(" second".to_string()),
            },
            SafeActivity {
                class: ActivityClass::End,
                timestamp_ms: 1_007,
                tool_name: None,
                status: Some("end_turn".to_string()),
                visible_text: None,
            },
        ];
        let mut stream = session
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for activity in activities {
            session.accept_activity(&mut stream, activity).unwrap();
        }
        drop(stream);

        let record = load_handoff_readonly(&session.directory).unwrap();
        assert_eq!(record.excerpt, "first second");
        assert_eq!(record.phase, "ending");
        assert_eq!(
            record
                .activities
                .iter()
                .map(|activity| activity.class.as_str())
                .collect::<Vec<_>>(),
            vec![
                "thought",
                "thought",
                "tool_call",
                "tool_call_update",
                "text",
                "text",
                "end"
            ]
        );
    }

    #[test]
    fn periodic_non_text_flush_persists_queued_visible_excerpt() {
        let session = test_handoff_session("periodic-visible-flush");
        let mut stream = session
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for activity in [
            SafeActivity {
                class: ActivityClass::Thought,
                timestamp_ms: 1_000,
                tool_name: None,
                status: None,
                visible_text: None,
            },
            SafeActivity {
                class: ActivityClass::Text,
                timestamp_ms: 1_001,
                tool_name: None,
                status: None,
                visible_text: Some("first".to_string()),
            },
            SafeActivity {
                class: ActivityClass::Text,
                timestamp_ms: 1_002,
                tool_name: None,
                status: None,
                visible_text: Some(" second".to_string()),
            },
            SafeActivity {
                class: ActivityClass::Thought,
                timestamp_ms: 1_001 + ACTIVITY_FLUSH_INTERVAL_MS,
                tool_name: None,
                status: None,
                visible_text: None,
            },
        ] {
            session.accept_activity(&mut stream, activity).unwrap();
        }
        assert!(!stream.visible_dirty);
        drop(stream);

        let record = load_handoff_readonly(&session.directory).unwrap();
        assert_eq!(record.excerpt, "first second");
        assert_eq!(record.phase, "reasoning");
    }

    #[test]
    fn durable_report_is_not_invalidated_by_followup_visible_text() {
        let session = test_handoff_session("durable-report-followup");
        let report = format!(
            "=== TWO_CHAIRS_HANDOFF_READY_V1 job={} handoff={} ===\nreport\n=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff={} ===",
            session.job_id, session.handoff_id, session.handoff_id
        );
        let mut stream = session
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        session
            .accept_activity(
                &mut stream,
                SafeActivity {
                    class: ActivityClass::Text,
                    timestamp_ms: 1_000,
                    tool_name: None,
                    status: None,
                    visible_text: Some(report.clone()),
                },
            )
            .unwrap();
        assert!(stream.report_persisted);
        session
            .accept_activity(
                &mut stream,
                SafeActivity {
                    class: ActivityClass::Text,
                    timestamp_ms: 1_001,
                    tool_name: None,
                    status: None,
                    visible_text: Some("receipt observed".to_string()),
                },
            )
            .unwrap();
        drop(stream);

        assert_eq!(
            read_bounded_report(&report_path(&session.directory)).unwrap(),
            report
        );
    }

    #[test]
    fn duplicate_report_markers_still_fail_after_first_report_is_durable() {
        let session = test_handoff_session("duplicate-report");
        let report = format!(
            "=== TWO_CHAIRS_HANDOFF_READY_V1 job={} handoff={} ===\nreport\n=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff={} ===\n",
            session.job_id, session.handoff_id, session.handoff_id
        );
        let mut stream = session
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        session
            .accept_activity(
                &mut stream,
                SafeActivity {
                    class: ActivityClass::Text,
                    timestamp_ms: 1_000,
                    tool_name: None,
                    status: None,
                    visible_text: Some(report.clone()),
                },
            )
            .unwrap();
        assert!(stream.report_persisted);
        let error = session
            .accept_activity(
                &mut stream,
                SafeActivity {
                    class: ActivityClass::Text,
                    timestamp_ms: 1_001,
                    tool_name: None,
                    status: None,
                    visible_text: Some(report),
                },
            )
            .unwrap_err();
        assert!(error.contains("more than one"));
    }

    #[test]
    fn split_start_marker_waits_for_its_streamed_newline() {
        let session = test_handoff_session("split-start-marker");
        let start = format!(
            "=== TWO_CHAIRS_HANDOFF_READY_V1 job={} handoff={} ===",
            session.job_id, session.handoff_id
        );
        let end = format!(
            "=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff={} ===",
            session.handoff_id
        );
        let mut stream = session
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        session
            .accept_activity(
                &mut stream,
                SafeActivity {
                    class: ActivityClass::Text,
                    timestamp_ms: 1_000,
                    tool_name: None,
                    status: None,
                    visible_text: Some(start.clone()),
                },
            )
            .unwrap();
        assert!(!stream.report_persisted);
        session
            .accept_activity(
                &mut stream,
                SafeActivity {
                    class: ActivityClass::Text,
                    timestamp_ms: 1_001,
                    tool_name: None,
                    status: None,
                    visible_text: Some(format!("\nstreamed report\n{end}")),
                },
            )
            .unwrap();
        assert!(stream.report_persisted);
        drop(stream);

        assert_eq!(
            read_bounded_report(&report_path(&session.directory)).unwrap(),
            format!("{start}\nstreamed report\n{end}")
        );
    }

    #[test]
    fn acknowledgement_is_strict_idempotent_and_supports_detached_receipt() {
        let session = test_handoff_session("acknowledgement");
        assert!(acknowledge_handoff_record(
            &session.directory,
            &session.job_id,
            &session.handoff_id
        )
        .unwrap_err()
        .contains("not ready"));
        assert!(acknowledge_handoff_record(
            &session.directory,
            &session.job_id,
            "11111111-2222-4333-8444-555555555555"
        )
        .unwrap_err()
        .contains("handoff_id"));
        let exact = format!(
            "=== TWO_CHAIRS_HANDOFF_READY_V1 job={} handoff={} ===\nreport\n=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff={} ===",
            session.job_id, session.handoff_id, session.handoff_id
        );
        session.persist_report(&exact).unwrap();
        session.finish_without_success().unwrap();
        assert_eq!(session.read().unwrap().state, "detached_unacked");
        let first =
            acknowledge_handoff_record(&session.directory, &session.job_id, &session.handoff_id)
                .unwrap();
        let second =
            acknowledge_handoff_record(&session.directory, &session.job_id, &session.handoff_id)
                .unwrap();
        assert_eq!(first.receipt_at_ms, second.receipt_at_ms);
        assert_eq!(second.phase, "detached_received");
        assert_eq!(second.process_state, "exited");
    }

    #[test]
    fn journal_head_repairs_a_stale_snapshot_and_ignores_partial_tail() {
        let session = test_handoff_session("journal-recovery");
        let mut newer = session.read().unwrap();
        newer.sequence += 1;
        newer.phase = "running".to_string();
        let encoded = handoff_json(&newer).to_compact_string();
        let mut journal = OpenOptions::new()
            .append(true)
            .open(journal_path(&session.directory))
            .unwrap();
        journal.write_all(encoded.as_bytes()).unwrap();
        journal.write_all(b"\n{\"partial\":").unwrap();
        journal.flush().unwrap();
        drop(journal);

        let recovered = load_handoff(&session.directory).unwrap();
        assert_eq!(recovered, newer);
        let repaired = parse_handoff(
            fs::read_to_string(snapshot_path(&session.directory))
                .unwrap()
                .trim(),
        )
        .unwrap();
        assert_eq!(repaired, newer);
    }

    #[test]
    fn readonly_handoff_load_ignores_writer_lock_and_never_repairs_snapshot() {
        let session = test_handoff_session("readonly-load");
        let stale = b"{\"stale\":true}\n";
        fs::write(snapshot_path(&session.directory), stale).unwrap();

        let _writer_lock = HandoffFileLock::acquire(&session.directory).unwrap();
        let record = load_handoff_readonly(&session.directory).unwrap();
        assert_eq!(record.job_id, session.job_id);
        assert_eq!(fs::read(snapshot_path(&session.directory)).unwrap(), stale);
    }

    #[test]
    fn stream_fails_closed_on_records_after_end_and_incomplete_tail() {
        let terminal = test_handoff_session("terminal-stream");
        terminal
            .observe_output(
                OutputStream::Stdout,
                b"{\"type\":\"end\",\"stopReason\":\"end_turn\"}\n",
            )
            .unwrap();
        let error = terminal
            .observe_output(
                OutputStream::Stdout,
                b"{\"type\":\"text\",\"data\":\"too late\"}\n",
            )
            .unwrap_err();
        assert!(error.contains("after terminal end"));
        assert!(terminal
            .complete(&Captured {
                stdout: String::new(),
                stderr: String::new(),
                success: false,
                timed_out: false,
                cancelled: false,
            })
            .unwrap_err()
            .contains("parser failed"));

        let incomplete = test_handoff_session("incomplete-stream");
        incomplete
            .observe_output(
                OutputStream::Stdout,
                b"{\"type\":\"text\",\"data\":\"tail\"}",
            )
            .unwrap();
        assert!(incomplete
            .complete(&Captured {
                stdout: String::new(),
                stderr: String::new(),
                success: true,
                timed_out: false,
                cancelled: false,
            })
            .unwrap_err()
            .contains("incomplete record"));
    }
}
