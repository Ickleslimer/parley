use std::path::Path;

use crate::error::{Denial, LaneError};
use crate::pathcheck::{self, grant_covers, normal_path, same_dir};
use crate::schema::{Access, GrantRecord, GrantState, CHILD_DEPTH, MAX_CHILD_SLOT};

pub struct SpawnClaim<'a> {
    pub role: &'a str,
    pub cwd: &'a Path,
    pub isolation: Option<&'a str>,
    pub model: Option<&'a str>,
    pub effort_overridden: bool,
    pub widening: bool,
    pub requested_depth: u32,
    pub child_slot: Option<u32>,
}

pub struct PathClaim<'a> {
    pub role: &'a str,
    pub session_id: &'a str,
    pub cwd: &'a Path,
    pub access: Access,
    pub path: &'a Path,
}

pub fn validate_spawn(
    grant: &GrantRecord,
    claim: &SpawnClaim<'_>,
    now_ms: u64,
) -> Result<(), LaneError> {
    if claim.requested_depth != CHILD_DEPTH || grant.depth != CHILD_DEPTH {
        return Err(LaneError::new(Denial::Depth, "child depth must be one"));
    }
    ensure_live(grant, now_ms)?;
    if grant.state != GrantState::Prepared {
        return Err(LaneError::new(
            Denial::ConsumedGrant,
            "child grant has already been claimed",
        ));
    }
    if claim.widening || claim.effort_overridden {
        return Err(LaneError::new(
            Denial::Uncontrolled,
            "spawn carries an authority-widening field",
        ));
    }
    if claim.isolation != Some("none") {
        return Err(LaneError::new(
            Denial::Isolation,
            "child isolation must be none",
        ));
    }
    if claim.role != grant.role.as_str() {
        return Err(LaneError::new(
            Denial::Role,
            "child role does not match the grant",
        ));
    }
    if claim.model.is_some() {
        return Err(LaneError::new(
            Denial::Model,
            "child model must be inherited without a spawn override",
        ));
    }
    if let Some(slot) = claim.child_slot {
        if slot > MAX_CHILD_SLOT || slot != grant.child_slot {
            return Err(LaneError::new(
                Denial::ChildSlot,
                "child slot does not match the grant",
            ));
        }
    }
    confirm_cwd(grant, claim.cwd)
}

pub fn validate_path(
    grant: &GrantRecord,
    claim: &PathClaim<'_>,
    now_ms: u64,
) -> Result<(), LaneError> {
    ensure_live(grant, now_ms)?;
    if grant.state != GrantState::Running
        || grant.child_session_id.as_deref() != Some(claim.session_id)
    {
        return Err(LaneError::new(
            Denial::Role,
            "child session is not bound to the running grant",
        ));
    }
    if claim.role != grant.role.as_str() {
        return Err(LaneError::new(
            Denial::Role,
            "actor role does not match the grant",
        ));
    }
    confirm_cwd(grant, claim.cwd)?;
    let cwd = pathcheck::observe_dir(claim.cwd)?;
    let resolved = pathcheck::resolve_operation(&cwd, claim.path)?;
    if claim.access == Access::Read {
        return Ok(());
    }
    if claim.access == Access::Write {
        pathcheck::reject_hardlinked_write(&resolved)?;
    }
    let matches = grant
        .path_grants
        .iter()
        .filter(|grant_path| grant_covers(grant_path, claim.access, &resolved))
        .count();
    if matches != 1 {
        return Err(LaneError::new(
            Denial::Path,
            "path is outside the grant or matches more than one grant",
        ));
    }
    Ok(())
}

fn ensure_live(grant: &GrantRecord, now_ms: u64) -> Result<(), LaneError> {
    if grant.state == GrantState::Consumed || grant.consumed_at_ms.is_some() {
        return Err(LaneError::new(
            Denial::ConsumedGrant,
            "grant is already consumed",
        ));
    }
    if now_ms < grant.issued_at_ms || now_ms >= grant.expires_at_ms {
        return Err(LaneError::new(
            Denial::StaleGrant,
            "grant is outside its lifetime",
        ));
    }
    Ok(())
}

fn confirm_cwd(grant: &GrantRecord, cwd: &Path) -> Result<(), LaneError> {
    let observed = pathcheck::observe_dir(cwd)?;
    let expected = normal_path(Path::new(&grant.canonical_cwd))?;
    if !same_dir(&observed.normal, &expected) {
        return Err(LaneError::new(
            Denial::Cwd,
            "cwd is not the granted worktree",
        ));
    }
    if observed.identity != grant.cwd_identity {
        return Err(LaneError::new(
            Denial::StaleGrant,
            "cwd identity no longer matches the grant",
        ));
    }
    Ok(())
}
