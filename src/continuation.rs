//! Fail-closed direct user continuation of one published canonical Grok task.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ask::AskRequest;
use crate::cli::{ContinuationCommand, ContinuationOptions};
use crate::context::refuse_reparse_chain;
use crate::event_log::ExchangeLog;
use crate::grok_acp::{AcpOutcome, AcpProfile, AcpRequest, SessionAction};
use crate::health_report::HealthReporter;
use crate::jobs::{GrantKind, PathGrant};
use crate::json::Json;
use crate::peer_session::{resolve_canonical_target, CanonicalAction, ConversationMode};
use crate::policy::RuntimePolicy;
use crate::process::CancellationToken;
use crate::profile_namespace;
use crate::sha256::{hex_encode, sha256};

const SCHEMA_VERSION: u64 = 1;
const PERMISSION_POLICY_VERSION: &str = "parley-continuation-acp-permissions-v4";
const LOCKED_PROFILE_PERMISSION_MODE: &str = "auto";
const EFFECTIVE_CONTINUATION_PERMISSION_MODE: &str = "acp_allow_once";
const CONTINUATION_TRANSPORT: &str = "acp-stdio";
const MAX_USER_TEXT_CHARS: usize = 8_000;
const MAX_USER_TEXT_BYTES: u64 = (MAX_USER_TEXT_CHARS as u64) * 4;
const MAX_EVIDENCE_TEXT_CHARS: usize = 256_000;
const MAX_DIRTY_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_GRANTS: usize = 64;
const MAX_RELATIVE_PATH_CHARS: usize = 512;
const MAX_SOURCE_SESSION_CHARS: usize = 256;
const MAX_GIT_CONFIG_BYTES: u64 = 1024 * 1024;
const DEFAULT_TTL_SECS: u64 = 21_600;
const EXECUTION_CEILING_SECS: u64 = 21_600;
const CANCEL_POLL_MS: u64 = 250;
static ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Manifest {
    manifest_id: String,
    profile_namespace: String,
    caller: String,
    source_session: String,
    worktree: PathBuf,
    allowed_root: PathBuf,
    head: String,
    grants: Vec<PathGrant>,
    created_at_ms: u128,
    expires_at_ms: u128,
    model: String,
    reasoning_effort: String,
    cli_version: String,
    permission_mode: String,
    canonical_epoch: u64,
    event_log: PathBuf,
    context_state_dir: PathBuf,
    job_state_dir: PathBuf,
    canary_hash: String,
    granted_fingerprint: String,
    outside_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DirtySnapshot {
    granted_fingerprint: String,
    outside_fingerprint: String,
    changed_paths: Vec<String>,
    outside_paths: Vec<String>,
    observed_paths: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
struct JournalState {
    running: Option<String>,
    last_granted_fingerprint: Option<String>,
    last_outside_fingerprint: Option<String>,
    terminal_evidence: BTreeMap<String, String>,
    received: BTreeSet<String>,
    observed_paths: BTreeSet<String>,
    reviewed_hash: Option<String>,
    terminal_failure: Option<String>,
}

#[derive(Clone, Debug)]
struct Evidence {
    continuation_id: String,
    manifest_id: String,
    state: String,
    prompt: String,
    reply: Option<String>,
    error: Option<String>,
    exchange_id: Option<String>,
    request_event_id: Option<String>,
    completion_event_id: Option<String>,
    changed_paths: Vec<String>,
    validation_state: String,
    granted_fingerprint: String,
    outside_fingerprint: String,
    diff_fingerprint: String,
    git_head: Option<String>,
    evidence_hash: String,
    completed_at_ms: u128,
}

struct EvidenceOutcome<'a> {
    state: &'a str,
    reply: Option<String>,
    error: Option<String>,
    receipt: Option<crate::event_log::ExchangeReceipt>,
}

#[derive(Clone)]
struct StatePaths {
    root: PathBuf,
    profile: PathBuf,
    manifests: PathBuf,
    evidence: PathBuf,
    cancels: PathBuf,
    runtime: PathBuf,
    journal: PathBuf,
    lock: PathBuf,
}

pub(crate) fn run_cli(options: ContinuationOptions) -> Result<(), String> {
    let output = match options.command {
        ContinuationCommand::Status => status_json()?,
        ContinuationCommand::Submit { continuation_id } => {
            let mut text = String::new();
            std::io::stdin()
                .take(MAX_USER_TEXT_BYTES + 1)
                .read_to_string(&mut text)
                .map_err(|error| format!("read continuation text from stdin: {error}"))?;
            if text.len() as u64 > MAX_USER_TEXT_BYTES {
                return Err("continuation stdin exceeds its UTF-8 byte bound".to_string());
            }
            run_submission(&continuation_id, &text)?
        }
        ContinuationCommand::Cancel { continuation_id } => request_cancel(&continuation_id)?,
        ContinuationCommand::Receive {
            continuation_id,
            evidence_hash,
        } => receive(&continuation_id, &evidence_hash)?,
        ContinuationCommand::Canary => run_permission_canary()?,
    };
    println!("{}", output.to_compact_string());
    Ok(())
}

pub(crate) fn publish_from_mcp(
    args: &Json,
    default_cwd: &Path,
    policy: &RuntimePolicy,
) -> Result<Json, String> {
    require_codex_caller()?;
    let manifest_id = required_string(args, "manifest_id")?;
    validate_uuid("manifest_id", &manifest_id)?;
    let source_session = required_string(args, "source_session")?;
    validate_source_session(&source_session)?;
    let cwd = args
        .get("cwd")
        .and_then(Json::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| default_cwd.to_path_buf());
    let worktree = policy.validate_spawn_cwd(&cwd)?;
    let grants = parse_grants(args.get("writable_paths"), &worktree)?;
    if grants.is_empty() {
        return Err("publish_continuation requires at least one writable path".to_string());
    }
    reject_validation_commands(args.get("validation_commands"))?;
    let ttl_secs = parse_ttl_seconds(args.get("ttl_seconds"))?;

    let allowed_root = policy
        .allowed_cwd_root()
        .ok_or_else(|| "continuation publication requires PARLEY_ALLOWED_CWD_ROOT".to_string())?
        .to_path_buf();
    let model = policy
        .grok_locked_model()
        .ok_or_else(|| "continuation publication requires a locked Grok model".to_string())?
        .to_string();
    let reasoning_effort = policy
        .grok_locked_reasoning_effort()
        .ok_or_else(|| "continuation publication requires locked Grok reasoning".to_string())?
        .to_string();
    let cli_version = policy
        .grok_locked_version()
        .ok_or_else(|| "continuation publication requires a locked Grok CLI version".to_string())?
        .to_string();
    let permission_mode = policy
        .grok_locked_permission_mode()
        .ok_or_else(|| "continuation publication requires a locked permission mode".to_string())?
        .to_string();
    if env::var("PARLEY_DISABLE_YOLO").ok().as_deref() != Some("true") {
        return Err("continuation publication requires PARLEY_DISABLE_YOLO=true".to_string());
    }
    let event_log = required_absolute_env("PARLEY_EVENT_LOG")?;
    let context_state_dir = required_absolute_env("PARLEY_CONTEXT_STATE_DIR")?;
    let job_state_dir = required_absolute_env("PARLEY_JOB_STATE_DIR")?;
    let profile_namespace = profile_namespace::from_env()?;
    let caller = env::var("PARLEY_CALLER").map_err(|_| "PARLEY_CALLER is required".to_string())?;
    let paths = StatePaths::for_profile(&profile_namespace)?;
    paths.ensure()?;
    let canary_hash = latest_valid_canary_hash(
        &paths,
        &cli_version,
        &model,
        &reasoning_effort,
        &permission_mode,
    )?;
    let head = git_head(&worktree)?;
    let snapshot = dirty_snapshot(&worktree, &grants, &BTreeSet::new())?;
    if !snapshot.outside_paths.is_empty() {
        return Err(format!(
            "continuation publication refuses dirty paths outside the grant: {}",
            snapshot.outside_paths.join(", ")
        ));
    }
    let created_at_ms = now_ms();
    let manifest = Manifest {
        manifest_id,
        profile_namespace,
        caller,
        source_session,
        worktree,
        allowed_root,
        head,
        grants,
        created_at_ms,
        expires_at_ms: created_at_ms.saturating_add((ttl_secs as u128) * 1_000),
        model,
        reasoning_effort,
        cli_version,
        permission_mode,
        canonical_epoch: policy.grok_canonical_epoch(),
        event_log,
        context_state_dir,
        job_state_dir,
        canary_hash,
        granted_fingerprint: snapshot.granted_fingerprint,
        outside_fingerprint: snapshot.outside_fingerprint,
    };
    if let Some(previous) = latest_manifest(&paths)? {
        let _ = read_journal_with_orphan_recovery(&paths, &previous.manifest_id)?;
    }
    let _lock = acquire_lock(&paths.lock)?;
    if let Some(previous) = latest_manifest(&paths)? {
        let previous_state = read_journal(&paths, &previous.manifest_id)?;
        if previous_state.running.is_some() && now_ms() <= previous.expires_at_ms {
            return Err("cannot supersede a continuation while it is running".to_string());
        }
        if !previous_state.terminal_evidence.is_empty()
            && !review_is_current(&previous, &previous_state)
        {
            return Err("cannot supersede unreviewed direct continuation evidence".to_string());
        }
        append_transition(
            &paths.journal,
            transition(
                &previous.manifest_id,
                None,
                "superseded",
                vec![("next_manifest_id", Json::Str(manifest.manifest_id.clone()))],
            ),
        )?;
    }
    let manifest_path = paths
        .manifests
        .join(format!("{}.json", manifest.manifest_id));
    create_new_json(&manifest_path, &manifest_to_json(&manifest))?;
    append_transition(
        &paths.journal,
        transition(
            &manifest.manifest_id,
            None,
            "published",
            vec![("canary_hash", Json::Str(manifest.canary_hash.clone()))],
        ),
    )?;
    Ok(manifest_public_json(&manifest, "armed", 0, None, None))
}

pub(crate) fn get_for_codex(policy: &RuntimePolicy) -> Result<Json, String> {
    require_codex_caller()?;
    let profile = profile_namespace::from_env()?;
    let paths = StatePaths::for_profile(&profile)?;
    let Some(manifest) = latest_manifest(&paths)? else {
        return Ok(object(vec![
            ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
            ("state", Json::Str("absent".to_string())),
        ]));
    };
    if policy
        .allowed_cwd_root()
        .is_none_or(|root| !paths_equal(root, &manifest.allowed_root))
    {
        return Err("continuation manifest does not belong to this locked profile".to_string());
    }
    let state = read_journal_with_orphan_recovery(&paths, &manifest.manifest_id)?;
    let aggregate_hash = aggregate_evidence_hash(&manifest, &state);
    let evidence = state
        .terminal_evidence
        .keys()
        .map(|continuation_id| read_evidence(&paths, continuation_id).map(evidence_to_json))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(object(vec![
        ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
        ("state", Json::Str(manifest_state(&manifest, &state))),
        (
            "manifest",
            manifest_public_json(&manifest, "", evidence.len(), None, None),
        ),
        ("aggregate_evidence_hash", Json::Str(aggregate_hash)),
        ("evidence", Json::Array(evidence)),
    ]))
}

pub(crate) fn review_from_mcp(args: &Json, policy: &RuntimePolicy) -> Result<Json, String> {
    require_codex_caller()?;
    let manifest_id = required_string(args, "manifest_id")?;
    let expected_hash = required_string(args, "evidence_hash")?;
    let profile = profile_namespace::from_env()?;
    let paths = StatePaths::for_profile(&profile)?;
    let manifest = load_manifest(&paths, &manifest_id)?;
    if policy
        .allowed_cwd_root()
        .is_none_or(|root| !paths_equal(root, &manifest.allowed_root))
    {
        return Err("continuation manifest does not belong to this locked profile".to_string());
    }
    read_journal_with_orphan_recovery(&paths, &manifest.manifest_id)?;
    let _lock = acquire_lock(&paths.lock)?;
    let state = read_journal(&paths, &manifest.manifest_id)?;
    if state.running.is_some() {
        return Err("continuation review is unavailable while a direct job is running".to_string());
    }
    if let Some(reason) = state.terminal_failure.as_deref() {
        return Err(format!(
            "continuation review is unavailable because durable evidence did not seal ({reason})"
        ));
    }
    if state.terminal_evidence.is_empty() {
        return Err(
            "continuation review requires at least one durable evidence record".to_string(),
        );
    }
    let actual_hash = aggregate_evidence_hash(&manifest, &state);
    if actual_hash != expected_hash {
        return Err("continuation evidence changed; retrieve it again before review".to_string());
    }
    append_transition(
        &paths.journal,
        transition(
            &manifest.manifest_id,
            None,
            "codex_reviewed",
            vec![("evidence_hash", Json::Str(actual_hash.clone()))],
        ),
    )?;
    Ok(object(vec![
        ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
        ("state", Json::Str("reviewed".to_string())),
        ("manifest_id", Json::Str(manifest.manifest_id)),
        ("evidence_hash", Json::Str(actual_hash)),
    ]))
}

pub(crate) fn require_reviewed(cwd: &Path) -> Result<(), String> {
    if env::var_os("LOCALAPPDATA").is_none() {
        return Ok(());
    }
    require_reviewed_in(cwd, all_latest_manifests()?)
}

fn require_reviewed_in(cwd: &Path, manifests: Vec<(StatePaths, Manifest)>) -> Result<(), String> {
    for (paths, manifest) in manifests {
        if !paths_overlap(&manifest.worktree, cwd) {
            continue;
        }
        let state = read_journal_with_orphan_recovery(&paths, &manifest.manifest_id)?;
        if state.reviewed_hash.is_some() {
            if review_is_current(&manifest, &state) {
                continue;
            }
            return Err(format!(
                "Grok work is blocked because the direct continuation review hash is stale for manifest {}",
                manifest.manifest_id
            ));
        }
        if let Some(reason) = state.terminal_failure.as_deref() {
            return Err(format!(
                "Grok work is blocked because direct user-Grok evidence did not seal ({reason}); inspect the event log and worktree before republishing"
            ));
        }
        if let Some(continuation_id) = state.running.as_deref() {
            return Err(format!(
                "Grok work is blocked while direct continuation {continuation_id} is running"
            ));
        }
        if !state.terminal_evidence.is_empty() {
            return Err(format!(
                "Grok work is blocked until Codex reviews direct user-Grok work for manifest {}; call get_continuation_evidence and review_continuation",
                manifest.manifest_id
            ));
        }
    }
    Ok(())
}

fn run_submission(continuation_id: &str, user_text: &str) -> Result<Json, String> {
    validate_uuid("continuation id", continuation_id)?;
    validate_user_text(user_text)?;
    let (paths, manifest) = select_active_manifest()?;
    let first_lock = acquire_lock(&paths.lock)?;
    let first_state = read_journal(&paths, &manifest.manifest_id)?;
    if review_is_current(&manifest, &first_state) {
        return Err("the published continuation has already been reviewed and closed".to_string());
    }
    if first_state.reviewed_hash.is_some() {
        return Err("the published continuation review hash is stale".to_string());
    }
    if first_state.terminal_failure.is_some() {
        return Err(
            "the published continuation ended without durable evidence; Codex must inspect and republish"
                .to_string(),
        );
    }
    if let Some(running) = first_state.running {
        return Err(format!("continuation {running} is already running"));
    }
    validate_manifest_runtime(&paths, &manifest, &first_state)?;
    drop(first_lock);

    apply_manifest_environment(&manifest)?;
    let target = resolve_canonical_target(
        &manifest.source_session,
        manifest.canonical_epoch,
        &manifest.worktree,
    )?;
    if target.action == CanonicalAction::New {
        return Err("canonical Grok session has not been established".to_string());
    }

    let _runtime_lock = acquire_lock(&paths.runtime_lock_path(continuation_id))?;
    let state_lock = acquire_lock(&paths.lock)?;
    let state = read_journal(&paths, &manifest.manifest_id)?;
    if review_is_current(&manifest, &state)
        || state.reviewed_hash.is_some()
        || state.running.is_some()
        || state.terminal_failure.is_some()
    {
        return Err("the continuation changed while the request was being prepared".to_string());
    }
    validate_manifest_runtime(&paths, &manifest, &state)?;
    RuntimePolicy::from_env()?.validate_locked_grok_version()?;
    let prompt = continuation_prompt(user_text, &manifest.grants);
    let request = AskRequest {
        harness: "grok".to_string(),
        prompt: prompt.clone(),
        model: None,
        reasoning_effort: None,
        provider: None,
        cwd: manifest.worktree.clone(),
        permission_mode: None,
        max_turns: None,
        session_id: None,
        resume_id: Some(target.session_id.clone()),
        conversation_mode: Some(ConversationMode::Isolated),
        yolo: false,
        context: None,
        max_context_chars: None,
        images: Vec::new(),
        allow_subagents: false,
    };
    let exchange_id = ExchangeLog::allocate_exchange_id();
    let log = ExchangeLog::start_with_exchange_id(&request, &prompt, exchange_id.clone())
        .map_err(|error| format!("request event logging failed; Grok was not started: {error}"))?;
    let reporter = match HealthReporter::from_env() {
        Ok(reporter) => reporter,
        Err(error) => {
            let message = format!("health configuration failed; Grok was not started: {error}");
            let _ = log.failure("health_preflight_error", &message, 0);
            return Err(message);
        }
    };
    if let Err(error) = reporter.request_started(log.request_receipt()) {
        let message = format!("health request logging failed; Grok was not started: {error}");
        let _ = log.failure("health_preflight_error", &message, 0);
        return Err(message);
    }
    let worktree_watcher = match WorktreeWatcher::start(&manifest.worktree) {
        Ok(watcher) => watcher,
        Err(error) => {
            let message = format!("worktree watcher failed; Grok was not started: {error}");
            if let Ok(completion) = log.failure("continuation_preflight_error", &message, 0) {
                let _ = reporter.turn_failure(&log.exchange_receipt(completion), "turn_error");
            }
            return Err(message);
        }
    };
    append_transition(
        &paths.journal,
        transition(
            &manifest.manifest_id,
            Some(continuation_id),
            "user_authorized",
            vec![("prompt_hash", Json::Str(hash_text(user_text)))],
        ),
    )?;
    append_transition(
        &paths.journal,
        transition(
            &manifest.manifest_id,
            Some(continuation_id),
            "running",
            vec![],
        ),
    )?;
    drop(state_lock);

    let cancellation = CancellationToken::default();
    let watcher_done = Arc::new(AtomicBool::new(false));
    let cancel_watcher = spawn_cancel_watcher(
        paths.cancel_path(continuation_id),
        cancellation.clone(),
        Arc::clone(&watcher_done),
    );
    let observer_paths = paths.clone_for_observer();
    let observer_manifest_id = manifest.manifest_id.clone();
    let observer_continuation_id = continuation_id.to_string();
    let observer_exchange_id = exchange_id.clone();
    let observer_request_event_id = log.request_receipt().event_id.clone();
    let started = Instant::now();
    let outcome = crate::grok_acp::run(
        AcpRequest {
            cwd: &manifest.worktree,
            session: SessionAction::Load(target.session_id),
            prompt: &prompt,
            model: &manifest.model,
            reasoning_effort: &manifest.reasoning_effort,
            grants: &manifest.grants,
            cancellation: &cancellation,
            timeout: Duration::from_secs(EXECUTION_CEILING_SECS),
            idle_timeout: Duration::ZERO,
            profile: AcpProfile::Continuation,
        },
        move |process_id| {
            let _lock = acquire_lock(&observer_paths.lock)?;
            append_transition(
                &observer_paths.journal,
                transition(
                    &observer_manifest_id,
                    Some(&observer_continuation_id),
                    "process_created",
                    vec![
                        ("process_id", Json::Number(process_id as f64)),
                        ("exchange_id", Json::Str(observer_exchange_id)),
                        ("request_event_id", Json::Str(observer_request_event_id)),
                    ],
                ),
            )
        },
    );
    watcher_done.store(true, Ordering::Release);
    let _ = cancel_watcher.join();

    let duration_ms = started.elapsed().as_millis();
    let acp_outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => AcpOutcome {
            session_id: Some(request.resume_id.clone().expect("resume id")),
            reply: None,
            error: Some(format!(
                "ACP preflight failed; Grok was not started: {error}"
            )),
            timed_out: false,
            cancelled: false,
            approved_mutations: 0,
            rejected_permissions: 0,
        },
    };
    let mut terminal_state = if acp_outcome.error.is_none() && acp_outcome.reply.is_some() {
        "succeeded"
    } else if acp_outcome.cancelled {
        "cancelled"
    } else if acp_outcome.timed_out {
        "timed_out"
    } else {
        "failed"
    };
    let mut reply = acp_outcome.reply;
    let mut error = acp_outcome.error;
    let completion_result = match (&reply, &error) {
        (Some(reply), None) => log.success(reply, duration_ms),
        (_, Some(error)) => log.failure(
            if acp_outcome.cancelled {
                "cancelled"
            } else if acp_outcome.timed_out {
                "timeout"
            } else {
                "error"
            },
            error,
            duration_ms,
        ),
        _ => log.failure(
            "error",
            "Grok ACP returned neither a reply nor an error",
            duration_ms,
        ),
    };
    let receipt = match completion_result {
        Ok(completion) => {
            let receipt = log.exchange_receipt(completion);
            let health_result = if terminal_state == "succeeded" {
                reporter.grok_success(&receipt)
            } else {
                reporter.turn_failure(
                    &receipt,
                    if acp_outcome.timed_out {
                        "watchdog_killed"
                    } else {
                        "turn_error"
                    },
                )
            };
            if let Err(health_error) = health_result {
                terminal_state = "health_logging_failed";
                error = Some(combine_error(
                    error,
                    format!(
                        "non-retriable health completion logging failure after Grok ran: {health_error}"
                    ),
                ));
            }
            Some(receipt)
        }
        Err(log_error) => {
            terminal_state = "logging_failed";
            let health_error = reporter.logging_failure(log.request_receipt()).err();
            error = Some(combine_error(
                error,
                format!(
                    "non-retriable event completion logging failure after Grok ran: {log_error}{}",
                    health_error
                        .map(|error| format!("; health reporting also failed: {error}"))
                        .unwrap_or_default()
                ),
            ));
            None
        }
    };
    let observed_head = match git_head(&manifest.worktree) {
        Ok(head) => {
            if head != manifest.head {
                terminal_state = "containment_violation";
                error = Some(combine_error(
                    error,
                    format!(
                        "continuation Git HEAD changed from {} to {head}",
                        manifest.head
                    ),
                ));
            }
            Some(head)
        }
        Err(head_error) => {
            terminal_state = "state_observation_failed";
            error = Some(combine_error(
                error,
                format!("post-run Git HEAD inspection failed: {head_error}"),
            ));
            None
        }
    };
    let observed_paths = match worktree_watcher.finish() {
        Ok(paths) => {
            let mut combined = state.observed_paths.clone();
            combined.extend(paths);
            Ok(combined)
        }
        Err(error) => Err(error),
    };
    let (post, observation_error) = match observed_paths.and_then(|observed_paths| {
        dirty_snapshot(&manifest.worktree, &manifest.grants, &observed_paths)
    }) {
        Ok(snapshot) => (snapshot, None),
        Err(snapshot_error) => (
            DirtySnapshot {
                granted_fingerprint: state
                    .last_granted_fingerprint
                    .clone()
                    .unwrap_or_else(|| manifest.granted_fingerprint.clone()),
                outside_fingerprint: state
                    .last_outside_fingerprint
                    .clone()
                    .unwrap_or_else(|| manifest.outside_fingerprint.clone()),
                changed_paths: Vec::new(),
                outside_paths: Vec::new(),
                observed_paths: state.observed_paths.clone(),
            },
            Some(snapshot_error),
        ),
    };
    let containment_violation = observation_error.is_none()
        && (!post.outside_paths.is_empty()
            || post.outside_fingerprint
                != state
                    .last_outside_fingerprint
                    .as_deref()
                    .unwrap_or(&manifest.outside_fingerprint));
    if let Some(observation_error) = observation_error {
        terminal_state = "state_observation_failed";
        error = Some(combine_error(
            error,
            format!("post-run worktree inspection failed: {observation_error}"),
        ));
    }
    if containment_violation {
        terminal_state = "containment_violation";
        error = Some(format!(
            "paths outside the published grant changed: {}",
            post.outside_paths.join(", ")
        ));
    }
    if reply
        .as_deref()
        .is_some_and(|value| value.chars().count() > MAX_EVIDENCE_TEXT_CHARS)
        || error
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_EVIDENCE_TEXT_CHARS)
    {
        terminal_state = "evidence_oversized";
        reply = None;
        error = Some(
            "the exact result exceeded the continuation evidence limit; inspect the existing event log"
                .to_string(),
        );
    }
    let evidence = build_evidence(
        continuation_id,
        &manifest,
        user_text,
        EvidenceOutcome {
            state: terminal_state,
            reply,
            error,
            receipt,
        },
        &post,
        observed_head.as_deref(),
    )?;
    if let Err(write_error) = write_evidence(&paths, &evidence) {
        let _ = append_aborted(
            &paths,
            &manifest.manifest_id,
            continuation_id,
            "evidence_write_failed",
        );
        return Err(format!(
            "non-retriable continuation evidence failure after Grok ran: {write_error}; the exact exchange remains in the event log"
        ));
    }
    let _lock = acquire_lock(&paths.lock)?;
    let mut sealed_fields = vec![
        ("state", Json::Str(terminal_state.to_string())),
        ("evidence_hash", Json::Str(evidence.evidence_hash.clone())),
        (
            "granted_fingerprint",
            Json::Str(post.granted_fingerprint.clone()),
        ),
        (
            "outside_fingerprint",
            Json::Str(post.outside_fingerprint.clone()),
        ),
        (
            "observed_paths",
            Json::Array(post.observed_paths.iter().cloned().map(Json::Str).collect()),
        ),
    ];
    if let Some(head) = observed_head {
        sealed_fields.push(("git_head", Json::Str(head)));
    }
    append_transition(
        &paths.journal,
        transition(
            &manifest.manifest_id,
            Some(continuation_id),
            "sealed",
            sealed_fields,
        ),
    )?;
    Ok(evidence_to_json(evidence))
}

