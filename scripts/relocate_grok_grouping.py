import argparse
from contextlib import ExitStack
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import uuid
from urllib.parse import quote

import relocate_grok_sessions as safe
from audit_local_paths import normalized, validate_mappings


KIND = "native-grok-git-root-only-v1"


def replace_root(raw, old_root, new_root):
    original = safe.decode(raw)
    if old_root == new_root or original.get("git_root_dir") != old_root:
        raise ValueError("root drift or unchanged root")
    before = json.dumps(old_root, ensure_ascii=False).encode()
    after = json.dumps(new_root, ensure_ascii=False).encode()
    if raw.count(before) != 1:
        raise ValueError("root JSON scalar is not unique or uses unsupported escaping")
    changed = raw.replace(before, after, 1)
    expected = copy.deepcopy(original)
    expected["git_root_dir"] = new_root
    if safe.decode(changed) != expected:
        raise ValueError("unrelated JSON change")
    return changed


def git_read(cwd, *arguments):
    return subprocess.run(["git", "--no-optional-locks", "-C", cwd, *arguments],
                          capture_output=True, check=True, timeout=30).stdout


def root_binding(cwd, old_root, alias, target):
    prefix = alias.as_posix() + "/"
    if "\\" in old_root or not old_root.startswith(prefix + "Parley/") or not old_root.endswith("/"):
        raise ValueError("unproved root spelling class")
    suffix = old_root[len(prefix):]
    if any(part in ("", ".", "..") for part in suffix[:-1].split("/")):
        raise ValueError("nonliteral root suffix")
    new_root = target.as_posix() + "/" + suffix
    safe.physical_cwd(old_root.rstrip("/"), alias, target)
    safe.require_physical(cwd)
    safe.require_physical(new_root)
    if not os.path.samefile(old_root, new_root):
        raise ValueError("repository identity mismatch")
    status = git_read(cwd, "status", "--porcelain=v1", "-z")
    detected = git_read(cwd, "rev-parse", "--show-toplevel").decode("utf-8").strip()
    if detected.rstrip("/") + "/" != new_root or not os.path.samefile(detected, new_root):
        raise ValueError("native spelling or Git root identity mismatch")
    if git_read(cwd, "status", "--porcelain=v1", "-z") != status:
        raise ValueError("Git status changed during inspection")
    return {"new_root": new_root, "root_identity": safe.identity(new_root),
            "git_status_sha256": hashlib.sha256(status).hexdigest()}


def permission_state(session, state):
    paths = [".", *state["directories"], *sorted(state["files"])]
    return {name: safe.read_security(session / name)["signature"] for name in paths}


def plan_grouping(root, alias, target, selected=None):
    root, alias, target = (Path(value).absolute() for value in (root, alias, target))
    safe.require_physical(root)
    mapping = validate_mappings({str(alias): str(target)})
    seen, rows, bindings = set(), [], {}
    for bucket in sorted(root.iterdir()):
        safe.require_physical(bucket)
        if not bucket.is_dir():
            continue
        for session in sorted(bucket.iterdir()):
            safe.require_physical(session)
            if not session.is_dir():
                continue
            try:
                session_id = str(uuid.UUID(session.name))
            except ValueError:
                raise ValueError("invalid native session directory") from None
            if session_id != session.name or session_id in seen:
                raise ValueError("duplicate or noncanonical UUID")
            seen.add(session_id)
            summary_path = session / "summary.json"
            if not summary_path.exists():
                continue
            safe.require_physical(summary_path)
            raw = summary_path.read_bytes()
            summary = safe.decode(raw)
            if summary["info"]["id"] != session_id:
                raise ValueError("summary UUID mismatch")
            old_root = summary.get("git_root_dir")
            if not isinstance(old_root, str) or not normalized(old_root).startswith(normalized(str(alias)) + "\\"):
                continue
            if selected is not None and session_id not in selected:
                continue
            if session_id in safe.DENIED_IDS:
                raise ValueError("protected current session")
            cwd = summary["info"]["cwd"]
            safe.require_physical(cwd)
            if bucket.name != quote(cwd, safe="-_.~"):
                raise ValueError("native cwd encoding mismatch")
            key = (cwd, old_root)
            if key not in bindings:
                bindings[key] = root_binding(cwd, old_root, alias, target)
            binding = bindings[key]
            changed = replace_root(raw, old_root, binding["new_root"])
            state = safe.tree_state(session)
            rows.append({"session_id": session_id, "source": str(session), "cwd": cwd,
                         "old_root": old_root, **binding,
                         "directory_identity": safe.identity(session), "state": state,
                         "permissions": permission_state(session, state),
                         "summary_security": safe.read_security(summary_path),
                         "summary_mtime_ns": summary_path.stat().st_mtime_ns,
                         "summary_after_sha256": hashlib.sha256(changed).hexdigest()})
    if selected is not None and {row["session_id"] for row in rows} != set(selected):
        raise ValueError("selected sessions missing or already physical")
    return {"kind": KIND, "root": str(root), "alias": str(alias), "target": str(target),
            "mapping": mapping, "sessions": rows}


