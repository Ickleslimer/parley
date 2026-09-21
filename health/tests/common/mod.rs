use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use parley_health::inbox;
use parley_health::model::HealthModel;
use parley_health::paths::HealthPaths;
use parley_health::schema::{ClosedClass, HealthRecord, InboxKind, Source};
use parley_health::scope::{normalize_windows_path, ExecutableIdentity, ScopeFile};
use parley_health::sound::RecordingSound;
use parley_health::supervisor::Supervisor;

static SEQ: AtomicU64 = AtomicU64::new(1);

pub struct TempHome {
    pub paths: HealthPaths,
}

impl TempHome {
    pub fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "parley-health-{}-{}-{seq}-{label}",
            std::process::id(),
            nonce
        ));
        let paths = HealthPaths::from_root(root);
        paths.ensure().expect("health home");
        Self { paths }
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.paths.root);
    }
}

pub fn supervisor(home: &TempHome) -> Supervisor {
    Supervisor::open(home.paths.clone()).expect("open supervisor")
}

pub fn supervisor_with_sound(home: &TempHome, sound: RecordingSound) -> (Supervisor, RecordingSound) {
    let mut supervisor = Supervisor::open(home.paths.clone()).expect("open supervisor");
    // RecordingSound is moved; tests that need both should wrap in Arc. Use set after.
    supervisor.set_sound(Box::new(RecordingSound::new()));
    let _ = sound;
    (supervisor, RecordingSound::new())
}

pub fn scope_for(root: &str, query_path: &str) -> ScopeFile {
    let mut scope = ScopeFile::empty();
    scope.main_root = normalize_windows_path(root);
    scope.worktree_roots = vec![normalize_windows_path(root)];
    scope.executables.query = Some(ExecutableIdentity {
        path: normalize_windows_path(query_path),
        volume_serial: None,
        file_index: None,
    });
    scope
}

pub fn write_scope(home: &TempHome, scope: &ScopeFile) {
    scope.save(&home.paths.scope()).expect("write scope");
}

pub fn fake_query_exe(home: &TempHome) -> PathBuf {
    let path = home.paths.root.join("parley-health-query.exe");
    fs::write(&path, b"query").expect("fake query");
    path
}

pub fn codex_sample(inbox_id: &str, as_of_ms: u64, reached: Option<&str>) -> HealthRecord {
    let mut record = HealthRecord::new(InboxKind::CodexSample, inbox_id.to_string(), as_of_ms);
    record.source = Some(Source::Codex);
    record.used_percent = Some(40.0);
    record.resets_at = Some("2026-09-21T00:00:00Z".into());
    record.plan_type = Some("plus".into());
    record.rate_limit_reached_type = reached.map(|value| value.to_string());
    record
}

pub fn grok_stop(
    inbox_id: &str,
    as_of_ms: u64,
    status: Option<u16>,
    code: Option<&str>,
) -> HealthRecord {
    let mut record = HealthRecord::new(InboxKind::GrokStopFailure, inbox_id.to_string(), as_of_ms);
    record.source = Some(Source::Grok);
    record.http_status = status;
    record.provider_code = code.map(|value| value.to_string());
    record.generic_rate_limit = Some(false);
    record.clipped = Some(false);
    record.ambiguous = Some(false);
    record
}

pub fn parley_success(inbox_id: &str, as_of_ms: u64) -> HealthRecord {
    let mut record = HealthRecord::new(
        InboxKind::ParleyObservation,
        inbox_id.to_string(),
        as_of_ms,
    );
    record.source = Some(Source::Parley);
    record.success = Some(true);
    record
}

pub fn watchdog(inbox_id: &str, as_of_ms: u64) -> HealthRecord {
    let mut record = HealthRecord::new(
        InboxKind::ParleyObservation,
        inbox_id.to_string(),
        as_of_ms,
    );
    record.source = Some(Source::Parley);
    record.success = Some(false);
    record.class = Some(ClosedClass::WatchdogKilled);
    record
}

pub fn mcp_undelivered(inbox_id: &str, as_of_ms: u64) -> HealthRecord {
    let mut record = HealthRecord::new(
        InboxKind::ParleyObservation,
        inbox_id.to_string(),
        as_of_ms,
    );
    record.source = Some(Source::Parley);
    record.success = Some(false);
    record.class = Some(ClosedClass::McpStdoutUndelivered);
    record
}

pub fn put_inbox(home: &TempHome, record: &HealthRecord) {
    inbox::write_record(&home.paths, record).expect("inbox write");
}

pub fn active_classes(model: &HealthModel) -> Vec<ClosedClass> {
    model
        .incidents
        .iter()
        .filter(|incident| incident.status == parley_health::schema::IncidentStatus::Active)
        .map(|incident| incident.class)
        .collect()
}

pub fn write_bytes(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}
