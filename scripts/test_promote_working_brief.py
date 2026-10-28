import copy
import tomllib
import unittest

from promote_working_brief import PROFILE, normalized, update_config


class PromotionTests(unittest.TestCase):
    def sample(self):
        return b'[mcp_servers.parley-widget-dev]\ncommand = "old.exe"\nargs = ["mcp"]\n[mcp_servers.parley-widget-dev.env]\nPIN = "unchanged"\n[mcp_servers.other]\ncommand = "other.exe"\n'

    def test_exact_semantic_difference_and_other_profile_preserved(self):
        content = self.sample()
        expected = copy.deepcopy(tomllib.loads(content.decode()))
        expected['mcp_servers'][PROFILE]['command'] = 'new.exe'
        expected['mcp_servers'][PROFILE]['env']['PARLEY_WORKING_BRIEF_ENABLED'] = 'true'
        updated = update_config(content, 'old.exe', 'new.exe')
        self.assertEqual(tomllib.loads(updated.decode()), expected)
        self.assertIn(b'PIN = "unchanged"', updated)

    def test_wrong_old_command_refuses(self):
        with self.assertRaises(ValueError):
            update_config(self.sample(), 'wrong.exe', 'new.exe')

    def test_existing_flag_is_not_silently_overridden(self):
        content = self.sample().replace(b'PIN =', b'PARLEY_WORKING_BRIEF_ENABLED = "false"\nPIN =')
        with self.assertRaisesRegex(ValueError, 'flag_already_present'):
            update_config(content, 'old.exe', 'new.exe')

    def test_crlf_and_quoted_path(self):
        updated = update_config(self.sample().replace(b'\n', b'\r\n'), 'old.exe', r'C:\Space here\new.exe')
        self.assertEqual(tomllib.loads(updated.decode())['mcp_servers'][PROFILE]['command'], r'C:\Space here\new.exe')

    def test_extended_spelling_normalization(self):
        self.assertEqual(normalized(r'\\?\D:\Workplaces\R3 Bio'), normalized(r'D:\Workplaces\R3 Bio'))


if __name__ == '__main__':
    unittest.main()
