//! Isolated Codex transcript reader, sanitizer, bounded seed/delta planner,
//! and durable cursor journal for the shared context bridge.
//!
//! The low-level readers and journal remain isolated here; `ask` owns policy,
//! logging, health, and child-process ordering around them.

mod error;
mod journal;
mod parse;
mod plan;
mod prompt_file;
mod reader;
mod root;
mod sanitize;
mod winfile;

pub(crate) use journal::{ContextKey, JournalStore, StateDirEnv};
pub(crate) use plan::{ContextBounds, ContextPlan, PlanKind};
pub(crate) use prompt_file::{cleanup_stale_prompt_files, PromptFile};
pub(crate) use root::{
    resolve_codex_rollout, resolve_codex_sessions_root, CodexHomeEnv, ResolvedSource,
};
#[cfg(test)]
pub(crate) use winfile::SourceIdentity;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ContextMode {
    #[default]
    Snapshot,
    Auto,
    Seed,
    Delta,
}

impl ContextMode {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "snapshot" => Ok(Self::Snapshot),
            "auto" => Ok(Self::Auto),
            "seed" => Ok(Self::Seed),
            "delta" => Ok(Self::Delta),
            _ => Err(format!(
                "context mode must be snapshot, auto, seed, or delta, got {value}"
            )),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Auto => "auto",
            Self::Seed => "seed",
            Self::Delta => "delta",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextRecovery {
    Replay,
    Skip,
}

impl ContextRecovery {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "replay" => Ok(Self::Replay),
            "skip" => Ok(Self::Skip),
            _ => Err(format!(
                "context recovery must be replay or skip, got {value}"
            )),
        }
    }
}

impl ContextBounds {
    pub(crate) fn from_env() -> Result<Self, String> {
        let bounds = Self {
            seed_max_chars: env_chars("PARLEY_CONTEXT_SEED_MAX_CHARS")?.unwrap_or(60_000),
            delta_max_chars: env_chars("PARLEY_CONTEXT_DELTA_MAX_CHARS")?.unwrap_or(24_000),
            hard_max_chars: env_chars("PARLEY_CONTEXT_HARD_MAX_CHARS")?.unwrap_or(120_000),
        };
        if bounds.seed_max_chars > bounds.hard_max_chars {
            return Err(
                "PARLEY_CONTEXT_SEED_MAX_CHARS cannot exceed PARLEY_CONTEXT_HARD_MAX_CHARS"
                    .to_string(),
            );
        }
        if bounds.delta_max_chars > bounds.hard_max_chars {
            return Err(
                "PARLEY_CONTEXT_DELTA_MAX_CHARS cannot exceed PARLEY_CONTEXT_HARD_MAX_CHARS"
                    .to_string(),
            );
        }
        Ok(bounds)
    }
}

fn env_chars(name: &str) -> Result<Option<usize>, String> {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(error) => return Err(format!("read {name}: {error}")),
    };
    let parsed = value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{name} must be a positive integer, got {value}"))?;
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests;
