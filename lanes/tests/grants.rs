use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use parley_lanes::{
    consume_grant, create_grant, read_grant, Access, ChildRole, Denial, GrantDraft, GrantKind,
    HookDecision, HookRequest, PathGrant, CHILD_DEPTH,
};
use serde_json::{json, Value};

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const NOW: u64 = 5_000;
const SENTINEL: &str = "SENTINEL_PROMPT_SHOULD_NOT_LEAK";

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("parley-lanes-{}-{nanos}-{seq}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        remove_tree(&self.0);
    }
}

struct World {
    _root: TempDir,
    state: PathBuf,
    common: PathBuf,
    work: PathBuf,
}

impl World {
    fn new() -> Self {
        let root = TempDir::new();
        let state = root.path().join("state");
        let common = root.path().join("common");
        let work = root.path().join("work");
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&common).unwrap();
        write_worktree(&work);
        Self {
            _root: root,
            state,
            common,
            work,
        }
    }

    fn other_work(&self, name: &str) -> PathBuf {
        let path = self._root.path().join(name);
        write_worktree(&path);
        path
    }
}

fn write_worktree(path: &Path) {
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src").join("lib.rs"), b"fn kept() {}\n").unwrap();
}

fn draft(world: &World, id: &str, slot: u32, work: &Path) -> GrantDraft {
    GrantDraft {
        grant_id: id.to_string(),
        lane_id: format!("grok-child-{slot}"),
        role: ChildRole::Writer,
        canonical_cwd: work.to_path_buf(),
        worktree_common_dir: world.common.clone(),
        base_commit: BASE.to_string(),
        path_grants: vec![PathGrant {
            access: Access::Write,
            kind: GrantKind::Tree,
            path: "src".to_string(),
        }],
        model: "grok-4.7".to_string(),
        depth: CHILD_DEPTH,
        child_slot: slot,
        issued_at_ms: 1_000,
        expires_at_ms: 9_000_000_000_000,
    }
}

fn grant_file(world: &World, id: &str) -> PathBuf {
    world.state.join("grants").join(format!("{id}.json"))
}

fn hook<'a>(payload: &'a [u8], world: &'a World, now_ms: u64) -> parley_lanes::HookResponse {
    parley_lanes::evaluate(HookRequest {
        payload,
        state_dir: Some(&world.state),
        now_ms,
    })
}

fn event(tool: &str, cwd: &Path, input: Value, actor: Option<&str>) -> Vec<u8> {
    let mut value = json!({
        "hook_event_name": "PreToolUse",
        "hookEventName": "pre_tool_use",
        "cwd": cwd.display().to_string(),
        "toolName": tool,
        "toolInput": input,
        "permissionDecision": "allow",
        "prompt": SENTINEL
    });
    if let Some(actor) = actor {
        value["subagentType"] = json!(actor);
    }
    serde_json::to_vec(&value).unwrap()
}

fn spawn_input(
    work: &Path,
    role: &str,
    isolation: &str,
    model: Option<&str>,
    slot: Option<u32>,
) -> Value {
    let mut input = json!({
        "subagent_type": role,
        "cwd": work.display().to_string(),
        "isolation": isolation,
        "prompt": SENTINEL,
        "description": "lane task"
    });
    if let Some(model) = model {
        input["model"] = json!(model);
    }
    if let Some(slot) = slot {
        input["child_slot"] = json!(slot);
    }
    input
}

fn denial(response: &parley_lanes::HookResponse) -> Denial {
    match response.decision {
        HookDecision::Deny(denial) => denial,
        HookDecision::Silent => panic!("expected deny, got silent"),
        HookDecision::PassThrough => {
            panic!("expected deny, got pass-through: {:?}", response.stdout)
        }
    }
}

fn assert_static_deny(response: &parley_lanes::HookResponse, expected: Denial) {
    assert_eq!(denial(response), expected);
    let stdout = response.stdout.as_deref().expect("deny stdout");
    assert!(!stdout.contains(SENTINEL));
    assert!(!stdout.contains("allow"));
    let parsed: Value = serde_json::from_str(stdout).unwrap();
    assert_eq!(parsed["decision"], "deny");
    assert_eq!(parsed["permissionDecision"], "deny");
    assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert!(parsed.get("updatedInput").is_none());
    assert!(parsed["hookSpecificOutput"].get("updatedInput").is_none());
}

