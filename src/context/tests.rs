use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::error::ErrorKind;
use super::journal::{ContextKey, CursorState, JournalStore};
use super::plan::{ContextBounds, PlanKind};
use super::reader::{SharedJsonlReader, MAX_PHYSICAL_RECORD};
use super::root::{resolve_codex_rollout, resolve_codex_sessions_root, CodexHomeEnv};
use super::sanitize::visible_messages;
use super::winfile::{SHARE_ALL, SHARE_DELETE, SHARE_READ, SHARE_WRITE};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

static SEQ: AtomicU64 = AtomicU64::new(1);

const SYNTH_STRINGS: &[&str] = &[
    "SYNTH_USER_ALPHA",
    "SYNTH_COMMENTARY_CHARLIE",
    "SYNTH_FINAL_DELTA",
    "SYNTH_HIDDEN_REASONING",
    "SYNTH_TOOL_ECHO",
    "SYNTH_DEV_HIDDEN",
    "SYNTH_SYS_HIDDEN",
    "SYNTH_COMPACTED",
    "SYNTH_EVENT_HIDDEN",
    "SYNTH_ENV_ACCOUNT",
    "café 😀",
];

fn temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "parley-ctx-{}-{}-{name}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn write_rollout(codex_home: &Path, session: &str, body: &str) -> PathBuf {
    let dir = codex_home
        .join("sessions")
        .join("2026")
        .join("09")
        .join("21");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("rollout-2026-09-21T00-00-00-{session}.jsonl"));
    fs::write(&path, body).unwrap();
    path
}

fn jsonl(lines: &[String]) -> String {
    let mut body = lines.join("\n");
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body
}

fn esc(text: &str) -> String {
    crate::json::escape_json(text)
}

fn session_meta(id: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-01-01T00:00:00.000Z","type":"session_meta","payload":{{"id":"{id}","cwd":"C:\\synth"}}}}"#
    )
}

fn turn_context(id: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-01-01T00:00:01.000Z","type":"turn_context","payload":{{"turn_id":"{id}"}}}}"#
    )
}

fn user_msg(id: &str, text: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-01-01T00:00:02.000Z","type":"response_item","payload":{{"type":"message","role":"user","id":"{id}","content":[{{"type":"input_text","text":"{}"}}]}}}}"#,
        esc(text)
    )
}

fn assistant_msg(id: &str, text: &str, phase: Option<&str>) -> String {
    let phase = match phase {
        Some(phase) => format!(r#","phase":"{phase}""#),
        None => String::new(),
    };
    format!(
        r#"{{"timestamp":"2026-01-01T00:00:03.000Z","type":"response_item","payload":{{"type":"message","role":"assistant","id":"{id}"{phase},"content":[{{"type":"output_text","text":"{}"}}]}}}}"#,
        esc(text)
    )
}

fn reasoning() -> String {
    r#"{"timestamp":"2026-01-01T00:00:04.000Z","type":"response_item","payload":{"type":"reasoning","summary":[{"text":"SYNTH_HIDDEN_REASONING"}]}}"#.to_string()
}

fn bounds(seed: usize, delta: usize, hard: usize) -> ContextBounds {
    ContextBounds {
        seed_max_chars: seed,
        delta_max_chars: delta,
        hard_max_chars: hard,
    }
}

fn key(target: &str, source: &str) -> ContextKey {
    ContextKey::new(target, "codex", source).unwrap()
}

fn env_home(codex_home: &Path) -> CodexHomeEnv {
    CodexHomeEnv {
        parley_codex_home: Some(codex_home.to_path_buf()),
        ..CodexHomeEnv::default()
    }
}

fn resolve(codex_home: &Path, session: &str) -> super::root::ResolvedSource {
    let root = resolve_codex_sessions_root(&env_home(codex_home)).unwrap();
    resolve_codex_rollout(&root, session).unwrap()
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/context/synthetic-rollout.jsonl")
}

fn store_at(root: &Path) -> JournalStore {
    JournalStore::open_at(root.to_path_buf())
}

fn commit_seed(
    store: &JournalStore,
    key: &ContextKey,
    source: &super::root::ResolvedSource,
    seed: usize,
    exchange: &str,
) -> super::plan::ContextPlan {
    let plan = store
        .plan_new_session_seed(key, source, &bounds(seed, seed, seed))
        .unwrap();
    store.append_in_flight(key, &plan, exchange).unwrap();
    store.commit(key, &plan, exchange).unwrap();
    plan
}

#[cfg(windows)]
fn make_junction(link: &Path, target: &Path) {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let status = std::process::Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            &link.to_string_lossy(),
            &target.to_string_lossy(),
        ])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "failed to create junction {}",
        link.display()
    );
}

