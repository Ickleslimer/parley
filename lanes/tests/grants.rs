use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use parley_lanes::{
    consume_grant, create_grant, read_grant, Access, ChildRole, Denial, GrantDraft, GrantKind,
    GrantState, HookDecision, HookRequest, PathGrant, CHILD_DEPTH,
};
use serde_json::{json, Value};

const NOW: u64 = 5_000;
const SENTINEL: &str = "SENTINEL_PROMPT_SHOULD_NOT_LEAK";
const PARENT_SESSION: &str = "parent-session";
const CHILD_SESSION: &str = "child-session";
const TOOL_USE: &str = "call-child-0";

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

#[test]
fn parses_the_zero_dependency_root_grant_fixture() {
    let temp = TempDir::new();
    let grants = temp.path().join("grants");
    fs::create_dir_all(&grants).unwrap();
    let grant_id = "00000000-0000-4000-8000-000000000000-c0";
    fs::write(
        grants.join(format!("{grant_id}.json")),
        include_bytes!("fixtures/root-grant-v1.json"),
    )
    .unwrap();
    let grant = read_grant(temp.path(), grant_id).unwrap();
    assert_eq!(grant.state, GrantState::Prepared);
    assert_eq!(grant.role, ChildRole::Writer);
    assert_eq!(grant.path_grants[0].path, "src/jobs");
}

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
    repo: PathBuf,
    state: PathBuf,
    common: PathBuf,
    work: PathBuf,
    base: String,
}

impl World {
    fn new() -> Self {
        let root = TempDir::new();
        let repo = root.path().join("repo");
        let state = root.path().join("state");
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::create_dir_all(&state).unwrap();
        git(root.path(), &["init", "-b", "main", repo.to_str().unwrap()]);
        git(&repo, &["config", "user.name", "Parley Tests"]);
        git(
            &repo,
            &["config", "user.email", "parley-tests@example.invalid"],
        );
        fs::write(repo.join("src/lib.rs"), b"fn kept() {}\n").unwrap();
        git(&repo, &["add", "src/lib.rs"]);
        git(&repo, &["commit", "-m", "fixture"]);
        let base = git_output(&repo, &["rev-parse", "HEAD"]);
        let common = fs::canonicalize(git_output(
            &repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ))
        .unwrap();
        let work = root.path().join("work");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "test-work",
                work.to_str().unwrap(),
                &base,
            ],
        );
        Self {
            _root: root,
            repo,
            state,
            common,
            work,
            base,
        }
    }

    fn other_work(&self, name: &str) -> PathBuf {
        let path = self._root.path().join(name);
        let branch = format!("test-{name}");
        git(
            &self.repo,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                path.to_str().unwrap(),
                &self.base,
            ],
        );
        path
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn draft(world: &World, id: &str, slot: u32, work: &Path) -> GrantDraft {
    GrantDraft {
        grant_id: id.to_string(),
        lane_id: format!("grok-child-{slot}"),
        role: ChildRole::Writer,
        canonical_cwd: work.to_path_buf(),
        worktree_common_dir: world.common.clone(),
        base_commit: world.base.clone(),
        path_grants: vec![PathGrant {
            access: Access::Write,
            kind: GrantKind::Tree,
            path: "src".to_string(),
        }],
        model: "grok-4.7".to_string(),
        reasoning_effort: "xhigh".to_string(),
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
    evaluate(payload, Some(&world.state), now_ms, true)
}

fn evaluate(
    payload: &[u8],
    state_dir: Option<&Path>,
    now_ms: u64,
    valid_runtime: bool,
) -> parley_lanes::HookResponse {
    parley_lanes::evaluate(HookRequest {
        payload,
        state_dir,
        model_inheritance: valid_runtime.then_some("1"),
        sampling_limit: valid_runtime.then_some("2"),
        active_agent_messages: valid_runtime.then_some("0"),
        workflows: valid_runtime.then_some("0"),
        memory: valid_runtime.then_some("0"),
        locked_model: valid_runtime.then_some("grok-4.7"),
        locked_reasoning_effort: valid_runtime.then_some("xhigh"),
        now_ms,
    })
}

fn event(tool: &str, cwd: &Path, input: Value, actor: Option<&str>) -> Vec<u8> {
    let mut value = json!({
        "hook_event_name": "PreToolUse",
        "hookEventName": "pre_tool_use",
        "sessionId": if actor.is_some() { CHILD_SESSION } else { PARENT_SESSION },
        "toolUseId": TOOL_USE,
        "toolInputTruncated": false,
        "cwd": cwd.display().to_string(),
        "toolName": tool,
        "toolInput": input,
        "prompt": SENTINEL
    });
    if let Some(actor) = actor {
        value["subagentType"] = json!(actor);
    }
    serde_json::to_vec(&value).unwrap()
}

fn spawn_input(work: &Path, role: &str, isolation: &str) -> Value {
    json!({
        "subagent_type": role,
        "cwd": work.display().to_string(),
        "isolation": isolation,
        "background": true,
        "prompt": SENTINEL,
        "description": "lane task"
    })
}

fn claim(world: &World, work: &Path, role: &str) {
    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(work, role, "none"),
            None,
        ),
        world,
        NOW,
    );
    assert_eq!(response.decision, HookDecision::PassThrough);
    assert!(response.stdout.is_none());
}

