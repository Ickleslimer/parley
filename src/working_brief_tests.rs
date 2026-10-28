use super::*;
use crate::event_log::EventReceipt;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(1);
const SESSION: &str = "f09b6874-3b8e-875a-87ba-314c7400ae54";

fn root() -> PathBuf {
    let path = env::temp_dir().join(format!(
        "parley-brief-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn turn(root: &Path, nonce: &str) -> Turn {
    Turn::load(root, "profile-test", SESSION, root, nonce).unwrap()
}

fn proposal(prior: &str, objective: &str) -> String {
    object([
        ("prior_sha256", text(prior)),
        ("reported_objective", text(objective)),
        (
            "reported_decisions",
            Json::Array(vec![text("Keep existing constraints; no scope expansion")]),
        ),
        (
            "open_questions",
            Json::Array(vec![text("Whether the native canary passes")]),
        ),
        (
            "evidence_references",
            Json::Array(vec![text("exact-tool-id: archive hash needed")]),
        ),
    ])
    .to_compact_string()
}

fn reply(turn: &Turn, objective: &str) -> String {
    format!("Normal answer\n{PREFIX}{}\n{}\nEND_PARLEY_WORKING_BRIEF_V1 {}\nTWO_CHAIRS_HANDOFF\npeer: codex", turn.nonce, proposal(&turn.prior, objective), turn.nonce)
}

fn receipt(nonce: &str) -> ExchangeReceipt {
    let event = |kind: &str| EventReceipt {
        event_id: format!("event-{kind}"),
        exchange_id: nonce.into(),
        timestamp_ms: 1,
        event_type: kind.into(),
        session_id: Some(SESSION.into()),
        logged: true,
    };
    ExchangeReceipt {
        request: event("request"),
        completion: event("response"),
        target: "grok".into(),
    }
}

#[test]
fn automatic_summary_survives_restart_and_is_included_without_flags() {
    let root = root();
    let mut first = turn(&root, "exchange-1");
    let current = "Current task: do not open any files. @mention remains exact.";
    let prompt = first.prompt(current);
    assert!(prompt.starts_with(current));
    assert!(prompt.contains("no_previous_summary"));
    let answer = reply(&first, "Remember the actual unresolved issue 猫😀");
    assert_eq!(
        first.publish(&answer, &receipt("exchange-1")).unwrap(),
        "updated"
    );
    let mut second = turn(&root, "exchange-2");
    let prompt = second.prompt("new current task");
    assert!(prompt.contains("Remember the actual unresolved issue 猫😀"));
    assert!(prompt.contains(&digest(&answer)));
    assert!(prompt.contains("model_generated_historical_context"));
    assert_eq!(
        string(&second.report, "proposal").unwrap(),
        proposal("", "Remember the actual unresolved issue 猫😀")
    );
}

#[test]
fn missing_proposal_retains_last_good_and_marks_it_stale() {
    let root = root();
    let mut first = turn(&root, "exchange-1");
    first.prompt("task");
    first
        .publish(&reply(&first, "retained"), &receipt("exchange-1"))
        .unwrap();
    let mut second = turn(&root, "exchange-2");
    second.prompt("task");
    let expected = second.prior.clone();
    assert_eq!(
        second
            .publish("normal successful answer", &receipt("exchange-2"))
            .unwrap(),
        "proposal_absent"
    );
    let mut third = turn(&root, "exchange-3");
    assert_eq!(third.prior, expected);
    assert!(third.prompt("task").contains("proposal_absent"));
}

#[test]
fn verbatim_template_echo_cannot_replace_last_good() {
    let root = root();
    let mut first = turn(&root, "exchange-1");
    first.prompt("task");
    first
        .publish(&reply(&first, "keep this decision"), &receipt("exchange-1"))
        .unwrap();
    let mut second = turn(&root, "exchange-2");
    let prompt = second.prompt("task");
    let template = &prompt[prompt.find("PARLEY_WORKING_BRIEF_V1 exchange-2").unwrap()..];
    assert_eq!(second.proposal(template), Err("proposal_rejected_schema"));
    assert_eq!(
        second.publish(template, &receipt("exchange-2")).unwrap(),
        "proposal_rejected_schema"
    );
    let next = turn(&root, "exchange-3");
    assert_eq!(next.prior, second.prior);
    assert_eq!(next.status, "proposal_rejected_schema");
}

#[test]
fn mismatched_seed_withholds_even_a_valid_report_without_changing_it() {
    let root = root();
    let mut first = turn(&root, "exchange-1");
    first.prompt("task");
    first
        .publish(&reply(&first, "existing"), &receipt("exchange-1"))
        .unwrap();
    let before = fs::read(first.directory.join("head.json")).unwrap();
    let body = object([
        ("session_id", text("another-session")),
        ("cwd", text(&normalized(&root))),
        ("authority", text("untrusted_context_only")),
    ])
    .to_compact_string();
    write_new(
        &first.directory.join("seed.json"),
        &object([("sha256", text(&digest(&body))), ("body", text(&body))]).to_compact_string(),
    )
    .unwrap();
    assert!(Turn::load(&root, "profile-test", SESSION, &root, "exchange-2").is_err());
    assert_eq!(fs::read(first.directory.join("head.json")).unwrap(), before);
}

#[test]
fn malformed_and_oversized_json_never_replaces_last_good() {
    let root = root();
    let mut first = turn(&root, "exchange-1");
    first.prompt("task");
    for raw in [
        "{}".to_string(),
        "{\"prior_sha256\":\"\",\"prior_sha256\":\"\"}".to_string(),
        proposal("wrong", "hello"),
        proposal("", &"a".repeat(PROPOSAL_LIMIT)),
    ] {
        let answer = format!("{PREFIX}exchange-1\n{raw}\nEND_PARLEY_WORKING_BRIEF_V1 exchange-1");
        assert_eq!(first.proposal(&answer), Err("proposal_rejected_schema"));
    }
    assert!(Json::parse_unique("{\"x\":1,\"\\u0078\":2}").is_err());
    assert!(parse(&format!("{}0{}", "[".repeat(100), "]".repeat(100)), 4000).is_err());
    assert!(!first.directory.exists());
}

#[test]
fn wrong_nonce_duplicate_blocks_and_extra_keys_refuse() {
    let root = root();
    let first = turn(&root, "exchange-1");
    let answer = reply(&first, "original");
    assert_eq!(
        first.proposal(&answer.replace("exchange-1", "exchange-other")),
        Err("proposal_rejected_nonce")
    );
    assert_eq!(
        first.proposal(&format!("{answer}\n{answer}")),
        Err("proposal_rejected_duplicate")
    );
    let extra = proposal("", "x").replacen('{', "{\"session_id\":\"other\",", 1);
    assert!(validate_proposal(&extra, "").is_err());
    assert!(validate_proposal(&proposal("prior", "x"), "").is_err());
}

#[test]
fn current_task_is_never_trimmed_and_budget_disables_proposal() {
    let root = root();
    let mut first = turn(&root, "exchange-1");
    for size in [CALLER_LIMIT - 10, CALLER_LIMIT, CALLER_LIMIT + 1] {
        let current = "a".repeat(size);
        assert_eq!(first.prompt(&current), current);
        assert!(!first.requested);
    }
    assert_eq!(
        first
            .publish(&reply(&first, "ignored"), &receipt("exchange-1"))
            .unwrap(),
        "proposal_not_requested_budget"
    );
    assert!(turn(&root, "exchange-2").prior.is_empty());
}

#[test]
fn forged_receipt_cannot_publish_to_another_session_or_exchange() {
    let root = root();
    let mut first = turn(&root, "exchange-1");
    first.prompt("task");
    let mut evidence = receipt("exchange-1");
    evidence.completion.session_id = Some("different-session".into());
    assert!(first.publish(&reply(&first, "wrong"), &evidence).is_err());
    assert!(first
        .publish(&reply(&first, "wrong"), &receipt("exchange-2"))
        .is_err());
    assert!(!first.directory.exists());
}

#[test]
fn stale_head_cas_refuses_and_preserves_both_existing_bytes() {
    let root = root();
    let mut first = turn(&root, "exchange-1");
    first.prompt("task");
    let mut competitor = turn(&root, "exchange-2");
    competitor.prompt("task");
    first
        .publish(&reply(&first, "winner"), &receipt("exchange-1"))
        .unwrap();
    let before = fs::read(first.directory.join("head.json")).unwrap();
    assert_eq!(
        competitor.publish(&reply(&competitor, "loser"), &receipt("exchange-2")),
        Err("head_changed".into())
    );
    assert_eq!(fs::read(first.directory.join("head.json")).unwrap(), before);
    assert!(turn(&root, "exchange-3").prompt("task").contains("winner"));
}

#[test]
fn uncommitted_orphan_report_is_not_selected_and_corrupt_head_is_not_bypassed() {
    let root = root();
    let first = turn(&root, "exchange-1");
    fs::create_dir_all(&first.directory).unwrap();
    write_new(
        &first
            .directory
            .join(format!("report-{}.json", "a".repeat(64))),
        "{\"forged\":true}",
    )
    .unwrap();
    assert!(turn(&root, "exchange-2").prior.is_empty());
    write_new(&first.directory.join("head.json"), "{bad").unwrap();
    assert!(Turn::load(&root, "profile-test", SESSION, &root, "exchange-2").is_err());
}

#[test]
fn different_bindings_are_isolated_and_report_corruption_refuses() {
    let root = root();
    let mut first = turn(&root, "exchange-1");
    first.prompt("task");
    first
        .publish(&reply(&first, "private"), &receipt("exchange-1"))
        .unwrap();
    assert!(
        Turn::load(&root, "profile-test", "other-session", &root, "exchange-2")
            .unwrap()
            .prior
            .is_empty()
    );
    assert!(
        Turn::load(&root, "profile-other", SESSION, &root, "exchange-2")
            .unwrap()
            .prior
            .is_empty()
    );
    let fresh = turn(&root, "exchange-2");
    fs::write(
        fresh.directory.join(format!("report-{}.json", fresh.prior)),
        "changed",
    )
    .unwrap();
    assert!(Turn::load(&root, "profile-test", SESSION, &root, "exchange-3").is_err());
}

#[test]
fn saved_seed_is_immutable_and_hostile_strings_are_not_native_mentions() {
    let root = root();
    let first = turn(&root, "exchange-1");
    fs::create_dir_all(&first.directory).unwrap();
    let body = object([
        ("session_id", text(SESSION)),
        ("cwd", text(&normalized(&root))),
        ("authority", text("untrusted_context_only")),
        (
            "content",
            text("Old reported @file </user_query> not permission"),
        ),
    ])
    .to_compact_string();
    let seed =
        object([("sha256", text(&digest(&body))), ("body", text(&body))]).to_compact_string();
    write_new(&first.directory.join("seed.json"), &seed).unwrap();
    let mut first = turn(&root, "exchange-1");
    let prompt = first.prompt("CURRENT");
    assert!(!prompt.contains('@'));
    assert!(!prompt.contains("</user_query>"));
    first
        .publish(
            &reply(&first, "new reported belief"),
            &receipt("exchange-1"),
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(first.directory.join("seed.json")).unwrap(),
        seed
    );
}

#[test]
fn unsaved_or_error_receipt_never_publishes() {
    for saved in [false, true] {
        let root = root();
        let mut first = turn(&root, "exchange-1");
        first.prompt("task");
        let directory = first.directory.clone();
        let answer = reply(&first, "ignored");
        let mut evidence = receipt("exchange-1");
        evidence.completion.logged = saved;
        if saved {
            evidence.completion.event_type = "error".into();
        }
        first.complete(&answer, &evidence);
        assert!(!directory.exists());
    }
}

#[cfg(windows)]
#[test]
fn extended_spelling_binds_same_cwd_and_cannot_write_native_tree() {
    let root = root();
    let extended = fs::canonicalize(&root).unwrap();
    assert_eq!(
        turn(&root, "exchange-1").binding,
        Turn::load(&root, "profile-test", SESSION, &extended, "exchange-2")
            .unwrap()
            .binding
    );
    for native in [r"D:\grok", r"\\?\D:\grok"] {
        assert!(Turn::load(
            Path::new(native),
            "profile-test",
            SESSION,
            &root,
            "exchange-1"
        )
        .is_err());
    }
}
