use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use parley_handoff::codex_activity::parse_rfc3339_ms;
use parley_handoff::schema::{ActivityRecord, StoredRecord};
use parley_handoff::service::{alert, evaluate_hook, peek, wait, Clock, HookDecision};
use parley_handoff::store::{load_readonly, repair_snapshot};
use parley_handoff::{sha256_hex, Binding, FailKind, RuntimeFacts};
use serde_json::Value;

const JOB: &str = "22222222-2222-4222-8222-222222222222";
const HANDOFF: &str = "11111111-1111-4111-8111-111111111111";
const SOURCE: &str = "33333333-3333-4333-8333-333333333333";
const TARGET: &str = "44444444-4444-4444-8444-444444444444";
const CAPABILITY: &str = "secret-capability-token";
const PID: u32 = 4242;

struct World {
    state: PathBuf,
    job: PathBuf,
    inbox: PathBuf,
    cwd: PathBuf,
    rollout: PathBuf,
    peek: PathBuf,
    wait_exe: PathBuf,
    alert_exe: PathBuf,
    hook: PathBuf,
    other: PathBuf,
    renamed: PathBuf,
}

struct JumpClock(AtomicU64);

impl Clock for JumpClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    fn sleep_ms(&self, milliseconds: u64) {
        self.0.fetch_add(milliseconds, Ordering::SeqCst);
    }
}

