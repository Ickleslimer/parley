use std::fmt;

/// Fail-closed reason. Hook stdout uses [`Denial::reason`] and never the detail string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    MalformedHook,
    MalformedGrant,
    StaleGrant,
    MissingGrant,
    ConsumedGrant,
    Role,
    Cwd,
    Isolation,
    Model,
    Depth,
    ChildSlot,
    Path,
    Uncontrolled,
    Lock,
    Duplicate,
}

impl Denial {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::MalformedHook => "parley-lane: deny malformed hook",
            Self::MalformedGrant => "parley-lane: deny malformed grant",
            Self::StaleGrant => "parley-lane: deny stale grant",
            Self::MissingGrant => "parley-lane: deny missing grant",
            Self::ConsumedGrant => "parley-lane: deny consumed grant",
            Self::Role => "parley-lane: deny role",
            Self::Cwd => "parley-lane: deny cwd",
            Self::Isolation => "parley-lane: deny isolation",
            Self::Model => "parley-lane: deny model",
            Self::Depth => "parley-lane: deny depth",
            Self::ChildSlot => "parley-lane: deny child slot",
            Self::Path => "parley-lane: deny path",
            Self::Uncontrolled => "parley-lane: deny uncontrolled operation",
            Self::Lock => "parley-lane: deny grant lock",
            Self::Duplicate => "parley-lane: deny duplicate grant",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneError {
    pub denial: Denial,
    pub detail: String,
}

impl LaneError {
    pub fn new(denial: Denial, detail: impl Into<String>) -> Self {
        Self {
            denial,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for LaneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.denial.reason(), self.detail)
    }
}

impl std::error::Error for LaneError {}