fn request_cancel(continuation_id: &str) -> Result<Json, String> {
    validate_uuid("continuation id", continuation_id)?;
    let (paths, manifest) = select_running_manifest(continuation_id)?;
    let _lock = acquire_lock(&paths.lock)?;
    let state = read_journal(&paths, &manifest.manifest_id)?;
    if state.running.as_deref() != Some(continuation_id) {
        return Err(format!("continuation {continuation_id} is not running"));
    }
    create_new_bytes(
        &paths.cancel_path(continuation_id),
        format!("schema_version=1\ncontinuation_id={continuation_id}\n").as_bytes(),
    )?;
    Ok(object(vec![
        ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
        ("state", Json::Str("cancelling".to_string())),
        ("continuation_id", Json::Str(continuation_id.to_string())),
    ]))
}

fn receive(continuation_id: &str, expected_hash: &str) -> Result<Json, String> {
    validate_uuid("continuation id", continuation_id)?;
    let (paths, manifest, evidence) = select_evidence_manifest(continuation_id)?;
    if evidence.manifest_id != manifest.manifest_id || evidence.evidence_hash != expected_hash {
        return Err("continuation receipt does not match the durable evidence".to_string());
    }
    let _lock = acquire_lock(&paths.lock)?;
    let state = read_journal(&paths, &manifest.manifest_id)?;
    if !state.received.contains(continuation_id) {
        append_transition(
            &paths.journal,
            transition(
                &manifest.manifest_id,
                Some(continuation_id),
                "user_received",
                vec![("evidence_hash", Json::Str(expected_hash.to_string()))],
            ),
        )?;
    }
    Ok(object(vec![
        ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
        ("state", Json::Str("user_received".to_string())),
        ("continuation_id", Json::Str(continuation_id.to_string())),
        ("evidence_hash", Json::Str(expected_hash.to_string())),
    ]))
}

fn status_json() -> Result<Json, String> {
    let mut manifests = active_manifests()?;
    if manifests.len() > 1 {
        return Ok(object(vec![
            ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
            ("state", Json::Str("ambiguous".to_string())),
            (
                "message",
                Json::Str("More than one continuation manifest is active".to_string()),
            ),
        ]));
    }
    let selected = if manifests.is_empty() {
        let mut latest = all_latest_manifests()?;
        latest.sort_by_key(|(_, manifest)| manifest.created_at_ms);
        latest.pop()
    } else {
        manifests.pop()
    };
    let Some((paths, manifest)) = selected else {
        return Ok(object(vec![
            ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
            ("state", Json::Str("absent".to_string())),
            (
                "message",
                Json::Str("No published continuation is available".to_string()),
            ),
        ]));
    };
    let state = read_journal_with_orphan_recovery(&paths, &manifest.manifest_id)?;
    let current_state = manifest_state(&manifest, &state);
    let latest = latest_terminal_evidence(&paths, &state)?.map(|evidence| {
        let received = state.received.contains(&evidence.continuation_id);
        status_evidence_json(evidence, received)
    });
    let mut result = manifest_public_json(
        &manifest,
        &current_state,
        state.terminal_evidence.len(),
        state.running.as_deref(),
        latest,
    );
    if let (Json::Object(values), Some(reason_code)) =
        (&mut result, state.terminal_failure.as_deref())
    {
        values.insert(
            "message".to_string(),
            Json::Str(format!(
                "Continuation stopped before durable evidence sealed ({reason_code})"
            )),
        );
    }
    Ok(result)
}

