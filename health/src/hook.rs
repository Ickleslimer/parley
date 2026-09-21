use std::io::{self, Read, Write};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::classifier::extract_grok_error;
use crate::command::{analyze_command, health_binary_from_token, is_shell_tool, HealthBinary};
use crate::fsutil;
use crate::inbox;
use crate::paths::HealthPaths;
use crate::schema::{
    fnv1a_64, now_ms, sanitize_id, HealthError, HealthRecord, InboxKind, Source, MAX_HOOK_STDIN,
    SCHEMA_VERSION,
};
use crate::scope::{query_matches_installed, resolve_candidate, ScopeFile};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    PreToolUse,
    StopFailure,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookDecision {
    Silent,
    Deny { reason: &'static str },
}

#[derive(Clone, Debug)]
pub struct HookOutput {
    pub stdout: Option<String>,
    pub inbox_written: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DenyDecision {
    permission_decision: DenyToken,
    permission_decision_reason: &'static str,
    hook_specific_output: HookSpecificOutput,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum DenyToken {
    Deny,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HookSpecificOutput {
    hook_event_name: &'static str,
    permission_decision: DenyToken,
    permission_decision_reason: &'static str,
}

pub fn run() -> Result<(), HealthError> {
    let (bytes, clipped) = read_bounded_stdin()?;
    let output = handle_bytes(&bytes, clipped, &HealthPaths::from_env())?;
    if let Some(stdout) = output.stdout {
        let mut out = io::stdout().lock();
        writeln!(out, "{stdout}")?;
        out.flush()?;
    }
    Ok(())
}

pub fn handle_bytes(
    bytes: &[u8],
    clipped: bool,
    paths: &HealthPaths,
) -> Result<HookOutput, HealthError> {
    if bytes.is_empty() {
        return Ok(HookOutput {
            stdout: None,
            inbox_written: false,
        });
    }
    let value: Value = match serde_json::from_slice(fsutil::strip_bom(bytes)) {
        Ok(Value::Object(map)) => Value::Object(map),
        _ => {
            return Ok(HookOutput {
                stdout: None,
                inbox_written: false,
            })
        }
    };
    let event = hook_event(&value);
    let scope = ScopeFile::load_or_empty(&paths.scope());
    match event {
        HookEvent::PreToolUse => {
            let decision = decide_pretool_use(&value, &scope);
            Ok(HookOutput {
                stdout: deny_stdout(decision),
                inbox_written: false,
            })
        }
        HookEvent::StopFailure => {
            let written = emit_stop_failure(&value, clipped, paths, &scope)?;
            Ok(HookOutput {
                stdout: None,
                inbox_written: written,
            })
        }
        HookEvent::Other => Ok(HookOutput {
            stdout: None,
            inbox_written: false,
        }),
    }
}

pub fn decide_pretool_use(value: &Value, scope: &ScopeFile) -> HookDecision {
    let tool_name = string_field(value, &["tool_name", "toolName"]).unwrap_or_default();
    let cwd = string_field(value, &["cwd"]);
    let workspace = string_field(value, &["workspace_root", "workspaceRoot"]);
    let in_scope = scope.in_scope(cwd.as_deref(), workspace.as_deref());
    let tool_input = value
        .get("tool_input")
        .or_else(|| value.get("toolInput"))
        .cloned()
        .unwrap_or(Value::Null);
    let command = if is_shell_tool(&tool_name) {
        extract_command(&tool_input)
    } else {
        None
    };
    let Some(command) = command else {
        return HookDecision::Silent;
    };
    let analysis = analyze_command(&command);
    if !analysis.is_health_invocation() {
        return HookDecision::Silent;
    }
    if !in_scope {
        return HookDecision::Deny {
            reason: "parley-health: deny out-of-scope",
        };
    }
    match analysis.health_binary {
        Some(HealthBinary::Query) => {
            if analysis.wrapper
                || analysis.chaining
                || analysis.redirect
                || analysis.extra_arguments
                || !analysis.exact_binary_name
            {
                return HookDecision::Deny {
                    reason: "parley-health: deny query variant",
                };
            }
            let Some(installed) = scope.executable(HealthBinary::Query) else {
                return HookDecision::Deny {
                    reason: "parley-health: deny query identity mismatch",
                };
            };
            let exe = analysis
                .executable_token
                .as_deref()
                .unwrap_or("parley-health-query.exe");
            let cwd = cwd.as_deref().unwrap_or("");
            let resolved = resolve_candidate(exe, cwd, &[]);
            let observed = fsutil::file_identity(Path::new(&resolved)).ok();
            if query_matches_installed(exe, cwd, &[], installed, observed) {
                HookDecision::Silent
            } else {
                HookDecision::Deny {
                    reason: "parley-health: deny query identity mismatch",
                }
            }
        }
        Some(HealthBinary::Supervisor) | Some(HealthBinary::Hook) => HookDecision::Deny {
            reason: "parley-health: deny health executable",
        },
        None => HookDecision::Silent,
    }
}

fn emit_stop_failure(
    value: &Value,
    clipped: bool,
    paths: &HealthPaths,
    scope: &ScopeFile,
) -> Result<bool, HealthError> {
    let cwd = string_field(value, &["cwd"]);
    let workspace = string_field(value, &["workspace_root", "workspaceRoot"]);
    if !scope.in_scope(cwd.as_deref(), workspace.as_deref()) {
        return Ok(false);
    }
    let evidence = extract_grok_error(value, clipped);
    let session = string_field(value, &["session_id", "sessionId"]).unwrap_or_default();
    let prompt = string_field(value, &["prompt_id", "promptId"]).unwrap_or_default();
    let timestamp = string_field(value, &["timestamp"]).unwrap_or_default();
    let as_of_ms = now_ms();
    let mut id_src = format!(
        "{}:{}:{}:{}:{}:{}:{}",
        session,
        prompt,
        timestamp,
        evidence.http_status.unwrap_or(0),
        evidence.provider_code.as_deref().unwrap_or(""),
        evidence.clipped,
        evidence.ambiguous
    );
    id_src.truncate(256);
    let inbox_id = sanitize_id(&format!("grok-stop-{:x}", fnv1a_64(id_src.as_bytes())))
        .unwrap_or_else(|| format!("grok-stop-{as_of_ms}"));
    let mut record = HealthRecord::new(InboxKind::GrokStopFailure, inbox_id, as_of_ms);
    record.schema_version = SCHEMA_VERSION;
    record.source = Some(Source::Grok);
    record.session_id = sanitize_id(&session);
    record.http_status = evidence.http_status;
    record.provider_code = evidence.provider_code;
    record.generic_rate_limit = Some(evidence.generic_rate_limit);
    record.clipped = Some(evidence.clipped);
    record.ambiguous = Some(evidence.ambiguous);
    inbox::write_record(paths, &record)?;
    Ok(true)
}

fn deny_stdout(decision: HookDecision) -> Option<String> {
    match decision {
        HookDecision::Silent => None,
        HookDecision::Deny { reason } => {
            let body = DenyDecision {
                permission_decision: DenyToken::Deny,
                permission_decision_reason: reason,
                hook_specific_output: HookSpecificOutput {
                    hook_event_name: "PreToolUse",
                    permission_decision: DenyToken::Deny,
                    permission_decision_reason: reason,
                },
            };
            Some(serde_json::to_string(&body).unwrap_or_else(|_| {
                r#"{"permissionDecision":"deny","permissionDecisionReason":"parley-health: deny"}"#
                    .to_string()
            }))
        }
    }
}

fn hook_event(value: &Value) -> HookEvent {
    let name = string_field(value, &["hook_event_name", "hookEventName"]).unwrap_or_default();
    match name.to_ascii_lowercase().as_str() {
        "pretooluse" | "pre_tool_use" => HookEvent::PreToolUse,
        "stopfailure" | "stop_failure" => HookEvent::StopFailure,
        _ => HookEvent::Other,
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    for key in keys {
        match object.get(*key) {
            Some(Value::String(text)) if !text.is_empty() => return Some(text.clone()),
            _ => {}
        }
    }
    None
}

fn extract_command(tool_input: &Value) -> Option<String> {
    let object = tool_input.as_object()?;
    for key in ["command", "cmd", "command_line", "commandLine", "script"] {
        if let Some(Value::String(text)) = object.get(key) {
            return Some(text.clone());
        }
    }
    None
}

fn read_bounded_stdin() -> Result<(Vec<u8>, bool), HealthError> {
    let mut buf = Vec::new();
    let mut stdin = io::stdin().lock();
    let mut tmp = [0_u8; 8192];
    let mut clipped = false;
    loop {
        let read = stdin.read(&mut tmp)?;
        if read == 0 {
            break;
        }
        let room = MAX_HOOK_STDIN.saturating_sub(buf.len());
        if read > room {
            buf.extend_from_slice(&tmp[..room]);
            clipped = true;
            break;
        }
        buf.extend_from_slice(&tmp[..read]);
    }
    Ok((buf, clipped))
}

pub fn command_mentions_health(command: &str) -> Option<HealthBinary> {
    analyze_command(command).health_binary.or_else(|| {
        crate::command::tokenize(command)
            .into_iter()
            .find_map(|token| health_binary_from_token(&token))
    })
}
