use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Denial, LaneError};
use crate::fsutil::{self, metadata_is_reparse};
use crate::pathcheck::{self, normal_path, same_dir};
use crate::schema::{
    parse_grant, seal, validate_draft_shape, validate_token, Access, GrantDraft, GrantKind,
    GrantRecord, GrantState, PathGrant, MAX_CHILDREN, MAX_GRANT_BYTES, SCHEMA_VERSION,
};

pub fn create_grant(state_dir: &Path, draft: GrantDraft) -> Result<GrantRecord, LaneError> {
    validate_draft_shape(&draft)?;
    let cwd = pathcheck::observe_dir(&draft.canonical_cwd)?;
    let common = pathcheck::observe_dir(&draft.worktree_common_dir)?;
    let path_grants = normalize_grants(&cwd, draft.role, &draft.path_grants)?;
    let _lock = fsutil::acquire_lock(state_dir)?;
    let existing = load_all(state_dir)?;
    ensure_capacity(&existing, &draft, &cwd, &common, &path_grants)?;
    let record = GrantRecord {
        schema_version: SCHEMA_VERSION,
        grant_id: draft.grant_id.clone(),
        state: GrantState::Active,
        lane_id: draft.lane_id.clone(),
        role: draft.role,
        canonical_cwd: cwd.canonical.to_string_lossy().into_owned(),
        cwd_identity: cwd.identity,
        worktree_common_dir: common.canonical.to_string_lossy().into_owned(),
        common_dir_identity: common.identity,
        base_commit: draft.base_commit.to_ascii_lowercase(),
        path_grants,
        model: draft.model,
        depth: draft.depth,
        child_slot: draft.child_slot,
        issued_at_ms: draft.issued_at_ms,
        expires_at_ms: draft.expires_at_ms,
        consumed_at_ms: None,
        metadata_hash: String::new(),
    };
    let bytes = seal(&record)?;
    if crate::schema::contains_forbidden_material(&bytes) {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant contains forbidden material",
        ));
    }
    fsutil::atomic_create_new(&grant_path(state_dir, &record.grant_id)?, &bytes)?;
    read_grant(state_dir, &record.grant_id)
}

pub fn consume_grant(
    state_dir: &Path,
    grant_id: &str,
    now_ms: u64,
) -> Result<GrantRecord, LaneError> {
    if !validate_token(grant_id) {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant_id is not a safe token",
        ));
    }
    let _lock = fsutil::acquire_lock(state_dir)?;
    let mut record = read_grant(state_dir, grant_id)?;
    ensure_live(&record, now_ms)?;
    confirm_bindings(&record)?;
    record.state = GrantState::Consumed;
    record.consumed_at_ms = Some(now_ms);
    record.metadata_hash.clear();
    let bytes = seal(&record)?;
    if crate::schema::contains_forbidden_material(&bytes) {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant contains forbidden material",
        ));
    }
    fsutil::atomic_replace(&grant_path(state_dir, grant_id)?, &bytes)?;
    let consumed = read_grant(state_dir, grant_id)?;
    if consumed.state != GrantState::Consumed {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "consumed transition was not durable",
        ));
    }
    Ok(consumed)
}

pub fn read_grant(state_dir: &Path, grant_id: &str) -> Result<GrantRecord, LaneError> {
    let path = grant_path(state_dir, grant_id)?;
    let bytes = fsutil::read_limited(&path, MAX_GRANT_BYTES)?;
    let record = parse_grant(&bytes)?;
    if record.grant_id != grant_id {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant file name does not match grant_id",
        ));
    }
    Ok(record)
}

pub fn activate_grant(
    state_dir: &Path,
    grant_id: &str,
    now_ms: u64,
) -> Result<GrantRecord, LaneError> {
    let record = read_grant(state_dir, grant_id)?;
    ensure_live(&record, now_ms)?;
    confirm_bindings(&record)?;
    Ok(record)
}

pub fn find_grant_for_cwd(
    state_dir: &Path,
    cwd: &Path,
    now_ms: u64,
) -> Result<Option<GrantRecord>, LaneError> {
    ensure_state_dir(state_dir)?;
    let observed = pathcheck::observe_dir(cwd)?;
    let records = load_all(state_dir)?;
    let mut matched = Vec::new();
    for record in records {
        let root = normal_path(Path::new(&record.canonical_cwd))?;
        if same_dir(&root, &observed.normal) {
            matched.push(record);
        }
    }
    let live: Vec<&GrantRecord> = matched
        .iter()
        .filter(|record| is_live_at(record, now_ms))
        .collect();
    match live.len() {
        1 => Ok(Some(activate_grant(state_dir, &live[0].grant_id, now_ms)?)),
        count if count > 1 => Err(LaneError::new(
            Denial::Cwd,
            "multiple active grants match one cwd",
        )),
        _ if matched.is_empty() => Ok(None),
        _ => {
            let latest = matched
                .iter()
                .max_by_key(|record| record.issued_at_ms)
                .expect("matched is not empty");
            Ok(Some(activate_grant(state_dir, &latest.grant_id, now_ms)?))
        }
    }
}

