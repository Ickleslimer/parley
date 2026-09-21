//! Agent-to-agent calls: run one agent headless and return its reply as a
//! value, optionally seeded with another agent's session transcript.
//!
//! This is the building block behind `par ask` and the MCP `ask_agent` tool:
//! because `par` already routes a prompt to any agent, "Claude asks Gemini" is
//! just routing the prompt to Gemini headless and capturing its stdout. With a
//! `context` reference, `par` first reads the source agent's transcript (via the
//! session module) and prepends it, so the answer is informed by that history.

use std::env;
use std::path::PathBuf;
use std::time::Instant;

use crate::cli::{AskOptions, CliOptions};
use crate::event_log::{ExchangeLog, ExchangeReceipt};
use crate::harness::{normalize_harness, HarnessFactory, Invocation, Request};
use crate::health_report::HealthReporter;
use crate::policy::{PolicyRequest, RuntimePolicy};
use crate::process::{capture_invocation_timeout, Captured, Timeouts};
use crate::session;

/// A reference to a prior session to inject as context: which agent, and which
/// session (`""`/`"latest"` for the newest in the cwd, or an explicit id).
#[derive(Clone, Debug)]
pub(crate) struct ContextRef {
    pub harness: String,
    pub session: String,
}

/// A fully-resolved request to ask one agent something.
#[derive(Clone, Debug)]
pub(crate) struct AskRequest {
    pub harness: String,
    pub prompt: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub provider: Option<String>,
    pub cwd: PathBuf,
    pub permission_mode: Option<String>,
    pub max_turns: Option<String>,
    pub session_id: Option<String>,
    pub resume_id: Option<String>,
    pub yolo: bool,
    pub context: Option<ContextRef>,
    pub max_context_chars: usize,
}

pub(crate) struct AskOutcome {
    pub(crate) captured: Captured,
    pub(crate) receipt: ExchangeReceipt,
}

pub(crate) struct AskFailure {
    pub(crate) message: String,
    pub(crate) receipt: Option<Box<ExchangeReceipt>>,
}

impl AskFailure {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            receipt: None,
        }
    }

    fn after_completion(message: impl Into<String>, receipt: ExchangeReceipt) -> Self {
        Self {
            message: message.into(),
            receipt: Some(Box::new(receipt)),
        }
    }
}

