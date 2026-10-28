import argparse
from contextlib import ExitStack, contextmanager
import copy
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import uuid
from urllib.parse import quote

from audit_local_paths import identity, normalized, require_physical, validate_mappings


DENIED_IDS = {"f09b6874-3b8e-875a-87ba-314c7400ae54"}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def security_api():
    api = ctypes.WinDLL("advapi32", use_last_error=True)
    api.GetFileSecurityW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, ctypes.c_void_p,
                                    wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
    api.SetFileSecurityW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, ctypes.c_void_p]
    api.GetSecurityDescriptorOwner.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p),
                                             ctypes.POINTER(wintypes.BOOL)]
    api.GetSecurityDescriptorGroup.argtypes = api.GetSecurityDescriptorOwner.argtypes
    api.GetLengthSid.argtypes = [ctypes.c_void_p]
    api.GetSecurityDescriptorDacl.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.BOOL),
                                            ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(wintypes.BOOL)]
    api.GetSecurityDescriptorControl.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.WORD),
                                               ctypes.POINTER(wintypes.DWORD)]
    return api


def security_signature(raw):
    api = security_api()
    buffer = ctypes.create_string_buffer(raw)
    pointer, defaulted = ctypes.c_void_p(), wintypes.BOOL()
    result = {}
    for name, function in (("owner", api.GetSecurityDescriptorOwner), ("group", api.GetSecurityDescriptorGroup)):
        if not function(buffer, ctypes.byref(pointer), ctypes.byref(defaulted)) or not pointer.value:
            raise ctypes.WinError(ctypes.get_last_error())
        result[name] = ctypes.string_at(pointer, api.GetLengthSid(pointer)).hex()
    present = wintypes.BOOL()
    if not api.GetSecurityDescriptorDacl(buffer, ctypes.byref(present), ctypes.byref(pointer), ctypes.byref(defaulted)):
        raise ctypes.WinError(ctypes.get_last_error())
    result["dacl_present"] = bool(present.value)
    result["dacl"] = None
    if pointer.value:
        length = int.from_bytes(ctypes.string_at(pointer.value + 2, 2), "little")
        result["dacl"] = ctypes.string_at(pointer, length).hex()
    control, revision = wintypes.WORD(), wintypes.DWORD()
    if not api.GetSecurityDescriptorControl(buffer, ctypes.byref(control), ctypes.byref(revision)):
        raise ctypes.WinError(ctypes.get_last_error())
    result["dacl_protected"] = bool(control.value & 0x1000)
    return result


def read_security(path):
    api = security_api()
    needed = wintypes.DWORD()
    api.GetFileSecurityW(str(path), 7, None, 0, ctypes.byref(needed))
    if not needed.value:
        raise ctypes.WinError(ctypes.get_last_error())
    buffer = ctypes.create_string_buffer(needed.value)
    if not api.GetFileSecurityW(str(path), 7, buffer, len(buffer), ctypes.byref(needed)):
        raise ctypes.WinError(ctypes.get_last_error())
    return {"descriptor": buffer.raw.hex(), "signature": security_signature(buffer.raw)}


def restore_dacl(path, saved):
    api = security_api()
    raw = bytes.fromhex(saved["descriptor"])
    buffer = ctypes.create_string_buffer(raw)
    if not api.SetFileSecurityW(str(path), 4, buffer):
        raise ctypes.WinError(ctypes.get_last_error())
    if read_security(path)["signature"] != saved["signature"]:
        raise ValueError("owner/group/access-control preservation failed")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def decode(raw):
    return json.loads(raw, object_pairs_hook=unique_object)


