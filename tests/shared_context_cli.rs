use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "parley-context-cli-{}-{}-{name}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn write_rollout(codex_home: &Path, session: &str) {
    let directory = codex_home
        .join("sessions")
        .join("2026")
        .join("09")
        .join("21");
    fs::create_dir_all(&directory).unwrap();
    let body = format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session}\"}}}}\n\
         {{\"type\":\"turn_context\",\"payload\":{{\"turn_id\":\"turn-1\"}}}}\n\
         {{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"id\":\"message-1\",\"content\":[{{\"type\":\"input_text\",\"text\":\"SEED_MARKER\"}}]}}}}\n"
    );
    fs::write(
        directory.join(format!("rollout-2026-09-21T00-00-00-{session}.jsonl")),
        body,
    )
    .unwrap();
}

fn par() -> Command {
    Command::new(env!("CARGO_BIN_EXE_par"))
}

#[test]
fn stateful_dry_run_is_fully_non_mutating_and_reports_prompt_transport() {
    let root = temp_dir("dry-run");
    let codex_home = root.join("codex-home");
    let state = root.join("context-state");
    let event_log = root.join("events.jsonl");
    let cwd = root.join("worker");
    fs::create_dir_all(&cwd).unwrap();
    write_rollout(&codex_home, "source-session");

    let output = par()
        .args([
            "ask",
            "-h",
            "grok",
            "-p",
            "task: dry run",
            "--context-from",
            "codex:source-session",
            "--context-mode",
            "auto",
            "--session-id",
            "01a06582-d66e-7811-b0c9-0b0266e17903",
            "--no-yolo",
            "--dry-run",
            "--cwd",
        ])
        .arg(&cwd)
        .env("PARLEY_CODEX_HOME", &codex_home)
        .env("PARLEY_CONTEXT_STATE_DIR", &state)
        .env("PARLEY_EVENT_LOG", &event_log)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--prompt-file"));
    assert!(stdout.contains("<PARLEY_PROMPT_FILE>"));
    assert!(stdout.contains("prompt_buffer"));
    assert!(stdout.contains("character_count"));
    assert!(stdout.contains("fingerprint"));
    assert!(!state.exists());
    assert!(!event_log.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn canonical_dry_run_derives_a_stable_new_session_without_writing_state() {
    let root = temp_dir("canonical-dry-run");
    let codex_home = root.join("codex-home");
    let state = root.join("context-state");
    let event_log = root.join("events.jsonl");
    let approved_root = root.join("approved");
    let cwd = approved_root.join("worker");
    let second_cwd = approved_root.join("second-worker");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&second_cwd).unwrap();
    write_rollout(&codex_home, "source-session");

    let run = |run_cwd: &Path| {
        par()
            .args([
                "ask",
                "-h",
                "grok",
                "-p",
                "task: canonical dry run",
                "--context-from",
                "codex:source-session",
                "--no-yolo",
                "--dry-run",
                "--cwd",
            ])
            .arg(run_cwd)
            .env("PARLEY_ALLOWED_CWD_ROOT", &approved_root)
            .env("PARLEY_CALLER", "codex")
            .env("PARLEY_CODEX_HOME", &codex_home)
            .env("PARLEY_CONTEXT_STATE_DIR", &state)
            .env("PARLEY_EVENT_LOG", &event_log)
            .env("PARLEY_CONTEXT_LOCKED_SOURCE", "codex")
            .env("PARLEY_CONTEXT_LOCKED_MODE", "auto")
            .env("PARLEY_CONTEXT_REQUIRE_EXPLICIT_SESSION", "true")
            .env("PARLEY_GROK_REQUIRE_SESSION_ID", "true")
            .env("PARLEY_GROK_DEFAULT_CONVERSATION_MODE", "canonical")
            .env("PARLEY_GROK_CANONICAL_EPOCH", "0")
            .output()
            .unwrap()
    };

    let first = run(&cwd);
    let second = run(&cwd);
    let other_worktree = run(&second_cwd);
    for output in [&first, &second, &other_worktree] {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let first_stdout = String::from_utf8(first.stdout).unwrap();
    let second_stdout = String::from_utf8(second.stdout).unwrap();
    let other_stdout = String::from_utf8(other_worktree.stdout).unwrap();
    let first_session = argument_after(&first_stdout, "--session-id").unwrap();
    let second_session = argument_after(&second_stdout, "--session-id").unwrap();
    let other_session = argument_after(&other_stdout, "--session-id").unwrap();
    assert_eq!(first_session, second_session);
    assert_ne!(first_session, other_session);
    assert_eq!(first_session.len(), 36);
    assert!(!first_stdout.contains("--resume"));
    assert!(!state.exists());
    assert!(!event_log.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn context_resolution_failure_logs_only_the_current_request_pair() {
    let root = temp_dir("preflight");
    let codex_home = root.join("codex-home");
    let state = root.join("context-state");
    let event_log = root.join("events.jsonl");
    let cwd = root.join("worker");
    fs::create_dir_all(codex_home.join("sessions")).unwrap();
    fs::create_dir_all(&cwd).unwrap();

    let output = par()
        .args([
            "ask",
            "-h",
            "grok",
            "-p",
            "task: current only",
            "--context-from",
            "codex:missing-session",
            "--context-mode",
            "auto",
            "--session-id",
            "01a06582-d66e-7811-b0c9-0b0266e17903",
            "--no-yolo",
            "--cwd",
        ])
        .arg(&cwd)
        .env("PARLEY_CODEX_HOME", &codex_home)
        .env("PARLEY_CONTEXT_STATE_DIR", &state)
        .env("PARLEY_EVENT_LOG", &event_log)
        .env("PARLEY_CALLER", "codex")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let events = fs::read_to_string(&event_log).unwrap();
    let lines = events.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("\"event_type\":\"request\""));
    assert!(lines[0].contains("task: current only"));
    assert!(!lines[0].contains("PARLEY_UNTRUSTED_CONTEXT_V1"));
    assert!(lines[1].contains("\"event_type\":\"error\""));
    assert!(lines[1].contains("\"status\":\"context_preflight_error\""));
    assert!(!state.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn long_grok_dry_run_keeps_prompt_out_of_argv() {
    let root = temp_dir("long-prompt");
    let cwd = root.join("worker");
    fs::create_dir_all(&cwd).unwrap();
    let prompt = format!("LONG_MARKER_{}", "x".repeat(25_000));
    let output = par()
        .args(["ask", "-h", "grok", "-p"])
        .arg(&prompt)
        .args(["--no-yolo", "--dry-run", "--cwd"])
        .arg(&cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--prompt-file"));
    assert!(!stdout.contains("LONG_MARKER"));
    assert!(stdout.contains("\"character_count\": 25012"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn image_dry_run_is_non_mutating_and_keeps_paths_and_bytes_out_of_output() {
    let root = temp_dir("image-dry-run");
    let cwd = root.join("worker");
    let image_state = root.join("image-state");
    let event_log = root.join("events.jsonl");
    fs::create_dir_all(&cwd).unwrap();
    let image = cwd.join("capture.png");
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&13_u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&4_u32.to_be_bytes());
    bytes.extend_from_slice(&5_u32.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(b"IEND");
    bytes.extend_from_slice(&[0; 4]);
    fs::write(&image, bytes).unwrap();

    let output = par()
        .args([
            "ask",
            "-h",
            "grok",
            "-p",
            "inspect",
            "--model",
            "grok-4.7",
            "--reasoning-effort",
            "xhigh",
            "--resume-id",
            "01a06582-d66e-7811-b0c9-0b0266e17903",
            "--no-yolo",
            "--dry-run",
            "--image",
        ])
        .arg(&image)
        .args(["--cwd"])
        .arg(&cwd)
        .env("PARLEY_IMAGE_STATE_DIR", &image_state)
        .env("PARLEY_EVENT_LOG", &event_log)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("<PARLEY_IMAGE_PROFILE>"));
    assert!(stdout.contains("prompt_buffer"));
    assert!(!stdout.contains(image.to_string_lossy().as_ref()));
    assert!(!stdout.contains("iVBOR"));
    assert!(!image_state.exists());
    assert!(!event_log.exists());
    fs::remove_dir_all(root).unwrap();
}

fn argument_after<'a>(json: &'a str, flag: &str) -> Option<&'a str> {
    let marker = format!("\"{flag}\"");
    let after_flag = json.get(json.find(&marker)? + marker.len()..)?;
    let value_start = after_flag.find('"')? + 1;
    let value = after_flag.get(value_start..)?;
    value.get(..value.find('"')?)
}
