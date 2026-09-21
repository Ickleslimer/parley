use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{Denial, LaneError};
use crate::fsutil::strip_bom;
use crate::schema::{Access, STATE_ENV};
use crate::store::{self, bind_grant_for_child, claim_grant_for_spawn, find_grant_for_cwd};
use crate::validate::{validate_path, PathClaim, SpawnClaim};

const MAX_HOOK_STDIN: usize = 256 * 1024;

pub struct HookRequest<'a> {
    pub payload: &'a [u8],
    pub state_dir: Option<&'a Path>,
    pub model_inheritance: Option<&'a str>,
    pub sampling_limit: Option<&'a str>,
    pub active_agent_messages: Option<&'a str>,
    pub workflows: Option<&'a str>,
    pub memory: Option<&'a str>,
    pub locked_model: Option<&'a str>,
    pub locked_reasoning_effort: Option<&'a str>,
    pub now_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookDecision {
    Silent,
    PassThrough,
    Deny(Denial),
}

pub struct HookResponse {
    pub decision: HookDecision,
    pub stdout: Option<String>,
}

pub fn evaluate(request: HookRequest<'_>) -> HookResponse {
    match decide(&request) {
        Ok(HookDecision::Silent) => HookResponse {
            decision: HookDecision::Silent,
            stdout: None,
        },
        Ok(HookDecision::PassThrough) => HookResponse {
            decision: HookDecision::PassThrough,
            stdout: None,
        },
        Ok(HookDecision::Deny(denial)) => deny_response(denial),
        Err(error) => deny_response(error.denial),
    }
}

pub fn run() -> i32 {
    let payload = match read_stdin() {
        Ok(payload) => payload,
        Err(error) => return emit(deny_response(error.denial).stdout.as_deref()),
    };
    let state_dir = match std::env::var(STATE_ENV) {
        Ok(value) if value.is_empty() || !Path::new(&value).is_absolute() => {
            return emit(deny_response(Denial::MalformedGrant).stdout.as_deref());
        }
        Ok(value) => Some(value),
        Err(_) => None,
    };
    let now_ms = match now_ms() {
        Ok(now_ms) => now_ms,
        Err(error) => return emit(deny_response(error.denial).stdout.as_deref()),
    };
    let model_inheritance = std::env::var("GROK_SUBAGENT_MODEL_INHERITANCE").ok();
    let sampling_limit = std::env::var("GROK_SUBAGENT_SAMPLING_LIMIT").ok();
    let active_agent_messages = std::env::var("GROK_ACTIVE_AGENT_MESSAGES").ok();
    let workflows = std::env::var("GROK_WORKFLOWS").ok();
    let memory = std::env::var("GROK_MEMORY").ok();
    let locked_model = std::env::var("PARLEY_GROK_LOCKED_MODEL").ok();
    let locked_reasoning_effort = std::env::var("PARLEY_GROK_LOCKED_REASONING_EFFORT").ok();
    let response = evaluate(HookRequest {
        payload: &payload,
        state_dir: state_dir.as_deref().map(Path::new),
        model_inheritance: model_inheritance.as_deref(),
        sampling_limit: sampling_limit.as_deref(),
        active_agent_messages: active_agent_messages.as_deref(),
        workflows: workflows.as_deref(),
        memory: memory.as_deref(),
        locked_model: locked_model.as_deref(),
        locked_reasoning_effort: locked_reasoning_effort.as_deref(),
        now_ms,
    });
    emit(response.stdout.as_deref())
}

