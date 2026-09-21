use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::harness::Invocation;
use crate::jobs::{GrantKind as PlanGrantKind, LaneGrant, LaneOwner, LanePlan, LaneRole};
use crate::json::Json;
use crate::lane_grants::{
    ChildRole, GrantDraft, GrantKind, GrantSet, PathGrant as StoredPathGrant,
};
use crate::policy::RuntimePolicy;

const PARENT_NAME: &str = "two-chairs-parent";
const WRITER_NAME: &str = "two-chairs-writer";
const REVIEWER_NAME: &str = "two-chairs-reviewer";
const PARENT_DENIES: &str = "run_terminal_cmd,run_terminal_command,web_search,web_fetch,search_tool,use_tool,workflow,send_subagent_message,scheduler_create,scheduler_delete,scheduler_list,monitor,update_goal,todo_write,ask_user_question,enter_plan_mode,exit_plan_mode";

#[derive(Clone, Debug)]
struct ChildSpec {
    grant_id: String,
    lane_id: String,
    role: ChildRole,
    cwd: PathBuf,
    writable_paths: Vec<StoredPathGrant>,
}

#[derive(Clone, Debug)]
pub(crate) struct GuardedSubagentLaunch {
    state_dir: PathBuf,
    hook_exe: PathBuf,
    model: String,
    reasoning_effort: String,
    base_commit: String,
    parent_cwd: PathBuf,
    parent_writable_paths: Vec<StoredPathGrant>,
    protected_roots: Vec<PathBuf>,
    children: Vec<ChildSpec>,
}

impl GuardedSubagentLaunch {
    pub(crate) fn from_plan(
        job_id: &str,
        plan: Option<&LanePlan>,
        policy: &RuntimePolicy,
    ) -> Result<Option<Self>, String> {
        let Some(plan) = plan else {
            return Ok(None);
        };
        let mut children = plan
            .lanes
            .iter()
            .filter(|lane| lane.owner == LaneOwner::GrokChild)
            .collect::<Vec<_>>();
        if children.is_empty() {
            return Ok(None);
        }
        children.sort_by(|left, right| left.lane_id.cmp(&right.lane_id));
        let guarded = policy.guarded_subagents().ok_or_else(|| {
            "Grok child lanes require PARLEY_GROK_SUBAGENT_MODE=guarded".to_string()
        })?;
        let parent = plan
            .lanes
            .iter()
            .find(|lane| lane.owner == LaneOwner::GrokParent)
            .ok_or_else(|| "guarded Grok launch has no parent lane".to_string())?;
        let parent_writable_paths = stored_path_grants(parent)?;
        let writer_count = children
            .iter()
            .filter(|lane| lane.role == LaneRole::Writer)
            .count();
        if children.len() > 2 || writer_count > guarded.max_writers {
            return Err("guarded Grok jobs permit at most two native child writers".to_string());
        }
        let state_dir = guarded
            .lane_state_dir
            .to_str()
            .ok_or_else(|| "lane state directory is not valid Unicode".to_string())?;
        let hook_exe = guarded
            .hook_exe
            .to_str()
            .ok_or_else(|| "lane hook executable path is not valid Unicode".to_string())?;
        let hook_now = fs::canonicalize(&guarded.hook_exe).map_err(|error| {
            format!(
                "canonicalize lane hook executable {}: {error}",
                guarded.hook_exe.display()
            )
        })?;
        if path_key(&hook_now) != path_key(&guarded.hook_exe) {
            return Err("lane hook executable identity changed during job preparation".to_string());
        }
        let mut specs = Vec::with_capacity(children.len());
        for (slot, lane) in children.into_iter().enumerate() {
            let role = match lane.role {
                LaneRole::Writer => ChildRole::Writer,
                LaneRole::Reviewer => ChildRole::Reviewer,
            };
            let writable_paths = stored_path_grants(lane)?;
            specs.push(ChildSpec {
                grant_id: format!("{job_id}-c{slot}"),
                lane_id: lane.lane_id.clone(),
                role,
                cwd: lane.cwd.clone(),
                writable_paths,
            });
        }
        let mut protected_roots = vec![plan.integration_cwd.clone()];
        protected_roots.extend(
            plan.lanes
                .iter()
                .filter(|lane| {
                    lane.owner == LaneOwner::Codex
                        || (lane.owner == LaneOwner::GrokChild && lane.role == LaneRole::Reviewer)
                })
                .map(|lane| lane.cwd.clone()),
        );
        Ok(Some(Self {
            state_dir: PathBuf::from(state_dir),
            hook_exe: PathBuf::from(hook_exe),
            model: guarded.model.clone(),
            reasoning_effort: guarded.reasoning_effort.clone(),
            base_commit: plan.base_commit.to_ascii_lowercase(),
            parent_cwd: parent.cwd.clone(),
            parent_writable_paths,
            protected_roots,
            children: specs,
        }))
    }