#[test]
fn max_physical_record_is_8_mib() {
    assert_eq!(MAX_PHYSICAL_RECORD, 8 * 1024 * 1024);
}

#[test]
fn root_precedence_uses_parley_codex_home() {
    let parley = temp_dir("parley-home");
    let codex = temp_dir("codex-home");
    let home = temp_dir("user-home");
    let userprofile = temp_dir("userprofile");
    write_rollout(
        &parley,
        "sess-parley",
        &jsonl(&[session_meta("sess-parley")]),
    );
    write_rollout(&codex, "sess-codex", &jsonl(&[session_meta("sess-codex")]));
    write_rollout(
        &home.join(".codex"),
        "sess-home",
        &jsonl(&[session_meta("sess-home")]),
    );
    write_rollout(
        &userprofile.join(".codex"),
        "sess-profile",
        &jsonl(&[session_meta("sess-profile")]),
    );
    let env = CodexHomeEnv {
        parley_codex_home: Some(parley.clone()),
        codex_home: Some(codex),
        home: Some(home),
        userprofile: Some(userprofile),
    };
    let root = resolve_codex_sessions_root(&env).unwrap();
    let found = resolve_codex_rollout(&root, "sess-parley").unwrap();
    assert!(found.path.starts_with(&root) || found.identity.canonical_path.contains("parley-home"));
    let missing = resolve_codex_rollout(&root, "sess-codex");
    assert_eq!(missing.unwrap_err().kind, ErrorKind::NotFound);
}

#[test]
fn userprofile_fallback_when_home_is_unset() {
    let userprofile = temp_dir("profile-only");
    write_rollout(
        &userprofile.join(".codex"),
        "sess-profile",
        &jsonl(&[session_meta("sess-profile")]),
    );
    let env = CodexHomeEnv {
        userprofile: Some(userprofile),
        ..CodexHomeEnv::default()
    };
    let root = resolve_codex_sessions_root(&env).unwrap();
    let found = resolve_codex_rollout(&root, "sess-profile").unwrap();
    assert_eq!(found.session_id, "sess-profile");
}

#[cfg(windows)]
#[test]
fn junction_and_canonical_paths_dedup_to_one_rollout() {
    let home = temp_dir("junction-home");
    let real_year = home
        .join("sessions")
        .join("real-year")
        .join("09")
        .join("21");
    fs::create_dir_all(&real_year).unwrap();
    let path = real_year.join("rollout-2026-09-21T00-00-00-sess-junc.jsonl");
    fs::write(&path, jsonl(&[session_meta("sess-junc")])).unwrap();
    make_junction(
        &home.join("sessions").join("2026"),
        &home.join("sessions").join("real-year"),
    );
    let root = resolve_codex_sessions_root(&env_home(&home)).unwrap();
    let found = resolve_codex_rollout(&root, "sess-junc").unwrap();
    assert_eq!(found.session_id, "sess-junc");
}

#[test]
fn explicit_session_resolution_and_duplicate_match_fail_closed() {
    let home = temp_dir("dup-home");
    write_rollout(&home, "sess-one", &jsonl(&[session_meta("sess-one")]));
    let root = resolve_codex_sessions_root(&env_home(&home)).unwrap();
    let found = resolve_codex_rollout(&root, "sess-one").unwrap();
    assert_eq!(found.session_id, "sess-one");
    assert!(resolve_codex_rollout(&root, "sess-missing").is_err());

    let extra = home.join("sessions").join("2026").join("09").join("22");
    fs::create_dir_all(&extra).unwrap();
    fs::write(
        extra.join("rollout-2026-09-22T00-00-00-copy.jsonl"),
        jsonl(&[session_meta("sess-one")]),
    )
    .unwrap();
    let duplicate = resolve_codex_rollout(&root, "sess-one").unwrap_err();
    assert_eq!(duplicate.kind, ErrorKind::Duplicate);
}