fn denial(response: &parley_lanes::HookResponse) -> Denial {
    match response.decision {
        HookDecision::Deny(denial) => denial,
        HookDecision::Silent => panic!("expected deny, got silent"),
        HookDecision::PassThrough => panic!("expected deny, got pass-through"),
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
}

#[test]
fn persisted_grant_is_sanitized_prepared_schema_v1() {
    let world = World::new();
    let record = create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    assert_eq!(record.schema_version, 1);
    assert_eq!(record.state, GrantState::Prepared);
    assert_eq!(record.model, "grok-4.7");
    assert_eq!(record.reasoning_effort, "xhigh");
    assert_eq!(record.base_commit, world.base);
    let bytes = fs::read(grant_file(&world, "child-0")).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["state"], "prepared");
    for key in [
        "schema_version",
        "grant_id",
        "lane_id",
        "canonical_cwd",
        "worktree_common_dir",
        "path_grants",
        "model",
        "reasoning_effort",
        "metadata_hash",
    ] {
        assert!(value.get(key).is_some(), "{key}");
    }
    for forbidden in ["prompt", "reply", "command", "env", "credential", "token"] {
        assert!(value.get(forbidden).is_none(), "{forbidden}");
    }
    assert!(!String::from_utf8_lossy(&bytes).contains(SENTINEL));
}

#[test]
fn spawn_claim_is_atomic_single_use_and_binds_one_child_session() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    claim(&world, &world.work, "two-chairs-writer");
    assert_eq!(
        read_grant(&world.state, "child-0").unwrap().state,
        GrantState::Claimed
    );

    let repeated = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&world.work, "two-chairs-writer", "none"),
            None,
        ),
        &world,
        NOW + 1,
    );
    assert_static_deny(&repeated, Denial::ConsumedGrant);

    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path":"src/lib.rs","old_string":"kept","new_string":"kept"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW + 2,
    );
    assert_eq!(response.decision, HookDecision::PassThrough);
    assert!(response.stdout.is_none());
    let running = read_grant(&world.state, "child-0").unwrap();
    assert_eq!(running.state, GrantState::Running);
    assert_eq!(running.child_session_id.as_deref(), Some(CHILD_SESSION));

    let other_session = json!({
        "hook_event_name":"PreToolUse","hookEventName":"pre_tool_use",
        "sessionId":"other-child","toolUseId":"call-other","toolInputTruncated":false,
        "cwd":world.work.display().to_string(),"toolName":"read_file",
        "toolInput":{"target_file":"src/lib.rs"},"subagentType":"two-chairs-writer"
    });
    let response = hook(
        &serde_json::to_vec(&other_session).unwrap(),
        &world,
        NOW + 3,
    );
    assert_static_deny(&response, Denial::Role);
}

#[test]
fn malformed_truncated_and_missing_state_fail_closed() {
    let world = World::new();
    let controlled = event(
        "spawn_subagent",
        &world.work,
        spawn_input(&world.work, "two-chairs-writer", "none"),
        None,
    );
    assert_static_deny(
        &evaluate(&controlled, None, NOW, true),
        Denial::MissingGrant,
    );
    let mut truncated: Value = serde_json::from_slice(&controlled).unwrap();
    truncated["toolInputTruncated"] = json!(true);
    assert_static_deny(
        &evaluate(
            &serde_json::to_vec(&truncated).unwrap(),
            Some(&world.state),
            NOW,
            true,
        ),
        Denial::MalformedHook,
    );
    assert_static_deny(
        &evaluate(b"{", Some(&world.state), NOW, true),
        Denial::MalformedHook,
    );
}

#[test]
fn capacity_depth_roles_overrides_and_runtime_lock_are_closed() {
    let world = World::new();
    let second = world.other_work("work-b");
    let third = world.other_work("work-c");
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    create_grant(&world.state, draft(&world, "child-1", 1, &second)).unwrap();
    let mut third_draft = draft(&world, "child-2", 0, &third);
    third_draft.lane_id = "grok-child-2".to_string();
    assert_eq!(
        create_grant(&world.state, third_draft).unwrap_err().denial,
        Denial::ChildSlot
    );

    let wrong_role = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&world.work, "two-chairs-reviewer", "none"),
            None,
        ),
        &world,
        NOW,
    );
    assert_static_deny(&wrong_role, Denial::Role);
    let wrong_isolation = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&world.work, "two-chairs-writer", "worktree"),
            None,
        ),
        &world,
        NOW,
    );
    assert_static_deny(&wrong_isolation, Denial::Isolation);
    let mut model_override = spawn_input(&world.work, "two-chairs-writer", "none");
    model_override["model"] = json!("grok-4.7");
    let response = hook(
        &event("spawn_subagent", &world.work, model_override, None),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::Uncontrolled);
    let response = evaluate(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&world.work, "two-chairs-writer", "none"),
            None,
        ),
        Some(&world.state),
        NOW,
        false,
    );
    assert_static_deny(&response, Denial::Model);
    let grandchild = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&second, "two-chairs-writer", "none"),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW,
    );
    assert_static_deny(&grandchild, Denial::Depth);
}

