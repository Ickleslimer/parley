//! Parse and validate explicit, disjoint Milestone 5 worktree lane manifests.

#![allow(dead_code)]

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::context::refuse_reparse_chain;
use crate::jobs::{GrantKind, JobMode, LaneGrant, LaneOwner, LanePlan, LaneRole, PathGrant};
use crate::json::Json;
use crate::policy::RuntimePolicy;

pub(crate) fn parse(value: &Json) -> Result<LanePlan, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "lane_plan must be an object".to_string())?;
    let base_commit = required_string(value, "base_commit")?;
    if !is_full_commit(&base_commit) {
        return Err("lane_plan.base_commit must be a full 40-character Git commit id".to_string());
    }
    let integration_cwd = PathBuf::from(required_string(value, "integration_worktree")?);
    let lanes = object
        .get("lanes")
        .and_then(Json::as_array)
        .ok_or_else(|| "lane_plan.lanes must be an array".to_string())?
        .iter()
        .map(parse_lane)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LanePlan {
        base_commit,
        integration_cwd,
        lanes,
    })
}

fn parse_lane(value: &Json) -> Result<LaneGrant, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "each lane_plan.lanes entry must be an object".to_string())?;
    let lane_id = required_string(value, "lane_id")?;
    if lane_id.is_empty()
        || lane_id.len() > 64
        || !lane_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!(
            "lane_id must contain only ASCII letters, digits, dot, dash, or underscore: {lane_id}"
        ));
    }
    let owner = match required_string(value, "owner")?.as_str() {
        "codex" => LaneOwner::Codex,
        "grok_parent" => LaneOwner::GrokParent,
        "grok_child" => LaneOwner::GrokChild,
        other => {
            return Err(format!(
                "lane {lane_id} owner must be codex, grok_parent, or grok_child, got {other}"
            ))
        }
    };
    let role = match required_string(value, "role")?.as_str() {
        "writer" => LaneRole::Writer,
        "reviewer" => LaneRole::Reviewer,
        other => {
            return Err(format!(
                "lane {lane_id} role must be writer or reviewer, got {other}"
            ))
        }
    };
    let cwd = PathBuf::from(required_string(value, "worktree")?);
    let writable_paths = object
        .get("writable_paths")
        .and_then(Json::as_array)
        .ok_or_else(|| format!("lane {lane_id} writable_paths must be an array"))?
        .iter()
        .map(|grant| parse_path_grant(&lane_id, grant))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LaneGrant {
        lane_id,
        owner,
        role,
        cwd,
        writable_paths,
    })
}

fn parse_path_grant(lane_id: &str, value: &Json) -> Result<PathGrant, String> {
    let _ = value
        .as_object()
        .ok_or_else(|| format!("lane {lane_id} writable_paths entries must be objects"))?;
    let kind = match required_string(value, "kind")?.as_str() {
        "file" => GrantKind::File,
        "tree" => GrantKind::Tree,
        other => {
            return Err(format!(
                "lane {lane_id} grant kind must be file or tree, got {other}"
            ))
        }
    };
    Ok(PathGrant {
        path: PathBuf::from(required_string(value, "path")?),
        kind,
    })
}

fn required_string(value: &Json, name: &str) -> Result<String, String> {
    value
        .get(name)
        .and_then(Json::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("missing or empty {name}"))
}

