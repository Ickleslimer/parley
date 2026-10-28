import copy
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import retire_parley_junctions as retirement


@unittest.skipUnless(os.name == "nt", "Windows junction tests")
class JunctionRetirementTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="parley-unlink-")
        self.root = Path(self.temporary.name)
        self.target = self.root / "target"
        self.target.mkdir()
        self.sentinel = self.target / "sentinel.txt"
        self.sentinel.write_bytes(b"Do not delete target data\r\n")
        (self.target / "nested").mkdir()
        (self.target / "nested" / "keep.txt").write_bytes(b"Keep nested data")
        self.link = self.root / "alias"
        self.allowed = {str(self.link): str(self.target)}
        subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
                        f"New-Item -ItemType Junction -Path '{self.link}' -Target '{self.target}' | Out-Null"],
                       check=True, capture_output=True)
        self.row = retirement.snapshot(self.link, self.target, self.sentinel, self.allowed)

    def tearDown(self):
        if os.path.lexists(self.link) and self.link.is_junction():
            if not os.path.samefile(self.link, self.target):
                raise RuntimeError("fixture junction ownership changed")
            os.rmdir(self.link)
        self.temporary.cleanup()

    def test_same_primitive_removes_only_link_and_restores(self):
        retirement.unlink_one(self.row, self.allowed)
        self.assertFalse(os.path.lexists(self.link))
        retirement.verify_target(self.row)
        self.assertEqual((self.target / "nested" / "keep.txt").read_bytes(), b"Keep nested data")
        retirement.restore_one(self.row, self.allowed)
        self.assertTrue(self.link.is_junction())
        self.assertTrue(os.path.samefile(self.link, self.target))
        retirement.verify_target(self.row)

    def test_nonallowlisted_refused(self):
        with self.assertRaisesRegex(ValueError, "allowlisted"):
            retirement.unlink_one(self.row, {})
        self.assertTrue(self.link.is_junction())

    def test_target_directory_cannot_be_deleted(self):
        row = copy.deepcopy(self.row)
        row["link"] = str(self.target)
        with self.assertRaisesRegex(ValueError, "junction"):
            retirement.unlink_one(row, {str(self.target): str(self.target)})
        retirement.verify_target(self.row)

    def test_link_identity_drift_refused(self):
        row = copy.deepcopy(self.row)
        row["link_identity"]["file_index"] += 1
        with self.assertRaisesRegex(ValueError, "drift"):
            retirement.unlink_one(row, self.allowed)

    def test_target_identity_drift_refused(self):
        row = copy.deepcopy(self.row)
        row["target_identity"]["file_index"] += 1
        with self.assertRaisesRegex(ValueError, "drift"):
            retirement.unlink_one(row, self.allowed)

    def test_changed_sentinel_refused(self):
        self.sentinel.write_bytes(b"new data")
        with self.assertRaisesRegex(ValueError, "drift"):
            retirement.unlink_one(self.row, self.allowed)
        self.assertTrue(self.link.is_junction())

    def test_wrong_target_refused(self):
        other = self.root / "other"
        other.mkdir()
        with self.assertRaisesRegex(ValueError, "target mismatch"):
            retirement.snapshot(self.link, other, self.sentinel, {str(self.link): str(other)})

    def test_sentinel_outside_target_refused(self):
        with self.assertRaisesRegex(ValueError, "outside"):
            retirement.snapshot(self.link, self.target, self.root / "outside", self.allowed)

    def test_restore_will_not_replace_existing_link(self):
        with self.assertRaisesRegex(ValueError, "occupied"):
            retirement.restore_one(self.row, self.allowed)

    def test_restore_will_not_replace_regular_directory(self):
        retirement.unlink_one(self.row, self.allowed)
        self.link.mkdir()
        (self.link / "unrelated.txt").write_bytes(b"keep")
        with self.assertRaisesRegex(ValueError, "occupied"):
            retirement.restore_one(self.row, self.allowed)
        self.assertEqual((self.link / "unrelated.txt").read_bytes(), b"keep")

    def test_restore_rejects_changed_target(self):
        retirement.unlink_one(self.row, self.allowed)
        self.sentinel.write_bytes(b"new")
        with self.assertRaisesRegex(ValueError, "changed"):
            retirement.restore_one(self.row, self.allowed)
        self.assertFalse(os.path.lexists(self.link))


if __name__ == "__main__":
    unittest.main()