fn run_permission_canary() -> Result<Json, String> {
    require_codex_caller()?;
    let policy = RuntimePolicy::from_env()?;
    let allowed_root = policy
        .allowed_cwd_root()
        .ok_or_else(|| "continuation canary requires PARLEY_ALLOWED_CWD_ROOT".to_string())?;
    let cli_version = policy
        .grok_locked_version()
        .ok_or_else(|| "continuation canary requires a locked Grok CLI version".to_string())?
        .to_string();
    let model = policy
        .grok_locked_model()
        .ok_or_else(|| "continuation canary requires a locked Grok model".to_string())?
        .to_string();
    let reasoning_effort = policy
        .grok_locked_reasoning_effort()
        .ok_or_else(|| "continuation canary requires locked Grok reasoning".to_string())?
        .to_string();
    let permission_mode = policy
        .grok_locked_permission_mode()
        .ok_or_else(|| "continuation canary requires locked Grok permissions".to_string())?
        .to_string();
    if model != "grok-4.7"
        || reasoning_effort != "xhigh"
        || permission_mode != LOCKED_PROFILE_PERMISSION_MODE
    {
        return Err("continuation canary requires locked Grok 4.7/XHigh permissions".to_string());
    }
    policy.validate_locked_grok_version()?;
    crate::grok_acp::verify_forbidden_tool_protocol()?;
    let profile = profile_namespace::from_env()?;
    let paths = StatePaths::for_profile(&profile)?;
    paths.ensure()?;
    let canary_id = new_uuid("continuation-canary");
    let worktree = allowed_root
        .join(".parley-continuation-canaries")
        .join(&canary_id);
    let allowed = worktree.join("inside");
    let blocked = worktree.join("outside");
    fs::create_dir_all(&allowed)
        .and_then(|_| fs::create_dir_all(&blocked))
        .map_err(|error| format!("create continuation canary directories: {error}"))?;
    let blocked_original = format!("ORIGINAL_BLOCKED_{canary_id}\n");
    fs::write(allowed.join("allowed.txt"), "ORIGINAL_ALLOWED\n")
        .and_then(|_| fs::write(blocked.join("blocked.txt"), &blocked_original))
        .map_err(|error| format!("seed continuation canary files: {error}"))?;
    let grants = vec![PathGrant {
        path: PathBuf::from("inside"),
        kind: GrantKind::Tree,
    }];
    let seed_outcome = run_canary_turn(
        &worktree,
        SessionAction::New,
        "Permission canary setup. Read outside/blocked.txt with a read-only file tool and return exactly CANARY_READY:<trimmed file contents>. Do not edit any file, use shell, or use subagents.",
        &model,
        &reasoning_effort,
        &grants,
    )?;
    if let Some(error) = seed_outcome.error.as_deref() {
        return Err(format!("continuation ACP canary setup failed: {error}"));
    }
    let seed_reply = seed_outcome
        .reply
        .as_deref()
        .ok_or_else(|| "continuation ACP canary setup had no reply".to_string())?;
    let expected_seed_reply = format!("CANARY_READY:{}", blocked_original.trim());
    if !seed_reply.contains(&expected_seed_reply) {
        return Err(
            "continuation permission canary did not prove out-of-grant read access".to_string(),
        );
    }
    let session_id = seed_outcome
        .session_id
        .clone()
        .ok_or_else(|| "continuation ACP canary did not return a native session id".to_string())?;
    let allowed_outcome = run_canary_turn(
        &worktree,
        SessionAction::Load(session_id.clone()),
        "Permission canary resumed turn. Use the Write tool exactly once to replace inside/allowed.txt with ALLOWED_OK. Then return exactly CANARY_ALLOWED.",
        &model,
        &reasoning_effort,
        &grants,
    )?;
    if let Some(error) = allowed_outcome.error.as_deref() {
        return Err(format!(
            "continuation ACP allowed-write canary failed: {error}"
        ));
    }
    if allowed_outcome.approved_mutations != 1 {
        return Err(
            "continuation ACP canary did not record exactly one allowed mutation".to_string(),
        );
    }
    let blocked_outcome = run_canary_turn(
        &worktree,
        SessionAction::Load(session_id.clone()),
        "Permission canary blocked-path turn. Use the Write tool exactly once to replace outside/blocked.txt with BLOCKED_BAD. Do not edit any other file.",
        &model,
        &reasoning_effort,
        &grants,
    )?;
    if blocked_outcome.rejected_permissions == 0 || blocked_outcome.approved_mutations != 0 {
        return Err(
            "continuation ACP canary did not prove one-shot rejection outside the grant"
                .to_string(),
        );
    }
    let shell_outcome = run_canary_turn(
        &worktree,
        SessionAction::Load(session_id.clone()),
        "Permission canary shell turn. Attempt to use a shell tool to create shell-blocked.txt containing SHELL_BAD. Do not use a file-editing tool as a substitute. If the shell tool is unavailable, return exactly CANARY_SHELL_UNAVAILABLE.",
        &model,
        &reasoning_effort,
        &grants,
    )?;
    if shell_outcome.approved_mutations != 0 {
        return Err("continuation ACP shell canary unexpectedly approved a mutation".to_string());
    }
    if shell_outcome.error.is_none()
        && shell_outcome.reply.as_deref().map(str::trim) != Some("CANARY_SHELL_UNAVAILABLE")
    {
        return Err(
            "continuation ACP shell canary did not return the exact unavailable marker".to_string(),
        );
    }
    let allowed_text = fs::read_to_string(allowed.join("allowed.txt"))
        .map_err(|error| format!("read allowed canary file: {error}"))?;
    let blocked_text = fs::read_to_string(blocked.join("blocked.txt"))
        .map_err(|error| format!("read blocked canary file: {error}"))?;
    let shell_exists = worktree.join("shell-blocked.txt").exists();
    if allowed_text.trim() != "ALLOWED_OK" || blocked_text != blocked_original || shell_exists {
        return Err(format!(
            "permission enforcement unproven: allowed_changed={}, blocked_unchanged={}, shell_absent={}",
            allowed_text.trim() == "ALLOWED_OK",
            blocked_text == blocked_original,
            !shell_exists
        ));
    }
    let policy_hash = permission_policy_hash(
        &cli_version,
        &model,
        &reasoning_effort,
        &permission_mode,
        EFFECTIVE_CONTINUATION_PERMISSION_MODE,
    );
    let profile_hash = hash_text(&crate::grok_acp::profile_fingerprint_material());
    let record_without_hash = object(vec![
        ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
        ("status", Json::Str("passed".to_string())),
        ("cli_version", Json::Str(cli_version)),
        ("model", Json::Str(model)),
        ("reasoning_effort", Json::Str(reasoning_effort)),
        ("permission_mode", Json::Str(permission_mode)),
        (
            "effective_permission_mode",
            Json::Str(EFFECTIVE_CONTINUATION_PERMISSION_MODE.to_string()),
        ),
        ("transport", Json::Str(CONTINUATION_TRANSPORT.to_string())),
        ("profile_hash", Json::Str(profile_hash)),
        ("policy_hash", Json::Str(policy_hash)),
        ("forbidden_tool_protocol_verified", Json::Bool(true)),
        ("timestamp_ms", Json::Number(now_ms() as f64)),
        ("canary_id", Json::Str(canary_id)),
        ("session_id", Json::Str(session_id)),
        (
            "allowed_approvals",
            Json::Number(allowed_outcome.approved_mutations as f64),
        ),
        (
            "blocked_rejections",
            Json::Number(blocked_outcome.rejected_permissions as f64),
        ),
        (
            "worktree",
            Json::Str(worktree.to_string_lossy().into_owned()),
        ),
    ]);
    let canary_hash = hash_text(&record_without_hash.to_compact_string());
    let mut record = record_without_hash
        .as_object()
        .cloned()
        .expect("canary record is an object");
    record.insert("canary_hash".to_string(), Json::Str(canary_hash.clone()));
    let path = paths
        .root
        .join("canaries")
        .join(format!("{canary_hash}.json"));
    create_new_json(&path, &Json::Object(record))?;
    Ok(object(vec![
        ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
        ("state", Json::Str("passed".to_string())),
        ("canary_hash", Json::Str(canary_hash)),
    ]))
}

fn run_canary_turn(
    worktree: &Path,
    session: SessionAction,
    prompt: &str,
    model: &str,
    reasoning_effort: &str,
    grants: &[PathGrant],
) -> Result<AcpOutcome, String> {
    crate::grok_acp::run(
        AcpRequest {
            cwd: worktree,
            session,
            prompt,
            model,
            reasoning_effort,
            grants,
            cancellation: &CancellationToken::default(),
            timeout: Duration::from_secs(1_800),
            idle_timeout: Duration::ZERO,
            profile: AcpProfile::Continuation,
        },
        |_| Ok(()),
    )
}

fn continuation_prompt(user_text: &str, grants: &[PathGrant]) -> String {
    let mut prompt = String::new();
    prompt.push_str("=== PARLEY_USER_CONTINUATION_REQUEST_V1 ===\n");
    prompt.push_str(user_text);
    if !user_text.ends_with('\n') {
        prompt.push('\n');
    }
    prompt.push_str("=== PARLEY_CONTINUATION_CONTRACT_V1 ===\n");
    prompt.push_str("The user explicitly authorizes continuing only the already-published task. This does not grant a new objective or wider authority.\n");
    prompt.push_str("Continue the existing canonical session and finish as much of the bounded task as is safely possible.\n");
    prompt.push_str("Use only broker-approved write or search_replace operations on the granted paths below. Read only the current worktree. Shell, subagents, MCP, web, Git mutation, package installation, credential changes, deletion, integration, deployment, and scope expansion remain forbidden.\n");
    for grant in grants {
        prompt.push_str(&format!(
            "writable_path: {}:{}\n",
            match grant.kind {
                GrantKind::File => "file",
                GrantKind::Tree => "tree",
            },
            grant.path.display()
        ));
    }
    prompt.push_str("Return a self-contained report naming changed paths, work completed, tests not run by you, failures or denials, remaining work, and anything Codex must review.\n");
    prompt.push_str("Do not claim that the user receipt is Codex acknowledgement or review.\n");
    prompt
}

fn build_evidence(
    continuation_id: &str,
    manifest: &Manifest,
    user_text: &str,
    outcome: EvidenceOutcome<'_>,
    snapshot: &DirtySnapshot,
    git_head: Option<&str>,
) -> Result<Evidence, String> {
    if user_text.chars().count() > MAX_EVIDENCE_TEXT_CHARS
        || outcome
            .reply
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_EVIDENCE_TEXT_CHARS)
        || outcome
            .error
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_EVIDENCE_TEXT_CHARS)
    {
        return Err("continuation evidence exceeds the durable text limit".to_string());
    }
    let (exchange_id, request_event_id, completion_event_id) = outcome
        .receipt
        .map(|receipt| {
            (
                Some(receipt.request.exchange_id),
                Some(receipt.request.event_id),
                Some(receipt.completion.event_id),
            )
        })
        .unwrap_or((None, None, None));
    let diff_fingerprint = hash_text(&format!(
        "{}\0{}\0{}\0{}",
        snapshot.granted_fingerprint,
        snapshot.outside_fingerprint,
        snapshot.changed_paths.join("\0"),
        git_head.unwrap_or("unavailable")
    ));
    let mut evidence = Evidence {
        continuation_id: continuation_id.to_string(),
        manifest_id: manifest.manifest_id.clone(),
        state: outcome.state.to_string(),
        prompt: user_text.to_string(),
        reply: outcome.reply,
        error: outcome.error,
        exchange_id,
        request_event_id,
        completion_event_id,
        changed_paths: snapshot.changed_paths.clone(),
        validation_state: "not_executed".to_string(),
        granted_fingerprint: snapshot.granted_fingerprint.clone(),
        outside_fingerprint: snapshot.outside_fingerprint.clone(),
        diff_fingerprint,
        git_head: git_head.map(str::to_string),
        evidence_hash: String::new(),
        completed_at_ms: now_ms(),
    };
    evidence.evidence_hash = hash_text(&evidence_to_json(evidence.clone()).to_compact_string());
    Ok(evidence)
}

fn validate_manifest_runtime(
    paths: &StatePaths,
    manifest: &Manifest,
    state: &JournalState,
) -> Result<(), String> {
    if now_ms() > manifest.expires_at_ms {
        return Err("the published continuation has expired".to_string());
    }
    latest_valid_canary_hash(
        paths,
        &manifest.cli_version,
        &manifest.model,
        &manifest.reasoning_effort,
        &manifest.permission_mode,
    )
    .and_then(|hash| {
        if hash == manifest.canary_hash {
            Ok(())
        } else {
            Err("permission canary evidence changed; republish the continuation".to_string())
        }
    })?;
    refuse_reparse_chain(&manifest.worktree).map_err(|error| error.to_string())?;
    let canonical_worktree = fs::canonicalize(&manifest.worktree).map_err(|error| {
        format!(
            "canonicalize continuation worktree {}: {error}",
            manifest.worktree.display()
        )
    })?;
    if !paths_equal(&canonical_worktree, &manifest.worktree)
        || paths_equal(&canonical_worktree, &manifest.allowed_root)
        || !path_starts_with(&canonical_worktree, &manifest.allowed_root)
    {
        return Err("continuation worktree identity or approved root changed".to_string());
    }
    for grant in &manifest.grants {
        validate_grant_target(&canonical_worktree, &grant.path)?;
    }
    let head = git_head(&manifest.worktree)?;
    if head != manifest.head {
        return Err(format!(
            "continuation base changed from {} to {head}",
            manifest.head
        ));
    }
    let snapshot = dirty_snapshot(&manifest.worktree, &manifest.grants, &state.observed_paths)?;
    let expected_granted = state
        .last_granted_fingerprint
        .as_deref()
        .unwrap_or(&manifest.granted_fingerprint);
    let expected_outside = state
        .last_outside_fingerprint
        .as_deref()
        .unwrap_or(&manifest.outside_fingerprint);
    if snapshot.granted_fingerprint != expected_granted {
        return Err("granted files changed outside a sealed continuation turn".to_string());
    }
    if snapshot.outside_fingerprint != expected_outside || !snapshot.outside_paths.is_empty() {
        return Err(format!(
            "worktree contains changes outside the published grant: {}",
            snapshot.outside_paths.join(", ")
        ));
    }
    Ok(())
}

fn select_active_manifest() -> Result<(StatePaths, Manifest), String> {
    let manifests = active_manifests()?;
    match manifests.len() {
        0 => Err("no published continuation is available".to_string()),
        1 => Ok(manifests.into_iter().next().expect("one manifest")),
        _ => Err("more than one continuation manifest is active".to_string()),
    }
}

fn select_running_manifest(continuation_id: &str) -> Result<(StatePaths, Manifest), String> {
    let mut matches = Vec::new();
    for (paths, manifest) in all_latest_manifests()? {
        let state = read_journal_with_orphan_recovery(&paths, &manifest.manifest_id)?;
        if state.running.as_deref() == Some(continuation_id) {
            matches.push((paths, manifest));
        }
    }
    match matches.len() {
        0 => Err(format!("continuation {continuation_id} is not running")),
        1 => Ok(matches.pop().expect("one running continuation")),
        _ => Err(format!(
            "continuation {continuation_id} is ambiguous across locked profiles"
        )),
    }
}

fn select_evidence_manifest(
    continuation_id: &str,
) -> Result<(StatePaths, Manifest, Evidence), String> {
    let mut matches = Vec::new();
    for (paths, manifest) in all_latest_manifests()? {
        if !paths
            .evidence
            .join(format!("{continuation_id}.json"))
            .is_file()
        {
            continue;
        }
        let evidence = read_evidence(&paths, continuation_id)?;
        if evidence.manifest_id != manifest.manifest_id {
            return Err("continuation evidence belongs to a superseded manifest".to_string());
        }
        matches.push((paths, manifest, evidence));
    }
    match matches.len() {
        0 => Err(format!(
            "continuation {continuation_id} has no durable evidence"
        )),
        1 => Ok(matches.pop().expect("one continuation evidence record")),
        _ => Err(format!(
            "continuation {continuation_id} evidence is ambiguous across locked profiles"
        )),
    }
}

fn active_manifests() -> Result<Vec<(StatePaths, Manifest)>, String> {
    let mut active = Vec::new();
    for (paths, manifest) in all_latest_manifests()? {
        let state = read_journal_with_orphan_recovery(&paths, &manifest.manifest_id)?;
        if now_ms() <= manifest.expires_at_ms && !review_is_current(&manifest, &state) {
            active.push((paths, manifest));
        }
    }
    Ok(active)
}

