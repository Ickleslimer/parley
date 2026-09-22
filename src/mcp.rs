//! Minimal MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Lets any MCP-capable agent ask "what's my last conversation here?" and get
//! back resumable session details plus the exact native resume command —
//! scoped, like every harness's own `--resume`, to a working directory.
//!
//! Built on the crate's zero-dependency `Json` type. The protocol surface is
//! intentionally small: `initialize`, `tools/list`, and `tools/call` with five
//! tools — three for session discovery/resume, `ask_agent` for one-shot
//! agent-to-agent calls, and `fuse` to convene a panel of agents on one prompt.
//! Resume tools return *commands* as text and never spawn an interactive
//! harness; `ask_agent` and `fuse` run the target agents headless.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use crate::ask::{self, AskRequest, ContextRef};
use crate::cli::{McpMode, McpOptions};
use crate::event_log::ExchangeReceipt;
use crate::fsx;
use crate::fuse;
use crate::harness::{normalize_harness, Invocation};
use crate::health_report::HealthReporter;
use crate::job_runtime::{self, JobRuntime};
use crate::jobs::{HandoffMode, JobMode, JobSpec, MAX_LISTED_JOBS};
use crate::json::Json;
use crate::policy::RuntimePolicy;
use crate::process::{capture_invocation, run_invocation};
use crate::session;

const PROTOCOL_VERSION: &str = "2024-11-05";

struct HandledResponse {
    json: Json,
    delivery: Option<ExchangeReceipt>,
}

/// Route a `par mcp ...` invocation to the right handler.
pub(crate) fn dispatch(options: McpOptions) -> Result<(), String> {
    match options.mode {
        McpMode::Serve => run(options),
        // `on` is just (re)registration.
        McpMode::Connect | McpMode::On => {
            let harness = options
                .harness
                .as_deref()
                .ok_or("mcp connect requires a harness: par mcp connect -h <agent>")?;
            connect(harness, options.dry_run)
        }
        McpMode::Off => {
            let harness = options
                .harness
                .as_deref()
                .ok_or("mcp off requires a harness: par mcp off -h <agent>")?;
            off(harness, options.dry_run)
        }
        McpMode::Status => {
            let harness = options
                .harness
                .as_deref()
                .ok_or("mcp status requires a harness: par mcp status -h <agent>")?;
            status(harness)
        }
    }
}

pub(crate) fn run(_options: McpOptions) -> Result<(), String> {
    let cwd = env::current_dir().map_err(|e| format!("failed to get cwd: {e}"))?;
    let policy = RuntimePolicy::from_env()?;
    let reporter = HealthReporter::from_env()?;
    let jobs = JobRuntime::from_env()?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let result = (|| {
        for line in stdin.lock().lines() {
            let line = line.map_err(|e| format!("stdin read error: {e}"))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let response = match Json::parse(trimmed) {
                Ok(request) => handle_request_with_receipt(&request, &cwd, &policy, &jobs),
                Err(_) => Some(HandledResponse {
                    json: error_response(&Json::Null, -32700, "parse error"),
                    delivery: None,
                }),
            };
            if let Some(response) = response {
                write_mcp_response(&mut out, &response, &reporter)?;
            }
        }
        Ok(())
    })();
    jobs.drain();
    result
}

fn write_mcp_response(
    out: &mut impl Write,
    response: &HandledResponse,
    reporter: &HealthReporter,
) -> Result<(), String> {
    let mut payload = response.json.to_compact_string().into_bytes();
    payload.push(b'\n');
    if let Err(error) = out.write_all(&payload) {
        return Err(stdout_delivery_error(
            "write",
            error,
            response.delivery.as_ref(),
            reporter,
        ));
    }
    if let Err(error) = out.flush() {
        return Err(stdout_delivery_error(
            "flush",
            error,
            response.delivery.as_ref(),
            reporter,
        ));
    }
    Ok(())
}

fn stdout_delivery_error(
    stage: &str,
    error: io::Error,
    receipt: Option<&ExchangeReceipt>,
    reporter: &HealthReporter,
) -> String {
    let base = format!("stdout {stage} error: {error}");
    let Some(receipt) = receipt else {
        return base;
    };
    match reporter.mcp_stdout_undelivered(receipt) {
        Ok(()) => format!(
            "{base}; recorded mcp_stdout_undelivered for response event {} exchange {}",
            receipt.completion.event_id, receipt.completion.exchange_id
        ),
        Err(report_error) => format!(
            "{base}; failed to record mcp_stdout_undelivered: {report_error}; response event {} exchange {} remains in the event log",
            receipt.completion.event_id, receipt.completion.exchange_id
        ),
    }
}

/// Register `par mcp` as an MCP server inside a harness. For harnesses with a
/// native `mcp add` we run it (some, like opencode, open their own add TUI);
/// cursor has no add command, so we merge `~/.cursor/mcp.json` directly.
pub(crate) fn connect(harness: &str, dry_run: bool) -> Result<(), String> {
    let normalized = normalize_harness(harness);
    let bin = par_bin();
    let b = bin.as_str();

    match normalized.as_str() {
        "claude" => exec_or_print(
            Invocation::new("claude", argv(&["mcp", "add", "-s", "user", "par", "--", b, "mcp"])),
            dry_run,
        ),
        "codex" => exec_or_print(
            Invocation::new("codex", argv(&["mcp", "add", "par", "--", b, "mcp"])),
            dry_run,
        ),
        "gemini" => exec_or_print(
            Invocation::new("gemini", argv(&["mcp", "add", "par", b, "mcp"])),
            dry_run,
        ),
        "opencode" => {
            eprintln!("opencode registers MCP servers interactively; launching `opencode mcp add`.");
            eprintln!("  When prompted: name = par, type = local, command = {b} mcp");
            exec_or_print(Invocation::new("opencode", argv(&["mcp", "add"])), dry_run)
        }
        "cursor" => connect_cursor(b, dry_run),
        other => Err(format!(
            "mcp connect does not support \"{other}\" yet (supported: claude, codex, gemini, opencode, cursor)"
        )),
    }
}

/// Run a registration command, or print it under `--dry-run`. Running replaces
/// the process (inheriting stdio, so a harness's add TUI works).
fn exec_or_print(inv: Invocation, dry_run: bool) -> Result<(), String> {
    if dry_run {
        println!("{}", session::render_command(&inv));
        Ok(())
    } else {
        run_invocation(inv, None, true)
    }
}