fn decide(request: &HookRequest<'_>) -> Result<HookDecision, LaneError> {
    if request.payload.is_empty() || request.payload.len() > MAX_HOOK_STDIN {
        return Err(LaneError::new(
            Denial::MalformedHook,
            "hook payload is empty or oversized",
        ));
    }
    let value: Value = serde_json::from_slice(strip_bom(request.payload))
        .map_err(|_| LaneError::new(Denial::MalformedHook, "hook payload is not JSON"))?;
    let Some(event) = value.as_object() else {
        return Err(LaneError::new(
            Denial::MalformedHook,
            "hook payload is not an object",
        ));
    };
    if !is_pretool(event)? {
        return Ok(HookDecision::Silent);
    }
    if optional_bool(event, &["toolInputTruncated", "tool_input_truncated"]) != Some(false) {
        return Err(LaneError::new(
            Denial::MalformedHook,
            "tool input truncation flag is missing or true",
        ));
    }
    let session_id = required_string(event, &["sessionId", "session_id"])?;
    let tool_use_id = required_string(event, &["toolUseId", "tool_use_id"])?;
    let tool_name = required_string(event, &["tool_name", "toolName"])?;
    let class = classify_tool(&tool_name);
    let tool_input = tool_input(event)?;
    let actor_role = optional_string(event, &["subagent_type", "subagentType"])?;
    let controlled_actor = actor_role.as_deref().is_some_and(is_controlled_role);
    let controlled_spawn = class == ToolClass::Spawn
        && optional_string(
            &tool_input,
            &["subagent_type", "subagentType", "agent_type", "agentType"],
        )?
        .as_deref()
        .is_some_and(is_controlled_role);
    let Some(state_dir) = request.state_dir else {
        if controlled_actor || controlled_spawn {
            return Err(LaneError::new(
                Denial::MissingGrant,
                "controlled lane operation has no state directory",
            ));
        }
        return Ok(HookDecision::Silent);
    };
    if !state_dir.is_absolute() {
        return Err(LaneError::new(
            Denial::MalformedGrant,
            "lane state dir must be absolute",
        ));
    }
    if class == ToolClass::Spawn && actor_role.is_some() {
        return Err(LaneError::new(
            Denial::Depth,
            "grandchild spawn is rejected",
        ));
    }
    if class == ToolClass::Spawn {
        return decide_spawn(request, state_dir, &tool_input, &session_id, &tool_use_id);
    }
    let cwd = required_string(event, &["cwd"])?;
    let cwd_path = PathBuf::from(&cwd);
    if controlled_actor {
        let role = actor_role.as_deref().expect("controlled actor has a role");
        let grant = bind_grant_for_child(state_dir, &cwd_path, role, &session_id, request.now_ms)?;
        validate_runtime_lock(request, &grant)?;
        return match class {
            ToolClass::Read | ToolClass::Write => {
                let access = if class == ToolClass::Read {
                    Access::Read
                } else {
                    Access::Write
                };
                let path = operation_path(&tool_name, &tool_input)?;
                validate_path(
                    &grant,
                    &PathClaim {
                        role,
                        session_id: &session_id,
                        cwd: &cwd_path,
                        access,
                        path: &path,
                    },
                    request.now_ms,
                )?;
                Ok(HookDecision::PassThrough)
            }
            ToolClass::Spawn => unreachable!("spawn is handled above"),
            ToolClass::Forbidden | ToolClass::Unknown => Err(LaneError::new(
                Denial::Uncontrolled,
                "tool is outside the child tool catalog",
            )),
        };
    }
    if find_grant_for_cwd(state_dir, &cwd_path, request.now_ms)?.is_some() {
        return Err(LaneError::new(
            Denial::Role,
            "a non-child session attempted to operate inside a child lane",
        ));
    }
    if matches!(class, ToolClass::Read | ToolClass::Write) {
        if let Ok(path) = operation_path(&tool_name, &tool_input) {
            let full = if path.is_absolute() {
                path
            } else {
                cwd_path.join(path)
            };
            if class == ToolClass::Write && full.is_file() && store::path_has_multiple_links(&full)?
            {
                return Err(LaneError::new(
                    Denial::Path,
                    "write target has multiple hard links",
                ));
            }
            if store::path_targets_lane(state_dir, &full)? {
                return Err(LaneError::new(Denial::Path, "path enters another lane"));
            }
        }
    }
    Ok(HookDecision::Silent)
}

