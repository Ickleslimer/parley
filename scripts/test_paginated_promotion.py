import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import promote_paginated_context as promotion


class PromotionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="parley-context-promotion-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / "build").mkdir()
        source = self.root / "runtime.rs"
        source.write_text("synthetic", encoding="utf-8")
        self.manifest = {"sha256": "a" * 64}
        self.dry_run = {"candidateSha256": "a" * 64, "exitCode": 0, "journalUnchanged": True}
        self.review = {"candidateSha256": "a" * 64, "accepted": True,
                       "terminalSucceeded": True,
                       "runtimeSources": {"runtime.rs": hashlib.sha256(source.read_bytes()).hexdigest()}}
        self.save()
        substitute = patch.object(promotion, "ROOT", self.root)
        substitute.start()
        self.addCleanup(substitute.stop)
        runtime = patch.object(promotion, "RUNTIME", ("runtime.rs",))
        runtime.start()
        self.addCleanup(runtime.stop)

    def save(self):
        (self.root / "build/live-dry-run-final.json").write_text(json.dumps(self.dry_run), encoding="utf-8")
        (self.root / "build/review-acceptance.json").write_text(json.dumps(self.review), encoding="utf-8")

    def test_complete_evidence_passes(self):
        promotion.gates(self.manifest)

    def test_failed_or_mutating_dry_run_refuses(self):
        for key, value in [("exitCode", 1), ("journalUnchanged", False), ("candidateSha256", "b" * 64)]:
            with self.subTest(key=key):
                saved = self.dry_run.copy()
                self.dry_run[key] = value
                self.save()
                with self.assertRaises(ValueError):
                    promotion.gates(self.manifest)
                self.dry_run = saved

    def test_receipt_without_review_or_terminal_success_refuses(self):
        for key in ["accepted", "terminalSucceeded"]:
            self.review[key] = False
            self.save()
            with self.assertRaises(ValueError):
                promotion.gates(self.manifest)
            self.review[key] = True

    def test_changed_runtime_refuses(self):
        (self.root / "runtime.rs").write_text("changed", encoding="utf-8")
        with self.assertRaises(ValueError):
            promotion.gates(self.manifest)

    def test_empty_reviewed_runtime_set_refuses(self):
        self.review["runtimeSources"] = {}
        self.save()
        with self.assertRaisesRegex(ValueError, "reviewed_runtime_set_incomplete"):
            promotion.gates(self.manifest)

    def test_unreviewed_configuration_helper_refuses(self):
        with patch.object(promotion, "HELPER_HASH", "0" * 64):
            with self.assertRaisesRegex(ValueError, "reviewed_configuration_helper_changed"):
                promotion.updated_configuration(b"", {})


if __name__ == "__main__":
    unittest.main()