/// Merge a `par` server entry into `~/.cursor/mcp.json` (cursor has no
/// `mcp add` subcommand), preserving any existing servers.
fn connect_cursor(bin: &str, dry_run: bool) -> Result<(), String> {
    let home = session::home_dir().ok_or("cannot resolve HOME")?;
    let path = home.join(".cursor").join("mcp.json");

    let server = obj(vec![
        ("command", Json::Str(bin.to_string())),
        ("args", Json::Array(vec![Json::Str("mcp".to_string())])),
    ]);

    let mut root = match fs::read_to_string(&path)
        .ok()
        .and_then(|raw| Json::parse(&raw).ok())
    {
        Some(Json::Object(map)) => map,
        _ => BTreeMap::new(),
    };
    let mut servers = match root.get("mcpServers") {
        Some(Json::Object(map)) => map.clone(),
        _ => BTreeMap::new(),
    };
    servers.insert("par".to_string(), server);
    root.insert("mcpServers".to_string(), Json::Object(servers));
    let merged = Json::Object(root);

    if dry_run {
        println!("# would write {}", path.display());
        print!("{}", merged.to_pretty_string());
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::write(&path, merged.to_pretty_string())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("Registered par MCP server in {}", path.display());
    println!("  command: {bin} mcp");
    Ok(())
}

/// Report whether `par` is registered as an MCP server in a harness. Native
/// `mcp list` is queried for the harnesses that have one; cursor is read from
/// its `mcp.json`.
fn status(harness: &str) -> Result<(), String> {
    let normalized = normalize_harness(harness);
    let registered = match normalized.as_str() {
        "claude" | "codex" | "gemini" => {
            let out =
                capture_invocation(Invocation::new(&normalized, argv(&["mcp", "list"])), None)?;
            if !out.success && out.stdout.trim().is_empty() {
                return Err(format!(
                    "could not query {normalized} mcp list: {}",
                    out.stderr.trim()
                ));
            }
            out.stdout.contains("par")
        }
        "cursor" => cursor_has_par()?,
        other => {
            return Err(format!(
                "mcp status supports claude, codex, gemini, cursor (got \"{other}\")"
            ))
        }
    };
    println!(
        "par MCP in {normalized}: {}",
        if registered {
            "registered (on)"
        } else {
            "not registered (off)"
        }
    );
    Ok(())
}

/// Unregister `par`'s MCP server from a harness. Native `mcp remove` for the
/// harnesses that have one; for cursor the entry is removed from `mcp.json` and
/// parked to a sidecar so `par mcp on` can restore any custom fields.
fn off(harness: &str, dry_run: bool) -> Result<(), String> {
    let normalized = normalize_harness(harness);
    match normalized.as_str() {
        "claude" => exec_or_print(
            Invocation::new("claude", argv(&["mcp", "remove", "-s", "user", "par"])),
            dry_run,
        ),
        "codex" => exec_or_print(
            Invocation::new("codex", argv(&["mcp", "remove", "par"])),
            dry_run,
        ),
        "gemini" => exec_or_print(
            Invocation::new("gemini", argv(&["mcp", "remove", "par"])),
            dry_run,
        ),
        "cursor" => off_cursor(dry_run),
        other => Err(format!(
            "mcp off supports claude, codex, gemini, cursor (got \"{other}\")"
        )),
    }
}

fn cursor_mcp_path() -> Result<PathBuf, String> {
    let home = session::home_dir().ok_or("cannot resolve HOME")?;
    Ok(home.join(".cursor").join("mcp.json"))
}

fn cursor_has_par() -> Result<bool, String> {
    let path = cursor_mcp_path()?;
    let Some(Json::Object(root)) = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| Json::parse(&raw).ok())
    else {
        return Ok(false);
    };
    Ok(matches!(root.get("mcpServers"), Some(Json::Object(servers)) if servers.contains_key("par")))
}

/// Remove `par` from `~/.cursor/mcp.json`, preserving the other servers, and
/// park the removed entry so it can be restored byte-for-byte by `mcp on`.
fn off_cursor(dry_run: bool) -> Result<(), String> {
    let path = cursor_mcp_path()?;
    let mut root = match fs::read_to_string(&path)
        .ok()
        .and_then(|raw| Json::parse(&raw).ok())
    {
        Some(Json::Object(map)) => map,
        _ => {
            println!("par not registered in {} (nothing to do)", path.display());
            return Ok(());
        }
    };
    let mut servers = match root.get("mcpServers") {
        Some(Json::Object(map)) => map.clone(),
        _ => BTreeMap::new(),
    };
    let Some(parked) = servers.remove("par") else {
        println!("par not registered in {} (nothing to do)", path.display());
        return Ok(());
    };
    root.insert("mcpServers".to_string(), Json::Object(servers));
    let merged = Json::Object(root);
    let parked_path = path.with_extension("json.par-parked");

    if dry_run {
        println!("# would remove the `par` server from {}", path.display());
        println!("# would park it to {}", parked_path.display());
        return Ok(());
    }
    fsx::write(&path, &merged.to_pretty_string())?;
    fsx::write(&parked_path, &parked.to_pretty_string())?;
    println!(
        "Removed par from {} (parked to {})",
        path.display(),
        parked_path.display()
    );
    println!("Restore with: par mcp on -h cursor");
    Ok(())
}

/// Absolute path to the running `par` binary, so the registered command works
/// regardless of the caller's PATH. Falls back to the bare name.
fn par_bin() -> String {
    env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| "par".to_string())
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Handle one JSON-RPC request. Returns `None` for notifications (no `id`),
/// which take no response.
#[cfg(test)]
pub(crate) fn handle_request(request: &Json, default_cwd: &Path) -> Option<Json> {
    handle_request_with_policy(request, default_cwd, &RuntimePolicy::default())
}

#[cfg(test)]
fn handle_request_with_policy(
    request: &Json,
    default_cwd: &Path,
    policy: &RuntimePolicy,
) -> Option<Json> {
    handle_request_with_receipt(request, default_cwd, policy, &JobRuntime::disabled())
        .map(|response| response.json)
}

fn handle_request_with_receipt(
    request: &Json,
    default_cwd: &Path,
    policy: &RuntimePolicy,
    jobs: &JobRuntime,
) -> Option<HandledResponse> {
    let method = request.get("method").and_then(Json::as_str).unwrap_or("");
    // Notifications carry no id and expect no reply.
    let id = request.get("id")?;
    let mut delivery = None;

    let result = match method {
        "initialize" => Ok(initialize_result()),
        "tools/list" => Ok(tools_list_result()),
        "tools/call" => call_tool(request, default_cwd, policy, jobs, &mut delivery),
        "ping" => Ok(obj(vec![])),
        other => Err((-32601, format!("method not found: {other}"))),
    };

    Some(HandledResponse {
        json: match result {
            Ok(value) => success_response(id, value),
            Err((code, message)) => error_response(id, code, &message),
        },
        delivery,
    })
}

fn initialize_result() -> Json {
    obj(vec![
        ("protocolVersion", Json::Str(PROTOCOL_VERSION.to_string())),
        ("capabilities", obj(vec![("tools", obj(vec![]))])),
        (
            "serverInfo",
            obj(vec![
                ("name", Json::Str("parley".to_string())),
                ("version", Json::Str(env!("CARGO_PKG_VERSION").to_string())),
            ]),
        ),
    ])
}

