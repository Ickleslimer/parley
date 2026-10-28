import hashlib
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

from audit_junction_consumers import database, document, references, scan_strings, summaries


class ConsumerAuditTests(unittest.TestCase):
    aliases = {r"D:\Workplaces\Parley": r"D:\Workplaces\R3 Bio\Parley"}

    def test_case_slash_and_extended_paths(self):
        for text in (r"\\?\d:\workplaces\parley\bin", "D:/Workplaces/Parley/bin"):
            self.assertEqual(references(text, self.aliases), list(self.aliases))

    def test_similar_prefix_is_not_a_match(self):
        self.assertEqual(references(r"D:\Workplaces\Parley-root", self.aliases), [])

    def test_embedded_commands_are_detected_without_disclosing_them(self):
        secret = 'tool --cwd "D:\\Workplaces\\Parley" --token PRIVATE_TEST_VALUE'
        hits = scan_strings({"args": secret}, self.aliases)
        self.assertEqual(len(hits), 1)
        self.assertNotIn("PRIVATE_TEST_VALUE", json.dumps(hits))

    def test_dictionary_keys_and_nested_values(self):
        hits = scan_strings({r"D:\Workplaces\Parley": [r"D:\Workplaces\Parley\bin"]}, self.aliases)
        self.assertEqual([hit["kind"] for hit in hits], ["key", "string"])

    def test_json_parser_unescapes_paths(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "config.json"
            raw = json.dumps({"cwd": r"D:\Workplaces\Parley"}).encode()
            path.write_bytes(raw)
            result = document(path, self.aliases)
            self.assertEqual(len(result["hits"]), 1)
            self.assertEqual(path.read_bytes(), raw)

    def test_database_is_readonly_and_skips_message_content(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "state_5.sqlite"
            connection = sqlite3.connect(path)
            connection.execute("CREATE TABLE threads (cwd TEXT, title TEXT)")
            connection.execute("INSERT INTO threads VALUES (?,?)", (r"D:\Workplaces\Parley", "PRIVATE_TEST_VALUE"))
            connection.execute("CREATE TABLE messages (cwd TEXT)")
            connection.commit()
            connection.close()
            before = hashlib.sha256(path.read_bytes()).hexdigest()
            result = database(path, self.aliases)
            self.assertEqual(len(result["hits"]), 1)
            self.assertNotIn("PRIVATE_TEST_VALUE", json.dumps(result))
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), before)

    def test_summaries_distinguish_binding_from_history(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            directory = root / "encoded" / "uuid"
            directory.mkdir(parents=True)
            path = directory / "summary.json"
            raw = json.dumps({"info": {"id": "uuid", "cwd": "physical"},
                              "git_root_dir": r"D:\Workplaces\Parley", "session_summary": r"D:\Workplaces\Parley"}).encode()
            path.write_bytes(raw)
            result = summaries(root, self.aliases)
            self.assertEqual(len(result["hits"]), 1)
            self.assertEqual(result["hits"][0]["pointer"], "/git_root_dir")
            self.assertEqual(path.read_bytes(), raw)


if __name__ == "__main__":
    unittest.main()