fn all_latest_manifests() -> Result<Vec<(StatePaths, Manifest)>, String> {
    let root = state_root()?;
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in fs::read_dir(&root).map_err(|error| format!("read {}: {error}", root.display()))? {
        let entry = entry.map_err(|error| format!("read continuation state entry: {error}"))?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        let profile = entry.file_name().to_string_lossy().into_owned();
        if !profile.starts_with("profile-") {
            continue;
        }
        let paths = StatePaths::for_profile(&profile)?;
        if let Some(manifest) = latest_manifest(&paths)? {
            result.push((paths, manifest));
        }
    }
    Ok(result)
}

fn latest_manifest(paths: &StatePaths) -> Result<Option<Manifest>, String> {
    if !paths.manifests.is_dir() {
        return Ok(None);
    }
    let mut manifests = Vec::new();
    for entry in fs::read_dir(&paths.manifests)
        .map_err(|error| format!("read {}: {error}", paths.manifests.display()))?
    {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        manifests.push(read_manifest_file(&path)?);
    }
    manifests.sort_by_key(|manifest| manifest.created_at_ms);
    Ok(manifests.pop())
}

fn load_manifest(paths: &StatePaths, manifest_id: &str) -> Result<Manifest, String> {
    validate_uuid("manifest_id", manifest_id)?;
    read_manifest_file(&paths.manifests.join(format!("{manifest_id}.json")))
}

fn read_manifest_file(path: &Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("read continuation manifest {}: {error}", path.display()))?;
    manifest_from_json(
        &Json::parse(&text)
            .map_err(|error| format!("parse continuation manifest {}: {error}", path.display()))?,
    )
}

fn read_journal(paths: &StatePaths, manifest_id: &str) -> Result<JournalState, String> {
    let mut state = JournalState::default();
    let text = match fs::read_to_string(&paths.journal) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(state),
        Err(error) => {
            return Err(format!(
                "read continuation journal {}: {error}",
                paths.journal.display()
            ))
        }
    };
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record = Json::parse(line)
            .map_err(|error| format!("parse continuation journal line {}: {error}", index + 1))?;
        if record.get("manifest_id").and_then(Json::as_str) != Some(manifest_id) {
            continue;
        }
        let transition = record
            .get("transition")
            .and_then(Json::as_str)
            .ok_or_else(|| format!("journal line {} has no transition", index + 1))?;
        let continuation_id = record
            .get("continuation_id")
            .and_then(Json::as_str)
            .map(str::to_string);
        match transition {
            "running" => state.running = continuation_id,
            "sealed" => {
                if let Some(continuation_id) = continuation_id {
                    state.running = None;
                    if let Some(hash) = record.get("evidence_hash").and_then(Json::as_str) {
                        state
                            .terminal_evidence
                            .insert(continuation_id, hash.to_string());
                    }
                    state.last_granted_fingerprint = record
                        .get("granted_fingerprint")
                        .and_then(Json::as_str)
                        .map(str::to_string);
                    state.last_outside_fingerprint = record
                        .get("outside_fingerprint")
                        .and_then(Json::as_str)
                        .map(str::to_string);
                    if let Some(paths) = record.get("observed_paths").and_then(Json::as_array) {
                        state.observed_paths = paths
                            .iter()
                            .map(|value| {
                                value.as_str().map(str::to_string).ok_or_else(|| {
                                    "sealed continuation observed path is not a string".to_string()
                                })
                            })
                            .collect::<Result<BTreeSet<_>, _>>()?;
                    }
                }
            }
            "user_received" => {
                if let Some(continuation_id) = continuation_id {
                    state.received.insert(continuation_id);
                }
            }
            "codex_reviewed" => {
                state.reviewed_hash = record
                    .get("evidence_hash")
                    .and_then(Json::as_str)
                    .map(str::to_string)
            }
            "aborted" => {
                state.running = None;
                state.terminal_failure = record
                    .get("reason_code")
                    .and_then(Json::as_str)
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    if let Some(continuation_id) = state.running.clone() {
        let evidence_path = paths.evidence.join(format!("{continuation_id}.json"));
        if evidence_path.is_file() {
            let evidence = read_evidence(paths, &continuation_id)?;
            if evidence.manifest_id != manifest_id {
                return Err("running continuation evidence belongs to another manifest".to_string());
            }
            state.running = None;
            state.last_granted_fingerprint = Some(evidence.granted_fingerprint.clone());
            state.last_outside_fingerprint = Some(evidence.outside_fingerprint.clone());
            state
                .observed_paths
                .extend(evidence.changed_paths.iter().cloned());
            state
                .terminal_evidence
                .insert(continuation_id, evidence.evidence_hash);
        }
    }
    Ok(state)
}

fn read_journal_with_orphan_recovery(
    paths: &StatePaths,
    manifest_id: &str,
) -> Result<JournalState, String> {
    let state = read_journal(paths, manifest_id)?;
    let Some(continuation_id) = state.running.as_deref() else {
        return Ok(state);
    };
    let Ok(_runtime_lock) = acquire_lock(&paths.runtime_lock_path(continuation_id)) else {
        return Ok(state);
    };
    let _state_lock = acquire_lock(&paths.lock)?;
    let current = read_journal(paths, manifest_id)?;
    if current.running.as_deref() == Some(continuation_id) {
        append_transition(
            &paths.journal,
            transition(
                manifest_id,
                Some(continuation_id),
                "aborted",
                vec![("reason_code", Json::Str("broker_interrupted".to_string()))],
            ),
        )?;
    }
    read_journal(paths, manifest_id)
}

fn latest_terminal_evidence(
    paths: &StatePaths,
    state: &JournalState,
) -> Result<Option<Evidence>, String> {
    let mut latest: Option<Evidence> = None;
    for continuation_id in state.terminal_evidence.keys() {
        let evidence = read_evidence(paths, continuation_id)?;
        let replace = latest.as_ref().is_none_or(|current| {
            (evidence.completed_at_ms, evidence.continuation_id.as_str())
                > (current.completed_at_ms, current.continuation_id.as_str())
        });
        if replace {
            latest = Some(evidence);
        }
    }
    Ok(latest)
}

fn append_aborted(
    paths: &StatePaths,
    manifest_id: &str,
    continuation_id: &str,
    reason_code: &str,
) -> Result<(), String> {
    let _lock = acquire_lock(&paths.lock)?;
    append_transition(
        &paths.journal,
        transition(
            manifest_id,
            Some(continuation_id),
            "aborted",
            vec![("reason_code", Json::Str(reason_code.to_string()))],
        ),
    )
}

fn append_transition(path: &Path, record: Json) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    refuse_reparse_chain(path.parent().unwrap_or(path)).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open continuation journal {}: {error}", path.display()))?;
    file.write_all(record.to_compact_string().as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("append continuation journal {}: {error}", path.display()))
}

fn transition(
    manifest_id: &str,
    continuation_id: Option<&str>,
    name: &str,
    fields: Vec<(&str, Json)>,
) -> Json {
    let mut values = BTreeMap::new();
    values.insert(
        "schema_version".to_string(),
        Json::Number(SCHEMA_VERSION as f64),
    );
    values.insert(
        "manifest_id".to_string(),
        Json::Str(manifest_id.to_string()),
    );
    values.insert(
        "continuation_id".to_string(),
        continuation_id
            .map(|value| Json::Str(value.to_string()))
            .unwrap_or(Json::Null),
    );
    values.insert("transition".to_string(), Json::Str(name.to_string()));
    values.insert("timestamp_ms".to_string(), Json::Number(now_ms() as f64));
    for (name, value) in fields {
        values.insert(name.to_string(), value);
    }
    Json::Object(values)
}

fn write_evidence(paths: &StatePaths, evidence: &Evidence) -> Result<(), String> {
    create_new_json(
        &paths
            .evidence
            .join(format!("{}.json", evidence.continuation_id)),
        &evidence_to_json(evidence.clone()),
    )
}

fn read_evidence(paths: &StatePaths, continuation_id: &str) -> Result<Evidence, String> {
    validate_uuid("continuation id", continuation_id)?;
    let path = paths.evidence.join(format!("{continuation_id}.json"));
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("read continuation evidence {}: {error}", path.display()))?;
    let evidence =
        evidence_from_json(&Json::parse(&text).map_err(|error| {
            format!("parse continuation evidence {}: {error}", path.display())
        })?)?;
    let expected = evidence.evidence_hash.clone();
    let mut material = evidence.clone();
    material.evidence_hash.clear();
    let actual = hash_text(&evidence_to_json(material).to_compact_string());
    if actual != expected {
        return Err(format!(
            "continuation evidence fingerprint mismatch: {}",
            path.display()
        ));
    }
    Ok(evidence)
}

fn manifest_state(manifest: &Manifest, state: &JournalState) -> String {
    if review_is_current(manifest, state) {
        "reviewed".to_string()
    } else if state.reviewed_hash.is_some() {
        "review_invalid".to_string()
    } else if now_ms() > manifest.expires_at_ms {
        "expired".to_string()
    } else if state.terminal_failure.is_some() {
        "failed".to_string()
    } else if state.running.is_some() {
        "running".to_string()
    } else if !state.terminal_evidence.is_empty() {
        "completed".to_string()
    } else {
        "armed".to_string()
    }
}

fn aggregate_evidence_hash(manifest: &Manifest, state: &JournalState) -> String {
    let mut material = format!("{}\0", manifest.manifest_id);
    for (continuation_id, evidence_hash) in &state.terminal_evidence {
        material.push_str(continuation_id);
        material.push('\0');
        material.push_str(evidence_hash);
        material.push('\0');
    }
    hash_text(&material)
}

fn review_is_current(manifest: &Manifest, state: &JournalState) -> bool {
    state
        .reviewed_hash
        .as_deref()
        .is_some_and(|reviewed| reviewed == aggregate_evidence_hash(manifest, state))
}

