import copy
import hashlib
import json
from pathlib import Path
import tomllib


ROOT = Path(__file__).resolve().parents[1]
CONFIG = Path(r"D:\AppData\Codex\UserProfile\.codex\config.toml")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    state = json.loads((ROOT / "build/activation-state.json").read_bytes())
    before_path = Path(state["backup"])
    before = tomllib.loads(before_path.read_text(encoding="utf-8-sig"))
    after = tomllib.loads(CONFIG.read_text(encoding="utf-8-sig"))
    expected = copy.deepcopy(before)
    copies = []
    for row in state["destinations"]:
        expected["mcp_servers"][row["profile"]]["command"] = row["destination"]
        actual = digest(Path(row["destination"]))
        if actual != state["candidateSha256"]:
            raise ValueError("installed_candidate_changed:" + row["profile"])
        copies.append({"profile": row["profile"], "sha256": actual})
    if after != expected or digest(CONFIG) != state["afterConfigurationSha256"]:
        raise ValueError("configuration_semantics_changed")
    if digest(before_path) != state["beforeConfigurationSha256"]:
        raise ValueError("rollback_configuration_changed")
    record = {"onlyTwoCommandFieldsChanged": True, "allEnvironmentAndPinsPreserved": True,
              "otherProfilesUnchanged": True, "configSha256": digest(CONFIG), "copies": copies}
    with (ROOT / "build/activation-verification.json").open("x", encoding="utf-8") as destination:
        json.dump(record, destination, indent=2)
    print(json.dumps(record))


if __name__ == "__main__":
    main()