    pub(crate) fn parent_profile_contents(&self) -> String {
        parent_agent_profile()
    }

    pub(crate) fn configure_invocation(
        &self,
        invocation: &mut Invocation,
        parent_profile: &Path,
    ) -> Result<(), String> {
        if !invocation.command.eq_ignore_ascii_case("grok") {
            return Err("guarded subagents are supported only for Grok".to_string());
        }
        if !parent_profile.is_absolute() {
            return Err("guarded parent profile path must be absolute".to_string());
        }
        let parent_profile = parent_profile
            .to_str()
            .ok_or_else(|| "guarded parent profile path is not valid Unicode".to_string())?;
        for argument in &invocation.args {
            let lower = argument.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "--agents" | "--agent" | "--tools" | "--disallowed-tools" | "--no-subagents"
            ) || lower.starts_with("--agents=")
                || lower.starts_with("--agent=")
                || lower.starts_with("--tools=")
                || lower.starts_with("--disallowed-tools=")
            {
                return Err(format!(
                    "guarded Grok launch conflicts with existing argument {argument}"
                ));
            }
        }
        let insertion = invocation
            .args
            .iter()
            .position(|argument| argument == "--no-auto-update")
            .map(|index| index + 1)
            .unwrap_or(0);
        let mut immutable_arguments = vec![
            "--agents".to_string(),
            agent_definitions_json(),
            "--agent".to_string(),
            parent_profile.to_string(),
            "--disallowed-tools".to_string(),
            PARENT_DENIES.to_string(),
        ];
        append_permission_allows(
            &mut immutable_arguments,
            &self.parent_cwd,
            &self.parent_writable_paths,
        )?;
        for child in &self.children {
            if child.role != ChildRole::Writer {
                continue;
            }
            append_permission_allows(&mut immutable_arguments, &child.cwd, &child.writable_paths)?;
        }
        for root in &self.protected_roots {
            let pattern = permission_tree_pattern(root)?;
            for prefix in ["Edit", "Write"] {
                immutable_arguments.push("--deny".to_string());
                immutable_arguments.push(format!("{prefix}({pattern})"));
            }
        }
        invocation
            .args
            .splice(insertion..insertion, immutable_arguments);
        let state_dir = self
            .state_dir
            .to_str()
            .ok_or_else(|| "lane state directory is not valid Unicode".to_string())?;
        let hook_exe = self
            .hook_exe
            .to_str()
            .ok_or_else(|| "lane hook executable is not valid Unicode".to_string())?;
        for (name, value) in [
            ("PARLEY_LANE_STATE_DIR", state_dir),
            ("PARLEY_GROK_LANE_HOOK_EXE", hook_exe),
            ("PARLEY_GROK_LOCKED_MODEL", self.model.as_str()),
            (
                "PARLEY_GROK_LOCKED_REASONING_EFFORT",
                self.reasoning_effort.as_str(),
            ),
            ("GROK_SUBAGENT_MODEL_INHERITANCE", "1"),
            ("GROK_SUBAGENT_SAMPLING_LIMIT", "2"),
            ("GROK_ACTIVE_AGENT_MESSAGES", "0"),
            ("GROK_WORKFLOWS", "0"),
            ("GROK_MEMORY", "0"),
        ] {
            invocation.env.insert(name.to_string(), value.to_string());
        }
        Ok(())
    }

    pub(crate) fn activate(&self, overall_timeout: Duration) -> Result<GrantSet, String> {
        let issued_at_ms = now_ms()?;
        let lifetime_ms = overall_timeout
            .saturating_add(Duration::from_secs(300))
            .as_millis();
        let lifetime_ms = u64::try_from(lifetime_ms)
            .map_err(|_| "guarded grant lifetime is out of range".to_string())?;
        let expires_at_ms = issued_at_ms
            .checked_add(lifetime_ms)
            .ok_or_else(|| "guarded grant expiry is out of range".to_string())?;
        let mut drafts = Vec::with_capacity(self.children.len());
        for (slot, child) in self.children.iter().enumerate() {
            let common_dir = git_common_dir(&child.cwd)?;
            drafts.push(GrantDraft {
                grant_id: child.grant_id.clone(),
                lane_id: child.lane_id.clone(),
                role: child.role,
                canonical_cwd: child.cwd.clone(),
                worktree_common_dir: common_dir,
                base_commit: self.base_commit.clone(),
                path_grants: child.writable_paths.clone(),
                model: self.model.clone(),
                reasoning_effort: self.reasoning_effort.clone(),
                child_slot: slot as u64,
                issued_at_ms,
                expires_at_ms,
            });
        }
        GrantSet::create(&self.state_dir, &drafts)
    }
}

