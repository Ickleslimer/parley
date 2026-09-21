use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use parley_health::inbox;
use parley_health::paths::HealthPaths;
use parley_health::schema::{
    now_ms, sanitize_id, ClosedClass, CodexSampleView, GrokObservationView, HealthRecord,
    InboxKind, IncidentStatus, IncidentView, QueryDiagnostics, QueryDocument, Source, Unavailable,
    UnavailableReason,
};
use serde::Serialize;

use crate::event_engine::{EventContent, EventEngine, EventType, IdMatch};

const CONTROL_WAIT: Duration = Duration::from_millis(1_000);
const CONTROL_POLL: Duration = Duration::from_millis(25);
const EXACT_RESPONSE_LABEL: &str =
    "Exact preserved response that was not delivered over MCP stdout";
const QUOTA_CONTEXT_LABEL: &str =
    "Latest preceding Grok reply; latest context only, not proven undelivered output";
const NO_HANDOFF_LABEL: &str = "No peer-health handoff is available";
static CONTROL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerHealthSnapshot {
    schema_version: u32,
    generated_ms: u64,
    as_of_ms: Option<u64>,
    muted: bool,
    unread_count: u64,
    latest_codex_sample: Option<CodexSample>,
    latest_grok_observation: Option<GrokObservation>,
    active_incidents: Vec<PeerIncident>,
    recent_incidents: Vec<PeerIncident>,
    unavailable: Option<PeerHealthUnavailable>,
    stale: bool,
    diagnostics: PeerHealthDiagnostics,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexSample {
    used_percent: Option<f64>,
    resets_at: Option<String>,
    plan_type: Option<String>,
    rate_limit_reached_type: Option<String>,
    as_of_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrokObservation {
    class: ClosedClass,
    as_of_ms: u64,
    success: bool,
    http_status: Option<u16>,
    provider_code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerIncident {
    incident_id: String,
    class: ClosedClass,
    source: Source,
    status: IncidentStatus,
    opened_ms: u64,
    as_of_ms: u64,
    recovered_ms: Option<u64>,
    acknowledged: bool,
    session_id: Option<String>,
    event_id: Option<String>,
    exchange_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PeerHealthUnavailable {
    reason: UnavailableReason,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerHealthDiagnostics {
    snapshot_missing: bool,
    snapshot_malformed: bool,
    snapshot_locked: bool,
    journal_incomplete_trailing: bool,
    malformed_journal_lines: u64,
    oversized_journal_lines: u64,
    unsupported_journal_records: u64,
    quarantined_inbox: u64,
    malformed_inbox: u64,
    sound_failures: u64,
    footer_missing: u64,
    codex_sample_failures: u64,
    last_codex_sample_failure_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffSelection {
    pub event: Option<EventContent>,
    pub label: String,
    pub incident_id: Option<String>,
    pub exact_undelivered: bool,
    pub diagnostic: Option<String>,
}

impl From<QueryDocument> for PeerHealthSnapshot {
    fn from(document: QueryDocument) -> Self {
        Self {
            schema_version: document.schema_version,
            generated_ms: document.generated_ms,
            as_of_ms: document.as_of_ms,
            muted: document.muted,
            unread_count: document.unread_count,
            latest_codex_sample: document.latest_codex_sample.map(Into::into),
            latest_grok_observation: document.latest_grok_observation.map(Into::into),
            active_incidents: document
                .active_incidents
                .into_iter()
                .map(Into::into)
                .collect(),
            recent_incidents: document
                .recent_incidents
                .into_iter()
                .map(Into::into)
                .collect(),
            unavailable: document.unavailable.map(Into::into),
            stale: document.stale,
            diagnostics: document.diagnostics.into(),
        }
    }
}

impl From<CodexSampleView> for CodexSample {
    fn from(sample: CodexSampleView) -> Self {
        Self {
            used_percent: sample.used_percent,
            resets_at: sample.resets_at,
            plan_type: sample.plan_type,
            rate_limit_reached_type: sample.rate_limit_reached_type,
            as_of_ms: sample.as_of_ms,
        }
    }
}

impl From<GrokObservationView> for GrokObservation {
    fn from(observation: GrokObservationView) -> Self {
        Self {
            class: observation.class,
            as_of_ms: observation.as_of_ms,
            success: observation.success,
            http_status: observation.http_status,
            provider_code: observation.provider_code,
        }
    }
}

impl From<IncidentView> for PeerIncident {
    fn from(incident: IncidentView) -> Self {
        Self {
            incident_id: incident.incident_id,
            class: incident.class,
            source: incident.source,
            status: incident.status,
            opened_ms: incident.opened_ms,
            as_of_ms: incident.as_of_ms,
            recovered_ms: incident.recovered_ms,
            acknowledged: incident.acknowledged,
            session_id: incident.session_id,
            event_id: incident.event_id,
            exchange_id: incident.exchange_id,
        }
    }
}

impl From<Unavailable> for PeerHealthUnavailable {
    fn from(unavailable: Unavailable) -> Self {
        Self {
            reason: unavailable.reason,
        }
    }
}

impl From<QueryDiagnostics> for PeerHealthDiagnostics {
    fn from(diagnostics: QueryDiagnostics) -> Self {
        Self {
            snapshot_missing: diagnostics.snapshot_missing,
            snapshot_malformed: diagnostics.snapshot_malformed,
            snapshot_locked: diagnostics.snapshot_locked,
            journal_incomplete_trailing: diagnostics.journal_incomplete_trailing,
            malformed_journal_lines: diagnostics.malformed_journal_lines,
            oversized_journal_lines: diagnostics.oversized_journal_lines,
            unsupported_journal_records: diagnostics.unsupported_journal_records,
            quarantined_inbox: diagnostics.quarantined_inbox,
            malformed_inbox: diagnostics.malformed_inbox,
            sound_failures: diagnostics.sound_failures,
            footer_missing: diagnostics.footer_missing,
            codex_sample_failures: diagnostics.codex_sample_failures,
            last_codex_sample_failure_ms: diagnostics.last_codex_sample_failure_ms,
        }
    }
}

pub fn snapshot() -> PeerHealthSnapshot {
    read_document(&HealthPaths::from_env()).into()
}

pub fn acknowledge(incident_id: &str) -> Result<PeerHealthSnapshot, String> {
    let incident_id = sanitize_id(incident_id).ok_or_else(|| "invalid incident_id".to_string())?;
    let current_ms = now_ms();
    let mut record = viewer_record(InboxKind::Acknowledge, "ack", current_ms);
    record.incident_id = Some(incident_id.clone());
    submit_and_wait(record, move |document| {
        document
            .active_incidents
            .iter()
            .chain(document.recent_incidents.iter())
            .any(|incident| incident.incident_id == incident_id && incident.acknowledged)
    })
}

pub fn set_muted(muted: bool) -> Result<PeerHealthSnapshot, String> {
    let current_ms = now_ms();
    let mut record = viewer_record(InboxKind::Mute, "mute", current_ms);
    record.muted = Some(muted);
    submit_and_wait(record, move |document| document.muted == muted)
}

pub fn test_chime() -> Result<(), String> {
    let current_ms = now_ms();
    let mut record = viewer_record(InboxKind::TestSound, "test-sound", current_ms);
    record.test_sound = Some(true);
    inbox::write_record(&HealthPaths::from_env(), &record)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn open_latest_handoff(engine: &EventEngine) -> HandoffSelection {
    select_handoff(&read_document(&HealthPaths::from_env()), engine)
}

pub fn tray_state() -> (u64, bool) {
    let document = read_document(&HealthPaths::from_env());
    (document.unread_count, document.muted)
}

fn read_document(paths: &HealthPaths) -> QueryDocument {
    parley_health::snapshot::read_with_retry(paths)
}

fn viewer_record(kind: InboxKind, label: &str, current_ms: u64) -> HealthRecord {
    let sequence = CONTROL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut record = HealthRecord::new(
        kind,
        format!(
            "viewer-{label}-{current_ms}-{}-{sequence}",
            std::process::id()
        ),
        current_ms,
    );
    record.source = Some(Source::Viewer);
    record
}

fn submit_and_wait(
    record: HealthRecord,
    reflected: impl Fn(&QueryDocument) -> bool,
) -> Result<PeerHealthSnapshot, String> {
    let paths = HealthPaths::from_env();
    inbox::write_record(&paths, &record).map_err(|error| error.to_string())?;
    wait_for_reflection(
        || read_document(&paths),
        reflected,
        CONTROL_WAIT,
        CONTROL_POLL,
    )
}

fn wait_for_reflection(
    mut read: impl FnMut() -> QueryDocument,
    reflected: impl Fn(&QueryDocument) -> bool,
    wait: Duration,
    poll: Duration,
) -> Result<PeerHealthSnapshot, String> {
    let deadline = Instant::now() + wait;
    loop {
        let document = read();
        if reflected(&document) {
            return Ok(document.into());
        }
        if Instant::now() >= deadline {
            return Err("peer health control was not reflected before timeout".to_string());
        }
        thread::sleep(poll);
    }
}

fn select_handoff(document: &QueryDocument, engine: &EventEngine) -> HandoffSelection {
    let incident = document
        .active_incidents
        .iter()
        .chain(document.recent_incidents.iter())
        .filter(|incident| {
            matches!(
                incident.class,
                ClosedClass::McpStdoutUndelivered | ClosedClass::QuotaExhausted
            )
        })
        .max_by_key(|incident| (incident.as_of_ms, incident.opened_ms));
    let Some(incident) = incident else {
        return HandoffSelection {
            event: None,
            label: NO_HANDOFF_LABEL.to_string(),
            incident_id: None,
            exact_undelivered: false,
            diagnostic: None,
        };
    };
    match incident.class {
        ClosedClass::McpStdoutUndelivered => match match_undelivered_response(incident, engine) {
            Err(diagnostic) => ambiguous_handoff(incident.incident_id.clone(), diagnostic),
            Ok(Some(event)) => HandoffSelection {
                exact_undelivered: true,
                event: Some(event),
                label: EXACT_RESPONSE_LABEL.to_string(),
                incident_id: Some(incident.incident_id.clone()),
                diagnostic: None,
            },
            Ok(None) => HandoffSelection {
                exact_undelivered: false,
                event: None,
                label: EXACT_RESPONSE_LABEL.to_string(),
                incident_id: Some(incident.incident_id.clone()),
                diagnostic: None,
            },
        },
        ClosedClass::QuotaExhausted => {
            match engine.match_latest_grok_response_before(
                incident.as_of_ms,
                incident.session_id.as_deref(),
            ) {
                IdMatch::Ambiguous { diagnostic, .. } => {
                    ambiguous_handoff(incident.incident_id.clone(), diagnostic)
                }
                IdMatch::Unique(event) => HandoffSelection {
                    event: Some(event),
                    label: QUOTA_CONTEXT_LABEL.to_string(),
                    incident_id: Some(incident.incident_id.clone()),
                    exact_undelivered: false,
                    diagnostic: None,
                },
                IdMatch::None => HandoffSelection {
                    event: None,
                    label: QUOTA_CONTEXT_LABEL.to_string(),
                    incident_id: Some(incident.incident_id.clone()),
                    exact_undelivered: false,
                    diagnostic: None,
                },
            }
        }
        _ => unreachable!("handoff incidents are filtered to supported classes"),
    }
}

fn match_undelivered_response(
    incident: &IncidentView,
    engine: &EventEngine,
) -> Result<Option<EventContent>, String> {
    let event_id = incident
        .event_id
        .as_deref()
        .filter(|value| !value.is_empty());
    let exchange_id = incident
        .exchange_id
        .as_deref()
        .filter(|value| !value.is_empty());
    let session_id = incident
        .session_id
        .as_deref()
        .filter(|value| !value.is_empty());

    let event = match event_id {
        Some(raw_id) => match engine.match_event_id(raw_id) {
            IdMatch::Unique(event) if event.event_type == EventType::Response => Some(event),
            IdMatch::Unique(_) => {
                return Err(format!(
                    "handoff event id '{raw_id}' did not identify a response; no selection made"
                ))
            }
            IdMatch::Ambiguous { diagnostic, .. } => return Err(diagnostic),
            IdMatch::None => {
                return Err(format!(
                    "handoff event id '{raw_id}' was not found; no selection made"
                ))
            }
        },
        None => None,
    };
    let exchange = match exchange_id {
        Some(raw_id) => match engine.match_exchange_response(raw_id) {
            IdMatch::Unique(event) => Some(event),
            IdMatch::Ambiguous { diagnostic, .. } => return Err(diagnostic),
            IdMatch::None => {
                return Err(format!(
                    "handoff exchange id '{raw_id}' was not found; no selection made"
                ))
            }
        },
        None => None,
    };
    if let (Some(by_event), Some(by_exchange)) = (&event, &exchange) {
        if by_event.event_key != by_exchange.event_key {
            return Err(
                "handoff event and exchange identifiers resolved to different responses; no selection made"
                    .to_string(),
            );
        }
    }
    let candidate = event.or(exchange);
    if let Some(raw_id) = session_id {
        let session_key = match engine.match_session_id(raw_id) {
            IdMatch::Unique(session_key) => session_key,
            IdMatch::Ambiguous { diagnostic, .. } => return Err(diagnostic),
            IdMatch::None => {
                return Err(format!(
                    "handoff session id '{raw_id}' was not found; no selection made"
                ))
            }
        };
        if candidate
            .as_ref()
            .is_some_and(|event| event.session_key != session_key)
        {
            return Err(
                "handoff session identifier did not match the response; no selection made"
                    .to_string(),
            );
        }
    }
    Ok(candidate)
}

fn ambiguous_handoff(incident_id: String, diagnostic: String) -> HandoffSelection {
    HandoffSelection {
        event: None,
        label: diagnostic.clone(),
        incident_id: Some(incident_id),
        exact_undelivered: false,
        diagnostic: Some(diagnostic),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn response_line(
        event_id: &str,
        exchange_id: &str,
        session_id: &str,
        content: &str,
        timestamp_ms: u64,
    ) -> String {
        json!({
            "schema_version": 1,
            "event_type": "response",
            "event_id": event_id,
            "exchange_id": exchange_id,
            "timestamp_ms": timestamp_ms,
            "source": "codex",
            "target": "grok",
            "cwd": "C:\\worker",
            "session_id": session_id,
            "session_action": "resume",
            "content": content,
            "status": "ok",
            "duration_ms": 1,
            "error": null
        })
        .to_string()
    }

    fn incident(class: ClosedClass, as_of_ms: u64) -> IncidentView {
        IncidentView {
            incident_id: format!("incident-{}", class.as_str()),
            class,
            source: Source::Parley,
            status: IncidentStatus::Active,
            opened_ms: as_of_ms,
            as_of_ms,
            recovered_ms: None,
            acknowledged: false,
            session_id: Some("session-a".to_string()),
            event_id: None,
            exchange_id: None,
        }
    }

    fn document_with(incident: IncidentView) -> QueryDocument {
        let mut document = QueryDocument::unavailable(UnavailableReason::Missing, 50);
        document.unavailable = None;
        document.active_incidents = vec![incident];
        document.stale = false;
        document
    }

    #[test]
    fn snapshot_dto_uses_frontend_camel_case_fields() {
        let value = serde_json::to_value(PeerHealthSnapshot::from(QueryDocument::unavailable(
            UnavailableReason::Missing,
            7,
        )))
        .unwrap();
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["generatedMs"], 7);
        assert!(value.get("generated_ms").is_none());
        assert_eq!(value["diagnostics"]["snapshotMissing"], true);
    }

    #[test]
    fn viewer_control_ids_are_unique_and_sanitized() {
        let first = viewer_record(InboxKind::Mute, "mute", 10);
        let second = viewer_record(InboxKind::Mute, "mute", 10);
        assert_ne!(first.inbox_id, second.inbox_id);
        assert_eq!(sanitize_id(&first.inbox_id), Some(first.inbox_id));
    }

    #[test]
    fn viewer_control_wait_reports_timeout_instead_of_false_success() {
        let document = QueryDocument::unavailable(UnavailableReason::Missing, 7);
        let error = wait_for_reflection(
            || document.clone(),
            |_| false,
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap_err();
        assert!(error.contains("not reflected before timeout"));
    }

    #[test]
    fn handoff_selection_distinguishes_exact_undelivered_from_quota_context() {
        let path = std::env::temp_dir().join(format!(
            "parley-viewer-handoff-{}-{}.jsonl",
            std::process::id(),
            CONTROL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let body = [
            response_line("event-a", "exchange-a", "session-a", "exact reply", 10),
            response_line("event-b", "exchange-b", "session-b", "other session", 15),
            response_line("event-after", "exchange-after", "session-a", "too late", 30),
        ]
        .join("\n");
        std::fs::write(&path, format!("{body}\n")).unwrap();
        let engine = EventEngine::new();
        engine.set_source(Some(path.clone())).unwrap();
        engine.poll();

        let mut undelivered = incident(ClosedClass::McpStdoutUndelivered, 20);
        undelivered.event_id = Some("event-a".to_string());
        undelivered.exchange_id = Some("exchange-a".to_string());
        let exact = select_handoff(&document_with(undelivered), &engine);
        assert!(exact.exact_undelivered);
        assert_eq!(exact.event.unwrap().content, "exact reply");
        assert!(exact.label.contains("Exact preserved response"));

        let quota = select_handoff(
            &document_with(incident(ClosedClass::QuotaExhausted, 20)),
            &engine,
        );
        assert!(!quota.exact_undelivered);
        assert_eq!(quota.event.unwrap().event_id, "event-a");
        assert!(quota.label.contains("not proven undelivered"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn handoff_selection_fails_closed_on_cross_source_ambiguity_and_id_mismatch() {
        let suffix = CONTROL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let first = std::env::temp_dir().join(format!(
            "parley-viewer-handoff-a-{}-{suffix}.jsonl",
            std::process::id()
        ));
        let second = std::env::temp_dir().join(format!(
            "parley-viewer-handoff-b-{}-{suffix}.jsonl",
            std::process::id()
        ));
        let first_body = [
            response_line("event-a", "exchange-a", "shared-session", "reply-a", 10),
            response_line(
                "duplicate-event",
                "duplicate-exchange-a",
                "session-a",
                "duplicate-a",
                11,
            ),
        ]
        .join("\n");
        let second_body = [
            response_line("event-b", "exchange-b", "shared-session", "reply-b", 12),
            response_line(
                "duplicate-event",
                "duplicate-exchange-b",
                "session-b",
                "duplicate-b",
                13,
            ),
        ]
        .join("\n");
        std::fs::write(&first, format!("{first_body}\n")).unwrap();
        std::fs::write(&second, format!("{second_body}\n")).unwrap();
        let engine = EventEngine::new();
        engine
            .set_sources(vec![first.clone(), second.clone()])
            .unwrap();
        engine.poll();

        let mut duplicate = incident(ClosedClass::McpStdoutUndelivered, 20);
        duplicate.event_id = Some("duplicate-event".to_string());
        duplicate.exchange_id = None;
        duplicate.session_id = None;
        let selection = select_handoff(&document_with(duplicate), &engine);
        assert!(selection.event.is_none());
        assert!(selection.diagnostic.unwrap().contains("ambiguous event id"));

        let mut mismatch = incident(ClosedClass::McpStdoutUndelivered, 21);
        mismatch.event_id = Some("event-a".to_string());
        mismatch.exchange_id = Some("exchange-b".to_string());
        mismatch.session_id = None;
        let selection = select_handoff(&document_with(mismatch), &engine);
        assert!(selection.event.is_none());
        assert!(selection
            .diagnostic
            .unwrap()
            .contains("resolved to different responses"));

        let mut session_collision = incident(ClosedClass::McpStdoutUndelivered, 22);
        session_collision.event_id = Some("event-a".to_string());
        session_collision.exchange_id = Some("exchange-a".to_string());
        session_collision.session_id = Some("shared-session".to_string());
        let selection = select_handoff(&document_with(session_collision), &engine);
        assert!(selection.event.is_none());
        assert!(selection
            .diagnostic
            .unwrap()
            .contains("ambiguous session id"));

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }
}
