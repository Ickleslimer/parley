import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tomllib


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--journal", type=Path, required=True)
    parser.add_argument("--cwd", required=True)
    parser.add_argument("--session", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    config = tomllib.loads(arguments.config.read_text(encoding="utf-8-sig"))
    profile = config["mcp_servers"]["parley-widget-dev"]
    environment = dict(os.environ)
    environment.update(profile.get("env", {}))
    before = digest(arguments.journal)
    command = [str(arguments.candidate), "ask", "-h", "grok", "-p",
               "Read-only shared context preflight.", "--context-from",
               "codex:" + arguments.session, "--context-mode", "auto",
               "--conversation-mode", "canonical", "--no-yolo", "--dry-run",
               "--cwd", arguments.cwd]
    result = subprocess.run(command, env=environment, capture_output=True,
                            text=True, encoding="utf-8", timeout=120)
    report = {"exitCode": result.returncode, "candidateSha256": digest(arguments.candidate),
              "journalBeforeSha256": before, "journalAfterSha256": digest(arguments.journal),
              "journalUnchanged": before == digest(arguments.journal),
              "configSha256": digest(arguments.config), "dryRun": True}
    if result.returncode == 0:
        report["plan"] = json.loads(result.stdout)
    else:
        report["error"] = result.stderr.strip()
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    with arguments.output.open("x", encoding="utf-8") as destination:
        json.dump(report, destination, indent=2)
        destination.write("\n")
    print(json.dumps(report))
    return result.returncode or (0 if report["journalUnchanged"] else 1)


if __name__ == "__main__":
    raise SystemExit(main())
