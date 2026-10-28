import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time
from urllib.parse import quote
import uuid
from relocate_grok_sessions import read_security


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_case(root, binary, variant):
    root.mkdir()
    workspace = root / "physical"
    workspace.mkdir()
    alias = root / "legacy"
    subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
                    f"New-Item -ItemType Junction -Path '{alias}' -Target '{workspace}' | Out-Null"],
                   check=True, capture_output=True)
    home = root / "home"
    home.mkdir()
    legacy = "\\\\?\\" + str(alias)
    physical = "\\\\?\\" + str(workspace)
    session_id = str(uuid.uuid4())
    storage_cwd = physical if variant in ("namespace-and-summary", "namespace-only") else legacy
    summary_cwd = legacy if variant in ("baseline", "namespace-only") else physical
    encoded = quote(storage_cwd, safe="-_.~")
    if len(encoded) > 255:
        raise ValueError("fixture cwd exceeds native plain encoding limit")
    session = home / "sessions" / encoded / session_id
    session.mkdir(parents=True)
    summary = {"info": {"id": session_id, "cwd": summary_cwd},
               "session_summary": "Synthetic relocation probe", "created_at": "2026-10-23T00:00:00Z",
               "updated_at": "2026-10-23T00:00:00Z", "num_messages": 2,
               "num_chat_messages": 2, "current_model_id": "grok-4.7", "next_trace_turn": 0,
               "chat_format_version": 1, "session_kind": "headless"}
    (session / "summary.json").write_text(json.dumps(summary), encoding="utf-8")
    records = [{"type": "system", "content": "Synthetic persistence fixture. No tool or model work."},
               {"type": "user", "content": [{"type": "text", "text": "SYNTHETIC_RELOCATION_HISTORY"}]}]
    chat = session / "chat_history.jsonl"
    chat.write_text("".join(json.dumps(row) + "\n" for row in records), encoding="utf-8")
    (session / "updates.jsonl").write_text("", encoding="utf-8")
    before = sha256(chat)
    if variant != "baseline":
        if not alias.is_junction() or not os.path.samefile(alias, workspace):
            raise ValueError("synthetic junction identity changed")
        alias.rmdir()
    profile = root / "offline-profile.md"
    profile.write_text("---\nname: offline-session-path-probe\ndescription: Synthetic offline persistence probe\npromptMode: extend\ntools: []\n"
                       "agentsMd: false\ndiscoverSkills: false\ninheritSkills: false\n"
                       "injectDefaultTools: false\nmcpInheritance: none\nmodel: inherit\n---\n"
                       "Synthetic persistence test only. No prompt will be submitted.\n", encoding="utf-8")
    environment = {key: value for key, value in os.environ.items()
                   if key.upper() in {"SYSTEMROOT", "WINDIR", "PATH", "PATHEXT", "COMSPEC", "TEMP", "TMP"}}
    environment.update({"GROK_HOME": str(home), "HOME": str(home), "USERPROFILE": str(home),
                        "APPDATA": str(home / "roaming"), "LOCALAPPDATA": str(home / "local"),
                        "GROK_MEMORY": "0", "GROK_WORKFLOWS": "0", "GROK_ACTIVE_AGENT_MESSAGES": "0",
                        "XAI_API_KEY": "synthetic-offline-fixture-not-a-credential",
                        "HTTP_PROXY": "http://127.0.0.1:9", "HTTPS_PROXY": "http://127.0.0.1:9",
                        "ALL_PROXY": "http://127.0.0.1:9", "NO_PROXY": "127.0.0.1,localhost"})
    result = {"variant": variant, "session_id": session_id, "storage_cwd": storage_cwd,
              "summary_cwd": summary_cwd, "requested_cwd": legacy if variant == "baseline" else physical,
              "legacy_exists": alias.exists(), "binary_sha256": sha256(binary), "messages": []}
    result["summary_security_before"] = read_security(session / "summary.json")
    command = [str(binary), "--no-auto-update", "agent", "--no-leader", "--model", "grok-4.7",
               "--reasoning-effort", "xhigh", "--agent-profile", str(profile),
               "--cli-chat-proxy-base-url", "http://127.0.0.1:9", "--xai-api-base-url", "http://127.0.0.1:9",
               "--debug-file", str(root / "native-debug.log"), "stdio"]
    messages = queue.Queue()
    with (root / "stderr.log").open("wb") as errors:
        child = subprocess.Popen(command, cwd=workspace, env=environment, stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=errors, creationflags=subprocess.CREATE_NO_WINDOW)

        def read_output():
            for line in child.stdout:
                messages.put(line)
            messages.put(None)

        reader = threading.Thread(target=read_output, daemon=True)
        reader.start()

        def request(request_id, method, params):
            child.stdin.write((json.dumps({"jsonrpc": "2.0", "id": request_id,
                                          "method": method, "params": params}) + "\n").encode())
            child.stdin.flush()
            deadline = time.monotonic() + 40
            while time.monotonic() < deadline:
                try:
                    line = messages.get(timeout=max(0.1, deadline - time.monotonic()))
                except queue.Empty:
                    break
                if line is None:
                    raise RuntimeError("native process ended before response")
                message = json.loads(line)
                result["messages"].append(message)
                if message.get("id") == request_id and "method" not in message:
                    return message
                if "id" in message and "method" in message:
                    child.stdin.write((json.dumps({"jsonrpc": "2.0", "id": message["id"],
                                                  "error": {"code": -32601, "message": "No client capabilities"}}) + "\n").encode())
                    child.stdin.flush()
            raise TimeoutError(method)

        try:
            result["initialize"] = request(1, "initialize", {"protocolVersion": 1, "clientCapabilities": {}})
            result["load"] = request(2, "session/load", {"sessionId": session_id,
                                                        "cwd": result["requested_cwd"], "mcpServers": []})
        except (OSError, ValueError, RuntimeError, TimeoutError) as error:
            result["driver_error"] = str(error)
        finally:
            child.stdin.close()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.terminate()
                child.wait(timeout=5)
            reader.join(timeout=2)
            result["exit_code"] = child.returncode
    result["chat_before_sha256"] = before
    result["summary_security_after"] = read_security(session / "summary.json")
    result["chat_after_sha256"] = sha256(chat)
    result["uuid_directories"] = [str(path) for path in (home / "sessions").glob(f"*/{session_id}")]
    (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    return {"variant": variant, "load_error": result.get("load", {}).get("error"),
            "load_detail": result.get("load", {}).get("result", {}).get("_meta", {}).get("x.ai/sessionDetail"),
            "driver_error": result.get("driver_error"), "exit_code": result["exit_code"]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=Path(r"D:\grok\bin\grok.exe"))
    parser.add_argument("--variant", choices=["baseline", "summary-only", "namespace-and-summary", "namespace-only"], required=True)
    args = parser.parse_args()
    print(json.dumps(run_case(args.root.absolute(), args.binary, args.variant), indent=2))
