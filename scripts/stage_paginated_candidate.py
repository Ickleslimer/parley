import hashlib
import json
from pathlib import Path
import shutil
import subprocess


ROOT = Path(__file__).resolve().parents[1]
RUNTIME = ["src/ask.rs", "src/context/active_index.rs", "src/context/lineage.rs",
           "src/context/logical_file.rs", "src/context/journal.rs", "src/context/mod.rs",
           "src/context/reader.rs", "src/context/root.rs", "src/context/winfile.rs"]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    source = ROOT / "target/release/par.exe"
    identity = digest(source)
    stage = ROOT / "build/staged"
    stage.mkdir(exist_ok=False)
    target = stage / ("par-paginated-context-" + identity[:16] + ".exe")
    shutil.copy2(source, target)
    if digest(target) != identity:
        raise ValueError("candidate_copy_changed")
    patch = subprocess.run(["git", "diff", "--no-ext-diff", "--binary", "HEAD"], cwd=ROOT,
                           capture_output=True, check=True).stdout
    (stage / "source.patch").write_bytes(patch)
    extra = ["src/context/active_index.rs", "src/context/lineage.rs", "src/context/logical_file.rs",
             "src/context/paginated_tests.rs"]
    for relative in extra:
        destination = stage / "new-source" / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / relative, destination)
    record = {"candidate": str(target), "sha256": identity, "bytes": target.stat().st_size,
              "baseCommit": "329c1cc2f91a97503b972dac6fd28a6d97388467",
              "baselinePatchSha256": "69b11aaeb69f310ccb4f8ce971c2fbc6b99e5298a524b68f851edb7d9e99327b",
              "sourcePatchSha256": digest(stage / "source.patch"),
              "runtimeSources": {relative: digest(ROOT / relative) for relative in RUNTIME},
              "newSources": {relative: digest(ROOT / relative) for relative in extra},
              "status": "activation_pending"}
    with (stage / "manifest.json").open("x", encoding="utf-8") as destination:
        json.dump(record, destination, indent=2)
    print(json.dumps(record))


if __name__ == "__main__":
    main()
