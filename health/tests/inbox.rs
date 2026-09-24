use std::fs;

use parley_health::inbox;
use parley_health::schema::{InboxKind, SCHEMA_VERSION};

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

#[test]
fn v1_v2_and_v3_inbox_records_are_accepted_and_new_writes_are_v3() {
    let home = common::TempHome::new("inbox-migration");
    let mut legacy = common::codex_sample("legacy-inbox", 1, None);
    legacy.schema_version = 1;
    let path = inbox::write_record(&home.paths, &legacy).unwrap();
    let written: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["schema_version"], SCHEMA_VERSION);

    for version in [1_u32, 2, 3] {
        let mut direct =
            common::codex_sample(&format!("direct-{version}"), u64::from(version), None);
        direct.schema_version = version;
        let direct_path = home.paths.inbox().join(format!("direct-{version}.json"));
        fs::write(&direct_path, serde_json::to_vec(&direct).unwrap()).unwrap();
        let parsed = inbox::read_record(&direct_path).unwrap();
        assert_eq!(parsed.schema_version, version);
    }

    let mut rejected = common::codex_sample("direct-4", 4, None);
    rejected.schema_version = 4;
    let rejected_path = home.paths.inbox().join("direct-4.json");
    fs::write(&rejected_path, serde_json::to_vec(&rejected).unwrap()).unwrap();
    assert!(inbox::read_record(&rejected_path).is_err());
}
