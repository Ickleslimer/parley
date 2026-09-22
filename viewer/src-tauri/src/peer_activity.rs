use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use parley_handoff::fsutil::{self, FailKind};
use parley_handoff::schema::{StoredRecord, MAX_EXCERPT_CHARS, MAX_REPORT_CHARS};
use parley_handoff::sha256_hex;
use parley_handoff::store;
use serde::Serialize;

const MAX_DETAIL_HANDOFFS: usize = 16;
const REPORT_BYTE_CEILING: usize = MAX_REPORT_CHARS.saturating_mul(4).saturating_add(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ConfiguredSourceKind {
    Environment,
    LocalAppData,
    Unconfigured,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EvidenceDiagnostic {
    Missing,
    Malformed,
    Locked,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReportAvailability {
    Absent,
    Available,
    Missing,
    Malformed,
    Locked,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfiguredSourceStatus {
    kind: ConfiguredSourceKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct PeerActivityDiagnostics {
    root_missing: bool,
    root_locked: bool,
    root_malformed: bool,
    jobs_missing: bool,
    jobs_locked: bool,
    jobs_malformed: bool,
    skipped_entries: u64,
    missing_jobs: u64,
    malformed_jobs: u64,
    locked_jobs: u64,
    unavailable_jobs: u64,
    report_mismatches: u64,
    report_missing: u64,
    report_locked: u64,
    report_malformed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerActivityEvent {
    class: String,
    timestamp_ms: u64,
    tool_name: Option<String>,
    status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerHandoffItem {
    job_id: String,
    handoff_id: Option<String>,
    source_session_id: Option<String>,
    target_session_id: Option<String>,
    state: Option<String>,
    phase: Option<String>,
    process_state: Option<String>,
    created_at_ms: Option<u64>,
    updated_at_ms: Option<u64>,
    last_activity_ms: Option<u64>,
    ready_at_ms: Option<u64>,
    deadline_ms: Option<u64>,
    receipt_at_ms: Option<u64>,
    alert_incident_id: Option<String>,
    record_diagnostic: Option<EvidenceDiagnostic>,
    excerpt_text: Option<String>,
    excerpt_truncated: bool,
    activities: Vec<PeerActivityEvent>,
    report_availability: ReportAvailability,
    report_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerActivitySnapshot {
    generated_ms: u64,
    assessment: String,
    source: ConfiguredSourceStatus,
    unavailable: Option<EvidenceDiagnostic>,
    diagnostics: PeerActivityDiagnostics,
    shown_count: u32,
    total_count: u32,
    truncated: bool,
    handoffs: Vec<PeerHandoffItem>,
}

enum RootChoice {
    Selected {
        root: PathBuf,
        kind: ConfiguredSourceKind,
    },
    Rejected {
        kind: ConfiguredSourceKind,
        reason: EvidenceDiagnostic,
    },
}

enum DirState {
    Ready,
    Missing,
    Locked,
    Reparse,
    NotDirectory,
    Unavailable,
}

pub(crate) fn snapshot() -> PeerActivitySnapshot {
    load_choice(resolve_root(), MAX_DETAIL_HANDOFFS, now_ms())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn resolve_root() -> RootChoice {
    resolve_root_with(
        std::env::var_os("PARLEY_HANDOFF_STATE_DIR"),
        std::env::var_os("LOCALAPPDATA"),
        std::env::var_os("USERPROFILE"),
    )
}

fn resolve_root_with(
    configured: Option<std::ffi::OsString>,
    local_app_data: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
) -> RootChoice {
    if let Some(configured) = configured {
        let path = PathBuf::from(&configured);
        if configured.is_empty() || !path.is_absolute() {
            return RootChoice::Rejected {
                kind: ConfiguredSourceKind::Environment,
                reason: EvidenceDiagnostic::Malformed,
            };
        }
        return RootChoice::Selected {
            root: path,
            kind: ConfiguredSourceKind::Environment,
        };
    }
    if let Some(root) = absolute_os_path(local_app_data) {
        return RootChoice::Selected {
            root: root.join("Parley").join("handoffs"),
            kind: ConfiguredSourceKind::LocalAppData,
        };
    }
    if let Some(profile) = absolute_os_path(user_profile) {
        return RootChoice::Selected {
            root: profile
                .join("AppData")
                .join("Local")
                .join("Parley")
                .join("handoffs"),
            kind: ConfiguredSourceKind::LocalAppData,
        };
    }
    RootChoice::Rejected {
        kind: ConfiguredSourceKind::Unconfigured,
        reason: EvidenceDiagnostic::Missing,
    }
}

fn absolute_os_path(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    path.is_absolute().then_some(path)
}

fn load_choice(choice: RootChoice, limit: usize, generated_ms: u64) -> PeerActivitySnapshot {
    match choice {
        RootChoice::Rejected { kind, reason } => {
            let mut diagnostics = PeerActivityDiagnostics::default();
            if reason == EvidenceDiagnostic::Malformed {
                diagnostics.root_malformed = true;
            }
            if reason == EvidenceDiagnostic::Missing {
                diagnostics.root_missing = true;
            }
            empty_snapshot(generated_ms, kind, Some(reason), diagnostics)
        }
        RootChoice::Selected { root, kind } => load_root(&root, kind, limit, generated_ms),
    }
}

fn empty_snapshot(
    generated_ms: u64,
    kind: ConfiguredSourceKind,
    unavailable: Option<EvidenceDiagnostic>,
    diagnostics: PeerActivityDiagnostics,
) -> PeerActivitySnapshot {
    PeerActivitySnapshot {
        generated_ms,
        assessment: parley_handoff::schema::NOT_INFERRED.to_string(),
        source: ConfiguredSourceStatus { kind },
        unavailable,
        diagnostics,
        shown_count: 0,
        total_count: 0,
        truncated: false,
        handoffs: Vec::new(),
    }
}

fn load_root(
    root: &Path,
    kind: ConfiguredSourceKind,
    limit: usize,
    generated_ms: u64,
) -> PeerActivitySnapshot {
    let mut diagnostics = PeerActivityDiagnostics::default();
    match directory_state(root) {
        DirState::Missing => {
            diagnostics.root_missing = true;
            return empty_snapshot(
                generated_ms,
                kind,
                Some(EvidenceDiagnostic::Missing),
                diagnostics,
            );
        }
        DirState::Locked => {
            diagnostics.root_locked = true;
            return empty_snapshot(
                generated_ms,
                kind,
                Some(EvidenceDiagnostic::Locked),
                diagnostics,
            );
        }
        DirState::Reparse | DirState::NotDirectory => {
            diagnostics.root_malformed = true;
            return empty_snapshot(
                generated_ms,
                kind,
                Some(EvidenceDiagnostic::Malformed),
                diagnostics,
            );
        }
        DirState::Unavailable => {
            return empty_snapshot(
                generated_ms,
                kind,
                Some(EvidenceDiagnostic::Unavailable),
                diagnostics,
            );
        }
        DirState::Ready => {}
    }

    let jobs = root.join("jobs");
    let mut items = match directory_state(&jobs) {
        DirState::Ready => match read_job_entries(&jobs, &mut diagnostics) {
            Ok(items) => items,
            Err(reason) => {
                note_jobs_directory(&mut diagnostics, reason);
                return empty_snapshot(generated_ms, kind, Some(reason), diagnostics);
            }
        },
        DirState::Missing => {
            diagnostics.jobs_missing = true;
            return empty_snapshot(
                generated_ms,
                kind,
                Some(EvidenceDiagnostic::Missing),
                diagnostics,
            );
        }
        DirState::Locked => {
            diagnostics.jobs_locked = true;
            return empty_snapshot(
                generated_ms,
                kind,
                Some(EvidenceDiagnostic::Locked),
                diagnostics,
            );
        }
        DirState::Reparse | DirState::NotDirectory => {
            diagnostics.jobs_malformed = true;
            return empty_snapshot(
                generated_ms,
                kind,
                Some(EvidenceDiagnostic::Malformed),
                diagnostics,
            );
        }
        DirState::Unavailable => {
            return empty_snapshot(
                generated_ms,
                kind,
                Some(EvidenceDiagnostic::Unavailable),
                diagnostics,
            );
        }
    };
    sort_newest_first(&mut items);
    let total = items.len();
    let limit = limit.max(1);
    let truncated = total > limit;
    if truncated {
        items.truncate(limit);
    }
    PeerActivitySnapshot {
        generated_ms,
        assessment: parley_handoff::schema::NOT_INFERRED.to_string(),
        source: ConfiguredSourceStatus { kind },
        unavailable: None,
        diagnostics,
        shown_count: u32::try_from(items.len()).unwrap_or(u32::MAX),
        total_count: u32::try_from(total).unwrap_or(u32::MAX),
        truncated,
        handoffs: items,
    }
}

fn note_jobs_directory(diagnostics: &mut PeerActivityDiagnostics, reason: EvidenceDiagnostic) {
    match reason {
        EvidenceDiagnostic::Missing => diagnostics.jobs_missing = true,
        EvidenceDiagnostic::Locked => diagnostics.jobs_locked = true,
        EvidenceDiagnostic::Malformed => diagnostics.jobs_malformed = true,
        EvidenceDiagnostic::Unavailable => {}
    }
}

fn read_job_entries(
    jobs: &Path,
    diagnostics: &mut PeerActivityDiagnostics,
) -> Result<Vec<PeerHandoffItem>, EvidenceDiagnostic> {
    let entries = fs::read_dir(jobs).map_err(diagnostic_from_io)?;
    let mut items = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                diagnostics.unavailable_jobs = diagnostics.unavailable_jobs.saturating_add(1);
                if fsutil::is_lock_denied(&error) {
                    diagnostics.locked_jobs = diagnostics.locked_jobs.saturating_add(1);
                }
                continue;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            diagnostics.skipped_entries = diagnostics.skipped_entries.saturating_add(1);
            continue;
        };
        if !safe_job_dir_name(&name) {
            diagnostics.skipped_entries = diagnostics.skipped_entries.saturating_add(1);
            continue;
        }
        let job_dir = jobs.join(&name);
        match directory_state(&job_dir) {
            DirState::Ready => items.push(load_job(&job_dir, &name, diagnostics)),
            DirState::Missing => {
                diagnostics.missing_jobs = diagnostics.missing_jobs.saturating_add(1);
                items.push(diagnostic_item(name, EvidenceDiagnostic::Missing));
            }
            DirState::Locked => {
                diagnostics.locked_jobs = diagnostics.locked_jobs.saturating_add(1);
                items.push(diagnostic_item(name, EvidenceDiagnostic::Locked));
            }
            DirState::Reparse | DirState::NotDirectory => {
                diagnostics.skipped_entries = diagnostics.skipped_entries.saturating_add(1);
            }
            DirState::Unavailable => {
                diagnostics.unavailable_jobs = diagnostics.unavailable_jobs.saturating_add(1);
                items.push(diagnostic_item(name, EvidenceDiagnostic::Unavailable));
            }
        }
    }
    Ok(items)
}

fn load_job(
    job_dir: &Path,
    name: &str,
    diagnostics: &mut PeerActivityDiagnostics,
) -> PeerHandoffItem {
    if !contained_job(job_dir, name) {
        diagnostics.unavailable_jobs = diagnostics.unavailable_jobs.saturating_add(1);
        return diagnostic_item(name.to_string(), EvidenceDiagnostic::Unavailable);
    }
    match store::load_readonly(job_dir) {
        Ok(record) => item_from_record(job_dir, record, diagnostics),
        Err(kind) => {
            let reason = diagnostic_from_fail(kind);
            note_job_failure(diagnostics, reason);
            diagnostic_item(name.to_string(), reason)
        }
    }
}

fn item_from_record(
    job_dir: &Path,
    record: StoredRecord,
    diagnostics: &mut PeerActivityDiagnostics,
) -> PeerHandoffItem {
    let (report_availability, report_text, mismatch) = match record.report_fingerprint.as_deref() {
        Some(fingerprint) => read_exact_report(job_dir, fingerprint),
        None => (ReportAvailability::Absent, None, false),
    };
    note_report(diagnostics, report_availability, mismatch);
    let excerpt_truncated =
        record.excerpt_truncated || record.excerpt.chars().count() > MAX_EXCERPT_CHARS;
    let excerpt_text = bounded_excerpt(&record.excerpt);
    PeerHandoffItem {
        job_id: record.job_id,
        handoff_id: Some(record.handoff_id),
        source_session_id: record.source_session_id,
        target_session_id: record.target_session_id,
        state: Some(record.state),
        phase: Some(record.phase),
        process_state: Some(record.process_state),
        created_at_ms: Some(record.created_at_ms),
        updated_at_ms: Some(record.updated_at_ms),
        last_activity_ms: record.last_activity_ms,
        ready_at_ms: record.ready_at_ms,
        deadline_ms: record.deadline_ms,
        receipt_at_ms: record.receipt_at_ms,
        alert_incident_id: record.alert_incident_id,
        record_diagnostic: None,
        excerpt_text: Some(excerpt_text),
        excerpt_truncated,
        activities: record
            .activities
            .into_iter()
            .take(parley_handoff::schema::MAX_ACTIVITY_EVENTS)
            .map(|activity| PeerActivityEvent {
                class: activity.class,
                timestamp_ms: activity.timestamp_ms,
                tool_name: activity.tool_name,
                status: activity.status,
            })
            .collect(),
        report_availability,
        report_text,
    }
}

fn bounded_excerpt(excerpt: &str) -> String {
    let count = excerpt.chars().count();
    if count <= MAX_EXCERPT_CHARS {
        return excerpt.to_string();
    }
    excerpt.chars().take(MAX_EXCERPT_CHARS).collect()
}

fn read_exact_report(
    job_dir: &Path,
    fingerprint: &str,
) -> (ReportAvailability, Option<String>, bool) {
    let path = job_dir.join("report.txt");
    match directory_file_state(&path) {
        DirState::Missing => return (ReportAvailability::Missing, None, false),
        DirState::Locked => return (ReportAvailability::Locked, None, false),
        DirState::Reparse | DirState::NotDirectory => {
            return (ReportAvailability::Unavailable, None, false);
        }
        DirState::Unavailable => return (ReportAvailability::Unavailable, None, false),
        DirState::Ready => {}
    }
    let bytes = match fsutil::read_shared(&path, REPORT_BYTE_CEILING) {
        Ok(bytes) => bytes,
        Err(FailKind::Missing) => return (ReportAvailability::Missing, None, false),
        Err(FailKind::Locked) => return (ReportAvailability::Locked, None, false),
        Err(FailKind::Malformed) => return (ReportAvailability::Malformed, None, false),
        Err(_) => return (ReportAvailability::Unavailable, None, false),
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return (ReportAvailability::Malformed, None, false);
    };
    if text.chars().count() > MAX_REPORT_CHARS {
        return (ReportAvailability::Malformed, None, false);
    }
    if !fingerprint.eq_ignore_ascii_case(&sha256_hex(&bytes)) {
        return (ReportAvailability::Malformed, None, true);
    }
    (ReportAvailability::Available, Some(text.to_string()), false)
}

fn directory_file_state(path: &Path) -> DirState {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return DirState::Missing,
        Err(error) if fsutil::is_lock_denied(&error) => return DirState::Locked,
        Err(_) => return DirState::Unavailable,
        Ok(metadata) if metadata.is_dir() => return DirState::NotDirectory,
        Ok(_) => {}
    }
    match fsutil::refuse_reparse_chain(path) {
        Ok(()) => DirState::Ready,
        Err(FailKind::Path) => DirState::Reparse,
        Err(FailKind::Locked) => DirState::Locked,
        Err(FailKind::Missing) => DirState::Missing,
        Err(_) => DirState::Unavailable,
    }
}

fn note_report(
    diagnostics: &mut PeerActivityDiagnostics,
    availability: ReportAvailability,
    mismatch: bool,
) {
    if mismatch {
        diagnostics.report_mismatches = diagnostics.report_mismatches.saturating_add(1);
    }
    match availability {
        ReportAvailability::Missing => {
            diagnostics.report_missing = diagnostics.report_missing.saturating_add(1);
        }
        ReportAvailability::Locked => {
            diagnostics.report_locked = diagnostics.report_locked.saturating_add(1);
        }
        ReportAvailability::Malformed => {
            diagnostics.report_malformed = diagnostics.report_malformed.saturating_add(1);
        }
        ReportAvailability::Absent
        | ReportAvailability::Available
        | ReportAvailability::Unavailable => {}
    }
}

fn note_job_failure(diagnostics: &mut PeerActivityDiagnostics, reason: EvidenceDiagnostic) {
    match reason {
        EvidenceDiagnostic::Missing => {
            diagnostics.missing_jobs = diagnostics.missing_jobs.saturating_add(1);
        }
        EvidenceDiagnostic::Malformed => {
            diagnostics.malformed_jobs = diagnostics.malformed_jobs.saturating_add(1);
        }
        EvidenceDiagnostic::Locked => {
            diagnostics.locked_jobs = diagnostics.locked_jobs.saturating_add(1);
        }
        EvidenceDiagnostic::Unavailable => {
            diagnostics.unavailable_jobs = diagnostics.unavailable_jobs.saturating_add(1);
        }
    }
}

fn diagnostic_item(job_id: String, reason: EvidenceDiagnostic) -> PeerHandoffItem {
    PeerHandoffItem {
        job_id,
        handoff_id: None,
        source_session_id: None,
        target_session_id: None,
        state: None,
        phase: None,
        process_state: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_activity_ms: None,
        ready_at_ms: None,
        deadline_ms: None,
        receipt_at_ms: None,
        alert_incident_id: None,
        record_diagnostic: Some(reason),
        excerpt_text: None,
        excerpt_truncated: false,
        activities: Vec::new(),
        report_availability: ReportAvailability::Absent,
        report_text: None,
    }
}

fn sort_newest_first(items: &mut [PeerHandoffItem]) {
    items.sort_by(|left, right| {
        right
            .updated_at_ms
            .is_some()
            .cmp(&left.updated_at_ms.is_some())
            .then(
                right
                    .updated_at_ms
                    .unwrap_or(0)
                    .cmp(&left.updated_at_ms.unwrap_or(0)),
            )
            .then(
                right
                    .created_at_ms
                    .unwrap_or(0)
                    .cmp(&left.created_at_ms.unwrap_or(0)),
            )
            .then(left.job_id.cmp(&right.job_id))
    });
}

fn contained_job(job_dir: &Path, name: &str) -> bool {
    let Ok(jobs) = fsutil::canonical_existing(job_dir.parent().unwrap_or(job_dir)) else {
        return false;
    };
    let Ok(job) = fsutil::canonical_existing(job_dir) else {
        return false;
    };
    let Ok(expected) = fsutil::canonical_existing(&jobs.join(name)) else {
        return false;
    };
    job == expected && job.starts_with(&jobs)
}

fn safe_job_dir_name(name: &str) -> bool {
    fsutil::valid_identifier(name)
        && !name.contains(['/', '\\', '.', ':', '<', '>', '|', '?', '*'])
        && name != "."
        && name != ".."
}

fn directory_state(path: &Path) -> DirState {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return DirState::Missing,
        Err(error) if fsutil::is_lock_denied(&error) => return DirState::Locked,
        Err(_) => return DirState::Unavailable,
        Ok(metadata) if !metadata.is_dir() => return DirState::NotDirectory,
        Ok(_) => {}
    }
    match fsutil::refuse_reparse_chain(path) {
        Ok(()) => DirState::Ready,
        Err(FailKind::Path) => DirState::Reparse,
        Err(FailKind::Locked) => DirState::Locked,
        Err(FailKind::Missing) => DirState::Missing,
        Err(_) => DirState::Unavailable,
    }
}

fn diagnostic_from_io(error: io::Error) -> EvidenceDiagnostic {
    if error.kind() == io::ErrorKind::NotFound {
        EvidenceDiagnostic::Missing
    } else if fsutil::is_lock_denied(&error) {
        EvidenceDiagnostic::Locked
    } else {
        EvidenceDiagnostic::Unavailable
    }
}

fn diagnostic_from_fail(kind: FailKind) -> EvidenceDiagnostic {
    match kind {
        FailKind::Missing => EvidenceDiagnostic::Missing,
        FailKind::Malformed => EvidenceDiagnostic::Malformed,
        FailKind::Locked => EvidenceDiagnostic::Locked,
        _ => EvidenceDiagnostic::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parley_handoff::schema::{ActivityRecord, HANDOFF_SCHEMA_VERSION};
    use serde_json::Value;
    use std::fs::OpenOptions;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "parley-viewer-activity-{label}-{}-{}",
                std::process::id(),
                now_ms()
            ));
            fs::create_dir_all(path.join("jobs")).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn record(job_id: &str, updated_at_ms: u64, state: &str) -> StoredRecord {
        let durable = matches!(
            state,
            "ready" | "awaiting_ack" | "acknowledged" | "detached_unacked"
        );
        StoredRecord {
            schema_version: HANDOFF_SCHEMA_VERSION,
            sequence: 1,
            job_id: job_id.to_string(),
            handoff_id: format!("handoff-{job_id}"),
            state: state.to_string(),
            source_session_id: Some("source-session".to_string()),
            target_session_id: Some("target-session".to_string()),
            capability_hash: "0123456789abcdef".repeat(4),
            process_id: Some(424242),
            process_state: "alive".to_string(),
            phase: "handoff_ready".to_string(),
            created_at_ms: 10,
            updated_at_ms,
            last_activity_ms: Some(updated_at_ms),
            ready_at_ms: durable.then_some(20),
            deadline_ms: durable.then_some(30),
            receipt_at_ms: (state == "acknowledged").then_some(40),
            wait_until_ms: None,
            report_fingerprint: None,
            alert_incident_id: None,
            excerpt: format!("excerpt-{job_id}"),
            excerpt_truncated: false,
            activities: vec![ActivityRecord {
                class: "tool_call".to_string(),
                timestamp_ms: updated_at_ms,
                tool_name: Some("read_file".to_string()),
                status: Some("completed".to_string()),
            }],
        }
    }

    fn write_journal(root: &Path, record: &StoredRecord) {
        let dir = root.join("jobs").join(&record.job_id);
        fs::create_dir_all(&dir).unwrap();
        let line = serde_json::to_string(record).unwrap();
        fs::write(dir.join("journal.jsonl"), format!("{line}\n")).unwrap();
    }

    fn load(root: &Path, limit: usize) -> PeerActivitySnapshot {
        load_root(root, ConfiguredSourceKind::Environment, limit, 7)
    }

    fn encoded(snapshot: &PeerActivitySnapshot) -> String {
        serde_json::to_string(snapshot).unwrap()
    }

    #[test]
    fn configured_root_falls_back_only_when_unset_and_never_embeds_a_path() {
        let selected =
            resolve_root_with(None, Some(r"C:\Users\example\AppData\Local".into()), None);
        match selected {
            RootChoice::Selected { root, kind } => {
                assert_eq!(kind, ConfiguredSourceKind::LocalAppData);
                assert!(root.ends_with(Path::new("Parley").join("handoffs")));
            }
            RootChoice::Rejected { .. } => panic!("expected local application data fallback"),
        }
        let profile = resolve_root_with(None, None, Some(r"C:\Users\example".into()));
        match profile {
            RootChoice::Selected { root, kind } => {
                assert_eq!(kind, ConfiguredSourceKind::LocalAppData);
                assert!(root.ends_with(
                    Path::new("AppData")
                        .join("Local")
                        .join("Parley")
                        .join("handoffs")
                ));
            }
            RootChoice::Rejected { .. } => panic!("expected profile fallback"),
        }
        let rejected = resolve_root_with(
            Some("relative\\handoffs".into()),
            Some(r"C:\Local".into()),
            None,
        );
        match rejected {
            RootChoice::Rejected { kind, reason } => {
                assert_eq!(kind, ConfiguredSourceKind::Environment);
                assert_eq!(reason, EvidenceDiagnostic::Malformed);
            }
            RootChoice::Selected { .. } => panic!("relative configuration must not fall through"),
        }
        let missing = resolve_root_with(None, None, None);
        match missing {
            RootChoice::Rejected { kind, reason } => {
                assert_eq!(kind, ConfiguredSourceKind::Unconfigured);
                assert_eq!(reason, EvidenceDiagnostic::Missing);
            }
            RootChoice::Selected { .. } => panic!("missing profile must not invent a path"),
        }
    }

    #[test]
    fn missing_malformed_and_locked_evidence_are_explicit() {
        let root = TempRoot::new("diagnostics");
        fs::write(root.path().join("outside.txt"), b"DO_NOT_LEAK_OUTSIDE").unwrap();
        fs::create_dir_all(root.path().join("jobs-old")).unwrap();
        fs::write(
            root.path().join("jobs-old").join("journal.jsonl"),
            b"DO_NOT_LEAK_OUTSIDE_JOURNAL\n",
        )
        .unwrap();

        let mut pending = record("job-pending", 50, "pending");
        pending.excerpt = "visible pending excerpt".to_string();
        pending.activities[0].class = "thought".to_string();
        pending.activities[0].tool_name = None;
        pending.activities[0].status = None;
        write_journal(root.path(), &pending);
        fs::write(
            root.path()
                .join("jobs")
                .join("job-pending")
                .join("report.txt"),
            b"DO_NOT_LEAK_PENDING_REPORT",
        )
        .unwrap();
        fs::create_dir_all(root.path().join("jobs").join("job-pending").join("nested")).unwrap();
        fs::write(
            root.path()
                .join("jobs")
                .join("job-pending")
                .join("nested")
                .join("secret.txt"),
            b"DO_NOT_LEAK_NESTED",
        )
        .unwrap();

        let broken = record("job-broken", 40, "pending");
        write_journal(root.path(), &broken);
        let journal = root
            .path()
            .join("jobs")
            .join("job-broken")
            .join("journal.jsonl");
        let mut bytes = fs::read(&journal).unwrap();
        bytes.extend_from_slice(b"{not-json}\n");
        fs::write(&journal, bytes).unwrap();

        fs::create_dir_all(root.path().join("jobs").join("job-empty")).unwrap();
        fs::create_dir_all(root.path().join("jobs").join("..not-a-job")).unwrap();
        fs::write(
            root.path()
                .join("jobs")
                .join("..not-a-job")
                .join("journal.jsonl"),
            b"DO_NOT_LEAK_BAD_NAME\n",
        )
        .unwrap();
        fs::write(
            root.path().join("jobs").join("notes.txt"),
            b"DO_NOT_LEAK_FILE",
        )
        .unwrap();

        let locked_dir = root.path().join("jobs").join("job-locked");
        fs::create_dir_all(&locked_dir).unwrap();
        let locked_journal = locked_dir.join("journal.jsonl");
        fs::write(&locked_journal, b"{}\n").unwrap();
        let held = exclusive_read(&locked_journal);

        let snapshot = load(root.path(), MAX_DETAIL_HANDOFFS);
        let body = encoded(&snapshot);
        assert!(snapshot.unavailable.is_none());
        assert_eq!(snapshot.assessment, "not_inferred");
        assert!(!body.contains("DO_NOT_LEAK"));
        assert!(!body.contains("0123456789abcdef0123456789abcdef"));
        assert!(!body.contains("424242"));
        assert!(!body.contains(&root.path().display().to_string()));
        assert_eq!(snapshot.diagnostics.malformed_jobs, 1);
        assert_eq!(snapshot.diagnostics.missing_jobs, 1);
        assert_eq!(snapshot.diagnostics.locked_jobs, 1);
        assert!(snapshot.diagnostics.skipped_entries >= 2);

        let pending = snapshot
            .handoffs
            .iter()
            .find(|item| item.job_id == "job-pending")
            .unwrap();
        assert_eq!(pending.report_availability, ReportAvailability::Absent);
        assert!(pending.report_text.is_none());
        assert_eq!(
            pending.excerpt_text.as_deref(),
            Some("visible pending excerpt")
        );
        assert_eq!(pending.activities[0].class, "thought");
        assert!(pending.activities[0].tool_name.is_none());
        let pending_json = serde_json::to_value(pending).unwrap();
        assert!(pending_json.get("capabilityHash").is_none());
        assert!(pending_json.get("processId").is_none());
        assert!(pending_json["activities"][0].get("input").is_none());
        assert!(pending_json["activities"][0].get("output").is_none());

        let broken = snapshot
            .handoffs
            .iter()
            .find(|item| item.job_id == "job-broken")
            .unwrap();
        assert_eq!(
            broken.record_diagnostic,
            Some(EvidenceDiagnostic::Malformed)
        );
        assert!(broken.excerpt_text.is_none());
        assert!(broken.state.is_none());

        let empty = snapshot
            .handoffs
            .iter()
            .find(|item| item.job_id == "job-empty")
            .unwrap();
        assert_eq!(empty.record_diagnostic, Some(EvidenceDiagnostic::Missing));

        let locked = snapshot
            .handoffs
            .iter()
            .find(|item| item.job_id == "job-locked")
            .unwrap();
        assert_eq!(locked.record_diagnostic, Some(EvidenceDiagnostic::Locked));
        drop(held);
        let _ = broken;
    }

    #[test]
    fn newest_records_keep_exact_reports_and_drop_mismatches() {
        let root = TempRoot::new("order");
        let older = durable_report(root.path(), "job-older", 100, "OLDER_EXACT_REPORT");
        let newest = durable_report(root.path(), "job-newest", 300, "NEWEST_EXACT_REPORT_α");
        let middle = durable_report(root.path(), "job-middle", 200, "MIDDLE_EXACT_REPORT");
        let _ = (older, newest, middle);

        let mut mismatched = record("job-mismatch", 400, "awaiting_ack");
        mismatched.report_fingerprint = Some(sha256_hex(b"expected-report"));
        mismatched.excerpt = "mismatch excerpt".to_string();
        write_journal(root.path(), &mismatched);
        fs::write(
            root.path()
                .join("jobs")
                .join("job-mismatch")
                .join("report.txt"),
            b"DO_NOT_LEAK_MISMATCH",
        )
        .unwrap();

        let mut oversized = record("job-oversize", 350, "awaiting_ack");
        let oversized_text = "x".repeat(MAX_REPORT_CHARS + 1);
        oversized.report_fingerprint = Some(sha256_hex(oversized_text.as_bytes()));
        write_journal(root.path(), &oversized);
        fs::write(
            root.path()
                .join("jobs")
                .join("job-oversize")
                .join("report.txt"),
            oversized_text.as_bytes(),
        )
        .unwrap();

        let snapshot = load(root.path(), 2);
        assert!(snapshot.truncated);
        assert_eq!(snapshot.total_count, 5);
        assert_eq!(snapshot.shown_count, 2);
        assert_eq!(snapshot.handoffs[0].job_id, "job-mismatch");
        assert_eq!(snapshot.handoffs[1].job_id, "job-oversize");
        assert!(snapshot.handoffs[0].report_text.is_none());
        assert_eq!(
            snapshot.handoffs[0].report_availability,
            ReportAvailability::Malformed
        );
        assert_eq!(snapshot.diagnostics.report_mismatches, 1);
        assert!(snapshot.diagnostics.report_malformed >= 2);
        let body = encoded(&snapshot);
        assert!(!body.contains("DO_NOT_LEAK_MISMATCH"));
        assert!(!body.contains("NEWEST_EXACT_REPORT"));
        assert!(!body.contains(&"x".repeat(80)));

        let full = load(root.path(), 16);
        assert!(!full.truncated);
        assert_eq!(
            full.handoffs
                .iter()
                .map(|item| item.job_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "job-mismatch",
                "job-oversize",
                "job-newest",
                "job-middle",
                "job-older"
            ]
        );
        let newest = full
            .handoffs
            .iter()
            .find(|item| item.job_id == "job-newest")
            .unwrap();
        assert_eq!(newest.report_availability, ReportAvailability::Available);
        assert_eq!(newest.report_text.as_deref(), Some("NEWEST_EXACT_REPORT_α"));
        assert_eq!(newest.state.as_deref(), Some("awaiting_ack"));
        assert_eq!(newest.ready_at_ms, Some(20));
        assert_eq!(newest.deadline_ms, Some(30));
        assert!(newest.receipt_at_ms.is_none());
        assert_eq!(newest.activities[0].tool_name.as_deref(), Some("read_file"));
        assert_eq!(newest.activities[0].status.as_deref(), Some("completed"));
        let value = serde_json::to_value(newest).unwrap();
        assert!(value.get("capabilityHash").is_none());
        assert!(value.get("processId").is_none());
        assert!(value.get("waitUntilMs").is_none());
    }

    #[test]
    fn missing_root_and_stale_snapshot_are_not_repaired() {
        let root = TempRoot::new("stale");
        let mut pending = record("job-stale", 80, "pending");
        pending.excerpt = "journal excerpt wins".to_string();
        write_journal(root.path(), &pending);
        let job = root.path().join("jobs").join("job-stale");
        fs::write(job.join("snapshot.json"), b"{\"stale\":true}\n").unwrap();
        let before = fs::read(job.join("snapshot.json")).unwrap();
        let snapshot = load(root.path(), 4);
        assert_eq!(
            snapshot.handoffs[0].excerpt_text.as_deref(),
            Some("journal excerpt wins")
        );
        assert_eq!(fs::read(job.join("snapshot.json")).unwrap(), before);

        let absent = load_root(
            &root.path().join("does-not-exist"),
            ConfiguredSourceKind::Environment,
            4,
            9,
        );
        assert_eq!(absent.unavailable, Some(EvidenceDiagnostic::Missing));
        assert!(absent.diagnostics.root_missing);
        assert!(absent.handoffs.is_empty());
        assert!(!encoded(&absent).contains("does-not-exist"));
    }

    #[test]
    fn pending_report_is_not_read_while_exclusively_locked() {
        let root = TempRoot::new("unread");
        let pending = record("job-locked-report", 60, "pending");
        write_journal(root.path(), &pending);
        let report = root
            .path()
            .join("jobs")
            .join("job-locked-report")
            .join("report.txt");
        fs::write(&report, b"DO_NOT_LEAK_LOCKED_PENDING").unwrap();
        let held = exclusive_read(&report);
        let snapshot = load(root.path(), 4);
        assert_eq!(
            snapshot.handoffs[0].report_availability,
            ReportAvailability::Absent
        );
        assert!(snapshot.handoffs[0].report_text.is_none());
        assert!(!encoded(&snapshot).contains("DO_NOT_LEAK_LOCKED_PENDING"));
        drop(held);
    }

    fn durable_report(root: &Path, job_id: &str, updated_at_ms: u64, report: &str) -> StoredRecord {
        let mut record = record(job_id, updated_at_ms, "awaiting_ack");
        record.report_fingerprint = Some(sha256_hex(report.as_bytes()));
        write_journal(root, &record);
        fs::write(
            root.join("jobs").join(job_id).join("report.txt"),
            report.as_bytes(),
        )
        .unwrap();
        record
    }

    fn exclusive_read(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .unwrap()
    }

    #[test]
    fn snapshot_json_uses_camel_case_and_no_stall_flag() {
        let root = TempRoot::new("camel");
        write_journal(root.path(), &record("job-camel", 5, "pending"));
        let value = serde_json::to_value(load(root.path(), 4)).unwrap();
        assert_eq!(value["assessment"], "not_inferred");
        assert_eq!(value["source"]["kind"], "environment");
        assert_eq!(value["generatedMs"], 7);
        assert!(value.get("generated_ms").is_none());
        assert!(value.get("stalled").is_none());
        assert!(value["handoffs"][0].get("processId").is_none());
        let _: Value = value;
    }
}
