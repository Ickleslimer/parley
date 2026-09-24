use std::io::{self, Read, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};

use crate::binding::{validate_binding, Binding, RuntimeFacts};
use crate::codex_activity;
use crate::command::{self, CommandClass, HelperKind};
use crate::fsutil::{self, FailKind};
use crate::schema::{PeerActivityDocument, StoredRecord, NOT_INFERRED};
use crate::store;

const MAX_HOOK_STDIN: usize = 256 * 1024;

pub trait Clock {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&self, milliseconds: u64);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        system_now_ms()
    }

    fn sleep_ms(&self, milliseconds: u64) {
        thread::sleep(Duration::from_millis(milliseconds));
    }
}

pub fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub fn peek(binding: &Binding, facts: &RuntimeFacts, now_ms: u64) -> PeerActivityDocument {
    match prepare(binding, facts, HelperKind::Peek, now_ms, false) {
        Ok(_) => codex_activity::read_activity(
            binding.codex_rollout.as_deref(),
            binding.codex_home.as_deref(),
            &binding.source_session_id,
            now_ms,
        ),
        Err(kind) => PeerActivityDocument::unavailable_at(kind.as_str(), now_ms),
    }
}

pub fn wait(
    binding: &Binding,
    facts: &RuntimeFacts,
    clock: &dyn Clock,
) -> Result<String, FailKind> {
    let now_ms = clock.now_ms();
    let record = prepare(binding, facts, HelperKind::Wait, now_ms, true)?;
    if !record.receipt_observed() && !record.durable_unacknowledged() {
        return Err(FailKind::Stale);
    }
    if !record.receipt_observed() {
        let until = bounded_until(&record, now_ms, binding.wait_slice_secs);
        if record.wait_until_ms != Some(until) {
            store::update(&binding.job_dir, now_ms, |record| {
                record.wait_until_ms = Some(until);
                Ok(true)
            })?;
        }
        loop {
            let current = clock.now_ms();
            let latest = store::load_readonly(&binding.job_dir)?;
            if latest.receipt_observed() || current >= until {
                break;
            }
            let step = (until - current).min(250);
            if step == 0 {
                break;
            }
            clock.sleep_ms(step);
        }
    }
    let record = store::load_readonly(&binding.job_dir)?;
    encode(&wait_body(binding, &record, clock.now_ms()))
}