fn stored_path_grants(lane: &LaneGrant) -> Result<Vec<StoredPathGrant>, String> {
    lane.writable_paths
        .iter()
        .map(|grant| {
            let path = grant.path.to_str().ok_or_else(|| {
                format!("lane {} contains a non-Unicode path grant", lane.lane_id)
            })?;
            Ok(StoredPathGrant {
                kind: match grant.kind {
                    PlanGrantKind::File => GrantKind::File,
                    PlanGrantKind::Tree => GrantKind::Tree,
                },
                path: path.to_string(),
            })
        })
        .collect()
}

fn append_permission_allows(
    arguments: &mut Vec<String>,
    cwd: &Path,
    writable_paths: &[StoredPathGrant],
) -> Result<(), String> {
    for grant in writable_paths {
        let pattern = permission_pattern(cwd, grant)?;
        for prefix in ["Edit", "Write"] {
            arguments.push("--allow".to_string());
            arguments.push(format!("{prefix}({pattern})"));
        }
    }
    Ok(())
}

pub(crate) fn append_parent_contract(prompt: &mut String, plan: &LanePlan) {
    let mut lanes = plan.lanes.iter().collect::<Vec<_>>();
    lanes.sort_by(|left, right| left.lane_id.cmp(&right.lane_id));
    let mut contract = String::from("\n\nTWO CHAIRS PARALLEL LANE CONTRACT\n");
    contract.push_str("The lane manifest below is an immutable execution boundary, not authority to expand the task. The integration worktree remains read-only. Spawn each declared Grok child exactly once with its exact subagent_type, cwd, isolation=none, and no model, effort, tool, MCP, sandbox, hook, resume, or permission override. Child prompts must repeat only the bounded assignment and exact granted paths.\n");
    contract.push_str(&format!("base_commit: {}\n", plan.base_commit));
    contract.push_str(&format!(
        "integration_worktree: {}\n",
        plan.integration_cwd.display()
    ));
    for lane in lanes {
        contract.push_str(&format!(
            "lane: {} owner={} role={} worktree={}\n",
            lane.lane_id,
            lane.owner.as_str(),
            lane.role.as_str(),
            lane.cwd.display()
        ));
        if lane.writable_paths.is_empty() {
            contract.push_str("  writable_paths: none\n");
        } else {
            for grant in &lane.writable_paths {
                contract.push_str(&format!(
                    "  writable_path: {}:{}\n",
                    grant.kind.as_str(),
                    grant.path.display()
                ));
            }
        }
    }
    contract.push_str("The final response must enumerate every child ID, role, branch/worktree, granted paths, changed paths, tests, failures or denials, and the parent\'s independent review. Do not claim a child result you did not receive.\n");
    let handoff_marker = "\n\nTWO CHAIRS LOCKED RESPONSE CONTRACT\n";
    if let Some(index) = prompt.rfind(handoff_marker) {
        prompt.insert_str(index, &contract);
    } else {
        prompt.push_str(&contract);
    }
}

