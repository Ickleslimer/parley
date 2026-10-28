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
from audit_local_paths import require_physical


def run_case(root, remove_alias, existing_worktree=None):
    root.mkdir()
    workspace = existing_worktree if existing_worktree is not None else root / "repo"
    if existing_worktree is None:
        workspace.mkdir()
        subprocess.run(["git", "init", str(workspace)], check=True, capture_output=True)
        subprocess.run(["git", "-C", str(workspace), "-c", "user.name=Synthetic fixture",
                        "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false",
                        "commit", "--allow-empty", "-m", "Synthetic grouping fixture"], check=True, capture_output=True)
    else:
        require_physical(workspace)
    status_command = ["git", "--no-optional-locks", "-C", str(workspace), "status", "--porcelain", "--untracked-files=all"]
    status_before = subprocess.run(status_command, check=True, capture_output=True).stdout
    current_head = subprocess.run(["git", "-C", str(workspace), "rev-parse", "HEAD"],
                                  check=True, capture_output=True, text=True).stdout.strip()
    alias = root / "legacy"
    subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
                    f"New-Item -ItemType Junction -Path '{alias}' -Target '{workspace}' | Out-Null"],
                   check=True, capture_output=True)
    if remove_alias:
        if not alias.is_junction() or not os.path.samefile(alias, workspace) or alias.parent != root:
            raise ValueError("synthetic link identity mismatch")
        alias.rmdir()
    home = root / "home"
    home.mkdir()
    physical_cwd = "\\\\?\\" + str(workspace)
    roots = {"physical": workspace.as_posix() + "/", "missing": (root / "never-existed").as_posix() + "/",
             "alias": alias.as_posix() + "/"}
    encoded = quote(physical_cwd, safe="-_.~")
    if len(encoded) > 255:
        raise ValueError("fixture namespace too long")
    fixtures = {}
    for label, git_root in roots.items():
        session_id = str(uuid.uuid4())
        directory = home / "sessions" / encoded / session_id
        directory.mkdir(parents=True)
        summary = {"info": {"id": session_id, "cwd": physical_cwd}, "session_summary": "Synthetic " + label,
                   "created_at": "2026-10-23T00:00:00Z", "updated_at": "2026-10-23T00:00:00Z",
                   "num_messages": 2, "num_chat_messages": 2, "current_model_id": "grok-4.7",
                   "next_trace_turn": 0, "chat_format_version": 1, "session_kind": "headless",
                   "git_root_dir": git_root, "head_commit": "0" * 40, "head_branch": "fixture-old-branch"}
        (directory / "summary.json").write_text(json.dumps(summary), encoding="utf-8")
        marker = "SYNTHETIC_GROUPING_HISTORY_" + label
        chat = [{"type": "system", "content": "Synthetic grouping probe; no model prompt."},
                {"type": "user", "content": [{"type": "text", "text": marker}]}]
        (directory / "chat_history.jsonl").write_text("".join(json.dumps(record) + "\n" for record in chat), encoding="utf-8")
        (directory / "updates.jsonl").write_text("", encoding="utf-8")
        fixtures[label] = {"id": session_id, "directory": str(directory), "git_root": git_root, "marker": marker}
    profile = root / "offline-profile.md"
    profile.write_text("---\nname: offline-grouping-probe\ndescription: Synthetic no-prompt fixture\npromptMode: extend\ntools: []\n"
                       "agentsMd: false\ndiscoverSkills: false\ninheritSkills: false\ninjectDefaultTools: false\n"
                       "mcpInheritance: none\nmodel: inherit\n---\nNo model prompt or tools.\n", encoding="utf-8")
    environment = {key: value for key, value in os.environ.items()
                   if key.upper() in {"SYSTEMROOT", "WINDIR", "PATH", "PATHEXT", "COMSPEC", "TEMP", "TMP"}}
    environment.update({"GROK_HOME": str(home), "HOME": str(home), "USERPROFILE": str(home),
                        "APPDATA": str(home / "roaming"), "LOCALAPPDATA": str(home / "local"),
                        "GROK_MEMORY": "0", "GROK_WORKFLOWS": "0", "GROK_ACTIVE_AGENT_MESSAGES": "0",
                        "XAI_API_KEY": "synthetic-offline-fixture-not-a-credential",
                        "HTTP_PROXY": "http://127.0.0.1:9", "HTTPS_PROXY": "http://127.0.0.1:9",
                        "ALL_PROXY": "http://127.0.0.1:9", "NO_PROXY": "127.0.0.1,localhost"})
    binary = Path(r"D:\grok\bin\grok.exe")
    result = {"binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "alias_exists": alias.exists(),
              "physical_cwd": physical_cwd, "current_head": current_head, "fixtures": fixtures, "messages": []}
    command = [str(binary), "--no-auto-update", "agent", "--no-leader", "--model", "grok-4.7",
               "--reasoning-effort", "xhigh", "--agent-profile", str(profile),
               "--cli-chat-proxy-base-url", "http://127.0.0.1:9", "--xai-api-base-url", "http://127.0.0.1:9",
               "--debug-file", str(root / "native-debug.log"), "stdio"]
    inbox = queue.Queue()
    with (root / "stderr.log").open("wb") as errors:
        child = subprocess.Popen(command, cwd=workspace, env=environment, stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=errors, creationflags=subprocess.CREATE_NO_WINDOW)

        def read_output():
            for line in child.stdout:
                inbox.put(line)
            inbox.put(None)

        reader = threading.Thread(target=read_output, daemon=True)
        reader.start()

        def request(request_id, method, params):
            child.stdin.write((json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}) + "\n").encode())
            child.stdin.flush()
            deadline = time.monotonic() + 40
            while time.monotonic() < deadline:
                try:
                    line = inbox.get(timeout=max(0.1, deadline - time.monotonic()))
                except queue.Empty:
                    break
                if line is None:
                    raise RuntimeError("native process ended")
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
            result["list"] = request(2, "session/list", {"cwd": physical_cwd})
            result["grouped_list"] = request(3, "_x.ai/session/list", {"cwd": physical_cwd, "headless": "include", "limit": 30})
            result["physical_filter"] = request(4, "_x.ai/session/list", {"cwd": physical_cwd, "headless": "include",
                                                "_meta": {"x.ai/facetFilters": {"gitRoot": [roots["physical"]]}}})
            result["missing_filter"] = request(5, "_x.ai/session/list", {"cwd": physical_cwd, "headless": "include",
                                               "_meta": {"x.ai/facetFilters": {"gitRoot": [roots["missing"]]}}})
            result["loads"] = {}
            for number, (label, fixture) in enumerate(fixtures.items(), 6):
                result["loads"][label] = request(number, "session/load", {"sessionId": fixture["id"], "cwd": physical_cwd, "mcpServers": []})
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
    result["history_markers_preserved"] = {label: fixture["marker"] in (Path(fixture["directory"]) / "chat_history.jsonl").read_text(encoding="utf-8")
                                            for label, fixture in fixtures.items()}
    result["existing_worktree_read_only"] = existing_worktree is not None
    result["git_status_unchanged"] = status_before == subprocess.run(status_command, check=True, capture_output=True).stdout
    result["git_head_unchanged"] = current_head == subprocess.run(["git", "-C", str(workspace), "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
    (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    return {"root": str(root), "alias_exists": result["alias_exists"], "driver_error": result.get("driver_error"),
            "load_errors": {label: reply.get("error") for label, reply in result.get("loads", {}).items()},
            "history": result["history_markers_preserved"]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    parser.add_argument("--remove-alias", action="store_true")
    parser.add_argument("--existing-worktree", type=Path)
    arguments = parser.parse_args()
    print(json.dumps(run_case(arguments.root.absolute(), arguments.remove_alias, arguments.existing_worktree), indent=2))
