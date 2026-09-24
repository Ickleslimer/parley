use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::process::Command;

use parley_health::query;
use parley_health::schema::{now_ms, UnavailableReason};
use serde_json::Value;

mod common;

fn query_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_parley-health-query"))
}

fn parse_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn zero_argument_query_reads_snapshot_and_rejects_extra_args() {
    let home = common::TempHome::new("query-args");
    let mut supervisor = common::supervisor(&home);
    let sample_ms = now_ms();
    supervisor
        .ingest(common::codex_sample("q-1", sample_ms, None), sample_ms)
        .unwrap();

    let output = Command::new(query_bin())
        .env("PARLEY_HEALTH_HOME", &home.paths.root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let json = parse_json(&String::from_utf8(output.stdout).unwrap());
    assert_eq!(json["schema_version"], parley_health::SCHEMA_VERSION);
    assert_eq!(json["stale"], false);
    assert!(json["unavailable"].is_null());
    assert!(json.get("peer_alive").is_none());

    let before = fs::metadata(home.paths.snapshot()).unwrap();
    let denied = Command::new(query_bin())
        .arg("--help")
        .env("PARLEY_HEALTH_HOME", &home.paths.root)
        .output()
        .unwrap();
    assert_eq!(denied.status.code(), Some(2));
    let denied_json = parse_json(&String::from_utf8(denied.stdout).unwrap());
    assert_eq!(
        denied_json["unavailable"]["reason"],
        "arguments_not_allowed"
    );
    assert_eq!(denied_json["stale"], true);
    let after = fs::metadata(home.paths.snapshot()).unwrap();
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
}

#[test]
fn missing_and_malformed_snapshots_exit_zero_with_unavailable_stale() {
    let missing_home = common::TempHome::new("query-missing");
    let missing = Command::new(query_bin())
        .env("PARLEY_HEALTH_HOME", &missing_home.paths.root)
        .output()
        .unwrap();
    assert!(missing.status.success());
    let json = parse_json(&String::from_utf8(missing.stdout).unwrap());
    assert_eq!(json["unavailable"]["reason"], "missing");
    assert_eq!(json["stale"], true);
    assert_eq!(json["diagnostics"]["snapshot_missing"], true);

    let malformed_home = common::TempHome::new("query-malformed");
    fs::write(malformed_home.paths.snapshot(), b"{nope").unwrap();
    let malformed = Command::new(query_bin())
        .env("PARLEY_HEALTH_HOME", &malformed_home.paths.root)
        .output()
        .unwrap();
    assert!(malformed.status.success());
    let json = parse_json(&String::from_utf8(malformed.stdout).unwrap());
    assert_eq!(json["unavailable"]["reason"], "malformed");
    assert_eq!(json["stale"], true);
}

#[test]
fn locked_snapshot_is_reported_unavailable_without_write() {
    let home = common::TempHome::new("query-locked");
    fs::write(home.paths.snapshot(), b"{}").unwrap();
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(home.paths.snapshot())
        .unwrap();
    let result = query::run_with_paths(&home.paths);
    assert_eq!(result.exit_code, 0);
    let json = parse_json(&result.json);
    assert_eq!(json["unavailable"]["reason"], "locked");
    assert_eq!(json["stale"], true);
    let _ = UnavailableReason::Locked;
}

#[test]
fn query_binary_does_not_create_inbox_or_journal() {
    let home = common::TempHome::new("query-readonly");
    let _ = Command::new(query_bin())
        .env("PARLEY_HEALTH_HOME", &home.paths.root)
        .output()
        .unwrap();
    assert!(!home.paths.journal().exists());
    let inbox_entries = fs::read_dir(home.paths.inbox()).unwrap().flatten().count();
    assert_eq!(inbox_entries, 0);
}