fn agent_definitions_json() -> String {
    let mut definitions = BTreeMap::new();
    definitions.insert(
        WRITER_NAME.to_string(),
        agent_definition(
            "Bounded Two Chairs writer",
            "read-write",
            &["read_file", "list_dir", "grep", "search_replace", "write"],
        ),
    );
    definitions.insert(
        REVIEWER_NAME.to_string(),
        agent_definition(
            "Bounded Two Chairs reviewer",
            "read-only",
            &["read_file", "list_dir", "grep"],
        ),
    );
    Json::Object(definitions).to_compact_string()
}

fn parent_agent_profile() -> String {
    let mut profile = format!(
        "---\nname: {PARENT_NAME}\ndescription: Guarded Two Chairs parent\npromptMode: extend\n"
    );
    profile.push_str("tools:\n");
    for tool in [
        "read_file",
        "list_dir",
        "grep",
        "search_replace",
        "write",
        "task",
        "Agent(two-chairs-writer, two-chairs-reviewer)",
        "kill_command_or_subagent",
        "get_command_or_subagent_output",
        "wait_commands_or_subagents",
    ] {
        profile.push_str(&format!("  - \"{tool}\"\n"));
    }
    profile.push_str("disallowedTools:\n");
    for tool in PARENT_DENIES.split(',') {
        profile.push_str(&format!("  - \"{tool}\"\n"));
    }
    profile.push_str(
        "agentsMd: false\ndiscoverSkills: false\ninheritSkills: false\ninjectDefaultTools: false\nmcpInheritance: none\nmodel: inherit\n---\nYou are the guarded Two Chairs parent. Work only inside the current lane contract. Spawn only the explicitly granted two-chairs-writer or two-chairs-reviewer children, always with the exact required subagent_type, cwd, and isolation:none. Independently review every child result and report all child IDs, grants, changes, tests, failures, and denials.\n",
    );
    profile
}

fn agent_definition(description: &str, capability: &str, tools: &[&str]) -> Json {
    let mut definition = BTreeMap::new();
    definition.insert("agentsMd".to_string(), Json::Bool(false));
    definition.insert(
        "capabilityMode".to_string(),
        Json::Str(capability.to_string()),
    );
    definition.insert(
        "description".to_string(),
        Json::Str(description.to_string()),
    );
    definition.insert("discoverSkills".to_string(), Json::Bool(false));
    definition.insert(
        "disallowedTools".to_string(),
        Json::Array(
            [
                "run_terminal_command",
                "run_terminal_cmd",
                "web_search",
                "web_fetch",
                "search_tool",
                "use_tool",
                "spawn_subagent",
                "task",
                "Agent",
                "send_subagent_message",
                "workflow",
            ]
            .into_iter()
            .map(|tool| Json::Str(tool.to_string()))
            .collect(),
        ),
    );
    definition.insert("inheritSkills".to_string(), Json::Bool(false));
    definition.insert("injectDefaultTools".to_string(), Json::Bool(false));
    definition.insert("isolation".to_string(), Json::Str("none".to_string()));
    definition.insert("mcpInheritance".to_string(), Json::Str("none".to_string()));
    definition.insert("model".to_string(), Json::Str("inherit".to_string()));
    definition.insert(
        "permissionMode".to_string(),
        Json::Str("dontAsk".to_string()),
    );
    definition.insert(
        "prompt".to_string(),
        Json::Str("You are a depth-one Two Chairs child. Follow only the current bounded lane assignment. Use no shell, web, MCP, Task, workflow, memory, or authority outside the exact worktree and path grants. Report changed paths, checks performed, failures, and denials exactly.".to_string()),
    );
    definition.insert(
        "tools".to_string(),
        Json::Array(
            tools
                .iter()
                .map(|tool| Json::Str((*tool).to_string()))
                .collect(),
        ),
    );
    Json::Object(definition)
}