pub fn has_stored_grants(state_dir: &Path) -> Result<bool, LaneError> {
    Ok(!load_all(state_dir)?.is_empty())
}

pub fn path_targets_lane(state_dir: &Path, path: &Path) -> Result<bool, LaneError> {
    ensure_state_dir(state_dir)?;
    let target = normal_path(path)?;
    for record in load_all(state_dir)? {
        let root = normal_path(Path::new(&record.canonical_cwd))?;
        if target.drive == root.drive
            && target.parts.len() >= root.parts.len()
            && target.parts[..root.parts.len()] == root.parts
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_state_dir(state_dir: &Path) -> Result<(), LaneError> {
    if !state_dir.is_absolute() {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "lane state dir must be absolute",
        ));
    }
    if !state_dir.exists() {
        return Err(LaneError::new(
            Denial::MissingGrant,
            "lane state dir is missing",
        ));
    }
    fsutil::refuse_reparse_chain(state_dir)?;
    if !state_dir.is_dir() {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "lane state dir is not a directory",
        ));
    }
    Ok(())
}

fn load_all(state_dir: &Path) -> Result<Vec<GrantRecord>, LaneError> {
    let grants_dir = state_dir.join("grants");
    if !grants_dir.exists() {
        return Ok(Vec::new());
    }
    fsutil::refuse_reparse_chain(&grants_dir)?;
    if !grants_dir.is_dir() {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grants path is not a directory",
        ));
    }
    let mut records = Vec::new();
    let entries = fs::read_dir(&grants_dir)
        .map_err(|error| LaneError::new(Denial::MalformedGrant, format!("read grants: {error}")))?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            LaneError::new(Denial::MalformedGrant, format!("read grants: {error}"))
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            LaneError::new(Denial::MalformedGrant, format!("inspect grant: {error}"))
        })?;
        if metadata_is_reparse(&metadata) || !metadata.is_file() {
            return Err(LaneError::new(
                Denial::MalformedGrant,
                "grant entry is not a real file",
            ));
        }
        let grant_id = name.trim_end_matches(".json");
        records.push(read_grant(state_dir, grant_id)?);
    }
    Ok(records)
}

fn normalize_grants(
    cwd: &pathcheck::ObservedDir,
    role: crate::schema::ChildRole,
    grants: &[PathGrant],
) -> Result<Vec<PathGrant>, LaneError> {
    let mut normalized = Vec::with_capacity(grants.len());
    for grant in grants {
        let resolved = pathcheck::normalize_grant(cwd, grant)?;
        if normalized
            .iter()
            .any(|existing| pathcheck::grants_overlap(existing, &resolved))
        {
            return Err(LaneError::new(Denial::Path, "path grants overlap"));
        }
        normalized.push(resolved);
    }
    let writes = normalized
        .iter()
        .filter(|grant| grant.access == Access::Write)
        .count();
    match role {
        crate::schema::ChildRole::Writer if writes == 0 => {
            return Err(LaneError::new(
                Denial::Path,
                "writer grant has no write path",
            ));
        }
        crate::schema::ChildRole::Reviewer if writes != 0 => {
            return Err(LaneError::new(
                Denial::Path,
                "reviewer grant contains a write path",
            ));
        }
        crate::schema::ChildRole::Writer | crate::schema::ChildRole::Reviewer => {}
    }
    Ok(normalized)
}