#[test]
fn persisted_grant_is_sanitized_schema_v1() {
    let world = World::new();
    let record = create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    assert_eq!(record.schema_version, 1);
    assert_eq!(record.model, "grok-4.7");
    assert_eq!(record.depth, 1);
    assert_eq!(record.child_slot, 0);
    assert_eq!(record.base_commit, BASE);
    let bytes = fs::read(grant_file(&world, "child-0")).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    for key in [
        "schema_version",
        "grant_id",
        "state",
        "lane_id",
        "role",
        "canonical_cwd",
        "worktree_common_dir",
        "base_commit",
        "path_grants",
        "model",
        "depth",
        "child_slot",
        "metadata_hash",
    ] {
        assert!(value.get(key).is_some(), "{key}");
    }
    assert_eq!(value["role"], "two-chairs-writer");
    assert_eq!(value["state"], "active");
    for forbidden in ["prompt", "reply", "command", "env", "credential", "token"] {
        assert!(value.get(forbidden).is_none(), "{forbidden}");
    }
    assert!(!String::from_utf8_lossy(&bytes).contains(SENTINEL));
}

#[test]
fn create_new_does_not_overwrite() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    let before = fs::read(grant_file(&world, "child-0")).unwrap();
    let error = create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap_err();
    assert_eq!(error.denial, Denial::Duplicate);
    assert_eq!(fs::read(grant_file(&world, "child-0")).unwrap(), before);
}

#[test]
fn consumed_transition_is_atomic_and_single_use() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    let consumed = consume_grant(&world.state, "child-0", NOW).unwrap();
    assert_eq!(consumed.consumed_at_ms, Some(NOW));
    let again = consume_grant(&world.state, "child-0", NOW).unwrap_err();
    assert_eq!(again.denial, Denial::ConsumedGrant);
    let bytes = fs::read(grant_file(&world, "child-0")).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["state"], "consumed");
    let entries = fs::read_dir(world.state.join("grants")).unwrap();
    for entry in entries {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(!name.ends_with(".tmp"), "{name}");
    }
}

#[test]
fn consumed_and_expired_grants_release_lane_capacity() {
    let world = World::new();
    let second = world.other_work("work-b");
    create_grant(&world.state, draft(&world, "old-0", 0, &world.work)).unwrap();
    create_grant(&world.state, draft(&world, "old-1", 1, &second)).unwrap();
    consume_grant(&world.state, "old-0", NOW).unwrap();
    consume_grant(&world.state, "old-1", NOW).unwrap();

    let mut replacement = draft(&world, "new-0", 0, &world.work);
    replacement.issued_at_ms = NOW + 1;
    create_grant(&world.state, replacement).unwrap();

    let expired_work = world.other_work("expired-work");
    let mut expired = draft(&world, "expired-1", 1, &expired_work);
    expired.issued_at_ms = 1_000;
    expired.expires_at_ms = 2_000;
    create_grant(&world.state, expired).unwrap();
    let mut replacement = draft(&world, "new-1", 1, &expired_work);
    replacement.issued_at_ms = 2_000;
    replacement.expires_at_ms = 9_000_000_000_001;
    create_grant(&world.state, replacement).unwrap();

    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path": "src/lib.rs"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW + 2,
    );
    assert_eq!(response.decision, HookDecision::PassThrough);
}

#[test]
fn malformed_stale_and_missing_grants_fail_closed() {
    let world = World::new();
    let error = read_grant(&world.state, "missing-grant").unwrap_err();
    assert_eq!(error.denial, Denial::MissingGrant);

    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    let path = grant_file(&world, "child-0");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["metadata_hash"] =
        json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let error = read_grant(&world.state, "child-0").unwrap_err();
    assert_eq!(error.denial, Denial::MalformedGrant);

    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path": "src/lib.rs"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::MalformedGrant);

    fs::remove_file(&path).unwrap();
    let mut expired = draft(&world, "child-0", 0, &world.work);
    expired.issued_at_ms = 1_000;
    expired.expires_at_ms = 2_000;
    create_grant(&world.state, expired).unwrap();
    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path": "src/lib.rs"}),
            Some("two-chairs-writer"),
        ),
        &world,
        2_000,
    );
    assert_static_deny(&response, Denial::StaleGrant);
}