def physical_cwd(cwd, alias, target):
    plain = cwd.replace("/", "\\").removeprefix("\\\\?\\")
    if ".." in plain.split("\\") or "." in plain.split("\\"):
        raise ValueError("nonliteral cwd components")
    prefix = normalized(str(alias)) + "\\"
    if not normalized(plain).startswith(prefix):
        raise ValueError("cwd outside alias")
    suffix = plain[len(str(alias).rstrip("\\")) + 1:]
    if not suffix.startswith("Parley\\"):
        raise ValueError("cwd outside Parley worktrees")
    destination = target.joinpath(*suffix.split("\\")).absolute()
    require_physical(destination)
    if not destination.is_relative_to(target) or not os.path.samefile(cwd, destination):
        raise ValueError("cwd identity mismatch")
    for component in Path(plain).parents:
        if component == alias:
            break
        if component.is_symlink() or component.is_junction():
            raise ValueError("nested alias reparse")
    return "\\\\?\\" + str(destination), identity(destination)


def replace_summary(raw, old_cwd, new_cwd):
    original = decode(raw)
    if original["info"]["cwd"] != old_cwd:
        raise ValueError("summary cwd drift")
    before = json.dumps(old_cwd, ensure_ascii=False).encode()
    after = json.dumps(new_cwd, ensure_ascii=False).encode()
    if raw.count(before) != 1:
        raise ValueError("cwd JSON scalar is not unique or uses unsupported escaping")
    changed = raw.replace(before, after, 1)
    expected = copy.deepcopy(original)
    expected["info"]["cwd"] = new_cwd
    if decode(changed) != expected:
        raise ValueError("unrelated JSON change")
    return changed


def tree_state(root):
    require_physical(root)
    files = {}
    directories = []
    for folder, children, names in os.walk(root, followlinks=False):
        for name in children:
            child = Path(folder) / name
            require_physical(child)
            directories.append(child.relative_to(root).as_posix())
        for name in names:
            child = Path(folder) / name
            require_physical(child)
            if not child.is_file():
                raise ValueError("unsupported session file")
            files[child.relative_to(root).as_posix()] = {"sha256": digest(child), "size": child.stat().st_size}
    return {"files": files, "directories": sorted(directories)}


def plan_relocation(root, alias, target, selected=None):
    root, alias, target = (Path(value).absolute() for value in (root, alias, target))
    require_physical(root)
    mapping = validate_mappings({str(alias): str(target)})
    seen = set()
    rows = []
    cwd_identities = {}
    for bucket in sorted(root.iterdir()):
        require_physical(bucket)
        if not bucket.is_dir():
            continue
        for session in sorted(bucket.iterdir()):
            require_physical(session)
            if not session.is_dir():
                continue
            summary_path = session / "summary.json"
            if not summary_path.exists():
                continue
            require_physical(summary_path)
            raw = summary_path.read_bytes()
            summary = decode(raw)
            session_id = summary["info"]["id"]
            if str(uuid.UUID(session_id)) != session.name or session_id in seen:
                raise ValueError("duplicate or mismatched UUID")
            seen.add(session_id)
            cwd = summary["info"]["cwd"]
            if not normalized(cwd).startswith(normalized(str(alias)) + "\\"):
                continue
            if selected is not None and session_id not in selected:
                continue
            if session_id in DENIED_IDS:
                raise ValueError("protected current session")
            new_cwd, cwd_identity = physical_cwd(cwd, alias, target)
            key = tuple(cwd_identity.values())
            if key in cwd_identities and cwd_identities[key] != normalized(cwd):
                raise ValueError("different cwd suffixes resolve to one directory")
            cwd_identities[key] = normalized(cwd)
            encoded = quote(new_cwd, safe="-_.~")
            if len(encoded) > 255 or session.parent.name != quote(cwd, safe="-_.~"):
                raise ValueError("unsupported native cwd encoding")
            destination = root / encoded / session_id
            if destination.exists():
                raise ValueError("destination collision")
            if destination.parent.exists():
                require_physical(destination.parent)
            changed = replace_summary(raw, cwd, new_cwd)
            rows.append({"session_id": session_id, "source": str(session), "destination": str(destination),
                         "old_cwd": cwd, "new_cwd": new_cwd, "cwd_identity": cwd_identity,
                         "directory_identity": identity(session), "state": tree_state(session),
                         "summary_security": read_security(summary_path),
                         "summary_after_sha256": hashlib.sha256(changed).hexdigest()})
    if selected is not None and {row["session_id"] for row in rows} != set(selected):
        raise ValueError("selected sessions missing or already physical")
    return {"schema_version": 1, "root": str(root), "alias": str(alias), "target": str(target),
            "mapping": mapping, "sessions": rows}