fn temp_dir(name: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let path = std::env::temp_dir().join(format!(
        "parley-handoff-{name}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn world() -> World {
    let root = temp_dir("world");
    let state = root.join("state");
    let job = state.join("jobs").join(JOB);
    let inbox = root.join("inbox");
    let cwd = root.join("cwd");
    fs::create_dir_all(&job).unwrap();
    fs::create_dir_all(&inbox).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let world = World {
        state,
        job,
        inbox,
        cwd,
        rollout: root.join("rollout.jsonl"),
        peek: bin.join("parley-peer-peek.exe"),
        wait_exe: bin.join("parley-handoff-wait.exe"),
        alert_exe: bin.join("parley-peer-alert.exe"),
        hook: bin.join("parley-handoff-hook.exe"),
        other: bin.join("renamed-peek.exe"),
        renamed: bin.join("renamed-helper.exe"),
    };
    fs::write(&world.peek, b"peek-image").unwrap();
    fs::write(&world.wait_exe, b"wait-image").unwrap();
    fs::write(&world.alert_exe, b"alert-image").unwrap();
    fs::write(&world.hook, b"hook-image").unwrap();
    fs::write(&world.other, b"other-image").unwrap();
    fs::hard_link(&world.peek, &world.renamed).unwrap();
    fs::write(world.job.join("report.txt"), b"REPORT_SECRET").unwrap();
    world
}

fn bound(world: &World) -> Binding {
    Binding {
        state_dir: world.state.clone(),
        job_dir: world.job.clone(),
        job_id: JOB.to_string(),
        handoff_id: HANDOFF.to_string(),
        capability: CAPABILITY.to_string(),
        source_session_id: SOURCE.to_string(),
        target_session_id: TARGET.to_string(),
        wait_slice_secs: 1,
        peek_exe: world.peek.clone(),
        wait_exe: world.wait_exe.clone(),
        alert_exe: world.alert_exe.clone(),
        hook_exe: world.hook.clone(),
        health_inbox: Some(world.inbox.clone()),
        codex_rollout: Some(world.rollout.clone()),
        codex_home: None,
    }
}

fn facts(world: &World, exe: &Path, pid: u32, ancestors: &[u32]) -> RuntimeFacts {
    RuntimeFacts {
        pid,
        ancestors: ancestors.to_vec(),
        exe: exe.to_path_buf(),
        cwd: world.cwd.clone(),
    }
}

fn record(state: &str, deadline: Option<u64>, receipt: Option<u64>) -> StoredRecord {
    StoredRecord {
        schema_version: 1,
        sequence: 1,
        job_id: JOB.to_string(),
        handoff_id: HANDOFF.to_string(),
        state: state.to_string(),
        source_session_id: Some(SOURCE.to_string()),
        target_session_id: Some(TARGET.to_string()),
        capability_hash: sha256_hex(CAPABILITY.as_bytes()),
        process_id: Some(u64::from(PID)),
        process_state: "alive".to_string(),
        phase: state.to_string(),
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
        last_activity_ms: Some(1_000),
        ready_at_ms: Some(1_000),
        deadline_ms: deadline,
        receipt_at_ms: receipt,
        wait_until_ms: None,
        report_fingerprint: if state == "pending" {
            None
        } else {
            Some(sha256_hex(b"report"))
        },
        alert_incident_id: None,
        excerpt: "GROK_EXCERPT_SECRET".to_string(),
        excerpt_truncated: false,
        activities: vec![ActivityRecord {
            class: "text".to_string(),
            timestamp_ms: 1_000,
            tool_name: None,
            status: None,
        }],
    }
}

fn write_records(world: &World, records: &[StoredRecord]) {
    let mut body = String::new();
    for record in records {
        body.push_str(&serde_json::to_string(record).unwrap());
        body.push('\n');
    }
    fs::write(world.job.join("journal.jsonl"), body).unwrap();
}

fn write_rollout(world: &World, visible: &str) {
    let body = format!(
        "\u{feff}{{\"timestamp\":\"2026-01-01T00:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SOURCE}\"}}}}\r\n\
         {{\"timestamp\":\"2026-01-01T00:00:01.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"reasoning\",\"summary\":[{{\"text\":\"SECRET_REASONING\"}}]}}}}\r\n\
         {{\"timestamp\":\"2026-01-01T00:00:02.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"function_call\",\"name\":\"shell\",\"arguments\":\"SECRET_ARGS\"}}}}\r\n\
         {{\"timestamp\":\"2026-01-01T00:00:03.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"function_call_output\",\"output\":\"SECRET_OUTPUT\"}}}}\r\n\
         {{\"timestamp\":\"2026-01-01T00:00:04.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"USER_PROMPT_SECRET\"}}]}}}}\r\n\
         {{\"timestamp\":\"2026-01-01T00:00:05.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"developer\",\"content\":[{{\"type\":\"input_text\",\"text\":\"SECRET_DEV\"}}]}}}}\r\n\
         {{\"timestamp\":\"2026-01-01T00:00:06.000Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"environment_context\",\"text\":\"SECRET_ENV\"}}}}\r\n\
         {{\"timestamp\":\"2026-01-01T00:00:07.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"phase\":\"commentary\",\"content\":[{{\"type\":\"output_text\",\"text\":\"{visible}\"}}]}}}}\r\n\
         {{\"timestamp\":\"2026-01-01T00:00:08.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"phase\":\"final_answer\",\"content\":[{{\"type\":\"output_text\",\"text\":\"VISIBLE_FINAL\"}}]}}}}\r\n\
         {{\"partial\"",
    );
    fs::write(&world.rollout, body).unwrap();
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() {
            files.push((
                path.file_name().unwrap().to_string_lossy().into_owned(),
                fs::read(&path).unwrap(),
            ));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn assert_public(text: &str) {
    assert!(!text.contains("stalled"), "{text}");
    for secret in [
        "SECRET_REASONING",
        "SECRET_ARGS",
        "SECRET_OUTPUT",
        "SECRET_DEV",
        "SECRET_ENV",
        "USER_PROMPT_SECRET",
        "GROK_EXCERPT_SECRET",
        "REPORT_SECRET",
        CAPABILITY,
    ] {
        assert!(!text.contains(secret), "{secret} leaked in {text}");
    }
}

#[test]
fn sha256_and_rfc3339_match_known_values() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        parse_rfc3339_ms("2026-01-01T00:00:00.000Z"),
        Some(1_767_225_600_000)
    );
    assert_eq!(parse_rfc3339_ms("2026-02-29T00:00:00Z"), None);
    assert!(parse_rfc3339_ms("2024-02-29T00:00:00Z").is_some());
}

#[test]
fn journal_recovers_partial_tail_repairs_stale_snapshot_and_rejects_malformed() {
    let world = world();
    let first = record("awaiting_ack", Some(50_000), None);
    write_rollout(&world, "VISIBLE_CODEX_NOTE");
    write_records(&world, std::slice::from_ref(&first));
    let path = world.job.join("journal.jsonl");
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(br#"{"partial""#);
    fs::write(&path, &bytes).unwrap();
    let loaded = load_readonly(&world.job).unwrap();
    assert_eq!(loaded.sequence, 1);
    assert!(fs::read(&path).unwrap().ends_with(br#"{"partial""#));

    let mut second = first.clone();
    second.sequence = 2;
    second.phase = "still_waiting".to_string();
    second.updated_at_ms = 2_000;
    write_records(&world, &[first.clone(), second.clone()]);
    fs::write(
        world.job.join("snapshot.json"),
        format!("{}\n", serde_json::to_string(&first).unwrap()),
    )
    .unwrap();
    #[cfg(windows)]
    let _open_snapshot = {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004)
            .open(world.job.join("snapshot.json"))
            .unwrap()
    };
    let before = fs::read(world.job.join("snapshot.json")).unwrap();
    let peek_facts = facts(&world, &world.peek, PID, &[]);
    let document = peek(&bound(&world), &peek_facts, 5_000);
    assert_eq!(document.unavailable.as_deref(), None);
    assert_eq!(fs::read(world.job.join("snapshot.json")).unwrap(), before);
    assert!(!world.job.join("lock").exists());

    let repaired = repair_snapshot(&world.job).unwrap();
    assert_eq!(repaired.sequence, 2);
    let snapshot: StoredRecord =
        serde_json::from_slice(&fs::read(world.job.join("snapshot.json")).unwrap()).unwrap();
    assert_eq!(snapshot.sequence, 2);
    assert!(world.job.join("lock").exists());

    let mut gap = second.clone();
    gap.sequence = 4;
    write_records(&world, &[first.clone(), gap]);
    assert_eq!(load_readonly(&world.job).unwrap_err(), FailKind::Malformed);

    let mut changed = second;
    changed.handoff_id = "99999999-9999-4999-8999-999999999999".to_string();
    write_records(&world, &[first, changed]);
    assert_eq!(load_readonly(&world.job).unwrap_err(), FailKind::Malformed);
}

#[test]
fn binding_failures_are_distinct_and_peek_does_not_write() {
    let world = world();
    write_rollout(&world, "VISIBLE_CODEX_NOTE");
    write_records(&world, &[record("awaiting_ack", Some(9_000_000), None)]);
    let before = snapshot(&world.job);
    let runtime_facts = facts(&world, &world.peek, PID, &[]);
    let document = peek(&bound(&world), &runtime_facts, 5_000);
    let text = serde_json::to_string(&document).unwrap();
    assert_eq!(document.assessment, "not_inferred");
    assert_eq!(document.phase, "final_answer");
    assert!(document.excerpt.text.contains("VISIBLE_CODEX_NOTE"));
    assert!(document.excerpt.text.contains("VISIBLE_FINAL"));
    assert!(text.contains("shell"));
    assert!(!text.contains("started") || text.contains("\"status\":\"started\""));
    assert_public(&text);
    assert_eq!(snapshot(&world.job), before);

    let mut capability = bound(&world);
    capability.capability = "other-token".to_string();
    assert_eq!(
        peek(&capability, &runtime_facts, 5_000)
            .unavailable
            .as_deref(),
        Some("capability")
    );
    let mut session = bound(&world);
    session.source_session_id = "other-session".to_string();
    assert_eq!(
        peek(&session, &runtime_facts, 5_000).unavailable.as_deref(),
        Some("session")
    );
    let wrong_process = facts(&world, &world.peek, 9, &[]);
    assert_eq!(
        peek(&bound(&world), &wrong_process, 5_000)
            .unavailable
            .as_deref(),
        Some("process")
    );
    let child = facts(&world, &world.peek, 9, &[PID]);
    assert!(peek(&bound(&world), &child, 5_000).unavailable.is_none());
    let wrong_file = facts(&world, &world.other, PID, &[]);
    assert_eq!(
        peek(&bound(&world), &wrong_file, 5_000)
            .unavailable
            .as_deref(),
        Some("file_identity")
    );
    write_records(&world, &[record("awaiting_ack", Some(1_000), None)]);
    assert_eq!(
        peek(&bound(&world), &runtime_facts, 1_000_000)
            .unavailable
            .as_deref(),
        Some("expired")
    );
    assert_eq!(snapshot(&world.job).len(), before.len());
}

#[test]
fn codex_excerpt_is_unicode_scalar_bounded() {
    let world = world();
    let marker = "é😀";
    let visible = format!("HEAD{}{marker}", "b".repeat(5_000));
    write_rollout(&world, &visible);
    write_records(&world, &[record("awaiting_ack", Some(9_000_000), None)]);
    let document = peek(&bound(&world), &facts(&world, &world.peek, PID, &[]), 5_000);
    assert!(document.excerpt.truncated);
    assert_eq!(document.excerpt.text.chars().count(), 4_096);
    assert!(document.excerpt.text.contains(marker));
    assert!(document.excerpt.text.ends_with("VISIBLE_FINAL"));
    assert!(!document.excerpt.text.contains("HEAD"));
    assert_public(&document.excerpt.text);
}

#[test]
fn missing_timestamps_never_fabricate_fresh_activity() {
    let world = world();
    fs::write(
        &world.rollout,
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SOURCE}\"}}}}\n\
             {{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"phase\":\"commentary\",\"content\":[{{\"type\":\"output_text\",\"text\":\"VISIBLE_WITHOUT_TIME\"}}]}}}}\n"
        ),
    )
    .unwrap();
    write_records(&world, &[record("awaiting_ack", Some(9_000_000), None)]);
    let document = peek(&bound(&world), &facts(&world, &world.peek, PID, &[]), 5_000);
    assert_eq!(document.phase, "commentary");
    assert_eq!(document.last_activity_ms, None);
    assert_eq!(document.age_ms, None);
    assert_eq!(document.freshness.as_deref(), Some("timestamp_unavailable"));
    assert!(document.excerpt.text.contains("VISIBLE_WITHOUT_TIME"));
}

#[test]
fn rollout_identity_survives_bounded_tail_reads() {
    let world = world();
    let mut file = fs::File::create(&world.rollout).unwrap();
    writeln!(
        file,
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SOURCE}\"}}}}"
    )
    .unwrap();
    file.write_all(b"{\"type\":\"event_msg\",\"padding\":\"")
        .unwrap();
    file.write_all(&vec![b'x'; 8_200_000]).unwrap();
    file.write_all(b"\"}\n").unwrap();
    let visible = format!("{}\u{00e9}\u{1f600}", "v".repeat(300_000));
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "timestamp": "2026-01-01T00:00:09.000Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "phase": "commentary",
                "content": [{"type": "output_text", "text": visible}]
            }
        })
    )
    .unwrap();
    file.flush().unwrap();
    write_records(&world, &[record("awaiting_ack", Some(9_000_000), None)]);

    let document = peek(&bound(&world), &facts(&world, &world.peek, PID, &[]), 5_000);
    assert_eq!(document.unavailable, None);
    assert_eq!(document.last_activity_ms, Some(1_767_225_609_000));
    assert_eq!(document.excerpt.text.chars().count(), 4_096);
    assert!(document.excerpt.text.ends_with("\u{00e9}\u{1f600}"));
}

