import json
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import uuid
from urllib.parse import quote

import relocate_grok_sessions as relocation


@unittest.skipUnless(os.name == "nt", "Windows native filesystem tests")
class SessionRelocationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="parley-relocation-")
        self.root = Path(self.temporary.name)
        self.target = self.root / "physical"
        self.cwd = self.target / "Parley" / "example"
        self.cwd.mkdir(parents=True)
        self.alias = self.root / "alias"
        subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
                        f"New-Item -ItemType Junction -Path '{self.alias}' -Target '{self.target}' | Out-Null"],
                       check=True, capture_output=True)
        self.store = self.root / "sessions"
        self.store.mkdir()
        self.old_cwd = "\\\\?\\" + str(self.alias / "Parley" / "example")
        self.session_id = str(uuid.uuid4())
        self.session = self.store / quote(self.old_cwd, safe="-_.~") / self.session_id
        self.session.mkdir(parents=True)
        summary = {"info": {"id": self.session_id, "cwd": self.old_cwd},
                   "git_root_dir": str(self.alias / "Parley" / "example"), "other": [1, "preserved"]}
        self.raw = (json.dumps(summary, indent=4) + "\r\n").encode()
        (self.session / "summary.json").write_bytes(self.raw)
        (self.session / "chat_history.jsonl").write_bytes(b'{"text":"exact historical path"}\r\n')
        (self.session / "summary.json.lock").touch()
        (self.session / "terminal").mkdir()
        self.transaction = self.root / "backup"
        self.quiescence = patch.object(relocation, "require_quiescent")
        self.quiescence.start()

    def tearDown(self):
        self.quiescence.stop()
        if not self.alias.is_junction() or not os.path.samefile(self.alias, self.target):
            raise RuntimeError("fixture junction ownership changed")
        self.alias.rmdir()
        self.temporary.cleanup()

    def plan(self):
        return relocation.plan_relocation(self.store, self.alias, self.target)

    def test_exact_migration_backup_and_rollback(self):
        plan = self.plan()
        row = plan["sessions"][0]
        result = relocation.apply_plan(plan, self.transaction)
        self.assertEqual(result["migrated"], 1)
        self.assertFalse(self.session.exists())
        destination = Path(row["destination"])
        self.assertEqual(relocation.identity(destination), row["directory_identity"])
        self.assertEqual((self.transaction / self.session_id / "summary.json").read_bytes(), self.raw)
        expected = json.loads(self.raw)
        expected["info"]["cwd"] = row["new_cwd"]
        self.assertEqual(json.loads((destination / "summary.json").read_bytes()), expected)
        self.assertEqual(len(list(self.store.glob(f"*/{self.session_id}"))), 1)
        self.assertEqual(self.plan()["sessions"], [])
        relocation.rollback_row(row, self.transaction)
        self.assertEqual(relocation.tree_state(self.session), row["state"])
        self.assertEqual((self.session / "summary.json").read_bytes(), self.raw)

    def test_duplicate_uuid_refused(self):
        shutil.copytree(self.session, self.store / "duplicate" / self.session_id)
        with self.assertRaisesRegex(ValueError, "duplicate"):
            self.plan()

    def test_json_duplicate_key_refused(self):
        with self.assertRaisesRegex(ValueError, "duplicate JSON"):
            relocation.decode(b'{"info": 1, "info": 2}')

    def test_unrelated_identical_scalar_refused(self):
        document = json.loads(self.raw)
        document["duplicate"] = self.old_cwd
        with self.assertRaisesRegex(ValueError, "not unique"):
            relocation.replace_summary(json.dumps(document).encode(), self.old_cwd, "new")

    def test_protected_session_refused(self):
        with patch.object(relocation, "DENIED_IDS", {self.session_id}):
            with self.assertRaisesRegex(ValueError, "protected"):
                self.plan()

    def test_traversal_refused(self):
        with self.assertRaisesRegex(ValueError, "nonliteral"):
            relocation.physical_cwd(str(self.alias / "Parley" / ".." / "example"), self.alias, self.target)

    def test_missing_target_refused(self):
        with self.assertRaisesRegex(ValueError, "missing physical"):
            relocation.physical_cwd(str(self.alias / "Parley" / "missing"), self.alias, self.target)

    def test_wrong_mapping_refused(self):
        other = self.root / "other"
        other.mkdir()
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            relocation.plan_relocation(self.store, self.alias, other)

    def test_destination_collision_refused(self):
        destination = Path(self.plan()["sessions"][0]["destination"])
        destination.mkdir(parents=True)
        with self.assertRaisesRegex(ValueError, "collision"):
            self.plan()

    def test_encoding_mismatch_refused(self):
        self.session.parent.rename(self.store / "wrong-encoding")
        with self.assertRaisesRegex(ValueError, "encoding"):
            self.plan()

    def test_hash_drift_refused_before_mutation(self):
        plan = self.plan()
        (self.session / "chat_history.jsonl").write_bytes(b"new event")
        with self.assertRaisesRegex(ValueError, "manifest drift"):
            relocation.apply_plan(plan, self.transaction)
        self.assertFalse(self.transaction.exists())

    def test_active_process_refused_before_mutation(self):
        plan = self.plan()
        with patch.object(relocation, "require_quiescent", side_effect=ValueError("native process active")):
            with self.assertRaisesRegex(ValueError, "active"):
                relocation.apply_plan(plan, self.transaction)
        self.assertFalse(self.transaction.exists())

    def test_write_lock_is_real(self):
        filename = self.session / "chat_history.jsonl"
        with relocation.deny_writes(filename):
            with self.assertRaises(PermissionError):
                filename.write_bytes(b"unexpected write")

    def test_failed_replace_rolls_back_without_rewriting_chat(self):
        plan = self.plan()
        with patch.object(relocation, "replace_bytes", side_effect=OSError("injected replacement failure")):
            with self.assertRaisesRegex(OSError, "injected"):
                relocation.apply_plan(plan, self.transaction)
        self.assertEqual(relocation.tree_state(self.session), plan["sessions"][0]["state"])
        journal = (self.transaction / "journal.jsonl").read_text()
        self.assertIn("rolled-back", journal)

    def test_crash_gap_recovery(self):
        plan = self.plan()
        row = plan["sessions"][0]
        self.transaction.mkdir()
        shutil.copytree(self.session, self.transaction / self.session_id)
        destination = Path(row["destination"])
        destination.parent.mkdir()
        self.session.rename(destination)
        relocation.rollback_row(row, self.transaction)
        self.assertEqual(relocation.tree_state(self.session), row["state"])

    def test_rollback_refuses_divergent_bytes(self):
        plan = self.plan()
        relocation.apply_plan(plan, self.transaction)
        row = plan["sessions"][0]
        (Path(row["destination"]) / "chat_history.jsonl").write_bytes(b"new event after migration")
        with self.assertRaisesRegex(ValueError, "divergent"):
            relocation.rollback_row(row, self.transaction)
        self.assertFalse(self.session.exists())

    def test_no_rerun_over_existing_transaction(self):
        plan = self.plan()
        self.transaction.mkdir()
        with self.assertRaises(FileExistsError):
            relocation.apply_plan(plan, self.transaction)
        self.assertTrue(self.session.exists())

    def test_backup_inside_store_refused(self):
        with self.assertRaisesRegex(ValueError, "outside"):
            relocation.apply_plan(self.plan(), self.store / "backup")

    def test_two_sessions_same_cwd_preserve_ids(self):
        other_id = str(uuid.uuid4())
        other = self.session.parent / other_id
        shutil.copytree(self.session, other)
        content = json.loads((other / "summary.json").read_bytes())
        content["info"]["id"] = other_id
        (other / "summary.json").write_text(json.dumps(content), encoding="utf-8")
        plan = self.plan()
        self.assertEqual(len(plan["sessions"]), 2)
        relocation.apply_plan(plan, self.transaction)
        self.assertEqual({entry.name for entry in self.store.glob("*/*") if entry.is_dir()},
                         {self.session_id, other_id})

    def test_corrupt_backup_refused_without_live_mutation(self):
        plan = self.plan()
        def corrupted_copy(source, destination):
            shutil.copy2(source, destination)
            if Path(destination).name == "summary.json":
                Path(destination).write_bytes(b"corrupt backup")
        with patch.object(relocation, "durable_copy", side_effect=corrupted_copy):
            with self.assertRaisesRegex(ValueError, "backup hash mismatch"):
                relocation.apply_plan(plan, self.transaction)
        self.assertEqual(relocation.tree_state(self.session), plan["sessions"][0]["state"])

    def test_locked_live_file_refused(self):
        plan = self.plan()
        with (self.session / "chat_history.jsonl").open("ab"):
            with self.assertRaises(PermissionError):
                relocation.apply_plan(plan, self.transaction)
        self.assertEqual(relocation.tree_state(self.session), plan["sessions"][0]["state"])

    def test_nested_reparse_refused(self):
        nested = self.session / "redirect"
        subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
                        f"New-Item -ItemType Junction -Path '{nested}' -Target '{self.cwd}' | Out-Null"],
                       check=True, capture_output=True)
        try:
            with self.assertRaisesRegex(ValueError, "redirected physical"):
                self.plan()
        finally:
            if not nested.is_junction() or not os.path.samefile(nested, self.cwd):
                raise RuntimeError("nested fixture identity changed")
            nested.rmdir()

    def test_replacement_removes_staging_acl_inheritance(self):
        original = relocation.read_security(self.session / "summary.json")
        self.transaction.mkdir()
        staging_security = relocation.read_security(self.transaction)
        api = relocation.security_api()
        api.ConvertStringSecurityDescriptorToSecurityDescriptorW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD,
                                                                            ctypes.POINTER(ctypes.c_void_p),
                                                                            ctypes.POINTER(wintypes.DWORD)]
        pointer = ctypes.c_void_p()
        if not api.ConvertStringSecurityDescriptorToSecurityDescriptorW(
                "D:(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;GR;;;BG)", 1, ctypes.byref(pointer), None):
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            if not api.SetFileSecurityW(str(self.transaction), 4, pointer):
                raise ctypes.WinError(ctypes.get_last_error())
        finally:
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.LocalFree.argtypes = [ctypes.c_void_p]
            kernel.LocalFree(pointer)
        try:
            relocation.replace_bytes(self.session / "summary.json", self.raw, self.transaction)
            self.assertEqual(relocation.read_security(self.session / "summary.json")["signature"],
                             original["signature"])
        finally:
            relocation.restore_dacl(self.transaction, staging_security)


if __name__ == "__main__":
    unittest.main()
