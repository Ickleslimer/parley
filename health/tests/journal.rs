use std::fs;
use std::io::Write;

use parley_health::journal;
use parley_health::schema::{InboxKind, SCHEMA_VERSION};

mod common;

#[test]
fn rebuilds_after_incomplete_trailing_journal_data() {
    let home = common::TempHome::new("journal-trail");
    let complete = common::codex_sample("codex-1", 10, None);
    journal::append(&home.paths.journal(), &complete).unwrap();
    {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.paths.journal())
            .unwrap();
        file.write_all(br#"{"schema_version":1,"inbox_id":"partial"#)
            .unwrap();
        file.flush().unwrap();
    }

    let loaded = journal::load(&home.paths.journal()).unwrap();
    assert!(loaded.diagnostics.journal_incomplete_trailing);
    assert_eq!(loaded.records.len(), 1);
    assert_eq!(loaded.records[0].inbox_id, "codex-1");
    assert_eq!(loaded.records[0].schema_version, SCHEMA_VERSION);
    assert_eq!(loaded.records[0].kind, InboxKind::CodexSample);

    let original = fs::read(home.paths.journal()).unwrap();
    assert!(original.windows(7).any(|w| w == b"partial"));
}

#[test]
fn skips_malformed_and_unsupported_journal_lines_without_deleting() {
    let home = common::TempHome::new("journal-malformed");
    let body = format!(
        "{}\nnot-json\n{{\"schema_version\":2,\"inbox_id\":\"x\",\"kind\":\"codex_sample\",\"as_of_ms\":1}}\n",
        serde_json::to_string(&common::codex_sample("codex-ok", 11, None)).unwrap()
    );
    fs::write(home.paths.journal(), body).unwrap();
    let loaded = journal::load(&home.paths.journal()).unwrap();
    assert_eq!(loaded.records.len(), 1);
    assert_eq!(loaded.records[0].inbox_id, "codex-ok");
    assert_eq!(loaded.diagnostics.malformed_journal_lines, 1);
    assert_eq!(loaded.diagnostics.unsupported_journal_records, 1);
    assert!(fs::read_to_string(home.paths.journal())
        .unwrap()
        .contains("not-json"));
}

#[test]
fn restart_replay_does_not_duplicate_incidents() {
    let home = common::TempHome::new("journal-replay");
    let mut first = common::supervisor(&home);
    first
        .ingest(common::codex_sample("codex-ex", 20, Some("primary")), 20)
        .unwrap();
    assert_eq!(first.model.incidents.len(), 1);
    let id = first.model.incidents[0].incident_id.clone();

    let second = common::supervisor(&home);
    assert_eq!(second.model.incidents.len(), 1);
    assert_eq!(second.model.incidents[0].incident_id, id);
}
