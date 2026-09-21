mod common;

use parley_health::classifier::CodexUsageSample;
use parley_health::sampler::CodexSampler;

use common::{active_classes, supervisor, TempHome};

struct FailingSampler;

impl CodexSampler for FailingSampler {
    fn sample(&self) -> Result<Option<CodexUsageSample>, String> {
        Err("raw provider detail must not be retained".to_string())
    }
}

#[test]
fn sampler_failure_is_diagnostic_only_and_never_an_incident() {
    let home = TempHome::new("sampler-failure");
    let mut supervisor = supervisor(&home);
    supervisor.poll_sampler(&FailingSampler, 42).unwrap();
    assert!(active_classes(&supervisor.model).is_empty());
    assert_eq!(supervisor.model.diagnostics.codex_sample_failures, 1);
    assert_eq!(
        supervisor.model.diagnostics.last_codex_sample_failure_ms,
        Some(42)
    );
    let snapshot = std::fs::read_to_string(home.paths.snapshot()).unwrap();
    assert!(!snapshot.contains("raw provider detail"));
}
