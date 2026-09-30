use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

const BROKER_FILE_NAME: &str = "parley-continuation-broker.exe";
const MAX_BROKER_OUTPUT_BYTES: usize = 1_048_576;
const MAX_USER_TEXT_CHARS: usize = 8_000;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct ContinuationEvidence {
    pub schema_version: u64,
    pub continuation_id: String,
    pub manifest_id: String,
    pub state: String,
    pub prompt: String,
    pub reply: Option<String>,
    pub error: Option<String>,
    pub exchange_id: Option<String>,
    pub request_event_id: Option<String>,
    pub completion_event_id: Option<String>,
    pub changed_paths: Vec<String>,
    pub validation_state: String,
    pub diff_fingerprint: String,
    pub evidence_hash: String,
    pub completed_at_ms: u64,
    #[serde(default)]
    pub user_received: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct ContinuationStatus {
    pub schema_version: u64,
    pub state: String,
    pub message: Option<String>,
    pub manifest_id: Option<String>,
    pub worktree_name: Option<String>,
    #[serde(default)]
    pub writable_paths: Vec<String>,
    pub expires_at_ms: Option<u64>,
    #[serde(default)]
    pub continuation_count: u64,
    pub running_continuation_id: Option<String>,
    pub latest: Option<ContinuationEvidence>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuationSubmission {
    pub continuation_id: String,
    pub state: String,
}

pub fn status<R: Runtime>(app: &AppHandle<R>) -> Result<ContinuationStatus, String> {
    let broker = resolve_broker(app)?;
    run_json(&broker, &["continuation", "status"], None)
}

pub fn submit<R: Runtime>(
    app: &AppHandle<R>,
    text: &str,
) -> Result<ContinuationSubmission, String> {
    validate_user_text(text)?;
    let continuation_id = create_uuid()?;
    let broker = resolve_broker(app)?;
    let _: ContinuationEvidence = run_json(
        &broker,
        &["continuation", "submit", "--id", &continuation_id],
        Some(text.as_bytes()),
    )?;
    Ok(ContinuationSubmission {
        continuation_id,
        state: "completed".to_string(),
    })
}

pub fn cancel<R: Runtime>(
    app: &AppHandle<R>,
    continuation_id: &str,
) -> Result<ContinuationStatus, String> {
    validate_uuid("continuation_id", continuation_id)?;
    let broker = resolve_broker(app)?;
    let _: serde_json::Value = run_json(
        &broker,
        &["continuation", "cancel", "--id", continuation_id],
        None,
    )?;
    status(app)
}

pub fn receive<R: Runtime>(
    app: &AppHandle<R>,
    continuation_id: &str,
    evidence_hash: &str,
) -> Result<ContinuationStatus, String> {
    validate_uuid("continuation_id", continuation_id)?;
    validate_hash(evidence_hash)?;
    let broker = resolve_broker(app)?;
    let _: serde_json::Value = run_json(
        &broker,
        &[
            "continuation",
            "receive",
            "--id",
            continuation_id,
            "--evidence-hash",
            evidence_hash,
        ],
        None,
    )?;
    status(app)
}

fn run_json<T: for<'de> Deserialize<'de>>(
    broker: &Path,
    arguments: &[&str],
    stdin: Option<&[u8]>,
) -> Result<T, String> {
    let mut command = Command::new(broker);
    command
        .args(arguments)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .map_err(|error| format!("start the continuation broker: {error}"))?;
    if let Some(input) = stdin {
        let mut child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| "continuation broker stdin is unavailable".to_string())?;
        child_stdin
            .write_all(input)
            .and_then(|_| child_stdin.flush())
            .map_err(|error| format!("write the continuation request: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for the continuation broker: {error}"))?;
    if output.stdout.len() > MAX_BROKER_OUTPUT_BYTES
        || output.stderr.len() > MAX_BROKER_OUTPUT_BYTES
    {
        return Err("continuation broker output exceeded its local bound".to_string());
    }
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            format!("continuation broker exited with {}", output.status)
        } else {
            format!("continuation broker refused the request: {detail}")
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parse continuation broker output: {error}"))
}

fn resolve_broker<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("PARLEY_CONTINUATION_BROKER_EXE") {
        return validate_broker(PathBuf::from(path));
    }
    let path = app
        .path()
        .resource_dir()
        .map_err(|error| format!("resolve viewer resources: {error}"))?
        .join("continuation")
        .join(BROKER_FILE_NAME);
    validate_broker(path)
}

fn validate_broker(path: PathBuf) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path.file_name().and_then(|value| value.to_str()) != Some(BROKER_FILE_NAME)
    {
        return Err("continuation broker path is not the installed fixed executable".to_string());
    }
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("inspect continuation broker {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("continuation broker is not a regular installed file".to_string());
    }
    #[cfg(windows)]
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("continuation broker may not be a reparse point".to_string());
    }
    let resolved = fs::canonicalize(&path)
        .map_err(|error| format!("resolve continuation broker {}: {error}", path.display()))?;
    if resolved.file_name().and_then(|value| value.to_str()) != Some(BROKER_FILE_NAME) {
        return Err("resolved continuation broker has an unexpected filename".to_string());
    }
    Ok(resolved)
}

