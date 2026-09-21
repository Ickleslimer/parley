use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{Denial, LaneError};
use crate::fsutil::strip_bom;
use crate::schema::{Access, STATE_ENV};
use crate::store::{self, find_grant_for_cwd};
use crate::validate::{validate_path, validate_spawn, PathClaim, SpawnClaim};

const MAX_HOOK_STDIN: usize = 256 * 1024;

pub struct HookRequest<'a> {
    pub payload: &'a [u8],
    pub state_dir: Option<&'a Path>,
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
            stdout: Some(defer_json()),
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
    let response = evaluate(HookRequest {
        payload: &payload,
        state_dir: state_dir.as_deref().map(Path::new),
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
        return decide_spawn(state_dir, &tool_input, request.now_ms);
    }
    let cwd = required_string(event, &["cwd"])?;
    let cwd_path = PathBuf::from(&cwd);
    match find_grant_for_cwd(state_dir, &cwd_path, request.now_ms)? {
        Some(grant) => {
            let role = actor_role
                .as_deref()
                .ok_or_else(|| LaneError::new(Denial::Role, "lane actor role is missing"))?;
            match class {
                ToolClass::Read | ToolClass::Write => {
                    let access = if class == ToolClass::Read {
                        Access::Read
                    } else {
                        Access::Write
                    };
                    let path = operation_path(&tool_input)?;
                    validate_path(
                        &grant,
                        &PathClaim {
                            role,
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
                    "tool is outside the lane grant",
                )),
            }
        }
        None => {
            if controlled_actor {
                return Err(LaneError::new(
                    Denial::MissingGrant,
                    "controlled lane actor has no active grant",
                ));
            }
            if matches!(class, ToolClass::Read | ToolClass::Write) {
                if let Ok(path) = operation_path(&tool_input) {
                    let full = if path.is_absolute() {
                        path
                    } else {
                        cwd_path.join(path)
                    };
                    if store::path_targets_lane(state_dir, &full)? {
                        return Err(LaneError::new(Denial::Path, "path enters another lane"));
                    }
                }
            }
            Ok(HookDecision::Silent)
        }
    }
}

fn is_controlled_role(role: &str) -> bool {
    matches!(
        role,
        crate::schema::WRITER_ROLE | crate::schema::REVIEWER_ROLE
    )
}

fn decide_spawn(
    state_dir: &Path,
    tool_input: &Map<String, Value>,
    now_ms: u64,
) -> Result<HookDecision, LaneError> {
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
    let child_slot = optional_u32(tool_input, &["child_slot", "childSlot"])?;
    let grant = match find_grant_for_cwd(state_dir, &cwd_path, now_ms)? {
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
    validate_spawn(
        &grant,
        &SpawnClaim {
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
            child_slot,
        },
        now_ms,
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

fn operation_path(tool_input: &Map<String, Value>) -> Result<PathBuf, LaneError> {
    if has_any(
        tool_input,
        &[
            "command",
            "cmd",
            "command_line",
            "commandLine",
            "script",
            "url",
            "urls",
        ],
    ) {
        return Err(LaneError::new(
            Denial::Uncontrolled,
            "path tool carries a command or url",
        ));
    }
    let path = required_string(
        tool_input,
        &[
            "file_path",
            "filePath",
            "target_file",
            "targetFile",
            "path",
            "notebook_path",
            "notebookPath",
        ],
    )?;
    Ok(PathBuf::from(path))
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

fn optional_u32(map: &Map<String, Value>, keys: &[&str]) -> Result<Option<u32>, LaneError> {
    let mut found = None;
    for key in keys {
        if let Some(value) = map.get(*key) {
            let Some(number) = value.as_u64() else {
                return Err(LaneError::new(
                    Denial::MalformedHook,
                    "child slot is not an integer",
                ));
            };
            let slot = u32::try_from(number)
                .map_err(|_| LaneError::new(Denial::ChildSlot, "child slot is outside u32"))?;
            match found {
                None => found = Some(slot),
                Some(existing) if existing != slot => {
                    return Err(LaneError::new(
                        Denial::MalformedHook,
                        "child slot aliases disagree",
                    ));
                }
                Some(_) => {}
            }
        }
    }
    Ok(found)
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

fn defer_json() -> String {
    r#"{"decision":"defer"}"#.to_string()
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
