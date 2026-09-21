use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::classifier::CodexUsageSample;
use crate::schema::now_ms;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const PROTOCOL_LINE_LIMIT: usize = 1024 * 1024;
const MAX_PROTOCOL_MESSAGES: usize = 256;

pub trait CodexSampler {
    fn sample(&self) -> Result<Option<CodexUsageSample>, String>;
}

#[derive(Clone, Debug)]
pub struct AppServerSampler {
    program: OsString,
    timeout: Duration,
}

impl Default for AppServerSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl AppServerSampler {
    pub fn new() -> Self {
        Self {
            program: OsString::from("codex"),
            timeout: REQUEST_TIMEOUT,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
    }
}

impl CodexSampler for AppServerSampler {
    fn sample(&self) -> Result<Option<CodexUsageSample>, String> {
        let mut child = self
            .command()
            .spawn()
            .map_err(|error| format!("start Codex app-server sampler: {error}"))?;
        let mut stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let _ = stop_and_reap(&mut child, GRACEFUL_STOP_TIMEOUT);
                return Err("Codex app-server sampler stdin unavailable".to_string());
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                drop(stdin);
                let _ = stop_and_reap(&mut child, GRACEFUL_STOP_TIMEOUT);
                return Err("Codex app-server sampler stdout unavailable".to_string());
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                drop(stdin);
                drop(stdout);
                let _ = stop_and_reap(&mut child, GRACEFUL_STOP_TIMEOUT);
                return Err("Codex app-server sampler stderr unavailable".to_string());
            }
        };

        let (line_tx, line_rx) = mpsc::channel();
        let stdout_thread = thread::spawn(move || read_protocol(stdout, line_tx));
        let stderr_thread = thread::spawn(move || {
            let mut stderr = stderr;
            let _ = io::copy(&mut stderr, &mut io::sink());
        });
        let deadline = Instant::now() + self.timeout;
        let sample = (|| {
            send_request(
                &mut stdin,
                &json!({
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "clientInfo": {
                            "name": "parley-health-supervisor",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                }),
            )?;
            wait_for_response(&line_rx, 1, deadline)?;
            send_request(
                &mut stdin,
                &json!({"id": 2, "method": "account/rateLimits/read", "params": {}}),
            )?;
            let response = wait_for_response(&line_rx, 2, deadline)?;
            parse_rate_limits(&response, now_ms())
        })();
        drop(stdin);

        stop_and_reap(&mut child, GRACEFUL_STOP_TIMEOUT)?;
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        sample.map(Some)
    }
}

fn send_request(stdin: &mut impl Write, request: &Value) -> Result<(), String> {
    writeln!(stdin, "{request}")
        .and_then(|_| stdin.flush())
        .map_err(|error| format!("write Codex app-server request: {error}"))
}

fn read_protocol(mut stdout: impl Read, sender: mpsc::Sender<Result<String, String>>) {
    loop {
        let mut line = Vec::new();
        let mut byte = [0_u8; 1];
        let mut clipped = false;
        loop {
            match stdout.read(&mut byte) {
                Ok(0) => {
                    if line.is_empty() {
                        return;
                    }
                    break;
                }
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => {
                    if line.len() < PROTOCOL_LINE_LIMIT {
                        line.push(byte[0]);
                    } else {
                        clipped = true;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(format!("read Codex app-server stdout: {error}")));
                    return;
                }
            }
        }
        if clipped {
            let _ = sender.send(Err("Codex app-server response exceeded bound".to_string()));
            return;
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        match String::from_utf8(line) {
            Ok(line) => {
                if sender.send(Ok(line)).is_err() {
                    return;
                }
            }
            Err(_) => {
                let _ = sender.send(Err("Codex app-server response was not UTF-8".to_string()));
                return;
            }
        }
    }
}

fn wait_for_response(
    receiver: &Receiver<Result<String, String>>,
    id: u64,
    deadline: Instant,
) -> Result<Value, String> {
    for _ in 0..MAX_PROTOCOL_MESSAGES {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| "Codex app-server sampler timed out".to_string())?;
        let line = receiver
            .recv_timeout(remaining)
            .map_err(|_| "Codex app-server sampler timed out or closed stdout".to_string())??;
        let value: Value = serde_json::from_str(&line)
            .map_err(|_| "Codex app-server returned malformed JSON".to_string())?;
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if value.get("error").is_some() {
            return Err(format!("Codex app-server request {id} returned an error"));
        }
        return Ok(value);
    }
    Err("Codex app-server response message bound exceeded".to_string())
}

fn parse_rate_limits(response: &Value, as_of_ms: u64) -> Result<CodexUsageSample, String> {
    let limits = response
        .pointer("/result/rateLimits")
        .and_then(Value::as_object)
        .ok_or_else(|| "Codex app-server rate-limit response unsupported".to_string())?;
    let primary = limits.get("primary").and_then(Value::as_object);
    let used_percent = primary
        .and_then(|value| value.get("usedPercent"))
        .and_then(Value::as_f64);
    let resets_at = primary
        .and_then(|value| value.get("resetsAt"))
        .and_then(scalar_string);
    let plan_type = limits
        .get("planType")
        .and_then(Value::as_str)
        .map(str::to_string);
    let rate_limit_reached_type = limits
        .get("rateLimitReachedType")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(CodexUsageSample {
        used_percent,
        resets_at,
        plan_type,
        rate_limit_reached_type,
        as_of_ms,
    })
}

fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn stop_and_reap(child: &mut Child, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(None) => break,
            Err(error) => return Err(format!("wait for Codex app-server sampler: {error}")),
        }
    }
    if let Err(error) = child.kill() {
        if child.try_wait().ok().flatten().is_none() {
            return Err(format!("terminate Codex app-server sampler: {error}"));
        }
    }
    child
        .wait()
        .map(|_| ())
        .map_err(|error| format!("reap Codex app-server sampler: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_rate_limit_fields_without_inferring_exhaustion() {
        let response = json!({
            "id": 2,
            "result": {
                "rateLimits": {
                    "primary": {"usedPercent": 99, "resetsAt": 1790408516},
                    "planType": "prolite",
                    "rateLimitReachedType": null
                }
            }
        });
        let sample = parse_rate_limits(&response, 42).unwrap();
        assert_eq!(sample.used_percent, Some(99.0));
        assert_eq!(sample.resets_at.as_deref(), Some("1790408516"));
        assert_eq!(sample.plan_type.as_deref(), Some("prolite"));
        assert_eq!(sample.rate_limit_reached_type, None);
        assert_eq!(sample.as_of_ms, 42);
    }

    #[test]
    fn preserves_only_non_null_authoritative_reached_signal() {
        let response = json!({
            "result": {
                "rateLimits": {
                    "primary": {"usedPercent": 100, "resetsAt": "soon"},
                    "planType": "team",
                    "rateLimitReachedType": "primary"
                }
            }
        });
        let sample = parse_rate_limits(&response, 7).unwrap();
        assert_eq!(sample.rate_limit_reached_type.as_deref(), Some("primary"));
    }

    #[test]
    fn unsupported_response_fails_closed() {
        assert!(parse_rate_limits(&json!({"result": {}}), 1).is_err());
    }
}