#[test]
fn missing_stale_and_silent_rollouts_are_explicit() {
    let world = world();
    write_records(&world, &[record("awaiting_ack", Some(9_000_000), None)]);
    let runtime_facts = facts(&world, &world.peek, PID, &[]);

    let missing = peek(&bound(&world), &runtime_facts, 5_000);
    assert_eq!(missing.unavailable.as_deref(), Some("missing"));

    fs::write(
        &world.rollout,
        b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"wrong-session\"}}\n",
    )
    .unwrap();
    let stale = peek(&bound(&world), &runtime_facts, 5_000);
    assert_eq!(stale.unavailable.as_deref(), Some("stale"));

    fs::write(
        &world.rollout,
        format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SOURCE}\"}}}}\n"),
    )
    .unwrap();
    let silent = peek(&bound(&world), &runtime_facts, 5_000);
    assert_eq!(silent.unavailable, None);
    assert!(!silent.excerpt.available);
    assert_eq!(silent.freshness.as_deref(), Some("no_activity_observed"));
    assert_eq!(silent.assessment, "not_inferred");
}

#[test]
fn wait_is_bounded_and_never_records_receipt() {
    let world = world();
    write_rollout(&world, "VISIBLE_CODEX_NOTE");
    let start = 5_000_000_u64;
    write_records(
        &world,
        &[record("awaiting_ack", Some(start + 1_000_000), None)],
    );
    let clock = JumpClock(AtomicU64::new(start));
    let mut binding = bound(&world);
    binding.wait_slice_secs = 1;
    let json = wait(&binding, &facts(&world, &world.wait_exe, PID, &[]), &clock).unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["receipt_state"], "pending");
    assert_eq!(value["deadline_state"], "pending");
    let until = value["wait_until_ms"].as_u64().unwrap();
    assert!(until <= start + 1_000);
    assert_eq!(load_readonly(&world.job).unwrap().receipt_at_ms, None);
    assert_public(&json);

    write_records(
        &world,
        &[record("acknowledged", Some(start + 1_000_000), Some(42))],
    );
    let before = fs::read(world.job.join("journal.jsonl")).unwrap();
    let json = wait(&binding, &facts(&world, &world.wait_exe, PID, &[]), &clock).unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["receipt_state"], "received");
    assert_eq!(fs::read(world.job.join("journal.jsonl")).unwrap(), before);

    write_records(&world, &[record("awaiting_ack", Some(start), None)]);
    let clock = JumpClock(AtomicU64::new(start));
    let json = wait(&binding, &facts(&world, &world.wait_exe, PID, &[]), &clock).unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["deadline_state"], "expired");
    assert!(value["wait_until_ms"].as_u64().unwrap() <= start + 1_000);
    assert_eq!(load_readonly(&world.job).unwrap().receipt_at_ms, None);
}