fn tools_list_result() -> Json {
    let cwd_prop = obj(vec![
        ("type", Json::Str("string".to_string())),
        (
            "description",
            Json::Str(
                "Working directory to scope sessions to (defaults to the server's cwd)."
                    .to_string(),
            ),
        ),
    ]);
    let harness_prop = obj(vec![
        ("type", Json::Str("string".to_string())),
        (
            "description",
            Json::Str("Optional harness filter: claude, codex, grok, opencode, cursor, gemini, pi (shorthands allowed).".to_string()),
        ),
    ]);

    let list_tool = tool(
        "list_sessions",
        "List resumable agent sessions for a directory across all harnesses, newest first.",
        obj(vec![
            ("cwd", cwd_prop.clone()),
            ("harness", harness_prop.clone()),
        ]),
        vec![],
    );
    let last_tool = tool(
        "get_last_session",
        "Get the most recent resumable session for a directory, with a ready-to-run resume command. Use this for 'pick up my last conversation from <agent>'.",
        obj(vec![(
            "cwd",
            cwd_prop.clone(),
        ), (
            "harness",
            harness_prop.clone(),
        )]),
        vec![],
    );
    let resume_tool = tool(
        "resume_command",
        "Build the native resume command for a specific harness + session id (does not run it).",
        obj(vec![
            ("harness", harness_prop),
            (
                "id",
                obj(vec![
                    ("type", Json::Str("string".to_string())),
                    (
                        "description",
                        Json::Str("Session id to resume.".to_string()),
                    ),
                ]),
            ),
            ("cwd", cwd_prop),
            (
                "yolo",
                obj(vec![
                    ("type", Json::Str("boolean".to_string())),
                    (
                        "description",
                        Json::Str("Append the harness's permission-bypass flag.".to_string()),
                    ),
                ]),
            ),
        ]),
        vec!["harness", "id"],
    );

    let str_prop = |desc: &str| {
        obj(vec![
            ("type", Json::Str("string".to_string())),
            ("description", Json::Str(desc.to_string())),
        ])
    };
    let ask_tool = tool(
        "ask_agent",
        "Ask another agent (including Grok) a headless question and return its reply. Use session_id for a new native session or resume_id to continue one. Optionally seed it with another agent's transcript via context_from.",
        obj(vec![
            ("harness", str_prop("Target agent to ask (shorthands allowed).")),
            ("prompt", str_prop("The question or task for the target agent.")),
            ("model", str_prop("Optional model override.")),
            (
                "reasoning_effort",
                obj(vec![
                    ("type", Json::Str("string".to_string())),
                    (
                        "enum",
                        Json::Array(
                            ["low", "medium", "high", "xhigh"]
                                .into_iter()
                                .map(|value| Json::Str(value.to_string()))
                                .collect(),
                        ),
                    ),
                    (
                        "description",
                        Json::Str("Optional Grok reasoning effort.".to_string()),
                    ),
                ]),
            ),
            ("provider", str_prop("Optional provider override.")),
            ("cwd", str_prop("Working directory (defaults to the server's cwd).")),
            ("permission_mode", str_prop("Optional target permission mode.")),
            ("session_id", str_prop("UUID for a new native session.")),
            ("resume_id", str_prop("Existing native session id to continue.")),
            (
                "max_turns",
                obj(vec![
                    ("type", Json::Str("integer".to_string())),
                    ("minimum", Json::Number(1.0)),
                    ("description", Json::Str("Optional maximum agent turns.".to_string())),
                ]),
            ),
            (
                "max_context_chars",
                obj(vec![
                    ("type", Json::Str("integer".to_string())),
                    ("minimum", Json::Number(1.0)),
                    ("description", Json::Str("Maximum injected transcript characters.".to_string())),
                ]),
            ),
            (
                "yolo",
                obj(vec![
                    ("type", Json::Str("boolean".to_string())),
                    ("description", Json::Str("Explicitly request permission bypass. Runtime policy may reject this.".to_string())),
                ]),
            ),
            (
                "context_from",
                obj(vec![
                    ("type", Json::Str("object".to_string())),
                    (
                        "description",
                        Json::Str("Seed the call with another agent's session transcript.".to_string()),
                    ),
                    (
                        "properties",
                        obj(vec![
                            ("harness", str_prop("Source agent (claude, codex, grok, opencode, pi).")),
                            ("session", str_prop("Session id, or 'latest' / omitted for the newest in cwd.")),
                            (
                                "mode",
                                obj(vec![
                                    ("type", Json::Str("string".to_string())),
                                    (
                                        "enum",
                                        Json::Array(
                                            ["snapshot", "auto", "seed", "delta"]
                                                .into_iter()
                                                .map(|value| Json::Str(value.to_string()))
                                                .collect(),
                                        ),
                                    ),
                                ]),
                            ),
                            (
                                "recovery",
                                obj(vec![
                                    ("type", Json::Str("string".to_string())),
                                    (
                                        "enum",
                                        Json::Array(
                                            ["replay", "skip"]
                                                .into_iter()
                                                .map(|value| Json::Str(value.to_string()))
                                                .collect(),
                                        ),
                                    ),
                                ]),
                            ),
                        ]),
                    ),
                ]),
            ),
        ]),
        vec!["harness", "prompt"],
    );

    let fuse_tool = tool(
        "fuse",
        "Convene a PANEL of agents on one prompt and get back a single, stronger synthesized answer. Sends `prompt` to every agent in `panel` in parallel, then a judge agent (Claude by default) fuses their replies: consensus is high-confidence, contradictions are resolved, gaps filled, blind spots flagged. Use on high-stakes questions where being wrong is expensive (design, security, migrations, hard trade-offs) and a diverse panel (different vendors) beats any single model. Seed the panel with a prior session via `context_from`. Returns the fused answer as text.",
        obj(vec![
            ("prompt", str_prop("The question or task to put to the whole panel.")),
            (
                "panel",
                obj(vec![
                    ("type", Json::Str("array".to_string())),
                    ("items", obj(vec![("type", Json::Str("string".to_string()))])),
                    (
                        "description",
                        Json::Str("Panel agents (shorthands allowed), e.g. [\"claude\",\"codex\",\"gemini\"]. Needs at least 2; duplicates allowed. Defaults to claude,codex,gemini.".to_string()),
                    ),
                ]),
            ),
            ("judge", str_prop("Agent that synthesizes the panel into the final answer. Defaults to claude.")),
            ("judge_model", str_prop("Optional model override for the judge.")),
            ("cwd", str_prop("Working directory (defaults to the server's cwd).")),
            (
                "yolo",
                obj(vec![
                    ("type", Json::Str("boolean".to_string())),
                    ("description", Json::Str("Explicitly request permission bypass for panelists and judge. Runtime policy may reject this.".to_string())),
                ]),
            ),
            (
                "context_from",
                obj(vec![
                    ("type", Json::Str("object".to_string())),
                    (
                        "description",
                        Json::Str("Seed every panelist with another agent's session transcript.".to_string()),
                    ),
                    (
                        "properties",
                        obj(vec![
                            ("harness", str_prop("Source agent (claude, codex, grok, opencode, pi).")),
                            ("session", str_prop("Session id, or 'latest' / omitted for the newest in cwd.")),
                        ]),
                    ),
                    ("required", Json::Array(vec![Json::Str("harness".to_string())])),
                ]),
            ),
        ]),
        vec!["prompt"],
    );

    let grant_schema = obj(vec![
        ("type", Json::Str("object".to_string())),
        (
            "properties",
            obj(vec![
                (
                    "kind",
                    obj(vec![
                        ("type", Json::Str("string".to_string())),
                        (
                            "enum",
                            Json::Array(vec![
                                Json::Str("file".to_string()),
                                Json::Str("tree".to_string()),
                            ]),
                        ),
                    ]),
                ),
                ("path", str_prop("Relative non-glob writable path.")),
            ]),
        ),
        (
            "required",
            Json::Array(vec![
                Json::Str("kind".to_string()),
                Json::Str("path".to_string()),
            ]),
        ),
    ]);
    let lane_schema = obj(vec![
        ("type", Json::Str("object".to_string())),
        (
            "properties",
            obj(vec![
                ("lane_id", str_prop("Stable lane identifier.")),
                (
                    "owner",
                    obj(vec![
                        ("type", Json::Str("string".to_string())),
                        (
                            "enum",
                            Json::Array(
                                ["codex", "grok_parent", "grok_child"]
                                    .into_iter()
                                    .map(|value| Json::Str(value.to_string()))
                                    .collect(),
                            ),
                        ),
                    ]),
                ),
                (
                    "role",
                    obj(vec![
                        ("type", Json::Str("string".to_string())),
                        (
                            "enum",
                            Json::Array(vec![
                                Json::Str("writer".to_string()),
                                Json::Str("reviewer".to_string()),
                            ]),
                        ),
                    ]),
                ),
                (
                    "worktree",
                    str_prop("Absolute pre-created Git worktree path."),
                ),
                (
                    "writable_paths",
                    obj(vec![
                        ("type", Json::Str("array".to_string())),
                        ("items", grant_schema),
                    ]),
                ),
            ]),
        ),
        (
            "required",
            Json::Array(
                ["lane_id", "owner", "role", "worktree", "writable_paths"]
                    .into_iter()
                    .map(|value| Json::Str(value.to_string()))
                    .collect(),
            ),
        ),
    ]);
    let lane_plan_prop = obj(vec![
        ("type", Json::Str("object".to_string())),
        (
            "properties",
            obj(vec![
                (
                    "base_commit",
                    str_prop("Exact 40-character immutable Git base commit."),
                ),
                (
                    "integration_worktree",
                    str_prop("Absolute read-only integration worktree path."),
                ),
                (
                    "lanes",
                    obj(vec![
                        ("type", Json::Str("array".to_string())),
                        ("items", lane_schema),
                        ("minItems", Json::Number(2.0)),
                        ("maxItems", Json::Number(4.0)),
                    ]),
                ),
            ]),
        ),
        (
            "required",
            Json::Array(
                ["base_commit", "integration_worktree", "lanes"]
                    .into_iter()
                    .map(|value| Json::Str(value.to_string()))
                    .collect(),
            ),
        ),
    ]);
    let mut start_properties = ask_tool
        .get("inputSchema")
        .and_then(|schema| schema.get("properties"))
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    start_properties.insert(
        "job_id".to_string(),
        str_prop("Caller-generated idempotency UUID."),
    );
    start_properties.insert(
        "job_mode".to_string(),
        obj(vec![
            ("type", Json::Str("string".to_string())),
            (
                "enum",
                Json::Array(
                    ["write", "review", "probe"]
                        .into_iter()
                        .map(|value| Json::Str(value.to_string()))
                        .collect(),
                ),
            ),
        ]),
    );
    start_properties.insert(
        "handoff_mode".to_string(),
        obj(vec![
            ("type", Json::Str("string".to_string())),
            (
                "enum",
                Json::Array(
                    ["none", "required"]
                        .into_iter()
                        .map(|value| Json::Str(value.to_string()))
                        .collect(),
                ),
            ),
        ]),
    );
    start_properties.insert("lane_plan".to_string(), lane_plan_prop);
    let start_job_tool = tool(
        "start_agent_job",
        "Start one idempotent contained Grok job and return after its request is logged and the process is running, or with its exact terminal preflight result.",
        Json::Object(start_properties),
        vec!["job_id", "job_mode", "harness", "prompt"],
    );
    let get_job_tool = tool(
        "get_agent_job",
        "Read one asynchronous job, including its exact terminal reply or error when available.",
        obj(vec![("job_id", str_prop("Caller-generated job UUID."))]),
        vec!["job_id"],
    );
    let peek_job_tool = tool(
        "peek_agent_job",
        "Read bounded sanitized peer activity and an exact visible-output excerpt without inferring whether the peer is stalled.",
        obj(vec![
            ("job_id", str_prop("Caller-generated job UUID.")),
            (
                "since_ms",
                obj(vec![
                    ("type", Json::Str("integer".to_string())),
                    ("minimum", Json::Number(0.0)),
                ]),
            ),
        ]),
        vec!["job_id"],
    );
    let ack_handoff_tool = tool(
        "ack_agent_handoff",
        "Record Codex receipt of one exact durable handoff. Receipt is not approval, acceptance, recovery, or continuity authorization.",
        obj(vec![
            ("job_id", str_prop("Caller-generated job UUID.")),
            ("handoff_id", str_prop("Collision-resistant durable handoff UUID.")),
        ]),
        vec!["job_id", "handoff_id"],
    );
    let list_job_tool = tool(
        "list_agent_jobs",
        "List newest-first bounded asynchronous job metadata without replies.",
        obj(vec![(
            "limit",
            obj(vec![
                ("type", Json::Str("integer".to_string())),
                ("minimum", Json::Number(1.0)),
                ("maximum", Json::Number(MAX_LISTED_JOBS as f64)),
            ]),
        )]),
        vec![],
    );
    let cancel_job_tool = tool(
        "cancel_agent_job",
        "Cancel one exact active job and its contained process tree without retrying it.",
        obj(vec![("job_id", str_prop("Caller-generated job UUID."))]),
        vec!["job_id"],
    );

    obj(vec![(
        "tools",
        Json::Array(vec![
            list_tool,
            last_tool,
            resume_tool,
            ask_tool,
            start_job_tool,
            get_job_tool,
            peek_job_tool,
            ack_handoff_tool,
            list_job_tool,
            cancel_job_tool,
            fuse_tool,
        ]),
    )])
}