def expected_state(row, original):
    changed = replace_root(original, row["old_root"], row["new_root"])
    if hashlib.sha256(changed).hexdigest() != row["summary_after_sha256"]:
        raise ValueError("approved replacement hash mismatch")
    expected = copy.deepcopy(row["state"])
    expected["files"]["summary.json"] = {"sha256": row["summary_after_sha256"], "size": len(changed)}
    return expected


def verify_row(row, transaction, migrated=True):
    session = Path(row["source"])
    backup = transaction / row["session_id"]
    if safe.tree_state(backup) != row["state"]:
        raise ValueError("backup hash mismatch")
    original = (backup / "summary.json").read_bytes()
    expected = expected_state(row, original) if migrated else row["state"]
    if safe.identity(session) != row["directory_identity"] or safe.tree_state(session) != expected:
        raise ValueError("live identity or bytes mismatch")
    if permission_state(session, expected) != row["permissions"]:
        raise ValueError("live permissions mismatch")
    if (session / "summary.json").stat().st_mtime_ns != row["summary_mtime_ns"]:
        raise ValueError("summary modified timestamp mismatch")


def rollback_row(row, transaction):
    safe.require_quiescent()
    session, backup = Path(row["source"]), transaction / row["session_id"]
    if safe.identity(session) != row["directory_identity"] or safe.tree_state(backup) != row["state"]:
        raise ValueError("rollback identity or backup mismatch")
    original = (backup / "summary.json").read_bytes()
    with ExitStack() as held:
        for filename in row["state"]["files"]:
            held.enter_context(safe.deny_writes(session / filename))
        if safe.tree_state(session) not in (row["state"], expected_state(row, original)):
            raise ValueError("rollback refuses divergent bytes")
        permissions = permission_state(session, row["state"])
        if any(permissions[name] != signature for name, signature in row["permissions"].items() if name != "summary.json"):
            raise ValueError("rollback refuses divergent non-summary permissions")
        summary = session / "summary.json"
        if summary.read_bytes() != original:
            safe.replace_bytes(summary, original, transaction)
        held.enter_context(safe.deny_writes(summary))
        safe.restore_dacl(summary, row["summary_security"])
        os.utime(summary, ns=(summary.stat().st_atime_ns, row["summary_mtime_ns"]))
        verify_row(row, transaction, migrated=False)
        safe.require_quiescent()


