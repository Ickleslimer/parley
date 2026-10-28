import copy
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import uuid
from urllib.parse import quote

import relocate_grok_grouping as grouping


@unittest.skipUnless(os.name == "nt", "Windows native filesystem tests")
class GroupingRelocationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="parley-grouping-")
        self.root = Path(self.temporary.name)
        self.target = self.root / "physical"
        self.cwd = self.target / "Parley" / "example"
        self.cwd.mkdir(parents=True)
        subprocess.run(["git", "init", str(self.cwd)], check=True, capture_output=True)
        self.alias = self.root / "alias"
        subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
                        f"New-Item -ItemType Junction -Path '{self.alias}' -Target '{self.target}' | Out-Null"],
                       check=True, capture_output=True)
        self.store = self.root / "sessions"
        self.store.mkdir()
        self.session_id = str(uuid.uuid4())
        self.cwd_text = "\\\\?\\" + str(self.cwd)
        self.session = self.store / quote(self.cwd_text, safe="-_.~") / self.session_id
        self.session.mkdir(parents=True)
        self.old = (self.alias / "Parley" / "example").as_posix() + "/"
        self.new = self.cwd.as_posix() + "/"
        self.document = {"info": {"id": self.session_id, "cwd": self.cwd_text},
                         "git_root_dir": self.old, "head_commit": "0" * 40,
                         "head_branch": "saved-branch", "other": [1, "exact history", "é"]}
        self.raw = (json.dumps(self.document, indent=4, ensure_ascii=False) + "\r\n").encode()
        (self.session / "summary.json").write_bytes(self.raw)
        (self.session / "chat_history.jsonl").write_bytes(b'{"text":"unmodified"}\r\n')
        (self.session / "summary.json.lock").touch()
        (self.session / "terminal").mkdir()
        self.transaction = self.root / "backup"
        self.quiescence = patch.object(grouping.safe, "require_quiescent")
        self.quiescence.start()

    def tearDown(self):
        self.quiescence.stop()
        if not self.alias.is_junction() or not os.path.samefile(self.alias, self.target):
            raise RuntimeError("fixture junction ownership changed")
        self.alias.rmdir()
        self.temporary.cleanup()

    def plan(self, selected=None):
        return grouping.plan_grouping(self.store, self.alias, self.target, selected)

    def rewrite(self, document):
        (self.session / "summary.json").write_text(json.dumps(document), encoding="utf-8")

    def test_exact_one_scalar_backup_permissions_and_rollback(self):
        plan = self.plan()
        row = plan["sessions"][0]
        self.assertEqual(row["new_root"], self.new)
        self.assertEqual(grouping.apply_plan(plan, self.transaction)["migrated"], 1)
        self.assertEqual((self.session / "summary.json").read_bytes(),
                         self.raw.replace(json.dumps(self.old).encode(), json.dumps(self.new).encode()))
        self.assertEqual((self.transaction / self.session_id / "summary.json").read_bytes(), self.raw)
        grouping.verify_row(row, self.transaction)
        self.assertEqual(len(list(self.store.glob(f"*/{self.session_id}"))), 1)
        self.assertEqual(self.plan()["sessions"], [])
        grouping.rollback_row(row, self.transaction)
        grouping.verify_row(row, self.transaction, migrated=False)
        self.assertEqual((self.session / "summary.json").read_bytes(), self.raw)

    def test_repeated_scalar_refused(self):
        self.document["other"] = self.old
        self.rewrite(self.document)
        with self.assertRaisesRegex(ValueError, "not unique"):
            self.plan()

    def test_unchanged_root_refused(self):
        with self.assertRaisesRegex(ValueError, "unchanged"):
            grouping.replace_root(self.raw, self.old, self.old)

    def test_wrong_old_root_refused(self):
        with self.assertRaisesRegex(ValueError, "drift"):
            grouping.replace_root(self.raw, "different", self.new)

    def test_duplicate_json_key_refused(self):
        with self.assertRaisesRegex(ValueError, "duplicate JSON"):
            grouping.replace_root(b'{"git_root_dir":"a","git_root_dir":"a"}', "a", "b")

    def test_duplicate_uuid_without_summary_refused(self):
        (self.store / "other" / self.session_id).mkdir(parents=True)
        with self.assertRaisesRegex(ValueError, "duplicate"):
            self.plan()

    def test_uuid_mismatch_refused(self):
        self.document["info"]["id"] = str(uuid.uuid4())
        self.rewrite(self.document)
        with self.assertRaisesRegex(ValueError, "UUID mismatch"):
            self.plan()

    def test_encoding_mismatch_refused(self):
        self.session.parent.rename(self.store / "wrong")
        with self.assertRaisesRegex(ValueError, "encoding"):
            self.plan()

    def test_nonphysical_cwd_refused(self):
        self.document["info"]["cwd"] = str(self.alias / "Parley" / "example")
        self.rewrite(self.document)
        with self.assertRaisesRegex(ValueError, "redirected physical"):
            self.plan()

    def test_protected_current_session_refused(self):
        with patch.object(grouping.safe, "DENIED_IDS", {self.session_id}):
            with self.assertRaisesRegex(ValueError, "protected"):
                self.plan()

    def test_unproved_spellings_refused(self):
        for value in (self.old[:-1], self.old.replace("/", "\\"), self.old.lower()):
            with self.subTest(value=value):
                with self.assertRaisesRegex(ValueError, "spelling"):
                    grouping.root_binding(self.cwd_text, value, self.alias, self.target)

    def test_traversal_refused(self):
        with self.assertRaisesRegex(ValueError, "nonliteral"):
            grouping.root_binding(self.cwd_text, self.alias.as_posix() + "/Parley/../Parley/example/", self.alias, self.target)

    def test_wrong_mapping_refused(self):
        other = self.root / "wrong"
        other.mkdir()
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            grouping.plan_grouping(self.store, self.alias, other)

    def test_git_root_mismatch_refused(self):
        other = self.cwd / "subdir"
        other.mkdir()
        with self.assertRaisesRegex(ValueError, "Git root"):
            grouping.root_binding(self.cwd_text, self.old + "subdir/", self.alias, self.target)

    def test_git_status_drift_refused(self):
        with patch.object(grouping, "git_read", side_effect=[b"", self.cwd.as_posix().encode(), b"new"]):
            with self.assertRaisesRegex(ValueError, "status changed"):
                grouping.root_binding(self.cwd_text, self.old, self.alias, self.target)

    def test_selected_missing_refused(self):
        with self.assertRaisesRegex(ValueError, "selected sessions missing"):
            self.plan([str(uuid.uuid4())])

    def test_hash_drift_refused_without_transaction(self):
        plan = self.plan()
        (self.session / "chat_history.jsonl").write_bytes(b"new event")
        with self.assertRaisesRegex(ValueError, "manifest drift"):
            grouping.apply_plan(plan, self.transaction)
        self.assertFalse(self.transaction.exists())

    def test_active_native_process_refused(self):
        plan = self.plan()
        with patch.object(grouping.safe, "require_quiescent", side_effect=ValueError("native process active")):
            with self.assertRaisesRegex(ValueError, "active"):
                grouping.apply_plan(plan, self.transaction)
        self.assertFalse(self.transaction.exists())

    def test_wrong_plan_kind_refused(self):
        plan = self.plan()
        plan["kind"] = "cwd-migration"
        with self.assertRaisesRegex(ValueError, "grouping manifest"):
            grouping.apply_plan(plan, self.transaction)

    def test_no_existing_transaction_reuse(self):
        plan = self.plan()
        self.transaction.mkdir()
        with self.assertRaises(FileExistsError):
            grouping.apply_plan(plan, self.transaction)

    def test_backup_inside_live_store_refused(self):
        with self.assertRaisesRegex(ValueError, "outside"):
            grouping.apply_plan(self.plan(), self.store / "backup")

    def test_locked_live_file_refused(self):
        plan = self.plan()
        with (self.session / "chat_history.jsonl").open("ab"):
            with self.assertRaises(PermissionError):
                grouping.apply_plan(plan, self.transaction)
        self.assertEqual((self.session / "summary.json").read_bytes(), self.raw)

    def test_corrupt_backup_does_not_modify_live(self):
        plan = self.plan()
        def corrupt(source, destination):
            shutil.copy2(source, destination)
            if Path(destination).name == "summary.json":
                Path(destination).write_bytes(b"corrupt")
        with patch.object(grouping.safe, "durable_copy", side_effect=corrupt):
            with self.assertRaisesRegex(ValueError, "backup hash mismatch"):
                grouping.apply_plan(plan, self.transaction)
        self.assertEqual((self.session / "summary.json").read_bytes(), self.raw)

    def test_failed_replace_rolls_back(self):
        plan = self.plan()
        with patch.object(grouping.safe, "replace_bytes", side_effect=OSError("injected failure")):
            with self.assertRaisesRegex(OSError, "injected"):
                grouping.apply_plan(plan, self.transaction)
        grouping.verify_row(plan["sessions"][0], self.transaction, migrated=False)
        self.assertIn("rolled-back", (self.transaction / "journal.jsonl").read_text())

    def test_replaced_then_failed_verification_rolls_back(self):
        plan = self.plan()
        verify = grouping.verify_row
        def fail_migrated(row, transaction, migrated=True):
            if migrated:
                raise ValueError("injected post-replace failure")
            return verify(row, transaction, migrated=False)
        with patch.object(grouping, "verify_row", side_effect=fail_migrated):
            with self.assertRaisesRegex(ValueError, "post-replace"):
                grouping.apply_plan(plan, self.transaction)
        verify(plan["sessions"][0], self.transaction, migrated=False)

    def test_rollback_refuses_new_history(self):
        plan = self.plan()
        grouping.apply_plan(plan, self.transaction)
        (self.session / "chat_history.jsonl").write_bytes(b"new event")
        with self.assertRaisesRegex(ValueError, "divergent"):
            grouping.rollback_row(plan["sessions"][0], self.transaction)
        self.assertEqual(json.loads((self.session / "summary.json").read_bytes())["git_root_dir"], self.new)

    def test_partial_batch_keeps_earlier_verified_row(self):
        other_id = str(uuid.uuid4())
        other = self.session.parent / other_id
        shutil.copytree(self.session, other)
        document = copy.deepcopy(self.document)
        document["info"]["id"] = other_id
        (other / "summary.json").write_text(json.dumps(document), encoding="utf-8")
        plan = self.plan()
        replace = grouping.safe.replace_bytes
        calls = []
        def fail_second(path, raw, staging):
            calls.append(path)
            if len(calls) == 2:
                raise OSError("second row failure")
            replace(path, raw, staging)
        with patch.object(grouping.safe, "replace_bytes", side_effect=fail_second):
            with self.assertRaisesRegex(OSError, "second row"):
                grouping.apply_plan(plan, self.transaction)
        grouping.verify_row(plan["sessions"][0], self.transaction)
        grouping.verify_row(plan["sessions"][1], self.transaction, migrated=False)


if __name__ == "__main__":
    unittest.main()