fn call_tool(
    request: &Json,
    default_cwd: &Path,
    policy: &RuntimePolicy,
    jobs: &JobRuntime,
    delivery: &mut Option<ExchangeReceipt>,
) -> Result<Json, (i64, String)> {
    let params = request
        .get("params")
        .ok_or((-32602, "missing params".to_string()))?;
    let name = params
        .get("name")
        .and_then(Json::as_str)
        .ok_or((-32602, "missing tool name".to_string()))?;
    let empty = Json::Object(BTreeMap::new());
    let args = params.get("arguments").unwrap_or(&empty);

    let cwd = arg_cwd(args, default_cwd);
    let harness = args.get("harness").and_then(Json::as_str);

    match name {
        "list_sessions" => {
            let json = session::list_sessions_json(&cwd, harness);
            Ok(text_content(&json.to_pretty_string(), false))
        }
        "get_last_session" => match session::last_session_json(&cwd, harness) {
            Some(json) => Ok(text_content(&json.to_pretty_string(), false)),
            None => Ok(text_content(
                &format!("No resumable sessions for {}", cwd.display()),
                false,
            )),
        },
        "resume_command" => {
            let harness = harness.ok_or((-32602, "missing harness".to_string()))?;
            let id = args.get("id").and_then(Json::as_str).unwrap_or("");
            let yolo = args.get("yolo").and_then(Json::as_bool).unwrap_or(false);
            if yolo {
                policy
                    .resolve_mcp_yolo(Some(true))
                    .map_err(|error| (-32602, error))?;
            }
            match session::resume_command_string(harness, id, &cwd, yolo) {
                Ok(cmd) => Ok(text_content(&cmd, false)),
                Err(e) => Ok(text_content(&e, true)),
            }
        }
        "ask_agent" => {
            let request = parse_ask_request(args, default_cwd, policy)?;
            let harness = request.harness.clone();
            match ask::run_with_receipt(&request) {
                Ok(outcome) => match outcome.captured.reply() {
                    Ok(reply) => {
                        if is_logged_grok_response(&outcome.receipt) {
                            *delivery = Some(outcome.receipt);
                        }
                        Ok(text_content(&reply, false))
                    }
                    Err(msg) => Ok(text_content(&format!("{harness} failed: {msg}"), true)),
                },
                Err(failure) => {
                    if let Some(receipt) = failure
                        .receipt
                        .filter(|receipt| is_logged_grok_response(receipt))
                    {
                        *delivery = Some(*receipt);
                    }
                    Ok(text_content(&failure.message, true))
                }
            }
        }
        "start_agent_job" => {
            let job_id = required_arg_string(args, "job_id")?;
            let mode = JobMode::parse(&required_arg_string(args, "job_mode")?)
                .map_err(|error| (-32602, error))?;
            let requested_handoff_mode = args
                .get("handoff_mode")
                .and_then(Json::as_str)
                .map(HandoffMode::parse)
                .transpose()
                .map_err(|error| (-32602, error))?;
            let handoff_mode = policy
                .resolve_handoff_mode(mode, requested_handoff_mode)
                .map_err(|error| (-32602, error))?;
            let request = parse_ask_request(args, default_cwd, policy)?;
            let lane_plan = args
                .get("lane_plan")
                .map(crate::lane_plan::parse)
                .transpose()
                .map_err(|error| (-32602, error))?;
            match jobs.start(JobSpec {
                job_id,
                mode,
                handoff_mode,
                request,
                lane_plan,
            }) {
                Ok(lookup) => {
                    if let Some(receipt) = lookup.delivery.clone() {
                        *delivery = Some(receipt);
                    }
                    Ok(text_content(
                        &job_runtime::lookup_json(&lookup, true).to_pretty_string(),
                        false,
                    ))
                }
                Err(error) => Ok(text_content(&error, true)),
            }
        }
        "get_agent_job" => {
            let job_id = required_arg_string(args, "job_id")?;
            match jobs.get(&job_id) {
                Ok(Some(lookup)) => {
                    if let Some(receipt) = lookup.delivery.clone() {
                        *delivery = Some(receipt);
                    }
                    Ok(text_content(
                        &job_runtime::lookup_json(&lookup, true).to_pretty_string(),
                        false,
                    ))
                }
                Ok(None) => Ok(text_content(&format!("job {job_id} was not found"), true)),
                Err(error) => Ok(text_content(&error, true)),
            }
        }
        "peek_agent_job" => {
            let job_id = required_arg_string(args, "job_id")?;
            let since_ms = optional_nonnegative_u128(args, "since_ms")?;
            match jobs.peek(&job_id, since_ms) {
                Ok(activity) => Ok(text_content(&activity.to_pretty_string(), false)),
                Err(error) => Ok(text_content(&error, true)),
            }
        }
        "ack_agent_handoff" => {
            policy
                .require_codex_handoff_caller()
                .map_err(|error| (-32602, error))?;
            let job_id = required_arg_string(args, "job_id")?;
            let handoff_id = required_arg_string(args, "handoff_id")?;
            match jobs.acknowledge(&job_id, &handoff_id) {
                Ok(lookup) => Ok(text_content(
                    &job_runtime::lookup_json(&lookup, true).to_pretty_string(),
                    false,
                )),
                Err(error) => Ok(text_content(&error, true)),
            }
        }
        "list_agent_jobs" => {
            let limit = optional_positive_usize(args, "limit")?.unwrap_or(20);
            match jobs.list(limit) {
                Ok(views) => Ok(text_content(
                    &Json::Array(
                        views
                            .iter()
                            .map(|view| job_runtime::view_json(view, false))
                            .collect(),
                    )
                    .to_pretty_string(),
                    false,
                )),
                Err(error) => Ok(text_content(&error, true)),
            }
        }
        "cancel_agent_job" => {
            let job_id = required_arg_string(args, "job_id")?;
            match jobs.cancel(&job_id) {
                Ok(lookup) => Ok(text_content(
                    &job_runtime::lookup_json(&lookup, true).to_pretty_string(),
                    false,
                )),
                Err(error) => Ok(text_content(&error, true)),
            }
        }
        "fuse" => {
            let prompt = args
                .get("prompt")
                .and_then(Json::as_str)
                .ok_or((-32602, "missing prompt".to_string()))?;
            let cwd = policy
                .validate_spawn_cwd(&cwd)
                .map_err(|error| (-32602, error))?;
            let yolo = policy
                .resolve_mcp_yolo(args.get("yolo").and_then(Json::as_bool))
                .map_err(|error| (-32602, error))?;

            // Panel: explicit array of agent codes, else the default trio.
            let panel_arg: Vec<String> = match args.get("panel").and_then(Json::as_array) {
                Some(items) => items
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_string)
                    .collect(),
                None => Vec::new(),
            };
            let panel = match fuse::resolve_panel(&panel_arg) {
                Ok(p) => p,
                Err(e) => return Ok(text_content(&e, true)),
            };

            let context = args.get("context_from").and_then(|c| {
                c.get("harness").and_then(Json::as_str).map(|h| ContextRef {
                    harness: h.to_string(),
                    session: c
                        .get("session")
                        .and_then(Json::as_str)
                        .unwrap_or("")
                        .to_string(),
                    mode: crate::context::ContextMode::Snapshot,
                    mode_explicit: false,
                    recovery: None,
                })
            });
            let max_context = args
                .get("max_context")
                .and_then(Json::as_number)
                .map(|n| n as usize)
                .unwrap_or(session::DEFAULT_CONTEXT_CHARS);
            let judge = normalize_harness(
                args.get("judge")
                    .and_then(Json::as_str)
                    .unwrap_or(fuse::DEFAULT_JUDGE),
            );
            let judge_model = args
                .get("judge_model")
                .and_then(Json::as_str)
                .map(str::to_string);

            // Fan the prompt out to the panel in parallel, then have the judge
            // (Claude by default) synthesize one answer from the replies.
            let (answers, skipped) = fuse::split_replies(fuse::run_panel(
                prompt,
                &panel,
                context,
                &cwd,
                max_context,
                yolo,
            ));
            if answers.len() < 2 {
                return Ok(text_content(
                    &fuse::insufficient_panel_message(answers.len(), &skipped),
                    true,
                ));
            }
            let note = if skipped.is_empty() {
                String::new()
            } else {
                format!("(skipped: {})\n\n", skipped.join(", "))
            };
            match fuse::run_judge(
                prompt,
                &answers,
                &judge,
                judge_model,
                &cwd,
                max_context,
                yolo,
            ) {
                Ok(fused) => Ok(text_content(&format!("{note}{fused}"), false)),
                Err(e) => Ok(text_content(&format!("judge {judge} failed: {e}"), true)),
            }
        }
        other => Err((-32602, format!("unknown tool: {other}"))),
    }
}