impl From<String> for AskFailure {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for AskFailure {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// Build the headless invocation, injecting transcript context into the prompt
/// when requested. Separated from running so `--dry-run` can show the command.
pub(crate) fn build(req: &AskRequest) -> Result<Invocation, String> {
    let req = prepare(req)?;
    let prompt = resolved_prompt(&req)?;
    build_prepared(&req, prompt)
}

fn resolved_prompt(req: &AskRequest) -> Result<String, String> {
    Ok(match &req.context {
        Some(ctx) => {
            let preamble = session::transcript_context(
                &ctx.harness,
                &ctx.session,
                &req.cwd,
                req.max_context_chars,
            )?;
            format!(
                "{preamble}\n\n---\n\nUsing the conversation above as context, respond to this:\n\n{}",
                req.prompt
            )
        }
        None => req.prompt.clone(),
    })
}

fn build_prepared(req: &AskRequest, prompt: String) -> Result<Invocation, String> {
    let options = CliOptions {
        harness: req.harness.clone(),
        provider: req.provider.clone(),
        model: req.model.clone(),
        reasoning_effort: req.reasoning_effort.clone(),
        cwd: req.cwd.to_str().map(str::to_string),
        prompt: Some(prompt),
        permission_mode: req.permission_mode.clone(),
        max_turns: req.max_turns.clone(),
        session_id: req.session_id.clone(),
        resume_id: req.resume_id.clone(),
        yolo: req.yolo,
        ..CliOptions::default()
    };

    let request = Request::from_options(options, String::new())?;
    let harness = HarnessFactory::default().create(&request.harness)?;
    harness.build(&request)
}

/// Build and run the call, returning the target agent's captured output. A
/// watchdog (configurable via `PARLEY_TIMEOUT` / `PARLEY_IDLE_TIMEOUT`) kills a
/// hung agent so a single stuck panelist can't wedge a whole `fuse`.
pub(crate) fn run(req: &AskRequest) -> Result<Captured, String> {
    run_with_receipt(req)
        .map(|outcome| outcome.captured)
        .map_err(|failure| failure.message)
}

pub(crate) fn run_with_receipt(req: &AskRequest) -> Result<AskOutcome, AskFailure> {
    let req = prepare(req)?;
    let prompt = resolved_prompt(&req)?;
    let invocation = build_prepared(&req, prompt.clone())?;
    let log = ExchangeLog::start(&req, &prompt)?;
    let reporter = HealthReporter::from_env()?;
    if req.harness == "grok" {
        if let Err(error) = reporter.request_started(log.request_receipt()) {
            let _ = log.failure("health_preflight_error", &error, 0);
            return Err(AskFailure::new(format!(
                "health request logging failed before Grok launch; agent was not started: {error}"
            )));
        }
    }
    let started = Instant::now();
    match capture_invocation_timeout(invocation, req.cwd.to_str(), Timeouts::from_env()) {
        Ok(out) => {
            let duration_ms = started.elapsed().as_millis();
            let reply_result = out.reply();
            let log_result = match &reply_result {
                Ok(reply) => log.success(reply, duration_ms),
                Err(error) => log.failure(
                    if out.timed_out { "timeout" } else { "error" },
                    error,
                    duration_ms,
                ),
            };
            let completion = match log_result {
                Ok(receipt) => receipt,
                Err(log_error) => {
                    let health_error = if req.harness == "grok" {
                        reporter.logging_failure(log.request_receipt()).err()
                    } else {
                        None
                    };
                    let captured = reply_result.unwrap_or_else(|error| error);
                    let health_suffix = health_error
                        .map(|error| format!("; health reporting also failed: {error}"))
                        .unwrap_or_default();
                    return Err(AskFailure::new(format!(
                        "event log completion failed after the agent ran; do not retry automatically: {log_error}{health_suffix}\nCaptured result:\n{captured}"
                    )));
                }
            };
            let receipt = log.exchange_receipt(completion);
            if req.harness == "grok" {
                let health_result = if reply_result.is_ok() {
                    reporter.grok_success(&receipt)
                } else {
                    reporter.turn_failure(
                        &receipt,
                        if out.timed_out {
                            "watchdog_killed"
                        } else {
                            "turn_error"
                        },
                    )
                };
                if let Err(health_error) = health_result {
                    let captured = reply_result.unwrap_or_else(|error| error);
                    return Err(AskFailure::after_completion(
                        format!(
                            "health completion logging failed after Grok ran; do not retry automatically: {health_error}\nCaptured result:\n{captured}"
                        ),
                        receipt,
                    ));
                }
                if RuntimePolicy::from_env()?.requires_handoff_footer()
                    && reply_result
                        .as_deref()
                        .is_ok_and(|reply| !has_handoff_footer(reply))
                {
                    let _ = reporter.footer_missing(&receipt);
                }
            }
            Ok(AskOutcome {
                captured: out,
                receipt,
            })
        }
        Err(error) => {
            let duration_ms = started.elapsed().as_millis();
            let completion = match log.failure("error", &error, duration_ms) {
                Ok(receipt) => receipt,
                Err(log_error) => {
                    return Err(AskFailure::new(format!(
                        "agent launch failed: {error}; event log completion also failed: {log_error}"
                    )));
                }
            };
            if req.harness == "grok" {
                let receipt = log.exchange_receipt(completion);
                if let Err(health_error) = reporter.turn_failure(&receipt, "turn_error") {
                    return Err(AskFailure::after_completion(
                        format!(
                            "agent launch failed: {error}; health completion also failed: {health_error}"
                        ),
                        receipt,
                    ));
                }
            }
            Err(AskFailure::new(error))
        }
    }
}

fn has_handoff_footer(reply: &str) -> bool {
    [
        "TWO_CHAIRS_HANDOFF",
        "peer:",
        "evidence_class:",
        "incident_id:",
        "as_of_ms:",
        "continuity: not_authorized",
    ]
    .iter()
    .all(|field| reply.contains(field))
}

fn prepare(req: &AskRequest) -> Result<AskRequest, String> {
    let policy = RuntimePolicy::from_env()?;
    let mut prepared = req.clone();
    prepared.harness = normalize_harness(&prepared.harness);
    prepared.cwd = policy.validate_spawn_cwd(&prepared.cwd)?;
    policy.apply_request(PolicyRequest {
        harness: &prepared.harness,
        yolo: prepared.yolo,
        model: &mut prepared.model,
        reasoning_effort: &mut prepared.reasoning_effort,
        permission_mode: &mut prepared.permission_mode,
        max_turns: &mut prepared.max_turns,
        session_id: prepared.session_id.as_deref(),
        resume_id: prepared.resume_id.as_deref(),
        passthrough: &[],
    })?;
    policy.apply_handoff_contract(&prepared.harness, &mut prepared.prompt);
    Ok(prepared)
}

/// `par ask` entry point: resolve options, then run (or print under dry-run).
pub(crate) fn run_cli(options: AskOptions) -> Result<(), String> {
    let dry_run = options.dry_run;
    let req = resolve(options)?;

    if dry_run {
        println!("{}", build(&req)?.to_json());
        return Ok(());
    }

    let out = run(&req)?;
    match out.reply() {
        Ok(reply) => {
            println!("{reply}");
            Ok(())
        }
        Err(msg) => Err(format!("{} produced no reply: {msg}", req.harness)),
    }
}

fn resolve(options: AskOptions) -> Result<AskRequest, String> {
    let cwd = match options.cwd {
        Some(path) => PathBuf::from(path),
        None => env::current_dir().map_err(|e| format!("failed to get cwd: {e}"))?,
    };
    Ok(AskRequest {
        harness: options.harness.ok_or("ask requires a target agent")?,
        prompt: options.prompt.ok_or("ask requires a prompt")?,
        model: options.model,
        reasoning_effort: options.reasoning_effort,
        provider: options.provider,
        cwd,
        permission_mode: options.permission_mode,
        max_turns: options.max_turns,
        session_id: options.session_id,
        resume_id: options.resume_id,
        yolo: options.yolo,
        context: options.context_from.as_deref().map(parse_context_spec),
        max_context_chars: options
            .max_context_chars
            .unwrap_or(session::DEFAULT_CONTEXT_CHARS),
    })
}

/// Parse a `harness[:session]` context spec. Missing session means "latest".
fn parse_context_spec(spec: &str) -> ContextRef {
    match spec.split_once(':') {
        Some((harness, session)) => ContextRef {
            harness: harness.to_string(),
            session: session.to_string(),
        },
        None => ContextRef {
            harness: spec.to_string(),
            session: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_spec_defaults_to_latest() {
        let c = parse_context_spec("claude");
        assert_eq!(c.harness, "claude");
        assert_eq!(c.session, "");
    }

    #[test]
    fn context_spec_parses_session_id() {
        let c = parse_context_spec("co:abc-123");
        assert_eq!(c.harness, "co");
        assert_eq!(c.session, "abc-123");
    }

    #[test]
    fn handoff_footer_requires_every_locked_field() {
        let complete = "TWO_CHAIRS_HANDOFF\npeer: codex\nevidence_class: usage_sample\nincident_id: null\nas_of_ms: 42\ncontinuity: not_authorized";
        assert!(has_handoff_footer(complete));
        assert!(!has_handoff_footer(
            "TWO_CHAIRS_HANDOFF\npeer: codex\nevidence_class: unavailable\nincident_id: null\nas_of_ms: 42"
        ));
    }
}
