use std::fs;

use parley_health::schema::{QueryDocument, SCHEMA_VERSION};
use parley_health::snapshot;

mod common;

#[test]
fn snapshot_uses_atomic_replace_and_is_readable_after_rewrite() {
    let home = common::TempHome::new("snapshot-atomic");
    let mut first = QueryDocument::unavailable(
        parley_health::schema::UnavailableReason::Missing,
        1,
    );
    first.stale = false;
    first.unavailable = None;
    first.muted = true;
    snapshot::write(&home.paths, &first).unwrap();
    let temps: Vec<_> = fs::read_dir(&home.paths.root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("tmp"))
        .collect();
    assert!(temps.is_empty(), "temp snapshot files must not remain: {temps:?}");

    let read = snapshot::read_with_retry(&home.paths);
    assert_eq!(read.schema_version, SCHEMA_VERSION);
    assert!(read.muted);
    assert!(read.unavailable.is_none());

    first.muted = false;
    first.unread_count = 3;
    snapshot::write(&home.paths, &first).unwrap();
    let read = snapshot::read_with_retry(&home.paths);
    assert!(!read.muted);
    assert_eq!(read.unread_count, 3);
}

#[test]
fn query_document_has_no_peer_alive_or_continuity_fields() {
    let home = common::TempHome::new("snapshot-fields");
    let mut supervisor = common::supervisor(&home);
    supervisor
        .ingest(common::codex_sample("s1", 5, None), 5)
        .unwrap();
    let json = fs::read_to_string(home.paths.snapshot()).unwrap();
    assert!(json.contains("schema_version"));
    assert!(json.contains("latest_codex_sample"));
    assert!(!json.contains("peer_alive"));
    assert!(!json.contains("continuity"));
    assert!(!json.contains("authorization"));
}
