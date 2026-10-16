#[cfg(windows)]
#[test]
fn installed_hook_command_executes_literal_path_and_preserves_exit_code() {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let root = std::env::temp_dir().join(format!("parley-hook-shell-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let hook = root.join("hook with ' quote & `literal.cmd");
    std::fs::write(
        &hook,
        "@echo off\r\necho {\"decision\":\"block\",\"reason\":\"synthetic\"}\r\nexit /b 2\r\n",
    )
    .unwrap();
    let document = parley_health::integration::hook_document(&hook, &hook, &hook);
    let command = document["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(!command.contains('$'));
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .stdin(Stdio::null())
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(wire["decision"], "block");
    assert!(output.stderr.is_empty());
}

#[cfg(windows)]
#[test]
fn hook_command_forwards_exact_utf8_stdin_and_fails_closed_when_missing() {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let root = std::env::temp_dir().join(format!("parley-hook-input-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let hook = root.join("input hook.cmd");
    let executable = std::env::current_exe().unwrap();
    std::fs::write(&hook, format!("@echo off\r\n\"{}\" --exact hook_stdin_child --ignored --nocapture\r\nexit /b %ERRORLEVEL%\r\n", executable.display())).unwrap();
    let document = parley_health::integration::hook_document(&hook, &hook, &hook);
    let command = document["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("SYNTHETIC_λ_🧪\r\nsecond line\n".as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let missing = root.join("missing.exe");
    let document = parley_health::integration::hook_document(&missing, &missing, &missing);
    let command = document["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .stdin(Stdio::null())
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[cfg(windows)]
#[test]
#[ignore = "subprocess fixture for exact native stdin forwarding"]
fn hook_stdin_child() {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, "SYNTHETIC_λ_🧪\r\nsecond line\n".as_bytes());
    std::process::exit(2);
}