pub(crate) fn validate(
    mut plan: LanePlan,
    mode: JobMode,
    target_cwd: &Path,
    policy: &RuntimePolicy,
) -> Result<LanePlan, String> {
    if mode != JobMode::Write {
        return Err("lane_plan is only valid for job_mode=write".to_string());
    }
    if plan.lanes.len() < 2 || plan.lanes.len() > 4 {
        return Err("locked write jobs require two to four lanes".to_string());
    }

    let integration = validate_worktree(&plan.integration_cwd, &plan.base_commit, policy)?;
    plan.integration_cwd = integration.root;
    let common_dir = integration.common_dir;

    let mut lane_ids = HashSet::new();
    let mut worktrees = HashSet::new();
    worktrees.insert(path_key(&plan.integration_cwd));
    let mut codex_count = 0usize;
    let mut parent_count = 0usize;
    let mut child_count = 0usize;
    let mut parent_cwd = None;
    let mut writer_grants: Vec<(String, GrantKind, PathBuf)> = Vec::new();

    for lane in &mut plan.lanes {
        if !lane_ids.insert(lane.lane_id.to_ascii_lowercase()) {
            return Err(format!("duplicate lane_id: {}", lane.lane_id));
        }
        match lane.owner {
            LaneOwner::Codex => codex_count += 1,
            LaneOwner::GrokParent => {
                parent_count += 1;
                parent_cwd = Some(lane.cwd.clone());
            }
            LaneOwner::GrokChild => child_count += 1,
        }
        if matches!(lane.owner, LaneOwner::Codex | LaneOwner::GrokParent)
            && lane.role != LaneRole::Writer
        {
            return Err(format!("{} must be a writer lane", lane.lane_id));
        }
        if lane.role == LaneRole::Writer && lane.writable_paths.is_empty() {
            return Err(format!(
                "writer lane {} requires at least one writable path",
                lane.lane_id
            ));
        }
        if lane.role == LaneRole::Reviewer && !lane.writable_paths.is_empty() {
            return Err(format!(
                "reviewer lane {} may not declare writable paths",
                lane.lane_id
            ));
        }

        let validated = validate_worktree(&lane.cwd, &plan.base_commit, policy)?;
        if path_key(&validated.common_dir) != path_key(&common_dir) {
            return Err(format!(
                "lane {} does not share integration Git common-dir {}",
                lane.lane_id,
                common_dir.display()
            ));
        }
        if !worktrees.insert(path_key(&validated.root)) {
            return Err(format!(
                "lane {} reuses another lane or the integration worktree: {}",
                lane.lane_id,
                validated.root.display()
            ));
        }
        lane.cwd = validated.root;

        let mut local_grants: Vec<(GrantKind, PathBuf)> = Vec::new();
        for grant in &lane.writable_paths {
            let logical = validate_grant_syntax(&lane.lane_id, grant)?;
            validate_grant(&lane.lane_id, &lane.cwd, grant)?;
            if local_grants
                .iter()
                .any(|(kind, path)| grants_overlap(*kind, path, grant.kind, &logical))
            {
                return Err(format!(
                    "lane {} contains overlapping writable grants at {}",
                    lane.lane_id,
                    grant.path.display()
                ));
            }
            if lane.role == LaneRole::Writer {
                for (other_lane, other_kind, other_path) in &writer_grants {
                    if grants_overlap(*other_kind, other_path, grant.kind, &logical) {
                        return Err(format!(
                            "writer grants overlap between lanes {other_lane} and {} at {}",
                            lane.lane_id,
                            grant.path.display()
                        ));
                    }
                }
                writer_grants.push((lane.lane_id.clone(), grant.kind, logical.clone()));
            }
            local_grants.push((grant.kind, logical));
        }
    }

    if codex_count != 1 || parent_count != 1 || child_count > 2 {
        return Err(format!(
            "locked write jobs require exactly one codex lane, one grok_parent lane, and zero to two grok_child lanes (got {codex_count}/{parent_count}/{child_count})"
        ));
    }
    if child_count > 0 {
        policy.require_grok_child_lanes_ready()?;
    }
    let target = policy.validate_spawn_cwd(target_cwd)?;
    let parent = parent_cwd
        .as_ref()
        .ok_or_else(|| "missing grok_parent lane".to_string())?;
    let parent = fs::canonicalize(parent).map_err(|error| {
        format!(
            "canonicalize Grok parent worktree {}: {error}",
            parent.display()
        )
    })?;
    if path_key(&target) != path_key(&parent) {
        return Err(format!(
            "job cwd {} must equal the grok_parent worktree {}",
            target.display(),
            parent.display()
        ));
    }
    Ok(plan)
}

pub(crate) fn normalize_existing(
    mut plan: LanePlan,
    policy: &RuntimePolicy,
) -> Result<LanePlan, String> {
    plan.base_commit = plan.base_commit.to_ascii_lowercase();
    plan.integration_cwd = canonical_existing_worktree(&plan.integration_cwd, policy)?;
    let mut seen = HashSet::new();
    seen.insert(path_key(&plan.integration_cwd));
    for lane in &mut plan.lanes {
        lane.cwd = canonical_existing_worktree(&lane.cwd, policy)?;
        if !seen.insert(path_key(&lane.cwd)) {
            return Err(format!(
                "lane {} reuses another lane or the integration worktree",
                lane.lane_id
            ));
        }
        for grant in &lane.writable_paths {
            validate_grant_syntax(&lane.lane_id, grant)?;
        }
    }
    Ok(plan)
}

