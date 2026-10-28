import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from audit_local_paths import alias_match, path_hits, require_physical, validate_mappings, viewer_plan


class PathAuditTests(unittest.TestCase):
    def test_component_boundaries(self):
        mappings = {r"D:\Workplaces\Parley": r"D:\new\Parley"}
        self.assertIsNotNone(alias_match(r"\\?\D:\WORKPLACES\Parley\src", mappings))
        self.assertIsNotNone(alias_match("D:/Workplaces/Parley/src", mappings))
        self.assertIsNone(alias_match(r"D:\Workplaces\Parley-other\src", mappings))
        self.assertIsNone(alias_match("The archive once used D:/Workplaces/Parley", mappings))

    def test_structured_live_path_not_archive_prose(self):
        mappings = {r"D:\old": r"D:\new"}
        hits = path_hits({"target": r"D:\old\app.exe", "note": "Earlier: D:/old/app.exe"}, mappings)
        self.assertEqual(hits, [{"pointer": "/target", "path": r"D:\old\app.exe"}])

    def test_only_log_fields_change(self):
        original = {"selectedLog": "old", "selectedLogs": ["old", "keep"], "width": 720,
                    "corner": "top-right", "settingsVersion": 3, "future": {"path": "old"}}
        with patch("audit_local_paths.remap_existing_file", side_effect=lambda value, _: (
                "new", {"before": value, "after": "new"}) if value == "old" else (value, None)):
            plan, encoded = viewer_plan(json.dumps(original).encode(), {})
        migrated = json.loads(encoded)
        self.assertEqual(migrated, {**original, "selectedLog": "new", "selectedLogs": ["new", "keep"]})
        self.assertEqual(len(plan["changes"]), 2)

    def test_invalid_log_list_refused(self):
        with self.assertRaises(ValueError):
            viewer_plan(b'{"selectedLogs": "not an array"}', {})

    def test_no_migration_is_idempotent(self):
        plan, encoded = viewer_plan(b'{"selectedLogs": ["C:/current/events.jsonl"], "width": 720}', {})
        second, repeated = viewer_plan(encoded, {})
        self.assertEqual(plan["changes"], [])
        self.assertEqual(encoded, repeated)
        self.assertEqual(second["before_sha256"], plan["after_sha256"])

    @unittest.skipUnless(os.name == "nt", "Windows junction integration")
    def test_real_junction_identity_migration_and_misdirection(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            physical = root / "physical"
            physical.mkdir()
            log = physical / "events.jsonl"
            log.write_text("exact events\n", encoding="utf-8")
            wrong = root / "other"
            wrong.mkdir()
            alias = root / "alias"
            command = ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
                       f"New-Item -ItemType Junction -Path '{alias}' -Target '{physical}' | Out-Null"]
            subprocess.run(command, check=True, capture_output=True)
            try:
                mappings = {str(alias): str(physical)}
                proof = validate_mappings(mappings)
                self.assertGreater(proof[0]["identity"]["file_index"], 0)
                archive = root / "journal.jsonl"
                archive.write_text(json.dumps({"cwd": str(alias)}), encoding="utf-8")
                original_archive = archive.read_bytes()
                raw = json.dumps({"selectedLogs": [str(alias / "events.jsonl")], "width": 720}).encode()
                plan, encoded = viewer_plan(raw, mappings)
                self.assertEqual(json.loads(encoded)["selectedLogs"], [str(log)])
                self.assertEqual(plan["changes"][0]["verified_prefix_bytes"], len(log.read_bytes()))
                self.assertEqual(archive.read_bytes(), original_archive)
                with self.assertRaises(ValueError):
                    validate_mappings({str(alias): str(wrong)})
                with self.assertRaises(ValueError):
                    require_physical(alias / "events.jsonl")
            finally:
                if not alias.is_junction() or not os.path.samefile(alias, physical):
                    raise RuntimeError("fixture junction ownership changed")
                alias.rmdir()
                self.assertTrue(log.is_file())


if __name__ == "__main__":
    unittest.main()