#[test]
fn alert_is_idempotent_sanitized_and_silent() {
    let world = world();
    write_rollout(&world, "VISIBLE_CODEX_NOTE");
    write_records(&world, &[record("awaiting_ack", Some(9_000_000), None)]);
    let binding = bound(&world);
    let facts = facts(&world, &world.alert_exe, PID, &[]);
    let first: Value = serde_json::from_str(&alert(&binding, &facts, 5_000).unwrap()).unwrap();
    let second: Value = serde_json::from_str(&alert(&binding, &facts, 6_000).unwrap()).unwrap();
    assert_eq!(first["sound"], false);
    assert_eq!(second["sound"], false);
    assert_eq!(first["created"], true);
    assert_eq!(second["created"], false);
    assert_eq!(first["incident_id"], second["incident_id"]);
    let files: Vec<_> = fs::read_dir(&world.inbox).unwrap().collect();
    assert_eq!(files.len(), 1);
    let body = fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap();
    let record: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(record["schema_version"], 3);
    assert_eq!(record["class"], "handoff_unacknowledged");
    assert_eq!(record["kind"], "peer_alert_requested");
    assert_eq!(record["incident_id"], first["incident_id"]);
    assert_public(&first.to_string());
    assert_public(&body);
    assert_eq!(
        load_readonly(&world.job)
            .unwrap()
            .alert_incident_id
            .as_deref(),
        first["incident_id"].as_str()
    );

    fs::write(files[0].as_ref().unwrap().path(), b"{}").unwrap();
    assert_eq!(
        alert(&binding, &facts, 7_000).unwrap_err(),
        FailKind::Malformed
    );
}

