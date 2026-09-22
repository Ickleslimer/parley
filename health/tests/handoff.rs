use parley_health::inbox;
use parley_health::schema::{
    ClosedClass, HealthRecord, InboxKind, IncidentStatus, Source, SOUND_COOLDOWN_MS,
};
use parley_health::sound::{RecordingSound, SoundKind};
use parley_health::supervisor::Supervisor;

mod common;

fn with_recorder(home: &common::TempHome) -> (Supervisor, RecordingSound) {
    let recorder = RecordingSound::new();
    let mut supervisor = Supervisor::open(home.paths.clone()).unwrap();
    supervisor.set_sound(Box::new(recorder.clone()));
    (supervisor, recorder)
}

fn incident<'a>(supervisor: &'a Supervisor, id: &str) -> &'a parley_health::model::Incident {
    supervisor
        .model
        .incidents
        .iter()
        .find(|incident| incident.incident_id == id)
        .unwrap()
}

#[test]
fn explicit_alerts_create_distinct_incidents_and_only_one_initial_sound() {
    let home = common::TempHome::new("handoff-distinct");
    let (mut supervisor, recorder) = with_recorder(&home);
    supervisor
        .ingest(common::peer_alert("handoff-a", 10), 10)
        .unwrap();
    supervisor
        .ingest(common::peer_alert("handoff-b", 11), 11)
        .unwrap();

    assert_eq!(supervisor.model.incidents.len(), 2);
    assert_eq!(recorder.recorded(), vec![SoundKind::Incident]);
    assert_eq!(
        incident(&supervisor, "handoff-a").status,
        IncidentStatus::Active
    );
    assert_eq!(
        incident(&supervisor, "handoff-b").status,
        IncidentStatus::Active
    );

    supervisor.tick(10 + SOUND_COOLDOWN_MS + 1).unwrap();
    assert_eq!(
        recorder.recorded(),
        vec![SoundKind::Incident, SoundKind::Incident]
    );
}

#[test]
fn generic_observations_cannot_open_handoff_incidents() {
    let home = common::TempHome::new("handoff-explicit-only");
    let (mut supervisor, recorder) = with_recorder(&home);
    let mut record = HealthRecord::new(InboxKind::ParleyObservation, "generic".into(), 10);
    record.source = Some(Source::Parley);
    record.success = Some(false);
    record.class = Some(ClosedClass::HandoffUnacknowledged);
    record.incident_id = Some("handoff-generic".into());
    supervisor.ingest(record, 10).unwrap();

    assert!(supervisor.model.incidents.is_empty());
    assert!(recorder.recorded().is_empty());
}

#[test]
fn viewer_acknowledgement_does_not_release_handoff() {
    let home = common::TempHome::new("handoff-viewer-ack");
    let mut supervisor = common::supervisor(&home);
    supervisor
        .ingest(common::peer_alert("handoff-ack", 10), 10)
        .unwrap();
    supervisor
        .ingest(inbox::acknowledge_record("handoff-ack", 11).unwrap(), 11)
        .unwrap();

    assert_eq!(
        incident(&supervisor, "handoff-ack").status,
        IncidentStatus::Active
    );
    assert!(incident(&supervisor, "handoff-ack").class.is_audible());
    assert_eq!(supervisor.model.snapshot(11).unread_count, 0);
}

#[test]
fn only_matching_current_or_newer_codex_receipt_recovers() {
    let home = common::TempHome::new("handoff-receipt-boundary");
    let mut supervisor = common::supervisor(&home);
    supervisor
        .ingest(common::peer_alert("handoff-target", 100), 100)
        .unwrap();
    supervisor
        .ingest(common::handoff_receipt("handoff-other", 110), 110)
        .unwrap();
    supervisor
        .ingest(common::handoff_receipt("handoff-target", 99), 111)
        .unwrap();
    assert_eq!(
        incident(&supervisor, "handoff-target").status,
        IncidentStatus::Active
    );

    supervisor
        .ingest(common::handoff_receipt("handoff-target", 100), 112)
        .unwrap();
    assert_eq!(
        incident(&supervisor, "handoff-target").status,
        IncidentStatus::Recovered
    );
}

#[test]
fn receipt_before_alert_replays_as_recovered_without_sound() {
    let home = common::TempHome::new("handoff-receipt-first");
    {
        let mut supervisor = common::supervisor(&home);
        supervisor
            .ingest(common::handoff_receipt("handoff-first", 200), 200)
            .unwrap();
    }

    let (mut restarted, recorder) = with_recorder(&home);
    restarted
        .ingest(common::peer_alert("handoff-first", 199), 201)
        .unwrap();
    assert_eq!(
        incident(&restarted, "handoff-first").status,
        IncidentStatus::Recovered
    );
    assert!(recorder.recorded().is_empty());

    let replayed = common::supervisor(&home);
    assert_eq!(
        incident(&replayed, "handoff-first").status,
        IncidentStatus::Recovered
    );
}

#[test]
fn duplicate_alert_restart_mute_and_cooldown_preserve_sparse_sound() {
    let home = common::TempHome::new("handoff-sparse");
    let (mut supervisor, recorder) = with_recorder(&home);
    supervisor.ingest(inbox::mute_record(true, 1), 1).unwrap();
    let alert = common::peer_alert("handoff-muted", 2);
    supervisor.ingest(alert.clone(), 2).unwrap();
    supervisor.ingest(alert, 3).unwrap();
    assert!(recorder.recorded().is_empty());
    assert_eq!(supervisor.model.incidents.len(), 1);

    let (mut restarted, recorder) = with_recorder(&home);
    restarted.ingest(inbox::mute_record(false, 4), 4).unwrap();
    restarted.tick(4).unwrap();
    assert_eq!(recorder.recorded(), vec![SoundKind::Incident]);
    restarted.tick(4 + SOUND_COOLDOWN_MS + 1).unwrap();
    assert_eq!(recorder.recorded(), vec![SoundKind::Incident]);
}

#[test]
fn inbox_rejects_wrong_sources_and_unsupported_versions() {
    let home = common::TempHome::new("handoff-inbox-validation");
    let mut wrong_source = common::peer_alert("handoff-wrong-source", 1);
    wrong_source.source = Some(Source::Parley);
    common::put_inbox(&home, &wrong_source);

    let mut unsupported = common::peer_alert("handoff-v3", 2);
    unsupported.schema_version = 3;
    let unsupported_path = home.paths.inbox().join("unsupported.json");
    std::fs::write(&unsupported_path, serde_json::to_vec(&unsupported).unwrap()).unwrap();

    let mut supervisor = common::supervisor(&home);
    let outcome = supervisor.tick(3).unwrap();
    assert_eq!(outcome.quarantined, 2);
    assert!(supervisor.model.incidents.is_empty());
}