fn is_controlled_role(role: &str) -> bool {
    matches!(
        role,
        crate::schema::WRITER_ROLE | crate::schema::REVIEWER_ROLE
    )
}

fn decide_spawn(
    request: &HookRequest<'_>,
    state_dir: &Path,
    tool_input: &Map<String, Value>,
    parent_session_id: &str,
    tool_use_id: &str,
) -> Result<HookDecision, LaneError> {
    validate_spawn_input(tool_input)?;
    if has_widening_field(tool_input) {
        return Err(LaneError::new(
            Denial::Uncontrolled,
            "spawn carries an authority-widening field",
        ));
    }
    let role = required_string(
        tool_input,
        &["subagent_type", "subagentType", "agent_type", "agentType"],
    )?;
    let cwd = required_string(tool_input, &["cwd"])?;
    let cwd_path = PathBuf::from(&cwd);
    let isolation = optional_string(tool_input, &["isolation"])?;
    let model = optional_string(tool_input, &["model", "model_id", "modelId"])?;
    let grant = match find_grant_for_cwd(state_dir, &cwd_path, request.now_ms)? {
        Some(grant) => grant,
        None => {
            if store::has_stored_grants(state_dir)? {
                return Err(LaneError::new(
                    Denial::Cwd,
                    "spawn cwd does not match a child grant",
                ));
            }
            return Err(LaneError::new(
                Denial::MissingGrant,
                "spawn cwd has no child grant",
            ));
        }
    };
    validate_runtime_lock(request, &grant)?;
    let claim = SpawnClaim {
        role: &role,
        cwd: &cwd_path,
        isolation: isolation.as_deref(),
        model: model.as_deref(),
        effort_overridden: has_any(
            tool_input,
            &["reasoning_effort", "reasoningEffort", "effort"],
        ),
        widening: false,
        requested_depth: 1,
        child_slot: None,
    };
    claim_grant_for_spawn(
        state_dir,
        &cwd_path,
        &claim,
        parent_session_id,
        tool_use_id,
        request.now_ms,
    )?;
    Ok(HookDecision::PassThrough)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolClass {
    Read,
    Write,
    Spawn,
    Forbidden,
    Unknown,
}

fn classify_tool(name: &str) -> ToolClass {
    match name.to_ascii_lowercase().as_str() {
        "read_file" | "read" | "grep" | "list_dir" | "glob" => ToolClass::Read,
        "search_replace" | "write" | "edit" | "multiedit" | "multi_edit" | "notebook_edit"
        | "delete_file" | "create_file" => ToolClass::Write,
        "spawn_subagent" | "task" => ToolClass::Spawn,
        "run_terminal_command"
        | "bash"
        | "shell"
        | "powershell"
        | "web_search"
        | "web_fetch"
        | "open_page"
        | "open_page_with_find"
        | "use_tool"
        | "mcp"
        | "send_subagent_message"
        | "workflow" => ToolClass::Forbidden,
        _ => ToolClass::Unknown,
    }
}

fn validate_runtime_lock(
    request: &HookRequest<'_>,
    grant: &crate::schema::GrantRecord,
) -> Result<(), LaneError> {
    let valid = request.model_inheritance == Some("1")
        && request.sampling_limit == Some("2")
        && request.active_agent_messages == Some("0")
        && request.workflows == Some("0")
        && request.memory == Some("0")
        && request.locked_model == Some(grant.model.as_str())
        && request.locked_reasoning_effort == Some(grant.reasoning_effort.as_str());
    if valid {
        Ok(())
    } else {
        Err(LaneError::new(
            Denial::Model,
            "locked child inheritance environment is missing or conflicting",
        ))
    }
}

fn validate_spawn_input(tool_input: &Map<String, Value>) -> Result<(), LaneError> {
    validate_keys(
        tool_input,
        &[
            "prompt",
            "description",
            "subagent_type",
            "background",
            "isolation",
            "cwd",
        ],
    )?;
    required_string(tool_input, &["prompt"])?;
    required_string(tool_input, &["description"])?;
    if let Some(value) = tool_input.get("background") {
        if !value.is_boolean() {
            return Err(LaneError::new(
                Denial::MalformedHook,
                "spawn background field is not boolean",
            ));
        }
    }
    Ok(())
}

fn operation_path(tool_name: &str, tool_input: &Map<String, Value>) -> Result<PathBuf, LaneError> {
    let (path_key, allowed): (&str, &[&str]) = match tool_name.to_ascii_lowercase().as_str() {
        "read_file" => (
            "target_file",
            &["target_file", "offset", "limit", "format", "pages"],
        ),
        "list_dir" => ("target_directory", &["target_directory"]),
        "grep" => (
            "path",
            &[
                "pattern",
                "path",
                "type",
                "glob",
                "output_mode",
                "-A",
                "-B",
                "-C",
                "-i",
                "multiline",
                "head_limit",
            ],
        ),
        "search_replace" => (
            "file_path",
            &["file_path", "old_string", "new_string", "replace_all"],
        ),
        "write" => ("filePath", &["filePath", "content"]),
        _ => {
            return Err(LaneError::new(
                Denial::Uncontrolled,
                "tool has no controlled path schema",
            ))
        }
    };
    validate_keys(tool_input, allowed)?;
    required_string(tool_input, &[path_key]).map(PathBuf::from)
}

fn validate_keys(map: &Map<String, Value>, allowed: &[&str]) -> Result<(), LaneError> {
    if map.keys().all(|key| allowed.contains(&key.as_str())) {
        Ok(())
    } else {
        Err(LaneError::new(
            Denial::Uncontrolled,
            "tool input contains an unsupported field",
        ))
    }
}

fn is_pretool(event: &Map<String, Value>) -> Result<bool, LaneError> {
    let pascal = optional_flag(event, "hook_event_name")?;
    let snake = optional_flag(event, "hookEventName")?;
    match (pascal, snake) {
        (None, None) => Err(LaneError::new(
            Denial::MalformedHook,
            "hook event name is missing",
        )),
        (Some(left), Some(right)) if left != right => Err(LaneError::new(
            Denial::MalformedHook,
            "hook event names disagree",
        )),
        (Some(flag), _) | (_, Some(flag)) => Ok(flag),
    }
}

fn optional_flag(event: &Map<String, Value>, key: &str) -> Result<Option<bool>, LaneError> {
    match event.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(is_pretool_name(value))),
        Some(_) => Err(LaneError::new(
            Denial::MalformedHook,
            "hook event name is not a string",
        )),
    }
}