fn ensure_capacity(
    existing: &[GrantRecord],
    draft: &GrantDraft,
    cwd: &pathcheck::ObservedDir,
    common: &pathcheck::ObservedDir,
    grants: &[PathGrant],
) -> Result<(), LaneError> {
    if existing
        .iter()
        .any(|grant| grant.grant_id == draft.grant_id)
    {
        return Err(LaneError::new(Denial::Duplicate, "grant_id already exists"));
    }
    let live: Vec<&GrantRecord> = existing
        .iter()
        .filter(|grant| lifetimes_overlap(grant, draft))
        .collect();
    if live.iter().any(|grant| grant.lane_id == draft.lane_id) {
        return Err(LaneError::new(Denial::Duplicate, "lane_id already exists"));
    }
    if live.iter().any(|grant| grant.cwd_identity == cwd.identity) {
        return Err(LaneError::new(
            Denial::Cwd,
            "cwd is already bound to a grant",
        ));
    }
    let base = draft.base_commit.to_ascii_lowercase();
    let group: Vec<&GrantRecord> = live
        .into_iter()
        .filter(|grant| {
            grant.common_dir_identity == common.identity
                && grant.base_commit.eq_ignore_ascii_case(&base)
        })
        .collect();
    if group.len() >= MAX_CHILDREN {
        return Err(LaneError::new(
            Denial::ChildSlot,
            "a base already has the maximum two child grants",
        ));
    }
    if group
        .iter()
        .any(|grant| grant.child_slot == draft.child_slot)
    {
        return Err(LaneError::new(
            Denial::ChildSlot,
            "child slot is already allocated",
        ));
    }
    for other in group {
        let other_root = normal_path(Path::new(&other.canonical_cwd))?;
        for left in other
            .path_grants
            .iter()
            .filter(|grant| grant.access == Access::Write)
        {
            for right in grants.iter().filter(|grant| grant.access == Access::Write) {
                if write_grants_overlap(&other_root, left, &cwd.normal, right)? {
                    return Err(LaneError::new(
                        Denial::Path,
                        "writer grants overlap across lanes",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn lifetimes_overlap(record: &GrantRecord, draft: &GrantDraft) -> bool {
    record.state == GrantState::Active
        && record.consumed_at_ms.is_none()
        && record.issued_at_ms < draft.expires_at_ms
        && draft.issued_at_ms < record.expires_at_ms
}

fn is_live_at(record: &GrantRecord, now_ms: u64) -> bool {
    record.state == GrantState::Active
        && record.consumed_at_ms.is_none()
        && record.issued_at_ms <= now_ms
        && now_ms < record.expires_at_ms
}

fn write_grants_overlap(
    left_root: &pathcheck::NormalPath,
    left: &PathGrant,
    right_root: &pathcheck::NormalPath,
    right: &PathGrant,
) -> Result<bool, LaneError> {
    let left_parts = extend_root(left_root, &left.path)?;
    let right_parts = extend_root(right_root, &right.path)?;
    Ok(absolute_overlap(
        left.kind,
        &left_parts,
        right.kind,
        &right_parts,
    ))
}

fn extend_root(root: &pathcheck::NormalPath, relative: &str) -> Result<Vec<String>, LaneError> {
    let mut parts = root.parts.clone();
    for component in relative.split('/') {
        if component.is_empty() {
            return Err(LaneError::new(Denial::Path, "grant path is empty"));
        }
        parts.push(component.to_string());
    }
    Ok(parts)
}

fn absolute_overlap(
    left_kind: GrantKind,
    left: &[String],
    right_kind: GrantKind,
    right: &[String],
) -> bool {
    (left_kind == GrantKind::Tree && right.starts_with(left))
        || (right_kind == GrantKind::Tree && left.starts_with(right))
        || left == right
}

fn ensure_live(record: &GrantRecord, now_ms: u64) -> Result<(), LaneError> {
    if record.state != GrantState::Active || record.consumed_at_ms.is_some() {
        return Err(LaneError::new(
            Denial::ConsumedGrant,
            "grant is already consumed",
        ));
    }
    if now_ms < record.issued_at_ms || now_ms >= record.expires_at_ms {
        return Err(LaneError::new(
            Denial::StaleGrant,
            "grant is outside its lifetime",
        ));
    }
    Ok(())
}

fn confirm_bindings(record: &GrantRecord) -> Result<(), LaneError> {
    let cwd = pathcheck::observe_dir(Path::new(&record.canonical_cwd))?;
    let expected = normal_path(Path::new(&record.canonical_cwd))?;
    if !same_dir(&cwd.normal, &expected) || cwd.identity != record.cwd_identity {
        return Err(LaneError::new(
            Denial::StaleGrant,
            "cwd identity no longer matches the grant",
        ));
    }
    let common = pathcheck::observe_dir(Path::new(&record.worktree_common_dir))?;
    if common.identity != record.common_dir_identity {
        return Err(LaneError::new(
            Denial::StaleGrant,
            "worktree common-dir identity no longer matches the grant",
        ));
    }
    Ok(())
}

fn grant_path(state_dir: &Path, grant_id: &str) -> Result<PathBuf, LaneError> {
    if !validate_token(grant_id) {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "grant_id is not a safe token",
        ));
    }
    Ok(state_dir.join("grants").join(format!("{grant_id}.json")))
}