#[test]
fn existing_v2_alert_file_is_preserved_without_rewrite() {
    let world = world();
    write_rollout(&world, "VISIBLE_CODEX_NOTE");
    let incident_id = format!("handoff-{HANDOFF}");
    let mut prior = record("awaiting_ack", Some(9_000_000), None);
    prior.alert_incident_id = Some(incident_id.clone());
    write_records(&world, &[prior]);
    let legacy = format!(
        "{{\"schema_version\":2,\"inbox_id\":\"{incident_id}\",\"kind\":\"peer_alert_requested\",\"as_of_ms\":1,\"source\":\"grok\",\"class\":\"handoff_unacknowledged\",\"incident_id\":\"{incident_id}\",\"session_id\":\"{TARGET}\",\"exchange_id\":\"{JOB}\"}}"
    );
    let path = world.inbox.join(format!("{incident_id}.json"));
    fs::write(&path, &legacy).unwrap();
    let binding = bound(&world);
    let facts = facts(&world, &world.alert_exe, PID, &[]);
    let response: Value = serde_json::from_str(&alert(&binding, &facts, 5_000).unwrap()).unwrap();
    assert_eq!(response["created"], false);
    assert_eq!(response["incident_id"], incident_id);
    assert_eq!(fs::read_to_string(&path).unwrap(), legacy);
    let stored: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(stored["schema_version"], 2);
}