def apply_plan(plan, transaction):
    if os.name != "nt" or plan.get("kind") != KIND:
        raise ValueError("Windows grouping manifest required")
    safe.require_quiescent()
    root, transaction = Path(plan["root"]), Path(transaction).absolute()
    safe.require_physical(transaction.parent)
    if transaction.is_relative_to(root) or root.is_relative_to(transaction):
        raise ValueError("backup must be outside active sessions tree")
    if transaction.parent.stat().st_dev != root.stat().st_dev:
        raise ValueError("backup staging must be on the same volume")
    selected = {row["session_id"] for row in plan["sessions"]}
    if plan_grouping(root, plan["alias"], plan["target"], selected) != plan:
        raise ValueError("manifest drift")
    transaction.mkdir()
    safe.write_json(transaction / "plan.json", plan)
    with (transaction / "journal.jsonl").open("x", encoding="utf-8") as journal:
        def record(state, session_id=None, **details):
            journal.write(json.dumps({"state": state, "session_id": session_id, **details}) + "\n")
            journal.flush()
            os.fsync(journal.fileno())

        record("planned")
        for row in plan["sessions"]:
            session, session_id = Path(row["source"]), row["session_id"]
            with ExitStack() as held:
                safe.require_quiescent()
                for filename in row["state"]["files"]:
                    held.enter_context(safe.deny_writes(session / filename))
                if safe.tree_state(session) != row["state"] or safe.identity(session) != row["directory_identity"]:
                    raise ValueError("source changed before backup")
                if permission_state(session, row["state"]) != row["permissions"]:
                    raise ValueError("source permissions changed before backup")
                backup = transaction / session_id
                shutil.copytree(session, backup, copy_function=safe.durable_copy)
                if safe.tree_state(backup) != row["state"]:
                    raise ValueError("backup hash mismatch")
                record("backed-up", session_id)
                safe.require_quiescent()
                try:
                    if root_binding(row["cwd"], row["old_root"], Path(plan["alias"]), Path(plan["target"])) != {
                            key: row[key] for key in ("new_root", "root_identity", "git_status_sha256")}:
                        raise ValueError("root binding changed before write")
                    if safe.tree_state(session) != row["state"]:
                        raise ValueError("source drift before write")
                    summary = session / "summary.json"
                    timestamps = summary.stat()
                    changed = replace_root((backup / "summary.json").read_bytes(), row["old_root"], row["new_root"])
                    safe.replace_bytes(summary, changed, transaction)
                    held.enter_context(safe.deny_writes(summary))
                    os.utime(summary, ns=(timestamps.st_atime_ns, row["summary_mtime_ns"]))
                    record("metadata-replaced", session_id)
                    verify_row(row, transaction)
                    safe.require_quiescent()
                    record("verified", session_id, summary_sha256=safe.digest(summary))
                except Exception as error:
                    record("stopped", session_id, error=str(error))
                    held.close()
                    try:
                        rollback_row(row, transaction)
                        record("rolled-back", session_id)
                    except Exception as rollback_error:
                        record("rollback-refused", session_id, error=str(rollback_error))
                    raise
        record("complete", count=len(plan["sessions"]))
    return {"migrated": len(plan["sessions"]), "transaction": str(transaction)}


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    planning = commands.add_parser("plan")
    for name in ("root", "alias", "target", "output"):
        planning.add_argument("--" + name, type=Path, required=True)
    planning.add_argument("--session", action="append")
    applying = commands.add_parser("apply")
    applying.add_argument("--plan", type=Path, required=True)
    applying.add_argument("--sha256", required=True)
    applying.add_argument("--transaction", type=Path, required=True)
    rolling = commands.add_parser("rollback")
    rolling.add_argument("--transaction", type=Path, required=True)
    rolling.add_argument("--session", required=True)
    rolling.add_argument("--sha256", required=True)
    args = parser.parse_args()
    if args.command == "plan":
        proposed = plan_grouping(args.root, args.alias, args.target, args.session)
        safe.write_json(args.output, proposed)
        print(json.dumps({"sessions": len(proposed["sessions"]), "plan_sha256": safe.digest(args.output)}))
    else:
        manifest = args.plan if args.command == "apply" else args.transaction / "plan.json"
        if safe.digest(manifest) != args.sha256:
            raise ValueError("plan hash mismatch")
        proposed = safe.decode(manifest.read_bytes())
        if proposed.get("kind") != KIND:
            raise ValueError("grouping manifest required")
        if args.command == "apply":
            print(json.dumps(apply_plan(proposed, args.transaction)))
        else:
            row = next(item for item in proposed["sessions"] if item["session_id"] == args.session)
            rollback_row(row, args.transaction)
            safe.write_json(args.transaction / ("rollback-" + args.session + ".json"), {"state": "rolled-back"})


if __name__ == "__main__":
    main()
