#![allow(dead_code)]

use std::path::PathBuf;

use crate::ask::AskRequest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobMode {
    Write,
    Review,
    Probe,
}

impl JobMode {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "write" => Ok(Self::Write),
            "review" => Ok(Self::Review),
            "probe" => Ok(Self::Probe),
            _ => Err(format!(
                "job_mode must be write, review, or probe, got {value}"
            )),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Review => "review",
            Self::Probe => "probe",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobState {
    Preparing,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Interrupted,
}

impl JobState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    pub(crate) fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::TimedOut | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaneOwner {
    Codex,
    GrokParent,
    GrokChild,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaneRole {
    Writer,
    Reviewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrantKind {
    File,
    Tree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PathGrant {
    pub path: PathBuf,
    pub kind: GrantKind,
}

#[derive(Clone, Debug)]
pub(crate) struct LaneGrant {
    pub lane_id: String,
    pub owner: LaneOwner,
    pub role: LaneRole,
    pub cwd: PathBuf,
    pub writable_paths: Vec<PathGrant>,
}

#[derive(Clone, Debug)]
pub(crate) struct LanePlan {
    pub base_commit: String,
    pub integration_cwd: PathBuf,
    pub lanes: Vec<LaneGrant>,
}

#[derive(Clone, Debug)]
pub(crate) struct JobSpec {
    pub job_id: String,
    pub mode: JobMode,
    pub request: AskRequest,
    pub lane_plan: Option<LanePlan>,
}

#[derive(Clone, Debug)]
pub(crate) struct JobView {
    pub job_id: String,
    pub state: JobState,
    pub exchange_id: Option<String>,
    pub session_id: Option<String>,
    pub request_event_id: Option<String>,
    pub completion_event_id: Option<String>,
    pub error: Option<String>,
    pub reply: Option<String>,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_modes_and_terminal_states_are_closed() {
        assert_eq!(JobMode::parse("write").unwrap(), JobMode::Write);
        assert!(JobMode::parse("other").is_err());
        assert!(!JobState::Running.is_terminal());
        assert!(JobState::Interrupted.is_terminal());
    }
}