fn permission_pattern(cwd: &Path, grant: &StoredPathGrant) -> Result<String, String> {
    let target = grant
        .path
        .split('/')
        .fold(cwd.to_path_buf(), |path, part| path.join(part));
    let mut pattern = permission_path(&target)?;
    if grant.kind == GrantKind::Tree {
        pattern.push_str("/**");
    }
    Ok(pattern)
}

fn permission_tree_pattern(path: &Path) -> Result<String, String> {
    let mut pattern = permission_path(path)?;
    pattern.push_str("/**");
    Ok(pattern)
}

fn permission_path(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or_else(|| format!("permission path is not valid Unicode: {}", path.display()))?;
    let text = normalize_windows_extended_path(text).replace('\\', "/");
    if text.chars().any(|character| {
        matches!(
            character,
            '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '\r' | '\n'
        )
    }) {
        return Err(format!(
            "permission path contains unsupported pattern syntax: {}",
            path.display()
        ));
    }
    Ok(text)
}

fn normalize_windows_extended_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

fn git_common_dir(cwd: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("resolve child Git common-dir: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "resolve child Git common-dir in {} failed",
            cwd.display()
        ));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "child Git common-dir is not UTF-8".to_string())?;
    fs::canonicalize(text.trim())
        .map_err(|error| format!("canonicalize child Git common-dir: {error}"))
}

