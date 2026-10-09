use super::*;
use crate::ask::AskRequest;
use crate::jobs::{JobErrorKind, STARTUP_RECOVERY_ERROR};
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

const JOB_ID: &str = "11111111-2222-4333-8444-555555555555";
const SECOND_JOB_ID: &str = "66666666-7777-4888-8999-aaaaaaaaaaaa";
const PROFILE: &str = "profile-test-ownership";

fn spec(root: &Path, job_id: &str) -> JobSpec {
    JobSpec {
        job_id: job_id.to_string(),
        mode: JobMode::Probe,
        handoff_mode: HandoffMode::None,
        request: AskRequest {
            harness: "grok".to_string(),
            prompt: "Synthetic ownership fixture".to_string(),
            model: None,
            reasoning_effort: None,
            provider: None,
            cwd: root.to_path_buf(),
            permission_mode: None,
            max_turns: None,
            session_id: Some("bbbbbbbb-cccc-4ddd-8eee-ffffffffffff".to_string()),
            resume_id: None,
            conversation_mode: Some(ConversationMode::Isolated),
            yolo: false,
            context: None,
            max_context_chars: None,
            images: Vec::new(),
            allow_subagents: false,
        },
        lane_plan: None,
    }
}

fn transition(state: JobState, error: Option<&str>) -> JobTransition {
    JobTransition {
        state,
        exchange_id: None,
        session_id: None,
        request_event_id: None,
        completion_event_id: None,
        error: error.map(str::to_string),
    }
}