def require_quiescent():
    command = "@(Get-CimInstance Win32_Process -ErrorAction Stop -Filter \"Name = 'grok.exe' OR Name = 'agent.exe'\" | Select-Object ProcessId,Name) | ConvertTo-Json -Compress"
    output = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", command],
                            capture_output=True, text=True, check=True, timeout=30).stdout.strip()
    if output and json.loads(output):
        raise ValueError("native Grok/agent process active; no live migration")


@contextmanager
def deny_writes(path):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p,
                                  wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    kernel.CreateFileW.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.CreateFileW(str(path), 0x80000000, 1 | 4, None, 3, 0x80, None)
    if handle == wintypes.HANDLE(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        yield
    finally:
        kernel.CloseHandle(handle)


def replace_bytes(path, raw, staging):
    security = read_security(path)
    temporary = staging / (".relocation-" + str(uuid.uuid4()) + ".tmp")
    with temporary.open("xb") as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())
    restore_dacl(temporary, security)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.ReplaceFileW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.LPCWSTR,
                                   wintypes.DWORD, ctypes.c_void_p, ctypes.c_void_p]
    kernel.ReplaceFileW.restype = wintypes.BOOL
    if not kernel.ReplaceFileW(str(path), str(temporary), None, 0, None, None):
        raise ctypes.WinError(ctypes.get_last_error())
    restore_dacl(path, security)