fn canonical_existing_worktree(path: &Path, policy: &RuntimePolicy) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!(
            "worktree path must be absolute: {}",
            path.display()
        ));
    }
    refuse_reparse_chain(path).map_err(|error| error.to_string())?;
    let root = policy.validate_spawn_cwd(path)?;
    let top_level = git_output(&root, &["rev-parse", "--show-toplevel"])?;
    let top_level = fs::canonicalize(top_level.trim())
        .map_err(|error| format!("canonicalize Git top-level for {}: {error}", root.display()))?;
    if path_key(&root) != path_key(&top_level) {
        return Err(format!(
            "lane path must be the Git worktree root: {}",
            root.display()
        ));
    }
    Ok(root)
}

struct WorktreeIdentity {
    root: PathBuf,
    common_dir: PathBuf,
}

fn validate_worktree(
    path: &Path,
    base_commit: &str,
    policy: &RuntimePolicy,
) -> Result<WorktreeIdentity, String> {
    if !path.is_absolute() {
        return Err(format!(
            "worktree path must be absolute: {}",
            path.display()
        ));
    }
    refuse_reparse_chain(path).map_err(|error| error.to_string())?;
    let root = policy.validate_spawn_cwd(path)?;
    let top_level = git_output(&root, &["rev-parse", "--show-toplevel"])?;
    let top_level = fs::canonicalize(top_level.trim())
        .map_err(|error| format!("canonicalize Git top-level for {}: {error}", root.display()))?;
    if path_key(&root) != path_key(&top_level) {
        return Err(format!(
            "lane path must be the Git worktree root: {}",
            root.display()
        ));
    }
    let head = git_output(&root, &["rev-parse", "--verify", "HEAD"])?;
    if !head.trim().eq_ignore_ascii_case(base_commit) {
        return Err(format!(
            "worktree {} is at {}, expected base {}",
            root.display(),
            head.trim(),
            base_commit
        ));
    }
    let status = git_output(
        &root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    if !status.trim().is_empty() {
        return Err(format!(
            "worktree {} is not clean at job start",
            root.display()
        ));
    }
    let common = git_output(
        &root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common_dir = fs::canonicalize(common.trim()).map_err(|error| {
        format!(
            "canonicalize Git common-dir for {}: {error}",
            root.display()
        )
    })?;
    Ok(WorktreeIdentity { root, common_dir })
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("run git in {}: {error}", cwd.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "git {} failed in {}: {}",
            args.join(" "),
            cwd.display(),
            if stderr.is_empty() {
                format!("exit {}", output.status)
            } else {
                stderr
            }
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| {
        format!(
            "git {} returned non-UTF-8 output in {}",
            args.join(" "),
            cwd.display()
        )
    })
}

fn validate_grant(lane_id: &str, root: &Path, grant: &PathGrant) -> Result<PathBuf, String> {
    let normalized = validate_grant_syntax(lane_id, grant)?;

    let target = root.join(&normalized);
    refuse_reparse_chain(&target).map_err(|error| error.to_string())?;
    let existing = deepest_existing(&target)?;
    let canonical_existing = fs::canonicalize(&existing).map_err(|error| {
        format!(
            "canonicalize grant ancestor {} for lane {lane_id}: {error}",
            existing.display()
        )
    })?;
    if !path_is_within(&canonical_existing, root) {
        return Err(format!(
            "lane {lane_id} grant escapes its worktree: {}",
            grant.path.display()
        ));
    }
    if target.exists() {
        let metadata = fs::metadata(&target)
            .map_err(|error| format!("inspect grant {}: {error}", target.display()))?;
        match grant.kind {
            GrantKind::File if !metadata.is_file() => {
                return Err(format!(
                    "lane {lane_id} file grant is not a file: {}",
                    grant.path.display()
                ))
            }
            GrantKind::Tree if !metadata.is_dir() => {
                return Err(format!(
                    "lane {lane_id} tree grant is not a directory: {}",
                    grant.path.display()
                ))
            }
            _ => {}
        }
    }
    Ok(root.join(normalized))
}

fn validate_grant_syntax(lane_id: &str, grant: &PathGrant) -> Result<PathBuf, String> {
    if grant.path.as_os_str().is_empty() || grant.path.is_absolute() {
        return Err(format!(
            "lane {lane_id} grants must be non-empty relative paths: {}",
            grant.path.display()
        ));
    }
    let text = grant.path.to_string_lossy();
    if text
        .chars()
        .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{' | '}'))
    {
        return Err(format!(
            "lane {lane_id} grants may not contain glob syntax: {}",
            grant.path.display()
        ));
    }
    let mut normalized = PathBuf::new();
    for component in grant.path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            _ => {
                return Err(format!(
                "lane {lane_id} grant must not contain root, prefix, dot, or parent components: {}",
                grant.path.display()
            ))
            }
        }
    }
    if normalized.components().next().is_some_and(|part| {
        part.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(".git")
    }) {
        return Err(format!(
            "lane {lane_id} may not grant Git administrative paths"
        ));
    }
    Ok(normalized)
}

