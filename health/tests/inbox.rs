use std::fs;

use parley_health::inbox;
use parley_health::schema::InboxKind;

mod common;

#[test]
fn malformed_inbox_records_are_quarantined_without_crash() {
    let home = common::TempHome::new("inbox-malformed");
    common::write_bytes(&home.paths.inbox().join("bad.json"), b"{not-json");
    let mut supervisor = common::supervisor(&home);
    let outcome = supervisor.tick(50).unwrap();
    assert_eq!(outcome.quarantined, 1);
    assert!(!home.paths.inbox().join("bad.json").exists());
    let quarantined = fs::read_dir(home.paths.quarantine())
        .unwrap()
        .flatten()
        .count();
    assert_eq!(quarantined, 1);
    assert!(supervisor.model.incidents.is_empty());
}

#[test]
fn partial_tmp_inbox_files_are_ignored_until_complete() {
    let home = common::TempHome::new("inbox-partial");
    common::write_bytes(
        &home.paths.inbox().join(".still-writing.json.tmp"),
        br#"{"schema_version":1,"inbox_id":"x","kind":"codex_sample","as_of_ms":1}"#,
    );
    let mut supervisor = common::supervisor(&home);
    let outcome = supervisor.tick(50).unwrap();
    assert_eq!(outcome.consumed, 0);
    assert_eq!(outcome.quarantined, 0);
    assert!(home.paths.inbox().join(".still-writing.json.tmp").exists());
}

#[test]
fn producer_atomic_rename_is_consumed_once() {
    let home = common::TempHome::new("inbox-atomic");
    let record = common::codex_sample("once-1", 9, None);
    inbox::write_record(&home.paths, &record).unwrap();
    let mut supervisor = common::supervisor(&home);
    let first = supervisor.tick(9).unwrap();
    assert_eq!(first.consumed, 1);
    let second = supervisor.tick(10).unwrap();
    assert_eq!(second.consumed, 0);
    assert_eq!(supervisor.model.processed_inbox_ids.len(), 1);
}

#[test]
fn viewer_control_records_are_explicit() {
    let ack = inbox::acknowledge_record("inc-1", 3).unwrap();
    assert_eq!(ack.kind, InboxKind::Acknowledge);
    assert_eq!(ack.incident_id.as_deref(), Some("inc-1"));

    let mute = inbox::mute_record(true, 4);
    assert_eq!(mute.kind, InboxKind::Mute);
    assert_eq!(mute.muted, Some(true));

    let test = inbox::test_sound_record(5);
    assert_eq!(test.kind, InboxKind::TestSound);
    assert_eq!(test.test_sound, Some(true));
    assert!(test.class.is_none());
}