#[test]
fn unrelated_malformed_rollouts_do_not_block_explicit_resolution() {
    let home = temp_dir("malformed-unrelated-home");
    write_rollout(
        &home,
        "wanted-session",
        &jsonl(&[session_meta("wanted-session")]),
    );
    write_rollout(&home, "broken-other", "{not-json}\n");
    let found = resolve(&home, "wanted-session");
    assert_eq!(found.session_id, "wanted-session");

    let selected = write_rollout(&home, "broken-selected", "{not-json}\n");
    assert!(selected
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("broken-selected"));
    let sessions = resolve_codex_sessions_root(&env_home(&home)).unwrap();
    assert!(resolve_codex_rollout(&sessions, "broken-selected").is_err());
}

#[cfg(windows)]
#[test]
fn reader_allows_shared_write_and_delete() {
    let home = temp_dir("share-home");
    let path = write_rollout(
        &home,
        "sess-share",
        &jsonl(&[session_meta("sess-share"), user_msg("u1", "hello")]),
    );
    let mut writer = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(SHARE_ALL)
        .open(&path)
        .unwrap();
    let mut reader = SharedJsonlReader::open(&path).unwrap();
    let first = reader.read_from(0).unwrap();
    assert_eq!(first.records.len(), 2);
    writer.seek(SeekFrom::End(0)).unwrap();
    writer
        .write_all(assistant_msg("a1", "there", None).as_bytes())
        .unwrap();
    writer.write_all(b"\n").unwrap();
    writer.flush().unwrap();
    let extra = reader.read_from(first.complete_end).unwrap();
    assert_eq!(extra.records.len(), 1);
    fs::remove_file(&path).unwrap();
    let still = reader.read_from(0).unwrap();
    assert!(!still.records.is_empty());
    let _ = SHARE_READ | SHARE_WRITE | SHARE_DELETE;
}

#[test]
fn reader_accepts_bom_crlf_unicode_and_retains_incomplete_tail() {
    let home = temp_dir("bom-home");
    let path = write_rollout(&home, "sess-bom", "");
    let mut body = b"\xEF\xBB\xBF".to_vec();
    let lines = [
        session_meta("sess-bom"),
        user_msg("u1", "café 😀"),
        assistant_msg("a1", "ok", None),
    ];
    body.extend_from_slice(lines.join("\r\n").as_bytes());
    body.extend_from_slice(b"\r\n{\"timestamp\":\"x\",\"type\":\"response_item\"");
    fs::write(&path, body).unwrap();
    let mut reader = SharedJsonlReader::open(&path).unwrap();
    let batch = reader.read_from(0).unwrap();
    assert_eq!(batch.records.len(), 3);
    assert_eq!(batch.records[0].start_offset, 3);
    assert!(!batch.incomplete_tail.is_empty());
    let messages = visible_messages(&batch.records).unwrap();
    assert_eq!(messages[0].text, "café 😀");
    assert_eq!(messages[0].text.chars().count(), 6);
}

#[test]
fn malformed_and_oversized_records_fail_closed() {
    let home = temp_dir("bad-home");
    let malformed = write_rollout(
        &home,
        "sess-bad",
        &jsonl(&[session_meta("sess-bad"), "{not-json}".to_string()]),
    );
    let mut reader = SharedJsonlReader::open(&malformed).unwrap();
    let batch = reader.read_from(0).unwrap();
    let err = visible_messages(&batch.records).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);

    let invalid = write_rollout(&home, "sess-utf", "");
    let mut bytes = session_meta("sess-utf").into_bytes();
    bytes.push(b'\n');
    bytes.extend_from_slice(b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"abc");
    bytes.push(0xFF);
    bytes.extend_from_slice(b"\"}]}}\n");
    fs::write(&invalid, bytes).unwrap();
    let mut reader = SharedJsonlReader::open(&invalid).unwrap();
    let err = reader.read_from(0).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);

    let oversized = write_rollout(
        &home,
        "sess-big",
        &format!("{}\n{}\n", session_meta("sess-big"), "x".repeat(40)),
    );
    let mut reader = SharedJsonlReader::open_with_limit(&oversized, 16).unwrap();
    let err = reader.read_from(0).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
}

