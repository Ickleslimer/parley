import argparse
import json
import os
from pathlib import Path
import subprocess

from audit_local_paths import identity, require_physical
from relocate_grok_sessions import decode, digest, require_quiescent, write_json


LINKS = {
    r"D:\Workplaces\.parley-evidence": r"D:\Workplaces\R3 Bio\.parley-evidence",
    r"D:\Workplaces\Parley": r"D:\Workplaces\R3 Bio\Parley",
    r"D:\Workplaces\.parley-worktrees": r"D:\Workplaces\R3 Bio\Parley-root\.parley-worktrees",
}


def entry_identity(path):
    info = path.lstat()
    return {"device": info.st_dev, "file_index": info.st_ino, "tag": info.st_reparse_tag}


def sentinel_state(path):
    require_physical(path)
    if not path.is_file():
        raise ValueError("sentinel must be an existing physical file")
    return {"path": str(path), "identity": identity(path), "sha256": digest(path), "size": path.stat().st_size}


def snapshot(link, target, sentinel, allowed):
    link, target, sentinel = (Path(value).absolute() for value in (link, target, sentinel))
    if str(link) not in allowed or Path(allowed[str(link)]) != target:
        raise ValueError("link not allowlisted")
    require_physical(link.parent)
    require_physical(target)
    if not link.is_junction() or entry_identity(link)["tag"] != 0xA0000003:
        raise ValueError("not an NTFS junction")
    if link.resolve(strict=True) != target or not os.path.samefile(link, target):
        raise ValueError("junction target mismatch")
    if not sentinel.is_relative_to(target):
        raise ValueError("sentinel outside target")
    return {"link": str(link), "target": str(target), "link_identity": entry_identity(link),
            "target_identity": identity(target), "sentinel": sentinel_state(sentinel)}


def verify_target(row):
    target = Path(row["target"])
    require_physical(target)
    if identity(target) != row["target_identity"] or sentinel_state(Path(row["sentinel"]["path"])) != row["sentinel"]:
        raise ValueError("target or sentinel changed")


def unlink_one(row, allowed):
    current = snapshot(row["link"], row["target"], row["sentinel"]["path"], allowed)
    if current != row:
        raise ValueError("junction manifest drift")
    os.rmdir(row["link"])
    if os.path.lexists(row["link"]):
        raise ValueError("link still present after nonrecursive unlink")
    verify_target(row)
    return {"link": row["link"], "absent": True, "target_preserved": True}


def restore_one(row, allowed):
    link, target = Path(row["link"]), Path(row["target"])
    if str(link) not in allowed or Path(allowed[str(link)]) != target:
        raise ValueError("link not allowlisted")
    require_physical(link.parent)
    verify_target(row)
    if os.path.lexists(link):
        raise ValueError("refusing occupied link path")
    if "'" in str(link) or "'" in str(target):
        raise ValueError("unsupported literal quoting")
    command = f"$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path '{link}' -Target '{target}' | Out-Null"
    subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", command],
                   capture_output=True, check=True, timeout=30)
    restored = snapshot(link, target, row["sentinel"]["path"], allowed)
    verify_target(row)
    return {"link": str(link), "restored": True, "new_link_identity": restored["link_identity"]}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("plan", "unlink", "restore"))
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--sha256")
    parser.add_argument("--link", choices=list(LINKS))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.command == "plan":
        sentinels = {
            r"D:\Workplaces\.parley-evidence": Path(LINKS[r"D:\Workplaces\.parley-evidence"]) / "desktop-conversation-widget" / "event-engine-daad2a44.jsonl",
            r"D:\Workplaces\Parley": Path(LINKS[r"D:\Workplaces\Parley"]) / "Cargo.toml",
            r"D:\Workplaces\.parley-worktrees": Path(LINKS[r"D:\Workplaces\.parley-worktrees"]) / "Parley" / "grok-image-lifecycle-repair" / ".git",
        }
        rows = [snapshot(link, target, sentinels[link], LINKS) for link, target in LINKS.items()]
        write_json(args.manifest, {"kind": "three-parley-junctions-v1", "links": rows})
        print(json.dumps({"links": len(rows), "sha256": digest(args.manifest)}))
        return
    if not args.sha256 or digest(args.manifest) != args.sha256 or not args.link or not args.output:
        raise ValueError("exact manifest hash, link and unused output required")
    require_physical(args.output.parent)
    if args.output.exists():
        raise FileExistsError(args.output)
    manifest = decode(args.manifest.read_bytes())
    if manifest["kind"] != "three-parley-junctions-v1" or len(manifest["links"]) != 3:
        raise ValueError("unexpected manifest")
    row = next(row for row in manifest["links"] if row["link"] == args.link)
    require_quiescent()
    result = unlink_one(row, LINKS) if args.command == "unlink" else restore_one(row, LINKS)
    write_json(args.output, result)
    print(json.dumps(result))


if __name__ == "__main__":
    main()