pub fn alert(binding: &Binding, facts: &RuntimeFacts, now_ms: u64) -> Result<String, FailKind> {
    let record = prepare(binding, facts, HelperKind::Alert, now_ms, true)?;
    if !record.durable_unacknowledged() {
        return Err(FailKind::Stale);
    }
    let inbox = binding.health_inbox.as_deref().ok_or(FailKind::Missing)?;
    let incident_id = match record.alert_incident_id.clone() {
        Some(existing) if fsutil::valid_identifier(&existing) => existing,
        Some(_) => return Err(FailKind::Malformed),
        None => {
            let created = format!("handoff-{}", binding.handoff_id);
            if !fsutil::valid_identifier(&created) {
                return Err(FailKind::Malformed);
            }
            created
        }
    };
    let first_request = record.alert_incident_id.is_none();
    let created_file = write_alert(
        inbox,
        &incident_id,
        &alert_record(binding, &incident_id, now_ms)?,
        binding,
    )?;
    if first_request {
        store::update(&binding.job_dir, now_ms, |record| {
            record.alert_incident_id = Some(incident_id.clone());
            Ok(true)
        })?;
    }
    encode(&AlertResponse {
        schema_version: 1,
        incident_id,
        created: first_request && created_file,
        sound: false,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookDecision {
    Silent,
    Deny,
}

pub struct HookOutput {
    pub decision: HookDecision,
    pub stdout: Option<String>,
}

pub fn evaluate_hook(
    payload: &[u8],
    binding: Option<&Binding>,
    facts: Option<&RuntimeFacts>,
    now_ms: u64,
) -> HookOutput {
    evaluate_hook_scoped(payload, binding, facts, now_ms, binding.is_some())
}

fn evaluate_hook_scoped(
    payload: &[u8],
    binding: Option<&Binding>,
    facts: Option<&RuntimeFacts>,
    now_ms: u64,
    scope_claimed: bool,
) -> HookOutput {
    if payload.is_empty() || payload.len() > MAX_HOOK_STDIN {
        return malformed_or_silent("PreToolUse", scope_claimed);
    }
    let value: Value = match serde_json::from_slice(strip_bom(payload)) {
        Ok(value) => value,
        Err(_) => return malformed_or_silent("PreToolUse", scope_claimed),
    };
    let Some(event) = value.as_object() else {
        return malformed_or_silent("PreToolUse", scope_claimed);
    };
    let name = match event_name(event) {
        Ok(Some(name)) => name,
        Ok(None) | Err(_) => return malformed_or_silent("PreToolUse", scope_claimed),
    };
    if name == "stop" {
        if !scope_claimed && binding.is_none() {
            return HookOutput::silent();
        }
        return evaluate_stop(event, binding, facts, now_ms);
    }
    if name != "pretooluse" && name != "pre_tool_use" {
        return HookOutput::silent();
    }
    let tool_name = match string_field(event, &["tool_name", "toolName"]) {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => return malformed_or_silent("PreToolUse", scope_claimed),
    };
    if !command::is_shell_tool(&tool_name) {
        return HookOutput::silent();
    }
    if !matches!(
        bool_field(event, &["toolInputTruncated", "tool_input_truncated"]),
        Ok(Some(false))
    ) {
        return malformed_or_silent("PreToolUse", scope_claimed);
    }
    let tool_input = match object_field(event, &["tool_input", "toolInput"]) {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => return malformed_or_silent("PreToolUse", scope_claimed),
    };
    let command_text = match command_text(tool_input) {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => return malformed_or_silent("PreToolUse", scope_claimed),
    };
    match command::classify_command(&command_text) {
        CommandClass::Unrelated => {
            if binding.is_some_and(|binding| command_uses_helper_identity(&command_text, binding)) {
                deny("PreToolUse", "parley-handoff: deny renamed helper")
            } else {
                HookOutput::silent()
            }
        }
        CommandClass::Denied(reason) => deny("PreToolUse", static_denial(reason)),
        CommandClass::Exact { kind, path } => {
            evaluate_exact(event, binding, facts, now_ms, kind, &path)
        }
    }
}

pub fn error_json(kind: FailKind) -> String {
    format!(
        "{{\"schema_version\":1,\"error\":\"{}\",\"assessment\":\"{NOT_INFERRED}\"}}",
        kind.as_str()
    )
}

pub fn run_peek() -> i32 {
    if std::env::args().len() != 1 {
        eprintln!("parley-peer-peek accepts no arguments");
        return 2;
    }
    let now_ms = system_now_ms();
    let document = match (Binding::from_env(), RuntimeFacts::capture()) {
        (Ok(binding), Ok(facts)) => peek(&binding, &facts, now_ms),
        (Err(kind), _) | (_, Err(kind)) => {
            PeerActivityDocument::unavailable_at(kind.as_str(), now_ms)
        }
    };
    match serde_json::to_string(&document) {
        Ok(json) => {
            println!("{json}");
            0
        }
        Err(_) => {
            println!(
                "{}",
                serde_json::to_string(&PeerActivityDocument::unavailable_at("io", now_ms))
                    .unwrap_or_else(|_| error_json(FailKind::Io))
            );
            0
        }
    }
}

pub fn run_wait() -> i32 {
    run_result(HelperKind::Wait, |binding, facts| {
        wait(binding, facts, &SystemClock)
    })
}

pub fn run_alert() -> i32 {
    run_result(HelperKind::Alert, |binding, facts| {
        alert(binding, facts, system_now_ms())
    })
}

pub fn run_hook() -> i32 {
    if std::env::args().len() != 1 {
        eprintln!("parley-handoff-hook accepts no arguments");
        return 2;
    }
    let scope_claimed = binding_env_claimed();
    let payload = match read_stdin() {
        Ok(payload) => payload,
        Err(_) => {
            if scope_claimed {
                emit_hook(&deny("PreToolUse", "parley-handoff: deny malformed hook"));
            }
            return 0;
        }
    };
    let binding = Binding::from_env().ok();
    let facts = RuntimeFacts::capture().ok();
    let output = evaluate_hook_scoped(
        &payload,
        binding.as_ref(),
        facts.as_ref(),
        system_now_ms(),
        scope_claimed,
    );
    emit_hook(&output);
    0
}

fn run_result<F>(role: HelperKind, action: F) -> i32
where
    F: FnOnce(&Binding, &RuntimeFacts) -> Result<String, FailKind>,
{
    if std::env::args().len() != 1 {
        eprintln!("{} accepts no arguments", role.stem());
        return 2;
    }
    let binding = match Binding::from_env() {
        Ok(binding) => binding,
        Err(kind) => {
            println!("{}", error_json(kind));
            return 1;
        }
    };
    let facts = match RuntimeFacts::capture() {
        Ok(facts) => facts,
        Err(kind) => {
            println!("{}", error_json(kind));
            return 1;
        }
    };
    match action(&binding, &facts) {
        Ok(json) => {
            println!("{json}");
            0
        }
        Err(kind) => {
            println!("{}", error_json(kind));
            1
        }
    }
}

fn prepare(
    binding: &Binding,
    facts: &RuntimeFacts,
    role: HelperKind,
    now_ms: u64,
    repair: bool,
) -> Result<StoredRecord, FailKind> {
    fsutil::require_job_dir(&binding.state_dir, &binding.job_dir, &binding.job_id)?;
    if !fsutil::same_file(&facts.exe, binding.helper_exe(role))? {
        return Err(FailKind::FileIdentity);
    }
    let record = if repair {
        store::repair_snapshot(&binding.job_dir)?
    } else {
        store::load_readonly(&binding.job_dir)?
    };
    validate_binding(binding, facts, role, &record, now_ms)?;
    Ok(record)
}

fn bounded_until(record: &StoredRecord, now_ms: u64, slice_secs: u64) -> u64 {
    let slice_ms = slice_secs.saturating_mul(1_000);
    let mut until = now_ms.saturating_add(slice_ms);
    if let Some(deadline) = record.deadline_ms {
        until = until.min(deadline.saturating_add(slice_ms));
    }
    if let Some(existing) = record.wait_until_ms {
        if existing > now_ms && existing < until {
            until = existing;
        }
    }
    until
}

fn wait_body(binding: &Binding, record: &StoredRecord, now_ms: u64) -> WaitResponse {
    let activity = codex_activity::read_activity(
        binding.codex_rollout.as_deref(),
        binding.codex_home.as_deref(),
        &binding.source_session_id,
        now_ms,
    );
    WaitResponse {
        schema_version: 1,
        receipt_state: if record.receipt_observed() {
            "received".to_string()
        } else {
            "pending".to_string()
        },
        deadline_state: match record.deadline_ms {
            None => "unset".to_string(),
            Some(deadline) if now_ms >= deadline => "expired".to_string(),
            Some(_) => "pending".to_string(),
        },
        wait_until_ms: record.wait_until_ms,
        state: record.state.clone(),
        activity,
    }
}

const HEALTH_EVIDENCE_SCHEMA_VERSION: u64 = 3;

fn accepted_health_evidence(version: u64) -> bool {
    matches!(version, 1 | 2 | HEALTH_EVIDENCE_SCHEMA_VERSION)
}

fn alert_record(binding: &Binding, incident_id: &str, now_ms: u64) -> Result<Vec<u8>, FailKind> {
    let body = json!({
        "schema_version": HEALTH_EVIDENCE_SCHEMA_VERSION,
        "inbox_id": incident_id,
        "kind": "peer_alert_requested",
        "as_of_ms": now_ms,
        "source": "grok",
        "class": "handoff_unacknowledged",
        "incident_id": incident_id,
        "session_id": binding.target_session_id,
        "event_id": Value::Null,
        "exchange_id": binding.job_id,
    });
    serde_json::to_vec(&body).map_err(|_| FailKind::Io)
}

fn write_alert(
    inbox: &Path,
    incident_id: &str,
    bytes: &[u8],
    binding: &Binding,
) -> Result<bool, FailKind> {
    if !inbox.is_absolute() {
        return Err(FailKind::Path);
    }
    let inbox = fsutil::canonical_existing(inbox)?;
    let path = inbox.join(format!("{incident_id}.json"));
    if !path.starts_with(&inbox) {
        return Err(FailKind::Path);
    }
    match fsutil::create_new_synced(&path, bytes) {
        Ok(()) => Ok(true),
        Err(FailKind::Exists) => {
            let existing = fsutil::read_shared(&path, 64 * 1024)?;
            let value: Value =
                serde_json::from_slice(&existing).map_err(|_| FailKind::Malformed)?;
            let valid = value
                .get("schema_version")
                .and_then(Value::as_u64)
                .is_some_and(accepted_health_evidence)
                && value.get("kind").and_then(Value::as_str) == Some("peer_alert_requested")
                && value.get("class").and_then(Value::as_str) == Some("handoff_unacknowledged")
                && value.get("incident_id").and_then(Value::as_str) == Some(incident_id)
                && value.get("session_id").and_then(Value::as_str)
                    == Some(binding.target_session_id.as_str())
                && value.get("exchange_id").and_then(Value::as_str)
                    == Some(binding.job_id.as_str());
            if valid {
                Ok(false)
            } else {
                Err(FailKind::Malformed)
            }
        }
        Err(error) => Err(error),
    }
}

fn evaluate_exact(
    event: &serde_json::Map<String, Value>,
    binding: Option<&Binding>,
    facts: Option<&RuntimeFacts>,
    now_ms: u64,
    kind: HelperKind,
    path: &Path,
) -> HookOutput {
    let (Some(binding), Some(facts)) = (binding, facts) else {
        return deny("PreToolUse", "parley-handoff: deny missing binding");
    };
    if let Err(kind) = require_exact_context(event, binding, facts) {
        return deny("PreToolUse", static_failure(kind));
    }
    if fsutil::same_file(path, binding.helper_exe(kind)).ok() != Some(true) {
        return deny("PreToolUse", "parley-handoff: deny file identity");
    }
    match prepare(binding, facts, HelperKind::Hook, now_ms, false) {
        Ok(_) => HookOutput::silent(),
        Err(kind) => deny("PreToolUse", static_failure(kind)),
    }
}

fn evaluate_stop(
    event: &serde_json::Map<String, Value>,
    binding: Option<&Binding>,
    facts: Option<&RuntimeFacts>,
    now_ms: u64,
) -> HookOutput {
    let (Some(binding), Some(facts)) = (binding, facts) else {
        return deny("Stop", "parley-handoff: deny missing binding");
    };
    if let Err(kind) = require_context(event, binding, facts) {
        return deny("Stop", static_failure(kind));
    }
    match prepare(binding, facts, HelperKind::Hook, now_ms, false) {
        Ok(record) if record.durable_unacknowledged() => {
            deny("Stop", "parley-handoff: deny unacknowledged handoff")
        }
        Ok(_) => HookOutput::silent(),
        Err(kind) => deny("Stop", static_failure(kind)),
    }
}

fn require_exact_context(
    event: &serde_json::Map<String, Value>,
    binding: &Binding,
    facts: &RuntimeFacts,
) -> Result<(), FailKind> {
    let session = string_field(event, &["sessionId", "session_id"])?.ok_or(FailKind::Session)?;
    if session != binding.target_session_id {
        return Err(FailKind::Session);
    }
    let cwd = string_field(event, &["cwd"])?.ok_or(FailKind::Cwd)?;
    if !fsutil::same_directory(Path::new(&cwd), &facts.cwd)? {
        return Err(FailKind::Cwd);
    }
    Ok(())
}

fn require_context(
    event: &serde_json::Map<String, Value>,
    binding: &Binding,
    facts: &RuntimeFacts,
) -> Result<(), FailKind> {
    require_exact_context(event, binding, facts)
}

fn command_uses_helper_identity(command: &str, binding: &Binding) -> bool {
    for token in command::candidate_tokens(command) {
        let path = Path::new(&token);
        if !path.is_absolute() || !path.is_file() {
            continue;
        }
        for kind in [
            HelperKind::Peek,
            HelperKind::Wait,
            HelperKind::Alert,
            HelperKind::Hook,
        ] {
            if fsutil::same_file(path, binding.helper_exe(kind)).ok() == Some(true) {
                return true;
            }
        }
    }
    false
}

fn command_text(tool_input: &serde_json::Map<String, Value>) -> Result<Option<String>, FailKind> {
    string_field(
        tool_input,
        &["command", "cmd", "command_line", "commandLine", "script"],
    )
}

fn event_name(event: &serde_json::Map<String, Value>) -> Result<Option<String>, FailKind> {
    Ok(string_field(event, &["hook_event_name", "hookEventName"])?
        .map(|value| value.to_ascii_lowercase()))
}

fn string_field(
    event: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<Option<String>, FailKind> {
    let mut found: Option<String> = None;
    for key in keys {
        if let Some(value) = event.get(*key) {
            let Value::String(value) = value else {
                return Err(FailKind::Malformed);
            };
            if value.is_empty() {
                return Err(FailKind::Malformed);
            }
            if found.as_deref().is_some_and(|existing| existing != value) {
                return Err(FailKind::Malformed);
            }
            found = Some(value.clone());
        }
    }
    Ok(found)
}

fn bool_field(
    event: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<Option<bool>, FailKind> {
    let mut found = None;
    for key in keys {
        if let Some(value) = event.get(*key) {
            let Value::Bool(value) = value else {
                return Err(FailKind::Malformed);
            };
            if found.is_some_and(|existing| existing != *value) {
                return Err(FailKind::Malformed);
            }
            found = Some(*value);
        }
    }
    Ok(found)
}

fn object_field<'a>(
    event: &'a serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<Option<&'a serde_json::Map<String, Value>>, FailKind> {
    let mut found: Option<&serde_json::Map<String, Value>> = None;
    for key in keys {
        if let Some(value) = event.get(*key) {
            let Value::Object(value) = value else {
                return Err(FailKind::Malformed);
            };
            if found.is_some_and(|existing| existing != value) {
                return Err(FailKind::Malformed);
            }
            found = Some(value);
        }
    }
    Ok(found)
}

fn malformed_or_silent(event: &str, scope_claimed: bool) -> HookOutput {
    if scope_claimed {
        deny(event, "parley-handoff: deny malformed hook")
    } else {
        HookOutput::silent()
    }
}

fn binding_env_claimed() -> bool {
    [
        "PARLEY_HANDOFF_STATE_DIR",
        "PARLEY_HANDOFF_JOB_DIR",
        "PARLEY_HANDOFF_JOB_ID",
        "PARLEY_HANDOFF_ID",
        "PARLEY_HANDOFF_CAPABILITY",
        "PARLEY_HANDOFF_SOURCE_SESSION_ID",
        "PARLEY_HANDOFF_TARGET_SESSION_ID",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some())
}

fn static_denial(reason: &str) -> &'static str {
    match reason {
        "wrapper" => "parley-handoff: deny wrapper",
        "redirect" => "parley-handoff: deny redirect",
        "chaining" => "parley-handoff: deny chaining",
        "arguments" => "parley-handoff: deny arguments",
        "renamed" => "parley-handoff: deny renamed helper",
        "out_of_scope" => "parley-handoff: deny out of scope",
        _ => "parley-handoff: deny malformed hook",
    }
}

fn static_failure(kind: FailKind) -> &'static str {
    match kind {
        FailKind::Capability => "parley-handoff: deny capability",
        FailKind::Process => "parley-handoff: deny process identity",
        FailKind::Session => "parley-handoff: deny session identity",
        FailKind::FileIdentity => "parley-handoff: deny file identity",
        FailKind::Path => "parley-handoff: deny path",
        FailKind::Expired => "parley-handoff: deny expired capability",
        FailKind::Cwd => "parley-handoff: deny cwd",
        FailKind::Stale => "parley-handoff: deny stale handoff",
        FailKind::Locked => "parley-handoff: deny locked handoff",
        FailKind::Missing => "parley-handoff: deny missing handoff",
        FailKind::Malformed => "parley-handoff: deny malformed hook",
        FailKind::Arguments => "parley-handoff: deny arguments",
        FailKind::Exists => "parley-handoff: deny malformed hook",
        FailKind::Io => "parley-handoff: deny malformed hook",
    }
}

fn deny(event: &str, reason: &'static str) -> HookOutput {
    let body = json!({
        "decision": "deny",
        "reason": reason,
        "permissionDecision": "deny",
        "permissionDecisionReason": reason,
        "hookSpecificOutput": {
            "hookEventName": event,
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    });
    HookOutput {
        decision: HookDecision::Deny,
        stdout: Some(body.to_string()),
    }
}

impl HookOutput {
    fn silent() -> Self {
        Self {
            decision: HookDecision::Silent,
            stdout: None,
        }
    }
}

fn encode<T: Serialize>(value: &T) -> Result<String, FailKind> {
    serde_json::to_string(value).map_err(|_| FailKind::Io)
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

fn read_stdin() -> io::Result<Vec<u8>> {
    let mut stdin = io::stdin().lock();
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stdin.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        if buf.len().saturating_add(read) > MAX_HOOK_STDIN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "hook payload exceeds bound",
            ));
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    Ok(buf)
}

fn emit_hook(output: &HookOutput) {
    if let Some(stdout) = &output.stdout {
        let _ = writeln!(io::stdout(), "{stdout}");
    }
}

#[derive(Serialize)]
struct WaitResponse {
    schema_version: u32,
    receipt_state: String,
    deadline_state: String,
    wait_until_ms: Option<u64>,
    state: String,
    activity: PeerActivityDocument,
}

#[derive(Serialize)]
struct AlertResponse {
    schema_version: u32,
    incident_id: String,
    created: bool,
    sound: bool,
}