#[test]
fn sanitizer_includes_and_excludes_the_required_classes() {
    let mut reader = SharedJsonlReader::open(&fixture_path()).unwrap();
    let batch = reader.read_from(0).unwrap();
    let messages = visible_messages(&batch.records).unwrap();
    let texts: Vec<_> = messages
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        texts,
        [
            "SYNTH_USER_ALPHA",
            "SYNTH_COMMENTARY_CHARLIE",
            "SYNTH_FINAL_DELTA",
            "line1\nline2",
            "café 😀"
        ]
    );
    assert_eq!(messages[1].phase.as_deref(), Some("commentary"));
    assert_eq!(messages[2].phase.as_deref(), Some("final_answer"));
    let rendered = super::sanitize::render_messages(&messages);
    assert_eq!(
        rendered,
        "[user]\nSYNTH_USER_ALPHA\n\n[assistant commentary]\nSYNTH_COMMENTARY_CHARLIE\n\n[assistant final_answer]\nSYNTH_FINAL_DELTA\n\n[user]\nline1\nline2\n\n[assistant]\ncafé 😀\n"
    );
    assert!(!rendered.contains("SYNTH_HIDDEN_REASONING"));
    assert!(!rendered.contains("SYNTH_TOOL_ECHO"));
    assert!(!rendered.contains("SYNTH_DEV_HIDDEN"));
    assert!(!rendered.contains("<synthetic-permission-block>"));
    assert!(!rendered.contains("# AGENTS.md instructions for"));
}