fn deepest_existing(path: &Path) -> Result<PathBuf, String> {
    let mut current = path;
    loop {
        if current.exists() {
            return Ok(current.to_path_buf());
        }
        current = current
            .parent()
            .ok_or_else(|| format!("grant has no existing ancestor: {}", path.display()))?;
    }
}

fn grants_overlap(left_kind: GrantKind, left: &Path, right_kind: GrantKind, right: &Path) -> bool {
    let left_key = path_key(left);
    let right_key = path_key(right);
    left_key == right_key
        || (left_kind == GrantKind::Tree && path_is_within(right, left))
        || (right_kind == GrantKind::Tree && path_is_within(left, right))
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path_components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let root_components = root
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    path_components.len() >= root_components.len()
        && path_components[..root_components.len()] == root_components
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase()
}

fn is_full_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(path: &str, kind: GrantKind) -> PathGrant {
        PathGrant {
            path: PathBuf::from(path),
            kind,
        }
    }

    #[test]
    fn parses_closed_lane_manifest() {
        let value = Json::parse(
            r#"{
                "base_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "integration_worktree":"C:\\repo\\integration",
                "lanes":[{
                    "lane_id":"grok-parent",
                    "owner":"grok_parent",
                    "role":"writer",
                    "worktree":"C:\\repo\\parent",
                    "writable_paths":[{"kind":"tree","path":"src/jobs"}]
                }]
            }"#,
        )
        .unwrap();
        let plan = parse(&value).unwrap();
        assert_eq!(plan.lanes.len(), 1);
        assert_eq!(plan.lanes[0].owner, LaneOwner::GrokParent);
        assert_eq!(plan.lanes[0].writable_paths[0].kind, GrantKind::Tree);
    }

    #[test]
    fn rejects_globs_and_escape_components() {
        let root = Path::new("C:\\repo\\lane");
        assert!(validate_grant("lane", root, &grant("src/**", GrantKind::Tree)).is_err());
        assert!(validate_grant("lane", root, &grant("../other", GrantKind::Tree)).is_err());
        assert!(validate_grant("lane", root, &grant(".git/config", GrantKind::File)).is_err());
    }

    #[test]
    fn detects_file_and_tree_overlap_without_prefix_tricks() {
        let root = Path::new("C:\\repo\\lane");
        let tree = root.join("src/jobs");
        let child = tree.join("journal.rs");
        let sibling_prefix = root.join("src/jobs-old");
        assert!(grants_overlap(
            GrantKind::Tree,
            &tree,
            GrantKind::File,
            &child
        ));
        assert!(!grants_overlap(
            GrantKind::Tree,
            &tree,
            GrantKind::Tree,
            &sibling_prefix
        ));
    }

    #[test]
    fn commit_ids_are_exact() {
        assert!(is_full_commit("0123456789abcdef0123456789abcdef01234567"));
        assert!(!is_full_commit("HEAD"));
        assert!(!is_full_commit("xyz3456789abcdef0123456789abcdef01234567"));
    }
}