fn validate_user_text(text: &str) -> Result<(), String> {
    let count = text.chars().count();
    if text.trim().is_empty() {
        return Err("Enter a message for Grok first".to_string());
    }
    if count > MAX_USER_TEXT_CHARS {
        return Err(format!(
            "Direct messages are limited to {MAX_USER_TEXT_CHARS} characters"
        ));
    }
    if text.contains('\0') {
        return Err("Direct messages cannot contain NUL".to_string());
    }
    Ok(())
}

fn validate_uuid(label: &str, value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.len() != 36
        || ![8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes.get(index) == Some(&b'-'))
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| ![8, 13, 18, 23].contains(&index) && !byte.is_ascii_hexdigit())
    {
        return Err(format!("{label} is invalid"));
    }
    Ok(())
}

fn validate_hash(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("evidence hash is invalid".to_string())
    }
}

#[cfg(windows)]
fn create_uuid() -> Result<String, String> {
    use windows_sys::Win32::System::Com::CoCreateGuid;

    let mut guid = unsafe { std::mem::zeroed() };
    let result = unsafe { CoCreateGuid(&mut guid) };
    if result < 0 {
        return Err(format!(
            "create continuation id: HRESULT 0x{:08x}",
            result as u32
        ));
    }
    Ok(format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_user_text_and_ids() {
        assert!(validate_user_text("Please finish the current task.").is_ok());
        assert!(validate_user_text("   ").is_err());
        assert!(validate_user_text(&"x".repeat(MAX_USER_TEXT_CHARS + 1)).is_err());
        assert!(validate_uuid("id", "123e4567-e89b-12d3-a456-426614174000").is_ok());
        assert!(validate_uuid("id", "not-an-id").is_err());
        assert!(validate_hash(&"a".repeat(64)).is_ok());
        assert!(validate_hash("short").is_err());
    }

    #[test]
    fn decodes_snake_case_and_encodes_camel_case() {
        let status: ContinuationStatus = serde_json::from_str(
            r#"{"schema_version":1,"state":"armed","manifest_id":"123e4567-e89b-12d3-a456-426614174000","worktree_name":"work","writable_paths":["src"],"expires_at_ms":42,"continuation_count":0,"running_continuation_id":null,"latest":null}"#,
        )
        .unwrap();
        let encoded = serde_json::to_value(status).unwrap();
        assert_eq!(encoded["schemaVersion"], 1);
        assert_eq!(encoded["worktreeName"], "work");
        assert_eq!(encoded["writablePaths"][0], "src");
    }
}
