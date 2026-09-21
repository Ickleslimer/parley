use parley_health::inbox;
use parley_health::schema::{ClosedClass, IncidentStatus, SOUND_COOLDOWN_MS};
use parley_health::sound::{RecordingSound, SoundKind};
use parley_health::supervisor::Supervisor;

mod common;

fn with_recorder(home: &common::TempHome) -> (Supervisor, RecordingSound) {
    let recorder = RecordingSound::new();
    let mut supervisor = Supervisor::open(home.paths.clone()).unwrap();
    supervisor.set_sound(Box::new(recorder.clone()));
    (supervisor, recorder)
}

#[test]
fn duplicate_inbox_and_restart_do_not_duplicate_incidents_or_sound() {
    let home = common::TempHome::new("dedup");
    let (mut supervisor, recorder) = with_recorder(&home);
    let record = common::codex_sample("dup-1", 100, Some("primary"));
    supervisor.ingest(record.clone(), 100).unwrap();
    supervisor.ingest(record, 101).unwrap();
    assert_eq!(supervisor.model.incidents.len(), 1);
    assert_eq!(recorder.recorded(), vec![SoundKind::Incident]);

    let (mut restarted, recorder2) = with_recorder(&home);
    common::put_inbox(&home, &common::codex_sample("dup-1", 100, Some("primary")));
    restarted.tick(200).unwrap();
    assert_eq!(restarted.model.incidents.len(), 1);
    assert!(recorder2.recorded().is_empty());
}

#[test]
fn recovery_requires_newer_authoritative_evidence_not_silence() {
    let home = common::TempHome::new("recovery");
    let mut supervisor = common::supervisor(&home);
    supervisor
        .ingest(common::codex_sample("ex-1", 10, Some("primary")), 10)
        .unwrap();
    assert_eq!(supervisor.model.incidents[0].status, IncidentStatus::Active);

    supervisor.tick(10 + SOUND_COOLDOWN_MS * 4).unwrap();
    assert_eq!(supervisor.model.incidents[0].status, IncidentStatus::Active);

    supervisor
        .ingest(common::codex_sample("ok-1", 11, None), 11)
        .unwrap();
    assert_eq!(
        supervisor.model.incidents[0].status,
        IncidentStatus::Recovered
    );

    supervisor
        .ingest(
            common::grok_stop(
                "g-ex",
                20,
                Some(429),
                Some("subscription:free-usage-exhausted"),
            ),
            20,
        )
        .unwrap();
    assert!(supervisor
        .model
        .incidents
        .iter()
        .any(|i| i.class == ClosedClass::QuotaExhausted
            && i.source == parley_health::schema::Source::Grok
            && i.status == IncidentStatus::Active));

    supervisor
        .ingest(common::codex_sample("ok-2", 21, None), 21)
        .unwrap();
    assert!(supervisor
        .model
        .incidents
        .iter()
        .any(|i| i.source == parley_health::schema::Source::Grok
            && i.status == IncidentStatus::Active));

    supervisor
        .ingest(common::parley_success("p-ok", 22), 22)
        .unwrap();
    assert!(supervisor
        .model
        .incidents
        .iter()
        .all(|i| i.source != parley_health::schema::Source::Grok
            || i.status == IncidentStatus::Recovered));
}

#[test]
fn acknowledgement_suppresses_resound_only() {
    let home = common::TempHome::new("ack");
    let (mut supervisor, recorder) = with_recorder(&home);
    supervisor
        .ingest(common::mcp_undelivered("mcp-1", 5), 5)
        .unwrap();
    assert_eq!(recorder.recorded(), vec![SoundKind::Incident]);
    let id = supervisor.model.incidents[0].incident_id.clone();
    assert_eq!(supervisor.model.incidents[0].status, IncidentStatus::Active);

    supervisor
        .ingest(inbox::acknowledge_record(&id, 6).unwrap(), 6)
        .unwrap();
    assert_eq!(supervisor.model.incidents[0].status, IncidentStatus::Active);
    assert_eq!(supervisor.model.snapshot(6).unread_count, 0);

    supervisor
        .ingest(common::mcp_undelivered("mcp-1b", 7), 7)
        .unwrap();
    assert_eq!(recorder.recorded(), vec![SoundKind::Incident]);
}

#[test]
fn mute_suppresses_incident_sound_but_test_sound_still_plays() {
    let home = common::TempHome::new("mute");
    let (mut supervisor, recorder) = with_recorder(&home);
    supervisor.ingest(inbox::mute_record(true, 1), 1).unwrap();
    supervisor
        .ingest(common::codex_sample("m-ex", 2, Some("primary")), 2)
        .unwrap();
    assert!(recorder.recorded().is_empty());
    assert_eq!(supervisor.model.incidents[0].status, IncidentStatus::Active);

    supervisor.ingest(inbox::test_sound_record(3), 3).unwrap();
    assert_eq!(recorder.recorded(), vec![SoundKind::Test]);
    assert_eq!(supervisor.model.incidents.len(), 1);

    supervisor.ingest(inbox::mute_record(false, 4), 4).unwrap();
    supervisor
        .ingest(common::mcp_undelivered("m-after-test", 5), 5)
        .unwrap();
    assert_eq!(
        recorder.recorded(),
        vec![SoundKind::Test, SoundKind::Incident]
    );
}

#[test]
fn successful_grok_turn_does_not_recover_terminal_transport_incidents() {
    let home = common::TempHome::new("terminal-transport");
    let mut supervisor = common::supervisor(&home);
    let mut undelivered = common::mcp_undelivered("mcp-terminal", 10);
    undelivered.event_id = Some("event-10".into());
    undelivered.exchange_id = Some("exchange-10".into());
    supervisor.ingest(undelivered, 10).unwrap();
    supervisor
        .ingest(common::parley_success("grok-success", 11), 11)
        .unwrap();

    let incident = supervisor
        .model
        .incidents
        .iter()
        .find(|incident| incident.class == ClosedClass::McpStdoutUndelivered)
        .unwrap();
    assert_eq!(incident.status, IncidentStatus::Active);
    assert_eq!(incident.event_id.as_deref(), Some("event-10"));
    assert_eq!(incident.exchange_id.as_deref(), Some("exchange-10"));
}

#[test]
fn watchdog_is_silent_and_cooldown_blocks_second_incident_sound() {
    let home = common::TempHome::new("cooldown");
    let (mut supervisor, recorder) = with_recorder(&home);
    supervisor.ingest(common::watchdog("wd-1", 1), 1).unwrap();
    assert!(recorder.recorded().is_empty());
    assert_eq!(
        supervisor.model.incidents[0].class,
        ClosedClass::WatchdogKilled
    );

    supervisor
        .ingest(common::codex_sample("q1", 2, Some("primary")), 2)
        .unwrap();
    supervisor
        .ingest(common::mcp_undelivered("m1", 3), 3)
        .unwrap();
    assert_eq!(recorder.recorded(), vec![SoundKind::Incident]);

    supervisor.tick(2 + SOUND_COOLDOWN_MS + 1).unwrap();
    assert_eq!(
        recorder.recorded(),
        vec![SoundKind::Incident, SoundKind::Incident]
    );
}
