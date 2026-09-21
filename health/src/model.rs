use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use crate::classifier::{
    classify_codex, classify_grok, classify_parley, sanitize_codex, CodexUsageSample,
    GrokErrorEvidence,
};
use crate::schema::{
    bound_string, nonempty_reached_type, sanitize_percent, ClosedClass, CodexSampleView,
    GrokObservationView, HealthRecord, InboxKind, IncidentStatus, IncidentView, QueryDiagnostics,
    QueryDocument, Source, MAX_ACTIVE_INCIDENTS, MAX_PLAN_LEN, MAX_RECENT_INCIDENTS,
    SCHEMA_VERSION, SOUND_COOLDOWN_MS,
};
use crate::sound::SoundKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incident {
    pub incident_id: String,
    pub class: ClosedClass,
    pub source: Source,
    pub status: IncidentStatus,
    pub opened_ms: u64,
    pub as_of_ms: u64,
    pub recovered_ms: Option<u64>,
    pub session_id: Option<String>,
    pub event_id: Option<String>,
    pub exchange_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct HealthModel {
    pub incidents: Vec<Incident>,
    pub processed_inbox_ids: HashSet<String>,
    pub muted: bool,
    pub acks: HashMap<String, u64>,
    pub sounded_incident_ids: HashSet<String>,
    pub last_sound_ms: Option<u64>,
    pub latest_codex: Option<CodexSampleView>,
    pub latest_grok: Option<GrokObservationView>,
    pub diagnostics: QueryDiagnostics,
}

#[derive(Clone, Debug, Default)]
pub struct ApplyOutcome {
    pub duplicate: bool,
    pub durable: Option<HealthRecord>,
    pub sound: Option<SoundKind>,
    pub opened: bool,
    pub recovered: bool,
}

impl HealthModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replay(&mut self, record: HealthRecord) {
        let replay_ms = record.recorded_ms.unwrap_or(record.as_of_ms);
        let _ = self.apply(record, replay_ms, true);
    }

    pub fn apply(&mut self, mut record: HealthRecord, now_ms: u64, replay: bool) -> ApplyOutcome {
        if !self.processed_inbox_ids.insert(record.inbox_id.clone()) {
            return ApplyOutcome {
                duplicate: true,
                ..ApplyOutcome::default()
            };
        }
        record.schema_version = SCHEMA_VERSION;
        record.recorded_ms = Some(now_ms);
        let mut outcome = match record.kind {
            InboxKind::CodexSample => self.apply_codex(&mut record, now_ms),
            InboxKind::GrokStopFailure => self.apply_grok(&mut record, now_ms),
            InboxKind::ParleyObservation => self.apply_parley(&mut record, now_ms),
            InboxKind::Acknowledge => self.apply_ack(&mut record, now_ms),
            InboxKind::Mute => self.apply_mute(&mut record),
            InboxKind::TestSound => self.apply_test_sound(&mut record, now_ms, replay),
        };
        if !replay {
            if let Some(sound) = outcome.sound {
                match sound {
                    SoundKind::Incident => {
                        if let Some(id) = record.incident_id.clone() {
                            self.sounded_incident_ids.insert(id.clone());
                            record.sounded_incident_id = Some(id);
                        }
                        self.last_sound_ms = Some(now_ms);
                        record.last_sound_ms = Some(now_ms);
                    }
                    SoundKind::Test => {
                        record.test_sound = Some(true);
                    }
                }
            }
        } else {
            outcome.sound = None;
            if let Some(id) = record.sounded_incident_id.clone() {
                self.sounded_incident_ids.insert(id);
            }
            if let Some(last) = record.last_sound_ms {
                self.last_sound_ms = Some(self.last_sound_ms.unwrap_or(0).max(last));
            }
        }
        outcome.durable = Some(record);
        outcome
    }

    pub fn pending_incident_sounds(&mut self, now_ms: u64) -> Vec<(String, SoundKind)> {
        let mut plays = Vec::new();
        let ids: Vec<(String, ClosedClass)> = self
            .incidents
            .iter()
            .filter(|incident| incident.status == IncidentStatus::Active)
            .filter(|incident| incident.class.is_audible())
            .map(|incident| (incident.incident_id.clone(), incident.class))
            .collect();
        for (id, class) in ids {
            if let Some(kind) = self.consider_sound(&id, class, now_ms, false) {
                self.sounded_incident_ids.insert(id.clone());
                self.last_sound_ms = Some(now_ms);
                plays.push((id, kind));
            }
        }
        plays
    }

    pub fn snapshot(&self, generated_ms: u64) -> QueryDocument {
        let mut active: Vec<IncidentView> = self
            .incidents
            .iter()
            .filter(|incident| incident.status == IncidentStatus::Active)
            .map(|incident| self.view(incident))
            .collect();
        active.sort_by_key(|incident| Reverse(incident.as_of_ms));
        active.truncate(MAX_ACTIVE_INCIDENTS);

        let mut recent: Vec<IncidentView> = self
            .incidents
            .iter()
            .map(|incident| self.view(incident))
            .collect();
        recent.sort_by_key(|incident| Reverse(incident.opened_ms));
        recent.truncate(MAX_RECENT_INCIDENTS);

        let unread_count = self
            .incidents
            .iter()
            .filter(|incident| {
                incident.status == IncidentStatus::Active
                    && !self.acks.contains_key(&incident.incident_id)
            })
            .count() as u64;

        let as_of_ms = [
            self.latest_codex.as_ref().map(|sample| sample.as_of_ms),
            self.latest_grok.as_ref().map(|obs| obs.as_of_ms),
            self.incidents
                .iter()
                .map(|incident| incident.as_of_ms)
                .max(),
        ]
        .into_iter()
        .flatten()
        .max();

        QueryDocument {
            schema_version: SCHEMA_VERSION,
            generated_ms,
            as_of_ms,
            muted: self.muted,
            unread_count,
            latest_codex_sample: self.latest_codex.clone(),
            latest_grok_observation: self.latest_grok.clone(),
            active_incidents: active,
            recent_incidents: recent,
            unavailable: None,
            stale: false,
            diagnostics: self.diagnostics.clone(),
        }
    }

    fn view(&self, incident: &Incident) -> IncidentView {
        IncidentView {
            incident_id: incident.incident_id.clone(),
            class: incident.class,
            source: incident.source,
            status: incident.status,
            opened_ms: incident.opened_ms,
            as_of_ms: incident.as_of_ms,
            recovered_ms: incident.recovered_ms,
            acknowledged: self.acks.contains_key(&incident.incident_id),
            session_id: incident.session_id.clone(),
            event_id: incident.event_id.clone(),
            exchange_id: incident.exchange_id.clone(),
        }
    }

    fn apply_codex(&mut self, record: &mut HealthRecord, now_ms: u64) -> ApplyOutcome {
        let sample = sanitize_codex(CodexUsageSample {
            used_percent: record.used_percent,
            resets_at: record.resets_at.clone(),
            plan_type: record.plan_type.clone(),
            rate_limit_reached_type: record.rate_limit_reached_type.clone(),
            as_of_ms: record.as_of_ms,
        });
        record.source = Some(Source::Codex);
        record.used_percent = sample.used_percent;
        record.resets_at = sample.resets_at.clone();
        record.plan_type = sample.plan_type.clone();
        record.rate_limit_reached_type = sample.rate_limit_reached_type.clone();
        let classified = classify_codex(&sample);
        record.class = Some(classified.class);
        self.latest_codex = Some(CodexSampleView {
            used_percent: sample.used_percent,
            resets_at: sample.resets_at.clone(),
            plan_type: sample.plan_type.clone(),
            rate_limit_reached_type: sample.rate_limit_reached_type.clone(),
            as_of_ms: sample.as_of_ms,
        });
        let mut outcome = ApplyOutcome::default();
        if classified.class == ClosedClass::QuotaExhausted {
            outcome.merge(self.open_or_attach(classified.class, Source::Codex, record, now_ms));
        } else {
            outcome.merge(self.recover_codex(sample.as_of_ms, now_ms, record));
        }
        outcome
    }

    fn apply_grok(&mut self, record: &mut HealthRecord, now_ms: u64) -> ApplyOutcome {
        let evidence = GrokErrorEvidence {
            http_status: record.http_status,
            provider_code: record.provider_code.clone(),
            generic_rate_limit: record.generic_rate_limit.unwrap_or(false),
            clipped: record.clipped.unwrap_or(false),
            ambiguous: record.ambiguous.unwrap_or(false),
        };
        let classified = classify_grok(&evidence);
        record.source = Some(Source::Grok);
        record.class = Some(classified.class);
        record.provider_code = evidence.provider_code.clone();
        self.latest_grok = Some(GrokObservationView {
            class: classified.class,
            as_of_ms: record.as_of_ms,
            success: false,
            http_status: evidence.http_status,
            provider_code: evidence.provider_code,
        });
        let mut outcome = ApplyOutcome::default();
        if classified.class.is_incident() {
            outcome.merge(self.open_or_attach(classified.class, Source::Grok, record, now_ms));
        }
        outcome
    }

    fn apply_parley(&mut self, record: &mut HealthRecord, now_ms: u64) -> ApplyOutcome {
        let success = record.success.unwrap_or(false);
        let classified = classify_parley(success, record.class);
        record.source = record.source.or(Some(Source::Parley));
        record.class = Some(classified.class);
        record.success = Some(success);
        let mut outcome = ApplyOutcome::default();
        if success {
            outcome.merge(self.recover_successful(record.as_of_ms, now_ms, record));
        } else if classified.class.is_incident() {
            outcome.merge(self.open_or_attach(
                classified.class,
                record.source.unwrap_or(Source::Parley),
                record,
                now_ms,
            ));
        }
        outcome
    }

    fn apply_ack(&mut self, record: &mut HealthRecord, now_ms: u64) -> ApplyOutcome {
        record.source = record.source.or(Some(Source::Viewer));
        if let Some(id) = record.incident_id.clone() {
            self.acks.insert(id, now_ms);
        }
        ApplyOutcome::default()
    }

    fn apply_mute(&mut self, record: &mut HealthRecord) -> ApplyOutcome {
        record.source = record.source.or(Some(Source::Viewer));
        self.muted = record.muted.unwrap_or(true);
        record.muted = Some(self.muted);
        ApplyOutcome::default()
    }

    fn apply_test_sound(
        &mut self,
        record: &mut HealthRecord,
        now_ms: u64,
        replay: bool,
    ) -> ApplyOutcome {
        record.source = record.source.or(Some(Source::Viewer));
        record.test_sound = Some(true);
        record.class = None;
        let mut outcome = ApplyOutcome::default();
        if !replay {
            outcome.sound = Some(SoundKind::Test);
            record.last_sound_ms = Some(now_ms);
        }
        outcome
    }

    fn open_or_attach(
        &mut self,
        class: ClosedClass,
        source: Source,
        record: &mut HealthRecord,
        now_ms: u64,
    ) -> ApplyOutcome {
        let attach_existing = !matches!(
            class,
            ClosedClass::WatchdogKilled | ClosedClass::McpStdoutUndelivered
        );
        if let Some(existing) = self.incidents.iter_mut().find(|incident| {
            attach_existing
                && incident.status == IncidentStatus::Active
                && incident.class == class
                && incident.source == source
        }) {
            existing.as_of_ms = existing.as_of_ms.max(record.as_of_ms);
            if record.session_id.is_some() {
                existing.session_id = record.session_id.clone();
            }
            if record.event_id.is_some() {
                existing.event_id = record.event_id.clone();
            }
            if record.exchange_id.is_some() {
                existing.exchange_id = record.exchange_id.clone();
            }
            record.incident_id = Some(existing.incident_id.clone());
            let id = existing.incident_id.clone();
            return ApplyOutcome {
                sound: self.consider_sound(&id, class, now_ms, false),
                ..ApplyOutcome::default()
            };
        }
        let incident_id = format!(
            "{}:{}:{:x}",
            class.as_str(),
            source.as_str(),
            crate::schema::fnv1a_64(format!("{}:{}", record.as_of_ms, record.inbox_id).as_bytes())
        );
        self.incidents.push(Incident {
            incident_id: incident_id.clone(),
            class,
            source,
            status: IncidentStatus::Active,
            opened_ms: now_ms,
            as_of_ms: record.as_of_ms,
            recovered_ms: None,
            session_id: record.session_id.clone(),
            event_id: record.event_id.clone(),
            exchange_id: record.exchange_id.clone(),
        });
        record.incident_id = Some(incident_id.clone());
        let mut outcome = ApplyOutcome {
            opened: true,
            ..ApplyOutcome::default()
        };
        outcome.sound = self.consider_sound(&incident_id, class, now_ms, false);
        outcome
    }

    fn recover_codex(
        &mut self,
        as_of_ms: u64,
        now_ms: u64,
        record: &mut HealthRecord,
    ) -> ApplyOutcome {
        let mut recovered_id = None;
        for incident in &mut self.incidents {
            if incident.status == IncidentStatus::Active
                && incident.source == Source::Codex
                && incident.class == ClosedClass::QuotaExhausted
                && as_of_ms > incident.as_of_ms
            {
                incident.status = IncidentStatus::Recovered;
                incident.recovered_ms = Some(now_ms);
                recovered_id = Some(incident.incident_id.clone());
            }
        }
        if let Some(id) = recovered_id {
            record.recovered_incident_id = Some(id);
            ApplyOutcome {
                recovered: true,
                ..ApplyOutcome::default()
            }
        } else {
            ApplyOutcome::default()
        }
    }

    fn recover_successful(
        &mut self,
        as_of_ms: u64,
        now_ms: u64,
        record: &mut HealthRecord,
    ) -> ApplyOutcome {
        let mut recovered_id = None;
        for incident in &mut self.incidents {
            if incident.status == IncidentStatus::Active
                && incident.source == Source::Grok
                && matches!(
                    incident.class,
                    ClosedClass::QuotaExhausted
                        | ClosedClass::CapacityThrottle
                        | ClosedClass::TurnError
                )
                && as_of_ms > incident.as_of_ms
            {
                incident.status = IncidentStatus::Recovered;
                incident.recovered_ms = Some(now_ms);
                recovered_id = Some(incident.incident_id.clone());
            }
        }
        if record.source == Some(Source::Grok) {
            self.latest_grok = Some(GrokObservationView {
                class: ClosedClass::UsageSample,
                as_of_ms,
                success: true,
                http_status: None,
                provider_code: None,
            });
        }
        if let Some(id) = recovered_id {
            record.recovered_incident_id = Some(id);
            ApplyOutcome {
                recovered: true,
                ..ApplyOutcome::default()
            }
        } else {
            ApplyOutcome::default()
        }
    }

    fn consider_sound(
        &self,
        incident_id: &str,
        class: ClosedClass,
        now_ms: u64,
        is_test: bool,
    ) -> Option<SoundKind> {
        if is_test {
            return Some(SoundKind::Test);
        }
        if !class.is_audible() {
            return None;
        }
        if self.muted {
            return None;
        }
        if self.acks.contains_key(incident_id) {
            return None;
        }
        if self.sounded_incident_ids.contains(incident_id) {
            return None;
        }
        if let Some(last) = self.last_sound_ms {
            if now_ms.saturating_sub(last) < SOUND_COOLDOWN_MS {
                return None;
            }
        }
        Some(SoundKind::Incident)
    }
}

