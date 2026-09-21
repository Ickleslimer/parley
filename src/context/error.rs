//! Fail-closed errors for the isolated context module.

use std::fmt;
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    NotFound,
    Duplicate,
    Unsupported,
    Malformed,
    Overflow,
    Truncated,
    Replaced,
    Mismatch,
    State,
    Locked,
    CorruptJournal,
    Io,
}

#[derive(Debug)]
pub(crate) struct ContextError {
    #[cfg_attr(not(test), allow(dead_code))]
    pub kind: ErrorKind,
    pub message: String,
}

impl ContextError {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    pub(crate) fn malformed(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Malformed, message)
    }

    pub(crate) fn state(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::State, message)
    }

    pub(crate) fn mismatch(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Mismatch, message)
    }

    pub(crate) fn overflow(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Overflow, message)
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, message)
    }

    pub(crate) fn corrupt(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::CorruptJournal, message)
    }
}

impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ContextError {}

impl From<io::Error> for ContextError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::Unsupported {
            return Self::unsupported(error.to_string());
        }
        Self::new(ErrorKind::Io, error.to_string())
    }
}
