//! Schema-v1 lane grants and the fail-closed `parley-lane-hook` decision core.
//!
//! Persisted records contain sanitized metadata and a content hash only.
//! Prompts, replies, environment values, credentials, and command lines are
//! not accepted and are not written.

mod error;
mod fsutil;
mod hash;
mod hook;
mod pathcheck;
mod schema;
mod store;
mod validate;

pub use error::{Denial, LaneError};
pub use hook::{evaluate, run, HookDecision, HookRequest, HookResponse};
pub use schema::{
    Access, ChildRole, FileIdentity, GrantDraft, GrantKind, GrantRecord, GrantState, PathGrant,
    CHILD_DEPTH, MAX_CHILD_SLOT, SCHEMA_VERSION, STATE_ENV,
};
pub use store::{
    activate_grant, bind_grant_for_child, claim_grant_for_spawn, consume_grant, create_grant,
    read_grant,
};
pub use validate::{validate_path, validate_spawn, PathClaim, SpawnClaim};