#[test]
fn controlled_operations_fail_closed_without_lane_state() {
    let world = World::new();
    let child_payload = event(
        "search_replace",
        &world.work,
        json!({"file_path": "src/lib.rs"}),
        Some("two-chairs-writer"),
    );
    let response = parley_lanes::evaluate(HookRequest {
        payload: &child_payload,
        state_dir: None,
        now_ms: NOW,
    });
    assert_static_deny(&response, Denial::MissingGrant);

    let spawn_payload = event(
        "spawn_subagent",
        &world.work,
        spawn_input(&world.work, "two-chairs-writer", "none", None, Some(0)),
        None,
    );
    let response = parley_lanes::evaluate(HookRequest {
        payload: &spawn_payload,
        state_dir: None,
        now_ms: NOW,
    });
    assert_static_deny(&response, Denial::MissingGrant);

    let unrelated_payload = event(
        "search_replace",
        &world.work,
        json!({"file_path": "src/lib.rs"}),
        None,
    );
    let response = parley_lanes::evaluate(HookRequest {
        payload: &unrelated_payload,
        state_dir: None,
        now_ms: NOW,
    });
    assert_eq!(response.decision, HookDecision::Silent);
}

#[test]
fn third_child_grandchild_and_consumed_slot_are_rejected() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    let second = world.other_work("work-b");
    create_grant(&world.state, draft(&world, "child-1", 1, &second)).unwrap();
    let third = world.other_work("work-c");
    let mut third_draft = draft(&world, "child-2", 0, &third);
    third_draft.lane_id = "grok-child-2".to_string();
    third_draft.child_slot = 0;
    let error = create_grant(&world.state, third_draft).unwrap_err();
    assert_eq!(error.denial, Denial::ChildSlot);

    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&world.work, "two-chairs-writer", "none", None, Some(2)),
            None,
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::ChildSlot);

    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&world.work, "two-chairs-writer", "none", None, None),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Depth);

    consume_grant(&world.state, "child-0", NOW).unwrap();
    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path": "src/lib.rs"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::ConsumedGrant);
}

#[test]
fn wrong_role_cwd_isolation_and_model_are_rejected() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    let elsewhere = world.other_work("elsewhere");

    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&world.work, "general-purpose", "none", None, None),
            None,
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Role);

    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&elsewhere, "two-chairs-writer", "none", None, None),
            None,
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Cwd);

    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&world.work, "two-chairs-writer", "worktree", None, None),
            None,
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Isolation);

    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(
                &world.work,
                "two-chairs-writer",
                "none",
                Some("grok-4.6"),
                None,
            ),
            None,
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Model);
}

