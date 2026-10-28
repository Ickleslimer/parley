#![cfg(windows)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

#[allow(dead_code)]
#[path = "../src/json.rs"]
mod json;
use json::Json;

static COUNTER: AtomicU64 = AtomicU64::new(1);
static STUB: OnceLock<PathBuf> = OnceLock::new();
const SESSION: &str = "e83aa8d6-b4d6-43c7-a2e8-f7ea154d4158";

fn scratch() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "parley-auto-brief-cli-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn stub() -> &'static PathBuf {
    STUB.get_or_init(|| {
        let root = scratch();
        let source = root.join("stub.rs");
        fs::write(&source, r#"
use std::{env,fs};
fn main() {
    let args:Vec<String>=env::args().collect();
    let prompt=if let Some(index)=args.iter().position(|arg|arg=="--prompt-file") {
        fs::read_to_string(&args[index+1]).unwrap()
    } else { args[args.iter().position(|arg|arg=="--single").unwrap()+1].clone() };
    fs::write(env::var("BRIEF_TRACE").unwrap(), &prompt).unwrap();
    let mode=env::var("BRIEF_MODE").unwrap_or_default();
    if mode=="wait" { std::thread::sleep(std::time::Duration::from_secs(20)); }
    if mode=="fail" { eprintln!("synthetic failure");std::process::exit(2); }
    println!("SYNTHETIC_RESULT");
    if mode=="omit" { return; }
    let Some(start)=prompt.lines().find(|line|line.starts_with("PARLEY_WORKING_BRIEF_V1 ")) else{return;};
    let nonce=start.strip_prefix("PARLEY_WORKING_BRIEF_V1 ").unwrap();
    let template=prompt.lines().find(|line|line.starts_with("{\"prior_sha256\":")).unwrap();
    let prior=template.split('"').nth(3).unwrap();
    println!("{start}\n{{\"prior_sha256\":\"{prior}\",\"reported_objective\":\"synthetic preserved decision\",\"reported_decisions\":[\"Use bounded context\"],\"open_questions\":[\"native review\"],\"evidence_references\":[]}}\nEND_PARLEY_WORKING_BRIEF_V1 {nonce}");
}
"#).unwrap();
        let build = Command::new("rustc").args(["--edition", "2021", "--crate-name", "working_brief_stub"])
            .arg(&source).arg("-o").arg(root.join("grok.exe")).output().unwrap();
        assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
        root
    })
}

fn configured(root: &Path, mode: &str) -> Command {
    fs::create_dir_all(root.join("worker")).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_par"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("PARLEY_") {
            command.env_remove(key);
        }
    }
    command
        .env(
            "PATH",
            format!("{};{}", stub().display(), std::env::var("PATH").unwrap()),
        )
        .env("PARLEY_WORKING_BRIEF_ENABLED", "true")
        .env("PARLEY_ASYNC_JOBS_ENABLED", "true")
        .env("PARLEY_CALLER", "brief-test")
        .env("PARLEY_ALLOWED_CWD_ROOT", root)
        .env("PARLEY_JOB_STATE_DIR", root.join("jobs"))
        .env("PARLEY_EVENT_LOG", root.join("events.jsonl"))
        .env("PARLEY_CONTEXT_STATE_DIR", root.join("context"))
        .env("BRIEF_TRACE", root.join("prompt.txt"))
        .env("BRIEF_MODE", mode);
    command
}

fn cli(root: &Path, mode: &str) -> std::process::Output {
    configured(root, mode)
        .args([
            "ask",
            "-h",
            "grok",
            "--no-yolo",
            "--resume-id",
            SESSION,
            "--cwd",
        ])
        .arg(root.join("worker"))
        .args(["-p", "Current synthetic task only."])
        .output()
        .unwrap()
}

fn head(root: &Path) -> String {
    let entries: Vec<_> = fs::read_dir(root.join("jobs/working-brief"))
        .unwrap()
        .collect();
    assert_eq!(entries.len(), 1);
    fs::read_to_string(entries[0].as_ref().unwrap().path().join("head.json")).unwrap()
}

#[test]
fn cli_restart_automatically_attaches_updates_and_preserves_failed_turns() {
    let root = scratch();
    let first = cli(&root, "normal");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(head(&root).contains("updated"));
    let second = cli(&root, "omit");
    assert!(second.status.success());
    let prompt = fs::read_to_string(root.join("prompt.txt")).unwrap();
    assert!(prompt.contains("synthetic preserved decision"));
    assert!(prompt.starts_with("Current synthetic task only."));
    let before = head(&root);
    assert!(before.contains("proposal_absent"));
    let failed = cli(&root, "fail");
    assert!(!failed.status.success());
    assert_eq!(head(&root), before);
    assert!(cli(&root, "normal").status.success());
    assert!(fs::read_to_string(root.join("prompt.txt"))
        .unwrap()
        .contains("proposal_absent"));
}

#[test]
fn timeout_and_conflicting_session_ids_cannot_publish() {
    let root = scratch();
    let result = configured(&root, "wait")
        .env("PARLEY_TIMEOUT", "1")
        .args([
            "ask",
            "-h",
            "grok",
            "--no-yolo",
            "--resume-id",
            SESSION,
            "--cwd",
        ])
        .arg(root.join("worker"))
        .args(["-p", "Synthetic timeout."])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!root.join("jobs/working-brief").exists());
    let result = configured(&root, "normal")
        .args([
            "ask",
            "-h",
            "grok",
            "--no-yolo",
            "--resume-id",
            SESSION,
            "--session-id",
            "another-session",
            "--cwd",
        ])
        .arg(root.join("worker"))
        .args(["-p", "Synthetic conflict."])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("mutually exclusive"));
    assert!(!root.join("jobs/working-brief").exists());
}

