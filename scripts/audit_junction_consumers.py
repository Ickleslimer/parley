import argparse
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import tomllib

from audit_local_paths import normalized, validate_mappings


def references(text, aliases):
    folded = text.replace("/", "\\").casefold()
    hits = []
    for alias in aliases:
        candidate = normalized(alias)
        pattern = re.escape(candidate) + r'(?=$|[\\\s"\'<>;,)\]])'
        if re.search(pattern, folded):
            hits.append(alias)
    return hits


def scan_strings(value, aliases, pointer=""):
    hits = []
    if isinstance(value, dict):
        for key, child in value.items():
            key_path = pointer + "/" + str(key).replace("~", "~0").replace("/", "~1")
            if references(str(key), aliases):
                hits.append({"pointer": key_path, "kind": "key", "aliases": references(str(key), aliases)})
            hits.extend(scan_strings(child, aliases, key_path))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            hits.extend(scan_strings(child, aliases, pointer + "/" + str(index)))
    elif isinstance(value, str):
        matches = references(value, aliases)
        if matches:
            hits.append({"pointer": pointer, "kind": "string", "aliases": matches,
                         "value_bytes": len(value.encode("utf-8")),
                         "value_sha256": hashlib.sha256(value.encode("utf-8")).hexdigest()})
    return hits


def document(path, aliases):
    raw = path.read_bytes()
    data = tomllib.loads(raw.decode("utf-8-sig")) if path.suffix == ".toml" else json.loads(raw)
    return {"file": str(path), "sha256": hashlib.sha256(raw).hexdigest(),
            "hits": scan_strings(data, aliases)}


def database(path, aliases):
    result = {"file": str(path), "mode": "read-only", "tables": [], "hits": []}
    connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=3)
    try:
        connection.execute("PRAGMA query_only=ON")
        tables = [row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")]
        for table in tables:
            if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", table):
                raise ValueError("unsupported SQLite table name")
            if path.name.startswith("state_") and table != "threads":
                continue
            columns = [row[1] for row in connection.execute(f'PRAGMA table_info("{table}")')]
            selected = [column for column in columns if any(part in column.lower() for part in
                        ("path", "cwd", "root", "worktree", "directory"))]
            selected = [column for column in selected if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", column)]
            if not selected:
                result["tables"].append({"table": table, "columns": columns, "scanned": False})
                continue
            quoted = ",".join('"' + column + '"' for column in selected)
            rows = connection.execute(f'SELECT {quoted} FROM "{table}" LIMIT 10001').fetchall()
            if len(rows) > 10000:
                raise ValueError("database audit row limit reached")
            result["tables"].append({"table": table, "columns": selected, "rows": len(rows), "scanned": True})
            for index, row in enumerate(rows):
                hits = scan_strings(dict(zip(selected, row)), aliases)
                result["hits"].extend({"table": table, "row_ordinal": index, **hit} for hit in hits)
    finally:
        connection.close()
    return result


def summaries(root, aliases):
    result = {"checked": 0, "hits": [], "errors": []}
    fields = ("git_root_dir", "source_workspace_dir", "worktree_label", "prompt_display_cwd", "grok_home")
    for path in root.glob("*/*/summary.json"):
        try:
            raw = path.read_bytes()
            data = json.loads(raw)
            selected = {"info": data["info"], **{key: data.get(key) for key in fields}}
            hits = scan_strings(selected, aliases)
            result["checked"] += 1
            result["hits"].extend({"file": str(path), "session_id": data["info"]["id"],
                                   "sha256": hashlib.sha256(raw).hexdigest(), **hit} for hit in hits)
        except (OSError, ValueError, KeyError) as error:
            result["errors"].append({"file": str(path), "error": type(error).__name__})
    return result


def audit(layout):
    aliases = layout["aliases"]
    result = {"mappings": validate_mappings(aliases), "documents": [], "databases": [], "errors": []}
    for filename in layout["documents"]:
        try:
            result["documents"].append(document(Path(filename), aliases))
        except (OSError, ValueError) as error:
            result["errors"].append({"file": filename, "error": type(error).__name__})
    for filename in layout["databases"]:
        try:
            result["databases"].append(database(Path(filename), aliases))
        except (OSError, ValueError, sqlite3.Error) as error:
            result["errors"].append({"file": filename, "error": type(error).__name__})
    result["native_summaries"] = summaries(Path(layout["sessions"]), aliases)
    result["retirement_ready"] = False
    result["scope"] = "Explicit configuration documents, selected database path columns and native grouping metadata only; not an exhaustive machine scan. No mutations."
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("layout", type=Path)
    parser.add_argument("output", type=Path)
    arguments = parser.parse_args()
    result = audit(json.loads(arguments.layout.read_bytes()))
    with arguments.output.open("x", encoding="utf-8") as destination:
        json.dump(result, destination, indent=2)
        destination.write("\n")
    print(json.dumps({"document_hits": sum(len(row["hits"]) for row in result["documents"]),
                      "database_hits": sum(len(row["hits"]) for row in result["databases"]),
                      "summary_hits": len(result["native_summaries"]["hits"]),
                      "errors": len(result["errors"]) + len(result["native_summaries"]["errors"])}))