def write_json(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def expected_state(row, original):
    changed = replace_summary(original, row["old_cwd"], row["new_cwd"])
    expected = copy.deepcopy(row["state"])
    expected["files"]["summary.json"] = {"sha256": row["summary_after_sha256"], "size": len(changed)}
    return expected


def rollback_row(row, transaction):
    require_quiescent()
    source, destination = Path(row["source"]), Path(row["destination"])
    backup = transaction / row["session_id"]
    if source.exists() or not destination.is_dir():
        raise ValueError("rollback source collision or missing destination")
    require_physical(source.parent)
    if identity(destination) != row["directory_identity"] or tree_state(backup) != row["state"]:
        raise ValueError("rollback identity or backup mismatch")
    original = (backup / "summary.json").read_bytes()
    with ExitStack() as held:
        for filename in row["state"]["files"]:
            held.enter_context(deny_writes(destination / filename))
        actual = tree_state(destination)
        if actual not in (row["state"], expected_state(row, original)):
            raise ValueError("rollback refuses divergent bytes")
        if actual != row["state"]:
            replace_bytes(destination / "summary.json", original, transaction)
        restore_dacl(destination / "summary.json", row["summary_security"])
        if tree_state(destination) != row["state"]:
            raise ValueError("rollback summary mismatch")
        held.close()
        require_quiescent()
        if tree_state(destination) != row["state"]:
            raise ValueError("rollback bytes changed before rename")
        destination.rename(source)
        if tree_state(source) != row["state"] or identity(source) != row["directory_identity"]:
            raise ValueError("rollback verification failed")


def durable_copy(source, destination):
    security = read_security(Path(source))
    with open(source, "rb") as reader, open(destination, "xb") as writer:
        shutil.copyfileobj(reader, writer)
        writer.flush()
        os.fsync(writer.fileno())
    shutil.copystat(source, destination)
    restore_dacl(Path(destination), security)
    return destination


def apply_plan(plan, transaction):
    if os.name != "nt":
        raise ValueError("Windows only")
    require_quiescent()
    root = Path(plan["root"])
    transaction = Path(transaction).absolute()
    require_physical(transaction.parent)
    if transaction.is_relative_to(root) or root.is_relative_to(transaction):
        raise ValueError("backup must be outside active sessions tree")
    selected = {row["session_id"] for row in plan["sessions"]}
    fresh = plan_relocation(root, plan["alias"], plan["target"], selected)
    if fresh != plan:
        raise ValueError("manifest drift")
    transaction.mkdir()
    write_json(transaction / "plan.json", plan)
    with (transaction / "journal.jsonl").open("x", encoding="utf-8") as journal:
        def record(state, session_id=None, **details):
            journal.write(json.dumps({"state": state, "session_id": session_id, **details}) + "\n")
            journal.flush()
            os.fsync(journal.fileno())

        record("planned")
        for row in plan["sessions"]:
            source, destination = Path(row["source"]), Path(row["destination"])
            session_id = row["session_id"]
            backup = transaction / session_id
            with ExitStack() as held:
                require_quiescent()
                for filename in row["state"]["files"]:
                    held.enter_context(deny_writes(source / filename))
                if tree_state(source) != row["state"] or identity(source) != row["directory_identity"]:
                    raise ValueError("source changed before backup")
                shutil.copytree(source, backup, copy_function=durable_copy)
                if tree_state(backup) != row["state"]:
                    raise ValueError("backup hash mismatch")
                record("backed-up", session_id)
                destination.parent.mkdir(exist_ok=True)
                require_physical(destination.parent)
                if destination.exists() or not destination.is_relative_to(root):
                    raise ValueError("destination collision or escape")
                require_quiescent()
                try:
                    held.close()
                    if tree_state(source) != row["state"]:
                        raise ValueError("source drift before rename")
                    source.rename(destination)
                    record("renamed", session_id)
                    for filename in row["state"]["files"]:
                        held.enter_context(deny_writes(destination / filename))
                    if tree_state(destination) != row["state"]:
                        raise ValueError("source drift during rename")
                    if identity(destination) != row["directory_identity"]:
                        raise ValueError("directory identity changed")
                    summary = destination / "summary.json"
                    original = (backup / "summary.json").read_bytes()
                    changed = replace_summary(original, row["old_cwd"], row["new_cwd"])
                    timestamps = summary.stat()
                    replace_bytes(summary, changed, transaction)
                    os.utime(summary, ns=(timestamps.st_atime_ns, timestamps.st_mtime_ns))
                    record("metadata-replaced", session_id)
                    if tree_state(destination) != expected_state(row, original):
                        raise ValueError("post-migration hash mismatch")
                    if read_security(summary)["signature"] != row["summary_security"]["signature"]:
                        raise ValueError("summary access-control mismatch")
                    if source.exists():
                        raise ValueError("old UUID directory reappeared")
                    require_quiescent()
                    record("verified", session_id, summary_sha256=digest(summary))
                except Exception as error:
                    record("stopped", session_id, error=str(error))
                    held.close()
                    if destination.exists() and not source.exists():
                        try:
                            rollback_row(row, transaction)
                            record("rolled-back", session_id)
                        except Exception as rollback_error:
                            record("rollback-refused", session_id, error=str(rollback_error))
                    raise
        record("complete", count=len(plan["sessions"]))
    return {"migrated": len(plan["sessions"]), "transaction": str(transaction)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    planning = commands.add_parser("plan")
    planning.add_argument("--root", type=Path, required=True)
    planning.add_argument("--alias", type=Path, required=True)
    planning.add_argument("--target", type=Path, required=True)
    planning.add_argument("--output", type=Path, required=True)
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
        proposed = plan_relocation(args.root, args.alias, args.target, args.session)
        write_json(args.output, proposed)
        print(json.dumps({"sessions": len(proposed["sessions"]), "plan_sha256": digest(args.output)}))
    elif args.command == "apply":
        if digest(args.plan) != args.sha256:
            raise ValueError("plan hash mismatch")
        print(json.dumps(apply_plan(decode(args.plan.read_bytes()), args.transaction)))
    else:
        saved = args.transaction / "plan.json"
        if digest(saved) != args.sha256:
            raise ValueError("rollback plan hash mismatch")
        proposed = decode(saved.read_bytes())
        row = next(item for item in proposed["sessions"] if item["session_id"] == args.session)
        rollback_row(row, args.transaction)
        write_json(args.transaction / ("rollback-" + args.session + ".json"), {"state": "rolled-back"})