fn is_pretool_name(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "pretooluse" | "pre_tool_use"
    )
}

fn tool_input(event: &Map<String, Value>) -> Result<Map<String, Value>, LaneError> {
    let snake = event.get("tool_input");
    let camel = event.get("toolInput");
    let selected = match (snake, camel) {
        (Some(left), Some(right)) if left != right => {
            return Err(LaneError::new(
                Denial::MalformedHook,
                "tool input aliases disagree",
            ));
        }
        (Some(value), _) | (_, Some(value)) => value,
        (None, None) => return Ok(Map::new()),
    };
    selected
        .as_object()
        .cloned()
        .ok_or_else(|| LaneError::new(Denial::MalformedHook, "tool input is not an object"))
}

fn required_string(map: &Map<String, Value>, keys: &[&str]) -> Result<String, LaneError> {
    optional_string(map, keys)?
        .ok_or_else(|| LaneError::new(Denial::MalformedHook, "required hook field is missing"))
}

fn optional_string(map: &Map<String, Value>, keys: &[&str]) -> Result<Option<String>, LaneError> {
    let mut found = None;
    for key in keys {
        if let Some(value) = map.get(*key) {
            let Some(text) = value.as_str() else {
                return Err(LaneError::new(
                    Denial::MalformedHook,
                    "hook field is not a string",
                ));
            };
            if text.is_empty() {
                return Err(LaneError::new(Denial::MalformedHook, "hook field is empty"));
            }
            match &found {
                None => found = Some(text.to_string()),
                Some(existing) if existing != text => {
                    return Err(LaneError::new(
                        Denial::MalformedHook,
                        "hook field aliases disagree",
                    ));
                }
                Some(_) => {}
            }
        }
    }
    Ok(found)
}

