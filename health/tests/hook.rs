use parley_health::hook::{decide_pretool_use, handle_bytes, HookDecision};
use parley_health::schema::InboxKind;
use serde_json::{json, Value};

mod common;

fn pretool(command: &str, cwd: &str, workspace: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "cwd": cwd,
        "workspaceRoot": workspace,
        "tool_input": { "command": command }
    })
}

fn deny_reason(decision: HookDecision) -> Option<&'static str> {
    match decision {
        HookDecision::Silent => None,
        HookDecision::Deny { reason } => Some(reason),
    }
}

#[test]
fn pretool_use_is_deny_only_and_silent_for_exact_in_scope_query() {
    let query = r"C:\Parley\parley-health-query.exe";
    let scope = common::scope_for(r"C:\repo", query);

    assert_eq!(
        decide_pretool_use(&pretool(query, r"C:\repo", r"C:\repo"), &scope),
        HookDecision::Silent
    );
    assert_eq!(
        decide_pretool_use(&pretool(r"C:\Parley\PARLEY-HEALTH-QUERY.EXE", r"C:\REPO\src", r"C:\repo"), &scope),
        HookDecision::Silent
    );
    assert_eq!(
        decide_pretool_use(&pretool("git status", r"C:\repo", r"C:\repo"), &scope),
        HookDecision::Silent
    );

    let extras = [
        format!("{query} --help"),
        format!("{query} snapshot.json"),
        format!("cmd /c {query}"),
        format!("powershell -Command {query}"),
        format!("{query} && echo pwned"),
        format!("{query} | more"),
        format!("{query} > out.json"),
        format!("{query} 2> err.txt"),
        r"C:\Parley\parley-health-supervisor.exe".to_string(),
        r"C:\Parley\parley-health-hook.exe".to_string(),
        r"C:\other\parley-health-query.exe".to_string(),
    ];
    for command in extras {
        let decision = decide_pretool_use(&pretool(&command, r"C:\repo", r"C:\repo"), &scope);
        assert!(
            matches!(decision, HookDecision::Deny { .. }),
            "expected deny for {command}, got {decision:?}"
        );
        let json = serde_json::to_string(&json!({ "permissionDecision": "deny" })).unwrap();
        assert!(json.contains("deny"));
        assert!(!format!("{decision:?}").to_ascii_lowercase().contains("allow"));
    }
}

#[test]
fn out_of_scope_health_invocation_denies_unrelated_tools_noop() {
    let query = r"C:\Parley\parley-health-query.exe";
    let scope = common::scope_for(r"C:\repo", query);
    assert!(matches!(
        decide_pretool_use(&pretool(query, r"C:\repo-evil", r"C:\repo-evil"), &scope),
        HookDecision::Deny { .. }
    ));
    assert_eq!(
        decide_pretool_use(&pretool("cargo test", r"C:\repo-evil", r"C:\repo-evil"), &scope),
        HookDecision::Silent
    );
    assert!(deny_reason(decide_pretool_use(
        &pretool(query, r"C:\repo-evil", r"C:\repo"),
        &scope
    ))
    .is_some());
}

#[test]
fn never_serializes_allow() {
    let query = r"C:\Parley\parley-health-query.exe";
    let scope = common::scope_for(r"C:\repo", query);
    let home = common::TempHome::new("hook-stdout");
    common::write_scope(&home, &scope);
    let payload = serde_json::to_vec(&pretool(
        r"C:\Parley\parley-health-supervisor.exe",
        r"C:\repo",
        r"C:\repo",
    ))
    .unwrap();
    let output = handle_bytes(&payload, false, &home.paths).unwrap();
    let stdout = output.stdout.expect("deny stdout");
    assert!(stdout.contains("\"permissionDecision\":\"deny\""));
    assert!(!stdout.to_ascii_lowercase().contains("allow"));
}

#[test]
fn stop_failure_writes_sanitized_inbox_only_inside_cached_roots() {
    let home = common::TempHome::new("hook-stop");
    let query = home.paths.root.join("parley-health-query.exe");
    std::fs::write(&query, b"q").unwrap();
    let scope = common::scope_for(&home.paths.root.to_string_lossy(), &query.to_string_lossy());
    common::write_scope(&home, &scope);

    let payload = serde_json::to_vec(&json!({
        "hook_event_name": "StopFailure",
        "cwd": home.paths.root,
        "workspaceRoot": home.paths.root,
        "error": {
            "status": 429,
            "code": "subscription:free-usage-exhausted",
            "message": "raw provider text must not be copied"
        }
    }))
    .unwrap();
    let output = handle_bytes(&payload, false, &home.paths).unwrap();
    assert!(output.inbox_written);
    assert!(output.stdout.is_none());

    let mut supervisor = common::supervisor(&home);
    supervisor.tick(1).unwrap();
    let snapshot = std::fs::read_to_string(home.paths.snapshot()).unwrap();
    assert!(snapshot.contains("quota_exhausted"));
    assert!(!snapshot.contains("raw provider text"));
    assert!(!snapshot.contains("PARLEY_"));

    let outsider = common::TempHome::new("hook-stop-out");
    common::write_scope(&outsider, &scope);
    let payload = serde_json::to_vec(&json!({
        "hook_event_name": "StopFailure",
        "cwd": r"C:\elsewhere",
        "workspaceRoot": r"C:\elsewhere",
        "error": { "status": 429, "code": "subscription:free-usage-exhausted" }
    }))
    .unwrap();
    let output = handle_bytes(&payload, false, &outsider.paths).unwrap();
    assert!(!output.inbox_written);
}

#[test]
fn hook_binary_reads_stdin_and_denies_writer_image() {
    let home = common::TempHome::new("hook-bin");
    let query = r"C:\Parley\parley-health-query.exe";
    common::write_scope(&home, &common::scope_for(r"C:\repo", query));
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_parley-health-hook"))
        .env("PARLEY_HEALTH_HOME", &home.paths.root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        use std::io::Write;
        stdin
            .write_all(
                serde_json::to_vec(&pretool(
                    r"C:\Parley\parley-health-supervisor.exe",
                    r"C:\repo",
                    r"C:\repo",
                ))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"permissionDecision\":\"deny\""));
    assert!(!stdout.to_ascii_lowercase().contains("allow"));
    let _ = InboxKind::GrokStopFailure;
}
