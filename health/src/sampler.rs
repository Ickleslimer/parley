use crate::classifier::CodexUsageSample;

/// Codex usage sampler. The supervisor defines the interface only; this crate
/// does not implement or spawn Codex.
pub trait CodexSampler {
    fn sample(&self) -> Result<Option<CodexUsageSample>, String>;
}