fn now_ms() -> Result<u64, String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_string())?;
    u64::try_from(duration.as_millis()).map_err(|_| "system clock is out of range".to_string())
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> GuardedSubagentLaunch {
        GuardedSubagentLaunch {
            state_dir: PathBuf::from(r"C:\state\lanes"),
            hook_exe: PathBuf::from(r"C:\Program Files\Parley\parley-lane-hook.exe"),
            model: "grok-4.7".to_string(),
            reasoning_effort: "xhigh".to_string(),
            base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            parent_cwd: PathBuf::from(r"C:\repo\parent"),
            parent_writable_paths: vec![StoredPathGrant {
                kind: GrantKind::File,
                path: "README.md".to_string(),
            }],
            protected_roots: vec![PathBuf::from(r"C:\repo\integration")],
            children: Vec::new(),
        }
    }

    #[test]
    fn agent_definitions_are_closed_and_inherit_the_model() {
        let value = Json::parse(&agent_definitions_json()).unwrap();
        assert!(value.get(PARENT_NAME).is_none());
        let writer = value.get(WRITER_NAME).unwrap();
        assert_eq!(
            writer.get("capabilityMode").and_then(Json::as_str),
            Some("read-write")
        );
        assert_eq!(writer.get("model").and_then(Json::as_str), Some("inherit"));
        assert_eq!(
            writer.get("mcpInheritance").and_then(Json::as_str),
            Some("none")
        );
        assert_eq!(
            writer.get("injectDefaultTools").and_then(Json::as_bool),
            Some(false)
        );
        assert!(!writer
            .get("tools")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .any(|tool| tool.as_str() == Some("run_terminal_command")));
    }

    #[test]
    fn parent_profile_is_file_selectable_and_closes_authority() {
        let profile = launch().parent_profile_contents();
        assert!(profile.starts_with("---\nname: two-chairs-parent\n"));
        assert!(profile.contains("  - \"task\"\n"));
        assert!(profile.contains("  - \"Agent(two-chairs-writer, two-chairs-reviewer)\"\n"));
        assert!(profile.contains("  - \"kill_command_or_subagent\"\n"));
        assert!(profile.contains("  - \"run_terminal_cmd\"\n"));
        assert!(profile.contains("injectDefaultTools: false\n"));
        assert!(profile.contains("mcpInheritance: none\n"));
        assert!(profile.contains("model: inherit\n"));
        assert!(!profile.contains("permissionMode:"));
        assert!(!profile.contains("bypassPermissions"));
    }

    #[test]
    fn invocation_receives_immutable_catalog_and_environment() {
        let mut invocation = Invocation::new(
            "grok",
            vec![
                "--no-auto-update".to_string(),
                "--single".to_string(),
                "task".to_string(),
            ],
        );
        launch()
            .configure_invocation(&mut invocation, Path::new(r"C:\Temp\parent.md"))
            .unwrap();
        assert!(invocation
            .args
            .iter()
            .any(|argument| argument == "--agents"));
        assert!(invocation
            .args
            .windows(2)
            .any(|arguments| arguments == ["--agent", r"C:\Temp\parent.md"]));
        assert!(!invocation.args.iter().any(|argument| argument == "--tools"));
        assert!(invocation
            .args
            .windows(2)
            .any(|arguments| arguments == ["--disallowed-tools", PARENT_DENIES]));
        assert_eq!(
            invocation.env.get("GROK_SUBAGENT_MODEL_INHERITANCE"),
            Some(&"1".to_string())
        );
        assert!(!invocation
            .args
            .iter()
            .any(|argument| argument == "--no-subagents"));
    }

    #[test]
    fn invocation_rejects_preexisting_authority_switches() {
        let mut invocation = Invocation::new("grok", vec!["--tools=all".to_string()]);
        assert!(launch()
            .configure_invocation(&mut invocation, Path::new(r"C:\Temp\parent.md"))
            .is_err());
    }

    #[test]
    fn invocation_rejects_relative_parent_profiles() {
        let mut invocation = Invocation::new("grok", vec!["--no-auto-update".to_string()]);
        assert!(launch()
            .configure_invocation(&mut invocation, Path::new("parent.md"))
            .is_err());
    }

    #[test]
    fn writer_grants_add_exact_dont_ask_allows_and_protected_root_denies() {
        let mut launch = launch();
        launch.children.push(ChildSpec {
            grant_id: "job-c0".to_string(),
            lane_id: "child".to_string(),
            role: ChildRole::Writer,
            cwd: PathBuf::from(r"C:\repo\child"),
            writable_paths: vec![StoredPathGrant {
                kind: GrantKind::Tree,
                path: "src/jobs".to_string(),
            }],
        });
        let mut invocation = Invocation::new("grok", vec!["--no-auto-update".to_string()]);
        launch
            .configure_invocation(&mut invocation, Path::new(r"C:\Temp\parent.md"))
            .unwrap();
        assert!(invocation
            .args
            .iter()
            .any(|argument| argument == "Edit(C:/repo/parent/README.md)"));
        assert!(invocation
            .args
            .iter()
            .any(|argument| argument == "Write(C:/repo/parent/README.md)"));
        assert!(invocation
            .args
            .iter()
            .any(|argument| argument == "Edit(C:/repo/child/src/jobs/**)"));
        assert!(invocation
            .args
            .iter()
            .any(|argument| argument == "Write(C:/repo/child/src/jobs/**)"));
        assert!(invocation
            .args
            .iter()
            .any(|argument| argument == "Edit(C:/repo/integration/**)"));
        assert!(invocation
            .args
            .iter()
            .any(|argument| argument == "Write(C:/repo/integration/**)"));
    }

    #[test]
    fn permission_patterns_reject_rule_metacharacters() {
        let grant = StoredPathGrant {
            kind: GrantKind::File,
            path: "safe.rs".to_string(),
        };
        assert!(permission_pattern(Path::new(r"C:\repo(parent)"), &grant).is_err());
    }

    #[test]
    fn permission_patterns_strip_only_the_windows_extended_prefix() {
        let grant = StoredPathGrant {
            kind: GrantKind::File,
            path: "README.md".to_string(),
        };
        assert_eq!(
            permission_pattern(Path::new(r"\\?\C:\repo\parent"), &grant).unwrap(),
            "C:/repo/parent/README.md"
        );
        assert_eq!(
            permission_pattern(Path::new(r"\\?\UNC\server\share\repo"), &grant).unwrap(),
            "//server/share/repo/README.md"
        );
        assert!(permission_pattern(Path::new(r"C:\repo?\parent"), &grant).is_err());
    }
}