#[test]
fn json_parser_combines_surrogate_pairs_and_rejects_unpaired_surrogates() {
    let parsed = super::parse::parse_json(r#"{"text":"\uD83D\uDE00"}"#).unwrap();
    assert_eq!(
        parsed.get("text").and_then(crate::json::Json::as_str),
        Some("😀")
    );
    assert!(super::parse::parse_json(r#"{"text":"\uD83D"}"#).is_err());
    assert!(super::parse::parse_json(r#"{"text":"\uDE00"}"#).is_err());
}

#[test]
fn sanitizer_reads_codex_metadata_passthrough_turn_ids() {
    let home = temp_dir("metadata-turn-home");
    let path = write_rollout(
        &home,
        "sess-metadata-turn",
        &jsonl(&[
            session_meta("sess-metadata-turn"),
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","id":"m1","internal_chat_message_metadata_passthrough":{"turn_id":"turn-from-metadata"},"content":[{"type":"output_text","text":"visible"}]}}"#.to_string(),
        ]),
    );
    let mut reader = SharedJsonlReader::open(&path).unwrap();
    let batch = reader.read_from(0).unwrap();
    let messages = visible_messages(&batch.records).unwrap();
    assert_eq!(messages[0].turn_id, "turn-from-metadata");
}

#[test]
fn read_only_seed_planning_creates_no_context_state() {
    let home = temp_dir("readonly-seed-home");
    write_rollout(
        &home,
        "sess-readonly",
        &jsonl(&[
            session_meta("sess-readonly"),
            turn_context("t1"),
            user_msg("u1", "visible"),
        ]),
    );
    let parent = temp_dir("readonly-seed-parent");
    let state = parent.join("state-must-not-exist");
    let store = store_at(&state);
    let source = resolve(&home, "sess-readonly");
    let key = key("grok-readonly", "sess-readonly");
    let plan = store
        .plan_new_session_seed_read_only(&key, &source, &bounds(100, 100, 100))
        .unwrap();
    assert!(plan.text.contains("visible"));
    assert!(!state.exists());
}

#[test]
fn seed_truncates_whole_turn_groups_and_overflows_closed() {
    let home = temp_dir("seed-home");
    write_rollout(
        &home,
        "sess-seed",
        &jsonl(&[
            session_meta("sess-seed"),
            turn_context("t1"),
            user_msg("u1", "AAAA"),
            assistant_msg("a1", "BBBB", None),
            turn_context("t2"),
            user_msg("u2", "CCCC"),
            assistant_msg("a2", "DDDD", None),
        ]),
    );
    let store = store_at(&temp_dir("seed-state"));
    let source = resolve(&home, "sess-seed");
    let key = key("grok-seed-1", "sess-seed");
    let truncated = store
        .plan_new_session_seed(&key, &source, &bounds(8, 100, 100))
        .unwrap();
    assert!(truncated.truncated_front);
    assert_eq!(truncated.kind, PlanKind::Seed);
    assert_eq!(truncated.character_count, 8);
    assert!(truncated.text.contains("CCCC"));
    assert!(!truncated.text.contains("AAAA"));
    let overflow = store
        .plan_new_session_seed(&key, &source, &bounds(3, 100, 100))
        .unwrap_err();
    assert_eq!(overflow.kind, ErrorKind::Overflow);
    let hard = store
        .plan_new_session_seed(&key, &source, &bounds(8, 100, 7))
        .unwrap_err();
    assert_eq!(hard.kind, ErrorKind::Overflow);
}

#[test]
fn delta_is_exact_empty_or_overflow() {
    let home = temp_dir("delta-home");
    let path = write_rollout(
        &home,
        "sess-delta",
        &jsonl(&[
            session_meta("sess-delta"),
            turn_context("t1"),
            user_msg("u1", "AAAA"),
        ]),
    );
    let state = temp_dir("delta-state");
    let store = store_at(&state);
    let source = resolve(&home, "sess-delta");
    let key = key("grok-delta-1", "sess-delta");
    let seed = commit_seed(&store, &key, &source, 100, "ex-seed");
    let empty = store
        .plan_resume_delta(&key, &source, &bounds(100, 100, 100))
        .unwrap();
    assert_eq!(empty.kind, PlanKind::Delta);
    assert!(empty.text.is_empty());
    assert_eq!(empty.start_offset, seed.end_offset);
    assert_eq!(empty.end_offset, seed.end_offset);

    let mut body = fs::read_to_string(&path).unwrap();
    body.push_str(&reasoning());
    body.push('\n');
    body.push_str(&assistant_msg("a1", "ZZZZ", None));
    body.push('\n');
    fs::write(&path, body).unwrap();
    let source = resolve(&home, "sess-delta");
    let delta = store
        .plan_resume_delta(&key, &source, &bounds(100, 100, 100))
        .unwrap();
    assert_eq!(delta.text, "[assistant]\nZZZZ\n");
    assert_eq!(delta.character_count, 4);
    assert!(delta.record_count >= 2);
    assert!(delta.end_offset > seed.end_offset);
    let overflow = store
        .plan_resume_delta(&key, &source, &bounds(100, 3, 100))
        .unwrap_err();
    assert_eq!(overflow.kind, ErrorKind::Overflow);
}

#[test]
fn independent_target_cursors_do_not_share_state() {
    let home = temp_dir("indep-home");
    write_rollout(
        &home,
        "sess-indep",
        &jsonl(&[session_meta("sess-indep"), user_msg("u1", "ONE")]),
    );
    let store = store_at(&temp_dir("indep-state"));
    let source = resolve(&home, "sess-indep");
    let a = key("grok-a", "sess-indep");
    let b = key("grok-b", "sess-indep");
    commit_seed(&store, &a, &source, 100, "ex-a");
    assert_eq!(store.inspect(&a).unwrap().state, CursorState::Committed);
    assert_eq!(store.inspect(&b).unwrap().state, CursorState::Absent);
    let seed_b = store
        .plan_new_session_seed(&b, &source, &bounds(100, 100, 100))
        .unwrap();
    assert!(seed_b.text.contains("ONE"));
}

#[test]
fn state_matrix_and_restart_in_flight_become_uncertain() {
    let home = temp_dir("matrix-home");
    write_rollout(
        &home,
        "sess-matrix",
        &jsonl(&[session_meta("sess-matrix"), user_msg("u1", "ONE")]),
    );
    let state = temp_dir("matrix-state");
    let store = store_at(&state);
    let source = resolve(&home, "sess-matrix");
    let key = key("grok-matrix", "sess-matrix");
    assert_eq!(store.inspect(&key).unwrap().state, CursorState::Absent);
    let resume_absent = store
        .plan_resume_delta(&key, &source, &bounds(100, 100, 100))
        .unwrap_err();
    assert_eq!(resume_absent.kind, ErrorKind::State);

    let plan = store
        .plan_new_session_seed(&key, &source, &bounds(100, 100, 100))
        .unwrap();
    store.append_in_flight(&key, &plan, "ex-1").unwrap();
    assert_eq!(store.inspect(&key).unwrap().state, CursorState::InFlight);
    let new_again = store
        .plan_new_session_seed(&key, &source, &bounds(100, 100, 100))
        .unwrap_err();
    assert_eq!(new_again.kind, ErrorKind::State);
    let replay_live = store.plan_uncertain_replay(&key, &source).unwrap_err();
    assert_eq!(replay_live.kind, ErrorKind::State);

    let restarted = store_at(&state);
    assert_eq!(
        restarted.inspect(&key).unwrap().state,
        CursorState::Uncertain
    );
    let resume_uncertain = restarted
        .plan_resume_delta(&key, &source, &bounds(100, 100, 100))
        .unwrap_err();
    assert_eq!(resume_uncertain.kind, ErrorKind::State);
    restarted.abort_to_committed(&key, "ex-abort").unwrap_err();

    store.commit(&key, &plan, "ex-1").unwrap();
    assert_eq!(store.inspect(&key).unwrap().state, CursorState::Committed);
    let recovery_on_committed = store.plan_uncertain_replay(&key, &source).unwrap_err();
    assert_eq!(recovery_on_committed.kind, ErrorKind::State);
    let skip_on_committed = store
        .plan_skip_then_delta(&key, &source, &bounds(100, 100, 100), "ex-skip")
        .unwrap_err();
    assert_eq!(skip_on_committed.kind, ErrorKind::State);
}

#[test]
fn replay_uses_exact_uncertain_range_and_skip_then_delta_advances() {
    let home = temp_dir("replay-home");
    let path = write_rollout(
        &home,
        "sess-replay",
        &jsonl(&[session_meta("sess-replay"), user_msg("u1", "ONE")]),
    );
    let state = temp_dir("replay-state");
    let store = store_at(&state);
    let source = resolve(&home, "sess-replay");
    let key = key("grok-replay", "sess-replay");
    let seed = store
        .plan_new_session_seed(&key, &source, &bounds(100, 100, 100))
        .unwrap();
    store.append_in_flight(&key, &seed, "ex-seed").unwrap();

    let mut body = fs::read_to_string(&path).unwrap();
    body.push_str(&assistant_msg("a1", "TWO", None));
    body.push('\n');
    fs::write(&path, &body).unwrap();
    let source = resolve(&home, "sess-replay");
    let restarted = store_at(&state);
    let replay = restarted.plan_uncertain_replay(&key, &source).unwrap();
    assert_eq!(replay.kind, PlanKind::Replay);
    assert!(replay.text.contains("ONE"));
    assert!(!replay.text.contains("TWO"));
    assert_eq!(replay.end_offset, seed.end_offset);

    let skip_delta = restarted
        .plan_skip_then_delta(&key, &source, &bounds(100, 100, 100), "ex-skip")
        .unwrap();
    assert_eq!(skip_delta.kind, PlanKind::Delta);
    assert!(skip_delta.text.contains("TWO"));
    assert!(!skip_delta.text.contains("ONE"));
    restarted
        .append_in_flight(&key, &skip_delta, "ex-delta")
        .unwrap();
    restarted.commit(&key, &skip_delta, "ex-delta").unwrap();
}

#[test]
fn replacement_truncation_and_content_mismatch_fail_closed() {
    let home = temp_dir("mismatch-home");
    let path = write_rollout(
        &home,
        "sess-mis",
        &jsonl(&[session_meta("sess-mis"), user_msg("u1", "ONE")]),
    );
    let store = store_at(&temp_dir("mismatch-state"));
    let source = resolve(&home, "sess-mis");
    let mismatch_key = key("grok-mis", "sess-mis");
    commit_seed(&store, &mismatch_key, &source, 100, "ex-1");

    let original = fs::read_to_string(&path).unwrap();
    fs::write(&path, original.replace("ONE", "TWO")).unwrap();
    let source = resolve(&home, "sess-mis");
    let mismatch = store
        .plan_resume_delta(&mismatch_key, &source, &bounds(100, 100, 100))
        .unwrap_err();
    assert_eq!(mismatch.kind, ErrorKind::Mismatch);

    fs::write(&path, format!("{}\n", session_meta("sess-mis"))).unwrap();
    let source = resolve(&home, "sess-mis");
    let truncated = store
        .plan_resume_delta(&mismatch_key, &source, &bounds(100, 100, 100))
        .unwrap_err();
    assert_eq!(truncated.kind, ErrorKind::Truncated);

    let replaced_home = temp_dir("replaced-home");
    let replaced_path = write_rollout(
        &replaced_home,
        "sess-rep",
        &jsonl(&[session_meta("sess-rep"), user_msg("u1", "ONE")]),
    );
    let store = store_at(&temp_dir("replaced-state"));
    let source = resolve(&replaced_home, "sess-rep");
    let before = source.identity.clone();
    let key = key("grok-rep", "sess-rep");
    commit_seed(&store, &key, &source, 100, "ex-1");
    let contents = fs::read(&replaced_path).unwrap();
    fs::remove_file(&replaced_path).unwrap();
    fs::write(&replaced_path, contents).unwrap();
    let source = resolve(&replaced_home, "sess-rep");
    assert!(!before.same_file(&source.identity));
    let replaced = store
        .plan_resume_delta(&key, &source, &bounds(100, 100, 100))
        .unwrap_err();
    assert_eq!(replaced.kind, ErrorKind::Replaced);
}

#[test]
fn corrupt_and_partial_journals_fail_closed_or_ignore_incomplete() {
    let home = temp_dir("journal-home");
    write_rollout(
        &home,
        "sess-j",
        &jsonl(&[session_meta("sess-j"), user_msg("u1", "ONE")]),
    );
    let state = temp_dir("journal-state");
    let store = store_at(&state);
    let source = resolve(&home, "sess-j");
    let key = key("grok-j", "sess-j");
    commit_seed(&store, &key, &source, 100, "ex-1");
    let journal = state
        .join("grok-j")
        .join("codex")
        .join("sess-j")
        .join("journal.jsonl");
    let mut body = fs::read(&journal).unwrap();
    body.extend_from_slice(b"{\"schema_version\":1,\"record_id\":\"partial\"");
    fs::write(&journal, &body).unwrap();
    assert_eq!(store.inspect(&key).unwrap().state, CursorState::Committed);

    let mut corrupt = fs::read_to_string(&journal).unwrap();
    corrupt.push_str("\n{not-json}\n");
    fs::write(&journal, corrupt).unwrap();
    let err = store.inspect(&key).unwrap_err();
    assert_eq!(err.kind, ErrorKind::CorruptJournal);
}

#[cfg(windows)]
#[test]
fn lock_contention_is_exclusive() {
    let home = temp_dir("lock-home");
    write_rollout(
        &home,
        "sess-lock",
        &jsonl(&[session_meta("sess-lock"), user_msg("u1", "ONE")]),
    );
    let state = temp_dir("lock-state");
    let store = store_at(&state);
    let key = key("grok-lock", "sess-lock");
    let _held = store.lock_key(&key).unwrap();
    let other = store_at(&state);
    let err = other.inspect(&key).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Locked);
}

#[test]
fn persisted_journal_contains_no_transcript_strings() {
    let home = temp_dir("meta-home");
    let fixture = fs::read_to_string(fixture_path()).unwrap();
    write_rollout(&home, "fixture-session-aaa", &fixture);
    let state = temp_dir("meta-state");
    let store = store_at(&state);
    let source = resolve(&home, "fixture-session-aaa");
    let key = key("grok-meta", "fixture-session-aaa");
    let plan = store
        .plan_new_session_seed(&key, &source, &bounds(10_000, 10_000, 10_000))
        .unwrap();
    assert!(plan.text.contains("SYNTH_USER_ALPHA"));
    store.append_in_flight(&key, &plan, "ex-meta").unwrap();
    store.commit(&key, &plan, "ex-meta").unwrap();
    let journal = fs::read_to_string(
        state
            .join("grok-meta")
            .join("codex")
            .join("fixture-session-aaa")
            .join("journal.jsonl"),
    )
    .unwrap();
    for needle in SYNTH_STRINGS {
        assert!(
            !journal.contains(needle),
            "journal leaked transcript string {needle}"
        );
    }
    assert!(!journal.contains("[user]"));
    assert!(!journal.contains("line1\\nline2") && !journal.contains("line1"));
    assert!(!journal.contains(&home.to_string_lossy().to_string()));
    assert!(journal.contains("path_fingerprint"));
    assert!(!journal.contains("canonical_path"));
}
