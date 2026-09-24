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

fn alert<'a>(supervisor: &'a Supervisor, id: &str) -> &'a parley_health::model::HandoffAlert {
    supervisor
        .model
        .handoff_alerts
        .iter()
        .find(|alert| alert.alert_id == id)
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

    assert!(supervisor.model.incidents.is_empty());
    assert_eq!(supervisor.model.handoff_alerts.len(), 2);
    assert_eq!(recorder.recorded(), vec![SoundKind::Incident]);
    assert_eq!(
        alert(&supervisor, "handoff-a").status,
        IncidentStatus::Active
    );
    assert_eq!(
        alert(&supervisor, "handoff-b").status,
        IncidentStatus::Active
    );
    let snapshot = supervisor.model.snapshot(11);
    assert_eq!(snapshot.unread_count, 0);
    assert!(snapshot.active_incidents.is_empty());
    assert!(snapshot.recent_incidents.is_empty());

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
    assert!(supervisor.model.handoff_alerts.is_empty());
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

    assert!(supervisor.model.incidents.is_empty());
    assert_eq!(
        alert(&supervisor, "handoff-ack").status,
        IncidentStatus::Active
    );
    let before = supervisor.model.snapshot(11);
    assert_eq!(before.unread_count, 0);
    assert!(before.active_incidents.is_empty());
    assert!(before.recent_incidents.is_empty());
    assert!(!ClosedClass::HandoffUnacknowledged.is_incident());
    assert!(!ClosedClass::HandoffUnacknowledged.is_audible());
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
        alert(&supervisor, "handoff-target").status,
        IncidentStatus::Active
    );

    supervisor
        .ingest(common::handoff_receipt("handoff-target", 100), 112)
        .unwrap();
    assert_eq!(
        alert(&supervisor, "handoff-target").status,
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
        alert(&restarted, "handoff-first").status,
        IncidentStatus::Recovered
    );
    assert!(recorder.recorded().is_empty());

    let replayed = common::supervisor(&home);
    assert_eq!(
        alert(&replayed, "handoff-first").status,
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
    assert!(supervisor.model.incidents.is_empty());
    assert_eq!(supervisor.model.handoff_alerts.len(), 1);

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

    let mut supported = common::peer_alert("handoff-v2", 3);
    supported.schema_version = 2;
    std::fs::write(
        home.paths.inbox().join("supported-v2.json"),
        serde_json::to_vec(&supported).unwrap(),
    )
    .unwrap();

    let mut unsupported = common::peer_alert("handoff-v4", 4);
    unsupported.schema_version = 4;
    let unsupported_path = home.paths.inbox().join("unsupported.json");
    std::fs::write(&unsupported_path, serde_json::to_vec(&unsupported).unwrap()).unwrap();

    let mut supervisor = common::supervisor(&home);
    let outcome = supervisor.tick(5).unwrap();
    assert_eq!(outcome.quarantined, 2);
    assert_eq!(outcome.consumed, 1);
    assert!(supervisor.model.incidents.is_empty());
    assert_eq!(
        alert(&supervisor, "handoff-v2").status,
        IncidentStatus::Active
    );
}

#[test]
fn legacy_v2_replay_keeps_diagnostics_and_alerts_out_of_incidents_without_rewriting_journal() {
    let home = common::TempHome::new("handoff-v2-replay");
    let mut throttle = common::grok_stop("throttle", 10, Some(503), None);
    throttle.schema_version = 2;
    let mut turn = common::grok_stop("turn", 11, None, None);
    turn.schema_version = 2;
    let mut watched = common::watchdog("wd", 12);
    watched.schema_version = 2;
    let mut quota = common::codex_sample("quota", 13, Some("primary"));
    quota.schema_version = 2;
    quota.sounded_incident_id = Some(format!(
        "quota_exhausted:codex:{:x}",
        parley_health::schema::fnv1a_64(
            format!("{}:{}", quota.as_of_ms, quota.inbox_id).as_bytes()
        )
    ));
    let mut legacy_alert = common::peer_alert("handoff-legacy", 14);
    legacy_alert.schema_version = 2;
    legacy_alert.sounded_incident_id = Some("handoff-legacy".into());
    let body = format!(
        "{}\n",
        [throttle, turn, watched, quota, legacy_alert]
            .into_iter()
            .map(|record| serde_json::to_string(&record).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    );
    std::fs::write(home.paths.journal(), &body).unwrap();

    let (mut supervisor, recorder) = with_recorder(&home);
    assert_eq!(
        std::fs::read(home.paths.journal()).unwrap(),
        body.as_bytes()
    );
    assert_eq!(supervisor.model.incidents.len(), 1);
    assert_eq!(
        supervisor.model.incidents[0].class,
        ClosedClass::QuotaExhausted
    );
    assert_eq!(
        alert(&supervisor, "handoff-legacy").status,
        IncidentStatus::Active
    );
    let snapshot = supervisor.model.snapshot(20);
    assert_eq!(snapshot.unread_count, 1);
    assert_eq!(snapshot.active_incidents.len(), 1);
    assert!(snapshot
        .recent_incidents
        .iter()
        .all(|incident| incident.class.is_incident()));
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("handoff_unacknowledged"));
    assert!(!json.contains("watchdog_killed"));
    assert!(!json.contains("capacity_throttle"));
    assert_eq!(
        supervisor.model.latest_grok.as_ref().map(|obs| obs.class),
        Some(ClosedClass::TurnError)
    );

    supervisor.tick(20 + SOUND_COOLDOWN_MS).unwrap();
    assert!(recorder.recorded().is_empty());
    assert_eq!(
        std::fs::read(home.paths.journal()).unwrap(),
        body.as_bytes()
    );
}