#[test]
fn path_grants_reject_escape_unknown_fields_reparse_and_hardlinks() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    claim(&world, &world.work, "two-chairs-writer");

    let escaped = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path":"../outside.rs","old_string":"a","new_string":"b"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW + 1,
    );
    assert_static_deny(&escaped, Denial::Path);
    let widened = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path":"src/lib.rs","old_string":"a","new_string":"b","other_path":"elsewhere"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW + 2,
    );
    assert_static_deny(&widened, Denial::Uncontrolled);

    let link = world.work.join("link");
    let created = std::os::windows::fs::symlink_dir(world.work.join("src"), &link).is_ok()
        || Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(world.work.join("src"))
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
    assert!(created);
    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path":"link/lib.rs","old_string":"a","new_string":"b"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW + 3,
    );
    assert_static_deny(&response, Denial::Path);
    let _ = fs::remove_dir(&link);

    let hardlink = world.work.join("src/hardlink.rs");
    fs::hard_link(world.work.join("src/lib.rs"), &hardlink).unwrap();
    let response = hook(
        &event(
            "search_replace",
            &world.work,
            json!({"file_path":"src/hardlink.rs","old_string":"a","new_string":"b"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW + 4,
    );
    assert_static_deny(&response, Denial::Path);
}

#[test]
fn consumed_stale_and_git_drift_fail_closed() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    consume_grant(&world.state, "child-0", NOW).unwrap();
    assert_eq!(
        consume_grant(&world.state, "child-0", NOW + 1)
            .unwrap_err()
            .denial,
        Denial::ConsumedGrant
    );

    let second = world.other_work("work-b");
    let mut expired = draft(&world, "expired", 1, &second);
    expired.issued_at_ms = 1_000;
    expired.expires_at_ms = 2_000;
    create_grant(&world.state, expired).unwrap();
    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&second, "two-chairs-writer", "none"),
            None,
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::StaleGrant);

    let third = world.other_work("work-c");
    create_grant(&world.state, draft(&world, "drift", 0, &third)).unwrap();
    fs::write(third.join("drift.txt"), b"drift\n").unwrap();
    git(&third, &["add", "drift.txt"]);
    git(&third, &["commit", "-m", "drift"]);
    let response = hook(
        &event(
            "spawn_subagent",
            &world.work,
            spawn_input(&third, "two-chairs-writer", "none"),
            None,
        ),
        &world,
        NOW,
    );
    assert_static_deny(&response, Denial::StaleGrant);
}

#[test]
fn unrelated_tools_are_untouched_and_valid_pass_through_is_silent() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    let elsewhere = world.repo.clone();
    let unrelated = hook(
        &event(
            "run_terminal_command",
            &elsewhere,
            json!({"command":"git status"}),
            None,
        ),
        &world,
        NOW,
    );
    assert_eq!(unrelated.decision, HookDecision::Silent);
    assert!(unrelated.stdout.is_none());

    claim(&world, &world.work, "two-chairs-writer");
    let valid = hook(
        &event(
            "read_file",
            &world.work,
            json!({"target_file":"src/lib.rs"}),
            Some("two-chairs-writer"),
        ),
        &world,
        NOW + 1,
    );
    assert_eq!(valid.decision, HookDecision::PassThrough);
    assert!(valid.stdout.is_none());
}

#[test]
fn hook_binary_never_echoes_payload_or_grants_authority() {
    let world = World::new();
    create_grant(&world.state, draft(&world, "child-0", 0, &world.work)).unwrap();
    claim(&world, &world.work, "two-chairs-writer");
    let payload = event(
        "search_replace",
        &world.work,
        json!({
            "file_path":"src/lib.rs",
            "old_string":"kept",
            "new_string":SENTINEL
        }),
        Some("two-chairs-writer"),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_parley-lane-hook"))
        .env("PARLEY_LANE_STATE_DIR", &world.state)
        .env("GROK_SUBAGENT_MODEL_INHERITANCE", "1")
        .env("GROK_SUBAGENT_SAMPLING_LIMIT", "2")
        .env("GROK_ACTIVE_AGENT_MESSAGES", "0")
        .env("GROK_WORKFLOWS", "0")
        .env("GROK_MEMORY", "0")
        .env("PARLEY_GROK_LOCKED_MODEL", "grok-4.7")
        .env("PARLEY_GROK_LOCKED_REASONING_EFFORT", "xhigh")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.as_mut().unwrap().write_all(&payload).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
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
