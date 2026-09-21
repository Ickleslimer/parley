//! Long-lived asynchronous Grok jobs for the MCP stdio server.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::ask::{self, AskFailure, AskSpawnObserver, AskSpawnReceipt};
use crate::context::refuse_reparse_chain;
use crate::event_log::{EventReceipt, ExchangeReceipt};
use crate::grok_subagents::{append_parent_contract, GuardedSubagentLaunch};
use crate::job_lock::ExecutionLease;
use crate::jobs::{
    BeginOutcome, JobMode, JobSpec, JobState, JobStateEnv, JobStore, JobTransition, JobView,
    LaneOwner, LaneRole, MAX_LISTED_JOBS,
};
use crate::json::Json;
use crate::lane_plan;
use crate::policy::RuntimePolicy;
use crate::process::CancellationToken;

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
        let caller = env::var("PARLEY_CALLER").unwrap_or_else(|_| "parley".to_string());
        let store = Arc::new(
            JobStore::open(&JobStateEnv {
                job_state_dir: Some(state_root.clone()),
                caller: Some(caller.clone()),
                localappdata: env::var_os("LOCALAPPDATA").map(PathBuf::from),
            })
            .map_err(|error| error.to_string())?,
        );
        store
            .recover_startup()
            .map_err(|error| format!("recover asynchronous jobs: {error}"))?;
        let escrow_dir = state_root.join("escrow").join(caller);
        Ok(Self {
            inner: Some(Arc::new(RuntimeInner {
                store,
                active: Mutex::new(HashMap::new()),
                terminal: Mutex::new(HashMap::new()),
                threads: Mutex::new(Vec::new()),
                event_log: env::var_os("PARLEY_EVENT_LOG").map(PathBuf::from),
                escrow_dir,
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

        let (mut request, policy) = ask::prepare_for_job(&spec.request)?;
        if request.harness != "grok" {
            return Err("start_agent_job supports only Grok".to_string());
        }
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
        request.allow_subagents = guarded_launch.is_some();
        if guarded_launch.is_some() {
            append_parent_contract(
                &mut request.prompt,
                spec.lane_plan
                    .as_ref()
                    .expect("guarded launch requires lane plan"),
            );
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
        let handle = thread::Builder::new()
            .name(format!("parley-job-{job_id}"))
            .spawn(move || {
                run_worker(
                    worker_inner,
                    worker_job_id,
                    spec,
                    policy,
                    guarded_launch,
                    cancellation,
                    started_tx,
                )
            })
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
            return Ok(JobLookup {
                view,
                delivery: None,
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
        Ok(JobLookup { view, delivery })
    }
}

fn run_worker(
    inner: Arc<RuntimeInner>,
    job_id: String,
    spec: JobSpec,
    policy: RuntimePolicy,
    guarded_launch: Option<GuardedSubagentLaunch>,
    cancellation: CancellationToken,
    started_tx: mpsc::SyncSender<StartSignal>,
) {
    let sent = Arc::new(AtomicBool::new(false));
    let observer = JobObserver {
        store: Arc::clone(&inner.store),
        job_id: job_id.clone(),
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
        Err(failure) => finish_ask_failure(
            &inner,
            &job_id,
            failure,
            cancellation.is_cancelled(),
            &started_tx,
            &sent,
        ),
    }
}

struct JobObserver {
    store: Arc<JobStore>,
    job_id: String,
    started_tx: mpsc::SyncSender<StartSignal>,
    sent: Arc<AtomicBool>,
}

impl AskSpawnObserver for JobObserver {
    fn process_created(&self, _receipt: &AskSpawnReceipt) -> Result<(), String> {
        Ok(())
    }

    fn process_resumed(&self, receipt: &AskSpawnReceipt) -> Result<(), String> {
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
}