#[test]
fn hook_denies_unsafe_invocations_and_gates_stop_without_touching_unrelated_calls() {
    let world = world();
    write_rollout(&world, "VISIBLE_CODEX_NOTE");
    write_records(&world, &[record("awaiting_ack", Some(9_000_000), None)]);
    let binding = bound(&world);
    let facts = facts(&world, &world.hook, PID, &[]);
    let peek_path = world.peek.display().to_string();

    assert_eq!(
        evaluate_hook(b"{", None, None, 5_000).decision,
        HookDecision::Silent
    );
    assert_eq!(
        evaluate_hook(&stop_event(&world.cwd), None, None, 5_000).decision,
        HookDecision::Silent
    );
    assert_eq!(
        evaluate_hook(&shell_event(&world.cwd, &peek_path), None, None, 5_000).decision,
        HookDecision::Deny
    );
    assert_eq!(
        evaluate_hook(
            &shell_event(&world.cwd, "echo \"unterminated"),
            None,
            None,
            5_000
        )
        .decision,
        HookDecision::Silent
    );

    let unrelated = hook_event(
        "PreToolUse",
        "read_file",
        &world.cwd,
        serde_json::json!({"target_file": "SECRET_PATH"}),
    );
    let output = evaluate_hook(&unrelated, Some(&binding), Some(&facts), 5_000);
    assert_eq!(output.decision, HookDecision::Silent);
    assert!(output.stdout.is_none());
    assert!(!world.job.join("lock").exists());

    let exact = shell_event(&world.cwd, &peek_path);
    let output = evaluate_hook(&exact, Some(&binding), Some(&facts), 5_000);
    assert_eq!(output.decision, HookDecision::Silent, "{:?}", output.stdout);

    for command in [
        format!("powershell.exe -Command {peek_path}"),
        format!("{peek_path} > out.txt"),
        format!("{peek_path} && echo SECRET_CMD"),
        format!("{peek_path} --extra"),
        format!("\"{peek_path}"),
        format!("{peek_path}`"),
        renamed_helper(&world),
    ] {
        let output = evaluate_hook(
            &shell_event(&world.cwd, &command),
            Some(&binding),
            Some(&facts),
            5_000,
        );
        assert_eq!(output.decision, HookDecision::Deny, "{command}");
        let stdout = output.stdout.unwrap();
        assert!(stdout.contains("\"decision\":\"deny\""));
        assert!(!stdout.contains("SECRET_CMD"));
        assert!(!stdout.contains(&peek_path));
    }

    let mut wrong = binding.clone();
    wrong.capability = "nope".to_string();
    let output = evaluate_hook(&exact, Some(&wrong), Some(&facts), 5_000);
    assert!(output.stdout.unwrap().contains("capability"));

    let output = evaluate_hook(&stop_event(&world.cwd), Some(&binding), Some(&facts), 5_000);
    assert_eq!(output.decision, HookDecision::Deny);

    let missing_context =
        serde_json::to_vec(&serde_json::json!({"hook_event_name": "Stop"})).unwrap();
    assert_eq!(
        evaluate_hook(&missing_context, Some(&binding), Some(&facts), 5_000).decision,
        HookDecision::Deny
    );
    let conflicting_context = serde_json::to_vec(&serde_json::json!({
        "hook_event_name": "Stop",
        "sessionId": TARGET,
        "session_id": "wrong-session",
        "cwd": world.cwd.display().to_string()
    }))
    .unwrap();
    assert_eq!(
        evaluate_hook(&conflicting_context, Some(&binding), Some(&facts), 5_000).decision,
        HookDecision::Deny
    );

    write_records(&world, &[record("pending", Some(9_000_000), None)]);
    let output = evaluate_hook(&stop_event(&world.cwd), Some(&binding), Some(&facts), 5_000);
    assert_eq!(output.decision, HookDecision::Silent);

    write_records(&world, &[record("acknowledged", Some(9_000_000), Some(7))]);
    let output = evaluate_hook(&stop_event(&world.cwd), Some(&binding), Some(&facts), 5_000);
    assert_eq!(output.decision, HookDecision::Silent);

    write_records(&world, &[record("catastrophe_exit", Some(9_000_000), None)]);
    let output = evaluate_hook(&stop_event(&world.cwd), Some(&binding), Some(&facts), 5_000);
    assert_eq!(output.decision, HookDecision::Silent);

    fs::remove_file(world.job.join("journal.jsonl")).unwrap();
    let output = evaluate_hook(&stop_event(&world.cwd), Some(&binding), Some(&facts), 5_000);
    assert_eq!(output.decision, HookDecision::Deny);
}