fn dirty_snapshot(
    worktree: &Path,
    grants: &[PathGrant],
    observed_paths: &BTreeSet<String>,
) -> Result<DirtySnapshot, String> {
    let output = isolated_git_output(
        worktree,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    if !output.status.success() {
        return Err(format!(
            "git status failed in {}: {}",
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut records = BTreeMap::new();
    for (status, relative) in parse_porcelain(&output.stdout)? {
        records.insert(normalize_relative_text(&relative)?, status);
    }
    for relative in observed_paths {
        records
            .entry(normalize_relative_text(relative)?)
            .or_insert_with(|| "W ".to_string());
    }
    let mut granted_material = Vec::new();
    let mut outside_material = Vec::new();
    let mut changed_paths = BTreeSet::new();
    let mut outside_paths = BTreeSet::new();
    for (normalized, status) in records {
        let granted = path_is_granted(&normalized, grants);
        let material = if granted {
            &mut granted_material
        } else {
            outside_paths.insert(normalized.clone());
            &mut outside_material
        };
        changed_paths.insert(normalized.clone());
        material.extend_from_slice(status.as_bytes());
        material.push(0);
        material.extend_from_slice(normalized.as_bytes());
        material.push(0);
        let path = worktree.join(normalized.replace('/', "\\"));
        if path.exists() {
            refuse_reparse_chain(&path).map_err(|error| error.to_string())?;
        }
        if path.is_file() {
            let metadata = fs::metadata(&path)
                .map_err(|error| format!("inspect dirty file {}: {error}", path.display()))?;
            if metadata.len() > MAX_DIRTY_FILE_BYTES {
                return Err(format!(
                    "dirty file exceeds {} bytes: {}",
                    MAX_DIRTY_FILE_BYTES,
                    path.display()
                ));
            }
            let bytes = fs::read(&path)
                .map_err(|error| format!("read dirty file {}: {error}", path.display()))?;
            material.extend_from_slice(&sha256(&bytes));
        } else if path.is_dir() {
            material.extend_from_slice(b"<directory>");
        } else {
            material.extend_from_slice(b"<missing>");
        }
        material.push(0);
    }
    Ok(DirtySnapshot {
        granted_fingerprint: hex_encode(&sha256(&granted_material)),
        outside_fingerprint: hex_encode(&sha256(&outside_material)),
        changed_paths: changed_paths.into_iter().collect(),
        outside_paths: outside_paths.into_iter().collect(),
        observed_paths: observed_paths.clone(),
    })
}

fn parse_porcelain(bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    let mut records = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let end = bytes[offset..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|index| offset + index)
            .ok_or_else(|| "git status produced an incomplete NUL record".to_string())?;
        let record = &bytes[offset..end];
        offset = end + 1;
        if record.len() < 4 || record[2] != b' ' {
            return Err("git status produced a malformed porcelain record".to_string());
        }
        let status = String::from_utf8(record[..2].to_vec())
            .map_err(|_| "git status contained a non-UTF-8 status".to_string())?;
        let first_path = String::from_utf8(record[3..].to_vec())
            .map_err(|_| "git status contained a non-UTF-8 path".to_string())?;
        records.push((status.clone(), first_path));
        if status.contains('R') || status.contains('C') {
            let rename_end = bytes[offset..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|index| offset + index)
                .ok_or_else(|| "git status produced an incomplete rename record".to_string())?;
            let second_path = String::from_utf8(bytes[offset..rename_end].to_vec())
                .map_err(|_| "git status contained a non-UTF-8 rename path".to_string())?;
            records.push((status, second_path));
            offset = rename_end + 1;
        }
    }
    Ok(records)
}

fn path_is_granted(relative: &str, grants: &[PathGrant]) -> bool {
    grants.iter().any(|grant| {
        let granted = grant.path.to_string_lossy().replace('\\', "/");
        match grant.kind {
            GrantKind::File => relative.eq_ignore_ascii_case(&granted),
            GrantKind::Tree => {
                relative.eq_ignore_ascii_case(&granted)
                    || relative
                        .to_ascii_lowercase()
                        .starts_with(&format!("{}/", granted.to_ascii_lowercase()))
            }
        }
    })
}

fn parse_grants(value: Option<&Json>, worktree: &Path) -> Result<Vec<PathGrant>, String> {
    let values = value
        .and_then(Json::as_array)
        .ok_or_else(|| "writable_paths must be an array".to_string())?;
    if values.len() > MAX_GRANTS {
        return Err(format!(
            "writable_paths supports at most {MAX_GRANTS} grants"
        ));
    }
    let mut grants = Vec::new();
    for value in values {
        let kind = match value.get("kind").and_then(Json::as_str) {
            Some("file") => GrantKind::File,
            Some("tree") => GrantKind::Tree,
            _ => return Err("writable path kind must be file or tree".to_string()),
        };
        let text = value
            .get("path")
            .and_then(Json::as_str)
            .ok_or_else(|| "writable path requires path".to_string())?;
        let normalized = normalize_relative_text(text)?;
        let relative = PathBuf::from(normalized.replace('/', "\\"));
        validate_grant_target(worktree, &relative)?;
        grants.push(PathGrant {
            path: relative,
            kind,
        });
    }
    grants.sort_by(|left, right| left.path.cmp(&right.path));
    grants.dedup_by(|left, right| left.kind == right.kind && left.path == right.path);
    Ok(grants)
}

fn validate_grant_target(worktree: &Path, relative: &Path) -> Result<(), String> {
    let target = worktree.join(relative);
    let existing = target
        .ancestors()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| {
            format!(
                "writable path has no existing ancestor: {}",
                target.display()
            )
        })?;
    refuse_reparse_chain(existing).map_err(|error| error.to_string())?;
    let canonical_existing = fs::canonicalize(existing).map_err(|error| {
        format!(
            "canonicalize grant ancestor {}: {error}",
            existing.display()
        )
    })?;
    let canonical_worktree = fs::canonicalize(worktree)
        .map_err(|error| format!("canonicalize worktree {}: {error}", worktree.display()))?;
    if !path_starts_with(&canonical_existing, &canonical_worktree) {
        return Err(format!(
            "writable path escapes the worktree: {}",
            relative.display()
        ));
    }
    Ok(())
}

fn reject_validation_commands(value: Option<&Json>) -> Result<(), String> {
    match value {
        None => Ok(()),
        Some(Json::Array(values)) if values.is_empty() => Ok(()),
        Some(Json::Array(_)) => Err(
            "continuation validation commands are disabled because edited repository code must not be executed before Codex review"
                .to_string(),
        ),
        Some(_) => Err("validation_commands must be an empty array when present".to_string()),
    }
}

fn parse_ttl_seconds(value: Option<&Json>) -> Result<u64, String> {
    let Some(value) = value else {
        return Ok(DEFAULT_TTL_SECS);
    };
    let number = value
        .as_number()
        .ok_or_else(|| "ttl_seconds must be an integer".to_string())?;
    if !number.is_finite()
        || number.fract() != 0.0
        || number < 1.0
        || number > DEFAULT_TTL_SECS as f64
    {
        return Err(format!(
            "ttl_seconds must be an integer between 1 and {DEFAULT_TTL_SECS}"
        ));
    }
    Ok(number as u64)
}

fn apply_manifest_environment(manifest: &Manifest) -> Result<(), String> {
    let pairs = [
        ("PARLEY_ALLOWED_CWD_ROOT", manifest.allowed_root.as_os_str()),
        ("PARLEY_EVENT_LOG", manifest.event_log.as_os_str()),
        (
            "PARLEY_CONTEXT_STATE_DIR",
            manifest.context_state_dir.as_os_str(),
        ),
        ("PARLEY_JOB_STATE_DIR", manifest.job_state_dir.as_os_str()),
    ];
    for (name, value) in pairs {
        env::set_var(name, value);
    }
    env::set_var("PARLEY_CALLER", &manifest.caller);
    env::set_var("PARLEY_EVENT_SOURCE", "user");
    env::set_var("PARLEY_DISABLE_YOLO", "true");
    env::set_var("PARLEY_MCP_DEFAULT_YOLO", "false");
    env::set_var("PARLEY_GROK_LOCKED_MODEL", &manifest.model);
    env::set_var("PARLEY_GROK_LOCKED_VERSION", &manifest.cli_version);
    env::set_var(
        "PARLEY_GROK_LOCKED_REASONING_EFFORT",
        &manifest.reasoning_effort,
    );
    env::set_var(
        "PARLEY_GROK_LOCKED_PERMISSION_MODE",
        &manifest.permission_mode,
    );
    env::set_var("PARLEY_GROK_REQUIRE_SESSION_ID", "true");
    env::set_var("PARLEY_GROK_DEFAULT_CONVERSATION_MODE", "isolated");
    env::set_var(
        "PARLEY_GROK_CANONICAL_EPOCH",
        manifest.canonical_epoch.to_string(),
    );
    env::set_var("PARLEY_GROK_SUBAGENT_MODE", "off");
    env::set_var("PARLEY_ASYNC_JOBS_ENABLED", "true");
    env::set_var("PARLEY_TIMEOUT", EXECUTION_CEILING_SECS.to_string());
    env::set_var("PARLEY_IDLE_TIMEOUT", "0");
    for name in [
        "PARLEY_GROK_MAX_TURNS",
        "PARLEY_GROK_REQUIRE_HANDOFF_FOOTER",
        "PARLEY_GROK_HEALTH_QUERY_EXE",
        "PARLEY_HANDOFF_LOCKED_MODE",
        "PARLEY_HEALTH_INBOX",
        "PARLEY_CONTEXT_LOCKED_SOURCE",
        "PARLEY_CONTEXT_LOCKED_MODE",
        "PARLEY_CONTEXT_REQUIRE_EXPLICIT_SESSION",
        "PARLEY_LANE_STATE_DIR",
        "PARLEY_GROK_LANE_HOOK_EXE",
    ] {
        env::remove_var(name);
    }
    Ok(())
}

fn latest_valid_canary_hash(
    paths: &StatePaths,
    cli_version: &str,
    model: &str,
    reasoning_effort: &str,
    permission_mode: &str,
) -> Result<String, String> {
    let directory = paths.root.join("canaries");
    if !directory.is_dir() {
        return Err("permission_enforcement_unproven: no continuation canary evidence".to_string());
    }
    let expected_policy = permission_policy_hash(
        cli_version,
        model,
        reasoning_effort,
        permission_mode,
        EFFECTIVE_CONTINUATION_PERMISSION_MODE,
    );
    let expected_profile_hash = hash_text(&crate::grok_acp::profile_fingerprint_material());
    let mut matches = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|error| {
        format!(
            "read continuation canaries {}: {error}",
            directory.display()
        )
    })? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("read canary {}: {error}", path.display()))?;
        let record = Json::parse(&text)
            .map_err(|error| format!("parse canary {}: {error}", path.display()))?;
        let stored_hash = required_string(&record, "canary_hash")?;
        let mut hash_material = record
            .as_object()
            .cloned()
            .ok_or_else(|| format!("continuation canary is not an object: {}", path.display()))?;
        hash_material.remove("canary_hash");
        let actual_hash = hash_text(&Json::Object(hash_material).to_compact_string());
        if actual_hash != stored_hash
            || path.file_stem().and_then(|value| value.to_str()) != Some(stored_hash.as_str())
        {
            return Err(format!(
                "permission_enforcement_unproven: canary fingerprint mismatch: {}",
                path.display()
            ));
        }
        if record.get("status").and_then(Json::as_str) != Some("passed")
            || record.get("cli_version").and_then(Json::as_str) != Some(cli_version)
            || record.get("model").and_then(Json::as_str) != Some(model)
            || record.get("reasoning_effort").and_then(Json::as_str) != Some(reasoning_effort)
            || record.get("permission_mode").and_then(Json::as_str) != Some(permission_mode)
            || record
                .get("effective_permission_mode")
                .and_then(Json::as_str)
                != Some(EFFECTIVE_CONTINUATION_PERMISSION_MODE)
            || record.get("transport").and_then(Json::as_str) != Some(CONTINUATION_TRANSPORT)
            || record.get("profile_hash").and_then(Json::as_str)
                != Some(expected_profile_hash.as_str())
            || record.get("policy_hash").and_then(Json::as_str) != Some(&expected_policy)
            || record
                .get("forbidden_tool_protocol_verified")
                .and_then(Json::as_bool)
                != Some(true)
        {
            continue;
        }
        let timestamp = json_u128(&record, "timestamp_ms")?;
        matches.push((timestamp, stored_hash));
    }
    matches.sort_by_key(|(timestamp, _)| *timestamp);
    matches.pop().map(|(_, hash)| hash).ok_or_else(|| {
        "permission_enforcement_unproven: no matching continuation canary".to_string()
    })
}

fn permission_policy_hash(
    cli_version: &str,
    model: &str,
    reasoning_effort: &str,
    permission_mode: &str,
    effective_permission_mode: &str,
) -> String {
    let profile_hash = hash_text(&crate::grok_acp::profile_fingerprint_material());
    hash_text(&format!(
        "{PERMISSION_POLICY_VERSION}\0{CONTINUATION_TRANSPORT}\0{profile_hash}\0{cli_version}\0{model}\0{reasoning_effort}\0{permission_mode}\0{effective_permission_mode}"
    ))
}

fn spawn_cancel_watcher(
    path: PathBuf,
    cancellation: CancellationToken,
    done: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
            if path.is_file() {
                cancellation.cancel();
                return;
            }
            thread::sleep(Duration::from_millis(CANCEL_POLL_MS));
        }
    })
}

fn create_new_json(path: &Path, value: &Json) -> Result<(), String> {
    create_new_bytes(path, value.to_pretty_string().as_bytes())
}

fn create_new_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
        refuse_reparse_chain(parent).map_err(|error| error.to_string())?;
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("persist {}: {error}", path.display()))
}

fn acquire_lock(path: &Path) -> Result<File, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
        refuse_reparse_chain(parent).map_err(|error| error.to_string())?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .share_mode(0)
            .open(path)
            .map_err(|error| format!("continuation state is busy ({}): {error}", path.display()))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("continuation state locking is supported only on Windows".to_string())
    }
}

impl StatePaths {
    fn for_profile(profile_namespace: &str) -> Result<Self, String> {
        if !profile_namespace.starts_with("profile-")
            || !profile_namespace
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err("invalid continuation profile namespace".to_string());
        }
        let root = state_root()?;
        let profile = root.join(profile_namespace);
        Ok(Self {
            root: root.clone(),
            manifests: profile.join("manifests"),
            evidence: profile.join("evidence"),
            cancels: profile.join("cancel"),
            runtime: profile.join("runtime"),
            journal: profile.join("journal.jsonl"),
            lock: profile.join("state.lock"),
            profile,
        })
    }

    fn ensure(&self) -> Result<(), String> {
        for path in [
            &self.root,
            &self.profile,
            &self.manifests,
            &self.evidence,
            &self.cancels,
            &self.runtime,
        ] {
            fs::create_dir_all(path).map_err(|error| {
                format!("create continuation state {}: {error}", path.display())
            })?;
            refuse_reparse_chain(path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn cancel_path(&self, continuation_id: &str) -> PathBuf {
        self.cancels.join(format!("{continuation_id}.cancel"))
    }

    fn runtime_lock_path(&self, continuation_id: &str) -> PathBuf {
        self.runtime.join(format!("{continuation_id}.lock"))
    }

    fn clone_for_observer(&self) -> ObserverPaths {
        ObserverPaths {
            journal: self.journal.clone(),
            lock: self.lock.clone(),
        }
    }
}

#[derive(Clone)]
struct ObserverPaths {
    journal: PathBuf,
    lock: PathBuf,
}

fn state_root() -> Result<PathBuf, String> {
    let root = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("Parley").join("continuations"))
        .ok_or_else(|| "continuation state requires LOCALAPPDATA".to_string())?;
    if !root.is_absolute() {
        return Err("continuation state directory must be absolute".to_string());
    }
    if let Some(existing) = root.ancestors().find(|path| path.exists()) {
        refuse_reparse_chain(existing).map_err(|error| error.to_string())?;
    }
    Ok(root)
}

fn required_absolute_env(name: &str) -> Result<PathBuf, String> {
    let path = env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} is required for continuation publication"))?;
    if !path.is_absolute() {
        return Err(format!("{name} must be absolute"));
    }
    Ok(path)
}

fn require_codex_caller() -> Result<(), String> {
    match env::var("PARLEY_CALLER") {
        Ok(caller) if caller.eq_ignore_ascii_case("codex") => Ok(()),
        _ => Err("continuation publication and review require the locked Codex caller".to_string()),
    }
}

fn validate_user_text(text: &str) -> Result<(), String> {
    let count = text.chars().count();
    if text.trim().is_empty() {
        return Err("continuation text cannot be empty".to_string());
    }
    if count > MAX_USER_TEXT_CHARS {
        return Err(format!(
            "continuation text exceeds {MAX_USER_TEXT_CHARS} Unicode scalars"
        ));
    }
    if text.contains('\0') {
        return Err("continuation text contains NUL".to_string());
    }
    Ok(())
}

fn validate_source_session(value: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.chars().count() > MAX_SOURCE_SESSION_CHARS
        || value.eq_ignore_ascii_case("latest")
        || value.eq_ignore_ascii_case("last")
        || value.chars().any(|character| character.is_control())
    {
        return Err("source_session must be an explicit session identifier".to_string());
    }
    Ok(())
}

fn validate_uuid(label: &str, value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.len() != 36
        || ![8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes.get(index) == Some(&b'-'))
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| ![8, 13, 18, 23].contains(&index) && !byte.is_ascii_hexdigit())
    {
        return Err(format!("{label} must be a UUID, got {value}"));
    }
    Ok(())
}

fn normalize_relative_text(value: &str) -> Result<String, String> {
    if value.trim().is_empty()
        || value.chars().count() > MAX_RELATIVE_PATH_CHARS
        || value.contains('*')
        || value.contains('?')
        || value.chars().any(|character| character.is_control())
    {
        return Err(format!("invalid relative path: {value}"));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(format!("writable path must be relative: {value}"));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| "writable path must be Unicode".to_string())?;
                let windows_name = part.trim_end_matches([' ', '.']);
                let base_name = windows_name.split('.').next().unwrap_or(windows_name);
                let reserved_device = matches!(
                    base_name.to_ascii_uppercase().as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                );
                if windows_name.eq_ignore_ascii_case(".git")
                    || windows_name != part
                    || part.contains(':')
                    || reserved_device
                {
                    return Err(format!("unsafe writable path component: {part}"));
                }
                parts.push(part.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(format!("writable path escapes its worktree: {value}"))
            }
        }
    }
    if parts.is_empty() {
        return Err("writable path cannot be the worktree root".to_string());
    }
    Ok(parts.join("/"))
}

fn git_head(worktree: &Path) -> Result<String, String> {
    let output = isolated_git_output(worktree, &["rev-parse", "HEAD"])?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse failed in {}: {}",
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let head = String::from_utf8(output.stdout)
        .map_err(|_| "Git head is not UTF-8".to_string())?
        .trim()
        .to_string();
    if head.len() != 40 || !head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Git head is not a full commit id".to_string());
    }
    Ok(head)
}

fn isolated_git_output(
    worktree: &Path,
    arguments: &[&str],
) -> Result<std::process::Output, String> {
    validate_nonexecuting_git_configuration(worktree)?;
    let mut command = Command::new("git");
    configure_isolated_git(&mut command);
    command
        .args(arguments)
        .current_dir(worktree)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            format!(
                "run isolated Git observation in {}: {error}",
                worktree.display()
            )
        })
}

fn configure_isolated_git(command: &mut Command) {
    let null_config = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let hooks_override = format!("core.hooksPath={null_config}");
    for (name, _) in env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_EXEC_PATH",
        "GIT_SSH",
        "GIT_SSH_COMMAND",
    ] {
        command.env_remove(name);
    }
    command
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "submodule.recurse=false",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
            "-c",
            hooks_override.as_str(),
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_config)
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat");
}

