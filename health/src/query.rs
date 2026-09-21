use std::ffi::OsString;

use crate::paths::HealthPaths;
use crate::schema::{now_ms, QueryDocument, UnavailableReason};
use crate::snapshot;

pub struct QueryResult {
    pub json: String,
    pub exit_code: i32,
}

pub fn run(extra_args: &[OsString]) -> QueryResult {
    if !extra_args.is_empty() {
        return encode(
            QueryDocument::unavailable(UnavailableReason::ArgumentsNotAllowed, now_ms()),
            2,
        );
    }
    let paths = HealthPaths::from_env();
    encode(snapshot::read_with_retry(&paths), 0)
}

pub fn run_with_paths(paths: &HealthPaths) -> QueryResult {
    encode(snapshot::read_with_retry(paths), 0)
}

fn encode(document: QueryDocument, exit_code: i32) -> QueryResult {
    let json = serde_json::to_string(&document).unwrap_or_else(|_| {
        serde_json::to_string(&QueryDocument::unavailable(
            UnavailableReason::Malformed,
            now_ms(),
        ))
        .unwrap_or_else(|_| {
            r#"{"schema_version":1,"generated_ms":0,"muted":false,"unread_count":0,"active_incidents":[],"recent_incidents":[],"unavailable":{"reason":"malformed"},"stale":true,"diagnostics":{"snapshot_missing":false,"snapshot_malformed":true,"snapshot_locked":false,"journal_incomplete_trailing":false,"malformed_journal_lines":0,"oversized_journal_lines":0,"unsupported_journal_records":0,"quarantined_inbox":0,"malformed_inbox":0,"sound_failures":0,"footer_missing":0}}"#.to_string()
        })
    });
    QueryResult { json, exit_code }
}