#[test]
fn path_overlap_escape_casing_and_new_files_follow_the_grant() {
    let world = World::new();
    let mut overlapping = draft(&world, "child-0", 0, &world.work);
    overlapping.path_grants.push(PathGrant {
        access: Access::Write,
        kind: GrantKind::File,
        path: "src/lib.rs".to_string(),
    });
    let error = create_grant(&world.state, overlapping).unwrap_err();
    assert_eq!(error.denial, Denial::Path);

    let mut ambiguous = draft(&world, "child-0", 0, &world.work);
    ambiguous.path_grants = vec![PathGrant {
        access: Access::Write,
        kind: GrantKind::File,
        path: "src/missing/new.rs".to_string(),
    }];
    let error = create_grant(&world.state, ambiguous).unwrap_err();
    assert_eq!(error.denial, Denial::Path);

    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    fs::create_dir_all(world.work.join("src-extra")).unwrap();
    let before = fs::read(grant_file(&world, "child-0")).unwrap();

    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path": "src-extra/a.rs", "prompt": SENTINEL}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Path);

    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path": "src/../../outside.rs"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Path);

    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"path": "SRC/LIB.rs", "file_path": "SRC/LIB.rs"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW,
    );
    assert_eq!(response.decision, HookDecision::PassThrough);

    let response = hook(
        &event(
            "write",
            &world.work,
            json!({"file_path": "src/nested/new.rs"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW,
    );
    assert_eq!(response.decision, HookDecision::PassThrough);
    let stdout = response.stdout.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap(),
        json!({"decision": "defer"})
    );
    assert!(!stdout.contains(SENTINEL));
    assert!(!stdout.contains("allow"));
    assert_eq!(fs::read(grant_file(&world, "child-0")).unwrap(), before);
}

#[test]
fn reparse_points_are_rejected() {
    let world = World::new();
    let link = world.work.join("link");
    let created = std::os::windows::fs::symlink_dir(world.work.join("src"), &link).is_ok()
        || Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(world.work.join("src"))
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
    assert!(
        created,
        "junction creation is required to prove reparse rejection"
    );
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path": "link/lib.rs"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Path);
    let _ = fs::remove_dir(&link);
}

#[test]
fn hook_pass_through_requires_a_fully_valid_grant() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    let response = hook(b"", &world, NOW);
    assert_static_deny(&response, Denial::MalformedHook);
    let response = hook(b"{", &world, NOW);
    assert_static_deny(&response, Denial::MalformedHook);

    let response = hook(
        &event(
            "run_terminal_command",
            &world.work,
            json!({"command": "git status"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Uncontrolled);

    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            {
                let mut input = spawn_input(&world.work, "two-chairs-writer", "none", None, None);
                input["tools"] = json!(["write"]);
                input
            },
            None,
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Uncontrolled);

    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(
                &world.work,
                "two-chairs-writer",
                "none",
                Some("grok-4.7"),
                None,
            ),
            None,
        ),
        &world,
        NOW,
    );
    assert_eq!(response.decision, HookDecision::PassThrough);
    assert_eq!(
        serde_json::from_str::<Value>(response.stdout.as_deref().unwrap()).unwrap(),
        json!({"decision": "defer"})
    );

    let elsewhere = world.other_work("parent");
    let response = hook(
        &event(
            "search_replace",
            &elsewhere,
            json!({"file_path": "src/lib.rs"}),
            None,
        ),
        &world,
        NOW,
    );
    assert_eq!(response.decision, HookDecision::Silent);
    assert!(response.stdout.is_none());
}

#[test]
fn hook_binary_fails_closed_without_granting_input_fields() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_parley-lane-hook"))
        .env("PARLEY_LANE_STATE_DIR", &world.state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = event(
        "search_replace",
        &world.work,
        json!({"file_path": "src/lib.rs", "new_string": SENTINEL}),
        Some("two-chairs-writer"),
    );
    let mut child = output;
    use std::io::Write;
    child.stdin.as_mut().unwrap().write_all(&payload).unwrap();
    drop(child.stdin.take());
    let finished = child.wait_with_output().unwrap();
    assert!(finished.status.success());
    let stdout = String::from_utf8(finished.stdout).unwrap();
    assert!(!stdout.contains(SENTINEL));
    assert_eq!(
        serde_json::from_str::<Value>(stdout.trim()).unwrap()["decision"],
        "defer"
    );

    let denied = Command::new(env!("CARGO_BIN_EXE_parley-lane-hook"))
        .env("PARLEY_LANE_STATE_DIR", "relative-state")
        .arg("ignored")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut denied = denied;
    denied.stdin.as_mut().unwrap().write_all(&payload).unwrap();
    drop(denied.stdin.take());
    let finished = denied.wait_with_output().unwrap();
    let stdout = String::from_utf8(finished.stdout).unwrap();
    assert!(stdout.contains("parley-lane: deny malformed grant"));
    assert!(!stdout.contains(SENTINEL));
}

fn remove_tree(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = metadata.file_type().is_symlink();
    if reparse {
        let _ = fs::remove_dir(path);
        let _ = fs::remove_file(path);
        return;
    }
    if metadata.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                remove_tree(&entry.path());
            }
        }
        let _ = fs::remove_dir(path);
    } else {
        let _ = fs::remove_file(path);
    }
}
