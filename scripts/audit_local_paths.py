import argparse
import copy
import hashlib
import json
import ntpath
import os
from pathlib import Path
import subprocess
import tomllib


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def normalized(value):
    value = value.replace("/", "\\")
    if value.startswith("\\\\?\\"):
        value = value[4:]
    return ntpath.normpath(value).casefold()


def alias_match(value, mappings):
    candidate = normalized(value)
    for alias, target in mappings.items():
        prefix = normalized(alias)
        if candidate == prefix or candidate.startswith(prefix + "\\"):
            return alias, target
    return None


def path_hits(value, mappings, pointer=""):
    hits = []
    if isinstance(value, dict):
        for key, child in value.items():
            hits.extend(path_hits(child, mappings, f"{pointer}/{key}"))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            hits.extend(path_hits(child, mappings, f"{pointer}/{index}"))
    elif isinstance(value, str) and alias_match(value, mappings):
        hits.append({"pointer": pointer, "path": value})
    return hits


def require_physical(path):
    path = Path(path).absolute()
    for ancestor in (path, *path.parents):
        if ancestor.is_symlink() or ancestor.is_junction():
            raise ValueError(f"redirected physical path: {ancestor}")
    if not path.exists():
        raise ValueError(f"missing physical path: {path}")


def identity(path):
    metadata = Path(path).stat()
    return {"device": metadata.st_dev, "file_index": metadata.st_ino}


def validate_mappings(mappings):
    evidence = []
    for alias, target in mappings.items():
        require_physical(target)
        alias_path = Path(alias)
        if not alias_path.is_junction() and not alias_path.is_symlink():
            raise ValueError(f"expected a junction or symlink: {alias}")
        if not os.path.samefile(alias, target):
            raise ValueError(f"alias target identity mismatch: {alias}")
        evidence.append({"alias": alias, "target": target, "identity": identity(target)})
    return evidence


def remap_existing_file(value, mappings):
    match = alias_match(value, mappings)
    if match is None:
        return value, None
    alias, target = match
    source = value.replace("/", "\\")
    if source.startswith("\\\\?\\"):
        source = source[4:]
    remainder = source[len(alias.replace("/", "\\")):].lstrip("\\")
    candidate = Path(target).joinpath(*remainder.split("\\"))
    require_physical(candidate)
    if not candidate.is_file() or not os.path.samefile(value, candidate):
        raise ValueError("selected log identity mismatch")
    with open(value, "rb") as before, candidate.open("rb") as after:
        if not os.path.samestat(os.fstat(before.fileno()), os.fstat(after.fileno())):
            raise ValueError("selected log changed during verification")
        length = os.fstat(before.fileno()).st_size
        remaining = length
        hasher = hashlib.sha256()
        while remaining:
            amount = min(remaining, 1024 * 1024)
            original = before.read(amount)
            current = after.read(amount)
            if len(original) != amount or original != current:
                raise ValueError("selected log prefix changed during verification")
            hasher.update(original)
            remaining -= amount
    return str(candidate), {
        "before": value, "after": str(candidate), "identity": identity(candidate),
        "verified_prefix_bytes": length, "verified_prefix_sha256": hasher.hexdigest(),
    }


def viewer_plan(raw, mappings):
    original = json.loads(raw)
    proposed = copy.deepcopy(original)
    changes = []
    selected = proposed.get("selectedLog")
    if isinstance(selected, str):
        proposed["selectedLog"], proof = remap_existing_file(selected, mappings)
        if proof:
            changes.append({"pointer": "/selectedLog", **proof})
    selected_logs = proposed.get("selectedLogs", [])
    if not isinstance(selected_logs, list) or any(not isinstance(path, str) for path in selected_logs):
        raise ValueError("invalid selectedLogs")
    for index, value in enumerate(selected_logs):
        selected_logs[index], proof = remap_existing_file(value, mappings)
        if proof:
            changes.append({"pointer": f"/selectedLogs/{index}", **proof})
    for key in original.keys() - {"selectedLog", "selectedLogs"}:
        if original[key] != proposed[key]:
            raise ValueError("unrelated settings change")
    encoded = (json.dumps(proposed, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    return {
        "before_sha256": digest(raw), "after_sha256": digest(encoded),
        "changes": changes, "proposed": proposed,
    }, encoded


def load_document(path):
    raw = Path(path).read_bytes()
    if Path(path).suffix == ".toml":
        return tomllib.loads(raw.decode("utf-8-sig"))
    return json.loads(raw)


def audit(layout):
    mappings = layout["aliases"]
    result = {"schema_version": 1, "mappings": validate_mappings(mappings),
              "live_references": [], "historical_session_dependencies": [], "errors": []}
    for filename in layout["active_documents"]:
        try:
            document = load_document(filename)
            if Path(filename).suffix == ".toml":
                document = {"mcp_servers": {name: config for name, config in document.get("mcp_servers", {}).items()
                                            if "parley" in name}}
            result["live_references"].extend({"file": filename, **hit} for hit in path_hits(document, mappings))
        except (OSError, ValueError) as error:
            result["errors"].append({"file": filename, "error": type(error).__name__})
    repository = Path(layout["repository"])
    listing = subprocess.run(["git", "-C", str(repository), "worktree", "list", "--porcelain"],
                             check=True, text=True, capture_output=True).stdout
    trees = [Path(line[9:]) for line in listing.splitlines() if line.startswith("worktree ")]
    result["git_worktrees_checked"] = len(trees)
    pointers = list((repository / ".git" / "worktrees").glob("*/gitdir"))
    pointers.extend(tree / ".git" for tree in trees if (tree / ".git").is_file())
    result["git_pointers_checked"] = len(pointers)
    for pointer in pointers:
        value = pointer.read_text(encoding="utf-8").strip().removeprefix("gitdir: ")
        if alias_match(value, mappings):
            result["live_references"].append({"file": str(pointer), "path": value})
    summaries = list(Path(layout["grok_sessions"]).glob("*/*/summary.json"))
    result["native_summaries_checked"] = len(summaries)
    for summary in summaries:
        try:
            info = load_document(summary)["info"]
            cwd = info["cwd"]
            if alias_match(cwd, mappings):
                result["historical_session_dependencies"].append({
                    "session_id": info["id"], "cwd": cwd, "summary": str(summary),
                    "summary_sha256": digest(summary.read_bytes()),
                })
        except (OSError, ValueError, KeyError, TypeError):
            result["errors"].append({"file": str(summary), "error": "unreadable_session_metadata"})
    result["audited_live_paths_clear"] = not result["live_references"] and not result["errors"]
    result["junction_retirement_ready"] = False
    result["retirement_note"] = "No unlink authorization or complete historical-session migration; keep aliases."
    return result


def main():
    parser = argparse.ArgumentParser(description="Read-only legacy Parley path audit and guarded settings proposal")
    parser.add_argument("layout", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--viewer-proposal", type=Path)
    args = parser.parse_args()
    layout = load_document(args.layout)
    result = audit(layout)
    if args.viewer_proposal:
        raw = Path(layout["viewer_settings"]).read_bytes()
        plan, proposed = viewer_plan(raw, layout["aliases"])
        result["viewer_plan"] = plan
        with args.viewer_proposal.open("xb") as destination:
            destination.write(proposed)
    with args.output.open("x", encoding="utf-8") as destination:
        json.dump(result, destination, indent=2)
        destination.write("\n")
    print(json.dumps({"live_references": len(result["live_references"]),
                      "historical_session_dependencies": len(result["historical_session_dependencies"]),
                      "errors": len(result["errors"]), "report": str(args.output)}))


if __name__ == "__main__":
    main()