fn validate_nonexecuting_git_configuration(worktree: &Path) -> Result<(), String> {
    let dot_git = worktree.join(".git");
    let git_dir = if dot_git.is_dir() {
        fs::canonicalize(&dot_git)
            .map_err(|error| format!("canonicalize {}: {error}", dot_git.display()))?
    } else {
        let text = read_bounded_text(&dot_git, 64 * 1024, "Git worktree pointer")?;
        let relative = text
            .trim()
            .strip_prefix("gitdir:")
            .map(str::trim)
            .ok_or_else(|| format!("invalid Git worktree pointer: {}", dot_git.display()))?;
        let path = PathBuf::from(relative);
        let resolved = if path.is_absolute() {
            path
        } else {
            worktree.join(path)
        };
        fs::canonicalize(&resolved)
            .map_err(|error| format!("canonicalize {}: {error}", resolved.display()))?
    };
    refuse_reparse_chain(&git_dir).map_err(|error| error.to_string())?;
    let common_dir = {
        let pointer = git_dir.join("commondir");
        if pointer.is_file() {
            let text = read_bounded_text(&pointer, 64 * 1024, "Git common-dir pointer")?;
            let path = PathBuf::from(text.trim());
            let resolved = if path.is_absolute() {
                path
            } else {
                git_dir.join(path)
            };
            fs::canonicalize(&resolved)
                .map_err(|error| format!("canonicalize {}: {error}", resolved.display()))?
        } else {
            git_dir.clone()
        }
    };
    refuse_reparse_chain(&common_dir).map_err(|error| error.to_string())?;
    let mut configs = BTreeSet::new();
    configs.insert(common_dir.join("config"));
    configs.insert(common_dir.join("config.worktree"));
    configs.insert(git_dir.join("config.worktree"));
    for config in configs {
        if !config.is_file() {
            continue;
        }
        let text = read_bounded_text(&config, MAX_GIT_CONFIG_BYTES, "Git configuration")?;
        reject_executable_git_config(&config, &text)?;
    }
    Ok(())
}