#[test]
fn disabled_profile_never_changes_request_or_creates_memory() {
    let root = scratch();
    let result = configured(&root, "normal")
        .env_remove("PARLEY_WORKING_BRIEF_ENABLED")
        .args([
            "ask",
            "-h",
            "grok",
            "--no-yolo",
            "--resume-id",
            SESSION,
            "--cwd",
        ])
        .arg(root.join("worker"))
        .args(["-p", "Exact current task."])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert_eq!(
        fs::read_to_string(root.join("prompt.txt")).unwrap(),
        "Exact current task."
    );
    assert!(!root.join("jobs/working-brief").exists());
}

struct Mcp {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Mcp {
    fn call(&mut self, id: usize, method: &str, params: Json) -> Json {
        let request = Json::Object(
            [
                ("jsonrpc".into(), Json::Str("2.0".into())),
                ("id".into(), Json::Number(id as f64)),
                ("method".into(), Json::Str(method.into())),
                ("params".into(), params),
            ]
            .into_iter()
            .collect(),
        );
        writeln!(self.input, "{}", request.to_compact_string()).unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let response = Json::parse_exact(&line).unwrap();
        assert!(response.get("error").is_none(), "{response:?}");
        response
    }

    fn tool(&mut self, id: usize, name: &str, args: Json) -> Json {
        let params = Json::Object(
            [
                ("name".into(), Json::Str(name.into())),
                ("arguments".into(), args),
            ]
            .into_iter()
            .collect(),
        );
        let response = self.call(id, "tools/call", params);
        let result = response.get("result").unwrap();
        assert_ne!(
            result.get("isError").and_then(Json::as_bool),
            Some(true),
            "{result:?}"
        );
        let text = result.get("content").unwrap().as_array().unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        Json::parse_exact(text).unwrap()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn cancelled_async_worker_does_not_publish_a_summary() {
    let root = scratch();
    let mut child = configured(&root, "wait")
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let input = child.stdin.take().unwrap();
    let output = BufReader::new(child.stdout.take().unwrap());
    let mut mcp = Mcp {
        child,
        input,
        output,
    };
    let worker = root.join("worker");
    let job = "ca2a6d96-a995-4a36-9806-2b4e4b7c553a";
    let args = Json::Object(
        [
            ("job_id", job),
            ("harness", "grok"),
            ("job_mode", "probe"),
            ("handoff_mode", "none"),
            ("conversation_mode", "isolated"),
            ("resume_id", SESSION),
            ("cwd", worker.to_str().unwrap()),
            ("prompt", "Synthetic cancellation only."),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), Json::Str(value.into())))
        .collect(),
    );
    mcp.tool(1, "start_agent_job", args);
    let args = Json::Object(
        [("job_id".into(), Json::Str(job.into()))]
            .into_iter()
            .collect(),
    );
    let mut result = mcp.tool(2, "cancel_agent_job", args.clone());
    for count in 0..50 {
        if result.get("state").and_then(Json::as_str) == Some("cancelled") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        result = mcp.tool(count + 3, "get_agent_job", args.clone());
    }
    assert_eq!(
        result.get("state").and_then(Json::as_str),
        Some("cancelled"),
        "{result:?}"
    );
    assert!(!root.join("jobs/working-brief").exists());
}

#[test]
fn real_mcp_async_worker_uses_same_automatic_summary_boundary() {
    let root = scratch();
    let mut child = configured(&root, "normal")
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let input = child.stdin.take().unwrap();
    let output = BufReader::new(child.stdout.take().unwrap());
    let mut mcp = Mcp {
        child,
        input,
        output,
    };
    for (index, job) in [
        "24b755ef-4645-4d54-bfe5-ff60f209da70",
        "64e4150e-0fb0-4f54-aa30-604d9300dc12",
    ]
    .iter()
    .enumerate()
    {
        let worker = root.join("worker");
        let args = Json::Object(
            [
                ("job_id", *job),
                ("harness", "grok"),
                ("job_mode", "probe"),
                ("handoff_mode", "none"),
                ("conversation_mode", "isolated"),
                ("resume_id", SESSION),
                ("cwd", worker.to_str().unwrap()),
                ("prompt", "Current synthetic async task only."),
            ]
            .into_iter()
            .map(|(key, value)| (key.into(), Json::Str(value.into())))
            .collect(),
        );
        let mut result = mcp.tool(index * 100 + 1, "start_agent_job", args);
        for count in 0..50 {
            if result.get("state").and_then(Json::as_str) == Some("succeeded") {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            let args = Json::Object(
                [("job_id".into(), Json::Str(job.to_string()))]
                    .into_iter()
                    .collect(),
            );
            result = mcp.tool(index * 100 + count + 2, "get_agent_job", args);
        }
        assert_eq!(
            result.get("state").and_then(Json::as_str),
            Some("succeeded"),
            "{result:?}"
        );
        assert!(head(&root).contains("updated"));
    }
    assert!(fs::read_to_string(root.join("prompt.txt"))
        .unwrap()
        .contains("synthetic preserved decision"));
}
