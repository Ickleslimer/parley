import ctypes
from ctypes import wintypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]
CONFIG = Path(r"D:\AppData\Codex\UserProfile\.codex\config.toml")
OLD_HASH = "8c305cdb56665d5ec38bf17c254934b12ce54577795b1f3cac7ecfdacc9cb3b5"
CONFIG_HASH = "d42fdbf4fb31cb15b765b26c8389d5021d846dcd28706d5a1b6c8d41ae1cd3d0"
BACKUP = CONFIG.parent / "config-backups/parley-paginated-context-20261020.toml"
HELPER = ROOT.parent / "grok-image-lifecycle-repair/build/activate_candidate.py"
HELPER_HASH = "b98f6d7782e59756b703e5b1238ca17a669730a4875c9bbd79118eabfaba81b8"
RUNTIME = ("src/ask.rs", "src/context/active_index.rs", "src/context/lineage.rs",
           "src/context/logical_file.rs", "src/context/journal.rs", "src/context/mod.rs",
           "src/context/reader.rs", "src/context/root.rs", "src/context/winfile.rs")
PREVIOUS = {
    "parley-widget-dev": Path(r"C:\Users\mrdyl\AppData\Local\Programs\par-widget-dev\par-grok-async-images-8c305cdb56665d5e.exe"),
    "r3-workbench-parley": Path(r"C:\Users\mrdyl\AppData\Local\Programs\par-r3\par-grok-async-images-8c305cdb56665d5e.exe"),
}


def digest(content):
    return hashlib.sha256(content).hexdigest()


def updated_configuration(content, replacements):
    if digest(HELPER.read_bytes()) != HELPER_HASH:
        raise ValueError("reviewed_configuration_helper_changed")
    spec = importlib.util.spec_from_file_location("reviewed_command_replacement", HELPER)
    helper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helper)
    return helper.updated_configuration(content, replacements)


def gates(manifest):
    dry_run = json.loads((ROOT / "build/live-dry-run-final.json").read_bytes())
    if (dry_run["candidateSha256"] != manifest["sha256"] or dry_run["exitCode"] != 0
            or dry_run["journalUnchanged"] is not True):
        raise ValueError("real_source_read_only_gate_failed")
    review = json.loads((ROOT / "build/review-acceptance.json").read_bytes())
    if (review["candidateSha256"] != manifest["sha256"]
            or review["accepted"] is not True or review["terminalSucceeded"] is not True):
        raise ValueError("source_review_not_accepted")
    if set(review["runtimeSources"]) != set(RUNTIME):
        raise ValueError("reviewed_runtime_set_incomplete")
    for relative, expected in review["runtimeSources"].items():
        if digest((ROOT / relative).read_bytes()) != expected:
            raise ValueError("reviewed_source_changed:" + relative)


def atomic_replace(content):
    pending = CONFIG.with_name("config.parley-paginated-context-20261020.pending.toml")
    if pending.exists() or BACKUP.exists():
        raise ValueError("pending_or_backup_exists")
    with pending.open("xb") as destination:
        destination.write(content)
        destination.flush()
        os.fsync(destination.fileno())
    replace = ctypes.WinDLL("kernel32", use_last_error=True).ReplaceFileW
    replace.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.LPCWSTR,
                        wintypes.DWORD, wintypes.LPVOID, wintypes.LPVOID]
    replace.restype = wintypes.BOOL
    if not replace(str(CONFIG), str(pending), str(BACKUP), 0, None, None):
        raise ctypes.WinError(ctypes.get_last_error())


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in ("plan", "apply"):
        raise ValueError("use_plan_or_apply")
    manifest = json.loads((ROOT / "build/staged/manifest.json").read_bytes())
    before = CONFIG.read_bytes()
    if digest(before) != CONFIG_HASH:
        raise ValueError("configuration_changed")
    binary = Path(manifest["candidate"]).read_bytes()
    if digest(binary) != manifest["sha256"]:
        raise ValueError("candidate_changed")
    destinations = []
    replacements = {}
    for name, previous in PREVIOUS.items():
        if digest(previous.read_bytes()) != OLD_HASH:
            raise ValueError("baseline_changed:" + name)
        target = previous.with_name(Path(manifest["candidate"]).name)
        if target.exists() and digest(target.read_bytes()) != manifest["sha256"]:
            raise ValueError("destination_collision:" + name)
        replacements[name] = (str(previous), str(target))
        destinations.append({"profile": name, "previous": str(previous), "destination": str(target)})
    after = updated_configuration(before, replacements)
    record = {"status": "activation_pending", "candidateSha256": manifest["sha256"],
              "beforeConfigurationSha256": digest(before), "afterConfigurationSha256": digest(after),
              "backup": str(BACKUP), "destinations": destinations,
              "onlyCommandFieldsChanged": True}
    if sys.argv[1] == "apply":
        gates(manifest)
        state = ROOT / "build/activation-state.json"
        if state.exists() or BACKUP.exists():
            raise ValueError("activation_already_recorded")
        for row in destinations:
            target = Path(row["destination"])
            if not target.exists():
                with target.open("xb") as destination:
                    destination.write(binary)
                    destination.flush()
                    os.fsync(destination.fileno())
            if digest(target.read_bytes()) != manifest["sha256"]:
                raise ValueError("installed_candidate_mismatch")
        if CONFIG.read_bytes() != before:
            raise ValueError("configuration_changed_before_replace")
        atomic_replace(after)
        if CONFIG.read_bytes() != after or BACKUP.read_bytes() != before:
            raise ValueError("configuration_readback_mismatch")
        record["status"] = "configuration_changed_host_reload_pending"
        with state.open("x", encoding="utf-8") as destination:
            json.dump(record, destination, indent=2)
    print(json.dumps(record))


if __name__ == "__main__":
    main()