impl ApplyOutcome {
    fn merge(&mut self, other: ApplyOutcome) {
        self.opened |= other.opened;
        self.recovered |= other.recovered;
        if self.sound.is_none() {
            self.sound = other.sound;
        }
    }
}

pub fn durable_state(model: &HealthModel) -> DurableState {
    DurableState {
        schema_version: SCHEMA_VERSION,
        muted: model.muted,
        acknowledgements: model.acks.clone(),
        sounded_incident_ids: model.sounded_incident_ids.iter().cloned().collect(),
        last_sound_ms: model.last_sound_ms,
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DurableState {
    pub schema_version: u32,
    pub muted: bool,
    #[serde(default)]
    pub acknowledgements: HashMap<String, u64>,
    #[serde(default)]
    pub sounded_incident_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sound_ms: Option<u64>,
}

pub fn codex_record(sample: CodexUsageSample, inbox_id: String) -> HealthRecord {
    let sample = sanitize_codex(sample);
    let mut record = HealthRecord::new(InboxKind::CodexSample, inbox_id, sample.as_of_ms);
    record.source = Some(Source::Codex);
    record.used_percent = sanitize_percent(sample.used_percent);
    record.resets_at = bound_string(sample.resets_at, MAX_PLAN_LEN);
    record.plan_type = bound_string(sample.plan_type, MAX_PLAN_LEN);
    record.rate_limit_reached_type =
        nonempty_reached_type(sample.rate_limit_reached_type.as_deref());
    record
}