fn parse_ask_request(
    args: &Json,
    default_cwd: &Path,
    policy: &RuntimePolicy,
) -> Result<AskRequest, (i64, String)> {
    let harness = required_arg_string(args, "harness")?;
    let prompt = required_arg_string(args, "prompt")?;
    let context = parse_ask_context(args)?;
    let cwd = policy
        .validate_spawn_cwd(&arg_cwd(args, default_cwd))
        .map_err(|error| (-32602, error))?;
    let yolo = policy
        .resolve_mcp_yolo(args.get("yolo").and_then(Json::as_bool))
        .map_err(|error| (-32602, error))?;
    Ok(AskRequest {
        harness,
        prompt,
        model: args.get("model").and_then(Json::as_str).map(str::to_string),
        reasoning_effort: args
            .get("reasoning_effort")
            .and_then(Json::as_str)
            .map(str::to_string),
        provider: args
            .get("provider")
            .and_then(Json::as_str)
            .map(str::to_string),
        cwd,
        permission_mode: args
            .get("permission_mode")
            .and_then(Json::as_str)
            .map(str::to_string),
        max_turns: optional_positive_integer(args, "max_turns")?,
        session_id: args
            .get("session_id")
            .and_then(Json::as_str)
            .map(str::to_string),
        resume_id: args
            .get("resume_id")
            .and_then(Json::as_str)
            .map(str::to_string),
        yolo,
        context,
        max_context_chars: optional_positive_usize(args, "max_context_chars")?,
        allow_subagents: false,
    })
}

