use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::error::ErrorKind;
use super::journal::{ContextKey, CursorState, JournalStore};
use super::plan::{fingerprint_records, ContextBounds, PlanKind};
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

fn append_oversized_compacted(path: &Path, payload_bytes: usize) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(br#"{"type":"compacted","payload":{"message":""#)
        .unwrap();
    let chunk = [b'x'; 64 * 1024];
    let mut remaining = payload_bytes;
    while remaining > 0 {
        let write = remaining.min(chunk.len());
        file.write_all(&chunk[..write]).unwrap();
        remaining -= write;
    }
    file.write_all(b"\"}}\n").unwrap();
    file.flush().unwrap();
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

fn user_msg_turn(id: &str, turn: &str, text: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-01-01T00:00:02.000Z","type":"response_item","payload":{{"type":"message","role":"user","id":"{id}","turn_id":"{turn}","content":[{{"type":"input_text","text":"{}"}}]}}}}"#,
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
    assert_eq!(batch.record_count, 3);
    assert_eq!(batch.fingerprint, fingerprint_records(&batch.records));
    assert_eq!(batch.records[0].start_offset, 3);
    assert!(!batch.incomplete_tail.is_empty());
    assert_eq!(
        batch.incomplete_tail_len,
        batch.incomplete_tail.len() as u64
    );
    assert!(!batch.incomplete_tail_oversized);
    let messages = visible_messages(&batch.records).unwrap();
    assert_eq!(messages[0].text, "café 😀");
    assert_eq!(messages[0].text.chars().count(), 6);
}

#[test]
fn reader_skips_only_closed_invalid_utf8_tool_output_records() {
    let home = temp_dir("invalid-utf8-excluded-home");
    let path = write_rollout(
        &home,
        "sess-invalid-utf8-excluded",
        &jsonl(&[session_meta("sess-invalid-utf8-excluded")]),
    );
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(br#"{"timestamp":"x","type":"response_item","payload":{"type":"custom_tool_call_output","id":"one","output":"before "#)
        .unwrap();
    file.write_all(&[0xf8]).unwrap();
    file.write_all(b" after\"}}\n").unwrap();
    file.write_all(user_msg("u1", "visible after excluded bytes").as_bytes())
        .unwrap();
    file.write_all(b"\n").unwrap();
    file.flush().unwrap();

    let mut reader = SharedJsonlReader::open(&path).unwrap();
    let batch = reader.read_from(0).unwrap();
    assert_eq!(batch.record_count, 3);
    assert_eq!(batch.records.len(), 2);
    assert_eq!(batch.skipped_invalid_utf8, 1);
    let messages = visible_messages(&batch.records).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "visible after excluded bytes");

    let visible = write_rollout(&home, "sess-invalid-utf8-visible", "");
    let mut bytes = br#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":""#.to_vec();
    bytes.push(0xf8);
    bytes.extend_from_slice(b"\"}]}}\n");
    fs::write(&visible, bytes).unwrap();
    let mut reader = SharedJsonlReader::open(&visible).unwrap();
    assert_eq!(reader.read_from(0).unwrap_err().kind, ErrorKind::Malformed);
}

#[test]
fn reader_skips_only_valid_oversized_excluded_records() {
    let home = temp_dir("oversized-reader-home");
    let path = write_rollout(
        &home,
        "sess-oversized-reader",
        &jsonl(&[session_meta("sess-oversized-reader")]),
    );
    append_oversized_compacted(&path, 256);
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(user_msg("u1", "visible").as_bytes())
        .unwrap();
    file.write_all(b"\n").unwrap();
    file.flush().unwrap();

    let mut reader = SharedJsonlReader::open_with_limit(&path, 200).unwrap();
    let batch = reader.read_from(0).unwrap();
    assert_eq!(batch.records.len(), 2);
    assert_eq!(batch.record_count, 3);
    assert_eq!(batch.skipped_oversized, 1);
    assert_ne!(batch.fingerprint, fingerprint_records(&batch.records));
    let exact = reader.read_until(0, batch.complete_end).unwrap();
    assert_eq!(exact.fingerprint, batch.fingerprint);
    assert_eq!(exact.record_count, batch.record_count);
}

#[test]
fn reader_defers_an_oversized_incomplete_tail_until_newline() {
    let home = temp_dir("oversized-tail-home");
    let path = write_rollout(
        &home,
        "sess-oversized-tail",
        &jsonl(&[session_meta("sess-oversized-tail")]),
    );
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(br#"{"type":"compacted","payload":{"message":""#)
        .unwrap();
    file.write_all(&[b'x'; 256]).unwrap();
    file.flush().unwrap();

    let mut reader = SharedJsonlReader::open_with_limit(&path, 64).unwrap();
    let first = reader.read_from(0).unwrap();
    assert!(first.incomplete_tail.is_empty());
    assert!(first.incomplete_tail_oversized);
    assert!(first.incomplete_tail_len > 64);
    assert_eq!(first.record_count, 1);

    file.write_all(b"\"}}\n").unwrap();
    file.flush().unwrap();
    let completed = reader.read_from(first.complete_end).unwrap();
    assert_eq!(completed.record_count, 1);
    assert_eq!(completed.skipped_oversized, 1);
    assert!(!completed.incomplete_tail_oversized);
}

#[test]
fn oversized_eligible_unknown_and_ambiguous_records_fail_closed() {
    let home = temp_dir("oversized-deny-home");
    let cases = [
        r#"{"type":"response_item","payload":{"type":"message"}}"#,
        r#"{"type":"future_record","payload":{}}"#,
        r#"{"payload":{"type":"compacted"}}"#,
        r#"{"type":"compacted","type":"compacted"}"#,
    ];
    for (index, line) in cases.iter().enumerate() {
        let path = write_rollout(
            &home,
            &format!("sess-oversized-deny-{index}"),
            &format!("{line}\n"),
        );
        let mut reader = SharedJsonlReader::open_with_limit(&path, 16).unwrap();
        assert_eq!(reader.read_from(0).unwrap_err().kind, ErrorKind::Malformed);
    }
}

#[test]
fn delta_cursor_round_trips_across_a_real_oversized_compacted_record() {
    let home = temp_dir("oversized-delta-home");
    let path = write_rollout(
        &home,
        "sess-oversized-delta",
        &jsonl(&[
            session_meta("sess-oversized-delta"),
            turn_context("turn-1"),
            user_msg("u1", "ONE"),
        ]),
    );
    let state = temp_dir("oversized-delta-state");
    let store = store_at(&state);
    let source = resolve(&home, "sess-oversized-delta");
    let context_key = key("grok-oversized-delta", "sess-oversized-delta");
    commit_seed(&store, &context_key, &source, 100, "ex-seed");

    append_oversized_compacted(&path, MAX_PHYSICAL_RECORD);
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(turn_context("turn-2").as_bytes()).unwrap();
    file.write_all(b"\n").unwrap();
    file.write_all(user_msg("u2", "TWO").as_bytes()).unwrap();
    file.write_all(b"\n").unwrap();
    file.flush().unwrap();

    let source = resolve(&home, "sess-oversized-delta");
    let delta = store
        .plan_resume_delta(&context_key, &source, &bounds(100, 100, 100))
        .unwrap();
    assert!(delta.text.contains("TWO"));
    assert_eq!(delta.record_count, 3);
    store
        .append_in_flight(&context_key, &delta, "ex-delta")
        .unwrap();
    store.commit(&context_key, &delta, "ex-delta").unwrap();

    let empty = store
        .plan_resume_delta(&context_key, &source, &bounds(100, 100, 100))
        .unwrap();
    assert!(empty.text.is_empty());
    assert_eq!(empty.record_count, 0);
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
    let parsed = super::parse::parse_json(r#"{"text":"\uD83D\uDE00\b\f"}"#).unwrap();
    assert_eq!(
        parsed.get("text").and_then(crate::json::Json::as_str),
        Some("😀\u{0008}\u{000c}")
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
fn sanitizer_bounds_message_and_turn_identifiers() {
    let home = temp_dir("bounded-identifiers-home");
    let long_id = "x".repeat(257);

    let message_path = write_rollout(
        &home,
        "sess-long-message-id",
        &jsonl(&[
            session_meta("sess-long-message-id"),
            user_msg(&long_id, "visible"),
        ]),
    );
    let mut reader = SharedJsonlReader::open(&message_path).unwrap();
    let batch = reader.read_from(0).unwrap();
    assert_eq!(
        visible_messages(&batch.records).unwrap_err().kind,
        ErrorKind::Overflow
    );

    let turn_path = write_rollout(
        &home,
        "sess-long-turn-id",
        &jsonl(&[
            session_meta("sess-long-turn-id"),
            turn_context(&long_id),
            user_msg("u1", "visible"),
        ]),
    );
    let mut reader = SharedJsonlReader::open(&turn_path).unwrap();
    let batch = reader.read_from(0).unwrap();
    assert_eq!(
        visible_messages(&batch.records).unwrap_err().kind,
        ErrorKind::Overflow
    );
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
    assert_eq!(replay.fingerprint, seed.fingerprint);
    assert_eq!(replay.record_count, seed.record_count);
    assert_eq!(replay.character_count, seed.character_count);
    assert_eq!(replay.first_message, seed.first_message);
    assert_eq!(replay.last_message, seed.last_message);

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

fn assert_catch_up_delta(plan: &super::plan::ContextPlan, source: &super::root::ResolvedSource) {
    assert_eq!(plan.kind, PlanKind::Delta);
    assert!(!plan.truncated_front);
    assert!(plan.end_offset > plan.start_offset);
    assert!(plan.character_count > 0);
    assert_eq!(plan.source_session, source.session_id);
    assert_eq!(plan.source_identity, source.identity);
}

fn push_crlf(body: &mut Vec<u8>, line: &str) {
    body.extend_from_slice(line.as_bytes());
    body.extend_from_slice(b"\r\n");
}

fn open_source(home: &Path, session: &str) -> (super::root::ResolvedSource, PathBuf) {
    let source = resolve(home, session);
    let path = source.path.clone();
    (source, path)
}

fn catch_up(
    source: &super::root::ResolvedSource,
    path: &Path,
    start: u64,
    delta: usize,
    hard: usize,
) -> Result<super::plan::ContextPlan, super::error::ContextError> {
    let mut reader = SharedJsonlReader::open(path).unwrap();
    super::plan::plan_catch_up(source, &mut reader, start, &bounds(10_000, delta, hard))
}

fn read_messages(path: &Path) -> (u64, Vec<super::sanitize::VisibleMessage>) {
    let mut reader = SharedJsonlReader::open(path).unwrap();
    let batch = reader.read_from(0).unwrap();
    let messages = visible_messages(&batch.records).unwrap();
    (batch.complete_end, messages)
}

#[test]
fn ordinary_delta_overflow_is_unchanged_and_catch_up_takes_the_next_prefix() {
    let home = temp_dir("catch-prefix-home");
    let session = "sess-catch-prefix";
    let path = write_rollout(
        &home,
        session,
        &jsonl(&[
            session_meta(session),
            turn_context("t1"),
            user_msg("u1", "AAAA"),
            turn_context("t2"),
            user_msg("u2", "BBBB"),
            turn_context("t3"),
            user_msg("u3", "CCCC"),
            turn_context("t4"),
            user_msg("u4", "DDDD"),
        ]),
    );
    let (source, _) = open_source(&home, session);
    let mut reader = SharedJsonlReader::open(&path).unwrap();
    let overflow =
        super::plan::plan_delta(&source, &mut reader, 0, &bounds(100, 8, 100)).unwrap_err();
    assert_eq!(overflow.kind, ErrorKind::Overflow);
    assert_eq!(overflow.message, "delta character count exceeds limit");

    let prefix = catch_up(&source, &path, 0, 8, 100).unwrap();
    assert_catch_up_delta(&prefix, &source);
    assert_eq!(prefix.text, "[user]\nAAAA\n\n[user]\nBBBB\n");
    assert_eq!(prefix.character_count, 8);
    assert_eq!(prefix.record_count, 5);
    assert_eq!(prefix.first_turn_id.as_deref(), Some("t1"));
    assert_eq!(prefix.last_turn_id.as_deref(), Some("t2"));
    assert_eq!(prefix.first_message.as_ref().unwrap().message_id, "u1");
    assert_eq!(prefix.last_message.as_ref().unwrap().message_id, "u2");
    let (_, messages) = read_messages(&path);
    assert_eq!(prefix.end_offset, messages[2].turn_start_offset);
    assert!(prefix.end_offset < messages[2].start_offset);
    assert!(!prefix.text.contains("CCCC"));
    assert!(!prefix.text.contains("DDDD"));

    let mut reader = SharedJsonlReader::open(&path).unwrap();
    let next = super::plan::plan_delta(
        &source,
        &mut reader,
        prefix.end_offset,
        &bounds(100, 8, 100),
    )
    .unwrap();
    assert_eq!(next.kind, PlanKind::Delta);
    assert!(!next.truncated_front);
    assert_eq!(next.start_offset, prefix.end_offset);
    assert_eq!(next.text, "[user]\nCCCC\n\n[user]\nDDDD\n");
    assert_eq!(next.character_count, 8);
    assert_eq!(next.first_turn_id.as_deref(), Some("t3"));
    assert_eq!(next.last_turn_id.as_deref(), Some("t4"));

    let refused = catch_up(&source, &path, prefix.end_offset, 8, 100).unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Unsupported);
    assert_eq!(refused.message, "unread tail fits in an ordinary delta");
}

#[test]
fn catch_up_rolls_back_the_entire_overflowing_multi_message_turn() {
    let home = temp_dir("catch-rollback-home");
    let session = "sess-catch-rollback";
    let path = write_rollout(
        &home,
        session,
        &jsonl(&[
            session_meta(session),
            turn_context("t1"),
            user_msg("u1", "AAAA"),
            turn_context("t2"),
            user_msg("u2", "BB"),
            assistant_msg("a2", "CCCC", None),
            turn_context("t3"),
            user_msg("u3", "DDDD"),
        ]),
    );
    let (source, _) = open_source(&home, session);
    let plan = catch_up(&source, &path, 0, 8, 100).unwrap();
    assert_catch_up_delta(&plan, &source);
    assert_eq!(plan.text, "[user]\nAAAA\n");
    assert_eq!(plan.character_count, 4);
    assert_eq!(plan.record_count, 3);
    assert_eq!(plan.first_turn_id.as_deref(), Some("t1"));
    assert_eq!(plan.last_turn_id.as_deref(), Some("t1"));
    let (_, messages) = read_messages(&path);
    assert_eq!(messages[1].turn_id, messages[2].turn_id);
    assert_eq!(messages[1].turn_start_offset, messages[2].turn_start_offset);
    assert_eq!(plan.end_offset, messages[1].turn_start_offset);
    assert!(plan.end_offset < messages[1].start_offset);
    assert!(plan.end_offset < messages[2].start_offset);
    assert!(!plan.text.contains("BB"));
    assert!(!plan.text.contains("CCCC"));
    assert!(!plan.text.contains("DDDD"));
}

#[test]
fn catch_up_limits_unicode_by_scalar_count() {
    let home = temp_dir("catch-unicode-home");
    let session = "sess-catch-unicode";
    let text = "café 😀";
    assert_eq!(text.chars().count(), 6);
    assert!(text.len() > text.chars().count());
    let path = write_rollout(
        &home,
        session,
        &jsonl(&[
            session_meta(session),
            turn_context("t1"),
            user_msg("u1", text),
            turn_context("t2"),
            user_msg("u2", "ZZZZZZZ"),
        ]),
    );
    let (source, _) = open_source(&home, session);
    let plan = catch_up(&source, &path, 0, 6, 100).unwrap();
    assert_catch_up_delta(&plan, &source);
    assert_eq!(plan.character_count, 6);
    assert!(plan.text.contains(text));
    assert!(!plan.text.contains('Z'));
    assert!(plan.text.len() > plan.character_count as usize);
    let (_, messages) = read_messages(&path);
    assert_eq!(plan.end_offset, messages[1].turn_start_offset);

    let overflow = catch_up(&source, &path, 0, 5, 100).unwrap_err();
    assert_eq!(overflow.kind, ErrorKind::Overflow);
    assert_eq!(
        overflow.message,
        "first unread turn exceeds delta_max_chars"
    );
}

#[test]
fn catch_up_stops_at_the_hard_cap_without_raising_it() {
    let home = temp_dir("catch-hard-home");
    let session = "sess-catch-hard";
    let path = write_rollout(
        &home,
        session,
        &jsonl(&[
            session_meta(session),
            turn_context("t1"),
            user_msg("u1", "AAAA"),
            turn_context("t2"),
            user_msg("u2", "BBBB"),
            turn_context("t3"),
            user_msg("u3", "CCCC"),
        ]),
    );
    let (source, _) = open_source(&home, session);
    let plan = catch_up(&source, &path, 0, 100, 8).unwrap();
    assert_catch_up_delta(&plan, &source);
    assert_eq!(plan.character_count, 8);
    assert_eq!(plan.text, "[user]\nAAAA\n\n[user]\nBBBB\n");
    assert!(!plan.text.contains("CCCC"));
    let mut reader = SharedJsonlReader::open(&path).unwrap();
    let ordinary =
        super::plan::plan_delta(&source, &mut reader, 0, &bounds(100, 100, 8)).unwrap_err();
    assert_eq!(ordinary.kind, ErrorKind::Overflow);

    let fits = catch_up(&source, &path, 0, 100, 100).unwrap_err();
    assert_eq!(fits.kind, ErrorKind::Unsupported);
    assert_eq!(fits.message, "unread tail fits in an ordinary delta");
}

#[test]
fn catch_up_refuses_a_fitting_tail_and_absent_history() {
    let home = temp_dir("catch-fit-home");
    let session = "sess-catch-fit";
    let path = write_rollout(
        &home,
        session,
        &jsonl(&[
            session_meta(session),
            turn_context("t1"),
            user_msg("u1", "AAAA"),
            turn_context("t2"),
            user_msg("u2", "BBBB"),
        ]),
    );
    let (source, _) = open_source(&home, session);
    let fits = catch_up(&source, &path, 0, 100, 100).unwrap_err();
    assert_eq!(fits.kind, ErrorKind::Unsupported);
    assert_eq!(fits.message, "unread tail fits in an ordinary delta");
    let mut reader = SharedJsonlReader::open(&path).unwrap();
    let delta = super::plan::plan_delta(&source, &mut reader, 0, &bounds(100, 100, 100)).unwrap();
    assert_eq!(delta.text, "[user]\nAAAA\n\n[user]\nBBBB\n");
    assert_eq!(delta.character_count, 8);

    let absent = catch_up(&source, &path, delta.end_offset, 100, 100).unwrap_err();
    assert_eq!(absent.kind, ErrorKind::Unsupported);
    assert_eq!(absent.message, "catch-up requires unread history");

    let quiet = temp_dir("catch-quiet-home");
    let quiet_session = "sess-catch-quiet";
    let quiet_path = write_rollout(
        &quiet,
        quiet_session,
        &jsonl(&[session_meta(quiet_session), reasoning()]),
    );
    let (quiet_source, _) = open_source(&quiet, quiet_session);
    let quiet_catch = catch_up(&quiet_source, &quiet_path, 0, 100, 100).unwrap_err();
    assert_eq!(quiet_catch.kind, ErrorKind::Unsupported);
    assert_eq!(quiet_catch.message, "unread tail fits in an ordinary delta");
    let mut reader = SharedJsonlReader::open(&quiet_path).unwrap();
    let quiet_delta =
        super::plan::plan_delta(&quiet_source, &mut reader, 0, &bounds(100, 100, 100)).unwrap();
    assert!(quiet_delta.text.is_empty());
    assert!(quiet_delta.end_offset > quiet_delta.start_offset);
    assert!(quiet_delta.record_count >= 2);
}

#[test]
fn catch_up_rejects_a_first_turn_that_cannot_fit() {
    let home = temp_dir("catch-first-home");
    let session = "sess-catch-first";
    let path = write_rollout(
        &home,
        session,
        &jsonl(&[
            session_meta(session),
            turn_context("t1"),
            user_msg("u1", "HELLO"),
            turn_context("t2"),
            user_msg("u2", "Z"),
        ]),
    );
    let (source, _) = open_source(&home, session);
    let delta_cap = catch_up(&source, &path, 0, 4, 100).unwrap_err();
    assert_eq!(delta_cap.kind, ErrorKind::Overflow);
    assert_eq!(
        delta_cap.message,
        "first unread turn exceeds delta_max_chars"
    );
    let hard_cap = catch_up(&source, &path, 0, 100, 4).unwrap_err();
    assert_eq!(hard_cap.kind, ErrorKind::Overflow);
    assert_eq!(hard_cap.message, "first unread turn exceeds hard_max_chars");

    let split = temp_dir("catch-first-split-home");
    let split_session = "sess-catch-first-split";
    let split_path = write_rollout(
        &split,
        split_session,
        &jsonl(&[
            session_meta(split_session),
            turn_context("t1"),
            user_msg("u1", "AB"),
            assistant_msg("a1", "CDE", None),
            turn_context("t2"),
            user_msg("u2", "Z"),
        ]),
    );
    let (split_source, _) = open_source(&split, split_session);
    let partial = catch_up(&split_source, &split_path, 0, 4, 100).unwrap_err();
    assert_eq!(partial.kind, ErrorKind::Overflow);
    assert_eq!(partial.message, "first unread turn exceeds delta_max_chars");
}

#[test]
fn catch_up_errors_on_malformed_or_oversized_records_inside_the_selected_range() {
    let home = temp_dir("catch-bad-home");
    let malformed_session = "sess-catch-malformed";
    let malformed_path = write_rollout(
        &home,
        malformed_session,
        &jsonl(&[
            session_meta(malformed_session),
            turn_context("t1"),
            user_msg("u1", "AAAA"),
            "{not-json}".to_string(),
            turn_context("t2"),
            user_msg("u2", "BBBBBBBB"),
        ]),
    );
    let (malformed_source, _) = open_source(&home, malformed_session);
    let malformed = catch_up(&malformed_source, &malformed_path, 0, 4, 100).unwrap_err();
    assert_eq!(malformed.kind, ErrorKind::Malformed);

    let oversized_session = "sess-catch-oversized";
    let oversized_path = write_rollout(
        &home,
        oversized_session,
        &jsonl(&[
            session_meta(oversized_session),
            turn_context("t1"),
            user_msg("u1", "AAAA"),
        ]),
    );
    let mut file = OpenOptions::new()
        .append(true)
        .open(&oversized_path)
        .unwrap();
    let mut huge = br#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":""#.to_vec();
    huge.extend(std::iter::repeat(b'x').take(700));
    huge.extend(br#""}]}}"#);
    huge.push(b'\n');
    file.write_all(&huge).unwrap();
    file.write_all(turn_context("t2").as_bytes()).unwrap();
    file.write_all(b"\n").unwrap();
    file.write_all(user_msg("u2", "BBBBBBBB").as_bytes())
        .unwrap();
    file.write_all(b"\n").unwrap();
    file.flush().unwrap();
    let (oversized_source, _) = open_source(&home, oversized_session);
    let mut reader = SharedJsonlReader::open_with_limit(&oversized_path, 512).unwrap();
    let oversized =
        super::plan::plan_catch_up(&oversized_source, &mut reader, 0, &bounds(10_000, 4, 100))
            .unwrap_err();
    assert_eq!(oversized.kind, ErrorKind::Malformed);
    assert!(oversized.message.contains("oversized"));
    assert!(!oversized.message.contains(&"x".repeat(32)));
}

#[test]
fn catch_up_preserves_excluded_records_bom_crlf_and_partial_tails() {
    let home = temp_dir("catch-boundary-home");
    let session = "sess-catch-boundary";
    let path = write_rollout(&home, session, "");
    let mut body = vec![0xEF, 0xBB, 0xBF];
    push_crlf(&mut body, &session_meta(session));
    push_crlf(&mut body, &turn_context("t1"));
    push_crlf(&mut body, &user_msg("u1", "AAAA"));
    body.extend_from_slice(
        br#"{"timestamp":"x","type":"response_item","payload":{"type":"custom_tool_call_output","id":"one","output":"before "#,
    );
    body.push(0xf8);
    body.extend_from_slice(b" after\"}}\r\n");
    let mut compacted = br#"{"type":"compacted","payload":{"message":""#.to_vec();
    compacted.extend(std::iter::repeat(b'x').take(700));
    compacted.extend(br#""}}"#);
    body.extend_from_slice(&compacted);
    body.extend_from_slice(b"\r\n");
    push_crlf(&mut body, &reasoning());
    push_crlf(&mut body, &turn_context("t2"));
    push_crlf(&mut body, &user_msg("u2", "BBBB"));
    push_crlf(&mut body, &turn_context("t3"));
    push_crlf(&mut body, &user_msg("u3", "CCCCCCCC"));
    body.extend_from_slice(br#"{"type":"response_item","payload":{"type":"message""#);
    fs::write(&path, &body).unwrap();

    let (source, _) = open_source(&home, session);
    let mut reader = SharedJsonlReader::open_with_limit(&path, 512).unwrap();
    let plan =
        super::plan::plan_catch_up(&source, &mut reader, 0, &bounds(10_000, 8, 100)).unwrap();
    assert_catch_up_delta(&plan, &source);
    assert_eq!(plan.start_offset, 0);
    assert_eq!(plan.character_count, 8);
    assert_eq!(plan.record_count, 8);
    assert_eq!(plan.text, "[user]\nAAAA\n\n[user]\nBBBB\n");
    assert!(!plan.text.contains("CCCCCCCC"));
    assert!(!plan.text.contains("SYNTH_HIDDEN_REASONING"));
    assert!(!plan.text.contains("before"));
    assert!(plan.end_offset < body.len() as u64);
    let exact = reader.read_until(0, plan.end_offset).unwrap();
    assert_eq!(exact.fingerprint, plan.fingerprint);
    assert_eq!(exact.record_count, plan.record_count);
    assert_eq!(exact.skipped_oversized, 1);
    assert_eq!(exact.skipped_invalid_utf8, 1);
    let tail = reader.read_from(plan.end_offset).unwrap();
    assert!(tail.incomplete_tail_len > 0);
    assert!(tail
        .records
        .iter()
        .any(|record| record.text.contains("CCCCCCCC")));
    assert_ne!(tail.fingerprint, plan.fingerprint);
}

#[test]
fn catch_up_prefix_fingerprint_ignores_a_later_tail_edit() {
    let home = temp_dir("catch-fingerprint-home");
    let session = "sess-catch-fingerprint";
    let path = write_rollout(
        &home,
        session,
        &jsonl(&[
            session_meta(session),
            turn_context("t1"),
            user_msg("u1", "AAAA"),
            turn_context("t2"),
            user_msg("u2", "BBBB"),
            turn_context("t3"),
            user_msg("u3", "CCCCCCCC"),
        ]),
    );
    let (source, _) = open_source(&home, session);
    let prefix = catch_up(&source, &path, 0, 8, 100).unwrap();
    let mut reader = SharedJsonlReader::open(&path).unwrap();
    let before = reader.read_from(0).unwrap();
    assert_eq!(
        reader.read_until(0, prefix.end_offset).unwrap().fingerprint,
        prefix.fingerprint
    );
    assert_ne!(before.fingerprint, prefix.fingerprint);

    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(b"{not-json-tail}\n").unwrap();
    file.write_all(user_msg("u4", "EDITED-TAIL").as_bytes())
        .unwrap();
    file.write_all(b"\n").unwrap();
    file.flush().unwrap();

    let (source, _) = open_source(&home, session);
    let again = catch_up(&source, &path, 0, 8, 100).unwrap();
    assert_eq!(again.fingerprint, prefix.fingerprint);
    assert_eq!(again.end_offset, prefix.end_offset);
    assert_eq!(again.text, prefix.text);
    assert_eq!(again.record_count, prefix.record_count);
    let mut reader = SharedJsonlReader::open(&path).unwrap();
    let after = reader.read_from(0).unwrap();
    assert_ne!(after.fingerprint, before.fingerprint);
    assert_ne!(after.fingerprint, again.fingerprint);
    assert_eq!(
        reader.read_until(0, again.end_offset).unwrap().fingerprint,
        again.fingerprint
    );
}

#[test]
fn catch_up_accepts_aligned_mid_turn_cursors_and_rejects_pathological_cuts() {
    let home = temp_dir("catch-cursor-home");
    let session = "sess-catch-cursor";
    let path = write_rollout(
        &home,
        session,
        &jsonl(&[
            session_meta(session),
            user_msg("u1", "AAAA"),
            assistant_msg("a1", "BBBB", None),
            assistant_msg("a2", "CCCC", None),
            user_msg("u2", "DDDDDDDD"),
        ]),
    );
    let (source, _) = open_source(&home, session);
    let (_, messages) = read_messages(&path);
    assert_eq!(messages[0].turn_id, messages[1].turn_id);
    assert_eq!(messages[1].turn_id, messages[2].turn_id);
    assert_ne!(messages[2].turn_id, messages[3].turn_id);
    assert_eq!(messages[1].start_offset, messages[0].end_offset);

    let whole = catch_up(&source, &path, 0, 8, 100).unwrap_err();
    assert_eq!(whole.kind, ErrorKind::Overflow);
    assert_eq!(whole.message, "first unread turn exceeds delta_max_chars");

    let suffix = catch_up(&source, &path, messages[1].start_offset, 8, 100).unwrap();
    assert_catch_up_delta(&suffix, &source);
    assert_eq!(suffix.start_offset, messages[1].start_offset);
    assert_eq!(suffix.text, "[assistant]\nBBBB\n\n[assistant]\nCCCC\n");
    assert_eq!(suffix.character_count, 8);
    assert_eq!(suffix.end_offset, messages[3].start_offset);
    assert!(!suffix.text.contains("AAAA"));
    assert!(!suffix.text.contains("DDDDDDDD"));
    assert_eq!(suffix.first_message.as_ref().unwrap().message_id, "a1");
    assert_eq!(suffix.last_message.as_ref().unwrap().message_id, "a2");

    let split_remainder = catch_up(&source, &path, messages[1].start_offset, 6, 100).unwrap_err();
    assert_eq!(split_remainder.kind, ErrorKind::Overflow);
    assert_eq!(
        split_remainder.message,
        "first unread turn exceeds delta_max_chars"
    );

    let mid_record = catch_up(&source, &path, messages[1].start_offset + 1, 8, 100).unwrap_err();
    assert_eq!(mid_record.kind, ErrorKind::Malformed);
    let inside_record = catch_up(&source, &path, messages[0].end_offset - 2, 8, 100).unwrap_err();
    assert_eq!(inside_record.kind, ErrorKind::Malformed);

    let interleaved = temp_dir("catch-interleave-home");
    let interleaved_session = "sess-catch-interleave";
    let interleaved_path = write_rollout(
        &interleaved,
        interleaved_session,
        &jsonl(&[
            session_meta(interleaved_session),
            user_msg_turn("m1", "t1", "AA"),
            user_msg_turn("m2", "t2", "BB"),
            user_msg_turn("m3", "t1", "CC"),
            user_msg_turn("m4", "t3", "DDDDDDDD"),
        ]),
    );
    let (interleaved_source, _) = open_source(&interleaved, interleaved_session);
    let repeated = catch_up(&interleaved_source, &interleaved_path, 0, 8, 100).unwrap_err();
    assert_eq!(repeated.kind, ErrorKind::Mismatch);
    assert_eq!(repeated.message, "catch-up turn boundary is not contiguous");

    let past_eof = catch_up(
        &source,
        &path,
        fs::metadata(&path).unwrap().len().saturating_add(10),
        8,
        100,
    )
    .unwrap_err();
    assert_eq!(past_eof.kind, ErrorKind::Truncated);
}