fn read_bounded_text(path: &Path, limit: u64, label: &str) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("inspect {}: {error}", path.display()))?;
    if metadata.len() > limit {
        return Err(format!("{label} exceeds {limit} bytes: {}", path.display()));
    }
    fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn reject_executable_git_config(path: &Path, text: &str) -> Result<(), String> {
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            if ["alias", "filter", "diff", "merge", "include", "includeif"]
                .iter()
                .any(|name| {
                    section == *name
                        || section.starts_with(&format!("{name} "))
                        || section.starts_with(&format!("{name}."))
                })
            {
                return Err(format!(
                    "continuation Git observation refuses executable configuration section [{section}] in {}",
                    path.display()
                ));
            }
            continue;
        }
        let key = line
            .split_once('=')
            .map(|(key, _)| key)
            .unwrap_or(line)
            .trim()
            .to_ascii_lowercase();
        if section == "core"
            && matches!(
                key.as_str(),
                "fsmonitor" | "hookspath" | "attributesfile" | "pager"
            )
        {
            return Err(format!(
                "continuation Git observation refuses core.{key} in {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn path_starts_with(path: &Path, root: &Path) -> bool {
    let path = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let root = root
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_ascii_lowercase();
    path == root || path.starts_with(&format!("{root}\\"))
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .eq_ignore_ascii_case(
            &right
                .to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .replace('/', "\\"),
        )
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    path_starts_with(left, right) || path_starts_with(right, left)
}

fn object(values: Vec<(&str, Json)>) -> Json {
    Json::Object(
        values
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
    )
}

fn required_string(value: &Json, name: &str) -> Result<String, String> {
    value
        .get(name)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing {name}"))
}

fn json_u128(value: &Json, name: &str) -> Result<u128, String> {
    value
        .get(name)
        .and_then(Json::as_number)
        .map(|number| number as u128)
        .ok_or_else(|| format!("missing {name}"))
}

fn optional_string(value: &Json, name: &str) -> Option<String> {
    value.get(name).and_then(Json::as_str).map(str::to_string)
}

fn manifest_to_json(manifest: &Manifest) -> Json {
    object(vec![
        ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
        ("manifest_id", Json::Str(manifest.manifest_id.clone())),
        (
            "profile_namespace",
            Json::Str(manifest.profile_namespace.clone()),
        ),
        ("caller", Json::Str(manifest.caller.clone())),
        ("source_session", Json::Str(manifest.source_session.clone())),
        (
            "worktree",
            Json::Str(manifest.worktree.to_string_lossy().into_owned()),
        ),
        (
            "allowed_root",
            Json::Str(manifest.allowed_root.to_string_lossy().into_owned()),
        ),
        ("head", Json::Str(manifest.head.clone())),
        (
            "grants",
            Json::Array(
                manifest
                    .grants
                    .iter()
                    .map(|grant| {
                        object(vec![
                            (
                                "kind",
                                Json::Str(
                                    match grant.kind {
                                        GrantKind::File => "file",
                                        GrantKind::Tree => "tree",
                                    }
                                    .to_string(),
                                ),
                            ),
                            (
                                "path",
                                Json::Str(grant.path.to_string_lossy().replace('\\', "/")),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("created_at_ms", Json::Number(manifest.created_at_ms as f64)),
        ("expires_at_ms", Json::Number(manifest.expires_at_ms as f64)),
        ("model", Json::Str(manifest.model.clone())),
        (
            "reasoning_effort",
            Json::Str(manifest.reasoning_effort.clone()),
        ),
        ("cli_version", Json::Str(manifest.cli_version.clone())),
        (
            "permission_mode",
            Json::Str(manifest.permission_mode.clone()),
        ),
        (
            "canonical_epoch",
            Json::Number(manifest.canonical_epoch as f64),
        ),
        (
            "event_log",
            Json::Str(manifest.event_log.to_string_lossy().into_owned()),
        ),
        (
            "context_state_dir",
            Json::Str(manifest.context_state_dir.to_string_lossy().into_owned()),
        ),
        (
            "job_state_dir",
            Json::Str(manifest.job_state_dir.to_string_lossy().into_owned()),
        ),
        ("canary_hash", Json::Str(manifest.canary_hash.clone())),
        (
            "granted_fingerprint",
            Json::Str(manifest.granted_fingerprint.clone()),
        ),
        (
            "outside_fingerprint",
            Json::Str(manifest.outside_fingerprint.clone()),
        ),
    ])
}

fn manifest_from_json(value: &Json) -> Result<Manifest, String> {
    if value.get("schema_version").and_then(Json::as_number) != Some(SCHEMA_VERSION as f64) {
        return Err("unsupported continuation manifest schema".to_string());
    }
    let worktree = PathBuf::from(required_string(value, "worktree")?);
    let grants = parse_grants(value.get("grants"), &worktree)?;
    reject_validation_commands(value.get("validation_commands"))?;
    Ok(Manifest {
        manifest_id: required_string(value, "manifest_id")?,
        profile_namespace: required_string(value, "profile_namespace")?,
        caller: required_string(value, "caller")?,
        source_session: required_string(value, "source_session")?,
        worktree,
        allowed_root: PathBuf::from(required_string(value, "allowed_root")?),
        head: required_string(value, "head")?,
        grants,
        created_at_ms: json_u128(value, "created_at_ms")?,
        expires_at_ms: json_u128(value, "expires_at_ms")?,
        model: required_string(value, "model")?,
        reasoning_effort: required_string(value, "reasoning_effort")?,
        cli_version: required_string(value, "cli_version")?,
        permission_mode: required_string(value, "permission_mode")?,
        canonical_epoch: json_u128(value, "canonical_epoch")? as u64,
        event_log: PathBuf::from(required_string(value, "event_log")?),
        context_state_dir: PathBuf::from(required_string(value, "context_state_dir")?),
        job_state_dir: PathBuf::from(required_string(value, "job_state_dir")?),
        canary_hash: required_string(value, "canary_hash")?,
        granted_fingerprint: required_string(value, "granted_fingerprint")?,
        outside_fingerprint: required_string(value, "outside_fingerprint")?,
    })
}

fn manifest_public_json(
    manifest: &Manifest,
    state: &str,
    continuation_count: usize,
    running_continuation_id: Option<&str>,
    latest: Option<Json>,
) -> Json {
    object(vec![
        ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
        ("state", Json::Str(state.to_string())),
        ("manifest_id", Json::Str(manifest.manifest_id.clone())),
        (
            "worktree_name",
            Json::Str(
                manifest
                    .worktree
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("worktree")
                    .to_string(),
            ),
        ),
        (
            "writable_paths",
            Json::Array(
                manifest
                    .grants
                    .iter()
                    .map(|grant| Json::Str(grant.path.to_string_lossy().replace('\\', "/")))
                    .collect(),
            ),
        ),
        ("expires_at_ms", Json::Number(manifest.expires_at_ms as f64)),
        (
            "continuation_count",
            Json::Number(continuation_count as f64),
        ),
        (
            "running_continuation_id",
            running_continuation_id
                .map(|value| Json::Str(value.to_string()))
                .unwrap_or(Json::Null),
        ),
        ("latest", latest.unwrap_or(Json::Null)),
    ])
}

fn status_evidence_json(evidence: Evidence, user_received: bool) -> Json {
    let mut value = evidence_to_json(evidence)
        .as_object()
        .cloned()
        .expect("continuation evidence is an object");
    value.insert("user_received".to_string(), Json::Bool(user_received));
    Json::Object(value)
}

fn evidence_to_json(evidence: Evidence) -> Json {
    let mut fields = vec![
        ("schema_version", Json::Number(SCHEMA_VERSION as f64)),
        ("continuation_id", Json::Str(evidence.continuation_id)),
        ("manifest_id", Json::Str(evidence.manifest_id)),
        ("state", Json::Str(evidence.state)),
        ("prompt", Json::Str(evidence.prompt)),
        ("reply", evidence.reply.map(Json::Str).unwrap_or(Json::Null)),
        ("error", evidence.error.map(Json::Str).unwrap_or(Json::Null)),
        (
            "exchange_id",
            evidence.exchange_id.map(Json::Str).unwrap_or(Json::Null),
        ),
        (
            "request_event_id",
            evidence
                .request_event_id
                .map(Json::Str)
                .unwrap_or(Json::Null),
        ),
        (
            "completion_event_id",
            evidence
                .completion_event_id
                .map(Json::Str)
                .unwrap_or(Json::Null),
        ),
        (
            "changed_paths",
            Json::Array(evidence.changed_paths.into_iter().map(Json::Str).collect()),
        ),
        ("validation_state", Json::Str(evidence.validation_state)),
        (
            "granted_fingerprint",
            Json::Str(evidence.granted_fingerprint),
        ),
        (
            "outside_fingerprint",
            Json::Str(evidence.outside_fingerprint),
        ),
        ("diff_fingerprint", Json::Str(evidence.diff_fingerprint)),
        ("evidence_hash", Json::Str(evidence.evidence_hash)),
        (
            "completed_at_ms",
            Json::Number(evidence.completed_at_ms as f64),
        ),
    ];
    if let Some(git_head) = evidence.git_head {
        fields.push(("git_head", Json::Str(git_head)));
    }
    object(fields)
}

fn evidence_from_json(value: &Json) -> Result<Evidence, String> {
    if value.get("schema_version").and_then(Json::as_number) != Some(SCHEMA_VERSION as f64) {
        return Err("unsupported continuation evidence schema".to_string());
    }
    let validation_state = required_string(value, "validation_state")?;
    if validation_state != "not_executed" {
        return Err("unsupported continuation validation state".to_string());
    }
    let git_head = match value.get("git_head") {
        None => None,
        Some(value) => {
            let head = value
                .as_str()
                .filter(|head| {
                    matches!(head.len(), 40 | 64)
                        && head.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .ok_or_else(|| "continuation evidence contains an invalid Git HEAD".to_string())?;
            Some(head.to_string())
        }
    };
    Ok(Evidence {
        continuation_id: required_string(value, "continuation_id")?,
        manifest_id: required_string(value, "manifest_id")?,
        state: required_string(value, "state")?,
        prompt: required_string(value, "prompt")?,
        reply: optional_string(value, "reply"),
        error: optional_string(value, "error"),
        exchange_id: optional_string(value, "exchange_id"),
        request_event_id: optional_string(value, "request_event_id"),
        completion_event_id: optional_string(value, "completion_event_id"),
        changed_paths: value
            .get("changed_paths")
            .and_then(Json::as_array)
            .ok_or_else(|| "continuation evidence is missing changed_paths".to_string())?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "continuation changed path is not a string".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
        validation_state,
        granted_fingerprint: required_string(value, "granted_fingerprint")?,
        outside_fingerprint: required_string(value, "outside_fingerprint")?,
        diff_fingerprint: required_string(value, "diff_fingerprint")?,
        git_head,
        evidence_hash: required_string(value, "evidence_hash")?,
        completed_at_ms: json_u128(value, "completed_at_ms")?,
    })
}

fn hash_text(value: &str) -> String {
    hex_encode(&sha256(value.as_bytes()))
}

fn combine_error(existing: Option<String>, additional: String) -> String {
    match existing {
        Some(existing) if !existing.is_empty() => format!("{existing}; {additional}"),
        _ => additional,
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(windows)]
mod worktree_watch {
    use std::collections::BTreeSet;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};

    type Handle = *mut c_void;

    const INVALID_HANDLE_VALUE: Handle = -1_isize as Handle;
    const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
    const FILE_NOTIFY_FILTER: u32 = 0x0000_0001
        | 0x0000_0002
        | 0x0000_0004
        | 0x0000_0008
        | 0x0000_0010
        | 0x0000_0040
        | 0x0000_0100;
    const ERROR_OPERATION_ABORTED: u32 = 995;
    const ERROR_IO_PENDING: u32 = 997;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_FAILED: u32 = 0xffff_ffff;
    const INFINITE: u32 = 0xffff_ffff;
    const BUFFER_BYTES: usize = 64 * 1024;

    #[repr(C)]
    #[derive(Default)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: Handle,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *mut c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: Handle,
        ) -> Handle;
        fn CreateEventW(
            event_attributes: *const c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> Handle;
        fn SetEvent(event: Handle) -> i32;
        fn ResetEvent(event: Handle) -> i32;
        fn ReadDirectoryChangesW(
            directory: Handle,
            buffer: *mut c_void,
            buffer_length: u32,
            watch_subtree: i32,
            notify_filter: u32,
            bytes_returned: *mut u32,
            overlapped: *mut Overlapped,
            completion_routine: *mut c_void,
        ) -> i32;
        fn GetOverlappedResult(
            handle: Handle,
            overlapped: *mut Overlapped,
            bytes_transferred: *mut u32,
            wait: i32,
        ) -> i32;
        fn CancelIoEx(handle: Handle, overlapped: *mut Overlapped) -> i32;
        fn WaitForMultipleObjects(
            count: u32,
            handles: *const Handle,
            wait_all: i32,
            milliseconds: u32,
        ) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetLastError() -> u32;
    }

    struct OwnedHandle(Handle);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    pub(super) struct WorktreeWatcher {
        stop_event: usize,
        thread: Option<JoinHandle<Result<BTreeSet<String>, String>>>,
    }

    impl WorktreeWatcher {
        pub(super) fn start(worktree: &Path) -> Result<Self, String> {
            let wide = worktree
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let thread = thread::spawn(move || watch_directory(wide, ready_tx));
            match ready_rx.recv() {
                Ok(Ok(stop_event)) => Ok(Self {
                    stop_event,
                    thread: Some(thread),
                }),
                Ok(Err(error)) => {
                    let _ = thread.join();
                    Err(error)
                }
                Err(_) => match thread.join() {
                    Ok(Err(error)) => Err(error),
                    Ok(Ok(_)) => Err("worktree watcher exited before readiness".to_string()),
                    Err(_) => Err("worktree watcher panicked before readiness".to_string()),
                },
            }
        }

        pub(super) fn finish(mut self) -> Result<BTreeSet<String>, String> {
            self.stop_and_join()
        }

        fn stop_and_join(&mut self) -> Result<BTreeSet<String>, String> {
            let Some(thread) = self.thread.take() else {
                return Ok(BTreeSet::new());
            };
            if unsafe { SetEvent(self.stop_event as Handle) } == 0 {
                return Err(last_error("SetEvent(worktree watcher stop)"));
            }
            thread
                .join()
                .map_err(|_| "worktree watcher thread panicked".to_string())?
        }
    }

    impl Drop for WorktreeWatcher {
        fn drop(&mut self) {
            let _ = self.stop_and_join();
        }
    }

    fn watch_directory(
        wide: Vec<u16>,
        ready_tx: mpsc::SyncSender<Result<usize, String>>,
    ) -> Result<BTreeSet<String>, String> {
        let directory = OwnedHandle(unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        });
        if directory.0 == INVALID_HANDLE_VALUE {
            let error = last_error("CreateFileW(worktree watcher)");
            let _ = ready_tx.send(Err(error.clone()));
            return Err(error);
        }
        let change_event = OwnedHandle(unsafe { CreateEventW(null(), 1, 0, null()) });
        if change_event.0.is_null() {
            let error = last_error("CreateEventW(worktree watcher change)");
            let _ = ready_tx.send(Err(error.clone()));
            return Err(error);
        }
        let stop_event = OwnedHandle(unsafe { CreateEventW(null(), 1, 0, null()) });
        if stop_event.0.is_null() {
            let error = last_error("CreateEventW(worktree watcher stop)");
            let _ = ready_tx.send(Err(error.clone()));
            return Err(error);
        }
        let mut buffer = vec![0_u8; BUFFER_BYTES];
        let mut overlapped = Overlapped {
            event: change_event.0,
            ..Overlapped::default()
        };
        issue_read(directory.0, &mut buffer, &mut overlapped)?;
        if ready_tx.send(Ok(stop_event.0 as usize)).is_err() {
            unsafe {
                CancelIoEx(directory.0, &mut overlapped);
            }
            return Err("worktree watcher readiness receiver closed".to_string());
        }

        let handles = [change_event.0, stop_event.0];
        let mut paths = BTreeSet::new();
        loop {
            let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            if wait == WAIT_OBJECT_0 + 1 {
                unsafe {
                    CancelIoEx(directory.0, &mut overlapped);
                }
                break;
            }
            if wait == WAIT_FAILED {
                return Err(last_error("WaitForMultipleObjects(worktree watcher)"));
            }
            if wait != WAIT_OBJECT_0 {
                return Err(format!("unexpected worktree watcher wait result {wait}"));
            }
            let mut transferred = 0_u32;
            if unsafe { GetOverlappedResult(directory.0, &mut overlapped, &mut transferred, 0) }
                == 0
            {
                let error = unsafe { GetLastError() };
                if error == ERROR_OPERATION_ABORTED {
                    break;
                }
                return Err(format!(
                    "GetOverlappedResult(worktree watcher) failed with Windows error {error}"
                ));
            }
            if transferred == 0 {
                return Err("worktree watcher notification buffer overflowed".to_string());
            }
            parse_notifications(&buffer[..transferred as usize], &mut paths)?;
            issue_read(directory.0, &mut buffer, &mut overlapped)?;
        }
        Ok(paths)
    }

    fn issue_read(
        directory: Handle,
        buffer: &mut [u8],
        overlapped: &mut Overlapped,
    ) -> Result<(), String> {
        if unsafe { ResetEvent(overlapped.event) } == 0 {
            return Err(last_error("ResetEvent(worktree watcher)"));
        }
        overlapped.internal = 0;
        overlapped.internal_high = 0;
        overlapped.offset = 0;
        overlapped.offset_high = 0;
        let started = unsafe {
            ReadDirectoryChangesW(
                directory,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                1,
                FILE_NOTIFY_FILTER,
                null_mut(),
                overlapped,
                null_mut(),
            )
        };
        if started == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_PENDING {
                return Err(format!(
                    "ReadDirectoryChangesW failed with Windows error {error}"
                ));
            }
        }
        Ok(())
    }

    fn parse_notifications(bytes: &[u8], paths: &mut BTreeSet<String>) -> Result<(), String> {
        let mut offset = 0_usize;
        loop {
            if bytes.len().saturating_sub(offset) < 12 {
                return Err("worktree watcher returned a truncated record".to_string());
            }
            let next = read_u32(bytes, offset)? as usize;
            let name_bytes = read_u32(bytes, offset + 8)? as usize;
            if name_bytes == 0 || !name_bytes.is_multiple_of(2) {
                return Err("worktree watcher returned an invalid UTF-16 path".to_string());
            }
            let name_start = offset + 12;
            let name_end = name_start
                .checked_add(name_bytes)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| "worktree watcher path exceeds its record".to_string())?;
            let mut utf16 = Vec::with_capacity(name_bytes / 2);
            for chunk in bytes[name_start..name_end].as_chunks::<2>().0 {
                utf16.push(u16::from_le_bytes([chunk[0], chunk[1]]));
            }
            let path = String::from_utf16(&utf16)
                .map_err(|_| "worktree watcher path is not valid UTF-16".to_string())?
                .replace('\\', "/");
            paths.insert(path);
            if next == 0 {
                return Ok(());
            }
            if next < 12 || offset.saturating_add(next) > bytes.len() {
                return Err("worktree watcher returned an invalid next-record offset".to_string());
            }
            offset += next;
        }
    }

    fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
        let value = bytes
            .get(offset..offset + 4)
            .ok_or_else(|| "worktree watcher returned a truncated integer".to_string())?;
        Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
    }

    fn last_error(operation: &str) -> String {
        format!("{operation} failed with Windows error {}", unsafe {
            GetLastError()
        })
    }
}

#[cfg(not(windows))]
mod worktree_watch {
    use std::collections::BTreeSet;
    use std::path::Path;

    pub(super) struct WorktreeWatcher;

    impl WorktreeWatcher {
        pub(super) fn start(_worktree: &Path) -> Result<Self, String> {
            Err("direct continuation worktree watching is supported only on Windows".to_string())
        }

        pub(super) fn finish(self) -> Result<BTreeSet<String>, String> {
            Err("direct continuation worktree watching is supported only on Windows".to_string())
        }
    }
}

use worktree_watch::WorktreeWatcher;

fn new_uuid(domain: &str) -> String {
    let material = format!(
        "{domain}\0{}\0{}\0{}",
        now_ms(),
        std::process::id(),
        ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let digest = sha256(material.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths(label: &str) -> StatePaths {
        let root = std::env::temp_dir().join(format!(
            "parley-continuation-test-{}-{}-{label}",
            std::process::id(),
            ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let profile = root.join("profile-1111111111111111");
        StatePaths {
            root: root.clone(),
            manifests: profile.join("manifests"),
            evidence: profile.join("evidence"),
            cancels: profile.join("cancel"),
            runtime: profile.join("runtime"),
            journal: profile.join("journal.jsonl"),
            lock: profile.join("state.lock"),
            profile,
        }
    }

    fn test_evidence(continuation_id: &str, manifest_id: &str) -> Evidence {
        let mut evidence = Evidence {
            continuation_id: continuation_id.to_string(),
            manifest_id: manifest_id.to_string(),
            state: "succeeded".to_string(),
            prompt: "finish".to_string(),
            reply: Some("done".to_string()),
            error: None,
            exchange_id: Some("exchange".to_string()),
            request_event_id: Some("request".to_string()),
            completion_event_id: Some("response".to_string()),
            changed_paths: vec!["src/lib.rs".to_string()],
            validation_state: "not_executed".to_string(),
            granted_fingerprint: "a".repeat(64),
            outside_fingerprint: "b".repeat(64),
            diff_fingerprint: "c".repeat(64),
            git_head: Some("d".repeat(40)),
            evidence_hash: String::new(),
            completed_at_ms: 1,
        };
        evidence.evidence_hash = hash_text(&evidence_to_json(evidence.clone()).to_compact_string());
        evidence
    }

    fn test_manifest(paths: &StatePaths, worktree: &Path) -> Manifest {
        Manifest {
            manifest_id: "11111111-1111-4111-8111-111111111111".to_string(),
            profile_namespace: "profile-1111111111111111".to_string(),
            caller: "codex".to_string(),
            source_session: "source-session".to_string(),
            worktree: worktree.to_path_buf(),
            allowed_root: worktree.parent().unwrap_or(worktree).to_path_buf(),
            head: "0".repeat(40),
            grants: vec![PathGrant {
                path: PathBuf::from("src"),
                kind: GrantKind::Tree,
            }],
            created_at_ms: 1,
            expires_at_ms: u128::MAX,
            model: "grok-4.7".to_string(),
            reasoning_effort: "xhigh".to_string(),
            cli_version: "1.0.40".to_string(),
            permission_mode: "auto".to_string(),
            canonical_epoch: 1,
            event_log: paths.root.join("events.jsonl"),
            context_state_dir: paths.root.join("context"),
            job_state_dir: paths.root.join("jobs"),
            canary_hash: "c".repeat(64),
            granted_fingerprint: "a".repeat(64),
            outside_fingerprint: "b".repeat(64),
        }
    }

    #[cfg(windows)]
    fn init_test_repo(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "parley-continuation-repo-{}-{}-{label}",
            std::process::id(),
            ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let run = |arguments: &[&str]| {
            let output = Command::new("git")
                .args(arguments)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                arguments,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init"]);
        run(&["config", "user.name", "Parley Test"]);
        run(&["config", "user.email", "parley@example.invalid"]);
        root
    }

    #[test]
    fn relative_paths_reject_escape_and_globs() {
        assert_eq!(
            normalize_relative_text(r"src\\lib.rs").unwrap(),
            "src/lib.rs"
        );
        assert!(normalize_relative_text("../outside").is_err());
        assert!(normalize_relative_text("src/**").is_err());
        assert!(normalize_relative_text("C:\\absolute").is_err());
        assert!(normalize_relative_text(".git/config").is_err());
        assert!(normalize_relative_text("src/file.rs:stream").is_err());
        assert!(normalize_relative_text("NUL.txt").is_err());
        assert!(normalize_relative_text("src/trailing. ").is_err());
        assert!(normalize_relative_text(&"x".repeat(MAX_RELATIVE_PATH_CHARS + 1)).is_err());
    }

    #[test]
    fn path_grants_are_exact_or_descendant_only() {
        let grants = vec![
            PathGrant {
                path: PathBuf::from("README.md"),
                kind: GrantKind::File,
            },
            PathGrant {
                path: PathBuf::from("src/jobs"),
                kind: GrantKind::Tree,
            },
        ];
        assert!(path_is_granted("README.md", &grants));
        assert!(path_is_granted("src/jobs/mod.rs", &grants));
        assert!(!path_is_granted("README.md.bak", &grants));
        assert!(!path_is_granted("src/jobs-old/mod.rs", &grants));
    }

    #[test]
    fn user_text_is_bounded_without_rewriting_it() {
        assert!(validate_user_text("finish the task").is_ok());
        assert!(validate_user_text("  ").is_err());
        assert!(validate_user_text("bad\0text").is_err());
        assert!(validate_user_text(&"x".repeat(MAX_USER_TEXT_CHARS + 1)).is_err());
    }

    #[test]
    fn continuation_prompt_preserves_user_whitespace_exactly() {
        let user_text = "  first line\nsecond line  \n";
        let prompt = continuation_prompt(user_text, &[]);
        assert!(prompt.starts_with(&format!(
            "=== PARLEY_USER_CONTINUATION_REQUEST_V1 ===\n{user_text}=== PARLEY_CONTINUATION_CONTRACT_V1 ===\n"
        )));
    }

    #[test]
    fn publication_bounds_are_enforced_at_runtime() {
        assert_eq!(parse_ttl_seconds(None).unwrap(), DEFAULT_TTL_SECS);
        assert_eq!(parse_ttl_seconds(Some(&Json::Number(60.0))).unwrap(), 60);
        assert!(parse_ttl_seconds(Some(&Json::Number(1.5))).is_err());
        assert!(parse_ttl_seconds(Some(&Json::Number(0.0))).is_err());
        assert!(validate_source_session(&"s".repeat(MAX_SOURCE_SESSION_CHARS)).is_ok());
        assert!(validate_source_session(&"s".repeat(MAX_SOURCE_SESSION_CHARS + 1)).is_err());
        let grants = Json::Array(
            (0..=MAX_GRANTS)
                .map(|index| {
                    object(vec![
                        ("kind", Json::Str("file".to_string())),
                        ("path", Json::Str(format!("src/file-{index}.rs"))),
                    ])
                })
                .collect(),
        );
        assert!(parse_grants(Some(&grants), Path::new(r"C:\worktree")).is_err());
    }

    #[test]
    fn permission_policy_hash_binds_the_locked_profile() {
        let baseline = permission_policy_hash(
            "1.0.40",
            "grok-4.7",
            "xhigh",
            "auto",
            EFFECTIVE_CONTINUATION_PERMISSION_MODE,
        );
        assert_ne!(
            baseline,
            permission_policy_hash(
                "1.0.41",
                "grok-4.7",
                "xhigh",
                "auto",
                EFFECTIVE_CONTINUATION_PERMISSION_MODE,
            )
        );
        assert_ne!(
            baseline,
            permission_policy_hash(
                "1.0.40",
                "grok-4.7",
                "high",
                "auto",
                EFFECTIVE_CONTINUATION_PERMISSION_MODE,
            )
        );
        assert_ne!(
            baseline,
            permission_policy_hash(
                "1.0.40",
                "grok-4.7",
                "xhigh",
                "dontAsk",
                EFFECTIVE_CONTINUATION_PERMISSION_MODE,
            )
        );
        assert_ne!(
            baseline,
            permission_policy_hash("1.0.40", "grok-4.7", "xhigh", "auto", "auto")
        );
    }

    #[test]
    fn permission_canary_fingerprint_is_verified() {
        let paths = test_paths("canary-fingerprint");
        paths.ensure().unwrap();
        let policy_hash = permission_policy_hash(
            "1.0.40",
            "grok-4.7",
            "xhigh",
            "auto",
            EFFECTIVE_CONTINUATION_PERMISSION_MODE,
        );
        let profile_hash = hash_text(&crate::grok_acp::profile_fingerprint_material());
        let material = object(vec![
            ("schema_version", Json::Number(1.0)),
            ("status", Json::Str("passed".to_string())),
            ("cli_version", Json::Str("1.0.40".to_string())),
            ("model", Json::Str("grok-4.7".to_string())),
            ("reasoning_effort", Json::Str("xhigh".to_string())),
            ("permission_mode", Json::Str("auto".to_string())),
            (
                "effective_permission_mode",
                Json::Str(EFFECTIVE_CONTINUATION_PERMISSION_MODE.to_string()),
            ),
            ("transport", Json::Str(CONTINUATION_TRANSPORT.to_string())),
            ("profile_hash", Json::Str(profile_hash)),
            ("policy_hash", Json::Str(policy_hash)),
            ("forbidden_tool_protocol_verified", Json::Bool(true)),
            ("timestamp_ms", Json::Number(1.0)),
            ("canary_id", Json::Str("canary".to_string())),
            ("exchange_id", Json::Str("exchange".to_string())),
            ("request_event_id", Json::Str("request".to_string())),
            ("response_event_id", Json::Str("response".to_string())),
            ("worktree", Json::Str("worktree".to_string())),
        ]);
        let canary_hash = hash_text(&material.to_compact_string());
        let mut record = material.as_object().cloned().unwrap();
        record.insert("canary_hash".to_string(), Json::Str(canary_hash.clone()));
        let directory = paths.root.join("canaries");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{canary_hash}.json"));
        fs::write(&path, Json::Object(record.clone()).to_pretty_string()).unwrap();
        assert_eq!(
            latest_valid_canary_hash(&paths, "1.0.40", "grok-4.7", "xhigh", "auto").unwrap(),
            canary_hash
        );

        record.insert("status".to_string(), Json::Str("tampered".to_string()));
        fs::write(&path, Json::Object(record).to_pretty_string()).unwrap();
        assert!(
            latest_valid_canary_hash(&paths, "1.0.40", "grok-4.7", "xhigh", "auto")
                .unwrap_err()
                .contains("fingerprint mismatch")
        );
        let _ = fs::remove_dir_all(paths.root);
    }

    #[test]
    #[cfg(windows)]
    fn isolated_git_observation_ignores_poisoned_git_environment() {
        let root = std::env::temp_dir().join(format!(
            "parley-continuation-git-{}-{}",
            std::process::id(),
            ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let run = |arguments: &[&str]| {
            let output = Command::new("git")
                .args(arguments)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                arguments,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init"]);
        run(&["config", "user.name", "Parley Test"]);
        run(&["config", "user.email", "parley@example.invalid"]);
        fs::write(root.join("tracked.txt"), "tracked\n").unwrap();
        run(&["add", "tracked.txt"]);
        run(&["commit", "-m", "fixture"]);
        let mut command = Command::new("git");
        command.env("GIT_DIR", root.join("missing-git-dir"));
        configure_isolated_git(&mut command);
        let output = command
            .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(output.status.success());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validation_commands_cannot_execute_edited_code() {
        assert!(reject_validation_commands(None).is_ok());
        assert!(reject_validation_commands(Some(&Json::Array(Vec::new()))).is_ok());
        assert!(
            reject_validation_commands(Some(&Json::Array(vec![object(vec![
                ("program", Json::Str("cargo".to_string())),
                ("args", Json::Array(vec![Json::Str("test".to_string())])),
            ])])))
            .is_err()
        );
    }

    #[test]
    fn uuid_validation_is_closed() {
        assert!(validate_uuid("id", "11111111-1111-4111-8111-111111111111").is_ok());
        assert!(validate_uuid("id", "latest").is_err());
        assert!(validate_uuid("id", "11111111-1111-4111-8111-11111111111z").is_err());
    }

    #[test]
    fn journal_recovers_a_sealed_evidence_file_after_transition_loss() {
        let paths = test_paths("recover-evidence");
        paths.ensure().unwrap();
        let manifest_id = "11111111-1111-4111-8111-111111111111";
        let continuation_id = "22222222-2222-4222-8222-222222222222";
        append_transition(
            &paths.journal,
            transition(manifest_id, Some(continuation_id), "running", vec![]),
        )
        .unwrap();
        write_evidence(&paths, &test_evidence(continuation_id, manifest_id)).unwrap();

        let state = read_journal(&paths, manifest_id).unwrap();
        assert!(state.running.is_none());
        assert!(state.terminal_evidence.contains_key(continuation_id));
        let granted_fingerprint = "a".repeat(64);
        assert_eq!(
            state.last_granted_fingerprint.as_deref(),
            Some(granted_fingerprint.as_str())
        );
        let _ = fs::remove_dir_all(paths.root);
    }

    #[test]
    fn evidence_fingerprint_rejects_tampering() {
        let paths = test_paths("tampered-evidence");
        paths.ensure().unwrap();
        let manifest_id = "11111111-1111-4111-8111-111111111111";
        let continuation_id = "22222222-2222-4222-8222-222222222222";
        write_evidence(&paths, &test_evidence(continuation_id, manifest_id)).unwrap();
        let evidence_path = paths.evidence.join(format!("{continuation_id}.json"));
        let mut value = Json::parse(&fs::read_to_string(&evidence_path).unwrap()).unwrap();
        let Json::Object(values) = &mut value else {
            panic!("evidence must be an object");
        };
        values.insert("reply".to_string(), Json::Str("tampered".to_string()));
        fs::write(&evidence_path, value.to_compact_string()).unwrap();

        let error = read_evidence(&paths, continuation_id).unwrap_err();
        assert!(error.contains("fingerprint mismatch"));
        let _ = fs::remove_dir_all(paths.root);
    }

    #[test]
    fn evidence_rejects_any_executable_validation_state_even_with_a_matching_hash() {
        let paths = test_paths("validation-state");
        paths.ensure().unwrap();
        let manifest_id = "11111111-1111-4111-8111-111111111111";
        let continuation_id = "22222222-2222-4222-8222-222222222222";
        let mut evidence = test_evidence(continuation_id, manifest_id);
        evidence.validation_state = "executed".to_string();
        evidence.evidence_hash.clear();
        evidence.evidence_hash = hash_text(&evidence_to_json(evidence.clone()).to_compact_string());
        write_evidence(&paths, &evidence).unwrap();

        let error = read_evidence(&paths, continuation_id).unwrap_err();
        assert!(error.contains("unsupported continuation validation state"));
        let _ = fs::remove_dir_all(paths.root);
    }

    #[test]
    fn orphaned_running_manifest_aborts_and_blocks_all_grok_work() {
        let paths = test_paths("orphan-running");
        paths.ensure().unwrap();
        let worktree = paths.root.join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        let manifest = test_manifest(&paths, &worktree);
        let continuation_id = "22222222-2222-4222-8222-222222222222";
        append_transition(
            &paths.journal,
            transition(
                &manifest.manifest_id,
                Some(continuation_id),
                "running",
                vec![],
            ),
        )
        .unwrap();

        let error =
            require_reviewed_in(&worktree, vec![(paths.clone(), manifest.clone())]).unwrap_err();
        assert!(error.contains("broker_interrupted"));
        let state = read_journal(&paths, &manifest.manifest_id).unwrap();
        assert!(state.running.is_none());
        assert_eq!(
            state.terminal_failure.as_deref(),
            Some("broker_interrupted")
        );
        let _ = fs::remove_dir_all(paths.root);
    }

    #[test]
    fn live_runtime_lock_keeps_running_manifest_blocked_without_false_recovery() {
        let paths = test_paths("live-running");
        paths.ensure().unwrap();
        let worktree = paths.root.join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        let manifest = test_manifest(&paths, &worktree);
        let continuation_id = "22222222-2222-4222-8222-222222222222";
        let runtime_lock = acquire_lock(&paths.runtime_lock_path(continuation_id)).unwrap();
        append_transition(
            &paths.journal,
            transition(
                &manifest.manifest_id,
                Some(continuation_id),
                "running",
                vec![],
            ),
        )
        .unwrap();

        let error =
            require_reviewed_in(&worktree, vec![(paths.clone(), manifest.clone())]).unwrap_err();
        assert!(error.contains("is running"));
        let state = read_journal(&paths, &manifest.manifest_id).unwrap();
        assert_eq!(state.running.as_deref(), Some(continuation_id));
        assert!(state.terminal_failure.is_none());
        drop(runtime_lock);
        let _ = fs::remove_dir_all(paths.root);
    }

    #[test]
    fn sealed_evidence_blocks_the_worktree_until_codex_review() {
        let paths = test_paths("sealed-review-gate");
        paths.ensure().unwrap();
        let worktree = paths.root.join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        let manifest = test_manifest(&paths, &worktree);
        let continuation_id = "22222222-2222-4222-8222-222222222222";
        let evidence = test_evidence(continuation_id, &manifest.manifest_id);
        write_evidence(&paths, &evidence).unwrap();
        append_transition(
            &paths.journal,
            transition(
                &manifest.manifest_id,
                Some(continuation_id),
                "sealed",
                vec![("evidence_hash", Json::Str(evidence.evidence_hash.clone()))],
            ),
        )
        .unwrap();

        let error =
            require_reviewed_in(&worktree, vec![(paths.clone(), manifest.clone())]).unwrap_err();
        assert!(error.contains("until Codex reviews"));

        append_transition(
            &paths.journal,
            transition(
                &manifest.manifest_id,
                None,
                "codex_reviewed",
                vec![("evidence_hash", Json::Str("0".repeat(64)))],
            ),
        )
        .unwrap();
        let error =
            require_reviewed_in(&worktree, vec![(paths.clone(), manifest.clone())]).unwrap_err();
        assert!(error.contains("review hash is stale"));

        let state = read_journal(&paths, &manifest.manifest_id).unwrap();
        let aggregate = aggregate_evidence_hash(&manifest, &state);
        append_transition(
            &paths.journal,
            transition(
                &manifest.manifest_id,
                None,
                "codex_reviewed",
                vec![("evidence_hash", Json::Str(aggregate))],
            ),
        )
        .unwrap();
        assert!(require_reviewed_in(&worktree, vec![(paths.clone(), manifest)]).is_ok());
        let _ = fs::remove_dir_all(paths.root);
    }

    #[test]
    fn review_gate_blocks_ancestor_descendant_and_lane_style_cwds() {
        let paths = test_paths("overlap-review-gate");
        paths.ensure().unwrap();
        let worktree = paths.root.join("integration").join("published");
        let descendant = worktree.join("src");
        let sibling = paths.root.join("integration").join("sibling");
        fs::create_dir_all(&descendant).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let manifest = test_manifest(&paths, &worktree);
        let continuation_id = "22222222-2222-4222-8222-222222222222";
        let evidence = test_evidence(continuation_id, &manifest.manifest_id);
        write_evidence(&paths, &evidence).unwrap();
        append_transition(
            &paths.journal,
            transition(
                &manifest.manifest_id,
                Some(continuation_id),
                "sealed",
                vec![("evidence_hash", Json::Str(evidence.evidence_hash))],
            ),
        )
        .unwrap();

        for cwd in [&worktree, &descendant, worktree.parent().unwrap()] {
            let error =
                require_reviewed_in(cwd, vec![(paths.clone(), manifest.clone())]).unwrap_err();
            assert!(error.contains("until Codex reviews"));
        }
        assert!(require_reviewed_in(&sibling, vec![(paths.clone(), manifest)]).is_ok());
        assert!(!paths_overlap(
            Path::new(r"C:\repo\lane"),
            Path::new(r"C:\repo\lane-old")
        ));
        let _ = fs::remove_dir_all(paths.root);
    }

    #[test]
    fn latest_status_evidence_uses_completion_time_not_uuid_order() {
        let paths = test_paths("latest-completion");
        paths.ensure().unwrap();
        let manifest_id = "11111111-1111-4111-8111-111111111111";
        let older_id = "ffffffff-ffff-4fff-8fff-ffffffffffff";
        let newer_id = "22222222-2222-4222-8222-222222222222";
        let mut older = test_evidence(older_id, manifest_id);
        older.completed_at_ms = 10;
        older.evidence_hash.clear();
        older.evidence_hash = hash_text(&evidence_to_json(older.clone()).to_compact_string());
        let mut newer = test_evidence(newer_id, manifest_id);
        newer.completed_at_ms = 20;
        newer.evidence_hash.clear();
        newer.evidence_hash = hash_text(&evidence_to_json(newer.clone()).to_compact_string());
        write_evidence(&paths, &older).unwrap();
        write_evidence(&paths, &newer).unwrap();
        let mut state = JournalState::default();
        state
            .terminal_evidence
            .insert(older_id.to_string(), older.evidence_hash);
        state
            .terminal_evidence
            .insert(newer_id.to_string(), newer.evidence_hash);

        let latest = latest_terminal_evidence(&paths, &state).unwrap().unwrap();
        assert_eq!(latest.continuation_id, newer_id);
        let _ = fs::remove_dir_all(paths.root);
    }

    #[test]
    #[cfg(windows)]
    fn watcher_exposes_ignored_outside_writes_to_containment() {
        let root = init_test_repo("ignored-watch");
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::write(root.join("tracked.txt"), "tracked\n").unwrap();
        let run = |arguments: &[&str]| {
            let output = Command::new("git")
                .args(arguments)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(output.status.success());
        };
        run(&["add", ".gitignore", "tracked.txt"]);
        run(&["commit", "-m", "fixture"]);
        let watcher = WorktreeWatcher::start(&root).unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::write(root.join("ignored").join("outside.txt"), "changed\n").unwrap();
        let observed = watcher.finish().unwrap();
        assert!(observed.iter().any(|path| path.starts_with("ignored")));
        let snapshot = dirty_snapshot(
            &root,
            &[PathGrant {
                path: PathBuf::from("tracked.txt"),
                kind: GrantKind::File,
            }],
            &observed,
        )
        .unwrap();
        assert!(snapshot
            .outside_paths
            .iter()
            .any(|path| path.starts_with("ignored")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(windows)]
    fn isolated_git_refuses_local_aliases_and_filters_before_execution() {
        let root = init_test_repo("git-config");
        let run = |arguments: &[&str]| {
            let output = Command::new("git")
                .args(arguments)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(output.status.success());
        };
        run(&["config", "alias.status", "!cmd /c echo alias-ran>ALIAS_RAN"]);
        let alias_error = isolated_git_output(&root, &["status", "--porcelain=v1"]).unwrap_err();
        assert!(alias_error.contains("[alias]"));
        assert!(!root.join("ALIAS_RAN").exists());
        run(&["config", "--unset", "alias.status"]);
        run(&[
            "config",
            "filter.parley.clean",
            "cmd /c echo filter-ran>FILTER_RAN",
        ]);
        let filter_error = isolated_git_output(&root, &["status", "--porcelain=v1"]).unwrap_err();
        assert!(filter_error.contains("[filter \"parley\"]"));
        assert!(!root.join("FILTER_RAN").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn status_evidence_marks_user_receipt_without_changing_hash() {
        let evidence = test_evidence(
            "22222222-2222-4222-8222-222222222222",
            "11111111-1111-4111-8111-111111111111",
        );
        let expected_hash = evidence.evidence_hash.clone();
        let value = status_evidence_json(evidence, true);
        assert_eq!(
            value.get("user_received").and_then(Json::as_bool),
            Some(true)
        );
        assert_eq!(
            value.get("evidence_hash").and_then(Json::as_str),
            Some(expected_hash.as_str())
        );
    }

    #[test]
    fn legacy_evidence_without_git_head_remains_hash_compatible() {
        let mut evidence = test_evidence(
            "22222222-2222-4222-8222-222222222222",
            "11111111-1111-4111-8111-111111111111",
        );
        evidence.git_head = None;
        evidence.evidence_hash.clear();
        evidence.evidence_hash = hash_text(&evidence_to_json(evidence.clone()).to_compact_string());
        let value = evidence_to_json(evidence.clone());
        assert!(value.get("git_head").is_none());
        let parsed = evidence_from_json(&value).unwrap();
        assert_eq!(parsed.git_head, None);
        let mut material = parsed.clone();
        material.evidence_hash.clear();
        assert_eq!(
            parsed.evidence_hash,
            hash_text(&evidence_to_json(material).to_compact_string())
        );
    }

    #[test]
    fn aborted_transition_clears_running_without_fabricating_evidence() {
        let paths = test_paths("aborted");
        paths.ensure().unwrap();
        let manifest_id = "11111111-1111-4111-8111-111111111111";
        let continuation_id = "22222222-2222-4222-8222-222222222222";
        append_transition(
            &paths.journal,
            transition(manifest_id, Some(continuation_id), "running", vec![]),
        )
        .unwrap();
        append_aborted(
            &paths,
            manifest_id,
            continuation_id,
            "evidence_write_failed",
        )
        .unwrap();

        let state = read_journal(&paths, manifest_id).unwrap();
        assert!(state.running.is_none());
        assert!(state.terminal_evidence.is_empty());
        assert_eq!(
            state.terminal_failure.as_deref(),
            Some("evidence_write_failed")
        );
        let _ = fs::remove_dir_all(paths.root);
    }
}