fn required_arg_string(args: &Json, name: &str) -> Result<String, (i64, String)> {
    args.get(name)
        .and_then(Json::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or((-32602, format!("missing {name}")))
}

fn is_logged_grok_response(receipt: &ExchangeReceipt) -> bool {
    receipt.target == "grok"
        && receipt.request.logged
        && receipt.request.event_type == "request"
        && receipt.completion.logged
        && receipt.completion.event_type == "response"
        && receipt.request.exchange_id == receipt.completion.exchange_id
}

fn parse_ask_context(args: &Json) -> Result<Option<ContextRef>, (i64, String)> {
    let Some(context) = args.get("context_from") else {
        return Ok(None);
    };
    if context.as_object().is_none() {
        return Err((-32602, "context_from must be an object".to_string()));
    }
    let string_field = |name: &str| -> Result<Option<&str>, (i64, String)> {
        match context.get(name) {
            Some(value) => value
                .as_str()
                .map(Some)
                .ok_or((-32602, format!("context_from.{name} must be a string"))),
            None => Ok(None),
        }
    };
    let mode_raw = string_field("mode")?;
    let mode = mode_raw
        .map(crate::context::ContextMode::parse)
        .transpose()
        .map_err(|error| (-32602, error))?
        .unwrap_or_default();
    let recovery = string_field("recovery")?
        .map(crate::context::ContextRecovery::parse)
        .transpose()
        .map_err(|error| (-32602, error))?;
    Ok(Some(ContextRef {
        harness: string_field("harness")?.unwrap_or("").to_string(),
        session: string_field("session")?.unwrap_or("").to_string(),
        mode,
        mode_explicit: mode_raw.is_some(),
        recovery,
    }))
}

fn optional_positive_integer(args: &Json, name: &str) -> Result<Option<String>, (i64, String)> {
    let Some(value) = args.get(name) else {
        return Ok(None);
    };
    let number = value
        .as_number()
        .ok_or((-32602, format!("{name} must be an integer")))?;
    if number < 1.0 || number.fract() != 0.0 {
        return Err((-32602, format!("{name} must be a positive integer")));
    }
    Ok(Some((number as u64).to_string()))
}

fn optional_positive_usize(args: &Json, name: &str) -> Result<Option<usize>, (i64, String)> {
    let Some(value) = args.get(name) else {
        return Ok(None);
    };
    let number = value
        .as_number()
        .ok_or((-32602, format!("{name} must be an integer")))?;
    if number < 1.0 || number.fract() != 0.0 || number > usize::MAX as f64 {
        return Err((-32602, format!("{name} must be a positive integer")));
    }
    Ok(Some(number as usize))
}

fn optional_nonnegative_u128(args: &Json, name: &str) -> Result<Option<u128>, (i64, String)> {
    let Some(value) = args.get(name) else {
        return Ok(None);
    };
    let number = value
        .as_number()
        .ok_or((-32602, format!("{name} must be an integer")))?;
    if number < 0.0 || number.fract() != 0.0 || !number.is_finite() {
        return Err((-32602, format!("{name} must be a non-negative integer")));
    }
    Ok(Some(number as u128))
}

fn arg_cwd(args: &Json, default_cwd: &Path) -> PathBuf {
    args.get("cwd")
        .and_then(Json::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| default_cwd.to_path_buf())
}

// ---- JSON-RPC envelope + MCP content helpers -------------------------------

fn success_response(id: &Json, result: Json) -> Json {
    obj(vec![
        ("jsonrpc", Json::Str("2.0".to_string())),
        ("id", id.clone()),
        ("result", result),
    ])
}

fn error_response(id: &Json, code: i64, message: &str) -> Json {
    obj(vec![
        ("jsonrpc", Json::Str("2.0".to_string())),
        ("id", id.clone()),
        (
            "error",
            obj(vec![
                ("code", Json::Number(code as f64)),
                ("message", Json::Str(message.to_string())),
            ]),
        ),
    ])
}

/// An MCP `tools/call` result: a single text content block.
fn text_content(text: &str, is_error: bool) -> Json {
    obj(vec![
        (
            "content",
            Json::Array(vec![obj(vec![
                ("type", Json::Str("text".to_string())),
                ("text", Json::Str(text.to_string())),
            ])]),
        ),
        ("isError", Json::Bool(is_error)),
    ])
}

fn tool(name: &str, description: &str, properties: Json, required: Vec<&str>) -> Json {
    let schema = obj(vec![
        ("type", Json::Str("object".to_string())),
        ("properties", properties),
        (
            "required",
            Json::Array(
                required
                    .into_iter()
                    .map(|r| Json::Str(r.to_string()))
                    .collect(),
            ),
        ),
    ]);
    obj(vec![
        ("name", Json::Str(name.to_string())),
        ("description", Json::Str(description.to_string())),
        ("inputSchema", schema),
    ])
}

fn obj(pairs: Vec<(&str, Json)>) -> Json {
    let mut map = BTreeMap::new();
    for (key, value) in pairs {
        map.insert(key.to_string(), value);
    }
    Json::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_log::EventReceipt;
    use std::io::ErrorKind;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    enum FailureStage {
        Write,
        Flush,
    }

    struct FailingWriter {
        stage: FailureStage,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            match self.stage {
                FailureStage::Write => Err(io::Error::new(ErrorKind::BrokenPipe, "closed")),
                FailureStage::Flush => Ok(buffer.len()),
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            match self.stage {
                FailureStage::Write => Ok(()),
                FailureStage::Flush => Err(io::Error::new(ErrorKind::BrokenPipe, "closed")),
            }
        }
    }

    fn cwd() -> PathBuf {
        PathBuf::from("/tmp/nonexistent-par-test-dir")
    }

    fn temp_root() -> PathBuf {
        env::temp_dir().join(format!(
            "parley-mcp-policy-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn tool_request(name: &str, arguments: &str) -> Json {
        Json::parse(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"tools/call\",\"params\":{{\"name\":\"{name}\",\"arguments\":{arguments}}}}}"
        ))
        .unwrap()
    }

    fn json_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "\\\\")
    }

    fn rpc_error(response: &Json) -> &str {
        response
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Json::as_str)
            .expect("expected JSON-RPC error")
    }

    fn delivery_receipt() -> ExchangeReceipt {
        let event = |event_type: &str, event_id: &str| EventReceipt {
            event_id: event_id.to_string(),
            exchange_id: "exchange-delivery-1".to_string(),
            timestamp_ms: 42,
            event_type: event_type.to_string(),
            session_id: Some("01a06582-d66e-7811-b0c9-0b0266e17903".to_string()),
            logged: true,
        };
        ExchangeReceipt {
            request: event("request", "event-request-1"),
            completion: event("response", "event-response-1"),
            target: "grok".to_string(),
        }
    }

    #[test]
    fn stdout_write_and_flush_failures_report_exact_undelivered_response_once() {
        for (label, stage) in [
            ("write", FailureStage::Write),
            ("flush", FailureStage::Flush),
        ] {
            let inbox = temp_root().join(label);
            let reporter = HealthReporter::at(inbox.clone());
            let response = HandledResponse {
                json: success_response(&Json::Number(1.0), text_content("exact reply", false)),
                delivery: Some(delivery_receipt()),
            };
            let error =
                write_mcp_response(&mut FailingWriter { stage }, &response, &reporter).unwrap_err();
            assert!(error.contains(&format!("stdout {label} error")));
            assert!(error.contains("event-response-1"));
            assert!(error.contains("exchange-delivery-1"));

            let files = fs::read_dir(&inbox)
                .unwrap()
                .flatten()
                .map(|entry| entry.path())
                .collect::<Vec<_>>();
            assert_eq!(files.len(), 1);
            let record = Json::parse(&fs::read_to_string(&files[0]).unwrap()).unwrap();
            assert_eq!(
                record.get("class").and_then(Json::as_str),
                Some("mcp_stdout_undelivered")
            );
            assert_eq!(
                record.get("event_id").and_then(Json::as_str),
                Some("event-response-1")
            );
            assert_eq!(
                record.get("exchange_id").and_then(Json::as_str),
                Some("exchange-delivery-1")
            );
            fs::remove_dir_all(inbox.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn notification_gets_no_reply() {
        let req = Json::parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert!(handle_request(&req, &cwd()).is_none());
    }

    #[test]
    fn initialize_reports_server_and_tools_capability() {
        let req =
            Json::parse(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).unwrap();
        let resp = handle_request(&req, &cwd()).unwrap();
        let result = resp.get("result").unwrap();
        assert_eq!(
            result.get("protocolVersion").and_then(Json::as_str),
            Some(PROTOCOL_VERSION)
        );
        assert_eq!(
            result
                .get("serverInfo")
                .and_then(|s| s.get("name"))
                .and_then(Json::as_str),
            Some("parley")
        );
        assert!(result
            .get("capabilities")
            .and_then(|c| c.get("tools"))
            .is_some());
    }

    #[test]
    fn tools_list_exposes_all_tools() {
        let req =
            Json::parse(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#).unwrap();
        let resp = handle_request(&req, &cwd()).unwrap();
        let tools = resp
            .get("result")
            .and_then(|r| r.get("tools"))
            .and_then(Json::as_array)
            .unwrap();
        assert_eq!(tools.len(), 11);
        let names: Vec<_> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(Json::as_str))
            .collect();
        assert!(names.contains(&"list_sessions"));
        assert!(names.contains(&"get_last_session"));
        assert!(names.contains(&"resume_command"));
        assert!(names.contains(&"ask_agent"));
        assert!(names.contains(&"start_agent_job"));
        assert!(names.contains(&"get_agent_job"));
        assert!(names.contains(&"peek_agent_job"));
        assert!(names.contains(&"ack_agent_handoff"));
        assert!(names.contains(&"list_agent_jobs"));
        assert!(names.contains(&"cancel_agent_job"));
        assert!(names.contains(&"fuse"));
    }

    #[test]
    fn async_job_schema_exposes_idempotency_modes_and_lane_grants() {
        let result = tools_list_result();
        let tools = result.get("tools").and_then(Json::as_array).unwrap();
        let start = tools
            .iter()
            .find(|tool| tool.get("name").and_then(Json::as_str) == Some("start_agent_job"))
            .unwrap();
        let schema = start.get("inputSchema").unwrap();
        let required = schema
            .get("required")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .filter_map(Json::as_str)
            .collect::<Vec<_>>();
        for name in ["job_id", "job_mode", "harness", "prompt"] {
            assert!(required.contains(&name));
        }
        let properties = schema.get("properties").unwrap();
        assert_eq!(
            properties
                .get("job_mode")
                .and_then(|property| property.get("enum"))
                .and_then(Json::as_array)
                .unwrap()
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>(),
            vec!["write", "review", "probe"]
        );
        assert_eq!(
            properties
                .get("handoff_mode")
                .and_then(|property| property.get("enum"))
                .and_then(Json::as_array)
                .unwrap()
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>(),
            vec!["none", "required"]
        );
        let plan = properties.get("lane_plan").unwrap();
        let lanes = plan
            .get("properties")
            .and_then(|value| value.get("lanes"))
            .unwrap();
        assert_eq!(lanes.get("minItems").and_then(Json::as_number), Some(2.0));
        assert_eq!(lanes.get("maxItems").and_then(Json::as_number), Some(4.0));
    }

    #[test]
    fn ask_schema_exposes_grok_session_and_permission_controls() {
        let result = tools_list_result();
        let tools = result.get("tools").and_then(Json::as_array).unwrap();
        let ask = tools
            .iter()
            .find(|tool| tool.get("name").and_then(Json::as_str) == Some("ask_agent"))
            .unwrap();
        let properties = ask
            .get("inputSchema")
            .and_then(|schema| schema.get("properties"))
            .unwrap();
        for name in [
            "permission_mode",
            "reasoning_effort",
            "max_turns",
            "session_id",
            "resume_id",
            "yolo",
        ] {
            assert!(properties.get(name).is_some(), "missing {name}");
        }
        assert_eq!(
            properties
                .get("reasoning_effort")
                .and_then(|property| property.get("enum"))
                .and_then(Json::as_array)
                .unwrap()
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>(),
            vec!["low", "medium", "high", "xhigh"]
        );
        let context = properties.get("context_from").unwrap();
        let context_properties = context.get("properties").unwrap();
        assert_eq!(
            context_properties
                .get("mode")
                .and_then(|property| property.get("enum"))
                .and_then(Json::as_array)
                .unwrap()
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>(),
            vec!["snapshot", "auto", "seed", "delta"]
        );
        assert_eq!(
            context_properties
                .get("recovery")
                .and_then(|property| property.get("enum"))
                .and_then(Json::as_array)
                .unwrap()
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>(),
            vec!["replay", "skip"]
        );
    }

    #[test]
    fn ask_context_parser_preserves_omissions_and_rejects_invalid_values() {
        let omitted = Json::parse(r#"{"context_from":{"session":"source-1"}}"#).unwrap();
        let context = parse_ask_context(&omitted).unwrap().unwrap();
        assert!(context.harness.is_empty());
        assert_eq!(context.session, "source-1");
        assert_eq!(context.mode, crate::context::ContextMode::Snapshot);
        assert!(!context.mode_explicit);

        let explicit = Json::parse(
            r#"{"context_from":{"harness":"codex","session":"source-1","mode":"auto","recovery":"skip"}}"#,
        )
        .unwrap();
        let context = parse_ask_context(&explicit).unwrap().unwrap();
        assert_eq!(context.mode, crate::context::ContextMode::Auto);
        assert_eq!(
            context.recovery,
            Some(crate::context::ContextRecovery::Skip)
        );
        assert!(context.mode_explicit);

        for bad in [
            r#"{"context_from":{"mode":"rolling"}}"#,
            r#"{"context_from":{"recovery":"guess"}}"#,
            r#"{"context_from":"codex:source-1"}"#,
        ] {
            let args = Json::parse(bad).unwrap();
            assert!(parse_ask_context(&args).is_err());
        }
    }

    #[test]
    fn spawn_tools_reject_root_missing_and_sibling_paths() {
        let root = temp_root();
        let child = root.join("worker");
        let sibling = root.with_file_name(format!(
            "{}-sibling",
            root.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let policy = RuntimePolicy::for_test(&root, false, true);

        for bad in [&root, &sibling, &root.join("missing")] {
            let request = tool_request(
                "ask_agent",
                &format!(
                    "{{\"harness\":\"grok\",\"prompt\":\"no spawn\",\"cwd\":\"{}\",\"yolo\":false}}",
                    json_path(bad)
                ),
            );
            let response = handle_request_with_policy(&request, &child, &policy).unwrap();
            let error = rpc_error(&response);
            assert!(
                error.contains("outside the approved worker root")
                    || error.contains("is unavailable"),
                "{error}"
            );
        }

        let fuse = tool_request(
            "fuse",
            &format!(
                "{{\"prompt\":\"no spawn\",\"cwd\":\"{}\",\"yolo\":false}}",
                json_path(&root)
            ),
        );
        let response = handle_request_with_policy(&fuse, &child, &policy).unwrap();
        assert!(rpc_error(&response).contains("outside the approved worker root"));

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(sibling).unwrap();
    }

    #[test]
    fn hardened_mcp_policy_rejects_explicit_yolo_before_spawn() {
        let root = temp_root();
        let child = root.join("worker");
        fs::create_dir_all(&child).unwrap();
        let policy = RuntimePolicy::for_test(&root, false, true);
        let request = tool_request(
            "ask_agent",
            &format!(
                "{{\"harness\":\"grok\",\"prompt\":\"no spawn\",\"cwd\":\"{}\",\"yolo\":true}}",
                json_path(&child)
            ),
        );
        let response = handle_request_with_policy(&request, &child, &policy).unwrap();
        assert!(rpc_error(&response).contains("permission bypass is disabled"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let req = Json::parse(r#"{"jsonrpc":"2.0","id":3,"method":"bogus"}"#).unwrap();
        let resp = handle_request(&req, &cwd()).unwrap();
        assert_eq!(
            resp.get("error")
                .and_then(|e| e.get("code"))
                .and_then(Json::as_number),
            Some(-32601.0)
        );
    }

    #[test]
    fn resume_command_builds_native_command() {
        let req = Json::parse(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"resume_command","arguments":{"harness":"claude","id":"abc-123"}}}"#,
        )
        .unwrap();
        let resp = handle_request(&req, &cwd()).unwrap();
        let text = resp
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(Json::as_array)
            .and_then(|c| c.first())
            .and_then(|b| b.get("text"))
            .and_then(Json::as_str)
            .unwrap();
        assert_eq!(text, "claude --resume abc-123");
    }

    #[test]
    fn list_sessions_returns_array_text() {
        let req = Json::parse(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}"#,
        )
        .unwrap();
        let resp = handle_request(&req, &cwd()).unwrap();
        let is_error = resp
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(Json::as_bool)
            .unwrap();
        assert!(!is_error);
    }
}