#[test]
fn helper_binaries_reject_arguments_and_peek_stays_nonjudgmental() {
    for exe in [
        env!("CARGO_BIN_EXE_parley-peer-peek"),
        env!("CARGO_BIN_EXE_parley-handoff-wait"),
        env!("CARGO_BIN_EXE_parley-peer-alert"),
        env!("CARGO_BIN_EXE_parley-handoff-hook"),
    ] {
        let output = Command::new(exe).arg("nope").env_clear().output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{exe}");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_parley-peer-peek"))
        .env_clear()
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("not_inferred"));
    assert!(!text.contains("stalled"));

    let hook = env!("CARGO_BIN_EXE_parley-handoff-hook");
    let payload = serde_json::to_vec(&serde_json::json!({"hook_event_name": "Stop"})).unwrap();
    let mut unbound = Command::new(hook)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    unbound.stdin.take().unwrap().write_all(&payload).unwrap();
    let unbound = unbound.wait_with_output().unwrap();
    assert!(unbound.status.success());
    assert!(unbound.stdout.is_empty());

    let mut partial = Command::new(hook)
        .env_clear()
        .env("PARLEY_HANDOFF_JOB_ID", JOB)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    partial.stdin.take().unwrap().write_all(&payload).unwrap();
    let partial = partial.wait_with_output().unwrap();
    assert!(partial.status.success());
    assert!(String::from_utf8(partial.stdout)
        .unwrap()
        .contains("\"decision\":\"deny\""));
}

fn renamed_helper(world: &World) -> String {
    world.renamed.display().to_string()
}

fn shell_event(cwd: &Path, command: &str) -> Vec<u8> {
    hook_event(
        "PreToolUse",
        "run_terminal_command",
        cwd,
        serde_json::json!({"command": command}),
    )
}

fn stop_event(cwd: &Path) -> Vec<u8> {
    hook_event("Stop", "unused", cwd, serde_json::json!({}))
}

fn hook_event(name: &str, tool: &str, cwd: &Path, input: Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "hook_event_name": name,
        "sessionId": TARGET,
        "toolUseId": "tool-1",
        "toolInputTruncated": false,
        "cwd": cwd.display().to_string(),
        "toolName": tool,
        "toolInput": input,
        "prompt": "SECRET_PROMPT"
    }))
    .unwrap()
}