fn optional_bool(map: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    let mut found = None;
    for key in keys {
        if let Some(value) = map.get(*key) {
            let boolean = value.as_bool()?;
            match found {
                None => found = Some(boolean),
                Some(existing) if existing != boolean => return None,
                Some(_) => {}
            }
        }
    }
    found
}

fn has_widening_field(map: &Map<String, Value>) -> bool {
    has_any(
        map,
        &[
            "tools",
            "allowed_tools",
            "allowedTools",
            "disallowed_tools",
            "disallowedTools",
            "mcp",
            "mcp_servers",
            "mcpServers",
            "permission_mode",
            "permissionMode",
            "sandbox",
            "agents",
            "agent",
            "resume_from",
            "resumeFrom",
            "additional_directories",
            "hooks",
            "yolo",
            "always_approve",
        ],
    )
}

fn has_any(map: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter().any(|key| map.contains_key(*key))
}

fn deny_response(denial: Denial) -> HookResponse {
    HookResponse {
        decision: HookDecision::Deny(denial),
        stdout: Some(deny_json(denial)),
    }
}

fn deny_json(denial: Denial) -> String {
    let reason = denial.reason();
    let body = DenyBody {
        decision: "deny",
        reason,
        permission_decision: "deny",
        permission_decision_reason: reason,
        hook_specific_output: DenySpecific {
            hook_event_name: "PreToolUse",
            permission_decision: "deny",
            permission_decision_reason: reason,
        },
    };
    serde_json::to_string(&body).unwrap_or_else(|_| {
        r#"{"decision":"deny","reason":"parley-lane: deny malformed hook","permissionDecision":"deny","permissionDecisionReason":"parley-lane: deny malformed hook"}"#
            .to_string()
    })
}

#[derive(Serialize)]
struct DenyBody<'a> {
    decision: &'a str,
    reason: &'a str,
    #[serde(rename = "permissionDecision")]
    permission_decision: &'a str,
    #[serde(rename = "permissionDecisionReason")]
    permission_decision_reason: &'a str,
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: DenySpecific<'a>,
}

#[derive(Serialize)]
struct DenySpecific<'a> {
    #[serde(rename = "hookEventName")]
    hook_event_name: &'a str,
    #[serde(rename = "permissionDecision")]
    permission_decision: &'a str,
    #[serde(rename = "permissionDecisionReason")]
    permission_decision_reason: &'a str,
}

fn read_stdin() -> Result<Vec<u8>, LaneError> {
    let mut stdin = io::stdin().lock();
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stdin
            .read(&mut chunk)
            .map_err(|_| LaneError::new(Denial::MalformedHook, "hook stdin could not be read"))?;
        if read == 0 {
            break;
        }
        if buf.len() + read > MAX_HOOK_STDIN {
            return Err(LaneError::new(
                Denial::MalformedHook,
                "hook payload exceeds bound",
            ));
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    Ok(buf)
}

fn now_ms() -> Result<u64, LaneError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| LaneError::new(Denial::StaleGrant, "system clock is unavailable"))?;
    u64::try_from(duration.as_millis())
        .map_err(|_| LaneError::new(Denial::StaleGrant, "system clock is out of range"))
}

fn emit(line: Option<&str>) -> i32 {
    if let Some(line) = line {
        let mut out = io::stdout().lock();
        if writeln!(out, "{line}").is_err() || out.flush().is_err() {
            return 2;
        }
    }
    0
}