fn temp(name: &str) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let root = env::temp_dir().join(format!(
        "parley-ownership-{name}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn durable_bytes(store: &JobStore, job_id: &str) -> (Vec<u8>, Vec<u8>) {
    let directory = store.namespace_dir().join(job_id);
    (
        fs::read(directory.join("journal.jsonl")).unwrap(),
        fs::read(directory.join("snapshot.json")).unwrap(),
    )
}

fn runtime(store: Arc<JobStore>, root: &Path) -> Arc<RuntimeInner> {
    Arc::new(RuntimeInner {
        store,
        active: Mutex::new(HashMap::new()),
        terminal: Mutex::new(HashMap::new()),
        threads: Mutex::new(Vec::new()),
        event_log: None,
        escrow_dir: root.join("escrow"),
        handoff_dir: None,
        handoff_lock: Mutex::new(()),
        max_concurrent: 1,
    })
}

#[test]
fn recovery_refuses_live_owner_through_terminal_commit() {
    let root = temp("full-lifetime");
    let store = Arc::new(JobStore::open_at(&root, PROFILE).unwrap());
    let lease = Arc::new(ExecutionLease::acquire_profile(&root, PROFILE, JOB_ID).unwrap());
    let worker_lease = Arc::clone(&lease);
    store.begin(&spec(&root, JOB_ID)).unwrap();
    for state in [JobState::Preparing, JobState::Running] {
        if state == JobState::Running {
            store.transition(JOB_ID, &transition(state, None)).unwrap();
        }
        let before = durable_bytes(&store, JOB_ID);
        assert!(open_recovered_store(&root, PROFILE, None).is_err());
        assert_eq!(durable_bytes(&store, JOB_ID), before);
        assert_eq!(store.get(JOB_ID).unwrap().unwrap().state, state);
    }
    drop(lease);
    let inner = runtime(Arc::clone(&store), &root);
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    finish_terminal(
        &inner,
        JOB_ID,
        transition(JobState::Succeeded, None),
        TerminalPayload {
            reply: Some("Synthetic exact reply".to_string()),
            error: None,
            receipt: None,
            state_override: None,
        },
        &started_tx,
        &AtomicBool::new(false),
    );
    assert!(matches!(started_rx.recv().unwrap(), StartSignal::Terminal));
    assert!(open_recovered_store(&root, PROFILE, None).is_err());
    let view = store.get(JOB_ID).unwrap().unwrap();
    assert_eq!(view.state, JobState::Succeeded);
    assert_eq!(view.error, None);
    let before = durable_bytes(&store, JOB_ID);
    drop(worker_lease);
    open_recovered_store(&root, PROFILE, None).unwrap();
    assert_eq!(durable_bytes(&store, JOB_ID), before);
}

#[test]
fn matching_request_is_idempotent_while_owner_is_live() {
    let root = temp("idempotency");
    let store = JobStore::open_at(&root, PROFILE).unwrap();
    let request = spec(&root, JOB_ID);
    assert!(store.existing(&request).unwrap().is_none());
    assert!(!store.namespace_dir().exists());
    let _lease = ExecutionLease::acquire_profile(&root, PROFILE, JOB_ID).unwrap();
    store.begin(&request).unwrap();
    let before = durable_bytes(&store, JOB_ID);
    assert_eq!(
        store.existing(&request).unwrap().unwrap().state,
        JobState::Preparing
    );
    let mut conflict = request.clone();
    conflict.request.prompt.push_str(" conflicting payload");
    assert_eq!(
        store.existing(&conflict).unwrap_err().kind,
        JobErrorKind::Conflict
    );
    assert!(store
        .existing(&spec(&root, SECOND_JOB_ID))
        .unwrap()
        .is_none());
    assert!(!store.namespace_dir().join(SECOND_JOB_ID).exists());
    assert_eq!(durable_bytes(&store, JOB_ID), before);
}

#[test]
fn canonical_existing_request_matches_resume_without_replaying() {
    let root = temp("canonical-idempotency");
    let store = JobStore::open_at(&root, PROFILE).unwrap();
    let mut request = spec(&root, JOB_ID);
    request.mode = JobMode::Review;
    request.request.conversation_mode = Some(ConversationMode::Canonical);
    store.begin(&request).unwrap();
    let before = durable_bytes(&store, JOB_ID);
    request.request.resume_id = request.request.session_id.take();
    assert_eq!(
        store.existing(&request).unwrap().unwrap().state,
        JobState::Preparing
    );
    assert!(!store.begin(&request).unwrap().created());
    assert_eq!(durable_bytes(&store, JOB_ID), before);
}

#[test]
fn distinct_profile_recovery_does_not_steal_an_owner() {
    let root = temp("distinct-profiles");
    let store = JobStore::open_at(&root, PROFILE).unwrap();
    let _lease = ExecutionLease::acquire_profile(&root, PROFILE, JOB_ID).unwrap();
    store.begin(&spec(&root, JOB_ID)).unwrap();
    let other = JobStore::open_at(&root, "profile-other").unwrap();
    other.begin(&spec(&root, SECOND_JOB_ID)).unwrap();
    let before = durable_bytes(&store, JOB_ID);
    open_recovered_store(&root, "profile-other", None).unwrap();
    assert_eq!(
        other.get(SECOND_JOB_ID).unwrap().unwrap().state,
        JobState::Interrupted
    );
    assert_eq!(durable_bytes(&store, JOB_ID), before);
}

#[test]
fn unavailable_recovery_lock_never_rewrites_jobs() {
    let root = temp("unavailable-lock");
    let store = JobStore::open_at(&root, PROFILE).unwrap();
    store.begin(&spec(&root, JOB_ID)).unwrap();
    fs::write(root.join("locks"), "Not a directory").unwrap();
    let before = durable_bytes(&store, JOB_ID);
    assert!(open_recovered_store(&root, PROFILE, None).is_err());
    assert_eq!(durable_bytes(&store, JOB_ID), before);
}

#[test]
fn corrupt_journal_aborts_recovery_before_transitions() {
    let root = temp("corrupt-recovery");
    let store = JobStore::open_at(&root, PROFILE).unwrap();
    store.begin(&spec(&root, JOB_ID)).unwrap();
    store.begin(&spec(&root, SECOND_JOB_ID)).unwrap();
    let invalid_path = store
        .namespace_dir()
        .join(SECOND_JOB_ID)
        .join("journal.jsonl");
    OpenOptions::new()
        .append(true)
        .open(invalid_path)
        .unwrap()
        .write_all(b"not-json\n")
        .unwrap();
    let before = durable_bytes(&store, JOB_ID);
    assert!(open_recovered_store(&root, PROFILE, None).is_err());
    assert_eq!(durable_bytes(&store, JOB_ID), before);
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
#[ignore]
fn synthetic_lease_owner_child() {
    let Some(root) = env::var_os("PARLEY_OWNERSHIP_TEST_ROOT").map(PathBuf::from) else {
        return;
    };
    let _lease = ExecutionLease::acquire_profile(&root, PROFILE, "synthetic-child").unwrap();
    let store = JobStore::open_at(&root, PROFILE).unwrap();
    store.begin(&spec(&root, JOB_ID)).unwrap();
    match env::var("PARLEY_OWNERSHIP_TEST_PHASE").unwrap().as_str() {
        "running" => {
            store
                .transition(JOB_ID, &transition(JobState::Running, None))
                .unwrap();
        }
        "cancelling" => {
            store
                .transition(JOB_ID, &transition(JobState::Running, None))
                .unwrap();
            store
                .transition(JOB_ID, &transition(JobState::Cancelling, None))
                .unwrap();
        }
        "preparing" => {}
        phase => panic!("unexpected test phase: {phase}"),
    }
    fs::write(root.join("ready"), b"ready").unwrap();
    let mut command = String::new();
    BufReader::new(std::io::stdin())
        .read_line(&mut command)
        .unwrap();
    assert_eq!(command.trim(), "exit");
}

#[test]
fn cross_process_live_owners_are_protected_and_dead_owners_recover_once() {
    for phase in ["preparing", "running", "cancelling"] {
        let root = temp(phase);
        let mut child = OwnedChild(
            Command::new(env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "job_runtime::ownership_tests::synthetic_lease_owner_child",
                    "--nocapture",
                ])
                .env("PARLEY_OWNERSHIP_TEST_ROOT", &root)
                .env("PARLEY_OWNERSHIP_TEST_PHASE", phase)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(0x08000000)
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !root.join("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "fixture child exited before readiness"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "fixture readiness timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let store = JobStore::open_at(&root, PROFILE).unwrap();
        let before = durable_bytes(&store, JOB_ID);
        assert!(open_recovered_store(&root, PROFILE, None).is_err());
        assert_eq!(durable_bytes(&store, JOB_ID), before);
        if phase == "cancelling" {
            child.0.kill().unwrap();
        } else {
            child
                .0
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"exit\n")
                .unwrap();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while child.0.try_wait().unwrap().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "fixture exit timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let recovered = open_recovered_store(&root, PROFILE, None).unwrap();
        let view = recovered.get(JOB_ID).unwrap().unwrap();
        assert_eq!(view.state, JobState::Interrupted);
        assert_eq!(view.error.as_deref(), Some(STARTUP_RECOVERY_ERROR));
        let stable = durable_bytes(&recovered, JOB_ID);
        open_recovered_store(&root, PROFILE, None).unwrap();
        assert_eq!(durable_bytes(&recovered, JOB_ID), stable);
        assert!(recovered
            .transition(JOB_ID, &transition(JobState::Running, None))
            .is_err());
        assert!(recovered
            .transition(JOB_ID, &transition(JobState::Succeeded, None))
            .is_err());
        assert!(!recovered.begin(&spec(&root, JOB_ID)).unwrap().created());
        assert_eq!(durable_bytes(&recovered, JOB_ID), stable);
    }
}

#[test]
#[ignore]
fn synthetic_runtime_child() {
    let root = PathBuf::from(env::var_os("PARLEY_OWNERSHIP_TEST_ROOT").unwrap());
    for key in env::vars_os().map(|(key, _)| key).collect::<Vec<_>>() {
        if key.to_string_lossy().starts_with("PARLEY_") {
            env::remove_var(key);
        }
    }
    let approved = root.join("approved");
    let parent = approved.join("parent");
    let extra = approved.join("extra");
    fs::create_dir_all(&parent).unwrap();
    fs::create_dir_all(&extra).unwrap();
    env::set_var("PARLEY_ASYNC_JOBS_ENABLED", "true");
    env::set_var("PARLEY_JOB_STATE_DIR", root.join("jobs"));
    env::set_var("PARLEY_ALLOWED_CWD_ROOT", &approved);
    env::set_var("PARLEY_CALLER", "synthetic-test");
    env::set_var("PARLEY_CODEX_HOME", root.join("codex-home"));
    env::set_var("PARLEY_EVENT_LOG", root.join("events.jsonl"));
    env::set_var("PATH", root.join("no-provider-bin"));
    let jobs = JobRuntime::from_env().unwrap();
    let inner = jobs.inner.as_ref().unwrap();
    let mut request = spec(&parent, JOB_ID);
    request.request = ask::prepare_for_job(&request.request).unwrap().0;
    let owner = ExecutionLease::acquire_for_job(
        &request.request,
        [parent.clone(), extra.clone(), extra.clone()],
        JOB_ID,
    )
    .unwrap();
    assert_eq!(
        fs::read_dir(root.join("jobs").join("locks"))
            .unwrap()
            .count(),
        4
    );
    inner.store.begin(&request).unwrap();
    let before = durable_bytes(&inner.store, JOB_ID);
    assert_eq!(
        jobs.start(request.clone()).unwrap().view.state,
        JobState::Preparing
    );
    assert!(jobs.start(spec(&parent, SECOND_JOB_ID)).is_err());
    assert!(!inner.store.namespace_dir().join(SECOND_JOB_ID).exists());
    let mut conflicting = request.clone();
    conflicting.request.prompt.push_str(" changed");
    assert!(jobs.start(conflicting).is_err());
    assert_eq!(durable_bytes(&inner.store, JOB_ID), before);
    assert!(!root.join("events.jsonl").exists());
    assert!(JobRuntime::from_env().is_err());
    assert_eq!(durable_bytes(&inner.store, JOB_ID), before);

    let synchronous = ask::run_prepared_with_receipt_controlled(
        &request.request,
        &RuntimePolicy::default(),
        None,
        &CancellationToken::default(),
        &JobObserver {
            store: Arc::clone(&inner.store),
            job_id: JOB_ID.to_string(),
            require_lane_result: false,
            handoff: None,
            started_tx: mpsc::sync_channel(1).0,
            sent: Arc::new(AtomicBool::new(false)),
        },
    );
    assert!(synchronous
        .err()
        .unwrap()
        .message
        .contains("execution lease preflight failed"));
    drop(owner);
    assert_eq!(
        jobs.start(request.clone()).unwrap().view.state,
        JobState::Preparing
    );
    drop(ExecutionLease::acquire_for_job(&request.request, Vec::new(), "released").unwrap());
    let recovered = JobRuntime::from_env().unwrap();
    assert_eq!(
        recovered.start(request).unwrap().view.state,
        JobState::Interrupted
    );

    let mut missing_context = spec(&parent, SECOND_JOB_ID);
    missing_context.request.context = Some(crate::ask::ContextRef {
        harness: "codex".to_string(),
        session: "synthetic-missing-source".to_string(),
        mode: crate::context::ContextMode::Snapshot,
        mode_explicit: true,
        recovery: None,
    });
    let outcome = recovered.start(missing_context.clone()).unwrap();
    assert_eq!(outcome.view.state, JobState::Failed);
    assert!(outcome
        .view
        .error
        .unwrap()
        .contains("context resolution failed"));
    recovered.drain();
    drop(
        ExecutionLease::acquire_for_job(&missing_context.request, Vec::new(), "post-terminal")
            .unwrap(),
    );

    env::remove_var("PARLEY_ASYNC_JOBS_ENABLED");
    assert!(
        ExecutionLease::acquire_for_job(&missing_context.request, Vec::new(), "disabled").is_err()
    );
}

#[test]
fn runtime_start_idempotency_contention_and_preflight_are_provider_free() {
    let root = temp("runtime-child");
    let mut child = OwnedChild(
        Command::new(env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "job_runtime::ownership_tests::synthetic_runtime_child",
                "--nocapture",
            ])
            .env("PARLEY_OWNERSHIP_TEST_ROOT", &root)
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x08000000)
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "runtime fixture timed out"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "stdout={} stderr={}", stdout, stderr);
}
