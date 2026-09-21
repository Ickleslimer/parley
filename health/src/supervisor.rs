use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::inbox::{self, InboxReadError};
use crate::journal;
use crate::model::{codex_record, durable_state, ApplyOutcome, HealthModel};
use crate::paths::HealthPaths;
use crate::sampler::CodexSampler;
use crate::schema::{
    now_ms, HealthError, HealthRecord, INBOX_BATCH, POLL_MS, SCHEMA_VERSION,
};
use crate::scope::{ScopeFile, ScopeUpdate};
use crate::snapshot;
use crate::sound::{SilentSound, Sound, SoundKind};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

pub struct TickOutcome {
    pub consumed: u32,
    pub quarantined: u32,
    pub sounds: Vec<SoundKind>,
    pub opened: u32,
    pub recovered: u32,
}

pub struct Supervisor {
    pub paths: HealthPaths,
    pub model: HealthModel,
    sound: Box<dyn Sound>,
}

impl Supervisor {
    pub fn open(paths: HealthPaths) -> Result<Self, HealthError> {
        paths.ensure()?;
        let loaded = journal::load(&paths.journal())?;
        let mut model = HealthModel::new();
        model.diagnostics.journal_incomplete_trailing = loaded.diagnostics.journal_incomplete_trailing;
        model.diagnostics.malformed_journal_lines = loaded.diagnostics.malformed_journal_lines;
        model.diagnostics.oversized_journal_lines = loaded.diagnostics.oversized_journal_lines;
        model.diagnostics.unsupported_journal_records =
            loaded.diagnostics.unsupported_journal_records;
        for record in loaded.records {
            model.replay(record);
        }
        if let Ok(bytes) = crate::fsutil::read_bounded(&paths.state(), 256 * 1024) {
            if !bytes.clipped {
                if let Ok(state) = serde_json::from_slice::<crate::model::DurableState>(
                    crate::fsutil::strip_bom(&bytes.bytes),
                ) {
                    if state.schema_version == SCHEMA_VERSION {
                        model.muted = state.muted;
                        model.acks = state.acknowledgements;
                        model.sounded_incident_ids = state.sounded_incident_ids.into_iter().collect();
                        model.last_sound_ms = state.last_sound_ms.or(model.last_sound_ms);
                    }
                }
            }
        }
        let mut supervisor = Self {
            paths,
            model,
            sound: Box::new(SilentSound),
        };
        supervisor.persist(now_ms())?;
        Ok(supervisor)
    }

    pub fn set_sound(&mut self, sound: Box<dyn Sound>) {
        self.sound = sound;
    }

    pub fn refresh_scope(&mut self, update: ScopeUpdate) -> Result<(), HealthError> {
        let mut scope = ScopeFile::load_or_empty(&self.paths.scope());
        scope.apply_update(update);
        scope.save(&self.paths.scope())
    }

    pub fn poll_sampler(
        &mut self,
        sampler: &dyn CodexSampler,
        now_ms: u64,
    ) -> Result<TickOutcome, HealthError> {
        let mut outcome = TickOutcome::default();
        if let Some(sample) = sampler.sample().map_err(HealthError::msg)? {
            let record = codex_record(sample, format!("codex-sample-{now_ms}"));
            self.apply_live(record, now_ms, &mut outcome)?;
        }
        Ok(outcome)
    }

    pub fn ingest(&mut self, record: HealthRecord, now_ms: u64) -> Result<TickOutcome, HealthError> {
        let mut outcome = TickOutcome::default();
        self.apply_live(record, now_ms, &mut outcome)?;
        Ok(outcome)
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<TickOutcome, HealthError> {
        let mut outcome = TickOutcome::default();
        let files = inbox::list_ready(&self.paths, INBOX_BATCH)?;
        for path in files {
            match inbox::read_record(&path) {
                Ok(record) => {
                    self.apply_live(record, now_ms, &mut outcome)?;
                    inbox::remove_consumed(&path)?;
                    outcome.consumed += 1;
                }
                Err(InboxReadError::Malformed(_)) => {
                    let _ = inbox::quarantine(&self.paths, &path);
                    self.model.diagnostics.quarantined_inbox += 1;
                    self.model.diagnostics.malformed_inbox += 1;
                    outcome.quarantined += 1;
                }
                Err(InboxReadError::Io(_)) => {
                    outcome.quarantined += 1;
                }
            }
        }
        for (incident_id, kind) in self.model.pending_incident_sounds(now_ms) {
            let _ = self.sound.play(kind);
            outcome.sounds.push(kind);
            let mut mark = HealthRecord::new(
                crate::schema::InboxKind::Acknowledge,
                format!("sounded-{incident_id}-{now_ms}"),
                now_ms,
            );
            mark.sounded_incident_id = Some(incident_id);
            mark.last_sound_ms = Some(now_ms);
            mark.source = Some(crate::schema::Source::Parley);
            journal::append(&self.paths.journal(), &mark)?;
            self.model.processed_inbox_ids.insert(mark.inbox_id);
        }
        self.persist(now_ms)?;
        Ok(outcome)
    }

    fn apply_live(
        &mut self,
        record: HealthRecord,
        now_ms: u64,
        outcome: &mut TickOutcome,
    ) -> Result<(), HealthError> {
        let applied = self.model.apply(record, now_ms, false);
        self.note(applied, outcome)?;
        Ok(())
    }

    fn note(&mut self, applied: ApplyOutcome, outcome: &mut TickOutcome) -> Result<(), HealthError> {
        if applied.duplicate {
            return Ok(());
        }
        if applied.opened {
            outcome.opened += 1;
        }
        if applied.recovered {
            outcome.recovered += 1;
        }
        if let Some(kind) = applied.sound {
            let _ = self.sound.play(kind);
            outcome.sounds.push(kind);
        }
        if let Some(durable) = applied.durable {
            journal::append(&self.paths.journal(), &durable)?;
        }
        Ok(())
    }

    fn persist(&mut self, generated_ms: u64) -> Result<(), HealthError> {
        let document = self.model.snapshot(generated_ms);
        snapshot::write(&self.paths, &document)?;
        let state = durable_state(&self.model);
        let bytes = serde_json::to_vec_pretty(&state)?;
        crate::fsutil::atomic_write(&self.paths.state(), &bytes)
    }
}

impl Default for TickOutcome {
    fn default() -> Self {
        Self {
            consumed: 0,
            quarantined: 0,
            sounds: Vec::new(),
            opened: 0,
            recovered: 0,
        }
    }
}

pub fn run_forever() -> Result<(), HealthError> {
    let _guard = crate::instance::acquire(r"Local\ParleyHealthSupervisor")?;
    let paths = HealthPaths::from_env();
    let mut supervisor = Supervisor::open(paths)?;
    #[cfg(windows)]
    {
        supervisor.set_sound(Box::new(crate::sound::WindowsSound::from_install()));
        install_shutdown_handler()?;
    }
    while !shutdown_requested() {
        let _ = supervisor.tick(now_ms());
        wait_poll();
    }
    Ok(())
}

fn wait_poll() {
    let slices = (POLL_MS / 10).max(1);
    for _ in 0..slices {
        if shutdown_requested() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
fn install_shutdown_handler() -> Result<(), HealthError> {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    let ok = unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), 1) };
    if ok == 0 {
        Err(HealthError::msg("SetConsoleCtrlHandler failed"))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
unsafe extern "system" fn ctrl_handler(_ctrltype: u32) -> windows_sys::core::BOOL {
    request_shutdown();
    1
}
